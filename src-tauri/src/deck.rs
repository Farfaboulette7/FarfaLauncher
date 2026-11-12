//! FarfaLauncherDeck : le moteur de FarfaLauncher sans interface, pour le
//! Steam Deck. Piloté par le plugin NieRDeck (onglet Catalogue) et lancé par
//! Steam (raccourci « jouer <id> » de chaque jeu installé).
//!
//! Chaque commande écrit des lignes JSON sur la sortie standard :
//! `{"type":"progress",...}` pendant une installation, puis une seule ligne
//! `{"type":"result",...}` ou `{"type":"error","code":...,"message":...}`.
//! Les journaux vont dans `~/.config/farfalauncherdeck/logs`.
//!
//! Le jeton RomM est lu sur l'entrée standard par `config` et gardé dans
//! `~/.config/farfalauncherdeck/token` (lisible par l'utilisateur seul) : il
//! n'apparaît jamais dans les arguments, la sortie ni les journaux.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::install::{self, detect, path_to_rel, state, InstallJob, InstallOutcome};
use crate::platform::{self, Target};
use crate::romm::client::{normalize_server_url, RommClient};
use crate::romm::library::{self, Game};
use crate::save_sync::{SaveCheck, SaveChoice};
use crate::state::AppState;

const USAGE: &str = "FarfaLauncherDeck — moteur de FarfaLauncher pour le Steam Deck

Commandes (réponses en JSON, une ligne par message) :
  info                          version, serveur, connexion, jeux installés
  config --serveur <url> [--gestionnaire <url>] [--dossier <chemin>] [--jeton]
                                règle le serveur ; --jeton lit le jeton sur l'entrée standard
  deconnexion                   oublie le jeton
  consoles                      consoles du serveur jouables sur le Deck
  jeux <console> [<début>]      jeux d'une console (par 100)
  recherche <texte>             recherche dans tout le serveur
  installes                     jeux installés
  jeu <id>                      fiche complète d'un jeu
  image <url> [<url>...]        télécharge des images du serveur en cache, renvoie leurs chemins
  installer <id> [--emulateur <id>] [--recomp]
  executables <id>              exécutables possibles d'un jeu installé
  choisir <id> <exécutable>     termine une installation qui attend ce choix
  desinstaller <id> [--garder-sauvegardes]
  jouer <id>                    sauvegarde du serveur, réglages, jeu, envoi de la sauvegarde
  synchroniser [<id>]           envoie les sauvegardes changées et le temps de jeu en attente";

/// Fermeture demandée du jeu en cours (`jouer`), levée à la réception du signal d'arrêt de Steam.
static GAME_STOP: std::sync::OnceLock<Arc<std::sync::atomic::AtomicBool>> = std::sync::OnceLock::new();

/// Taille des pages de `jeux`.
const PAGE: usize = 100;

/// Point d'entrée du programme `farfalauncherdeck`.
pub fn main() {
    // Lancé par Steam, la sortie d'erreur est perdue : une panique va aussi dans le journal.
    std::panic::set_hook(Box::new(|info| {
        let place = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        tracing::error!("panique à {place} : {msg}\n{}", std::backtrace::Backtrace::force_capture());
        emit(json!({ "type": "error", "code": "internal", "message": format!("panique à {place} : {msg}") }));
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Journal écrit par un fil à part : sa garde le vide à la fin du programme.
    let log_guard = crate::logging::init(&config_dir().join("logs"));
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            emit(json!({ "type": "error", "code": "internal", "message": e.to_string() }));
            std::process::exit(1);
        }
    };
    let code = runtime.block_on(async move {
        // « Quitter le jeu » de Steam envoie le signal d'arrêt à tout le groupe : le
        // jeu se ferme, et ce programme doit survivre pour envoyer la sauvegarde.
        #[cfg(unix)]
        if args.first().is_some_and(|c| c == "jouer") {
            use tokio::signal::unix::{signal, SignalKind};
            for kind in [SignalKind::terminate(), SignalKind::interrupt(), SignalKind::hangup()] {
                if let Ok(mut s) = signal(kind) {
                    tokio::spawn(async move {
                        while s.recv().await.is_some() {
                            tracing::info!("arrêt demandé ({kind:?}) : fermeture du jeu, puis envoi de la sauvegarde");
                            // Le jeu tourne dans son propre groupe : on lui transmet la demande.
                            if let Some(stop) = GAME_STOP.get() {
                                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    });
                }
            }
        }
        match run(args).await {
            Ok(result) => {
                emit(json!({ "type": "result", "data": result }));
                0
            }
            Err(e) => {
                emit(json!({ "type": "error", "code": e.code(), "message": e.to_string() }));
                1
            }
        }
    });
    drop(runtime);
    drop(log_guard);
    std::process::exit(code);
}

/// Une ligne JSON sur la sortie standard, envoyée tout de suite.
fn emit(v: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn config_dir() -> PathBuf {
    match std::env::var_os("FARFALAUNCHERDECK_CONFIG") {
        Some(d) => PathBuf::from(d),
        None => home().join(".config").join("farfalauncherdeck"),
    }
}

fn token_file(config_dir: &Path) -> PathBuf {
    config_dir.join("token")
}

fn load_token(config_dir: &Path) -> Option<String> {
    std::fs::read_to_string(token_file(config_dir)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Écrit le jeton, lisible par l'utilisateur seul.
fn store_token(config_dir: &Path, token: &str) -> AppResult<()> {
    std::fs::create_dir_all(config_dir).map_err(|_| AppError::Internal)?;
    let path = token_file(config_dir);
    let tmp = path.with_extension("tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(|_| AppError::Internal)?;
        f.write_all(token.as_bytes()).map_err(|_| AppError::Internal)?;
    }
    std::fs::rename(&tmp, &path).map_err(|_| AppError::Internal)
}

/// État du moteur, comme au démarrage du launcher, avec le client connecté.
fn open_state() -> AppState {
    let config_dir = config_dir();
    let log_dir = config_dir.join("logs");
    let cache_dir = config_dir.join("cache");
    crate::saves::set_prefixes_dir(crate::pc::proton::prefixes_dir(&config_dir));
    let default_root = home().join("FarfaLauncher").join("Jeux");
    let state = AppState::new(config_dir.clone(), log_dir, cache_dir, default_root);
    if let (Some(url), Some(token)) = (state.settings().server_url, load_token(&config_dir)) {
        match RommClient::new(&url, Some(token)) {
            Ok(client) => state.set_client(Some(client)),
            Err(e) => tracing::error!("client RomM impossible: {}", e.code()),
        }
    }
    state
}

/// Catalogue de FarfaRomManager et recomps partagées, relus sur le serveur
/// (gardés tels quels s'il ne répond pas).
async fn refresh_catalog(state: &AppState) {
    crate::runtimes::refresh(state).await;
    crate::manager::refresh(state).await;
    crate::runtimes::combine(state);
}

fn rom_id(arg: Option<&String>) -> AppResult<i64> {
    arg.and_then(|a| a.parse().ok()).ok_or(AppError::Internal)
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1))
}

async fn run(args: Vec<String>) -> AppResult<Value> {
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return Err(AppError::Internal);
    };
    let rest = &args[1..];
    match cmd {
        "aide" | "--help" | "-h" => {
            eprintln!("{USAGE}");
            Ok(Value::Null)
        }
        "version" | "--version" => Ok(json!(env!("CARGO_PKG_VERSION"))),
        "info" => info(&open_state()).await,
        "config" => config(&open_state(), rest).await,
        "deconnexion" => {
            let state = open_state();
            let _ = std::fs::remove_file(token_file(&state.config_dir));
            Ok(json!({ "connected": false }))
        }
        "consoles" => consoles(&open_state()).await,
        "jeux" => {
            let state = open_state();
            let console = rom_id(rest.first())?;
            let offset = rest.get(1).and_then(|o| o.parse().ok()).unwrap_or(0);
            games_page(&state, console, offset).await
        }
        "recherche" => {
            let state = open_state();
            let term = rest.join(" ");
            let client = state.client()?;
            refresh_catalog(&state).await;
            let games = library::search_games(&client, &term, target(), 50).await?;
            Ok(json!({ "games": summaries(&state, games) }))
        }
        "installes" => {
            let state = open_state();
            Ok(json!({ "games": state::list_installed(&state.config_dir) }))
        }
        "jeu" => game(&open_state(), rom_id(rest.first())?).await,
        "image" => images(&open_state(), rest).await,
        "installer" => install_game(&open_state(), rom_id(rest.first())?, option(rest, "--emulateur").cloned(), flag(rest, "--recomp")).await,
        "executables" => {
            let state = open_state();
            let st = state::find(&state.config_dir, rom_id(rest.first())?).ok_or(AppError::NotInstalled)?;
            Ok(json!({ "candidates": candidates(&st) }))
        }
        "choisir" => choose(&open_state(), rom_id(rest.first())?, rest.get(1).cloned().ok_or(AppError::NoExecutable)?).await,
        "desinstaller" => {
            let state = open_state();
            let id = rom_id(rest.first())?;
            let _lock = lock(&state, id)?;
            let st = state::find(&state.config_dir, id).ok_or(AppError::NotInstalled)?;
            install::uninstall(&state.config_dir, st, flag(rest, "--garder-sauvegardes")).await?;
            Ok(json!({ "romId": id }))
        }
        "jouer" => play(&open_state(), rom_id(rest.first())?).await,
        "synchroniser" => sync(&open_state(), rest.first().and_then(|a| a.parse().ok())).await,
        _ => {
            eprintln!("{USAGE}");
            Err(AppError::Internal)
        }
    }
}

/// Le Deck : SteamOS (builds Linux acceptées).
fn target() -> Target {
    platform::detect()
}

async fn info(state: &AppState) -> AppResult<Value> {
    let settings = state.settings();
    let (connected, username) = match state.client() {
        Ok(client) => match crate::romm::auth::me(&client).await {
            Ok(me) => (json!(true), Some(me.username)),
            Err(AppError::ServerUnreachable | AppError::Server) => (json!("offline"), None),
            Err(_) => (json!(false), None),
        },
        Err(_) => (json!(false), None),
    };
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "target": target(),
        "serverUrl": settings.server_url,
        "managerUrl": crate::manager::base_url(&settings),
        "installDir": state.install_root(),
        "connected": connected,
        "username": username,
        "installed": state::list_installed(&state.config_dir).len(),
        "pendingSessions": crate::romm::playtime::pending_count(&state.config_dir),
    }))
}

async fn config(state: &AppState, args: &[String]) -> AppResult<Value> {
    let server = option(args, "--serveur").map(|u| normalize_server_url(u)).transpose()?;
    let manager = option(args, "--gestionnaire").map(|u| normalize_server_url(u)).transpose()?;
    let dir = option(args, "--dossier").map(PathBuf::from);
    if let Some(d) = &dir {
        if !d.is_absolute() {
            return Err(AppError::Internal);
        }
    }
    state.update_settings(|s| {
        if let Some(url) = &server {
            s.server_url = Some(url.clone());
        }
        if manager.is_some() {
            s.manager_url = manager.clone();
        }
        if dir.is_some() {
            s.install_dir = dir.clone();
        }
    })?;
    if flag(args, "--jeton") {
        let mut token = String::new();
        std::io::stdin().read_line(&mut token).map_err(|_| AppError::Internal)?;
        let token = token.trim();
        let url = state.server_url()?;
        let client = RommClient::new(&url, Some(token.to_string()))?;
        // Vérifié avant d'être gardé.
        crate::romm::auth::me(&client).await?;
        store_token(&state.config_dir, token)?;
        state.set_client(Some(client));
    }
    info(state).await
}

/// Consoles où le Deck peut jouer : recomps (`portage-*`), jeux PC et consoles avec un émulateur.
async fn consoles(state: &AppState) -> AppResult<Value> {
    let client = state.client()?;
    refresh_catalog(state).await;
    let catalog = state.catalog();
    let me = target();
    let mut platforms = library::list_platforms(&client).await?;
    for p in platforms.iter_mut() {
        p.has_emulator = catalog.offer(&p.slug, me).is_some_and(|o| o.available) || catalog.offer(&p.fs_slug, me).is_some_and(|o| o.available);
    }
    platforms.retain(|p| p.rom_count > 0 && (p.has_emulator || library::is_portage(&p.fs_slug) || crate::pc::is_pc_folder(&p.fs_slug)));
    // Les recomps sont rangées dans leur vraie console (souvent sans émulateur pour le
    // Deck) : regroupées dans une console à part, en tête.
    let recomps = deck_recomps(state).len();
    let mut list = vec![json!({ "id": RECOMPS, "slug": "recomps", "fsSlug": "recomps", "name": "Recomps", "romCount": recomps, "iconUrls": [], "hasEmulator": false })];
    list.extend(platforms.iter().map(|p| json!(p)));
    Ok(json!({ "consoles": list }))
}

/// Console virtuelle des recomps (`jeux -1`).
const RECOMPS: i64 = -1;

/// Jeux dont la recomp a une build pour le Deck.
fn deck_recomps(state: &AppState) -> Vec<i64> {
    let me = target();
    let mut ids: Vec<i64> = state
        .catalog()
        .recomps
        .iter()
        .filter(|r| r.archives(me).1.is_some())
        .flat_map(|r| r.games.iter().filter_map(|g| g.rom_id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

async fn games_page(state: &AppState, console: i64, offset: usize) -> AppResult<Value> {
    let client = state.client()?;
    refresh_catalog(state).await;
    if console == RECOMPS {
        let mut games = library::list_games_by_ids(&client, &deck_recomps(state), target()).await;
        games.sort_by_key(|g| g.name.to_lowercase());
        return Ok(json!({ "games": summaries(state, games), "more": false, "offset": 0 }));
    }
    let (games, more) = library::list_games_page(&client, &[console], target(), offset, PAGE).await?;
    Ok(json!({ "games": summaries(state, games), "more": more, "offset": offset }))
}

/// Ce que la liste affiche d'un jeu, avec son état d'installation.
fn summaries(state: &AppState, mut games: Vec<Game>) -> Vec<Value> {
    let catalog = state.catalog();
    let manifest = state.manifest();
    let recomps = catalog.recomp_rom_ids();
    let installed: HashSet<i64> = state::list_installed(&state.config_dir).iter().map(|g| g.rom_id).collect();
    games.retain(|g| !manifest.is_runtime(g.id));
    games
        .iter_mut()
        .map(|g| {
            if let Some((runtime, def)) = manifest.page(g.id) {
                g.runtime = Some(crate::runtimes::link(runtime, def));
            }
            crate::runtimes::annotate(g, &catalog, &recomps, target());
            json!({
                "id": g.id,
                "name": g.name,
                "platformId": g.platform_id,
                "console": g.console,
                "coverUrl": g.cover_url,
                "releaseYear": g.release_year,
                "recomp": g.recomp,
                "pc": g.pc.is_some(),
                "emulator": g.emulator.as_ref().filter(|e| e.available).map(|e| &e.name),
                "installed": installed.contains(&g.id),
            })
        })
        .collect()
}

async fn game(state: &AppState, id: i64) -> AppResult<Value> {
    let client = state.client()?;
    refresh_catalog(state).await;
    let catalog = state.catalog();
    let mut game = crate::runtimes::fetch_details(&client, &state.manifest(), &catalog, id, target()).await?;
    crate::runtimes::annotate(&mut game, &catalog, &catalog.recomp_rom_ids(), target());
    let installed = state::find(&state.config_dir, id);
    // Recomp (ou jeu PC) pour le Deck, sinon émulateur de la console.
    let recomp_ok = game.my_archive.is_some() || game.runtime.is_some() || game.pc.is_some();
    let emulator_ok = game.emulator.as_ref().is_some_and(|e| e.available);
    Ok(json!({
        "game": game,
        "installed": installed,
        "canInstall": recomp_ok || emulator_ok,
        "installWith": if recomp_ok { "recomp" } else if emulator_ok { "emulator" } else { "none" },
    }))
}

/// Images du serveur (jaquettes, captures…) téléchargées dans le cache du programme.
async fn images(state: &AppState, urls: &[String]) -> AppResult<Value> {
    let client = state.client()?;
    let dir = state.cache_dir.join("images");
    let prefix = format!("{}/assets/", client.base());
    let mut out = serde_json::Map::new();
    for url in urls {
        // N'accepte que des ressources statiques du serveur configuré.
        let Some(rest) = url.strip_prefix(&prefix) else {
            out.insert(url.clone(), Value::Null);
            continue;
        };
        let path = crate::cache::cached_download(&client, &dir, url, &format!("assets/{rest}"), None).await.ok();
        out.insert(url.clone(), json!(path));
    }
    Ok(Value::Object(out))
}

/// Verrou d'un jeu entre les programmes lancés en même temps (installation et partie).
fn lock(state: &AppState, rom_id: i64) -> AppResult<std::fs::File> {
    let dir = state.config_dir.join("locks");
    std::fs::create_dir_all(&dir).map_err(|_| AppError::Internal)?;
    let file = std::fs::File::create(dir.join(format!("{rom_id}.lock"))).map_err(|_| AppError::Internal)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(_) => Err(AppError::Busy),
    }
}

async fn install_game(state: &AppState, id: i64, emulator: Option<String>, force_recomp: bool) -> AppResult<Value> {
    let _lock = lock(state, id)?;
    let client = state.client()?;
    refresh_catalog(state).await;
    let manifest = state.manifest();
    let catalog = state.catalog();
    // Comme le launcher : la recomp si le jeu en a une pour ce système, sinon l'émulateur.
    let with_emulator = if emulator.is_some() {
        true
    } else if force_recomp || manifest.page(id).is_some() {
        false
    } else {
        let game = crate::runtimes::fetch_details(&client, &manifest, &catalog, id, target()).await?;
        game.my_archive.is_none() && game.pc.is_none()
    };
    let job = InstallJob {
        client,
        config_dir: state.config_dir.clone(),
        install_root: state.install_root(),
        target: target(),
        cancel: CancellationToken::new(),
        proton: None,
        manifest,
        catalog,
        manager_base: crate::manager::base_url(&state.settings()),
        with_emulator,
        emulator_id: emulator,
        runtime_lock: state.runtime_lock.clone(),
        on_progress: Arc::new(|p| emit(json!({ "type": "progress", "data": p }))),
    };
    match job.run(id).await? {
        InstallOutcome::Installed { game } => Ok(json!({ "status": "installed", "game": game })),
        InstallOutcome::NeedsChoice { game } => {
            let candidates = candidates(&game);
            Ok(json!({ "status": "needsChoice", "game": game, "candidates": candidates }))
        }
    }
}

/// Système des exécutables d'un jeu : un jeu Windows sous Proton a des `.exe`.
fn exe_target(st: &state::InstalledGame) -> Target {
    match &st.pc {
        Some(pc) if pc.os == crate::pc::PcOs::Windows => Target::Windows,
        _ => target(),
    }
}

fn candidates(st: &state::InstalledGame) -> Vec<String> {
    detect::find_candidates(&st.game_dir, exe_target(st)).iter().map(|p| path_to_rel(p)).collect()
}

async fn choose(state: &AppState, id: i64, executable: String) -> AppResult<Value> {
    let st = state::find(&state.config_dir, id).ok_or(AppError::NotInstalled)?;
    if !candidates(&st).contains(&executable) {
        return Err(AppError::NoExecutable);
    }
    let game = if st.executable.is_none() {
        let details = crate::runtimes::fetch_details(&state.client()?, &state.manifest(), &state.catalog(), id, target()).await?;
        install::finalize(&state.config_dir, st, &executable, &details).await?
    } else {
        install::change_executable(&state.config_dir, st, &executable).await?
    };
    Ok(json!({ "game": game }))
}

/// Pas de question possible pendant un lancement depuis Steam : en cas de
/// différence, la sauvegarde la plus récente l'emporte (l'autre reste en
/// copie de secours : sur le serveur, ou dans `.farfalauncher/backups`).
fn save_choice(check: &SaveCheck) -> SaveChoice {
    match check {
        SaveCheck::Conflict { local_modified, server_modified } => {
            let server = time::OffsetDateTime::parse(server_modified, &time::format_description::well_known::Rfc3339).ok().map(|d| d.unix_timestamp());
            match (local_modified, server) {
                (None, _) => SaveChoice::Server,
                (Some(local), Some(server)) if server > *local as i64 => SaveChoice::Server,
                _ => SaveChoice::Local,
            }
        }
        _ => SaveChoice::Unchanged,
    }
}

async fn play(state: &AppState, id: i64) -> AppResult<Value> {
    let _lock = lock(state, id)?;
    let st = state::find(&state.config_dir, id).ok_or(AppError::NotInstalled)?;
    st.executable.as_ref().ok_or(AppError::NoExecutable)?;
    // Réglages du catalogue à jour (langue, optimisations) ; hors ligne, ceux du cache.
    if state.client().is_ok() {
        refresh_catalog(state).await;
    }
    let check = match crate::save_sync::check(state, id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("« {} » : sauvegardes non comparées ({}), on joue avec la locale", st.name, e.code());
            SaveCheck::Ready
        }
    };
    let choice = save_choice(&check);
    tracing::info!("« {} » : lancement depuis le Deck (sauvegarde : {choice:?})", st.name);
    #[cfg(not(feature = "gui"))]
    let pads = crate::gamepad::snapshot();
    // Compilé avec le launcher : ses manettes sont déjà lues.
    #[cfg(feature = "gui")]
    let pads = crate::gamepad::connected();
    let started = crate::play::start(state, id, choice, None, false, &pads).await?;
    emit(json!({ "type": "started", "data": { "romId": id, "saveChoice": format!("{choice:?}") } }));
    let crate::play::Started { running, started, shader_job, .. } = started;
    let _ = GAME_STOP.set(running.stop_flag());
    let (learned, crashed) = tokio::task::spawn_blocking(move || running.wait_outcome(|| {})).await.unwrap_or((None, false));
    let finished = crate::play::finish(state, id, started, learned, shader_job).await;
    Ok(json!({ "finished": finished, "crashed": crashed }))
}

/// Envoie les sauvegardes changées depuis le dernier échange (partie interrompue
/// par Steam avant l'envoi…) et le temps de jeu en attente. Appelé par NieRDeck
/// à la fermeture d'un jeu.
async fn sync(state: &AppState, only: Option<i64>) -> AppResult<Value> {
    let client = state.client()?;
    let since = std::time::SystemTime::now() - std::time::Duration::from_secs(6 * 3600);
    let mut out = Vec::new();
    for st in state::list_installed(&state.config_dir) {
        if only.is_some_and(|id| id != st.rom_id) || st.saves.dir.is_none() || st.saves.off {
            continue;
        }
        // Jeu en cours (son `jouer` tient le verrou) : il enverra lui-même.
        let Ok(_lock) = lock(state, st.rom_id) else { continue };
        let (save, error) = crate::save_sync::after_play(state, Some(client.clone()), st.rom_id, since).await;
        out.push(json!({ "romId": st.rom_id, "save": save, "error": error }));
    }
    let pending = crate::romm::playtime::flush(&client, &state.config_dir).await.unwrap_or_else(|_| crate::romm::playtime::pending_count(&state.config_dir));
    Ok(json!({ "games": out, "pendingSessions": pending }))
}
