//! Extraction des archives RECOMP et fusion « sans casse » dans le dossier du jeu.
//!
//! Règles (cahier des charges, mises à jour) :
//! 1. ne jamais toucher aux dossiers/fichiers `save` / `saves` (à toute
//!    profondeur, casse ignorée), ni à `mods_downloads/` ;
//! 2. n'écraser que les fichiers présents dans la nouvelle archive, ne jamais
//!    supprimer un fichier absent de l'archive ;
//! 3. sauvegarder les dossiers `save`/`saves` avant une mise à jour.
//!
//! Toutes ces fonctions sont bloquantes : à appeler dans `spawn_blocking`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::install::download::disk_error;
use crate::platform::ArchiveKind;

/// Dossier interne du launcher dans chaque dossier de jeu.
pub const INTERNAL_DIR: &str = ".farfalauncher";
pub const STATE_FILE: &str = "farfalauncher.json";
pub const MODS_DIR: &str = "mods_downloads";

/// Chemin (relatif au dossier du jeu) que le launcher ne doit jamais écraser.
pub fn is_protected(rel: &Path) -> bool {
    let mut comps = rel.components().filter_map(|c| match c {
        Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
        _ => None,
    });
    let Some(first) = comps.next() else { return false };
    if first == MODS_DIR || first == INTERNAL_DIR || first == STATE_FILE {
        return true;
    }
    std::iter::once(first).chain(comps).any(|c| c == "save" || c == "saves")
}

/// Taille décompressée totale (pour vérifier l'espace disque avant extraction).
pub fn uncompressed_size(archive: &Path, kind: ArchiveKind) -> AppResult<u64> {
    match kind {
        ArchiveKind::Zip => {
            let f = fs::File::open(archive).map_err(|e| log_err(AppError::Disk, "ouverture de l'archive", e))?;
            let mut zip = zip::ZipArchive::new(f).map_err(|e| log_err(AppError::Extract, "lecture du zip", e))?;
            let mut total = 0u64;
            for i in 0..zip.len() {
                if let Ok(entry) = zip.by_index_raw(i) {
                    total += entry.size();
                }
            }
            Ok(total)
        }
        // Inconnue sans tout décompresser : estimation prudente.
        ArchiveKind::TarGz => Ok(fs::metadata(archive).map(|m| m.len() * 3).unwrap_or(0)),
        ArchiveKind::AppImage => Ok(fs::metadata(archive).map(|m| m.len()).unwrap_or(0)),
    }
}

/// Décompresse `archive` dans `staging` (vidé au préalable).
/// `progress(fait, total)` compte les entrées traitées.
pub fn extract(
    archive: &Path,
    kind: ArchiveKind,
    staging: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> AppResult<()> {
    if staging.exists() {
        fs::remove_dir_all(staging).map_err(|e| log_err(AppError::Disk, "nettoyage du dossier temporaire", e))?;
    }
    fs::create_dir_all(staging).map_err(|e| disk_error("création du dossier temporaire", e))?;
    match kind {
        ArchiveKind::Zip => extract_zip(archive, staging, cancel, &mut progress),
        ArchiveKind::TarGz => extract_tar_gz(archive, staging, cancel, &mut progress),
        ArchiveKind::AppImage => {
            let name = archive.file_name().ok_or(AppError::Extract)?;
            fs::copy(archive, staging.join(name)).map_err(|e| disk_error("copie de l'AppImage", e))?;
            progress(1, 1);
            Ok(())
        }
    }
}

fn skip_entry(rel: &Path) -> bool {
    rel.components().any(|c| matches!(c, Component::Normal(s) if s == "__MACOSX" || s == ".DS_Store"))
}

fn extract_zip(archive: &Path, staging: &Path, cancel: &CancellationToken, progress: &mut impl FnMut(u64, u64)) -> AppResult<()> {
    let f = fs::File::open(archive).map_err(|e| log_err(AppError::Disk, "ouverture de l'archive", e))?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(f)).map_err(|e| log_err(AppError::Extract, "lecture du zip", e))?;
    let total = zip.len() as u64;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = zip.by_index(i).map_err(|e| log_err(AppError::Extract, "entrée du zip", e))?;
        // `enclosed_name` refuse les chemins absolus et les `..` (zip slip).
        let Some(rel) = entry.enclosed_name() else {
            tracing::warn!("entrée de zip ignorée (chemin dangereux): {}", entry.name().map(|n| n.to_string()).unwrap_or_default());
            continue;
        };
        if skip_entry(&rel) {
            continue;
        }
        let out = staging.join(&rel);
        let mode = entry.unix_mode();
        let is_symlink = mode.map(|m| m & 0o170000 == 0o120000).unwrap_or(false);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| disk_error("création d'un dossier", e))?;
        } else if is_symlink {
            // Liens symboliques des bundles macOS (frameworks).
            let mut target = String::new();
            io::Read::read_to_string(&mut entry, &mut target).map_err(|e| log_err(AppError::Extract, "lien du zip", e))?;
            make_symlink(&target, &out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| disk_error("création d'un dossier", e))?;
            }
            let mut w = fs::File::create(&out).map_err(|e| disk_error("écriture d'un fichier", e))?;
            io::copy(&mut entry, &mut w).map_err(|e| match e.kind() {
                io::ErrorKind::InvalidData => log_err(AppError::Extract, "décompression", e),
                _ => disk_error("écriture d'un fichier", e),
            })?;
            #[cfg(unix)]
            if let Some(m) = mode {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(m & 0o7777));
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        progress(i as u64 + 1, total);
    }
    Ok(())
}

fn extract_tar_gz(archive: &Path, staging: &Path, cancel: &CancellationToken, progress: &mut impl FnMut(u64, u64)) -> AppResult<()> {
    let f = fs::File::open(archive).map_err(|e| log_err(AppError::Disk, "ouverture de l'archive", e))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(f)));
    tar.set_preserve_permissions(true);
    let mut done = 0u64;
    for entry in tar.entries().map_err(|e| log_err(AppError::Extract, "lecture du tar.gz", e))? {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = entry.map_err(|e| log_err(AppError::Extract, "entrée du tar.gz", e))?;
        let rel = entry.path().map(|p| p.into_owned()).unwrap_or_default();
        if skip_entry(&rel) {
            continue;
        }
        // `unpack_in` refuse les chemins qui sortent du dossier cible.
        entry.unpack_in(staging).map_err(|e| disk_error("extraction du tar.gz", e))?;
        done += 1;
        progress(done, 0);
    }
    Ok(())
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) -> AppResult<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(|e| disk_error("création d'un dossier", e))?;
    }
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink(target, link).map_err(|e| disk_error("création d'un lien", e))
}

#[cfg(not(unix))]
fn make_symlink(target: &str, link: &Path) -> AppResult<()> {
    // Sans intérêt sous Windows (liens des bundles macOS) : ignoré.
    tracing::debug!("lien symbolique ignoré: {} -> {target}", link.display());
    Ok(())
}

/// Si l'archive ne contient qu'un dossier racine (hors bundle `.app`), on
/// travaille à l'intérieur de celui-ci (aplatissement).
pub fn content_root(staging: &Path) -> AppResult<PathBuf> {
    let entries: Vec<_> = fs::read_dir(staging)
        .map_err(|e| log_err(AppError::Disk, "lecture du dossier temporaire", e))?
        .filter_map(Result::ok)
        .collect();
    if let [only] = entries.as_slice() {
        let name = only.file_name().to_string_lossy().to_lowercase();
        let is_dir = only.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && !name.ends_with(".app") {
            tracing::info!("archive à dossier racine unique « {name} » : aplatie");
            return Ok(only.path());
        }
    }
    Ok(staging.to_path_buf())
}

/// Fusionne `src` dans `dest` : écrase les fichiers de l'archive, ne supprime
/// rien, ne touche à aucun chemin protégé. Renvoie le nombre de fichiers posés.
pub fn merge_into(src: &Path, dest: &Path) -> AppResult<u64> {
    fs::create_dir_all(dest).map_err(|e| disk_error("création du dossier du jeu", e))?;
    let mut count = 0;
    merge_dir(src, dest, Path::new(""), &mut count)?;
    Ok(count)
}

fn merge_dir(src_root: &Path, dest_root: &Path, rel: &Path, count: &mut u64) -> AppResult<()> {
    let dir = src_root.join(rel);
    for entry in fs::read_dir(&dir).map_err(|e| log_err(AppError::Disk, "lecture de l'archive extraite", e))? {
        let entry = entry.map_err(|e| log_err(AppError::Disk, "lecture de l'archive extraite", e))?;
        let child_rel = rel.join(entry.file_name());
        if is_protected(&child_rel) {
            tracing::info!("chemin protégé conservé tel quel: {}", child_rel.display());
            continue;
        }
        let ft = entry.file_type().map_err(|e| log_err(AppError::Disk, "type de fichier", e))?;
        let target = dest_root.join(&child_rel);
        if ft.is_dir() {
            if target.is_file() {
                fs::remove_file(&target).map_err(|e| disk_error("remplacement d'un fichier", e))?;
            }
            fs::create_dir_all(&target).map_err(|e| disk_error("création d'un dossier", e))?;
            merge_dir(src_root, dest_root, &child_rel, count)?;
        } else {
            if target.is_dir() {
                // Un dossier remplacé par un fichier dans la nouvelle version : on ne
                // supprime pas un dossier existant (il peut contenir des données).
                tracing::warn!("conflit dossier/fichier, conservé: {}", child_rel.display());
                continue;
            }
            if ft.is_symlink() || target.is_symlink() {
                let _ = fs::remove_file(&target);
            }
            // Même volume (dossier interne du jeu) : simple renommage.
            if fs::rename(entry.path(), &target).is_err() {
                fs::copy(entry.path(), &target).map_err(|e| disk_error("copie d'un fichier du jeu", e))?;
            }
            *count += 1;
        }
    }
    Ok(())
}

/// Copie les dossiers `save`/`saves` du jeu dans `backup_dir` (avant une mise à jour).
/// Renvoie le nombre de dossiers sauvegardés.
pub fn backup_saves(game_dir: &Path, backup_dir: &Path) -> AppResult<u32> {
    let mut found = Vec::new();
    find_save_dirs(game_dir, Path::new(""), &mut found);
    for rel in &found {
        copy_dir(&game_dir.join(rel), &backup_dir.join(rel))?;
    }
    Ok(found.len() as u32)
}

fn find_save_dirs(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root.join(rel)) else { return };
    for entry in rd.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let child = rel.join(entry.file_name());
        if rel.as_os_str().is_empty() && (name == INTERNAL_DIR || name == MODS_DIR) {
            continue;
        }
        if name == "save" || name == "saves" {
            out.push(child);
        } else {
            find_save_dirs(root, &child, out);
        }
    }
}

pub fn copy_dir(src: &Path, dest: &Path) -> AppResult<()> {
    fs::create_dir_all(dest).map_err(|e| disk_error("création d'un dossier", e))?;
    for entry in fs::read_dir(src).map_err(|e| log_err(AppError::Disk, "lecture d'un dossier", e))?.filter_map(Result::ok) {
        let target = dest.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| log_err(AppError::Disk, "type de fichier", e))?;
        if ft.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ft.is_file() {
            fs::copy(entry.path(), &target).map_err(|e| disk_error("copie d'un fichier", e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("farfa-test-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn protected_paths() {
        assert!(is_protected(Path::new("save")));
        assert!(is_protected(Path::new("Saves/slot1.dat")));
        assert!(is_protected(Path::new("data/SAVE/x")));
        assert!(is_protected(Path::new("mods_downloads/mod.zip")));
        assert!(is_protected(Path::new(".farfalauncher/staging")));
        assert!(is_protected(Path::new("farfalauncher.json")));
        assert!(!is_protected(Path::new("data/mods_downloads/x")));
        assert!(!is_protected(Path::new("savestate.bin")));
        assert!(!is_protected(Path::new("game.exe")));
    }

    #[test]
    fn merge_overwrites_but_never_deletes_nor_touches_saves() {
        let src = tmp("src");
        let dest = tmp("dest");
        fs::write(dest.join("game.exe"), "old").unwrap();
        fs::write(dest.join("user_config.ini"), "mine").unwrap();
        fs::create_dir_all(dest.join("saves")).unwrap();
        fs::write(dest.join("saves/slot1"), "progress").unwrap();

        fs::write(src.join("game.exe"), "new").unwrap();
        fs::create_dir_all(src.join("saves")).unwrap();
        fs::write(src.join("saves/slot1"), "EMPTY").unwrap();
        fs::create_dir_all(src.join("data")).unwrap();
        fs::write(src.join("data/a.bin"), "a").unwrap();

        let n = merge_into(&src, &dest).unwrap();
        assert_eq!(n, 2);
        assert_eq!(fs::read_to_string(dest.join("game.exe")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dest.join("user_config.ini")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(dest.join("saves/slot1")).unwrap(), "progress");
        assert_eq!(fs::read_to_string(dest.join("data/a.bin")).unwrap(), "a");
    }

    #[test]
    fn flattens_single_root_but_not_app_bundles() {
        let s = tmp("flat");
        fs::create_dir_all(s.join("Game-1.0/bin")).unwrap();
        assert_eq!(content_root(&s).unwrap(), s.join("Game-1.0"));
        let s2 = tmp("app");
        fs::create_dir_all(s2.join("Game.app/Contents")).unwrap();
        assert_eq!(content_root(&s2).unwrap(), s2);
        let s3 = tmp("multi");
        fs::create_dir_all(s3.join("a")).unwrap();
        fs::write(s3.join("b.txt"), "").unwrap();
        assert_eq!(content_root(&s3).unwrap(), s3);
    }

    #[test]
    fn zip_roundtrip_skips_macosx_and_slips() {
        let dir = tmp("zip");
        let zpath = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&zpath).unwrap());
            let o: zip::write::SimpleFileOptions = Default::default();
            w.start_file("Game/game.exe", o).unwrap();
            w.write_all(b"exe").unwrap();
            w.start_file("__MACOSX/._junk", o).unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("../evil.txt", o).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let staging = dir.join("staging");
        extract(&zpath, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        assert!(staging.join("Game/game.exe").exists());
        assert!(!staging.join("__MACOSX").exists());
        assert!(!dir.join("evil.txt").exists());
        assert_eq!(content_root(&staging).unwrap(), staging.join("Game"));
        assert_eq!(uncompressed_size(&zpath, ArchiveKind::Zip).unwrap(), 5);
    }

    #[test]
    fn backup_finds_nested_saves() {
        let g = tmp("bk");
        fs::create_dir_all(g.join("user/Saves")).unwrap();
        fs::write(g.join("user/Saves/a"), "1").unwrap();
        fs::create_dir_all(g.join("save")).unwrap();
        fs::write(g.join("save/b"), "2").unwrap();
        let b = g.join(".farfalauncher/backups/x");
        assert_eq!(backup_saves(&g, &b).unwrap(), 2);
        assert_eq!(fs::read_to_string(b.join("user/Saves/a")).unwrap(), "1");
        assert_eq!(fs::read_to_string(b.join("save/b")).unwrap(), "2");
    }
}
