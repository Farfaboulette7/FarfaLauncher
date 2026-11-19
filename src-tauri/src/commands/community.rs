//! Onglet « Communautaire » d'un jeu : avis des joueurs (notes RomM) et signalements du jeu
//! (FarfaRomManager). Le classement est dans `stats`.
//!
//! Un avis est une note RomM publique (`/api/roms/<id>/notes`), une par joueur et par jeu, de titre
//! `Avis`. RomM ne laisse lire à un compte que ses propres notes de jeu (`rating`, `difficulty`,
//! `completion`) : elles voyagent donc aussi dans les étiquettes de l'avis (`note:8`,
//! `difficulte:4`, `completion:100`), et sont posées dans RomM pour le joueur.

use serde_json::{json, Value};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::romm::client::parse_json;
use crate::state::AppState;

/// Titre des avis écrits par le launcher.
const REVIEW_TITLE: &str = "Avis";

/// Avis publics du jeu (et les notes privées du joueur), avec l'ID du joueur.
#[tauri::command]
pub async fn game_reviews(state: State<'_, AppState>, rom_id: i64) -> AppResult<Value> {
    let client = state.client()?;
    let me = crate::romm::auth::me(&client).await?;
    let notes: Vec<Value> = client.get_json(&format!("api/roms/{rom_id}/notes")).await?;
    let notes: Vec<Value> = notes.into_iter().filter(|n| n["is_public"].as_bool() == Some(true) || n["user_id"].as_i64() == Some(me.id)).collect();
    Ok(json!({ "me": me.id, "notes": notes }))
}

/// Notes du joueur sur le jeu (0 : pas de note).
fn clamp(v: Option<i64>, max: i64) -> Option<i64> {
    v.map(|v| v.clamp(0, max))
}

/// Étiquettes de l'avis : ses notes, lisibles par les autres joueurs.
fn review_tags(rating: Option<i64>, difficulty: Option<i64>, completion: Option<i64>) -> Vec<String> {
    [("note", rating), ("difficulte", difficulty), ("completion", completion)]
        .into_iter()
        .filter_map(|(k, v)| v.filter(|v| *v > 0).map(|v| format!("{k}:{v}")))
        .collect()
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
    let client = state.client()?;
    let (rating, difficulty, completion) = (clamp(rating, 10), clamp(difficulty, 10), clamp(completion, 100));
    let body = json!({
        "title": REVIEW_TITLE,
        "content": content.trim(),
        "is_public": true,
        "tags": review_tags(rating, difficulty, completion),
    });
    let req = match note_id {
        Some(id) => client.put(&format!("api/roms/{rom_id}/notes/{id}"), &body),
        None => client.post(&format!("api/roms/{rom_id}/notes"), &body),
    };
    let note: Value = parse_json(client.send(req).await?).await?;
    // Ses notes dans RomM aussi (page du jeu sur RomM) ; l'avis est déjà publié si ça échoue.
    let props = json!({ "rating": rating.unwrap_or(0), "difficulty": difficulty.unwrap_or(0), "completion": completion.unwrap_or(0) });
    if let Err(e) = client.send(client.put(&format!("api/roms/{rom_id}/props"), &props)).await {
        tracing::warn!("notes du jeu non posées dans RomM: {}", e.code());
    }
    Ok(note)
}

#[tauri::command]
pub async fn delete_game_review(state: State<'_, AppState>, rom_id: i64, note_id: i64) -> AppResult<()> {
    let client = state.client()?;
    client.send(client.delete(&format!("api/roms/{rom_id}/notes/{note_id}"), &json!({}))).await?;
    Ok(())
}

/// Signalements du jeu, tous joueurs confondus. `None` : gestionnaire sans cette liste.
#[tauri::command]
pub async fn game_reports(state: State<'_, AppState>, rom_id: i64) -> AppResult<Option<Value>> {
    let client = crate::presence::manager(&state)?;
    match client.send(client.get(&format!("api/reports/game/{rom_id}"))).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        Err(AppError::Server) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_travel_in_the_tags() {
        assert_eq!(review_tags(Some(8), Some(0), Some(100)), ["note:8", "completion:100"]);
        assert!(review_tags(None, None, None).is_empty());
        assert_eq!(clamp(Some(14), 10), Some(10));
    }
}
