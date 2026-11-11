//! Citron et Eden (dérivés de Yuzu) : mises à jour et DLC d'un jeu Switch lus directement
//! dans son dossier, sans « Installer dans la NAND ». Ces émulateurs parcourent leurs
//! « dossiers de contenu externe » (`[UI] Paths\external_content_dirs` de `qt-config.ini`) au
//! démarrage et y prennent les NSP/XCI de mise à jour et de DLC.
//!
//! Avant chaque partie, le dossier du jeu lancé est mis dans cette liste. Les dossiers ajoutés
//! par l'utilisateur ou par l'émulateur (Citron y range `sdmc/citron/content`) sont gardés ;
//! ceux d'autres jeux du launcher (sous le même dossier `Roms`) sont retirés, pour que
//! l'émulateur ne parcoure pas toute la bibliothèque à chaque démarrage.

use std::path::Path;

use crate::language::set_ini;

const SECTION: &str = "UI";
const LIST: &str = "Paths\\external_content_dirs";

/// Chemin tel que l'écrit l'émulateur (barres obliques).
fn qt_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn inside(path: &str, root: &str) -> bool {
    let (p, r) = (path.trim_end_matches('/'), root.trim_end_matches('/'));
    let (p, r) = if cfg!(windows) { (p.to_lowercase(), r.to_lowercase()) } else { (p.to_string(), r.to_string()) };
    p == r || p.starts_with(&format!("{r}/"))
}

fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut current = "";
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            current = &t[1..t.len() - 1];
        } else if current == SECTION {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

/// Nouveau texte de `qt-config.ini` avec `game_dir` dans les dossiers de contenu externe.
/// `library` : dossier qui contient ceux de tous les jeux du launcher.
pub fn with_game_dir(text: &str, game_dir: &Path, library: &Path) -> String {
    let (game, library) = (qt_path(game_dir), qt_path(library));
    let size: usize = value(text, &format!("{LIST}\\size")).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut dirs: Vec<String> = (1..=size)
        .filter_map(|i| value(text, &format!("{LIST}\\{i}\\path")).map(str::to_string))
        .filter(|d| !d.is_empty() && !inside(d, &library))
        .collect();
    dirs.push(game);
    let mut out = text.to_string();
    for (i, dir) in dirs.iter().enumerate() {
        out = set_ini(&out, &format!("{SECTION}.{LIST}\\{}\\path", i + 1), dir);
    }
    // Les entrées au-delà de `size` ne sont plus lues par l'émulateur.
    set_ini(&out, &format!("{SECTION}.{LIST}\\size"), &dirs.len().to_string())
}

/// Met le dossier du jeu dans la configuration de l'émulateur (dans `exe_dir`, mode portable).
pub fn apply(exe_dir: &Path, rom_dir: &Path) -> Result<(), String> {
    let path = exe_dir.join("user").join("config").join("qt-config.ini");
    let library = rom_dir.parent().and_then(Path::parent).unwrap_or(rom_dir);
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let after = with_game_dir(&before, rom_dir, library);
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

    #[test]
    fn keeps_other_dirs_and_replaces_the_previous_game() {
        let lib = Path::new("C:/Jeux/Roms");
        let citron = "[UI]\nPaths\\language=fr\nPaths\\external_content_dirs\\size=1\nPaths\\external_content_dirs\\1\\path=C:/Jeux/Emulateurs/citron/user/sdmc/citron/content\n[System]\nx=1\n";
        let first = with_game_dir(citron, Path::new("C:/Jeux/Roms/switch/Zelda BOTW"), lib);
        assert_eq!(value(&first, "Paths\\external_content_dirs\\size"), Some("2"));
        assert_eq!(value(&first, "Paths\\external_content_dirs\\1\\path"), Some("C:/Jeux/Emulateurs/citron/user/sdmc/citron/content"));
        assert_eq!(value(&first, "Paths\\external_content_dirs\\2\\path"), Some("C:/Jeux/Roms/switch/Zelda BOTW"));
        assert!(first.contains("[System]\nx=1"));
        // Autre jeu ensuite : il remplace le précédent, le dossier de Citron reste.
        let second = with_game_dir(&first, Path::new("C:\\Jeux\\Roms\\switch\\Castlevania"), lib);
        assert_eq!(value(&second, "Paths\\external_content_dirs\\size"), Some("2"));
        assert_eq!(value(&second, "Paths\\external_content_dirs\\2\\path"), Some("C:/Jeux/Roms/switch/Castlevania"));
        // Déjà en place : rien ne change.
        assert_eq!(with_game_dir(&second, Path::new("C:/Jeux/Roms/switch/Castlevania"), lib), second);
        // Configuration encore absente (premier lancement).
        let fresh = with_game_dir("", Path::new("C:/Jeux/Roms/switch/Kirby"), lib);
        assert_eq!(value(&fresh, "Paths\\external_content_dirs\\1\\path"), Some("C:/Jeux/Roms/switch/Kirby"));
        assert_eq!(value(&fresh, "Paths\\external_content_dirs\\size"), Some("1"));
    }
}
