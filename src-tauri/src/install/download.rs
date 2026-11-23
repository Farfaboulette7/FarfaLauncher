//! Téléchargement d'un fichier RomM avec reprise (`Range`), détection des
//! connexions bloquées et vérification d'intégrité (SHA-1 / MD5 si RomM les
//! fournit, taille sinon).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use md5::Md5;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::{error_chain, RommClient};
use crate::romm::library::{file_content_path, RemoteFile};

/// Délai sans recevoir un octet avant d'abandonner (connexion bloquée).
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Nombre de reprises automatiques après une coupure.
const RETRIES: u32 = 3;

/// Limite de débit des téléchargements, en octets par seconde (0 : illimité). Changée par
/// les Paramètres, elle s'applique aussi au téléchargement en cours.
static LIMIT: AtomicU64 = AtomicU64::new(0);

/// Limite de débit en Mo/s (0 : illimité).
pub fn set_limit_mb(mb: u32) {
    LIMIT.store(u64::from(mb) * 1024 * 1024, Ordering::Relaxed);
}

/// Attente à faire pour rester sous `limit` octets/s, après `bytes` octets reçus en `elapsed`.
fn throttle_wait(bytes: u64, elapsed: Duration, limit: u64) -> Option<Duration> {
    if limit == 0 {
        return None;
    }
    let expected = Duration::from_secs_f64(bytes as f64 / limit as f64);
    expected.checked_sub(elapsed).filter(|d| *d >= Duration::from_millis(5))
}

/// Télécharge `file` vers `dest` (via `dest.part`, repris s'il existe).
/// `on_progress(octets reçus au total)` est appelé régulièrement.
pub async fn download(
    client: &RommClient,
    file: &RemoteFile,
    dest: &Path,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u64),
    on_verify: impl FnOnce(),
) -> AppResult<()> {
    download_from(client, &file_content_path(file), file, dest, cancel, on_progress, on_verify).await
}

/// Comme [`download`], depuis RomM ou FarfaRomManager selon l'origine du fichier.
pub async fn download_any(
    client: &RommClient,
    manager_base: Option<&str>,
    file: &RemoteFile,
    dest: &Path,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u64),
    on_verify: impl FnOnce(),
) -> AppResult<()> {
    if !file.from_manager {
        return download(client, file, dest, cancel, on_progress, on_verify).await;
    }
    let base = manager_base.ok_or(AppError::Download)?;
    let sha1 = file.sha1.as_deref().ok_or(AppError::Download)?;
    download_from(&client.with_base(base), &format!("files/{sha1}"), file, dest, cancel, on_progress, on_verify).await
}

/// Comme [`download`], depuis un chemin donné (fichier de FarfaRomManager).
pub async fn download_from(
    client: &RommClient,
    path: &str,
    file: &RemoteFile,
    dest: &Path,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64),
    on_verify: impl FnOnce(),
) -> AppResult<()> {
    if is_complete(dest, file).await {
        on_progress(file.size);
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| log_err(AppError::Disk, "création du dossier de téléchargement", e))?;
    }
    let part = part_path(dest);
    let mut attempt = 0;
    loop {
        match download_once(client, path, file, &part, cancel, &mut on_progress).await {
            Ok(()) => break,
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(e @ (AppError::AuthInvalid | AppError::Forbidden | AppError::Disk)) => return Err(e),
            Err(e) if attempt < RETRIES => {
                attempt += 1;
                tracing::warn!("téléchargement de {} interrompu ({}), reprise {attempt}/{RETRIES}", file.file_name, e.code());
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
            }
            Err(e) => return Err(e),
        }
    }
    on_verify();
    verify(&part, file).await?;
    tokio::fs::rename(&part, dest).await.map_err(|e| log_err(AppError::Disk, "finalisation du téléchargement", e))
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Un fichier déjà téléchargé et vérifié lors d'une tentative précédente.
async fn is_complete(dest: &Path, file: &RemoteFile) -> bool {
    match tokio::fs::metadata(dest).await {
        Ok(m) if m.len() == file.size => verify(dest, file).await.is_ok(),
        _ => false,
    }
}

async fn download_once(
    client: &RommClient,
    path: &str,
    file: &RemoteFile,
    part: &Path,
    cancel: &CancellationToken,
    on_progress: &mut impl FnMut(u64),
) -> AppResult<()> {
    let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
    if have > file.size {
        have = 0;
    }
    if have == file.size && file.size > 0 {
        return Ok(());
    }
    let mut req = client.get_stream(path);
    if have > 0 {
        req = req.header(RANGE, format!("bytes={have}-"));
    }
    // Délai d'attente de la réponse (en-têtes) ; le corps a ensuite son délai par morceau (L6).
    let resp = match tokio::time::timeout(STALL_TIMEOUT, req.send()).await {
        Ok(sent) => sent.map_err(|e| log_err(AppError::Download, "requête de téléchargement", error_chain(&e)))?,
        Err(_) => return Err(log_err(AppError::Download, "requête de téléchargement", "aucune réponse depuis 60 s")),
    };
    if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        // Fichier .part incohérent : on repart de zéro.
        let _ = tokio::fs::remove_file(part).await;
        return Err(AppError::Download);
    }
    let resp = client.check(resp).await.map_err(|e| match e {
        AppError::Server => AppError::Download,
        other => other,
    })?;
    let resumed = resp.status() == StatusCode::PARTIAL_CONTENT && resp.headers().contains_key(CONTENT_RANGE);
    if have > 0 && !resumed {
        tracing::info!("le serveur ne reprend pas {}, téléchargement complet", file.file_name);
        have = 0;
    }
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(part)
        .await
        .map_err(|e| log_err(AppError::Disk, "ouverture du fichier de téléchargement", e))?;

    let mut received = have;
    on_progress(received);
    let mut stream = resp.bytes_stream();
    // Mesure du débit depuis le dernier changement de limite.
    let (mut limit, mut since, mut counted) = (LIMIT.load(Ordering::Relaxed), Instant::now(), 0u64);
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => {
                out.flush().await.ok();
                return Err(AppError::Cancelled);
            }
            r = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => r,
        };
        let chunk = match next {
            Err(_) => return Err(log_err(AppError::Download, "téléchargement", "aucune donnée reçue depuis 60 s")),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(log_err(AppError::Download, "téléchargement", error_chain(&e))),
            Ok(Some(Ok(c))) => c,
        };
        out.write_all(&chunk).await.map_err(|e| disk_error("écriture du téléchargement", e))?;
        received += chunk.len() as u64;
        on_progress(received);
        let now_limit = LIMIT.load(Ordering::Relaxed);
        if now_limit != limit {
            (limit, since, counted) = (now_limit, Instant::now(), 0);
        }
        counted += chunk.len() as u64;
        if let Some(wait) = throttle_wait(counted, since.elapsed(), limit) {
            tokio::select! {
                _ = cancel.cancelled() => {
                    out.flush().await.ok();
                    return Err(AppError::Cancelled);
                }
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
    out.flush().await.map_err(|e| disk_error("écriture du téléchargement", e))?;
    if received != file.size {
        return Err(log_err(AppError::Download, "téléchargement", format!("{received} octets reçus sur {}", file.size)));
    }
    Ok(())
}

pub fn disk_error(context: &str, e: std::io::Error) -> AppError {
    // ERROR_DISK_FULL (112) sous Windows, ENOSPC (28) ailleurs.
    let full = matches!(e.raw_os_error(), Some(112) | Some(28)) || e.kind() == std::io::ErrorKind::StorageFull;
    log_err(if full { AppError::DiskFull } else { AppError::Disk }, context, e)
}

/// Vérifie la taille, puis le SHA-1 (ou à défaut le MD5) fourni par RomM.
async fn verify(path: &Path, file: &RemoteFile) -> AppResult<()> {
    let len = tokio::fs::metadata(path).await.map(|m| m.len()).unwrap_or(0);
    if len != file.size {
        return Err(log_err(AppError::Integrity, "vérification", format!("taille {len} au lieu de {}", file.size)));
    }
    // Pour les fichiers compressés, RomM hache le contenu décompressé, pas le
    // fichier : la comparaison est impossible. On s'en tient à la taille ; les
    // CRC internes du zip sont contrôlés à l'extraction.
    if is_compressed_container(path, &file.file_name).await {
        return Ok(());
    }
    let (expected, algo) = match (&file.sha1, &file.md5) {
        (Some(h), _) => (h.to_ascii_lowercase(), "sha1"),
        (None, Some(h)) => (h.to_ascii_lowercase(), "md5"),
        (None, None) => return Ok(()),
    };
    let mut f = tokio::fs::File::open(path).await.map_err(|e| log_err(AppError::Disk, "lecture pour vérification", e))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut sha = Sha1::new();
    let mut md = Md5::new();
    loop {
        let n = f.read(&mut buf).await.map_err(|e| log_err(AppError::Disk, "lecture pour vérification", e))?;
        if n == 0 {
            break;
        }
        if algo == "sha1" {
            sha.update(&buf[..n]);
        } else {
            md.update(&buf[..n]);
        }
    }
    let actual: String = if algo == "sha1" { hex(&sha.finalize()) } else { hex(&md.finalize()) };
    if actual != expected {
        let _ = tokio::fs::remove_file(path).await;
        return Err(log_err(AppError::Integrity, "vérification", format!("{algo} {actual} ≠ {expected} pour {}", file.file_name)));
    }
    Ok(())
}

/// Fichier que RomM hache après décompression (cf. `_calculate_rom_hashes`) :
/// reconnu à son extension ou à sa signature (zip, gzip, 7z, bzip2, tar).
async fn is_compressed_container(path: &Path, name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if [".zip", ".tar", ".gz", ".tgz", ".7z", ".bz2"].iter().any(|e| lower.ends_with(e)) {
        return true;
    }
    let mut head = [0u8; 262];
    let n = match tokio::fs::File::open(path).await {
        Ok(mut f) => f.read(&mut head).await.unwrap_or(0),
        Err(_) => 0,
    };
    let h = &head[..n];
    h.starts_with(b"PK\x03\x04")
        || h.starts_with(&[0x1f, 0x8b])
        || h.starts_with(b"7z\xbc\xaf\x27\x1c")
        || h.starts_with(b"BZh")
        || (n >= 262 && &h[257..262] == b"ustar")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod throttle_tests {
    use super::*;

    #[test]
    fn waits_only_when_ahead_of_the_limit() {
        let mb = 1024 * 1024;
        assert_eq!(throttle_wait(10 * mb, Duration::from_secs(1), 0), None);
        // 10 Mo reçus en 1 s avec une limite de 5 Mo/s : 1 s d'attente.
        assert_eq!(throttle_wait(10 * mb, Duration::from_secs(1), 5 * mb), Some(Duration::from_secs(1)));
        // Déjà en retard sur la limite : pas d'attente.
        assert_eq!(throttle_wait(mb, Duration::from_secs(2), 5 * mb), None);
    }
}
