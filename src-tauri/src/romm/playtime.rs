//! Temps de jeu via les sessions de jeu RomM (docs/romm-api.md §5).
//!
//! Chaque session terminée est d'abord écrite dans une file locale
//! (`pending_sessions.json`), puis envoyée à RomM. Si le serveur est
//! injoignable, elle part au prochain envoi ; RomM ignore les doublons, donc
//! renvoyer une session déjà reçue est sans risque.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use super::client::{parse_json, RommClient};
use crate::error::{log_err, AppError, AppResult};

const PAGE: usize = 500;
/// Taille maximale d'un lot accepté par RomM.
const MAX_BATCH: usize = 100;

/// Sérialise les accès à la file (plusieurs jeux peuvent se fermer en même temps).
static QUEUE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Deserialize)]
struct RawSession {
    duration_ms: u64,
}

/// Temps total (ms) de l'utilisateur sur un jeu, tous appareils confondus.
/// `None` si le token n'a pas le droit `devices.read` (on n'en fait pas une
/// déconnexion : c'est un droit optionnel).
pub async fn total_ms(client: &RommClient, rom_id: i64) -> AppResult<Option<u64>> {
    let mut total = 0u64;
    let mut offset = 0usize;
    loop {
        let path = format!("api/play-sessions?rom_id={rom_id}&limit={PAGE}&offset={offset}");
        let page: Vec<RawSession> = match client.get_json(&path).await {
            Ok(p) => p,
            Err(AppError::Forbidden) => {
                tracing::warn!("lecture du temps de jeu refusée (scope devices.read manquant ?)");
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        total += page.iter().map(|s| s.duration_ms).sum::<u64>();
        if page.len() < PAGE {
            return Ok(Some(total));
        }
        offset += PAGE;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingSession {
    pub rom_id: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub start_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub end_time: OffsetDateTime,
    pub duration_ms: u64,
}

impl PendingSession {
    pub fn new(rom_id: i64, start: OffsetDateTime, end: OffsetDateTime) -> Self {
        // RomM tronque aux secondes et exige fin > début.
        let start = start.replace_nanosecond(0).unwrap_or(start);
        let end = end.replace_nanosecond(0).unwrap_or(end);
        let duration_ms = (end - start).whole_milliseconds().max(0) as u64;
        Self { rom_id, start_time: start, end_time: end, duration_ms }
    }
}

fn queue_path(config_dir: &Path) -> PathBuf {
    config_dir.join("pending_sessions.json")
}

fn load_queue(config_dir: &Path) -> Vec<PendingSession> {
    std::fs::read_to_string(queue_path(config_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_queue(config_dir: &Path, queue: &[PendingSession]) -> AppResult<()> {
    let path = queue_path(config_dir);
    if queue.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let text = serde_json::to_string_pretty(queue).map_err(|e| log_err(AppError::Internal, "file des sessions", e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| log_err(AppError::Disk, "écriture de la file des sessions", e))
}

/// Ajoute une session terminée à la file locale.
pub fn enqueue(config_dir: &Path, session: PendingSession) -> AppResult<()> {
    let _guard = QUEUE_LOCK.lock().unwrap();
    let mut queue = load_queue(config_dir);
    if !queue.contains(&session) {
        queue.push(session);
    }
    save_queue(config_dir, &queue)
}

pub fn pending_count(config_dir: &Path) -> usize {
    let _guard = QUEUE_LOCK.lock().unwrap();
    load_queue(config_dir).len()
}

#[derive(Serialize)]
struct IngestEntry {
    rom_id: i64,
    start_time: String,
    end_time: String,
    duration_ms: u64,
}

#[derive(Deserialize)]
struct IngestResponse {
    results: Vec<IngestResult>,
}

#[derive(Deserialize)]
struct IngestResult {
    index: usize,
    status: String,
    #[serde(default)]
    detail: Option<String>,
}

/// Envoie la file à RomM. Renvoie le nombre de sessions restant en attente.
pub async fn flush(client: &RommClient, config_dir: &Path) -> AppResult<usize> {
    let queue = {
        let _guard = QUEUE_LOCK.lock().unwrap();
        load_queue(config_dir)
    };
    if queue.is_empty() {
        return Ok(0);
    }
    let mut done: Vec<PendingSession> = Vec::new();
    let mut failure = None;
    for batch in queue.chunks(MAX_BATCH) {
        let sessions: Vec<IngestEntry> = batch
            .iter()
            .map(|s| IngestEntry {
                rom_id: s.rom_id,
                start_time: s.start_time.format(&Rfc3339).unwrap_or_default(),
                end_time: s.end_time.format(&Rfc3339).unwrap_or_default(),
                duration_ms: s.duration_ms,
            })
            .collect();
        // device_id omis : RomM prend l'appareil lié au token.
        let resp = match client.send(client.post("api/play-sessions", &serde_json::json!({ "sessions": sessions }))).await {
            Ok(r) => r,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        let body: IngestResponse = match parse_json(resp).await {
            Ok(b) => b,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        for r in body.results {
            let Some(s) = batch.get(r.index) else { continue };
            match r.status.as_str() {
                "created" | "duplicate" => tracing::info!("session de jeu enregistrée ({}, rom {}, {} ms)", r.status, s.rom_id, s.duration_ms),
                // Refusée définitivement (jeu supprimé…) : inutile de la renvoyer.
                _ => tracing::warn!("session de jeu refusée par RomM (rom {}): {:?}", s.rom_id, r.detail),
            }
            done.push(s.clone());
        }
    }
    let remaining = {
        let _guard = QUEUE_LOCK.lock().unwrap();
        let mut current = load_queue(config_dir);
        current.retain(|s| !done.contains(s));
        save_queue(config_dir, &current)?;
        current.len()
    };
    match failure {
        Some(e) if remaining > 0 => {
            tracing::warn!("{remaining} session(s) de jeu en attente d'envoi ({})", e.code());
            Err(e)
        }
        _ => Ok(remaining),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_truncates_and_computes_duration() {
        let start = OffsetDateTime::from_unix_timestamp(1_792_800_000).unwrap() + time::Duration::milliseconds(700);
        let end = start + time::Duration::seconds(90);
        let s = PendingSession::new(42, start, end);
        assert_eq!(s.duration_ms, 90_000);
        assert_eq!(s.start_time.nanosecond(), 0);
    }

    #[test]
    fn queue_roundtrip_dedups() {
        let dir = std::env::temp_dir().join(format!("farfa-queue-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let start = OffsetDateTime::from_unix_timestamp(1_792_800_000).unwrap();
        let s = PendingSession::new(1, start, start + time::Duration::minutes(5));
        enqueue(&dir, s.clone()).unwrap();
        enqueue(&dir, s.clone()).unwrap();
        assert_eq!(pending_count(&dir), 1);
        let json = std::fs::read_to_string(queue_path(&dir)).unwrap();
        assert!(json.contains("2026-10-24T"), "{json}");
        save_queue(&dir, &[]).unwrap();
        assert_eq!(pending_count(&dir), 0);
    }
}
