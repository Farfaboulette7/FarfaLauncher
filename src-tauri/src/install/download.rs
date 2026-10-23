//! Téléchargement d'un fichier RomM avec reprise (`Range`), détection des
//! connexions bloquées et vérification d'intégrité (SHA-1 / MD5 si RomM les
//! fournit, taille sinon).

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use md5::Md5;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::romm::client::{check_status, error_chain, RommClient};
use crate::romm::library::{file_content_path, RemoteFile};

/// Délai sans recevoir un octet avant d'abandonner (connexion bloquée).
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Nombre de reprises automatiques après une coupure.
const RETRIES: u32 = 3;

/// Télécharge `file` vers `dest` (via `dest.part`, repris s'il existe).
/// `on_progress(octets reçus au total)` est appelé régulièrement.
pub async fn download(
    client: &RommClient,
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
        match download_once(client, file, &part, cancel, &mut on_progress).await {
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
    let mut req = client.get_stream(&file_content_path(file));
    if have > 0 {
        req = req.header(RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| log_err(AppError::Download, "requête de téléchargement", error_chain(&e)))?;
    if resp.status() == StatusCode::RANGE_NOT_SATISFIABLE {
        // Fichier .part incohérent : on repart de zéro.
        let _ = tokio::fs::remove_file(part).await;
        return Err(AppError::Download);
    }
    let resp = check_status(resp).await.map_err(|e| match e {
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
