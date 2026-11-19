//! Statistiques entre amis : profil, classements et « Masquer mes statistiques » (voir `crate::stats`).

use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::error::AppResult;
use crate::state::AppState;

/// Envoie au gestionnaire les sessions RomM qui lui manquent, en tâche de fond. `delay_secs` :
/// au démarrage, on laisse d'abord la bibliothèque se charger.
pub fn sync_later(app: &AppHandle, delay_secs: u64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        crate::stats::sync(&app.state::<AppState>()).await;
    });
}

#[tauri::command]
pub async fn player_stats(state: State<'_, AppState>, id: Option<i64>) -> AppResult<Option<Value>> {
    crate::stats::profile(&state, id).await
}

/// « Rafraîchir » : les sessions RomM qui manquent au gestionnaire partent tout de suite.
#[tauri::command]
pub async fn stats_sync(state: State<'_, AppState>) -> AppResult<()> {
    crate::stats::sync(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn stats_ranking(state: State<'_, AppState>) -> AppResult<Option<Value>> {
    crate::stats::ranking(&state).await
}

#[tauri::command]
pub async fn game_ranking(state: State<'_, AppState>, rom_id: i64) -> AppResult<Option<Value>> {
    crate::stats::game_ranking(&state, rom_id).await
}

#[tauri::command]
pub async fn set_stats_hidden(state: State<'_, AppState>, hidden: bool) -> AppResult<bool> {
    crate::stats::set_hidden(&state, hidden).await?;
    Ok(hidden)
}
