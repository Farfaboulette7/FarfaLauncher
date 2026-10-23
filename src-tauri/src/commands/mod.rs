pub mod install;
pub mod library;
pub mod session;

use tauri::State;
use tauri_plugin_opener::OpenerExt;

use crate::error::{log_err, AppError, AppResult};
use crate::state::AppState;

#[tauri::command]
pub fn open_logs(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    app.opener()
        .open_path(state.log_dir.to_string_lossy(), None::<&str>)
        .map_err(|e| log_err(AppError::Internal, "ouverture du dossier de logs", e))
}

#[tauri::command]
pub fn open_url(app: tauri::AppHandle, state: State<'_, AppState>, url: String) -> AppResult<()> {
    // N'ouvre que des pages du serveur RomM configuré (ex. page d'approbation).
    let server = state.server_url()?;
    if !url.starts_with(&format!("{server}/")) {
        return Err(AppError::Internal);
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| log_err(AppError::Internal, "ouverture du navigateur", e))
}
