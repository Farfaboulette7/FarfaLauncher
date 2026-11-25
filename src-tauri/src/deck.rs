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
  installer <id> [--emulateur <id>] [--recomp [<id>]]
                                avec l'émulateur ou la recomp choisis (sinon ceux conseillés) ;
                                la façon de jouer d'avant reste installée, en réserve
  utiliser <id> recomp|emulator <id>
                                joue avec une autre façon déjà installée, sans retélécharger
  executables <id>              exécutables possibles d'un jeu installé
  choisir <id> <exécutable>     termine une installation qui attend ce choix
  fiche <id> [<console>]        ce que l'onglet Informations du launcher montre en plus de `jeu` :
                                temps de jeu, nouveautés, classement, shaders partagés, page RomM
  langue <id> [fr|en]         langue du jeu (Black Ops III, recomps qui proposent l'anglais)
  historique <id>               versions précédentes de la sauvegarde
  restaurer <id> <version>      remet une version (l'actuelle est gardée dans l'historique)
  desinstaller <id> [--garder-sauvegardes]
  jouer <id>                    sauvegarde du serveur, réglages, jeu, envoi de la sauvegarde
  synchroniser [<id>]           envoie les sauvegardes changées et le temps de jeu en attente
  essai <id> [<secondes>]       smoke test : lance le jeu sans rien envoyer au serveur, vérifie
                                qu'il tient <secondes> (10 par défaut), puis le ferme

Variable FARFALAUNCHERDECK_JETON : jeton RomM pris là plutôt que dans le fichier `token`
(smoke test : le jeton n'est jamais écrit sur le disque).";

/// Fermeture demandée du jeu en cours (`jouer`), levée à la réception du signal d'arrêt de Steam.
static GAME_STOP: std::sync::OnceLock<Arc<std::sync::atomic::AtomicBool>> = std::sync::OnceLock::new();

/// Annulation de l'installation en cours (`installer`), levée par le signal d'arrêt que
/// NieRDeck envoie quand le joueur annule : le téléchargement s'arrête proprement.
static INSTALL_CANCEL: std::sync::OnceLock<CancellationToken> = std::sync::OnceLock::new();

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
    // Avant tout autre fil : retirer la variable de l'environnement est alors sans risque.
    take_env_token();
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
        if args.first().is_some_and(|c| c == "installer") {
            // Prêt dès le début : une annulation avant le premier téléchargement compte aussi.
            let _ = INSTALL_CANCEL.set(CancellationToken::new());
        }
        #[cfg(unix)]
        if args.first().is_some_and(|c| c == "jouer" || c == "installer") {
            use tokio::signal::unix::{signal, SignalKind};
            for kind in [SignalKind::terminate(), SignalKind::interrupt(), SignalKind::hangup()] {
                if let Ok(mut s) = signal(kind) {
                    tokio::spawn(async move {
                        while s.recv().await.is_some() {
                            if let Some(cancel) = INSTALL_CANCEL.get() {
                                tracing::info!("arrêt demandé ({kind:?}) : installation annulée");
                                cancel.cancel();
                                continue;
                            }
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

/// Variable du jeton pour le smoke test (jamais écrit sur le disque).
const TOKEN_VAR: &str = "FARFALAUNCHERDECK_JETON";

/// Jeton lu dans `TOKEN_VAR` au démarrage, puis retiré de l'environnement : les jeux,
/// émulateurs et outils lancés ensuite n'en héritent pas.
static ENV_TOKEN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn take_env_token() {
    let token = std::env::var(TOKEN_VAR).ok();
    std::env::remove_var(TOKEN_VAR);
    let _ = ENV_TOKEN.set(token);
}

fn load_token(config_dir: &Path) -> Option<String> {
    pick_token(ENV_TOKEN.get().cloned().flatten(), config_dir)
}

/// Le jeton de l'environnement d'abord (smoke test), sinon celui du fichier `token`.
fn pick_token(env: Option<String>, config_dir: &Path) -> Option<String> {
    let clean = |t: String| Some(t.trim().to_string()).filter(|t| !t.is_empty());
    env.and_then(clean).or_else(|| std::fs::read_to_string(token_file(config_dir)).ok().and_then(clean))
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
        "installer" => {
            // `--recomp` seul force la recomp ; suivi d'un identifiant, il la choisit.
            let recomp = option(rest, "--recomp").filter(|v| !v.starts_with("--")).cloned();
            install_game(&open_state(), rom_id(rest.first())?, option(rest, "--emulateur").cloned(), flag(rest, "--recomp"), recomp).await
        }
        "utiliser" => {
            let state = open_state();
            let id = rom_id(rest.first())?;
            let kind = rest.get(1).filter(|k| matches!(k.as_str(), "recomp" | "emulator")).ok_or(AppError::Internal)?;
            let way = rest.get(2).ok_or(AppError::Internal)?;
            use_install(&state, id, kind, way)
        }
        "fiche" => sheet(&open_state(), rom_id(rest.first())?, rest.get(1).cloned().unwrap_or_default()).await,
        "langue" => language(&open_state(), rom_id(rest.first())?, rest.get(1).map(String::as_str)).await,
        "historique" => {
            let state = open_state();
            Ok(json!({ "versions": crate::save_sync::history(&state, rom_id(rest.first())?).await? }))
        }
        "restaurer" => {
            let state = open_state();
            let id = rom_id(rest.first())?;
            let version = rest.get(1).ok_or(AppError::Internal)?;
            // Pas pendant une partie : son `jouer` tient le verrou.
            let _lock = lock(&state, id)?;
            crate::save_sync::restore_version(&state, id, version).await?;
            Ok(json!({ "romId": id }))
        }
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
        // Interne : veilleur lancé par `jouer`.
        "apres" => {
            let pid = rest.first().and_then(|a| a.parse().ok()).ok_or(AppError::Internal)?;
            let id = rom_id(rest.get(1))?;
            let started = rest.get(2).and_then(|a| a.parse().ok()).ok_or(AppError::Internal)?;
            after_game(&open_state(), pid, id, started).await
        }
        "synchroniser" => sync(&open_state(), rest.first().and_then(|a| a.parse().ok())).await,
        "essai" => {
            let secs = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
            smoke_launch(&open_state(), rom_id(rest.first())?, secs).await
        }
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
        // Vérifié avant d'être gardé ; refusé s'il a les droits d'administration de RomM,
        // comme dans le launcher (faille SL11).
        let me = crate::romm::auth::me(&client).await?;
        if crate::romm::auth::has_admin_rights(&client, &me).await {
            tracing::warn!("jeton refusé : droits d'administration de RomM");
            return Err(AppError::TokenTooPowerful);
        }
        store_token(&state.config_dir, token)?;
        state.set_client(Some(client));
    } else if let (Some(url), Some(token)) = (server, load_token(&state.config_dir)) {
        // Serveur changé avec un jeton déjà connu (fichier ou FARFALAUNCHERDECK_JETON) : connecté tout de suite.
        state.set_client(Some(RommClient::new(&url, Some(token))?));
    }
    info(state).await
}

/// Consoles où le Deck peut jouer : recomps, jeux PC et consoles avec un émulateur.
async fn consoles(state: &AppState) -> AppResult<Value> {
    let client = state.client()?;
    refresh_catalog(state).await;
    let catalog = state.catalog();
    let me = target();
    let mut platforms = library::list_platforms(&client).await?;
    for p in platforms.iter_mut() {
        p.has_emulator = catalog.offer(&p.slug, me).is_some_and(|o| o.available) || catalog.offer(&p.fs_slug, me).is_some_and(|o| o.available);
    }
    platforms.retain(|p| p.rom_count > 0 && (p.has_emulator || crate::pc::is_pc_folder(&p.fs_slug)));
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

/// Garde la recomp choisie pour un jeu qui en a plusieurs (comme le launcher) : elle reste la
/// sienne même si le conseil du catalogue change.
fn keep_recomp_choice(state: &AppState, id: i64, wanted: Option<String>) -> AppResult<()> {
    let catalog = state.catalog();
    let choices = catalog.recomps_for_rom(id);
    let wanted = wanted.filter(|w| choices.iter().any(|r| &r.id == w)).or_else(|| (choices.len() > 1).then(|| catalog.recomp_for_rom(id).map(|r| r.id.clone())).flatten());
    drop(catalog);
    if let Some(w) = wanted {
        if state.settings().recomp_choice.get(&id) != Some(&w) {
            tracing::info!("recomp choisie pour le jeu {id} : {w}");
            state.update_settings(|s| {
                s.recomp_choice.insert(id, w);
            })?;
            crate::runtimes::combine(state);
        }
    }
    Ok(())
}

async fn install_game(state: &AppState, id: i64, emulator: Option<String>, force_recomp: bool, recomp: Option<String>) -> AppResult<Value> {
    let _lock = lock(state, id)?;
    let client = state.client()?;
    refresh_catalog(state).await;
    if emulator.is_none() {
        keep_recomp_choice(state, id, recomp)?;
    }
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
        cancel: INSTALL_CANCEL.get_or_init(CancellationToken::new).clone(),
        proton: None,
        manifest,
        catalog,
        manager_base: crate::manager::base_url(&state.settings()),
        with_emulator,
        emulator_id: emulator,
        runtime_lock: state.runtime_lock.clone(),
        on_progress: Arc::new(|p| emit(json!({ "type": "progress", "data": p }))),
    };
    // Dossiers des jeux avant l'installation : une annulation retire celui qu'elle vient de créer.
    let root = state.install_root();
    let before: HashSet<PathBuf> = std::fs::read_dir(&root).map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect()).unwrap_or_default();
    let result = job.run(id).await;
    if matches!(result, Err(AppError::Cancelled)) && state::find(&state.config_dir, id).is_none() {
        remove_cancelled_download(&root, &before);
    }
    match result? {
        InstallOutcome::Installed { game } => Ok(json!({ "status": "installed", "game": game })),
        InstallOutcome::NeedsChoice { game } => {
            let candidates = candidates(&game);
            Ok(json!({ "status": "needsChoice", "game": game, "candidates": candidates }))
        }
    }
}

/// Le reste de la fiche d'un jeu (onglets Informations et Nouveautés du launcher). Chaque
/// partie vaut `null` si le serveur ne la donne pas : la fiche s'affiche quand même.
async fn sheet(state: &AppState, id: i64, console: String) -> AppResult<Value> {
    let client = state.client()?;
    refresh_catalog(state).await;
    let secs = std::time::Duration::from_secs(10);
    let playtime = tokio::time::timeout(secs, crate::romm::playtime::total_ms(&client, id)).await.ok().and_then(Result::ok).flatten();
    let ranking = tokio::time::timeout(secs, crate::stats::game_ranking(state, id)).await.ok().and_then(Result::ok).flatten();
    let shaders = match crate::manager::base_url(&state.settings()) {
        Some(base) => {
            let manager = client.with_base(&base);
            let path = format!("api/shaders/rom/{id}?systeme={}", crate::language::current_system());
            tokio::time::timeout(secs, manager.get_json::<Value>(&path)).await.ok().and_then(Result::ok)
        }
        None => None,
    };
    let changelog = state.catalog().changelog(id, &console, target());
    Ok(json!({
        "playtimeMs": playtime,
        "ranking": ranking,
        "shaders": shaders,
        "changelog": changelog,
        "rommPage": format!("{}/rom/{id}", client.base().trim_end_matches('/')),
    }))
}

/// Installation annulée d'un jeu qui n'était pas installé : le dossier qu'elle a créé ne
/// contient que le téléchargement en cours (`.farfalauncher`), on le supprime pour rendre la
/// place. Un dossier avec autre chose dedans (autre installation en même temps) est gardé.
fn remove_cancelled_download(root: &Path, before: &HashSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for dir in rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir() && !before.contains(p)) {
        let only_partial = std::fs::read_dir(&dir).is_ok_and(|rd| rd.filter_map(Result::ok).all(|e| e.file_name() == crate::install::extract::INTERNAL_DIR));
        if only_partial {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => tracing::info!("installation annulée : {} supprimé", dir.display()),
                Err(e) => tracing::warn!("installation annulée : {} non supprimé ({e})", dir.display()),
            }
        }
    }
}

/// Met en service une installation en réserve (recomp ou émulateur) ; celle d'avant y passe.
fn use_install(state: &AppState, id: i64, kind: &str, way: &str) -> AppResult<Value> {
    let _lock = lock(state, id)?;
    let st = state::activate(&state.config_dir, id, kind, way)?;
    if kind == "recomp" && state.catalog().recomps_for_rom(id).iter().any(|r| r.id == way) && state.settings().recomp_choice.get(&id).map(String::as_str) != Some(way) {
        state.update_settings(|s| {
            s.recomp_choice.insert(id, way.to_string());
        })?;
        crate::runtimes::combine(state);
    }
    tracing::info!("« {} » se joue maintenant avec {kind} {way}", st.name);
    Ok(json!({ "installed": st }))
}

/// Langue d'un jeu installé : `{"language": "fr"|"en"|null, "bo3": bool}` (null : pas de
/// choix pour ce jeu). Avec `lang`, la change d'abord.
async fn language(state: &AppState, id: i64, lang: Option<&str>) -> AppResult<Value> {
    use crate::pc::cod;
    let st = state::find(&state.config_dir, id).ok_or(AppError::NotInstalled)?;
    if let Some(l) = lang {
        if !matches!(l, "fr" | "en") {
            return Err(AppError::Internal);
        }
    }
    if matches!(st.pc.as_ref().and_then(|p| p.cod.as_ref()), Some(cod::CodGame::Bo3)) {
        let dir = st.game_dir.clone();
        if let Some(l) = lang {
            let _lock = lock(state, id)?;
            let l = l.to_string();
            let d = dir.clone();
            tokio::task::spawn_blocking(move || -> AppResult<()> {
                cod::repair_bo3_zone(&d).map_err(|e| crate::error::log_err(AppError::Disk, "zone/ de Black Ops III", e))?;
                cod::set_bo3_language(&d, &l).map_err(|e| crate::error::log_err(AppError::Disk, "langue de Black Ops III", e))
            })
            .await
            .map_err(|_| AppError::Internal)??;
        }
        return Ok(json!({ "language": cod::bo3_language(&dir), "bo3": true }));
    }
    let rules = state.catalog().language_for_install(&st);
    if !crate::language::offers_english(&rules) {
        return Ok(json!({ "language": null, "bo3": false }));
    }
    if let Some(l) = lang {
        let _lock = lock(state, id)?;
        crate::language::set_choice(&st.game_dir, l).map_err(|e| crate::error::log_err(AppError::Disk, "langue du jeu", e))?;
    }
    Ok(json!({ "language": crate::language::choice(&st.game_dir), "bo3": false }))
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
    spawn_watcher(id, started.unix_timestamp());
    let presence = presence_while_playing(state, id);
    let (learned, crash) = tokio::task::spawn_blocking(move || running.wait_outcome(|| {})).await.unwrap_or((None, None));
    if let Some((task, client)) = presence {
        task.abort();
        // Partie finie : le Deck n'a pas de launcher ouvert, le joueur passe hors ligne.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), client.send(client.delete("api/presence", &json!({})))).await;
    }
    let crashed = crash.is_some();
    let finished = crate::play::finish(state, id, started, learned, shader_job).await;
    // Statistiques entre amis : la session part aussi au gestionnaire (sans retenir plus de 10 s).
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), crate::stats::sync(state)).await;
    // Fin de partie traitée ici : le veilleur n'a plus que la synchronisation à vérifier.
    let _ = std::fs::write(session_marker(state, id, started.unix_timestamp()), b"");
    Ok(json!({ "finished": finished, "crashed": crashed, "crash": crash }))
}

/// Essai permis : un jeu d'émulateur sans shaders partagés (`shaders:`) ni sauvegarde à
/// compléter au lancement (`{profil}`). Écart E9 : la fiche décide, quelle que soit la console.
fn smoke_allowed(catalog: &crate::manager::Catalog, st: &state::InstalledGame) -> bool {
    st.pc.is_none() && catalog.emulator_for_install(st).is_some_and(|e| smoke_emulator(catalog, e))
}

fn smoke_emulator(catalog: &crate::manager::Catalog, e: &crate::manager::Emulator) -> bool {
    catalog.shader_rule(e).is_none() && !crate::switch_saves::has_profile_rule(e)
}

/// Smoke test (`scripts/smoke-test.ps1`) : lance un jeu installé comme `jouer`, mais sans
/// rien écrire sur le serveur.
/// - Seulement un jeu d'émulateur sans partage avec le serveur (`smoke_allowed`) : `play::start`
///   ne prépare alors ni jeu PC (client Call of Duty) ni partage des shaders
///   (`shaders::Job::before` peut envoyer au gestionnaire). Le reste est refusé.
/// - `SaveChoice::Unchanged` : ni téléchargement ni envoi de sauvegarde.
/// - Ni `play::finish` (temps de jeu, sauvegarde, shaders), ni `stats::sync`, ni la présence.
/// Le jeu doit tenir `secs` secondes, puis il est fermé (prié de se fermer, puis arrêté).
async fn smoke_launch(state: &AppState, id: i64, secs: u64) -> AppResult<Value> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let _lock = lock(state, id)?;
    let st = state::find(&state.config_dir, id).ok_or(AppError::NotInstalled)?;
    st.executable.as_ref().ok_or(AppError::NoExecutable)?;
    // Réglages du catalogue à jour (lecture seule), comme `jouer`.
    if state.client().is_ok() {
        refresh_catalog(state).await;
    }
    if !smoke_allowed(&state.catalog(), &st) {
        tracing::warn!("essai refusé pour « {} » : seulement un jeu d'émulateur sans shaders partagés ni sauvegarde à compléter (rien ne doit partir au serveur)", st.name);
        return Err(AppError::NotAvailable);
    }
    let crate::play::Started { running, shader_job, .. } = crate::play::start(state, id, SaveChoice::Unchanged, None, false, &[]).await?;
    // Impossible sans `shaders:` ; vérifié quand même : aucun partage de shaders.
    debug_assert!(shader_job.is_none());
    emit(json!({ "type": "started", "data": { "romId": id } }));
    let stop = running.stop_flag();
    let window = Arc::new(AtomicBool::new(false));
    let seen = window.clone();
    let mut game = tokio::task::spawn_blocking(move || running.wait_outcome(|| seen.store(true, Ordering::Relaxed)));
    // Fini avant le délai : le jeu s'est fermé tout seul (planté).
    let early = tokio::time::timeout(std::time::Duration::from_secs(secs), &mut game).await.ok();
    let alive = early.is_none();
    stop.store(true, Ordering::Relaxed);
    let (_, crash) = match early {
        Some(r) => r,
        None => game.await,
    }
    .unwrap_or((None, None));
    Ok(json!({ "romId": id, "alive": alive, "seconds": secs, "window": window.load(Ordering::Relaxed), "crash": crash }))
}

/// Présence des amis pendant la partie : « en jeu » toutes les 60 s (sauf « Apparaître hors
/// ligne » ou hors ligne). Si Steam arrête ce programme, le gestionnaire l'oublie après 3 min.
fn presence_while_playing(state: &AppState, id: i64) -> Option<(tokio::task::JoinHandle<()>, crate::romm::client::RommClient)> {
    if state.settings().presence_hidden {
        return None;
    }
    let client = crate::presence::manager(state).ok()?;
    let body = crate::presence::body(Some(crate::presence::game_ref(state, id)));
    let c = client.clone();
    let task = tokio::spawn(async move {
        loop {
            crate::presence::post(&c, &body).await;
            tokio::time::sleep(std::time::Duration::from_secs(crate::presence::INTERVAL_SECS)).await;
        }
    });
    Some((task, client))
}

/// Partie dont `jouer` a terminé lui-même le traitement (temps de jeu enregistré).
fn session_marker(state: &AppState, id: i64, started: i64) -> PathBuf {
    let dir = state.config_dir.join("sessions");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("{id}-{started}.done"))
}

/// En mode Jeu, « Quitter le jeu » de Steam arrête ce programme peu après le jeu,
/// parfois avant l'envoi de la sauvegarde. Un veilleur lancé hors de Steam
/// (service utilisateur systemd) attend sa fin, puis finit le travail : temps de
/// jeu et sauvegarde.
fn spawn_watcher(id: i64, started: i64) {
    #[cfg(target_os = "linux")]
    {
        let Ok(exe) = std::env::current_exe() else { return };
        let mut cmd = std::process::Command::new("systemd-run");
        cmd.args(["--user", "--quiet", "--collect", "--unit", &format!("farfalauncherdeck-{id}-{started}")]);
        if let Some(dir) = std::env::var_os("FARFALAUNCHERDECK_CONFIG") {
            cmd.arg(format!("--setenv=FARFALAUNCHERDECK_CONFIG={}", dir.to_string_lossy()));
        }
        cmd.arg("--").arg(exe).args(["apres", &std::process::id().to_string(), &id.to_string(), &started.to_string()]);
        cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        match cmd.status() {
            Ok(s) if s.success() => tracing::info!("veilleur de fin de partie lancé"),
            other => tracing::warn!("veilleur de fin de partie non lancé: {other:?}"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (id, started);
}

/// Veilleur : attend la fin du programme `pid` (la partie), puis enregistre le
/// temps de jeu s'il ne l'a pas fait et envoie la sauvegarde.
async fn after_game(state: &AppState, pid: u32, id: i64, started: i64) -> AppResult<Value> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(24 * 3600);
    while proc_dir.exists() && std::time::Instant::now() < limit {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    let marker = session_marker(state, id, started);
    if marker.exists() {
        let _ = std::fs::remove_file(&marker);
    } else if let Ok(start) = time::OffsetDateTime::from_unix_timestamp(started) {
        let end = time::OffsetDateTime::now_utc();
        tracing::info!("partie arrêtée avant sa fin normale : temps de jeu et sauvegarde repris par le veilleur");
        if (end - start).whole_seconds() >= crate::play::MIN_SESSION_SECS {
            if let Err(e) = crate::romm::playtime::enqueue(&state.config_dir, crate::romm::playtime::PendingSession::new(id, start, end)) {
                tracing::error!("session de jeu non enregistrée localement: {}", e.code());
            }
        }
    }
    sync(state, Some(id)).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_from_env_first_then_file() {
        let dir = std::env::temp_dir().join(format!("farfa-deck-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Ni variable ni fichier.
        assert_eq!(pick_token(None, &dir), None);
        std::fs::write(token_file(&dir), " rmm_fichier \n").unwrap();
        // Fichier seul, nettoyé.
        assert_eq!(pick_token(None, &dir).as_deref(), Some("rmm_fichier"));
        // La variable passe avant le fichier.
        assert_eq!(pick_token(Some("rmm_variable".into()), &dir).as_deref(), Some("rmm_variable"));
        // Variable vide : le fichier.
        assert_eq!(pick_token(Some("  ".into()), &dir).as_deref(), Some("rmm_fichier"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Écart E9 : l'essai est refusé pour un émulateur qui a `shaders:` ou une sauvegarde à
    // compléter (`{profil}`), quelle que soit sa console ; permis sinon, même sur Switch.
    #[test]
    fn smoke_test_depends_on_the_card_not_the_console() {
        let catalog: crate::manager::Catalog = serde_json::from_str(r#"{"emulators":[
            {"id":"a","name":"A","consoles":["gba"],"version":{"cle":"k","nom":"1"},"shaders":"cache/{titleid}/v.bin"},
            {"id":"b","name":"B","consoles":["wii"],"version":{"cle":"k","nom":"1"},"saves":{"dir":"nand/{profil}"}},
            {"id":"c","name":"C","consoles":["switch"],"version":{"cle":"k","nom":"1"},"saves":{"dir":"saves"},"shaders":false},
            {"id":"s","name":"S","consoles":["switch"],"version":{"cle":"k","nom":"1"},"saves":{"dir":"saves"}},
            {"id":"d","name":"D","consoles":["psx"],"version":{"cle":"k","nom":"1"},"saves":{"dir":"{romdir}"}}]}"#).unwrap();
        let ok: Vec<bool> = catalog.emulators.iter().map(|e| smoke_emulator(&catalog, e)).collect();
        // s : Switch sans `shaders:` → ancienne règle, partage, donc refusé.
        assert_eq!(ok, [false, false, true, false, true]);
    }
}
