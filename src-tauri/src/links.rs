//! Dossiers hors du jeu reliés à un dossier du jeu (`lien:` des fiches de recomp).
//!
//! Certains programmes écrivent toujours au même endroit (boiii : `%LOCALAPPDATA%\boiii`,
//! ses données et ses plantages). Le launcher remplace ce dossier par un lien (une jonction
//! sous Windows, sans droits d'administrateur) vers un dossier du jeu : tout reste dans le
//! dossier « Jeux ».

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    #[serde(default)]
    pub system: Option<String>,
    /// Dossier hors du jeu : `{localappdata}/boiii`.
    pub path: String,
    /// Dossier du jeu, relatif à celui de l'exécutable.
    pub target: String,
}

/// Pose les liens de ce système. Une erreur est notée au journal sans bloquer le lancement.
pub fn apply(links: &[Link], exe_dir: &Path) {
    for link in links.iter().filter(|l| l.system.as_deref().is_none_or(|s| s == crate::language::current_system())) {
        let Some(path) = crate::language::expand(&link.path, exe_dir) else {
            tracing::warn!("lien ignoré : dossier « {} » incompris", link.path);
            continue;
        };
        let Some(target) = crate::language::expand(&format!("{{exedir}}/{}", link.target), exe_dir) else {
            tracing::warn!("lien ignoré : dossier du jeu « {} » incompris", link.target);
            continue;
        };
        match place(&path, &target) {
            Ok(true) => tracing::info!("lien {} -> {}", path.display(), target.display()),
            Ok(false) => {}
            Err(e) => tracing::warn!("lien {} -> {} non posé: {e}", path.display(), target.display()),
        }
    }
}

/// Retire les liens qui mènent dans ce dossier de jeu (désinstallation) : ils ne pointeraient
/// plus sur rien.
pub fn remove_into(links: &[Link], game_dir: &Path) {
    let Ok(game) = fs::canonicalize(game_dir) else { return };
    for link in links.iter().filter(|l| l.system.as_deref().is_none_or(|s| s == crate::language::current_system())) {
        if let Some(path) = crate::language::expand(&link.path, game_dir) {
            remove_if_inside(&path, &game);
        }
    }
}

fn remove_if_inside(path: &Path, game: &Path) -> bool {
    if !(is_link(path) && fs::canonicalize(path).is_ok_and(|t| t.starts_with(game))) {
        return false;
    }
    match remove_link(path) {
        Ok(()) => {
            tracing::info!("lien {} retiré", path.display());
            true
        }
        Err(e) => {
            tracing::warn!("lien {} non retiré: {e}", path.display());
            false
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false) || fs::read_link(path).is_ok()
}

/// Nom libre à côté de `path` : `boiii.avant-farfa`, puis `boiii.avant-farfa-2`…
fn free_name(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (1..)
        .map(|i| path.with_file_name(if i == 1 { format!("{name}.avant-farfa") } else { format!("{name}.avant-farfa-{i}") }))
        .find(|p| !p.exists())
        .unwrap()
}

fn is_empty_dir(dir: &Path) -> bool {
    fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// `Ok(true)` : lien posé, `Ok(false)` : déjà en place.
fn place(path: &Path, target: &Path) -> std::io::Result<bool> {
    if is_link(path) {
        if same_dir(path, target) {
            return Ok(false);
        }
        // Lien vers une autre installation (ou cassé) : remplacé.
        remove_link(path)?;
    } else if path.is_dir() {
        // Vrai dossier (le programme a déjà tourné hors du launcher) : ses fichiers vont dans le
        // jeu s'il n'y a encore rien, sinon il est mis de côté.
        if is_empty_dir(path) {
            fs::remove_dir(path)?;
        } else if is_empty_dir(target) && (!target.exists() || fs::remove_dir(target).is_ok()) && fs::rename(path, target).is_ok() {
            tracing::info!("{} déplacé dans {}", path.display(), target.display());
        } else {
            let aside = free_name(path);
            fs::rename(path, &aside)?;
            tracing::info!("{} existant mis de côté : {}", path.display(), aside.display());
        }
    } else if path.exists() {
        return Err(std::io::Error::other("un fichier porte déjà ce nom"));
    }
    fs::create_dir_all(target)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    make_link(target, path)?;
    Ok(true)
}

fn remove_link(path: &Path) -> std::io::Result<()> {
    // Jonction ou lien vers un dossier : `remove_dir` ; lien symbolique Unix : `remove_file`.
    fs::remove_dir(path).or_else(|_| fs::remove_file(path))
}

#[cfg(windows)]
fn make_link(target: &Path, link: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Une jonction (mklink /J) ne demande ni droits d'administrateur ni mode développeur.
    let out = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if out.status.success() && link.is_dir() {
        Ok(())
    } else {
        Err(std::io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_moves_then_keeps_then_replaces() {
        let root = std::env::temp_dir().join(format!("farfa-links-{}", uuid::Uuid::new_v4()));
        let outside = root.join("appdata").join("boiii");
        let game = root.join("jeu");
        fs::create_dir_all(outside.join("data")).unwrap();
        fs::write(outside.join("data").join("a.txt"), "a").unwrap();
        fs::create_dir_all(&game).unwrap();

        // Vrai dossier déjà là : son contenu passe dans le jeu.
        assert!(place(&outside, &game.join("boiii")).unwrap());
        assert!(is_link(&outside));
        assert_eq!(fs::read_to_string(game.join("boiii").join("data").join("a.txt")).unwrap(), "a");
        fs::write(outside.join("b.txt"), "b").unwrap();
        assert!(game.join("boiii").join("b.txt").exists());

        // Déjà en place : rien à faire.
        assert!(!place(&outside, &game.join("boiii")).unwrap());

        // Autre installation : le lien suit le nouveau jeu, l'ancien jeu garde ses fichiers.
        let other = root.join("autre-jeu").join("boiii");
        assert!(place(&outside, &other).unwrap());
        assert!(same_dir(&outside, &other));
        assert!(game.join("boiii").join("b.txt").exists());

        // Vrai dossier rempli alors que le jeu a déjà le sien : mis de côté.
        remove_link(&outside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("c.txt"), "c").unwrap();
        fs::write(other.join("d.txt"), "d").unwrap();
        assert!(place(&outside, &other).unwrap());
        assert!(root.join("appdata").join("boiii.avant-farfa").join("c.txt").exists());
        assert!(same_dir(&outside, &other));

        // Désinstallation d'un autre jeu : le lien reste ; de celui-ci : il part.
        assert!(!remove_if_inside(&outside, &fs::canonicalize(&game).unwrap()));
        assert!(is_link(&outside));
        assert!(remove_if_inside(&outside, &fs::canonicalize(root.join("autre-jeu")).unwrap()));
        assert!(!outside.exists() && other.join("d.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
