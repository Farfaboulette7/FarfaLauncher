//! Rapport de bug, pour n'importe quel jeu installé (PC, recomp, émulateur) et sur tous les
//! systèmes. Le joueur décrit le problème ; le launcher joint ce qu'il sait trouver :
//! - toujours : matériel, système, versions, jeu et ce qui le lance, réglages optimisés,
//!   journaux du launcher, sortie du dernier lancement ;
//! - en plus, les fichiers décrits par la fiche du gestionnaire (`journaux:`) : journaux et
//!   réglages de l'émulateur ou de la recomp.
//!
//! `journaux:` ne joint que des fichiers du dossier du jeu ou de dossiers connus de
//! l'émulateur, jamais ceux du launcher (faille SL10, `pattern_allowed`).
//!
//! Le dossier personnel devient `~`, le nom du compte et celui du PC disparaissent. Le zip part sur
//! FarfaRomManager (`PUT /api/reports`), où une tâche Claude l'analyse.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::install::extract::INTERNAL_DIR;
use crate::install::rel_to_path;
use crate::install::state::InstalledGame;
use crate::manager::{Catalog, LogPattern};

/// Taille maximale du zip (le gestionnaire refuse au-delà de 20 Mo).
const MAX_ZIP: usize = 19 << 20;
/// Part gardée de chaque journal : sa fin.
const MAX_FILE: u64 = 2 << 20;
/// Fichiers au plus par motif de `journaux:`.
const MAX_PER_PATTERN: usize = 5;

/// Un fichier joint : son nom dans le rapport et son chemin sur ce PC.
pub struct Attachment {
    pub name: String,
    pub path: PathBuf,
}

/// Fichiers joints au rapport de ce jeu, dans l'ordre.
///
/// `config_dir` : dossier de réglages du launcher (jeton `token.json` sous Linux sans
/// trousseau, `settings.json`…). Aucun fichier de `journaux:` n'y est jamais pris (faille SL10).
pub fn attachments(catalog: &Catalog, config_dir: &Path, log_dir: &Path, st: &InstalledGame) -> Vec<Attachment> {
    let mut out = Vec::new();
    // Journaux du launcher : les deux plus récents (aujourd'hui, hier).
    if let Ok(rd) = std::fs::read_dir(log_dir) {
        let mut logs: Vec<(std::time::SystemTime, PathBuf)> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("farfalauncher"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        logs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in logs.into_iter().take(2) {
            out.push(Attachment { name: format!("launcher/{}", file_name(&p)), path: p });
        }
    }
    // Sortie du jeu au dernier lancement (capturée pour tous les jeux).
    let run_log = st.game_dir.join(INTERNAL_DIR).join("dernier-lancement.log");
    if run_log.is_file() {
        out.push(Attachment { name: "dernier-lancement.log".into(), path: run_log });
    }
    // Journaux et réglages propres à l'émulateur ou à la recomp.
    let exe_dir = exe_dir(catalog, st);
    let roots = known_roots(catalog, st);
    for pattern in catalog.logs_for_install(st).iter().filter(|p| p.system.as_deref().is_none_or(|s| s == crate::language::current_system())) {
        if !pattern_allowed(&pattern.pattern, &roots) {
            tracing::warn!("rapport : motif « {} » refusé (hors du dossier du jeu et de l'émulateur)", pattern.pattern);
            continue;
        }
        for path in find(pattern, &exe_dir) {
            // Seconde barrière, sur le fichier trouvé : jamais dans les dossiers du launcher.
            if is_inside(&path, config_dir) || is_inside(&path, log_dir) {
                tracing::warn!("rapport : {} refusé (dossier du launcher)", path.display());
                continue;
            }
            let rel = path.strip_prefix(&exe_dir).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| file_name(&path));
            if !out.iter().any(|a| a.path == path) {
                out.push(Attachment { name: format!("jeu/{rel}"), path });
            }
        }
    }
    out
}

/// Noms de dossier du launcher lui-même (réglages, jeton, journaux) : jamais joints par une
/// fiche, quel que soit le joker (`{config}`, `{appdata}`, `{localappdata}`…).
const LAUNCHER_DIRS: &[&str] = &["fr.farfaboulette.farfalauncher", "farfalauncher", "farfalauncherdeck"];

/// Dossiers « connus » de l'émulateur ou de la recomp de ce jeu, hors de son dossier : le
/// premier dossier après le joker de ses `lien:`, `dossier-mods:` et dossiers de sauvegarde
/// (`{appdata}/lichii/…` → `lichii`), plus son identifiant. En minuscules.
fn known_roots(catalog: &Catalog, st: &InstalledGame) -> Vec<String> {
    let mut paths: Vec<&str> = Vec::new();
    let mut roots: Vec<String> = Vec::new();
    if let Some(e) = catalog.emulator_for_install(st) {
        roots.push(e.id.to_lowercase());
        paths.extend(e.links.iter().map(|l| l.path.as_str()));
        paths.extend(e.mods_dirs.iter().map(|m| m.path.as_str()));
        paths.extend(e.saves.by_system.values().map(String::as_str));
    }
    if let Some(r) = catalog.recomp_for_install(st) {
        roots.push(r.id.to_lowercase());
        paths.extend(r.links.iter().map(|l| l.path.as_str()));
        paths.extend(r.mods_dirs.iter().map(|m| m.path.as_str()));
    }
    roots.extend(paths.into_iter().filter_map(joker_root));
    roots.retain(|r| !r.is_empty());
    roots
}

/// Premier dossier après un joker autre que `{exedir}` (`{appdata}/lichii/x` → `lichii`).
fn joker_root(path: &str) -> Option<String> {
    let (var, rest) = path.strip_prefix('{')?.split_once('}')?;
    if var == "exedir" {
        return None;
    }
    let first = rest.strip_prefix('/')?.split('/').next()?;
    Some(first.to_lowercase())
}

/// Motif de `journaux:` accepté (faille SL10) :
/// - relatif, ou `{exedir}/…` : dans le dossier du jeu ou de l'émulateur (`expand` empêche
///   d'en sortir) ;
/// - `{appdata}/x/…`, `{localappdata}/x/…`, `{config}/x/…`, `{documents}/x/…` : seulement si
///   `x` est un dossier connu de l'émulateur ou de la recomp (`known_roots`), jamais un dossier
///   du launcher, et jamais un fichier posé directement dans le dossier du joker ;
/// - tout le reste est refusé.
fn pattern_allowed(pattern: &str, known_roots: &[String]) -> bool {
    let Some(after) = pattern.strip_prefix('{') else { return true };
    let Some((var, rest)) = after.split_once('}') else { return false };
    if var == "exedir" {
        return true;
    }
    if !["appdata", "localappdata", "config", "documents"].contains(&var) {
        return false;
    }
    // `/dossier/…/fichier` : au moins un dossier avant le nom du fichier.
    let Some(rest) = rest.strip_prefix('/') else { return false };
    let Some((first, _)) = rest.split_once('/') else { return false };
    let first = first.to_lowercase();
    !LAUNCHER_DIRS.contains(&first.as_str()) && known_roots.iter().any(|r| *r == first)
}

/// `path` est dans `dir` (ou est `dir`) ? Chemins réels comparés quand ils existent (liens
/// résolus), sinon tels quels.
fn is_inside(path: &Path, dir: &Path) -> bool {
    if dir.as_os_str().is_empty() {
        return false;
    }
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(path).starts_with(real(dir)) || path.starts_with(dir)
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Dossier de l'émulateur ou de l'exécutable du jeu.
pub fn exe_dir(catalog: &Catalog, st: &InstalledGame) -> PathBuf {
    let exe = st.executable.clone().unwrap_or_default();
    match catalog.emulator_for_install(st) {
        Some(e) => e.data_dir(&st.game_dir, &exe),
        None => st.game_dir.join(rel_to_path(&exe)).parent().map(Path::to_path_buf).unwrap_or_else(|| st.game_dir.clone()),
    }
}

/// Fichiers d'un motif (`user/log/*.txt`, `{appdata}/x/log.txt`), les plus récents d'abord.
fn find(pattern: &LogPattern, exe_dir: &Path) -> Vec<PathBuf> {
    let p = &pattern.pattern;
    let full = if p.starts_with('{') { p.clone() } else { format!("{{exedir}}/{p}") };
    let (dir, name) = full.rsplit_once('/').unwrap_or(("{exedir}", full.as_str()));
    let Some(dir) = crate::language::expand(&format!("{dir}/_"), exe_dir).and_then(|p| p.parent().map(Path::to_path_buf)) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && wildcard(name, &e.file_name().to_string_lossy()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(MAX_PER_PATTERN).map(|(_, p)| p).collect()
}

/// `*` : n'importe quelle suite de caractères (casse ignorée).
pub(crate) fn wildcard(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.to_lowercase(), name.to_lowercase());
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            let Some(at) = rest.find(part) else { return false };
            rest = &rest[at + part.len()..];
        }
    }
    true
}

/// Remplace le dossier personnel par `~`, le nom du compte par `<compte>` et le nom du PC
/// par `<pc>` (faille SL10).
pub fn anonymize(text: &str) -> String {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    // Même nom que celui de l'historique des sauvegardes (COMPUTERNAME, HOSTNAME, /etc/hostname).
    let host = crate::save_store::device_name();
    anonymize_with(text, &home, &user, &host)
}

/// `anonymize` avec les valeurs données (testable).
fn anonymize_with(text: &str, home: &str, user: &str, host: &str) -> String {
    let mut out = text.to_string();
    if home.len() > 3 {
        for form in [home.to_string(), home.replace('\\', "/"), home.replace('\\', "\\\\")] {
            out = out.replace(&form, "~");
        }
    }
    if user.len() >= 3 {
        out = replace_word(&out, user, "<compte>");
    }
    // Le nom du PC : en entier, comme le compte (« DESKTOP-AB12CD », « steamdeck »).
    if host.len() >= 3 && host != "Ordinateur" {
        out = replace_word(&out, host, "<pc>");
    }
    out
}

/// Remplace `word` là où il n'est pas collé à une lettre ou un chiffre (casse ignorée).
fn replace_word(text: &str, word: &str, by: &str) -> String {
    let lower = text.to_lowercase();
    let w = word.to_lowercase();
    if lower.len() != text.len() {
        return text.replace(word, by);
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&w) {
        let start = i + pos;
        let end = start + w.len();
        let before_ok = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after_ok = text[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        out.push_str(&text[i..start]);
        out.push_str(if before_ok && after_ok { by } else { &text[start..end] });
        i = end;
    }
    out.push_str(&text[i..]);
    out
}

/// Fin d'un fichier (au plus `MAX_FILE`), en texte.
fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(MAX_FILE);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        text = format!("[… début coupé, {} Mo au total]\n{text}", len >> 20);
    }
    Some(text)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportInfo<'a> {
    pub launcher: &'a str,
    pub created_at: u64,
    pub description: &'a str,
    /// Le jeu venait de planter quand le rapport a été proposé.
    pub after_crash: bool,
    pub os: String,
    pub hardware: &'a crate::hardware::Hardware,
    pub game: String,
    pub rom_id: i64,
    /// Ce qui lance le jeu : « Lichii build-160 », « mkw-recomp v0.1.8 », « jeu PC (Proton) »…
    pub runner: String,
    pub optimizations: Vec<crate::optimize::Applied>,
    pub optimizations_enabled: bool,
    pub files: Vec<String>,
}

/// Ce qui lance ce jeu, en clair.
pub fn runner(catalog: &Catalog, st: &InstalledGame) -> String {
    if let Some(e) = catalog.emulator_for_install(st) {
        return format!("{} {} (émulateur)", e.name, e.version.nom);
    }
    if let Some(r) = catalog.recomp_for_install(st) {
        return format!("{} {} (recomp)", r.name, r.version.nom);
    }
    match &st.pc {
        Some(pc) => format!("jeu PC {:?}{}", pc.os, if pc.proton.is_some() { " sous Proton" } else { "" }),
        None => format!("jeu installé {}", st.version.as_deref().unwrap_or("")),
    }
}

/// Zip du rapport : `rapport.json`, `installation.json` et les fichiers joints, anonymisés.
pub fn build(info: &ReportInfo, st: &InstalledGame, attachments: &[Attachment]) -> std::io::Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("rapport.json", opts)?;
        z.write_all(anonymize(&serde_json::to_string_pretty(info).unwrap_or_default()).as_bytes())?;
        z.start_file("installation.json", opts)?;
        z.write_all(anonymize(&serde_json::to_string_pretty(st).unwrap_or_default()).as_bytes())?;
        let mut total = 0usize;
        for a in attachments {
            let Some(text) = read_tail(&a.path) else { continue };
            let text = anonymize(&text);
            // Dernière limite : le zip doit rester sous la taille admise.
            if total + text.len() / 4 > MAX_ZIP {
                tracing::warn!("rapport : {} laissé de côté (rapport trop gros)", a.name);
                continue;
            }
            total += text.len() / 4;
            z.start_file(&a.name, opts)?;
            z.write_all(text.as_bytes())?;
        }
        z.finish()?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard("*.txt", "eden_log.TXT"));
        assert!(wildcard("eden_log*", "eden_log.txt.old.txt"));
        assert!(wildcard("qt-config.ini", "qt-config.ini"));
        assert!(wildcard("a*b*c", "a--b--c"));
        assert!(!wildcard("*.txt", "log.bin"));
        assert!(!wildcard("qt-config.ini", "qt-config.ini.bak"));
    }

    #[test]
    fn anonymizes_home_and_account() {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap();
        let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap();
        let text = format!("jeu dans {home}\\Jeux et {} ; compte {user} ; mot{user}", home.replace('\\', "/"));
        let out = anonymize(&text);
        assert!(!out.contains(&home) && !out.contains(&home.replace('\\', "/")), "{out}");
        assert!(out.contains("~\\Jeux") && out.contains("compte <compte>"), "{out}");
        // Collé à un mot : laissé tel quel (évite d'abîmer d'autres textes).
        assert!(out.contains(&format!("mot{user}")), "{out}");
    }

    #[test]
    fn anonymizes_computer_name() {
        let out = anonymize_with(r"C:\Users\Jean\x ; PC DESKTOP-AB12CD ; jean", r"C:\Users\Jean", "Jean", "DESKTOP-AB12CD");
        assert_eq!(out, r"~\x ; PC <pc> ; <compte>");
        // Casse ignorée, nom collé à un mot laissé tel quel.
        assert_eq!(anonymize_with("hote steamdeck / SteamDeck / steamdeckX", "", "", "steamdeck"), "hote <pc> / <pc> / steamdeckX");
        // Nom de repli ou trop court : rien n'est remplacé.
        assert_eq!(anonymize_with("Ordinateur pc", "", "", "Ordinateur"), "Ordinateur pc");
        assert_eq!(anonymize_with("pc", "", "", "pc"), "pc");
    }

    #[test]
    fn journaux_patterns_limited_to_game_and_emulator() {
        let roots = vec!["lichii".to_string(), "bbport".to_string()];
        // Dossier du jeu ou de l'émulateur : toujours.
        assert!(pattern_allowed("user/log/*.txt", &roots));
        assert!(pattern_allowed("{exedir}/user/config/qt-config.ini", &roots));
        // Dossier connu de l'émulateur (identifiant, `lien:`, `dossier-mods:`…), casse ignorée.
        assert!(pattern_allowed("{appdata}/lichii/nx2_switches.txt", &roots));
        assert!(pattern_allowed("{localappdata}/BBPort/logs/*.log", &roots));
        // Autre dossier : refusé.
        assert!(!pattern_allowed("{documents}/*.docx", &roots));
        assert!(!pattern_allowed("{appdata}/autre/x.txt", &roots));
        assert!(!pattern_allowed("{config}/lichii", &roots), "fichier directement dans le joker");
        // Dossiers du launcher : jamais, même déclarés « connus ».
        let bad = vec!["fr.farfaboulette.farfalauncher".to_string(), "farfalauncherdeck".to_string()];
        assert!(!pattern_allowed("{config}/fr.farfaboulette.farfalauncher/token.json", &bad));
        assert!(!pattern_allowed("{config}/farfalauncherdeck/token", &bad));
        // Joker inconnu ou mal formé.
        assert!(!pattern_allowed("{home}/lichii/x.txt", &roots));
        assert!(!pattern_allowed("{appdata", &roots));
    }

    #[test]
    fn known_roots_from_joker_paths() {
        assert_eq!(joker_root("{appdata}/Lichii/x").as_deref(), Some("lichii"));
        assert_eq!(joker_root("{localappdata}/boiii").as_deref(), Some("boiii"));
        assert_eq!(joker_root("{exedir}/user"), None);
        assert_eq!(joker_root("user/load"), None);
    }

    #[test]
    fn never_inside_launcher_dirs() {
        let base = std::env::temp_dir().join(format!("farfa-report-in-{}", std::process::id()));
        let config = base.join("fr.farfaboulette.farfalauncher");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("token.json"), "{}").unwrap();
        assert!(is_inside(&config.join("token.json"), &config));
        assert!(!is_inside(&base.join("jeu.log"), &config));
        assert!(!is_inside(&config.join("x"), Path::new("")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn finds_logs_by_pattern() {
        let dir = std::env::temp_dir().join(format!("farfa-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("user/log")).unwrap();
        std::fs::write(dir.join("user/log/eden_log.txt"), "a").unwrap();
        std::fs::write(dir.join("user/log/autre.bin"), "b").unwrap();
        let found = find(&LogPattern { system: None, pattern: "user/log/*.txt".into() }, &dir);
        assert_eq!(found, vec![dir.join("user").join("log").join("eden_log.txt")]);
        assert!(find(&LogPattern { system: None, pattern: "absent/*.txt".into() }, &dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
