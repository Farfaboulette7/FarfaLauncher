//! Onglet « Communautaire » d'un jeu : avis des joueurs (notes RomM) et signalements du jeu
//! (FarfaRomManager). Le classement est dans `stats`. Le cœur est dans `community.rs`, partagé
//! avec le moteur du Steam Deck.

use serde_json::Value;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

/// Avis publics du jeu (et les notes privées du joueur), avec l'ID du joueur.
#[tauri::command]
pub async fn game_reviews(state: State<'_, AppState>, rom_id: i64) -> AppResult<Value> {
    crate::community::game_reviews(&state, rom_id).await
}

/// Publie ou met à jour l'avis du joueur, et ses notes dans RomM.
#[tauri::command]
pub async fn save_game_review(
    state: State<'_, AppState>,
    rom_id: i64,
    note_id: Option<i64>,
    content: String,
    rating: Option<i64>,
    difficulty: Option<i64>,
    completion: Option<i64>,
) -> AppResult<Value> {
    crate::community::save_game_review(&state, rom_id, note_id, &content, rating, difficulty, completion).await
}

#[tauri::command]
pub async fn delete_game_review(state: State<'_, AppState>, rom_id: i64, note_id: i64) -> AppResult<()> {
    crate::community::delete_game_review(&state, rom_id, note_id).await
}

/// Signalements du jeu, tous joueurs confondus. `None` : gestionnaire sans cette liste.
#[tauri::command]
pub async fn game_reports(state: State<'_, AppState>, rom_id: i64) -> AppResult<Option<Value>> {
    crate::community::game_reports(&state, rom_id).await
}
