//! Musique de fond : chaque morceau existe en version normale et 8-bit, avec ses
//! points de boucle. Comme les bruitages, elle vient du serveur des assets
//! (`assets.json` + `music/*.ogg`), réservé aux utilisateurs connectés.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::assets;
use crate::cache;
use crate::error::{log_err, AppError, AppResult};
use crate::state::AppState;

const MANIFEST: &str = "assets.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicVariant {
    /// Chemin sur le serveur des assets (`music/<nom>.ogg`).
    pub file: String,
    /// Début et fin de la boucle, en secondes.
    pub loop_start: f64,
    pub loop_end: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicTrack {
    pub id: String,
    pub title: String,
    pub normal: MusicVariant,
    /// `8bit` dans assets.json, `eightBit` côté interface (qui le renvoie à `music_files`).
    #[serde(rename(deserialize = "8bit", serialize = "eightBit"), alias = "eightBit")]
    pub eight_bit: MusicVariant,
}

#[derive(Deserialize)]
struct Manifest {
    music: Vec<MusicTrack>,
}

/// Fichiers locaux d'un morceau, prêts à être lus.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicFiles {
    pub normal: String,
    pub eight_bit: String,
}

/// Un nom simple sous `music/` : pas de sous-dossier, de fichier caché ni de `..`.
fn valid_file(path: &str) -> bool {
    let Some(name) = path.strip_prefix("music/") else { return false };
    name.ends_with(".ogg")
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !name.contains("..")
}

fn valid_variant(v: &MusicVariant) -> bool {
    valid_file(&v.file) && v.loop_start >= 0.0 && v.loop_end > v.loop_start
}

fn parse_manifest(text: &str) -> AppResult<Vec<MusicTrack>> {
    let manifest: Manifest =
        serde_json::from_str(text).map_err(|e| log_err(AppError::Server, "assets.json invalide", e))?;
    Ok(manifest
        .music
        .into_iter()
        .filter(|t| {
            let ok = valid_variant(&t.normal) && valid_variant(&t.eight_bit);
            if !ok {
                tracing::warn!("morceau {} ignoré : fichier ou boucle invalide", t.id);
            }
            ok
        })
        .collect())
}

fn manifest_cache(state: &AppState) -> std::path::PathBuf {
    state.cache_dir.join("music").join(MANIFEST)
}

/// Liste des morceaux : celle du serveur, sinon la dernière connue (hors ligne).
#[tauri::command]
pub async fn music_tracks(state: State<'_, AppState>) -> AppResult<Vec<MusicTrack>> {
    let Some(base) = assets::base_url(&state.settings()) else {
        return Ok(Vec::new());
    };
    let client = state.client()?.with_base(&base);
    let cached = manifest_cache(&state);
    let text = match client.send(client.get(MANIFEST)).await {
        Ok(resp) => {
            let text = resp.text().await.map_err(|e| log_err(AppError::Server, "lecture de assets.json", e))?;
            let tracks = parse_manifest(&text)?;
            if let Some(dir) = cached.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&cached, &text) {
                tracing::warn!("assets.json non mis en cache: {e}");
            }
            return Ok(tracks);
        }
        Err(e) => match std::fs::read_to_string(&cached) {
            Ok(text) => {
                tracing::info!("assets.json injoignable ({}), copie en cache utilisée", e.code());
                text
            }
            Err(_) => return Err(e),
        },
    };
    parse_manifest(&text)
}

/// Télécharge (ou reprend du cache) les deux versions d'un morceau.
#[tauri::command]
pub async fn music_files(state: State<'_, AppState>, track: MusicTrack) -> AppResult<MusicFiles> {
    if !valid_variant(&track.normal) || !valid_variant(&track.eight_bit) {
        return Err(AppError::Internal);
    }
    let base = assets::base_url(&state.settings()).ok_or(AppError::NoServer)?;
    let client = state.client()?.with_base(&base);
    let dir = state.cache_dir.join("music");
    let get = |file: &str| {
        let key = format!("{base}/{file}");
        let (client, dir, file) = (client.clone(), dir.clone(), file.to_string());
        async move { cache::cached_download_checked(&client, &dir, &key, &file, "ogg", b"OggS").await }
    };
    let (normal, eight_bit) = tokio::try_join!(get(&track.normal.file), get(&track.eight_bit.file))?;
    Ok(MusicFiles {
        normal: normal.to_string_lossy().into_owned(),
        eight_bit: eight_bit.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_server_manifest() {
        let text = r#"{"music":[{"id":"x","title":"X",
            "normal":{"file":"music/x.ogg","loopStart":1.5,"loopEnd":10,"sampleRate":48000},
            "8bit":{"file":"music/x-8bit.ogg","loopStart":0.5,"loopEnd":9}}]}"#;
        let tracks = parse_manifest(text).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].eight_bit.loop_start, 0.5);
        let out = serde_json::to_value(&tracks[0]).unwrap();
        assert!(out.get("eightBit").is_some());
        // L'interface renvoie le morceau tel qu'elle l'a reçu.
        let back: MusicTrack = serde_json::from_value(out).unwrap();
        assert_eq!(back.eight_bit.file, "music/x-8bit.ogg");
    }

    #[test]
    fn rejects_unsafe_files() {
        assert!(valid_file("music/a-beautiful-song-8bit.ogg"));
        for bad in ["music/../x.ogg", "music/.x.ogg", "music/a/b.ogg", "sfx/x.ogg", "music/x.wav", "/music/x.ogg"] {
            assert!(!valid_file(bad), "{bad}");
        }
    }

    #[test]
    fn skips_tracks_with_bad_loops() {
        let text = r#"{"music":[{"id":"x","title":"X",
            "normal":{"file":"music/x.ogg","loopStart":10,"loopEnd":5},
            "8bit":{"file":"music/y.ogg","loopStart":0,"loopEnd":9}}]}"#;
        assert!(parse_manifest(text).unwrap().is_empty());
    }
}
