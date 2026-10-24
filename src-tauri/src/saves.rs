//! Sauvegardes locales d'un jeu : où elles sont, empreinte du contenu,
//! archive zip pour le serveur et restauration (docs/romm-api.md §7).
//!
//! Chaque recomp range ses sauvegardes à sa façon. Ordre de recherche :
//! 1. un dossier `save`/`saves` à côté de l'exécutable ou à la racine du jeu ;
//! 2. quelques recomps connues (table ci-dessous) ;
//! 3. un dossier `save`/`saves` dans le dossier de données du jeu
//!    (`%APPDATA%/<exe>`, `~/.local/share/<exe>`…) ;
//! 4. sinon, l'utilisateur indique le dossier.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha1::{Digest, Sha1};

use crate::error::{log_err, AppError, AppResult};

/// Au-delà, ce n'est sans doute pas un dossier de sauvegardes (mauvais choix).
pub const MAX_SAVE_BYTES: u64 = 256 * 1024 * 1024;
const SAVE_DIR_NAMES: &[&str] = &["save", "saves"];

#[derive(Debug, Clone, PartialEq)]
pub struct SaveLocation {
    pub dir: PathBuf,
    /// Fichiers retenus au premier niveau de `dir` (`*` joker) ; vide = tout.
    pub patterns: Vec<String>,
}

/// Recomps dont le dossier de données contient bien plus que les sauvegardes
/// (ROM, mods…) : on ne prend que les fichiers de sauvegarde.
/// (nom de l'exécutable sans extension, dossier de données, motifs)
const KNOWN: &[(&str, &str, &[&str])] = &[("sm64coopdx", "sm64coopdx", &["sm64_save_file*.bin"])];

/// Dossiers où les jeux rangent leurs données utilisateur, par système.
pub fn data_roots() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut roots = Vec::new();
    if cfg!(windows) {
        roots.extend(env("APPDATA"));
        roots.extend(env("LOCALAPPDATA"));
        if let Some(home) = env("USERPROFILE") {
            roots.push(home.join("Documents"));
            roots.push(home.join("Saved Games"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            roots.push(home.join("Library/Application Support"));
            roots.push(home.join("Documents"));
        }
    } else if let Some(home) = env("HOME") {
        roots.push(env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")));
        roots.push(env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")));
        roots.push(home.join("Documents"));
    }
    roots
}

fn child_dir_ci(parent: &Path, names: &[&str]) -> Option<PathBuf> {
    fs::read_dir(parent).ok()?.filter_map(Result::ok).find_map(|e| {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = e.file_name().to_string_lossy().to_lowercase();
        (is_dir && names.contains(&name.as_str())).then(|| e.path())
    })
}

fn exe_stem(exe: &Path) -> Option<String> {
    let name = exe.file_name()?.to_string_lossy().into_owned();
    // `jeu.exe`, `Jeu.AppImage`, `Jeu.app`, `jeu.sh`, `jeu`
    let stem = name.split('.').next().unwrap_or(&name);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// Emplacement des sauvegardes deviné pour un jeu installé.
pub fn detect(game_dir: &Path, exe_rel: &Path) -> Option<SaveLocation> {
    let exe = game_dir.join(exe_rel);
    let exe_dir = exe.parent().unwrap_or(game_dir);
    for base in [exe_dir, game_dir] {
        if let Some(dir) = child_dir_ci(base, SAVE_DIR_NAMES) {
            return Some(SaveLocation { dir, patterns: Vec::new() });
        }
    }
    let stem = exe_stem(&exe)?;
    let roots = data_roots();
    if let Some((_, folder, patterns)) = KNOWN.iter().find(|(e, _, _)| e.eq_ignore_ascii_case(&stem)) {
        // Le dossier n'existe peut-être pas encore (jeu jamais lancé) : on le
        // retient quand même, il sera créé par le jeu.
        let root = roots.iter().find(|r| r.join(folder).is_dir()).or(roots.first())?;
        return Some(SaveLocation { dir: root.join(folder), patterns: patterns.iter().map(|p| p.to_string()).collect() });
    }
    roots.iter().find_map(|root| {
        let data = child_dir_ci(root, &[stem.to_lowercase().as_str()])?;
        child_dir_ci(&data, SAVE_DIR_NAMES).map(|dir| SaveLocation { dir, patterns: Vec::new() })
    })
}

/// Dossiers à proposer quand rien n'a été trouvé : données du jeu existantes.
pub fn suggestions(game_dir: &Path, exe_rel: &Path, game_name: &str) -> Vec<PathBuf> {
    let mut names: Vec<String> = Vec::new();
    names.extend(exe_stem(&game_dir.join(exe_rel)).map(|s| s.to_lowercase()));
    names.push(game_name.to_lowercase());
    let mut out = Vec::new();
    for root in data_roots() {
        for name in &names {
            if let Some(dir) = child_dir_ci(&root, &[name.as_str()]) {
                if !out.contains(&dir) {
                    out.push(dir);
                }
            }
        }
    }
    out
}

/// Refuse les dossiers manifestement trop larges (profil, racine, AppData…).
pub fn check_choice(dir: &Path, game_dir: &Path) -> AppResult<()> {
    let too_broad = dir.parent().is_none()
        || data_roots().iter().any(|r| r == dir)
        || std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).is_some_and(|h| Path::new(&h) == dir)
        || dir == game_dir;
    if too_broad || !dir.is_dir() {
        return Err(AppError::SaveDirInvalid);
    }
    Ok(())
}

fn matches(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let mut rest = n.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(pos) => rest = &rest[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// Fichiers de sauvegarde (chemin relatif `a/b.sav`, chemin complet), triés.
pub fn collect(loc: &SaveLocation) -> AppResult<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    if !loc.dir.is_dir() {
        return Ok(out);
    }
    if loc.patterns.is_empty() {
        walk(&loc.dir, "", &mut out)?;
    } else {
        for e in fs::read_dir(&loc.dir).map_err(|e| log_err(AppError::Disk, "lecture des sauvegardes", e))?.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) && loc.patterns.iter().any(|p| matches(p, &name)) {
                out.push((name, e.path()));
            }
        }
    }
    out.sort();
    let total: u64 = out.iter().filter_map(|(_, p)| fs::metadata(p).ok()).map(|m| m.len()).sum();
    if total > MAX_SAVE_BYTES {
        tracing::warn!("dossier de sauvegardes trop gros ({total} octets): {}", loc.dir.display());
        return Err(AppError::SaveDirInvalid);
    }
    Ok(out)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> AppResult<()> {
    for e in fs::read_dir(dir).map_err(|e| log_err(AppError::Disk, "lecture des sauvegardes", e))?.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let ft = e.file_type().map_err(|e| log_err(AppError::Disk, "lecture des sauvegardes", e))?;
        if ft.is_dir() {
            walk(&e.path(), &child, out)?;
        } else if ft.is_file() {
            out.push((child, e.path()));
        }
    }
    Ok(())
}

fn feed(hasher: &mut Sha1, rel: &str, data: &[u8]) {
    hasher.update(rel.as_bytes());
    hasher.update([0]);
    hasher.update((data.len() as u64).to_le_bytes());
    hasher.update(data);
}

fn finish(hasher: Sha1) -> String {
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Empreinte du contenu local ; `None` s'il n'y a aucune sauvegarde.
pub fn hash_local(loc: &SaveLocation) -> AppResult<Option<String>> {
    let files = collect(loc)?;
    if files.is_empty() {
        return Ok(None);
    }
    let mut h = Sha1::new();
    for (rel, path) in &files {
        let data = fs::read(path).map_err(|e| log_err(AppError::Disk, "lecture d'une sauvegarde", e))?;
        feed(&mut h, rel, &data);
    }
    Ok(Some(finish(h)))
}

/// Même empreinte, calculée sur le contenu d'une archive du serveur.
pub fn hash_zip(bytes: &[u8]) -> AppResult<Option<String>> {
    let entries = read_zip(bytes)?;
    if entries.is_empty() {
        return Ok(None);
    }
    let mut h = Sha1::new();
    for (rel, data) in &entries {
        feed(&mut h, rel, data);
    }
    Ok(Some(finish(h)))
}

/// Fichiers d'une archive (chemins sûrs uniquement), triés par chemin.
fn read_zip(bytes: &[u8]) -> AppResult<Vec<(String, Vec<u8>)>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| log_err(AppError::Extract, "archive de sauvegarde", e))?;
    let mut out = Vec::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| log_err(AppError::Extract, "archive de sauvegarde", e))?;
        if f.is_dir() {
            continue;
        }
        let Some(path) = f.enclosed_name() else {
            tracing::warn!("chemin dangereux ignoré dans une sauvegarde");
            continue;
        };
        let rel = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| log_err(AppError::Extract, "archive de sauvegarde", e))?;
        out.push((rel, data));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Archive zip des sauvegardes, à envoyer au serveur.
pub fn zip_local(loc: &SaveLocation) -> AppResult<Vec<u8>> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (rel, path) in collect(loc)? {
        let data = fs::read(&path).map_err(|e| log_err(AppError::Disk, "lecture d'une sauvegarde", e))?;
        w.start_file(rel.as_str(), opts).map_err(|e| log_err(AppError::Internal, "archive de sauvegarde", e))?;
        w.write_all(&data).map_err(|e| log_err(AppError::Internal, "archive de sauvegarde", e))?;
    }
    let cursor = w.finish().map_err(|e| log_err(AppError::Internal, "archive de sauvegarde", e))?;
    Ok(cursor.into_inner())
}

/// Remplace les sauvegardes locales par celles de l'archive, après les avoir
/// copiées dans `backup_dir`.
pub fn restore(loc: &SaveLocation, bytes: &[u8], backup_dir: &Path) -> AppResult<()> {
    let entries = read_zip(bytes)?;
    let current = collect(loc)?;
    for (rel, path) in &current {
        let dest = backup_dir.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| log_err(AppError::Disk, "copie de secours des sauvegardes", e))?;
        }
        fs::copy(path, &dest).map_err(|e| log_err(AppError::Disk, "copie de secours des sauvegardes", e))?;
    }
    for (_, path) in &current {
        fs::remove_file(path).map_err(|e| log_err(AppError::Disk, "remplacement des sauvegardes", e))?;
    }
    for (rel, data) in entries {
        // Avec des motifs, on ne pose que des fichiers de sauvegarde reconnus.
        if !loc.patterns.is_empty() && (rel.contains('/') || !loc.patterns.iter().any(|p| matches(p, &rel))) {
            continue;
        }
        let dest = loc.dir.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| log_err(AppError::Disk, "écriture des sauvegardes", e))?;
        }
        fs::write(&dest, data).map_err(|e| log_err(AppError::Disk, "écriture des sauvegardes", e))?;
    }
    Ok(())
}

/// Date de la sauvegarde locale la plus récente (secondes Unix).
pub fn latest_modified(loc: &SaveLocation) -> Option<u64> {
    collect(loc)
        .ok()?
        .iter()
        .filter_map(|(_, p)| fs::metadata(p).and_then(|m| m.modified()).ok())
        .max()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("farfa-saves-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn wildcard() {
        assert!(matches("sm64_save_file*.bin", "sm64_save_file.bin"));
        assert!(matches("sm64_save_file*.bin", "SM64_save_file_1.bin"));
        assert!(!matches("sm64_save_file*.bin", "sm64config.txt"));
        assert!(matches("*.sav", "a.sav"));
        assert!(matches("exact.txt", "Exact.TXT"));
    }

    #[test]
    fn detects_save_dir_next_to_exe() {
        let game = tmp();
        fs::create_dir_all(game.join("bin/Saves")).unwrap();
        fs::write(game.join("bin/game.exe"), b"").unwrap();
        let loc = detect(&game, Path::new("bin/game.exe")).unwrap();
        assert_eq!(loc.dir, game.join("bin/Saves"));
        assert!(loc.patterns.is_empty());
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn roundtrip_zip_hash_restore() {
        let root = tmp();
        let loc = SaveLocation { dir: root.join("saves"), patterns: vec![] };
        fs::create_dir_all(loc.dir.join("slot")).unwrap();
        fs::write(loc.dir.join("a.sav"), b"AAA").unwrap();
        fs::write(loc.dir.join("slot/b.sav"), b"BB").unwrap();

        let h = hash_local(&loc).unwrap().unwrap();
        let zip = zip_local(&loc).unwrap();
        assert_eq!(hash_zip(&zip).unwrap().as_deref(), Some(h.as_str()));

        // Modification locale : l'empreinte change ; la restauration la remet.
        fs::write(loc.dir.join("a.sav"), b"changed").unwrap();
        fs::write(loc.dir.join("new.sav"), b"x").unwrap();
        assert_ne!(hash_local(&loc).unwrap().as_deref(), Some(h.as_str()));
        restore(&loc, &zip, &root.join("backup")).unwrap();
        assert_eq!(hash_local(&loc).unwrap().as_deref(), Some(h.as_str()));
        assert_eq!(fs::read(root.join("backup/a.sav")).unwrap(), b"changed");
        assert!(!loc.dir.join("new.sav").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn patterns_limit_files() {
        let root = tmp();
        fs::write(root.join("sm64_save_file.bin"), b"S").unwrap();
        fs::write(root.join("baserom.us.z64"), b"ROM").unwrap();
        let loc = SaveLocation { dir: root.clone(), patterns: vec!["sm64_save_file*.bin".into()] };
        let files: Vec<String> = collect(&loc).unwrap().into_iter().map(|(r, _)| r).collect();
        assert_eq!(files, vec!["sm64_save_file.bin"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_saves_have_no_hash() {
        let root = tmp();
        let loc = SaveLocation { dir: root.join("nothing"), patterns: vec![] };
        assert_eq!(hash_local(&loc).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }
}
