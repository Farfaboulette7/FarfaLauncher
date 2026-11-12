//! Français par défaut. Chaque recomp a sa façon de choisir sa langue : FarfaRomManager
//! décrit la sienne (fichier de réglages, clé, valeur) et le launcher la pose avant le
//! lancement. Une fois par installation : le joueur reste libre d'en changer ensuite.
//!
//! Sous Linux, tout jeu lancé reçoit aussi une locale française : beaucoup de jeux et
//! Proton choisissent leur langue d'après elle.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::install::extract::INTERNAL_DIR;

/// Règles déjà posées pour cette installation (leur signature).
const APPLIED_FILE: &str = "langue";
/// Locale donnée aux jeux sous Linux.
#[cfg(all(unix, not(target_os = "macos")))]
const LINUX_LOCALE: &str = "fr_FR.UTF-8";

/// Une règle de FarfaRomManager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageRule {
    /// `windows`, `linux` ; absent : tous les systèmes.
    #[serde(default)]
    pub system: Option<String>,
    /// `env`, `ini` (et TOML), `kv` (« clé valeur »), `json`.
    pub format: String,
    /// Fichier de réglages, avec `{exedir}`, `{appdata}`, `{config}`, `{documents}`.
    #[serde(default)]
    pub file: Option<String>,
    /// `section.clé` (ini), `a/b` (json), ou nom de la variable (env).
    pub key: String,
    /// Écrite telle quelle (ini, kv), lue comme du JSON (json).
    pub value: String,
}

/// Nom de ce système dans les fichiers du gestionnaire.
pub fn current_system() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

impl LanguageRule {
    fn for_this_system(&self) -> bool {
        self.system.as_deref().is_none_or(|s| s == current_system())
    }
}

/// Variables d'environnement à donner au jeu, et fichiers posés si ce n'est pas déjà fait.
pub fn prepare(game_dir: &Path, exe_dir: &Path, rules: &[LanguageRule]) -> Vec<(String, String)> {
    let rules: Vec<&LanguageRule> = rules.iter().filter(|r| r.for_this_system()).collect();
    // `{exedir}` dans une variable : le dossier de l'exécutable (Savage Wind sous Linux y garde ses données).
    let env = rules
        .iter()
        .filter(|r| r.format == "env")
        .map(|r| (r.key.clone(), r.value.replace("{exedir}", &exe_dir.to_string_lossy())))
        .collect();
    let files: Vec<&LanguageRule> = rules.into_iter().filter(|r| r.format != "env").collect();
    if files.is_empty() {
        return env;
    }
    let marker = game_dir.join(INTERNAL_DIR).join(APPLIED_FILE);
    let signature = serde_json::to_string(&files).unwrap_or_default();
    if std::fs::read_to_string(&marker).ok().as_deref() == Some(signature.as_str()) {
        return env;
    }
    let mut all_done = true;
    for rule in files {
        match apply(rule, exe_dir) {
            Ok(path) => tracing::info!("langue : {} = {} dans {}", rule.key, rule.value, path.display()),
            Err(e) => {
                all_done = false;
                tracing::warn!("langue non posée ({} {}) : {e}", rule.format, rule.key);
            }
        }
    }
    if all_done {
        if let Err(e) = std::fs::write(&marker, signature) {
            tracing::warn!("langue posée mais non mémorisée : {e}");
        }
    }
    env
}

/// Sous Linux : locale française, sauf si le système en a déjà une.
pub fn linux_locale(cmd: &mut std::process::Command) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let current = std::env::var("LC_ALL").ok().filter(|v| !v.is_empty()).or_else(|| std::env::var("LANG").ok());
        if current.as_deref().is_some_and(|l| l.starts_with("fr")) {
            return;
        }
        cmd.env("LANG", LINUX_LOCALE).env("LANGUAGE", "fr").env("LC_MESSAGES", LINUX_LOCALE).env_remove("LC_ALL");
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = cmd;
}

fn apply(rule: &LanguageRule, exe_dir: &Path) -> Result<PathBuf, String> {
    let file = rule.file.as_deref().ok_or("fichier manquant")?;
    let path = expand(file, exe_dir).ok_or("dossier introuvable")?;
    let old = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let new = match rule.format.as_str() {
        "ini" => set_ini(&old, &rule.key, &rule.value),
        "kv" => set_kv(&old, &rule.key, &rule.value),
        "json" => set_json(&old, &rule.key, &rule.value)?,
        other => return Err(format!("format inconnu : {other}")),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, new).map_err(|e| e.to_string())?;
    Ok(path)
}

/// `{exedir}/…`, `{appdata}/…`, `{config}/…` ou `{documents}/…` en chemin de ce système.
pub fn expand(file: &str, exe_dir: &Path) -> Option<PathBuf> {
    let (var, rest) = file.strip_prefix('{').and_then(|f| f.split_once('}'))?;
    let base = match var {
        "exedir" => exe_dir.to_path_buf(),
        "appdata" => appdata()?,
        "config" => config_dir()?,
        "documents" => documents()?,
        _ => return None,
    };
    let rest = rest.trim_start_matches('/');
    // Jamais en dehors du dossier de départ.
    if rest.split('/').any(|p| p == ".." || p.is_empty()) {
        return None;
    }
    Some(rest.split('/').fold(base, |p, part| p.join(part)))
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `%APPDATA%`, `~/Library/Application Support` sur Mac, `~/.local/share` sous Linux.
fn appdata() -> Option<PathBuf> {
    if cfg!(windows) {
        env_dir("APPDATA")
    } else if cfg!(target_os = "macos") {
        // Dossier des préférences SDL et de la plupart des portages sur Mac.
        env_dir("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env_dir("XDG_DATA_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".local/share")))
    }
}

/// `%APPDATA%`, `~/.config` sous Linux.
fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        env_dir("APPDATA")
    } else {
        env_dir("XDG_CONFIG_HOME").or_else(|| env_dir("HOME").map(|h| h.join(".config")))
    }
}

/// Documents, même déplacé (OneDrive…).
fn documents() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
        let mut raw: windows_sys::core::PWSTR = std::ptr::null_mut();
        // SAFETY : chemin rendu par le système, libéré une fois copié.
        unsafe {
            if SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw) == 0 && !raw.is_null() {
                let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
                let path = PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(raw, len)));
                CoTaskMemFree(raw as *const _);
                return Some(path);
            }
        }
        env_dir("USERPROFILE").map(|h| h.join("Documents"))
    }
    #[cfg(not(windows))]
    env_dir("HOME").map(|h| h.join("Documents"))
}

/// INI ou TOML : `section.clé` (ou `clé` avant toute section), remplacée ou ajoutée.
pub(crate) fn set_ini(text: &str, key: &str, value: &str) -> String {
    let (section, name) = match key.rsplit_once('.') {
        Some((s, n)) => (Some(s), n),
        None => (None, key),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let mut current: Option<String> = None;
    let mut section_end = None;
    for (i, line) in lines.iter_mut().enumerate() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if section_end.is_none() && current.as_deref() == section {
                section_end = Some(i);
            }
            current = Some(t[1..t.len() - 1].trim().to_string());
            continue;
        }
        if current.as_deref() == section && t.split(['=']).next().map(str::trim) == Some(name) && t.contains('=') {
            *line = format!("{name}={value}");
            return join_lines(lines, text);
        }
    }
    let entry = format!("{name}={value}");
    let seen = section.is_none() || text.lines().any(|l| l.trim() == format!("[{}]", section.unwrap_or_default()));
    match (section, seen) {
        // Section présente : à sa fin (avant la suivante, ou en fin de fichier).
        (Some(_), true) => lines.insert(section_end.unwrap_or(lines.len()), entry),
        (Some(s), false) => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{s}]"));
            lines.push(entry);
        }
        // Sans section : avant la première.
        (None, _) => {
            let first = lines.iter().position(|l| l.trim().starts_with('[')).unwrap_or(lines.len());
            lines.insert(first, entry);
        }
    }
    join_lines(lines, text)
}

/// Une ligne « clé valeur » par réglage (sm64coopdx).
fn set_kv(text: &str, key: &str, value: &str) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match lines.iter_mut().find(|l| l.split_whitespace().next() == Some(key)) {
        Some(line) => *line = format!("{key} {value}"),
        None => lines.push(format!("{key} {value}")),
    }
    join_lines(lines, text)
}

/// `a/b` : objets imbriqués ; une clé à point (`game.language`) reste une seule clé.
fn set_json(text: &str, key: &str, value: &str) -> Result<String, String> {
    let mut doc: serde_json::Value = if text.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(text).map_err(|e| format!("JSON illisible : {e}"))?
    };
    let value: serde_json::Value = serde_json::from_str(value).map_err(|e| format!("valeur JSON invalide : {e}"))?;
    let mut node = &mut doc;
    let parts: Vec<&str> = key.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        let obj = node.as_object_mut().ok_or("réglages JSON inattendus")?;
        if i == parts.len() - 1 {
            obj.insert(part.to_string(), value);
            break;
        }
        node = obj.entry(part.to_string()).or_insert_with(|| serde_json::json!({}));
    }
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

fn join_lines(lines: Vec<String>, original: &str) -> String {
    let newline = if original.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = lines.join(newline);
    out.push_str(newline);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_and_toml() {
        // Clé déjà là : remplacée, le reste intact.
        assert_eq!(set_ini("[game]\nlang=\"en\"\nx=1\n", "game.lang", "\"fr\""), "[game]\nlang=\"fr\"\nx=1\n");
        // Section présente sans la clé : ajoutée dans la section, pas dans la suivante.
        assert_eq!(set_ini("[game]\nx=1\n[video]\ny=2\n", "game.language", "\"fr\""), "[game]\nx=1\nlanguage=\"fr\"\n[video]\ny=2\n");
        // Section absente : créée.
        assert_eq!(set_ini("[video]\ny=2\n", "game.language", "\"fr\""), "[video]\ny=2\n\n[game]\nlanguage=\"fr\"\n");
        // Sans section (larecomp.ini), espaces autour du = acceptés.
        assert_eq!(set_ini("volume = 1\nlanguage = auto\n", "language", "fr"), "volume = 1\nlanguage=fr\n");
        assert_eq!(set_ini("", "language", "fr"), "language=fr\n");
        assert_eq!(set_ini("a=1\r\n", "language", "fr"), "a=1\r\nlanguage=fr\r\n");
        assert_eq!(set_ini("a=1\n[s]\nb=2\n", "language", "fr"), "a=1\nlanguage=fr\n[s]\nb=2\n");
        // « languageX » n'est pas « language ».
        assert_eq!(set_ini("languageX=1\n", "language", "fr"), "languageX=1\nlanguage=fr\n");
    }

    #[test]
    fn key_value_lines() {
        assert_eq!(set_kv("fullscreen true\nlanguage English\n", "language", "French"), "fullscreen true\nlanguage French\n");
        assert_eq!(set_kv("", "language", "French"), "language French\n");
    }

    #[test]
    fn json_settings() {
        // Clé plate à point (Dusklight) et chemin imbriqué (SMS Launcher).
        assert_eq!(serde_json::from_str::<serde_json::Value>(&set_json(r#"{"game.language":0,"a":1}"#, "game.language", "2").unwrap()).unwrap(), serde_json::json!({"game.language": 2, "a": 1}));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&set_json("", "settings/language", "\"fr\"").unwrap()).unwrap(), serde_json::json!({"settings": {"language": "fr"}}));
        assert!(set_json("pas du json", "a", "1").is_err());
    }

    #[test]
    fn paths_stay_inside_their_folder() {
        let exe = Path::new("jeu");
        assert_eq!(expand("{exedir}/larecomp.ini", exe), Some(exe.join("larecomp.ini")));
        assert_eq!(expand("{exedir}/../x", exe), None);
        assert_eq!(expand("{inconnu}/x", exe), None);
        assert_eq!(expand("/etc/x", exe), None);
    }

    #[test]
    fn applies_once_per_installation() {
        let base = std::env::temp_dir().join(format!("farfa-langue-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join(INTERNAL_DIR)).unwrap();
        let rules = vec![
            LanguageRule { system: None, format: "ini".into(), file: Some("{exedir}/larecomp.ini".into()), key: "language".into(), value: "fr".into() },
            LanguageRule { system: None, format: "env".into(), file: None, key: "MHGU_LANG".into(), value: "fr".into() },
            LanguageRule { system: Some("autre".into()), format: "env".into(), file: None, key: "X".into(), value: "1".into() },
        ];
        assert_eq!(prepare(&base, &base, &rules), [("MHGU_LANG".to_string(), "fr".to_string())]);
        assert_eq!(std::fs::read_to_string(base.join("larecomp.ini")).unwrap(), "language=fr\n");
        // Le joueur change de langue : elle n'est plus touchée.
        std::fs::write(base.join("larecomp.ini"), "language=en\n").unwrap();
        prepare(&base, &base, &rules);
        assert_eq!(std::fs::read_to_string(base.join("larecomp.ini")).unwrap(), "language=en\n");
        std::fs::remove_dir_all(&base).unwrap();
    }
}

#[cfg(test)]
mod live_catalog {
    /// Catalogue réel enregistré : `FARFA_E2E_CATALOG=<fichier> cargo test -- --ignored live_language_rules`.
    #[test]
    #[ignore]
    fn live_language_rules() {
        let text = std::fs::read_to_string(std::env::var("FARFA_E2E_CATALOG").unwrap()).unwrap();
        let catalog: crate::manager::Catalog = serde_json::from_str(&text).unwrap();
        let rules: Vec<_> = catalog.recomps.iter().flat_map(|r| r.language.iter().map(move |l| (r.id.clone(), l))).collect();
        assert_eq!(catalog.recomps.iter().filter(|r| !r.language.is_empty()).count(), 7);
        for (id, rule) in rules {
            let target = rule.file.as_deref().map(|f| super::expand(f, std::path::Path::new("<exe>")));
            println!("{id}: {} {:?} {} = {} -> {:?}", rule.format, rule.system, rule.key, rule.value, target);
            if rule.format != "env" {
                assert!(target.unwrap().is_some(), "{id} : dossier introuvable");
            }
        }
    }
}
