//! Bouton « Jouer » : lancement, suivi de la session et envoi du temps de jeu.
//! Événements : `game-started` puis `game-stopped`. La préparation et la fin
//! de partie sont dans `play`, partagé avec le programme du Steam Deck.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};
use crate::gamepad;
use crate::install::state;
use crate::play::{self, CodChoice};
use crate::romm::playtime;
use crate::save_sync::{SaveChoice, SaveUpload};
use crate::state::AppState;
use crate::error::LockOrRecover;

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
    /// Le jeu semble avoir planté : l'interface propose d'envoyer un rapport.
    crashed: bool,
    /// Détails du plantage (durée, erreur), pour préremplir le rapport.
    crash: Option<crate::launch::Crash>,
    /// Ouvrir tout de suite la proposition de rapport (le joueur n'a pas coupé la
    /// question pour ce jeu).
    ask_report: bool,
}

use crate::community::ask_report;

/// « Ne plus demander pour ce jeu » (`ask = false`) ou réactive la question après un plantage.
#[tauri::command]
pub fn set_crash_prompt(state: State<'_, AppState>, rom_id: i64, ask: bool) -> AppResult<()> {
    crate::community::set_crash_prompt(&state, rom_id, ask)
}

#[tauri::command]
pub fn running_games(state: State<'_, AppState>) -> Vec<i64> {
    state.running.lock_or_recover().iter().copied().collect()
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
    st.executable.as_ref().ok_or(AppError::NoExecutable)?;
    // Même programme et mêmes données pour tous les jeux d'un runtime : un seul à la fois.
    state.check_family_idle(rom_id)?;
    // Ni en installation, ni occupé, ni déjà lancé : vérifié et inscrit d'un bloc (L13).
    state.start_running(rom_id)?;
    let started = match play::start(&state, rom_id, save_choice, cod, menu == Some(true), &gamepad::connected()).await {
        Ok(s) => s,
        Err(e) => {
            state.running.lock_or_recover().remove(&rom_id);
            return Err(e);
        }
    };
    let play::Started { running, started, shader_job, game: st } = started;
    state.playing.lock_or_recover().insert(rom_id, crate::ingame::Playing { started: std::time::Instant::now(), stop: running.stop_flag(), control: running.control() });
    let _ = app.emit("game-started", rom_id);
    crate::commands::presence::notify(&app);
    crate::ingame::prepare(&app);
    gamepad::game_started();
    let ended_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    crate::dialogs::watch(state.catalog().dialogs_for_install(&st), ended_flag.clone());

    tauri::async_runtime::spawn(async move {
        let shown = app.clone();
        let (learned, crash) = tauri::async_runtime::spawn_blocking(move || {
            running.wait_outcome(move || {
                let _ = shown.emit("game-window", rom_id);
            })
        })
        .await
        .unwrap_or((None, None));
        ended_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        let state = app.state::<AppState>();
        state.running.lock_or_recover().remove(&rom_id);
        state.playing.lock_or_recover().remove(&rom_id);
        crate::commands::presence::notify(&app);
        if state.playing.lock_or_recover().is_empty() {
            crate::ingame::close(&app);
        }
        gamepad::game_stopped();
        // Retour au launcher, pilotable à la manette sans clic pour reprendre le focus.
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        let f = play::finish(&state, rom_id, started, learned, shader_job).await;
        // Session envoyée à RomM par `finish` : le gestionnaire la reçoit à son tour.
        crate::commands::stats::sync_later(&app, 0);
        let ask_report = ask_report(crash.as_ref(), &state.settings().crash_prompt_off, rom_id);
        let _ = app.emit(
            "game-stopped",
            GameStopped {
                rom_id,
                duration_ms: f.duration_ms,
                recorded: f.recorded,
                pending: f.pending,
                save: f.save,
                save_error: f.save_error,
                game: f.game,
                crashed: crash.is_some(),
                crash: crash.clone(),
                ask_report,
            },
        );
        crate::commands::steam::after_game(&app, rom_id, crash.is_some());
    });
    Ok(())
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
