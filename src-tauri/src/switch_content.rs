//! Mises à jour et DLC d'un jeu lus directement dans son dossier, sans « Installer dans la
//! NAND » (Citron, Eden et autres dérivés de Yuzu). Ces émulateurs parcourent leurs « dossiers
//! de contenu externe » (`[UI] Paths\external_content_dirs` de `qt-config.ini`) au démarrage et
//! y prennent les NSP/XCI de mise à jour et de DLC.
//!
//! La règle vient de la fiche de l'émulateur (`contenu-externe: ini | fichier | section.clé`,
//! écart E6), et non plus de sa console : voir `Catalog::external_content` (repli pour un
//! ancien gestionnaire).
//!
//! Avant chaque partie, le dossier du jeu lancé est mis dans cette liste. Les dossiers ajoutés
//! par l'utilisateur ou par l'émulateur (Citron y range `sdmc/citron/content`) sont gardés ;
//! ceux d'autres jeux du launcher (sous le même dossier `Roms`) sont retirés, pour que
//! l'émulateur ne parcoure pas toute la bibliothèque à chaque démarrage.

use std::path::Path;

use crate::language::set_ini;
use crate::manager::ExternalContent;

/// Chemin tel que l'écrit l'émulateur (barres obliques).
fn qt_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn inside(path: &str, root: &str) -> bool {
    let (p, r) = (path.trim_end_matches('/'), root.trim_end_matches('/'));
    let (p, r) = if cfg!(windows) { (p.to_lowercase(), r.to_lowercase()) } else { (p.to_string(), r.to_string()) };
    p == r || p.starts_with(&format!("{r}/"))
}

fn value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut current = "";
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            current = &t[1..t.len() - 1];
        } else if current == section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

/// Nouveau texte du fichier ini avec `game_dir` dans la liste `key` (`section.liste`).
/// `library` : dossier qui contient ceux de tous les jeux du launcher.
pub fn with_game_dir(text: &str, key: &str, game_dir: &Path, library: &Path) -> String {
    let (section, list) = key.split_once('.').unwrap_or(("UI", key));
    let (game, library) = (qt_path(game_dir), qt_path(library));
    let size: usize = value(text, section, &format!("{list}\\size")).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut dirs: Vec<String> = (1..=size)
        .filter_map(|i| value(text, section, &format!("{list}\\{i}\\path")).map(str::to_string))
        .filter(|d| !d.is_empty() && !inside(d, &library))
        .collect();
    dirs.push(game);
    let mut out = text.to_string();
    for (i, dir) in dirs.iter().enumerate() {
        out = set_ini(&out, &format!("{section}.{list}\\{}\\path", i + 1), dir);
    }
    // Les entrées au-delà de `size` ne sont plus lues par l'émulateur.
    set_ini(&out, &format!("{section}.{list}\\size"), &dirs.len().to_string())
}

/// Met le dossier du jeu dans la configuration de l'émulateur (`rule.file`, `{exedir}` = `exe_dir`).
pub fn apply(rule: &ExternalContent, exe_dir: &Path, rom_dir: &Path) -> Result<(), String> {
    if rule.format != "ini" {
        return Err(format!("format de contenu externe inconnu : {}", rule.format));
    }
    let path = crate::language::expand(&rule.file, exe_dir).ok_or_else(|| format!("fichier refusé : {}", rule.file))?;
    let library = rom_dir.parent().and_then(Path::parent).unwrap_or(rom_dir);
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let after = with_game_dir(&before, &rule.key, rom_dir, library);
    if after == before {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, after).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "UI.Paths\\external_content_dirs";

    fn value(text: &str, key: &str) -> Option<String> {
        super::value(text, "UI", key).map(str::to_string)
    }

    fn with_game_dir(text: &str, game_dir: &Path, library: &Path) -> String {
        super::with_game_dir(text, KEY, game_dir, library)
    }

    // Écart E6 : la règle de la fiche (fichier et clé) est suivie, ailleurs que dans user/config.
    #[test]
    fn rule_of_the_card_is_followed() {
        let dir = std::env::temp_dir().join(format!("farfa-contenu-{}", std::process::id()));
        let rule = ExternalContent { format: "ini".into(), file: "{exedir}/cfg/emu.ini".into(), key: "Chemins.contenu".into() };
        apply(&rule, &dir, Path::new("C:/Jeux/Roms/switch-2/Mario")).unwrap();
        let text = std::fs::read_to_string(dir.join("cfg").join("emu.ini")).unwrap();
        assert_eq!(super::value(&text, "Chemins", "contenu\\1\\path"), Some("C:/Jeux/Roms/switch-2/Mario"));
        assert_eq!(super::value(&text, "Chemins", "contenu\\size"), Some("1"));
        let bad = ExternalContent { format: "json".into(), ..rule.clone() };
        assert!(apply(&bad, &dir, Path::new("C:/Jeux/Roms/x/y")).is_err());
        let outside = ExternalContent { file: "{exedir}/../emu.ini".into(), ..rule };
        assert!(apply(&outside, &dir, Path::new("C:/Jeux/Roms/x/y")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_other_dirs_and_replaces_the_previous_game() {
        let lib = Path::new("C:/Jeux/Roms");
        let citron = "[UI]\nPaths\\language=fr\nPaths\\external_content_dirs\\size=1\nPaths\\external_content_dirs\\1\\path=C:/Jeux/Emulateurs/citron/user/sdmc/citron/content\n[System]\nx=1\n";
        let first = with_game_dir(citron, Path::new("C:/Jeux/Roms/switch/Zelda BOTW"), lib);
        assert_eq!(value(&first, "Paths\\external_content_dirs\\size"), Some("2".to_string()));
        assert_eq!(value(&first, "Paths\\external_content_dirs\\1\\path"), Some("C:/Jeux/Emulateurs/citron/user/sdmc/citron/content".to_string()));
        assert_eq!(value(&first, "Paths\\external_content_dirs\\2\\path"), Some("C:/Jeux/Roms/switch/Zelda BOTW".to_string()));
        assert!(first.contains("[System]\nx=1"));
        // Autre jeu ensuite : il remplace le précédent, le dossier de Citron reste.
        let second = with_game_dir(&first, Path::new("C:\\Jeux\\Roms\\switch\\Castlevania"), lib);
        assert_eq!(value(&second, "Paths\\external_content_dirs\\size"), Some("2".to_string()));
        assert_eq!(value(&second, "Paths\\external_content_dirs\\2\\path"), Some("C:/Jeux/Roms/switch/Castlevania".to_string()));
        // Déjà en place : rien ne change.
        assert_eq!(with_game_dir(&second, Path::new("C:/Jeux/Roms/switch/Castlevania"), lib), second);
        // Configuration encore absente (premier lancement).
        let fresh = with_game_dir("", Path::new("C:/Jeux/Roms/switch/Kirby"), lib);
        assert_eq!(value(&fresh, "Paths\\external_content_dirs\\1\\path"), Some("C:/Jeux/Roms/switch/Kirby".to_string()));
        assert_eq!(value(&fresh, "Paths\\external_content_dirs\\size"), Some("1".to_string()));
    }
}
