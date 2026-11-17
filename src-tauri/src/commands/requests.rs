//! Demandes de jeux envoyées à FarfaRomManager (`/api/requests`) : le joueur en envoie et suit
//! les siennes ; l'administrateur de RomM les voit toutes et change leur statut.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::romm::client::{parse_json, RommClient};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub console: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub message: String,
    /// `nouveau`, `accepté`, `refusé` ou `ajouté`.
    pub status: String,
    #[serde(default)]
    pub response: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Compte qui l'a envoyée (liste de l'administrateur seulement).
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Deserialize)]
struct Me {
    #[serde(default)]
    admin: bool,
}

/// Client du gestionnaire (même jeton que RomM), refusé hors ligne.
fn manager(state: &AppState) -> AppResult<RommClient> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    Ok(state.online_client()?.with_base(&base))
}

#[tauri::command]
pub async fn my_game_requests(state: State<'_, AppState>) -> AppResult<Vec<GameRequest>> {
    manager(&state)?.get_json("api/requests").await
}

#[tauri::command]
pub async fn send_game_request(state: State<'_, AppState>, name: String, console: String, link: String, message: String) -> AppResult<GameRequest> {
    let client = manager(&state)?;
    let body = serde_json::json!({ "name": name.trim(), "console": console.trim(), "link": link.trim(), "message": message.trim() });
    let created: GameRequest = parse_json(client.send(client.post("api/requests", &body)).await?).await?;
    tracing::info!("demande de jeu envoyée : {} ({})", created.name, created.id);
    Ok(created)
}

/// L'administrateur voit toutes les demandes ; `None` pour un autre compte (ou un gestionnaire
/// trop ancien pour les demandes).
#[tauri::command]
pub async fn all_game_requests(state: State<'_, AppState>) -> AppResult<Option<Vec<GameRequest>>> {
    let client = manager(&state)?;
    let me: Me = match client.get_json("api/me").await {
        Ok(me) => me,
        // Gestionnaire sans les demandes (404) : pas de gestion à montrer.
        Err(AppError::Server) => return Ok(None),
        Err(e) => return Err(e),
    };
    if !me.admin {
        return Ok(None);
    }
    client.get_json("api/requests/all").await.map(Some)
}

/// Statut d'une demande (administrateur), avec une réponse pour le joueur.
#[tauri::command]
pub async fn update_game_request(state: State<'_, AppState>, id: String, status: String, response: String) -> AppResult<GameRequest> {
    if !id.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(AppError::NotAvailable);
    }
    let client = manager(&state)?;
    let body = serde_json::json!({ "status": status, "response": response.trim() });
    let req = client.patch(&format!("api/requests/{id}"), &body);
    parse_json(client.send(req).await?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_request_from_the_manager() {
        let r: GameRequest = serde_json::from_str(
            r#"{"id":"20261117-101500-2-1","name":"Okami HD","console":"PC","link":"","message":"Merci","status":"nouveau","response":"","createdAt":1,"updatedAt":2}"#,
        )
        .unwrap();
        assert_eq!(r.status, "nouveau");
        assert_eq!(r.username, None);
    }
}
