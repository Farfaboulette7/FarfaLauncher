//! Dossiers hors du jeu reliés à un dossier du jeu (`lien:` des fiches de recomp).
//!
//! Certains programmes écrivent toujours au même endroit (boiii : `%LOCALAPPDATA%\boiii`,
//! ses données et ses plantages). Le launcher remplace ce dossier par un lien (une jonction
//! sous Windows, sans droits d'administrateur) vers un dossier du jeu : tout reste dans le
//! dossier « Jeux ».
//!
//! Un vrai dossier déjà là (le programme a tourné hors du launcher) passe dans le jeu sans
//! perte, même sur un autre disque : fusion sans écrasement, vérifiée avant d'effacer la source
//! (voir `move_into`). Avant le 25/11/2026, il était mis de côté en `<nom>.avant-farfa`, où le
//! joueur ne voyait plus ses sauvegardes.

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

/// Le dossier de ce jeu sert-il encore à un autre jeu (runtime partagé : launcher Pokemon,
/// émulateur) ? `pages` : jeux installés sur ce dossier (`state::pages_in`, réserve comprise).
/// Ses liens doivent alors rester à la désinstallation de celui-ci.
pub fn used_by_others(pages: &[i64], rom_id: i64) -> bool {
    pages.iter().any(|id| *id != rom_id)
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
pub(crate) fn place(path: &Path, target: &Path) -> std::io::Result<bool> {
    if is_link(path) {
        if same_dir(path, target) {
            return Ok(false);
        }
        // Lien vers une autre installation (ou cassé) : remplacé.
        remove_link(path)?;
    } else if path.is_dir() {
        // Vrai dossier (le programme a déjà tourné hors du launcher) : ses fichiers vont dans le
        // jeu, sans jamais rien perdre (voir `move_into`). En cas d'échec, rien n'est effacé et
        // le lien n'est pas posé : le programme garde son dossier tel quel.
        if is_empty_dir(path) {
            fs::remove_dir(path)?;
        } else {
            move_into(path, target)?;
            tracing::info!("{} déplacé dans {}", path.display(), target.display());
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

/// Fait passer le contenu du vrai dossier `src` dans `dst` (dossier du jeu), puis efface `src`.
///
/// - `dst` absent ou vide : simple déplacement (`rename`). S'il échoue (autre disque : dossier
///   « Jeux » sur D: ou sur la carte SD du Deck), copie, vérification, puis effacement.
/// - `dst` a déjà des fichiers : **fusion sans écrasement**. Un fichier absent du jeu y est
///   copié ; un fichier identique n'est pas recopié ; un fichier différent est gardé à côté,
///   sous `nom.avant-farfa` (puis `-2`…), dans le dossier du jeu, où le joueur le voit. Le
///   dossier du jeu reste celui qui sert (c'est lui que les sauvegardes du launcher suivent).
///
/// `src` n'est effacé qu'après vérification : chaque fichier copié existe à destination avec la
/// même taille. Sinon, erreur, et `src` reste intact (rien n'est jamais perdu ; une copie
/// partielle dans le jeu est reprise sans doublon au prochain essai).
fn move_into(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if is_empty_dir(dst) && (!dst.exists() || fs::remove_dir(dst).is_ok()) && fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    let mut copied = Vec::new();
    merge_copy(src, dst, &mut copied)?;
    for (from, to) in &copied {
        let (a, b) = (fs::metadata(from)?, fs::metadata(to)?);
        if a.len() != b.len() {
            return Err(std::io::Error::other(format!("copie incomplète : {}", to.display())));
        }
    }
    fs::remove_dir_all(src)
}

/// Copie récursive de `src` dans `dst`, sans rien écraser ; note chaque (source, copie).
fn merge_copy(src: &Path, dst: &Path, copied: &mut Vec<(PathBuf, PathBuf)>) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            if to.exists() && !to.is_dir() {
                // Un fichier du jeu porte le nom de ce dossier : le dossier va à côté.
                merge_copy(&from, &free_name(&to), copied)?;
            } else {
                merge_copy(&from, &to, copied)?;
            }
        } else if !to.exists() {
            fs::copy(&from, &to)?;
            copied.push((from, to));
        } else if !same_content(&from, &to) {
            let aside = free_name(&to);
            fs::copy(&from, &aside)?;
            tracing::info!("{} gardé à côté : {}", from.display(), aside.display());
            copied.push((from, aside));
        } else {
            copied.push((from, to)); // identique : déjà là
        }
    }
    Ok(())
}

fn same_content(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) if x.len() == y.len() => fs::read(a).ok() == fs::read(b).ok(),
        _ => false,
    }
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

        // Vrai dossier rempli alors que le jeu a déjà le sien : fusionné dans le jeu, sans écraser.
        remove_link(&outside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("c.txt"), "c").unwrap();
        fs::write(other.join("d.txt"), "d").unwrap();
        assert!(place(&outside, &other).unwrap());
        assert!(other.join("c.txt").exists() && !root.join("appdata").join("boiii.avant-farfa").exists());
        assert!(same_dir(&outside, &other));

        // Désinstallation d'un autre jeu : le lien reste ; de celui-ci : il part.
        assert!(!remove_if_inside(&outside, &fs::canonicalize(&game).unwrap()));
        assert!(is_link(&outside));
        assert!(remove_if_inside(&outside, &fs::canonicalize(root.join("autre-jeu")).unwrap()));
        assert!(!outside.exists() && other.join("d.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Désinstallation d'un jeu d'un runtime partagé : les liens restent tant qu'un autre jeu y est.
    #[test]
    fn links_stay_while_another_game_uses_the_runtime() {
        assert!(used_by_others(&[10550, 7, 8], 7));
        assert!(!used_by_others(&[7], 7));
        assert!(!used_by_others(&[], 7));
    }

    // Jamais de perte : parent de la cible absent (créé), jeu qui a déjà des fichiers (fusion :
    // copie, vérification, puis effacement de la source), conflit gardé à côté dans le jeu.
    #[test]
    fn data_is_merged_never_lost() {
        let root = std::env::temp_dir().join(format!("farfa-links-merge-{}", uuid::Uuid::new_v4()));
        // Parent de la cible absent : déplacement quand même.
        let outside = root.join("config").join("dolphin-emu");
        fs::create_dir_all(outside.join("GC")).unwrap();
        fs::write(outside.join("GC").join("carte.raw"), "sauvegarde").unwrap();
        let target = root.join("Jeux").join("Emulateurs").join("dolphin").join("config");
        assert!(place(&outside, &target).unwrap());
        assert_eq!(fs::read_to_string(target.join("GC").join("carte.raw")).unwrap(), "sauvegarde");

        // Le jeu a déjà ses fichiers : fusion par copie (même chemin que le changement de disque).
        let outside2 = root.join("appdata").join("emu");
        let target2 = root.join("Jeux").join("emu");
        fs::create_dir_all(outside2.join("saves")).unwrap();
        fs::create_dir_all(target2.join("saves")).unwrap();
        fs::write(outside2.join("saves").join("nouveau.sav"), "n").unwrap();
        fs::write(outside2.join("saves").join("meme.sav"), "m").unwrap();
        fs::write(outside2.join("saves").join("conflit.sav"), "hors du jeu").unwrap();
        fs::write(target2.join("saves").join("meme.sav"), "m").unwrap();
        fs::write(target2.join("saves").join("conflit.sav"), "dans le jeu").unwrap();
        assert!(place(&outside2, &target2).unwrap());
        let saves = target2.join("saves");
        assert_eq!(fs::read_to_string(saves.join("nouveau.sav")).unwrap(), "n");
        assert_eq!(fs::read_to_string(saves.join("conflit.sav")).unwrap(), "dans le jeu");
        assert_eq!(fs::read_to_string(saves.join("conflit.sav.avant-farfa")).unwrap(), "hors du jeu");
        assert!(!saves.join("meme.sav.avant-farfa").exists());
        assert!(is_link(&outside2) && same_dir(&outside2, &target2));

        // Copie seule (rename impossible) : la source n'est effacée qu'après vérification.
        let src = root.join("src");
        fs::create_dir_all(src.join("a")).unwrap();
        fs::write(src.join("a").join("x"), "x").unwrap();
        let dst = root.join("dst");
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("y"), "y").unwrap();
        move_into(&src, &dst).unwrap();
        assert!(!src.exists() && dst.join("a").join("x").exists() && dst.join("y").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
