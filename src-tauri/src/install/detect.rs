//! Détection de l'exécutable d'un jeu installé (c'est le launcher qui décide,
//! pas le serveur).
//!
//! - Windows : fichiers `.exe`, hors désinstalleurs, crash handlers, updaters,
//!   redistribuables…
//! - Linux / SteamOS : `.AppImage` en priorité, sinon binaire ELF ; les `.so`
//!   et scripts ne sont retenus que s'il n'y a rien d'autre.
//! - macOS : bundle `.app`.
//!
//! S'il reste plusieurs candidats, l'utilisateur choisit (une fois).
//!
//! Point d'extension : `GameOverrides` représentera un futur fichier optionnel
//! de surcharge par jeu (exécutable forcé, nom de ROM attendu). Il n'est pas
//! encore lu : `GameOverrides::load` renvoie toujours la valeur par défaut.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::install::extract::{INTERNAL_DIR, MODS_DIR};
use crate::platform::Target;

/// Profondeur maximale explorée (les jeux rangent rarement leur exe plus bas).
const MAX_DEPTH: usize = 5;

/// Surcharges par jeu (non implémentées : point d'extension prévu).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GameOverrides {
    /// Exécutable imposé, relatif au dossier du jeu.
    pub executable: Option<String>,
    /// Nom sous lequel la ROM doit être copiée.
    pub rom_name: Option<String>,
}

impl GameOverrides {
    /// Futur : lire un fichier de surcharge (local ou fourni par le serveur).
    pub fn load(_game_dir: &Path) -> Self {
        Self::default()
    }
}

/// Mots qui désignent un exécutable annexe sous Windows.
const WINDOWS_EXCLUDED_WORDS: &[&str] = &[
    "unins", "uninstall", "uninstaller", "setup", "installer", "install", "crash", "crashpad", "crashreporter",
    "crashhandler", "reporter", "bugreport", "updater", "update", "autoupdate", "redist", "vcredist", "vc_redist",
    "dxsetup", "dxwebsetup", "directx", "dotnet", "dotnetfx", "ndp", "physx", "oalinst", "ue4prereq", "prereq",
    "helper", "elevate", "notification_helper", "cefsharp", "subprocess", "quicksfv", "7z", "7za", "unitycrashhandler",
    "unitycrashhandler64", "createdump",
];

/// Dossiers dont les exécutables ne sont jamais le jeu.
const EXCLUDED_DIRS: &[&str] = &[
    "redist", "_commonredist", "commonredist", "__installer", "installer", "directx", "vcredist", "dotnet", "support",
    "prerequisites", "_redist", "crashpad", "tools",
];

pub fn find_candidates(game_dir: &Path, target: Target) -> Vec<PathBuf> {
    let overrides = GameOverrides::load(game_dir);
    if let Some(forced) = overrides.executable {
        let p = PathBuf::from(forced);
        if game_dir.join(&p).exists() {
            return vec![p];
        }
    }
    let mut files = Vec::new();
    walk(game_dir, Path::new(""), 0, target, &mut files);
    let mut candidates = match target {
        Target::Windows => files.into_iter().filter(|p| is_windows_game_exe(p)).collect(),
        Target::Linux | Target::SteamOS => linux_candidates(game_dir, files),
        Target::MacArm | Target::MacIntel => files,
    };
    // Les moins profonds d'abord, puis par nom.
    candidates.sort_by_key(|p: &PathBuf| (p.components().count(), p.to_string_lossy().to_lowercase()));
    candidates
}

fn walk(root: &Path, rel: &Path, depth: usize, target: Target, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(root.join(rel)) else { return };
    for entry in rd.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        let child = rel.join(&name);
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth == 0 && (lower == INTERNAL_DIR || lower == MODS_DIR) {
                continue;
            }
            if lower == "save" || lower == "saves" || EXCLUDED_DIRS.contains(&lower.as_str()) {
                continue;
            }
            if matches!(target, Target::MacArm | Target::MacIntel) && lower.ends_with(".app") {
                // On ne descend pas dans un bundle : il est lui-même le candidat.
                if !lower.contains("uninstall") {
                    out.push(child);
                }
                continue;
            }
            walk(root, &child, depth + 1, target, out);
        } else if ft.is_file() && !matches!(target, Target::MacArm | Target::MacIntel) {
            out.push(child);
        }
    }
}

fn words(stem: &str) -> Vec<String> {
    stem.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn is_windows_game_exe(rel: &Path) -> bool {
    let name = rel.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let Some(stem) = name.strip_suffix(".exe") else { return false };
    // Désinstalleurs Inno Setup : unins000.exe, unins001.exe…
    if WINDOWS_EXCLUDED_WORDS.contains(&stem) || stem.starts_with("unins") {
        return false;
    }
    !words(stem).iter().any(|w| WINDOWS_EXCLUDED_WORDS.contains(&w.as_str()))
}

fn linux_candidates(game_dir: &Path, files: Vec<PathBuf>) -> Vec<PathBuf> {
    let lower = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let appimages: Vec<PathBuf> = files.iter().filter(|p| lower(p).ends_with(".appimage")).cloned().collect();
    if !appimages.is_empty() {
        return appimages;
    }
    let is_lib = |n: &str| n.ends_with(".so") || n.contains(".so.");
    let elves: Vec<PathBuf> = files
        .iter()
        .filter(|p| {
            let n = lower(p);
            !is_lib(&n) && !n.contains("crash") && is_elf_executable(&game_dir.join(p))
        })
        .cloned()
        .collect();
    if !elves.is_empty() {
        return elves;
    }
    // Rien d'autre : scripts de lancement, puis bibliothèques en dernier recours.
    let scripts: Vec<PathBuf> = files.iter().filter(|p| lower(p).ends_with(".sh")).cloned().collect();
    if !scripts.is_empty() {
        return scripts;
    }
    files.into_iter().filter(|p| is_elf_executable(&game_dir.join(p))).collect()
}

/// En-tête ELF de type exécutable (ET_EXEC) ou PIE (ET_DYN).
fn is_elf_executable(path: &Path) -> bool {
    let mut header = [0u8; 18];
    let Ok(mut f) = fs::File::open(path) else { return false };
    if f.read_exact(&mut header).is_err() || &header[..4] != b"\x7fELF" {
        return false;
    }
    let e_type = if header[5] == 2 { u16::from_be_bytes([header[16], header[17]]) } else { u16::from_le_bytes([header[16], header[17]]) };
    matches!(e_type, 2 | 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("farfa-detect-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn windows_filters_annex_executables() {
        let g = tmp("win");
        for f in [
            "Game.exe",
            "unins000.exe",
            "UnityCrashHandler64.exe",
            "updater.exe",
            "_CommonRedist/vcredist/vc_redist.x64.exe",
            "redist/DXSETUP.exe",
            "crashpad_handler.exe",
            "bin/notification_helper.exe",
            "saves/backup.exe",
            "mods_downloads/tool.exe",
            ".farfalauncher/staging/x.exe",
            "readme.txt",
        ] {
            touch(&g, f, b"MZ");
        }
        assert_eq!(find_candidates(&g, Target::Windows), [PathBuf::from("Game.exe")]);
    }

    #[test]
    fn windows_several_candidates_are_all_returned() {
        let g = tmp("win2");
        touch(&g, "sm64coopdx.exe", b"MZ");
        touch(&g, "tools2/editor.exe", b"MZ");
        assert_eq!(find_candidates(&g, Target::Windows).len(), 2);
    }

    #[test]
    fn linux_prefers_appimage_then_elf() {
        let g = tmp("lin");
        let mut elf = b"\x7fELF\x02\x01\x01".to_vec();
        elf.resize(16, 0);
        elf.extend_from_slice(&[3, 0]); // ET_DYN (PIE), little-endian
        touch(&g, "game", &elf);
        touch(&g, "libfoo.so", &elf);
        touch(&g, "libbar.so.1", &elf);
        touch(&g, "run.sh", b"#!/bin/sh");
        assert_eq!(find_candidates(&g, Target::Linux), [PathBuf::from("game")]);
        touch(&g, "Game-x86_64.AppImage", &elf);
        assert_eq!(find_candidates(&g, Target::SteamOS), [PathBuf::from("Game-x86_64.AppImage")]);
    }

    #[test]
    fn linux_falls_back_to_scripts() {
        let g = tmp("lin2");
        touch(&g, "start.sh", b"#!/bin/sh");
        touch(&g, "data.pak", b"x");
        assert_eq!(find_candidates(&g, Target::Linux), [PathBuf::from("start.sh")]);
    }

    #[test]
    fn mac_finds_app_bundles() {
        let g = tmp("mac");
        touch(&g, "Game.app/Contents/MacOS/Game", b"x");
        touch(&g, "Game.app/Contents/Frameworks/Helper.app/Contents/MacOS/h", b"x");
        touch(&g, "Uninstall.app/Contents/MacOS/u", b"x");
        assert_eq!(find_candidates(&g, Target::MacArm), [PathBuf::from("Game.app")]);
    }
}
