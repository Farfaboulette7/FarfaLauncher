//! Menu en jeu : un appui maintenu sur Select + Start (lu par gamepad.rs pendant
//! la partie) fait passer le launcher au premier plan avec un petit menu : infos
//! du jeu, temps de jeu, reprendre ou quitter. Il ne dépend ni de l'émulateur ni
//! de la recomp : le launcher ferme lui-même le jeu (voir launch::RunningGame).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};
use crate::gamepad;
use crate::state::AppState;

pub const EVENT: &str = "ingame-menu";

/// Partie en cours.
pub struct Playing {
    pub started: Instant,
    /// Levé pour fermer le jeu (voir `RunningGame::stop_flag`).
    pub stop: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MenuOpened {
    rom_id: i64,
    /// Durée de la partie en cours.
    session_ms: u64,
}

/// Ouvre le menu sur la partie la plus récente (appelé par le fil des manettes).
pub fn open(app: &AppHandle) {
    let state = app.state::<AppState>();
    let latest = state.playing.lock().unwrap().iter().max_by_key(|(_, p)| p.started).map(|(id, p)| (*id, p.started.elapsed()));
    let Some((rom_id, elapsed)) = latest else { return };
    tracing::info!("menu en jeu ouvert (jeu {rom_id})");
    gamepad::set_menu(true);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_always_on_top(true);
        bring_to_front(&w);
    }
    let _ = app.emit(EVENT, MenuOpened { rom_id, session_ms: elapsed.as_millis() as u64 });
}

/// Referme le menu : la manette retourne au jeu, le launcher n'est plus au-dessus.
pub fn close(app: &AppHandle) {
    gamepad::set_menu(false);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_always_on_top(false);
    }
}

/// Reprendre la partie : le launcher s'efface, le jeu revient devant.
#[tauri::command]
pub fn ingame_resume(app: AppHandle) {
    close(&app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

/// Quitter la partie : le jeu est prié de se fermer, puis arrêté s'il ne le fait pas.
#[tauri::command]
pub fn quit_game(state: State<'_, AppState>, rom_id: i64) -> AppResult<()> {
    let stop = state.playing.lock().unwrap().get(&rom_id).map(|p| p.stop.clone()).ok_or(AppError::NotInstalled)?;
    tracing::info!("quitter le jeu {rom_id} depuis le menu en jeu");
    gamepad::set_menu(false);
    stop.store(true, Ordering::Relaxed);
    Ok(())
}

/// Premier plan, même quand le launcher n'a pas la main (tao contourne le
/// blocage de Windows par un appui simulé sur Alt).
fn bring_to_front(w: &tauri::WebviewWindow) {
    let _ = w.set_focus();
}
