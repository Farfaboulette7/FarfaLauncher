//! Statistiques entre amis (FarfaRomManager, `/api/stats`).
//!
//! RomM ne laisse lire à un compte que ses propres sessions de jeu : le launcher relit donc les
//! siennes dans RomM et envoie au gestionnaire celles qu'il n'a pas encore (numéro de session
//! RomM plus grand que le dernier reçu). Historique compris, au premier envoi. Un gestionnaire
//! sans les statistiques répond 404 : on se tait.

use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::error::{AppError, AppResult};
use crate::romm::client::{parse_json, RommClient};
use crate::state::AppState;

const PAGE: usize = 500;
/// Lot accepté par le gestionnaire.
const BATCH: usize = 1000;

#[derive(Deserialize)]
struct RommSession {
    id: i64,
    /// `null` pour un jeu supprimé de RomM (ou remplacé) : session ignorée. Lu comme un nombre
    /// obligatoire, une seule de ces sessions empêchait tout envoi (compte 1, 2026-11-19).
    rom_id: Option<i64>,
    start_time: String,
    duration_ms: i64,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Upload {
    pub id: i64,
    pub rom_id: i64,
    pub start: i64,
    pub duration_ms: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncInfo {
    last_id: i64,
}

/// Date RomM en secondes Unix. RomM l'écrit en UTC, souvent sans fuseau (`2026-11-17T10:47:07`).
fn parse_utc(text: &str) -> Option<i64> {
    let parsed = OffsetDateTime::parse(text, &Rfc3339).or_else(|_| OffsetDateTime::parse(&format!("{text}Z"), &Rfc3339));
    parsed.ok().map(|d| d.unix_timestamp())
}

/// Sessions RomM à envoyer : après `last_id`, durée positive, date lisible.
fn to_upload(sessions: Vec<RommSession>, last_id: i64) -> Vec<Upload> {
    let mut out: Vec<Upload> = sessions
        .into_iter()
        .filter(|s| s.id > last_id && s.duration_ms > 0)
        .filter_map(|s| {
            let start = parse_utc(&s.start_time)?;
            Some(Upload { id: s.id, rom_id: s.rom_id?, start, duration_ms: s.duration_ms })
        })
        .collect();
    out.sort_by_key(|u| u.id);
    out
}

async fn romm_sessions(client: &RommClient) -> AppResult<Vec<RommSession>> {
    let mut all = Vec::new();
    let mut offset = 0usize;
    loop {
        let page: Vec<RommSession> = client.get_json(&format!("api/play-sessions?limit={PAGE}&offset={offset}")).await?;
        let n = page.len();
        all.extend(page);
        if n < PAGE {
            return Ok(all);
        }
        offset += PAGE;
    }
}

/// Envoie au gestionnaire les sessions RomM qu'il n'a pas. Silencieux si l'un des deux ne
/// répond pas : le prochain envoi rattrapera.
pub async fn sync(state: &AppState) {
    let Ok(romm) = state.online_client() else { return };
    let Some(base) = crate::manager::base_url(&state.settings()) else { return };
    let manager = romm.with_base(&base);
    let info: SyncInfo = match manager.get_json("api/stats/sync").await {
        Ok(i) => i,
        Err(e) => return tracing::info!("statistiques : gestionnaire sans réponse ({})", e.code()),
    };
    let sessions = match romm_sessions(&romm).await {
        Ok(s) => s,
        Err(e) => return tracing::warn!("statistiques : sessions RomM non lues ({})", e.code()),
    };
    let read = sessions.len();
    let uploads = to_upload(sessions, info.last_id);
    let mut sent = 0;
    for chunk in uploads.chunks(BATCH) {
        if manager.send(manager.post("api/stats/sessions", &json!({ "sessions": chunk }))).await.is_err() {
            break;
        }
        sent += chunk.len();
    }
    if sent > 0 || uploads.len() != sent {
        tracing::info!("statistiques : {read} sessions lues dans RomM, {sent} sur {} envoyées au gestionnaire", uploads.len());
    }
}

/// Client du gestionnaire, refusé hors ligne.
fn manager(state: &AppState) -> AppResult<RommClient> {
    crate::presence::manager(state)
}

/// Réponse JSON ; `None` si le gestionnaire ne connaît pas les statistiques (404).
async fn get_opt<T: serde::de::DeserializeOwned>(client: &RommClient, path: &str) -> AppResult<Option<T>> {
    match client.send(client.get(path)).await {
        Ok(resp) => parse_json(resp).await.map(Some),
        Err(AppError::Server) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Profil d'un joueur (`None` : le sien), avec `me` (c'est le sien). `None` en retour : masqué,
/// ou gestionnaire trop ancien.
pub async fn profile(state: &AppState, id: Option<i64>) -> AppResult<Option<serde_json::Value>> {
    let client = manager(state)?;
    let Some(me) = get_opt::<serde_json::Value>(&client, "api/me").await? else { return Ok(None) };
    let me = me["id"].as_i64().ok_or(AppError::Server)?;
    let id = id.unwrap_or(me);
    let mut p: Option<serde_json::Value> = get_opt(&client, &format!("api/stats/players/{id}")).await?;
    if let Some(p) = p.as_mut() {
        p["me"] = json!(id == me);
    }
    Ok(p)
}

pub async fn ranking(state: &AppState) -> AppResult<Option<serde_json::Value>> {
    get_opt(&manager(state)?, "api/stats/players").await
}

pub async fn game_ranking(state: &AppState, rom_id: i64) -> AppResult<Option<serde_json::Value>> {
    get_opt(&manager(state)?, &format!("api/stats/games/{rom_id}")).await
}

pub async fn set_hidden(state: &AppState, hidden: bool) -> AppResult<()> {
    let client = manager(state)?;
    client.send(client.put("api/stats/prefs", &json!({ "hidden": hidden }))).await?;
    tracing::info!("statistiques masquées : {}", if hidden { "oui" } else { "non" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_new_sessions_are_uploaded() {
        let raw: Vec<RommSession> = serde_json::from_str(
            r#"[
              {"id":12,"rom_id":7,"start_time":"2026-11-17T10:00:00+00:00","duration_ms":60000,"user_id":1},
              {"id":10,"rom_id":42,"start_time":"2026-11-16T20:00:00Z","duration_ms":3600000},
              {"id":11,"rom_id":42,"start_time":"pas une date","duration_ms":1000},
              {"id":14,"rom_id":42,"start_time":"2026-11-17T10:47:07","duration_ms":2366000},
              {"id":13,"rom_id":42,"start_time":"2026-11-17T11:00:00Z","duration_ms":0},
              {"id":15,"rom_id":null,"start_time":"2026-11-17T12:00:00Z","duration_ms":60000}
            ]"#,
        )
        .unwrap();
        let up = to_upload(raw, 10);
        assert_eq!(
            up,
            vec![
                Upload { id: 12, rom_id: 7, start: 1_794_909_600, duration_ms: 60_000 },
                Upload { id: 14, rom_id: 42, start: 1_794_909_600 + 47 * 60 + 7, duration_ms: 2_366_000 },
            ]
        );
        assert_eq!(serde_json::to_value(&up[0]).unwrap()["durationMs"], 60_000);
    }
}
