//! Rapports de bug envoyés par le joueur (report.rs) à FarfaRomManager. Le cœur est dans
//! `community.rs`, partagé avec le moteur du Steam Deck.

use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

/// Fichiers qui seront joints au rapport de ce jeu (montrés avant l'envoi).
#[tauri::command]
pub fn report_files(state: State<'_, AppState>, rom_id: i64) -> AppResult<Vec<String>> {
    crate::community::report_files(&state, rom_id)
}

/// Construit le rapport et l'envoie ; rend son identifiant sur le gestionnaire.
#[tauri::command]
pub async fn send_report(state: State<'_, AppState>, rom_id: i64, description: String, after_crash: bool) -> AppResult<String> {
    crate::community::send_report(&state, rom_id, &description, after_crash).await
}

/// Rapports envoyés par ce joueur (tous ses PC), avec la réponse après analyse.
#[tauri::command]
pub async fn my_reports(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    crate::community::my_reports(&state).await
}

/// Rapports lus : identifiant → `updatedAt` vu.
#[tauri::command]
pub fn reports_seen(state: State<'_, AppState>) -> std::collections::BTreeMap<String, u64> {
    state.settings().reports_seen
}

/// Retient les rapports qui viennent d'être lus.
#[tauri::command]
pub fn mark_reports_seen(state: State<'_, AppState>, seen: std::collections::BTreeMap<String, u64>) -> AppResult<()> {
    state.update_settings(|s| s.reports_seen.extend(seen))?;
    Ok(())
}

/// Réponse du joueur à l'analyse d'un de ses rapports (Claude lui répondra dans le fil).
#[tauri::command]
pub async fn reply_report(state: State<'_, AppState>, id: String, text: String) -> AppResult<serde_json::Value> {
    crate::community::reply_report(&state, &id, &text).await
}

/// Le support (le PC qui analyse les rapports) est-il allumé ? `{ "enLigne": bool, "vuA": secondes|null }`.
#[tauri::command]
pub async fn support_status(state: State<'_, AppState>) -> AppResult<serde_json::Value> {
    crate::community::support_status(&state).await
}
