pub mod install;
pub mod launch;
pub mod mods;
pub mod music;
pub mod saves;
pub mod library;
pub mod session;
pub mod sounds;
pub mod system;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[tauri::command]
pub fn open_logs(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    crate::external::open_path(&app, &state.log_dir.to_string_lossy(), "ouverture du dossier de logs")
}

/// Erreurs de l'interface, recopiées dans le fichier de logs.
#[tauri::command]
pub fn log_frontend(level: String, message: String) {
    let message: String = message.chars().take(8000).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "interface", "{message}"),
        "warn" => tracing::warn!(target: "interface", "{message}"),
        _ => tracing::info!(target: "interface", "{message}"),
    }
}

#[tauri::command]
pub fn open_url(app: tauri::AppHandle, state: State<'_, AppState>, url: String) -> AppResult<()> {
    // N'ouvre que des pages du serveur RomM configuré (ex. page d'approbation).
    let server = state.server_url()?;
    if !url.starts_with(&format!("{server}/")) {
        return Err(AppError::Internal);
    }
    crate::external::open_url(&app, &url)
}
