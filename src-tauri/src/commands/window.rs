//! Fenêtre : plein écran (par défaut, façon console) ou fenêtrée, et manettes.

use tauri::{State, WebviewWindow};

use crate::error::{log_err, AppError, AppResult};
use crate::state::AppState;

#[tauri::command]
pub fn is_fullscreen(window: WebviewWindow) -> bool {
    window.is_fullscreen().unwrap_or(false)
}

/// Passe en plein écran ou en fenêtre, et retient le choix.
#[tauri::command]
pub fn set_fullscreen(window: WebviewWindow, state: State<'_, AppState>, on: bool) -> AppResult<()> {
    window.set_fullscreen(on).map_err(|e| log_err(AppError::Internal, "plein écran", e))?;
    state.update_settings(|s| s.fullscreen = on)?;
    Ok(())
}

#[tauri::command]
pub fn gamepads() -> Vec<crate::gamepad::PadInfo> {
    crate::gamepad::connected()
}
