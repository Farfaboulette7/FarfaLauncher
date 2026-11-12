//! Lancement d'un jeu installé et fin de partie, sans Tauri : partagé par le
//! bouton « Jouer » du launcher (commands/launch.rs) et par le programme du
//! Steam Deck (deck.rs).
//!
//! `start` : sauvegarde du serveur si choisie, Proton / Call of Duty, langue,
//! programmes à lancer avant, manettes, contenu Switch, réglages optimisés,
//! shaders partagés, puis lancement. `finish` : temps de jeu, envoi de la
//! sauvegarde et des shaders, jeu démarré par son launcher retenu.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::{AppError, AppResult};
use crate::gamepad::PadInfo;
use crate::install::state::DirectLaunch;
use crate::install::{rel_to_path, state};
use crate::launch::{self, Launch, RunningGame};
use crate::pc::cod::CodMode;
use crate::pc::dosbox;
use crate::pc::proton::{self, Runner};
use crate::romm::playtime::{self, PendingSession};
use crate::save_sync::{self, SaveChoice, SaveUpload};
use crate::state::AppState;

/// Sessions plus courtes ignorées (plantage au démarrage, fausse manip).
pub const MIN_SESSION_SECS: i64 = 30;

/// Mode choisi pour un Call of Duty.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodChoice {
    mode: CodMode,
    /// Plutonium : en ligne (`true`) ou en LAN.
    #[serde(default)]
    online: bool,
}

/// Lancement d'un jeu PC, préparé avant la partie.
struct PreparedPc {
    exe: std::path::PathBuf,
    args: Vec<String>,
    cwd: Option<std::path::PathBuf>,
    runner: Option<Runner>,
}

/// Nom du joueur en LAN : celui de la session de l'ordinateur.
fn player_name() -> String {
    ["USERNAME", "USER"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|n| n.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(15).collect::<String>())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Joueur".into())
}

/// Jeu PC : Proton prêt (téléchargé si besoin), client Call of Duty à jour.
async fn prepare_pc(state: &AppState, st: &state::InstalledGame, cod: Option<CodChoice>) -> AppResult<Option<PreparedPc>> {
    let Some(pc) = &st.pc else { return Ok(None) };
    let runner = match &pc.proton {
        Some(setup) => Some(proton::runner(&state.config_dir, setup, pc.steam_app_id, &tokio_util::sync::CancellationToken::new(), |_, _| {}).await?),
        None => None,
    };
    if let Some(game) = pc.cod {
        let choice = cod.ok_or(AppError::ModeRequired)?;
        let (exe, args, cwd) =
            crate::pc::install::prepare_cod(game, choice.mode, choice.online, &st.game_dir, runner.as_ref(), &player_name()).await?;
        return Ok(Some(PreparedPc { exe, args, cwd, runner }));
    }
    let exe = st.executable.as_deref().ok_or(AppError::NoExecutable)?;
    let mut args = pc.args.clone();
    if dosbox::is_dosbox(exe) {
        // Touches personnalisées : config lue en dernier par DOSBox.
        let path_for = |p: &std::path::Path| if runner.is_some() { proton::windows_path(p) } else { p.to_string_lossy().into_owned() };
        match dosbox::write(&st.game_dir, &pc.keys, path_for) {
            Ok(Some(conf)) => args.extend(["-conf".to_string(), path_for(&conf)]),
            Ok(None) => {}
            Err(e) => tracing::warn!("touches DOSBox non écrites: {e}"),
        }
    }
    Ok(Some(PreparedPc { exe: rel_to_path(exe), args, cwd: pc.cwd.as_deref().map(rel_to_path), runner }))
}

/// Jeu lancé par `start`.
pub struct Started {
    pub running: RunningGame,
    pub started: OffsetDateTime,
    pub shader_job: Option<crate::shaders::Job>,
    /// État du jeu au lancement.
    pub game: state::InstalledGame,
}

/// Prépare et lance le jeu. L'appelant a déjà vérifié qu'il n'est ni occupé ni
/// lancé, et l'a marqué en cours (`state.running`). `pads` : manettes branchées.
pub async fn start(state: &AppState, rom_id: i64, save_choice: SaveChoice, cod: Option<CodChoice>, menu: bool, pads: &[PadInfo]) -> AppResult<Started> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let exe = st.executable.clone().ok_or(AppError::NoExecutable)?;
    save_sync::before_play(state, rom_id, save_choice).await?;
    let prepared = prepare_pc(state, &st, cod).await?;
    // Jeu déjà relevé derrière le launcher : lancé directement.
    let open_menu = menu && st.shared.is_some();
    // Émulateur : arguments relus sur le catalogue.
    // Recomp à un seul jeu avec `argument:` : lancée sur sa ROM, sans son menu.
    let refreshed = state
        .catalog()
        .emulator_for_install(&st)
        .and_then(|e| crate::install::emulator::refreshed_launch(e, &st))
        .or_else(|| state.catalog().recomp_launch(&st));
    let direct = refreshed.as_ref().or(st.direct_launch.as_ref()).filter(|d| !open_menu && !st.prefer_launcher && st.game_dir.join(rel_to_path(&d.exe)).exists());
    // Recomp ou émulateur : français par défaut, posé avant le lancement.
    let mut env: Vec<(String, String)> = direct.map(|d| d.env.clone()).unwrap_or_default();
    let mut shader_job: Option<crate::shaders::Job> = None;
    if prepared.is_none() {
        let program = st.game_dir.join(rel_to_path(direct.map(|d| d.exe.as_str()).unwrap_or(&exe)));
        let exe_dir = match state.catalog().emulator_for_install(&st) {
            Some(e) => e.data_dir(&st.game_dir, direct.map(|d| d.exe.as_str()).unwrap_or(&exe)),
            None => program.parent().unwrap_or(&st.game_dir).to_path_buf(),
        };
        let rules = state.catalog().language_for_install(&st);
        if !rules.is_empty() {
            env.extend(crate::language::prepare(&st.game_dir, &exe_dir, &rules));
        }
        // Programmes de l'émulateur à lancer d'abord (ex. shaders partagés de lichii-sync).
        if let Some(e) = state.catalog().emulator_for_install(&st) {
            launch::before::clean(&e.launch.clean, &exe_dir);
        }
        let before = state.catalog().emulator_for_install(&st).map(|e| launch::before::applicable(&e.launch.before)).unwrap_or_default();
        if !before.is_empty() {
            let dir = exe_dir.clone();
            let _ = tokio::task::spawn_blocking(move || launch::before::run_all(&before, &dir)).await;
        }
        // Manette de chaque joueur, posée avant que le jeu ne prenne les manettes.
        if let Some(format) = state.catalog().controllers_for_install(&st) {
            if let Err(e) = crate::controllers::apply(&format, &st.players, pads, &exe_dir) {
                tracing::warn!("manettes non attribuées: {e}");
            }
        }
        // Citron et Eden : mises à jour et DLC du jeu lus dans son dossier.
        if let Some(e) = state.catalog().emulator_for_install(&st).filter(|e| e.consoles.iter().any(|c| c == "switch")) {
            if let Some(rom_dir) = st.shared.as_ref().and_then(|s| s.rom_dir.as_ref()) {
                match crate::switch_content::apply(&exe_dir, rom_dir) {
                    Ok(()) => tracing::info!("{} : contenu (mises à jour, DLC) lu dans {}", e.name, rom_dir.display()),
                    Err(err) => tracing::warn!("contenu du jeu non déclaré à {}: {err}", e.name),
                }
            }
        }
        // Réglages optimisés pour ce PC (FarfaRomManager), sauf si le joueur les a coupés.
        let rules = state.catalog().optimize_for_install(&st);
        if !rules.is_empty() && !state.settings().optimize_off.contains(&st.rom_id) {
            let title = crate::switch_saves::title_id(&state.catalog(), &state.config_dir, &st);
            let game = crate::optimize::GameRef { rom_id: st.rom_id, title: title.as_deref() };
            env.extend(crate::optimize::apply(&rules, crate::hardware::get(), &game, &exe_dir));
        }
        // Citron et Eden : shaders déjà vus par les autres joueurs, reçus du serveur.
        shader_job = shaders_job(state, &st, &exe_dir);
        if let Some(job) = &shader_job {
            job.before().await;
        }
    }
    let spec = match (&prepared, direct) {
        (Some(p), _) => Launch { exe: p.exe.clone(), args: &p.args, env: &[], cwd: p.cwd.clone(), learn: false, runner: p.runner.as_ref() },
        (None, Some(d)) => Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &env, cwd: d.cwd.as_deref().map(rel_to_path), learn: false, runner: None },
        // Jeu d'un runtime : son lancement est connu, le menu de la recomp n'a rien à apprendre.
        (None, None) => Launch { exe: rel_to_path(&exe), args: &[], env: &env, cwd: None, learn: st.shared.is_none(), runner: None },
    };
    let running = launch::spawn(&st.game_dir, spec)?;
    Ok(Started { running, started: OffsetDateTime::now_utc(), shader_job, game: st })
}

/// Bilan d'une partie terminée.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub duration_ms: u64,
    /// Session comptée (assez longue).
    pub recorded: bool,
    /// Sessions encore en attente d'envoi à RomM (serveur injoignable).
    pub pending: usize,
    /// Envoi de la sauvegarde après la partie.
    pub save: SaveUpload,
    pub save_error: Option<AppError>,
    /// État du jeu, quand le jeu démarré par son launcher vient d'être relevé
    /// ou que le dossier des sauvegardes a été trouvé.
    pub game: Option<state::InstalledGame>,
}

/// Fin de partie : temps de jeu envoyé à RomM, sauvegarde et shaders envoyés.
/// `learned` : vrai jeu démarré par le launcher du jeu, relevé pendant la partie.
pub async fn finish(state: &AppState, rom_id: i64, started: OffsetDateTime, learned: Option<DirectLaunch>, shader_job: Option<crate::shaders::Job>) -> Finished {
    let ended = OffsetDateTime::now_utc();
    let game = learned.and_then(|d| remember_direct(state, rom_id, d));
    let session = PendingSession::new(rom_id, started, ended);
    let recorded = (ended - started).whole_seconds() >= MIN_SESSION_SECS;
    let mut pending = 0;
    if recorded {
        if let Err(e) = playtime::enqueue(&state.config_dir, session.clone()) {
            tracing::error!("session de jeu non enregistrée localement: {}", e.code());
        }
        pending = match state.client() {
            Ok(client) => playtime::flush(&client, &state.config_dir).await.unwrap_or_else(|_| playtime::pending_count(&state.config_dir)),
            Err(_) => playtime::pending_count(&state.config_dir),
        };
    } else {
        tracing::info!("session de {} s ignorée (moins de {MIN_SESSION_SECS} s)", (ended - started).whole_seconds());
    }
    // Même pour une session courte : le joueur a pu sauvegarder.
    let (save, save_error) = save_sync::after_play(state, state.client().ok(), rom_id, std::time::SystemTime::from(started)).await;
    if let Some(job) = &shader_job {
        job.after(std::time::SystemTime::from(started)).await;
    }
    // Dossier des sauvegardes peut-être trouvé à la sortie du jeu : état à jour.
    let game = game.or_else(|| state::find(&state.config_dir, rom_id));
    Finished { duration_ms: session.duration_ms, recorded, pending, save, save_error, game }
}

/// Partage des shaders pour un jeu Switch lancé par Citron ou Eden.
fn shaders_job(state: &AppState, st: &state::InstalledGame, exe_dir: &std::path::Path) -> Option<crate::shaders::Job> {
    let emulator = state.catalog().emulator_for_install(st).filter(|e| e.consoles.iter().any(|c| c == "switch" || c == "switch-2"))?.id.clone();
    let (shader_root, cache_name) = crate::shaders::locate(exe_dir)?;
    let base = crate::manager::base_url(&state.settings())?;
    let rom_dir = st.shared.as_ref()?.rom_dir.clone()?;
    Some(crate::shaders::Job {
        manager: state.client().ok()?.with_base(&base),
        emulator,
        rom_id: st.rom_id,
        shader_root,
        cache_name,
        config_dir: state.config_dir.clone(),
        roms: st.roms.iter().map(|r| rom_dir.join(rel_to_path(r))).collect(),
    })
}

/// Retient le jeu démarré par le launcher ; renvoie l'état s'il a changé.
fn remember_direct(state: &AppState, rom_id: i64, direct: DirectLaunch) -> Option<state::InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id)?;
    if st.direct_launch.as_ref() == Some(&direct) {
        return None;
    }
    st.direct_launch = Some(direct);
    match state::write_state(&state.config_dir, &st) {
        Ok(()) => Some(st),
        Err(e) => {
            tracing::error!("lancement direct non retenu: {}", e.code());
            None
        }
    }
}
