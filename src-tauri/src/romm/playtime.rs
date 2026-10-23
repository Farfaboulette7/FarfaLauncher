//! Temps de jeu via les sessions de jeu RomM (docs/romm-api.md §5).

use serde::Deserialize;

use super::client::RommClient;
use crate::error::{AppError, AppResult};

const PAGE: usize = 500;

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
