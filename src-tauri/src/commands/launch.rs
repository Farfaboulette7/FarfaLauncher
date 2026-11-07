//! Bouton « Jouer » : lancement, suivi de la session et envoi du temps de jeu.
//! Événements : `game-started` puis `game-stopped`.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use time::OffsetDateTime;

use super::saves::{self, SaveChoice, SaveUpload};
use crate::error::{AppError, AppResult};
use crate::gamepad;
use crate::install::{rel_to_path, state};
use crate::install::state::DirectLaunch;
use crate::launch::{self, Launch};
use crate::pc::cod::CodMode;
use crate::pc::dosbox;
use crate::pc::proton::{self, Runner};
use crate::romm::playtime::{self, PendingSession};
use crate::state::AppState;

/// Sessions plus courtes ignorées (plantage au démarrage, fausse manip).
const MIN_SESSION_SECS: i64 = 30;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct GameStopped {
    rom_id: i64,
    duration_ms: u64,
    /// Session comptée (assez longue).
    recorded: bool,
    /// Sessions encore en attente d'envoi à RomM (serveur injoignable).
    pending: usize,
    /// Envoi de la sauvegarde après la partie.
    save: SaveUpload,
    save_error: Option<AppError>,
    /// État du jeu, quand le jeu démarré par son launcher vient d'être relevé.
    game: Option<state::InstalledGame>,
}

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

#[tauri::command]
pub fn running_games(state: State<'_, AppState>) -> Vec<i64> {
    state.running.lock().unwrap().iter().copied().collect()
}

/// Lance le jeu. `save_choice` : réponse à `save_check` (sauvegarde locale ou
/// du serveur), appliquée avant le lancement. `menu` : ouvrir le menu d'un runtime
/// partagé plutôt que le jeu.
#[tauri::command]
pub async fn play_game(
    app: AppHandle,
    state: State<'_, AppState>,
    rom_id: i64,
    save_choice: SaveChoice,
    cod: Option<CodChoice>,
    menu: Option<bool>,
) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let exe = st.executable.clone().ok_or(AppError::NoExecutable)?;
    if state.installs.lock().unwrap().contains_key(&rom_id) || state.is_busy(rom_id) {
        return Err(AppError::Busy);
    }
    // Même programme et mêmes données pour tous les jeux d'un runtime : un seul à la fois.
    state.check_family_idle(rom_id)?;
    if !state.running.lock().unwrap().insert(rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    if let Err(e) = saves::before_play(&state, rom_id, save_choice).await {
        state.running.lock().unwrap().remove(&rom_id);
        return Err(e);
    }
    let prepared = match prepare_pc(&state, &st, cod).await {
        Ok(p) => p,
        Err(e) => {
            state.running.lock().unwrap().remove(&rom_id);
            return Err(e);
        }
    };
    // Jeu déjà relevé derrière le launcher : lancé directement.
    let open_menu = menu == Some(true) && st.shared.is_some();
    // Émulateur : arguments relus sur le catalogue.
    let refreshed = state.catalog().emulator_for_install(&st).and_then(|e| crate::install::emulator::refreshed_launch(e, &st));
    let direct = refreshed.as_ref().or(st.direct_launch.as_ref()).filter(|d| !open_menu && !st.prefer_launcher && st.game_dir.join(rel_to_path(&d.exe)).exists());
    // Recomp ou émulateur : français par défaut, posé avant le lancement.
    let mut env: Vec<(String, String)> = direct.map(|d| d.env.clone()).unwrap_or_default();
    if prepared.is_none() {
        let program = st.game_dir.join(rel_to_path(direct.map(|d| d.exe.as_str()).unwrap_or(&exe)));
        let exe_dir = program.parent().unwrap_or(&st.game_dir).to_path_buf();
        let rules = state.catalog().language_for_install(&st);
        if !rules.is_empty() {
            env.extend(crate::language::prepare(&st.game_dir, &exe_dir, &rules));
        }
        // Manette de chaque joueur, posée avant que le jeu ne prenne les manettes.
        if let Some(format) = state.catalog().controllers_for_install(&st) {
            if let Err(e) = crate::controllers::apply(&format, &st.players, &gamepad::connected(), &exe_dir) {
                tracing::warn!("manettes non attribuées: {e}");
            }
        }
    }
    let spec = match (&prepared, direct) {
        (Some(p), _) => Launch { exe: p.exe.clone(), args: &p.args, env: &[], cwd: p.cwd.clone(), learn: false, runner: p.runner.as_ref() },
        (None, Some(d)) => Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &env, cwd: d.cwd.as_deref().map(rel_to_path), learn: false, runner: None },
        // Jeu d'un runtime : son lancement est connu, le menu de la recomp n'a rien à apprendre.
        (None, None) => Launch { exe: rel_to_path(&exe), args: &[], env: &env, cwd: None, learn: st.shared.is_none(), runner: None },
    };
    let running = match launch::spawn(&st.game_dir, spec) {
        Ok(r) => r,
        Err(e) => {
            state.running.lock().unwrap().remove(&rom_id);
            return Err(e);
        }
    };
    let started = OffsetDateTime::now_utc();
    let _ = app.emit("game-started", rom_id);
    gamepad::game_started();
    let ended_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::dialogs::watch(state.catalog().dialogs_for_install(&st), ended_flag.clone());

    tauri::async_runtime::spawn(async move {
        let shown = app.clone();
        let learned = tauri::async_runtime::spawn_blocking(move || {
            running.wait(move || {
                let _ = shown.emit("game-window", rom_id);
            })
        })
        .await
        .ok()
        .flatten();
        let ended = OffsetDateTime::now_utc();
        ended_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let state = app.state::<AppState>();
        state.running.lock().unwrap().remove(&rom_id);
        gamepad::game_stopped();
        // Retour au launcher, pilotable à la manette sans clic pour reprendre le focus.
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        let game = learned.and_then(|d| remember_direct(&state, rom_id, d));

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
        let (save, save_error) = saves::after_play(&state, state.client().ok(), rom_id, std::time::SystemTime::from(started)).await;
        // Dossier des sauvegardes peut-être trouvé à la sortie du jeu : état à jour.
        let game = game.or_else(|| state::find(&state.config_dir, rom_id));
        let _ = app.emit("game-stopped", GameStopped { rom_id, duration_ms: session.duration_ms, recorded, pending, save, save_error, game });
    });
    Ok(())
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

/// Passer par le launcher du jeu (`true`) ou lancer directement le jeu relevé.
#[tauri::command]
pub fn set_prefer_launcher(state: State<'_, AppState>, rom_id: i64, prefer: bool) -> AppResult<state::InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    st.prefer_launcher = prefer;
    state::write_state(&state.config_dir, &st)?;
    Ok(st)
}

/// Renvoie à RomM les sessions restées en attente (appelé au démarrage).
#[tauri::command]
pub async fn flush_play_sessions(state: State<'_, AppState>) -> AppResult<usize> {
    if playtime::pending_count(&state.config_dir) == 0 {
        return Ok(0);
    }
    playtime::flush(&state.client()?, &state.config_dir).await
}
