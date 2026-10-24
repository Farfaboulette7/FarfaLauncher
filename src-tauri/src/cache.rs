//! Cache local : métadonnées de la bibliothèque (JSON) et fichiers distants
//! (jaquettes, captures, livrets) pour un affichage rapide et hors ligne.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::RommClient;
use crate::romm::library::{Game, Platform};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySnapshot {
    pub server_url: String,
    /// Horodatage Unix (secondes) du dernier rafraîchissement.
    pub fetched_at: u64,
    #[serde(default)]
    pub platforms: Vec<Platform>,
    pub games: Vec<Game>,
}

fn library_file(cache_dir: &Path) -> PathBuf {
    cache_dir.join("library.json")
}

pub fn load_library(cache_dir: &Path, server_url: &str) -> Option<LibrarySnapshot> {
    let text = std::fs::read_to_string(library_file(cache_dir)).ok()?;
    let snap: LibrarySnapshot = serde_json::from_str(&text)
        .map_err(|e| tracing::warn!("cache bibliothèque illisible: {e}"))
        .ok()?;
    // Cache d'avant les plateformes `portage-*` : ignoré, la liste est rechargée.
    (snap.server_url == server_url && !snap.platforms.is_empty()).then_some(snap)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FavoritesCache {
    server_url: String,
    rom_ids: Vec<i64>,
}

fn favorites_file(cache_dir: &Path) -> PathBuf {
    cache_dir.join("favorites.json")
}

/// Derniers favoris connus (affichage hors ligne).
pub fn load_favorites(cache_dir: &Path, server_url: &str) -> Option<Vec<i64>> {
    let text = std::fs::read_to_string(favorites_file(cache_dir)).ok()?;
    let c: FavoritesCache = serde_json::from_str(&text).ok()?;
    (c.server_url == server_url).then_some(c.rom_ids)
}

pub fn save_favorites(cache_dir: &Path, server_url: &str, rom_ids: &[i64]) {
    let c = FavoritesCache { server_url: server_url.to_string(), rom_ids: rom_ids.to_vec() };
    let res = std::fs::create_dir_all(cache_dir)
        .and_then(|_| std::fs::write(favorites_file(cache_dir), serde_json::to_string(&c).unwrap_or_default()));
    if let Err(e) = res {
        tracing::warn!("favoris non mis en cache: {e}");
    }
}

pub fn save_library(cache_dir: &Path, snap: &LibrarySnapshot) -> AppResult<()> {
    std::fs::create_dir_all(cache_dir).map_err(|e| log_err(AppError::Cache, "création du cache", e))?;
    let text = serde_json::to_string(snap).map_err(|e| log_err(AppError::Cache, "sérialisation du cache", e))?;
    let path = library_file(cache_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| log_err(AppError::Cache, "écriture du cache", e))
}

pub fn clear(cache_dir: &Path) -> AppResult<()> {
    match std::fs::remove_dir_all(cache_dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(log_err(AppError::Cache, "suppression du cache", e)),
    }
}

/// Anciennes versions : cache rangé directement dans le dossier de cache de
/// l'application. On n'efface que nos propres entrées, jamais le reste
/// (logs, données du WebView).
pub fn remove_legacy_entries(app_cache: &Path) {
    for dir in ["images", "manuals"] {
        let _ = std::fs::remove_dir_all(app_cache.join(dir));
    }
    let _ = std::fs::remove_file(app_cache.join("library.json"));
}

pub fn hash_name(key: &str) -> String {
    let digest = Sha1::digest(key.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn extension_of(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => ext,
        _ => "bin",
    }
}

/// Chemin local d'un fichier distant, téléchargé s'il n'est pas déjà en cache.
/// `key` identifie la version du fichier (URL avec cache-busting, id + date…).
pub async fn cached_download(
    client: &RommClient,
    dir: &Path,
    key: &str,
    url_path: &str,
    ext_hint: Option<&str>,
) -> AppResult<PathBuf> {
    let ext = ext_hint.unwrap_or_else(|| extension_of(url_path)).to_ascii_lowercase();
    let path = dir.join(format!("{}.{ext}", hash_name(key)));
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(path);
    }
    tokio::fs::create_dir_all(dir).await.map_err(|e| log_err(AppError::Cache, "création du cache", e))?;
    let resp = client.send(client.get_stream(url_path)).await?;
    // Nom temporaire unique : deux demandes simultanées du même fichier ne se marchent pas dessus.
    let tmp = path.with_extension(format!("{ext}.part-{}", uuid::Uuid::new_v4().simple()));
    let mut out = tokio::fs::File::create(&tmp).await.map_err(|e| log_err(AppError::Cache, "fichier de cache", e))?;
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| log_err(AppError::Download, "téléchargement vers le cache", e))?;
        out.write_all(&chunk).await.map_err(|e| log_err(AppError::Cache, "écriture du cache", e))?;
    }
    out.flush().await.map_err(|e| log_err(AppError::Cache, "écriture du cache", e))?;
    drop(out);
    if let Err(e) = tokio::fs::rename(&tmp, &path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        // Une autre demande l'a déjà posé : c'est le même fichier.
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Err(log_err(AppError::Cache, "finalisation du cache", e));
        }
    }
    Ok(path)
}

async fn starts_with(path: &Path, magic: &[u8]) -> bool {
    let mut head = vec![0u8; magic.len()];
    match tokio::fs::File::open(path).await {
        Ok(mut f) => f.read_exact(&mut head).await.is_ok() && head == magic,
        Err(_) => false,
    }
}

/// `cached_download` pour un fichier à signature connue (`RIFF`, `OggS`…). Un fichier
/// qui ne la porte pas (ex. page HTML d'un reverse proxy renvoyée avec un 200) est
/// retiré du cache et retéléchargé une fois.
pub async fn cached_download_checked(
    client: &RommClient,
    dir: &Path,
    key: &str,
    url_path: &str,
    ext: &str,
    magic: &[u8],
) -> AppResult<PathBuf> {
    for attempt in 0..2 {
        let path = cached_download(client, dir, key, url_path, Some(ext)).await?;
        if starts_with(&path, magic).await {
            return Ok(path);
        }
        let _ = tokio::fs::remove_file(&path).await;
        tracing::warn!("{url_path} : réponse inattendue (pas un fichier .{ext}), retirée du cache (essai {})", attempt + 1);
    }
    Err(AppError::Download)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_only_touches_our_cache() {
        let root = std::env::temp_dir().join(format!("farfa-cache-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::create_dir_all(root.join("EBWebView")).unwrap();
        std::fs::create_dir_all(root.join("cache/images")).unwrap();
        std::fs::write(root.join("library.json"), "{}").unwrap();
        clear(&root.join("cache")).unwrap();
        remove_legacy_entries(&root);
        assert!(root.join("logs").exists() && root.join("EBWebView").exists());
        assert!(!root.join("cache").exists() && !root.join("library.json").exists());
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_of("https://x/assets/romm/resources/roms/1/2/cover/big.png?ts=2026"), "png");
        assert_eq!(extension_of("/a/b/noext"), "bin");
        assert_eq!(extension_of("/a/b/c.webp#x"), "webp");
    }
}
