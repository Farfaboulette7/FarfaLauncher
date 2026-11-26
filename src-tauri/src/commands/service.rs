//! Onglet « Service » de l'écran « Fiches » (administrateurs) : tableau de bord du gestionnaire
//! (santé, disque du NAS, recomps et émulateurs en échec, dernières alertes, rapports en attente,
//! demandes de jeux, joueurs). Lecture seule : `GET /api/service` du gestionnaire.
//!
//! Le gestionnaire revérifie que le compte est administrateur et ne met rien de secret dans la
//! réponse (son `docs/architecture.md`, 6.8). Ici, on relaie le JSON tel quel : l'interface
//! (`components/ServicePanel.tsx`) lit les champs qu'elle connaît et ignore les autres, ce qui
//! laisse le gestionnaire ajouter des blocs sans nouveau launcher.
//!
//! Pas sur le Steam Deck (module `commands`, interface seulement).

use tauri::State;

use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::{error_chain, truncate, RommClient};
use crate::state::AppState;

/// Tableau de bord ; `None` si le gestionnaire n'a pas encore la route (404 : gestionnaire d'avant
/// la phase 3, à déployer). L'interface l'explique sans erreur.
#[tauri::command]
pub async fn service_status(state: State<'_, AppState>) -> AppResult<Option<serde_json::Value>> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    let client: RommClient = state.online_client()?.with_base(&base);
    let resp = client
        .get("api/service")
        .send()
        .await
        .map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
    let status = resp.status().as_u16();
    let body = resp.text().await.map_err(|e| log_err(AppError::Server, "lecture du tableau de bord", error_chain(&e)))?;
    interpret(status, &body)
}

/// Réponse du gestionnaire → tableau de bord. Séparé pour être testé sans réseau.
fn interpret(status: u16, body: &str) -> AppResult<Option<serde_json::Value>> {
    match status {
        200 => match serde_json::from_str::<serde_json::Value>(body) {
            Ok(v) if v.is_object() => Ok(Some(v)),
            _ => {
                tracing::warn!("service : réponse illisible : {}", truncate(body, 200));
                Err(AppError::Server)
            }
        },
        404 => Ok(None),
        // Compte qui n'est plus administrateur (rôle revérifié par le gestionnaire toutes les 60 s).
        403 => Err(AppError::Forbidden),
        // 401 du gestionnaire : jamais « déconnecté » (seul RomM peut refuser le jeton, voir `RommClient::check`).
        _ => {
            tracing::warn!("service : HTTP {status} : {}", truncate(body, 200));
            Err(AppError::Server)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_manager_and_refusals() {
        // Gestionnaire d'avant (route absente) : pas d'erreur, l'onglet l'explique.
        assert!(interpret(404, r#"{"detail":"Not Found"}"#).unwrap().is_none());
        assert!(matches!(interpret(403, r#"{"detail":"Réservé à l'administrateur"}"#), Err(AppError::Forbidden)));
        assert!(matches!(interpret(401, ""), Err(AppError::Server)));
        assert!(matches!(interpret(503, "<html>"), Err(AppError::Server)));
        assert!(matches!(interpret(200, "<html>proxy</html>"), Err(AppError::Server)));
        assert!(matches!(interpret(200, "[1]"), Err(AppError::Server)));
    }

    #[test]
    fn dashboard_is_relayed_as_is() {
        let v = interpret(200, r#"{"sante":{"status":"ok"},"bloc_futur":{"x":1}}"#).unwrap().unwrap();
        assert_eq!(v["sante"]["status"], "ok");
        // Un bloc inconnu de ce launcher passe aussi : l'interface l'ignore.
        assert_eq!(v["bloc_futur"]["x"], 1);
    }
}
