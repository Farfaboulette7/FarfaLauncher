//! Bouton « Jouer » : lancement, suivi de la session et envoi du temps de jeu.
//! Événements : `game-started` puis `game-stopped`.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use time::OffsetDateTime;

use super::saves::{self, SaveChoice, SaveUpload};
use crate::error::{AppError, AppResult};
use crate::install::{rel_to_path, state};
use crate::install::state::DirectLaunch;
use crate::launch::{self, Launch};
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

#[tauri::command]
pub fn running_games(state: State<'_, AppState>) -> Vec<i64> {
    state.running.lock().unwrap().iter().copied().collect()
}

/// Lance le jeu. `save_choice` : réponse à `save_check` (sauvegarde locale ou
/// du serveur), appliquée avant le lancement.
#[tauri::command]
pub async fn play_game(app: AppHandle, state: State<'_, AppState>, rom_id: i64, save_choice: SaveChoice) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let exe = st.executable.clone().ok_or(AppError::NoExecutable)?;
    if state.installs.lock().unwrap().contains_key(&rom_id) {
        return Err(AppError::Busy);
    }
    if !state.running.lock().unwrap().insert(rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    if let Err(e) = saves::before_play(&state, rom_id, save_choice).await {
        state.running.lock().unwrap().remove(&rom_id);
        return Err(e);
    }
    // Jeu déjà relevé derrière le launcher : lancé directement.
    let direct = st.direct_launch.as_ref().filter(|d| !st.prefer_launcher && st.game_dir.join(rel_to_path(&d.exe)).exists());
    let spec = match direct {
        Some(d) => Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: d.cwd.as_deref().map(rel_to_path), learn: false },
        None => Launch { exe: rel_to_path(&exe), args: &[], env: &[], cwd: None, learn: true },
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

    tauri::async_runtime::spawn(async move {
        let learned = tauri::async_runtime::spawn_blocking(move || running.wait()).await.ok().flatten();
        let ended = OffsetDateTime::now_utc();
        let state = app.state::<AppState>();
        state.running.lock().unwrap().remove(&rom_id);
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
        let (save, save_error) = saves::after_play(&state, state.client().ok(), rom_id).await;
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
