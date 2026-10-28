//! Téléchargements hors RomM (GitHub, CDN des clients Call of Duty, Steam) :
//! sans authentification, avec reprise (`.part`) et progression.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};

const USER_AGENT: &str = concat!("FarfaLauncher/", env!("CARGO_PKG_VERSION"));
/// Délai sans recevoir un octet avant d'abandonner.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .expect("client HTTP")
}

/// Petit document (JSON, texte) : délai court.
pub async fn get_text(http: &reqwest::Client, url: &str) -> AppResult<String> {
    let resp = http
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| log_err(AppError::Download, &format!("GET {url}"), e))?;
    if !resp.status().is_success() {
        tracing::warn!("GET {url}: {}", resp.status());
        return Err(AppError::Download);
    }
    resp.text().await.map_err(|e| log_err(AppError::Download, &format!("lecture de {url}"), e))
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Télécharge `url` vers `dest` (repris depuis `dest.part` s'il existe).
/// `on_progress(reçu, total)` ; `total` vaut 0 s'il est inconnu.
pub async fn download(
    http: &reqwest::Client,
    url: &str,
    dest: &Path,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> AppResult<()> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| log_err(AppError::Disk, "dossier de téléchargement", e))?;
    }
    let part = part_path(dest);
    let mut have = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
    let mut req = http.get(url);
    if have > 0 {
        req = req.header(RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| log_err(AppError::Download, &format!("GET {url}"), e))?;
    let status = resp.status();
    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        // Le .part dépasse le fichier : on recommence.
        let _ = tokio::fs::remove_file(&part).await;
        return Box::pin(download(http, url, dest, cancel, on_progress)).await;
    }
    if !status.is_success() {
        tracing::warn!("téléchargement {url}: {status}");
        return Err(AppError::Download);
    }
    let resumed = status == StatusCode::PARTIAL_CONTENT && resp.headers().contains_key(CONTENT_RANGE);
    if !resumed {
        have = 0;
    }
    let total = resp.content_length().map(|l| l + have).unwrap_or(0);
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)
        .await
        .map_err(|e| log_err(AppError::Disk, "écriture du téléchargement", e))?;
    let mut stream = resp.bytes_stream();
    let mut got = have;
    on_progress(got, total);
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            n = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => n,
        };
        let chunk = match next {
            Err(_) => {
                tracing::warn!("téléchargement {url} bloqué");
                return Err(AppError::Download);
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(log_err(AppError::Download, &format!("téléchargement {url}"), e)),
            Ok(Some(Ok(c))) => c,
        };
        out.write_all(&chunk).await.map_err(|e| crate::install::download::disk_error("écriture du téléchargement", e))?;
        got += chunk.len() as u64;
        on_progress(got, total);
    }
    out.flush().await.map_err(|e| log_err(AppError::Disk, "écriture du téléchargement", e))?;
    drop(out);
    if total > 0 && got != total {
        tracing::warn!("téléchargement {url} incomplet: {got}/{total}");
        return Err(AppError::Download);
    }
    tokio::fs::rename(&part, dest).await.map_err(|e| log_err(AppError::Disk, "finalisation du téléchargement", e))
}

/// Encode chaque segment d'un chemin relatif pour une URL.
pub fn encode_path(rel: &str) -> String {
    rel.split('/')
        .map(|s| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20"))
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_segments() {
        assert_eq!(super::encode_path("iw4x/html/a b.png"), "iw4x/html/a%20b.png");
    }
}
