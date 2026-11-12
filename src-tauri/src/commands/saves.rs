//! Commandes Tauri des sauvegardes. Le moteur (comparaison avec le serveur,
//! restauration, envoi après la partie) est dans `save_sync`, partagé avec le
//! programme du Steam Deck.

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::install::state::{self, InstalledGame};
use crate::save_sync::SaveCheck;
use crate::state::AppState;

#[tauri::command]
pub async fn save_check(state: State<'_, AppState>, rom_id: i64) -> AppResult<SaveCheck> {
    crate::save_sync::check(&state, rom_id).await
}

/// Choix du dossier des sauvegardes par l'utilisateur (`dir = None` +
/// `off = true` : jouer sans synchroniser ; `dir = None` + `off = false` :
/// redemander / redeviner).
#[tauri::command]
pub fn set_save_location(state: State<'_, AppState>, rom_id: i64, dir: Option<String>, off: bool) -> AppResult<InstalledGame> {
    crate::save_sync::set_location(&state, rom_id, dir, off)
}

#[tauri::command]
pub fn open_save_folder(app: tauri::AppHandle, state: State<'_, AppState>, rom_id: i64) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let dir = st.saves.dir.filter(|d| d.is_dir()).ok_or(AppError::SaveDirInvalid)?;
    crate::external::open_path(&app, &dir.to_string_lossy(), "ouverture du dossier des sauvegardes")
}

/// Shaders partagés de ce jeu sur FarfaRomManager, par émulateur Switch (vide sinon).
#[tauri::command]
pub async fn shader_info(state: State<'_, AppState>, rom_id: i64) -> AppResult<serde_json::Value> {
    let Some(base) = crate::manager::base_url(&state.settings()) else { return Ok(serde_json::json!({})) };
    let manager = state.client()?.with_base(&base);
    match manager.get_json::<serde_json::Value>(&format!("api/shaders/rom/{rom_id}")).await {
        Ok(v) => Ok(v),
        // Gestionnaire sans les shaders (pas encore à jour) ou injoignable : rien à montrer.
        Err(_) => Ok(serde_json::json!({})),
    }
}
