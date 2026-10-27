//! Sauvegardes locales d'un jeu : où elles sont, empreinte du contenu,
//! archive zip pour le serveur et restauration (docs/romm-api.md §7).
//!
//! Chaque recomp range ses sauvegardes à sa façon. Ordre de recherche :
//! 1. un dossier de sauvegardes (`save`, `saves`, `savedata`…) à côté de
//!    l'exécutable, ou ailleurs dans le dossier du jeu (quelques niveaux) ;
//! 2. quelques recomps connues (table ci-dessous) ;
//! 3. le dossier de données du jeu, d'après le nom de l'exécutable ou du jeu
//!    (`%APPDATA%`, `%LOCALAPPDATA%`, `AppData/LocalLow`, Documents, `My Games`,
//!    `Saved Games`, `~/.local/share`, `~/.config`, `~/Library/Application
//!    Support`…, aussi sous un dossier d'éditeur) : son dossier de sauvegardes,
//!    sinon ses fichiers `.sav`, `.srm`… ;
//! 4. sinon, l'utilisateur indique le dossier.

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha1::{Digest, Sha1};

use crate::error::{log_err, AppError, AppResult};

/// Au-delà, ce n'est sans doute pas un dossier de sauvegardes (mauvais choix).
pub const MAX_SAVE_BYTES: u64 = 256 * 1024 * 1024;
const SAVE_DIR_NAMES: &[&str] = &[
    "save", "saves", "savedata", "save_data", "save data", "savegame", "savegames", "saved games", "savefiles", "memcard", "memcards",
];
/// Fichiers de sauvegarde reconnus dans un dossier de données sans sous-dossier dédié.
const SAVE_EXTS: &[&str] = &["sav", "save", "srm", "sra", "eep", "fla", "mpk", "gci", "sav0", "sav1", "sav2"];
/// Dossiers jamais fouillés : lourds, ou sans sauvegardes.
const SKIP_DIRS: &[&str] = &[
    ".farfalauncher", ".git", "node_modules", "cache", "caches", "shadercache", "logs", "log", "mods", "texture_replacements",
    "texture-mips", "build-tools", "electron", "locales", "crashdumps", "sentry", "temp", "tmp",
];
/// Profondeur fouillée sous le dossier du jeu ou de données.
const SEARCH_DEPTH: usize = 5;
/// Profondeur fouillée pour les fichiers écrits pendant une partie.
const ACTIVITY_DEPTH: usize = 6;
/// Garde-fou : nombre de dossiers lus par recherche.
const SEARCH_MAX_DIRS: usize = 5000;
/// Fichiers écrits pendant une partie, même dans un dossier de sauvegardes,
/// qui n'en sont pas.
const NOT_SAVE_EXTS: &[&str] = &["log", "tmp", "lock", "pid", "dmp"];

#[derive(Debug, Clone, PartialEq)]
pub struct SaveLocation {
    pub dir: PathBuf,
    /// Fichiers retenus au premier niveau de `dir` (`*` joker) ; vide = tout.
    pub patterns: Vec<String>,
}

/// Recomps dont le dossier de données contient bien plus que les sauvegardes
/// (ROM, mods…) : on ne prend que les fichiers de sauvegarde.
/// (nom de l'exécutable sans extension, dossier de données, motifs)
const KNOWN: &[(&str, &str, &[&str])] = &[
    ("sm64coopdx", "sm64coopdx", &["sm64_save_file*.bin"]),
    // %APPDATA%/MHGURecomp/save/save_0/system…
    ("mhgurecomp", "MHGURecomp/save", &[]),
];

/// Dossiers où les jeux rangent leurs données utilisateur, par système.
pub fn data_roots() -> Vec<PathBuf> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut roots = Vec::new();
    if cfg!(windows) {
        roots.extend(env("APPDATA"));
        roots.extend(env("LOCALAPPDATA"));
        if let Some(home) = env("USERPROFILE") {
            // Jeux Unity : AppData/LocalLow/<éditeur>/<jeu>.
            roots.push(home.join("AppData").join("LocalLow"));
            roots.push(home.join("Documents"));
            roots.push(home.join("Documents").join("My Games"));
            roots.push(home.join("Saved Games"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            roots.push(home.join("Library/Application Support"));
            roots.push(home.join("Library/Preferences"));
            roots.push(home.join("Library/Containers"));
            roots.push(home.join("Documents"));
        }
    } else if let Some(home) = env("HOME") {
        roots.push(env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share")));
        roots.push(env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")));
        roots.push(env("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")));
        roots.push(home.join("Documents"));
        roots.push(home.join("Documents/My Games"));
    }
    // Moteurs qui rangent tous leurs jeux sous un même dossier : Godot
    // (`<données>/Godot/app_userdata/<jeu>`), LÖVE (`<données>/LOVE/<jeu>`).
    let engines: Vec<PathBuf> = roots
        .iter()
        .flat_map(|r| [r.join("Godot").join("app_userdata"), r.join("godot").join("app_userdata"), r.join("LOVE"), r.join("love")])
        .collect();
    roots.extend(engines);
    // Sans doublons (Windows ne distingue pas `Godot` de `godot`).
    let mut seen = std::collections::HashSet::new();
    roots.retain(|r| r.is_dir() && seen.insert(r.to_string_lossy().to_lowercase()));
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

fn is_skipped(name: &str) -> bool {
    SKIP_DIRS.contains(&name.to_lowercase().as_str())
}

/// Ce nom de dossier évoque-t-il des sauvegardes ? 2 : nom attendu (`saves`) ;
/// 1 : nom qui en parle (`SaveData_US`, `user_saves`, `memcard-a`…) ; 0 : non.
fn save_dir_rank(name: &str) -> u8 {
    let n = name.to_lowercase();
    if SAVE_DIR_NAMES.contains(&n.as_str()) {
        2
    } else if (n.contains("save") && !n.contains("autosave")) || n.contains("memcard") || n.contains("memorycard") {
        1
    } else {
        0
    }
}

/// Dossier de sauvegardes sous `base`, en descendant dans tous les
/// sous-dossiers : un nom attendu (`saves`) le moins profond, sinon le premier
/// nom qui parle de sauvegardes.
fn find_save_dir(base: &Path) -> Option<PathBuf> {
    let mut level = vec![base.to_path_buf()];
    let mut read = 0;
    let mut fallback: Option<PathBuf> = None;
    for _ in 0..SEARCH_DEPTH {
        let mut next = Vec::new();
        for dir in &level {
            read += 1;
            if read > SEARCH_MAX_DIRS {
                return fallback;
            }
            let Ok(rd) = fs::read_dir(dir) else { continue };
            let mut children: Vec<(String, PathBuf)> = rd
                .filter_map(Result::ok)
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .collect();
            children.sort();
            for (name, path) in &children {
                match save_dir_rank(name) {
                    2 => return Some(path.clone()),
                    1 if fallback.is_none() => fallback = Some(path.clone()),
                    _ => {}
                }
            }
            next.extend(children.into_iter().filter(|(n, _)| !is_skipped(n)).map(|(_, p)| p));
        }
        level = next;
    }
    fallback
}

/// Après une partie : sauvegardes déduites des fichiers que le jeu vient
/// d'écrire (dans son dossier ou son dossier de données), quand aucun nom de
/// dossier ne les trahit. Un fichier compte s'il ressemble à une sauvegarde
/// (extension, nom) ou s'il est rangé dans un dossier qui en parle.
pub fn detect_by_activity(game_dir: &Path, exes: &[PathBuf], game_name: &str, since: SystemTime) -> Option<SaveLocation> {
    // Marge pour la précision des dates des systèmes de fichiers.
    let since = since.checked_sub(std::time::Duration::from_secs(2)).unwrap_or(since);
    let mut bases = vec![game_dir.to_path_buf()];
    bases.extend(data_dirs(game_dir, &Names::new(game_dir, exes, game_name, false)));
    // (date, fichier, dossier fouillé)
    let mut best: Option<(SystemTime, PathBuf, PathBuf)> = None;
    for base in &bases {
        let mut level = vec![base.clone()];
        let mut read = 0;
        for _ in 0..ACTIVITY_DEPTH {
            let mut next = Vec::new();
            for dir in &level {
                read += 1;
                if read > SEARCH_MAX_DIRS {
                    break;
                }
                let Ok(rd) = fs::read_dir(dir) else { continue };
                for e in rd.filter_map(Result::ok) {
                    let Ok(ft) = e.file_type() else { continue };
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    if ft.is_dir() {
                        if !is_skipped(&name) {
                            next.push(e.path());
                        }
                        continue;
                    }
                    let Some(modified) = e.metadata().ok().and_then(|m| m.modified().ok()).filter(|m| *m >= since) else { continue };
                    let ext = Path::new(&name).extension().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                    let in_save_dir = !NOT_SAVE_EXTS.contains(&ext.as_str())
                        && e.path().parent().is_some_and(|p| {
                            p.ancestors()
                                .take_while(|a| a.starts_with(base) && *a != base.as_path())
                                .any(|a| a.file_name().is_some_and(|n| save_dir_rank(&n.to_string_lossy()) > 0))
                        });
                    let looks_saved = SAVE_EXTS.contains(&ext.as_str()) || (name.contains("save") && !name.contains("autosave"));
                    if (in_save_dir || looks_saved) && best.as_ref().is_none_or(|(t, _, _)| modified > *t) {
                        best = Some((modified, e.path(), base.clone()));
                    }
                }
            }
            level = next;
        }
    }
    let (_, file, base) = best?;
    let parent = file.parent()?;
    // Le dossier « sauvegardes » le plus haut entre le fichier et le dossier fouillé.
    let save_dir = parent
        .ancestors()
        .take_while(|a| a.starts_with(&base) && *a != base.as_path())
        .filter(|a| a.file_name().is_some_and(|n| save_dir_rank(&n.to_string_lossy()) > 0))
        .last();
    if let Some(dir) = save_dir {
        return Some(SaveLocation { dir: dir.to_path_buf(), patterns: Vec::new() });
    }
    // Fichier isolé parmi d'autres données : ce type de fichiers seulement.
    let pattern = match file.extension() {
        Some(x) => format!("*.{}", x.to_string_lossy().to_lowercase()),
        None => file.file_name()?.to_string_lossy().into_owned(),
    };
    Some(SaveLocation { dir: parent.to_path_buf(), patterns: vec![pattern] })
}

/// Motifs des fichiers de sauvegarde présents au premier niveau de `dir`.
fn save_file_patterns(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut exts: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| Path::new(&e.file_name()).extension().map(|x| x.to_string_lossy().to_lowercase()))
        .filter(|x| SAVE_EXTS.contains(&x.as_str()))
        .collect();
    exts.sort();
    exts.dedup();
    exts.into_iter().map(|x| format!("*.{x}")).collect()
}

/// Nom réduit à ses lettres et chiffres, en minuscules (`Super Mario 64` → `supermario64`).
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Noms d'exécutables qui ne disent rien du jeu.
const GENERIC_EXES: &[&str] = &["game", "launcher", "main", "start", "run", "play", "app", "client", "setup", "update", "updater"];

/// Noms sous lesquels un jeu peut ranger ses données.
struct Names {
    /// Exécutables : un dossier qui commence pareil compte (`melee` → `melee-pc`).
    exes: Vec<String>,
    /// Nom du jeu et de son dossier : nom identique seulement (trop génériques).
    exact: Vec<String>,
    /// Pour de simples suggestions : le nom du jeu peut aussi commencer le
    /// dossier (`Pokemon` → `pokemon-love2d`).
    loose: bool,
}

impl Names {
    fn new(game_dir: &Path, exes: &[PathBuf], game_name: &str, loose: bool) -> Self {
        let mut n = Names { exes: Vec::new(), exact: Vec::new(), loose };
        for exe in exes {
            if let Some(s) = exe_stem(exe).map(|s| squash(&s)) {
                if s.len() >= 3 && !GENERIC_EXES.contains(&s.as_str()) && !n.exes.contains(&s) {
                    n.exes.push(s);
                }
            }
        }
        for name in [Some(game_name.to_string()), game_dir.file_name().map(|f| f.to_string_lossy().into_owned())].into_iter().flatten() {
            let s = squash(&name);
            if s.len() >= 3 && !n.exact.contains(&s) {
                n.exact.push(s);
            }
        }
        n
    }

    fn matches(&self, dir_name: &str) -> bool {
        let d = squash(dir_name);
        if d.len() < 3 {
            return false;
        }
        self.exact.contains(&d)
            || (self.loose && self.exact.iter().any(|e| e.len() >= 4 && d.starts_with(e.as_str())))
            || self.exes.iter().any(|e| *e == d || (e.len().min(d.len()) >= 4 && (d.starts_with(e.as_str()) || e.starts_with(d.as_str()))))
    }
}

/// Dossiers de données au nom du jeu : `<racine>/<jeu>` puis `<racine>/<éditeur>/<jeu>`.
fn data_dirs(game_dir: &Path, names: &Names) -> Vec<PathBuf> {
    let subdirs = |p: &Path| -> Vec<PathBuf> {
        fs::read_dir(p)
            .map(|rd| rd.filter_map(Result::ok).filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()).collect())
            .unwrap_or_default()
    };
    let named = |p: &Path| p.file_name().is_some_and(|n| names.matches(&n.to_string_lossy()));
    let roots = data_roots();
    let mut out: Vec<PathBuf> = Vec::new();
    for root in &roots {
        out.extend(subdirs(root).into_iter().filter(|d| named(d)));
    }
    for root in &roots {
        for company in subdirs(root) {
            // Une autre racine (`Documents/My Games`) est déjà fouillée pour elle-même.
            if roots.contains(&company) || named(&company) {
                continue;
            }
            out.extend(subdirs(&company).into_iter().filter(|d| named(d)));
        }
    }
    let mut seen = std::collections::HashSet::new();
    // Le dossier du jeu lui-même (installé sous AppData…) est déjà fouillé.
    out.retain(|d| !d.starts_with(game_dir) && seen.insert(d.to_string_lossy().to_lowercase()));
    out
}

/// Emplacement des sauvegardes deviné pour un jeu installé. `exes` : ses
/// exécutables relatifs au dossier du jeu (celui choisi, puis le vrai jeu
/// démarré par son launcher s'il est connu).
pub fn detect(game_dir: &Path, exes: &[PathBuf], game_name: &str) -> Option<SaveLocation> {
    let whole = |dir: PathBuf| SaveLocation { dir, patterns: Vec::new() };
    for exe in exes {
        let exe_dir = game_dir.join(exe).parent().map(Path::to_path_buf).unwrap_or_else(|| game_dir.to_path_buf());
        if let Some(dir) = child_dir_ci(&exe_dir, SAVE_DIR_NAMES) {
            return Some(whole(dir));
        }
    }
    if let Some(dir) = find_save_dir(game_dir) {
        return Some(whole(dir));
    }
    let roots = data_roots();
    for exe in exes {
        let Some(stem) = exe_stem(exe) else { continue };
        if let Some((_, folder, patterns)) = KNOWN.iter().find(|(e, _, _)| e.eq_ignore_ascii_case(&stem)) {
            // Le dossier n'existe peut-être pas encore (jeu jamais lancé) : on le
            // retient quand même, il sera créé par le jeu.
            let root = roots.iter().find(|r| r.join(folder).is_dir()).or(roots.first())?;
            return Some(SaveLocation { dir: root.join(folder), patterns: patterns.iter().map(|p| p.to_string()).collect() });
        }
    }
    let data = data_dirs(game_dir, &Names::new(game_dir, exes, game_name, false));
    if let Some(dir) = data.iter().find_map(|d| find_save_dir(d)) {
        return Some(whole(dir));
    }
    // Pas de dossier dédié : les fichiers de sauvegarde posés à la racine des
    // données (le reste — ROM, mods, journaux — n'est pas pris).
    data.into_iter().find_map(|dir| {
        let patterns = save_file_patterns(&dir);
        (!patterns.is_empty()).then_some(SaveLocation { dir, patterns })
    })
}

/// Dossiers à proposer quand rien n'a été trouvé : données du jeu existantes.
pub fn suggestions(game_dir: &Path, exes: &[PathBuf], game_name: &str) -> Vec<PathBuf> {
    data_dirs(game_dir, &Names::new(game_dir, exes, game_name, true))
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
        let loc = detect(&game, &[PathBuf::from("bin/game.exe")], "Jeu").unwrap();
        assert_eq!(loc.dir, game.join("bin/Saves"));
        assert!(loc.patterns.is_empty());
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn detects_nested_save_dir_in_game() {
        // Launcher portable : <jeu>/Launcher-Portable/saves, à côté d'un cache.
        let game = tmp();
        fs::create_dir_all(game.join("Launcher-Portable/cache/save")).unwrap();
        fs::create_dir_all(game.join("Launcher-Portable/saves/card-a")).unwrap();
        fs::write(game.join("launcher.exe"), b"").unwrap();
        let loc = detect(&game, &[PathBuf::from("launcher.exe")], "Jeu").unwrap();
        assert_eq!(loc.dir, game.join("Launcher-Portable/saves"));
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn deep_and_loosely_named_save_dirs() {
        let game = tmp();
        fs::create_dir_all(game.join("a/b/c/d/UserSaveData")).unwrap();
        assert_eq!(find_save_dir(&game), Some(game.join("a/b/c/d/UserSaveData")));
        // Un nom attendu l'emporte, même plus profond ; jamais « autosave ».
        fs::create_dir_all(game.join("x/y/z/saves")).unwrap();
        fs::create_dir_all(game.join("autosaves")).unwrap();
        assert_eq!(find_save_dir(&game), Some(game.join("x/y/z/saves")));
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn activity_finds_files_written_while_playing() {
        let game = tmp();
        fs::create_dir_all(game.join("port/user/card-a")).unwrap();
        fs::write(game.join("port/old.sav"), b"old").unwrap();
        let since = SystemTime::now() + std::time::Duration::from_secs(5);
        // Rien d'écrit depuis le début de la partie.
        assert_eq!(detect_by_activity(&game, &[], "Zzz Jeu Test", since), None);
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        fs::write(game.join("port/user/card-a/01-GMSE.gci"), b"S").unwrap();
        fs::write(game.join("port/game.log"), b"log").unwrap();
        let loc = detect_by_activity(&game, &[], "Zzz Jeu Test", since).unwrap();
        assert_eq!(loc, SaveLocation { dir: game.join("port/user/card-a"), patterns: vec!["*.gci".into()] });
        // Dans un dossier qui parle de sauvegardes : tout ce dossier.
        fs::create_dir_all(game.join("port/SaveData/slot1")).unwrap();
        fs::write(game.join("port/SaveData/slot1/data.bin"), b"S").unwrap();
        let loc = detect_by_activity(&game, &[], "Zzz Jeu Test", since).unwrap();
        assert_eq!(loc, SaveLocation { dir: game.join("port/SaveData"), patterns: vec![] });
        fs::remove_dir_all(game).unwrap();
    }

    #[test]
    fn save_files_without_save_dir() {
        let root = tmp();
        fs::write(root.join("achievements.sav"), b"S").unwrap();
        fs::write(root.join("baserom.nes"), b"ROM").unwrap();
        fs::write(root.join("slot1.SRM"), b"S").unwrap();
        assert_eq!(save_file_patterns(&root), vec!["*.sav", "*.srm"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn data_dir_names() {
        let exes = [PathBuf::from("SMB1R.exe"), PathBuf::from("bin/melee.exe")];
        let n = Names::new(Path::new("/jeux/Super Mario Bros"), &exes, "Super Mario Bros", false);
        assert!(n.matches("SMB1R"));
        assert!(n.matches("melee-pc"));
        assert!(n.matches("Super Mario Bros"));
        assert!(n.matches("SuperMarioBros"));
        assert!(!n.matches("Super Mario Bros Wonder"));
        assert!(!n.matches("Microsoft"));
        let generic = Names::new(Path::new("/jeux/X"), &[PathBuf::from("Launcher.exe")], "X", false);
        assert!(!generic.matches("LauncherCache"));
        let loose = Names::new(Path::new("/jeux/Pokemon"), &[], "Pokemon", true);
        assert!(loose.matches("pokemon-love2d"));
        assert!(!Names::new(Path::new("/jeux/Pokemon"), &[], "Pokemon", false).matches("pokemon-love2d"));
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
