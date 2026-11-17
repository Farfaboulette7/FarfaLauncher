//! Présence des amis (FarfaRomManager, `/api/presence`) : le launcher dit qu'il est ouvert et à
//! quoi joue le joueur, toutes les 60 s et à chaque lancement ou arrêt de jeu. Le gestionnaire
//! oublie un compte après 3 min sans signal.
//!
//! « Apparaître hors ligne » (`Settings::presence_hidden`) : plus aucun signal, seulement le
//! choix lui-même, pour que le gestionnaire efface le compte tout de suite. Un gestionnaire trop
//! ancien répond 404 : on se tait.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::romm::client::{parse_json, RommClient};
use crate::state::AppState;

/// Intervalle entre deux signaux (le gestionnaire attend 3 min avant d'oublier).
pub const INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FriendGame {
    pub rom_id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Friend {
    pub id: i64,
    pub username: String,
    #[serde(default)]
    pub avatar: bool,
    /// `en jeu`, `en ligne` ou `hors ligne`.
    pub status: String,
    #[serde(default)]
    pub game: Option<FriendGame>,
    /// Début de la partie ou de la présence (Unix, secondes).
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub last_seen: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Friends {
    #[serde(default)]
    pub hidden: bool,
    pub amis: Vec<Friend>,
}

/// Client du gestionnaire (même jeton que RomM), refusé hors ligne.
pub fn manager(state: &AppState) -> AppResult<RommClient> {
    let base = crate::manager::base_url(&state.settings()).ok_or(AppError::NotAvailable)?;
    Ok(state.online_client()?.with_base(&base))
}

/// Corps du signal : en jeu si un jeu tourne (le premier lancé), sinon en ligne.
pub fn body(game: Option<FriendGame>) -> Value {
    match game {
        Some(g) => json!({ "state": "en jeu", "game": { "romId": g.rom_id, "name": g.name }, "hidden": false }),
        None => json!({ "state": "en ligne", "hidden": false }),
    }
}

/// Nom d'un jeu : bibliothèque en mémoire, sinon le jeu installé.
pub fn game_ref(state: &AppState, rom_id: i64) -> FriendGame {
    let from_library = state.library.lock().unwrap().as_ref().and_then(|l| l.games.iter().find(|g| g.id == rom_id).map(|g| g.name.clone()));
    let name = from_library
        .or_else(|| crate::install::state::find(&state.config_dir, rom_id).map(|g| g.name))
        .unwrap_or_default();
    FriendGame { rom_id, name }
}

/// Jeu en cours dans ce launcher (le plus petit identifiant s'il y en a plusieurs).
pub fn current_game(state: &AppState) -> Option<FriendGame> {
    let id = state.running.lock().unwrap().iter().copied().min()?;
    Some(game_ref(state, id))
}

/// Envoie un signal ; silencieux en cas d'échec (le prochain réessaie).
pub async fn post(client: &RommClient, body: &Value) {
    match client.send(client.post("api/presence", body)).await {
        Ok(_) => {}
        // 404 : gestionnaire sans la présence. Le reste (réseau…) : déjà journalisé par `send`.
        Err(e) => tracing::debug!("présence non envoyée : {}", e.code()),
    }
}

/// Signal de ce launcher, sauf « Apparaître hors ligne » ou hors ligne.
pub async fn signal(state: &AppState) {
    if state.settings().presence_hidden {
        return;
    }
    let Ok(client) = manager(state) else { return };
    post(&client, &body(current_game(state))).await;
}

/// Le launcher se ferme : hors ligne tout de suite (sinon après 3 min).
pub async fn leave(state: &AppState) {
    if state.settings().presence_hidden {
        return;
    }
    let Ok(client) = manager(state) else { return };
    let _ = client.send(client.delete("api/presence", &json!({}))).await;
}

/// Amis et leur présence ; `None` si le gestionnaire ne connaît pas la présence (404).
pub async fn friends(state: &AppState) -> AppResult<Option<Friends>> {
    let client = manager(state)?;
    match client.send(client.get("api/presence/amis")).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        Err(AppError::Server) => Ok(None),
        Err(e) => Err(e),
    }
}

/// « Apparaître hors ligne » : enregistré, puis dit au gestionnaire (il efface le compte, ou le
/// remontre aussitôt).
pub async fn set_hidden(state: &AppState, hidden: bool) -> AppResult<()> {
    state.update_settings(|s| s.presence_hidden = hidden)?;
    tracing::info!("apparaître hors ligne : {}", if hidden { "oui" } else { "non" });
    let Ok(client) = manager(state) else { return Ok(()) };
    let mut b = body(current_game(state));
    b["hidden"] = json!(hidden);
    post(&client, &b).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_body() {
        assert_eq!(body(None), json!({ "state": "en ligne", "hidden": false }));
        let b = body(Some(FriendGame { rom_id: 42, name: "Zelda".into() }));
        assert_eq!(b["state"], "en jeu");
        assert_eq!(b["game"]["romId"], 42);
        assert_eq!(b["game"]["name"], "Zelda");
    }

    #[test]
    fn reads_friends_from_the_manager() {
        let f: Friends = serde_json::from_str(
            r#"{"hidden":false,"amis":[
                {"id":2,"username":"Paul","avatar":true,"status":"en jeu","game":{"romId":42,"name":"Zelda"},"since":10,"lastSeen":70},
                {"id":3,"username":"Léa","avatar":false,"status":"hors ligne","game":null,"since":null,"lastSeen":null}]}"#,
        )
        .unwrap();
        assert_eq!(f.amis.len(), 2);
        assert_eq!(f.amis[0].game, Some(FriendGame { rom_id: 42, name: "Zelda".into() }));
        assert_eq!(f.amis[1].last_seen, None);
    }
}
