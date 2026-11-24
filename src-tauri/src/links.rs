//! Dossiers hors du jeu reliés à un dossier du jeu (`lien:` des fiches de recomp et, depuis
//! l'écart E20, d'émulateur).
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
        if crate::language::is_sensitive(&link.path) {
            tracing::warn!("lien refusé : dossier « {} » protégé", link.path);
            continue;
        }
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

/// Repère posé dans le dossier du jeu quand on y a déplacé un vrai dossier qui existait avant
/// le lien (ex. configuration d'un Dolphin installé à part) : à la désinstallation, son
/// contenu retourne à son chemin d'origine au lieu de partir avec le jeu.
const MIGRATED: &str = ".farfa-migre";

/// Retire les liens qui mènent dans ce dossier de jeu (désinstallation) : ils ne pointeraient
/// plus sur rien. Un dossier qui existait avant le lien (repère `.farfa-migre`) est remis à
/// son chemin d'origine, par la même copie sûre. Erreur : la remise a échoué, le lien est
/// reposé et la désinstallation doit s'arrêter (sinon ces fichiers partiraient avec le jeu).
pub fn remove_into(links: &[Link], game_dir: &Path) -> Result<(), crate::error::AppError> {
    let Ok(game) = fs::canonicalize(game_dir) else { return Ok(()) };
    for link in links.iter().filter(|l| l.system.as_deref().is_none_or(|s| s == crate::language::current_system())) {
        if let Some(path) = crate::language::expand(&link.path, game_dir) {
            remove_if_inside(&path, &game)?;
        }
    }
    Ok(())
}

/// Le dossier de ce jeu sert-il encore à un autre jeu (runtime partagé : launcher Pokemon,
/// émulateur) ? `pages` : jeux installés sur ce dossier (`state::pages_in`, réserve comprise).
/// Ses liens doivent alors rester à la désinstallation de celui-ci.
pub fn used_by_others(pages: &[i64], rom_id: i64) -> bool {
    pages.iter().any(|id| *id != rom_id)
}

/// Un seul lien (jeu PC : `%LOCALAPPDATA%\boiii`), avec le même traitement que `remove_into`.
pub fn remove_link_into(path: &Path, game_dir: &Path) -> Result<bool, crate::error::AppError> {
    match fs::canonicalize(game_dir) {
        Ok(game) => remove_if_inside(path, &game),
        Err(_) => Ok(false),
    }
}

/// Erreur de remise en place : disque plein, sinon l'erreur dédiée (avec le chemin).
fn not_restored(path: &Path, e: &std::io::Error) -> crate::error::AppError {
    tracing::error!("{} non remis à son chemin d'origine : {e}", path.display());
    let full = matches!(e.raw_os_error(), Some(112) | Some(28)) || e.kind() == std::io::ErrorKind::StorageFull;
    if full {
        crate::error::AppError::DiskFull
    } else {
        crate::error::AppError::FolderNotRestored { path: path.display().to_string() }
    }
}

fn remove_if_inside(path: &Path, game: &Path) -> Result<bool, crate::error::AppError> {
    let target = match fs::canonicalize(path) {
        Ok(t) if is_link(path) && t.starts_with(game) => t,
        _ => return Ok(false),
    };
    let migrated = target.join(MIGRATED).is_file();
    if let Err(e) = remove_link(path) {
        // Lien impossible à retirer : sans importance s'il ne mène qu'à des fichiers du jeu ;
        // mais un dossier d'avant (repère) partirait avec le jeu : on arrête.
        if migrated {
            return Err(not_restored(path, &e));
        }
        tracing::warn!("lien {} non retiré: {e}", path.display());
        return Ok(false);
    }
    tracing::info!("lien {} retiré", path.display());
    if migrated {
        if let Err(e) = move_into(&target, path) {
            let _ = make_link(&target, path);
            return Err(not_restored(path, &e));
        }
        let _ = fs::remove_file(path.join(MIGRATED));
        tracing::info!("{} remis à son chemin d'origine", path.display());
    }
    Ok(true)
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
    }
    let mut moved = false;
    if !is_link(path) && path.is_dir() {
        // Vrai dossier (le programme a déjà tourné hors du launcher) : ses fichiers vont dans le
        // jeu, sans jamais rien perdre (voir `move_into`). Un lien d'essai est fait d'abord :
        // si cet endroit n'accepte pas de lien, le vrai dossier n'est pas touché. Si le déplacement
        // échoue, rien n'est effacé et le lien n'est pas posé : le programme garde son dossier.
        if is_empty_dir(path) {
            fs::remove_dir(path)?;
        } else {
            try_link(path, target)?;
            move_into(path, target)?;
            moved = true;
            // Dossier d'avant le lien : rendu à son chemin d'origine à la désinstallation.
            if let Err(e) = fs::write(target.join(MIGRATED), path.to_string_lossy().as_bytes()) {
                match move_into(target, path) {
                    Ok(()) => tracing::warn!("repère non posé : {} remis à sa place", path.display()),
                    Err(back) => tracing::error!("repère non posé, et {} non remis à sa place ({back})", path.display()),
                }
                return Err(e);
            }
            tracing::info!("{} déplacé dans {}", path.display(), target.display());
        }
    } else if !is_link(path) && path.exists() {
        return Err(std::io::Error::other("un fichier porte déjà ce nom"));
    }
    fs::create_dir_all(target)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Err(e) = make_link(target, path) {
        // Lien impossible malgré l'essai : le dossier est remis à sa place (avec ce que le jeu
        // avait déjà), pour que le programme ne reparte pas d'un dossier vide.
        if moved {
            let _ = fs::remove_file(target.join(MIGRATED));
            match move_into(target, path) {
                Ok(()) => tracing::warn!("lien impossible : {} remis à sa place", path.display()),
                Err(back) => tracing::error!("lien impossible, et {} non remis à sa place ({back}) : fichiers dans {}", path.display(), target.display()),
            }
        }
        return Err(e);
    }
    Ok(true)
}

/// Lien d'essai à côté des vrais (`<nom>.farfa-essai`), aussitôt retiré : dit si cet endroit
/// accepte un lien vers ce disque, avant de déplacer quoi que ce soit.
fn try_link(path: &Path, target: &Path) -> std::io::Result<()> {
    let essai = |p: &Path| {
        let mut s = p.as_os_str().to_owned();
        s.push(".farfa-essai");
        PathBuf::from(s)
    };
    let (link, dir) = (essai(path), essai(target));
    if is_link(&link) {
        remove_link(&link)?;
    }
    fs::create_dir_all(&dir)?;
    let result = make_link(&dir, &link);
    if result.is_ok() {
        let _ = remove_link(&link);
    }
    let _ = fs::remove_dir(&dir);
    result
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

/// Suffixe d'un fichier en cours de copie : jamais pris pour un vrai fichier.
const PART: &str = ".farfa-part";

/// Copie un fichier sans laisser de fichier partiel sous son vrai nom : copie vers
/// `<to>.farfa-part`, puis renommage. Une copie interrompue (coupure, disque plein) ne laisse
/// qu'un `.farfa-part`, effacé au prochain essai : jamais une sauvegarde tronquée active.
fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut part = to.as_os_str().to_owned();
    part.push(PART);
    let part = PathBuf::from(part);
    fs::copy(from, &part)?;
    fs::rename(&part, to).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })
}

/// Copie récursive de `src` dans `dst`, sans rien écraser ; note chaque (source, copie).
fn merge_copy(src: &Path, dst: &Path, copied: &mut Vec<(PathBuf, PathBuf)>) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    // Restes d'une copie interrompue : effacés (la source, elle, est intacte).
    for entry in fs::read_dir(dst)?.filter_map(Result::ok) {
        if entry.file_name().to_string_lossy().ends_with(PART) && entry.path().is_file() {
            let _ = fs::remove_file(entry.path());
        }
    }
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
            copy_file(&from, &to)?;
            copied.push((from, to));
        } else if !same_content(&from, &to) {
            let aside = free_name(&to);
            copy_file(&from, &aside)?;
            tracing::info!("{} gardé à côté : {}", from.display(), aside.display());
            copied.push((from, aside));
        } else {
            copied.push((from, to)); // identique : déjà là
        }
    }
    Ok(())
}

/// Même contenu ? Comparé par blocs de 64 Kio, sans charger les deux fichiers en mémoire
/// (une sauvegarde ou une carte mémoire peut peser des centaines de Mo).
fn same_content(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) if x.len() == y.len() => {}
        _ => return false,
    }
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let Ok(n) = read_full(&mut fa, &mut ba) else { return false };
        let Ok(m) = read_full(&mut fb, &mut bb) else { return false };
        if n != m || ba[..n] != bb[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }

    /// Remplit le tampon autant que possible (un `read` peut rendre moins que demandé).
    fn read_full(f: &mut fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut total = 0;
        while total < buf.len() {
            match f.read(&mut buf[total..])? {
                0 => break,
                n => total += n,
            }
        }
        Ok(total)
    }
}

fn remove_link(path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::FAIL_UNLINK.with(|f| f.replace(false)) {
        return Err(std::io::Error::other("lien verrouillé (test)"));
    }
    // Jonction ou lien vers un dossier : `remove_dir` ; lien symbolique Unix : `remove_file`.
    fs::remove_dir(path).or_else(|_| fs::remove_file(path))
}

/// Crée le lien `link` vers `target`. En test, `tests::FAIL_LINKS` peut faire échouer les
/// prochains appels (un `true` par appel à faire échouer), pour vérifier qu'aucun cas ne perd
/// de données.
fn make_link(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if tests::FAIL_LINKS.with(|plan| plan.borrow_mut().pop_front()).unwrap_or(false) {
        return Err(std::io::Error::other("lien refusé (test)"));
    }
    make_link_os(target, link)
}

#[cfg(windows)]
fn make_link_os(target: &Path, link: &Path) -> std::io::Result<()> {
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
fn make_link_os(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Appels à `make_link` à faire échouer, dans l'ordre (`true` : échec).
        pub(super) static FAIL_LINKS: std::cell::RefCell<std::collections::VecDeque<bool>> = Default::default();
        /// Le prochain `remove_link` échoue (lien verrouillé).
        pub(super) static FAIL_UNLINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    // Faille SL4 : un `lien:` ne remplace jamais un dossier protégé, quelle que soit la casse.
    #[test]
    fn sl4_protected_folders_are_never_linked() {
        let refused = [
            "{appdata}/Microsoft/Windows/Start Menu/Programs/Startup",
            "{appdata}/microsoft/windows/start menu/programs/startup",
            "{appdata}/MICROSOFT/Windows",
            "{appdata}/MICROS~1/Windows",
            "{appdata}/Microsoft./Windows",
            "{appdata}/x/Start Menu",
            "{appdata}/x/STARTUP",
            "{documents}/WindowsPowerShell",
            "{documents}/PowerShell",
            "{config}/autostart",
            "{config}/AutoStart",
            "{config}/systemd/user",
            "{appdata}/systemd/user",
            "{config}/environment.d",
            "{config}/fish/conf.d",
            "{appdata}/x/.ssh",
            "{appdata}/x/.SSH",
            "{appdata}/x/.gnupg",
            "{appdata}/x/.bashrc",
            "{appdata}/x/.profile",
            "{appdata}/x/.zshrc",
        ];
        for path in refused {
            assert!(crate::language::is_sensitive(path), "{path} doit être refusé");
        }
        // Les `lien:` des fiches réelles (24/11/2026) passent tous.
        let real = [
            "{appdata}/bbport-launcher",
            "{appdata}/TwilitRealm/Dusklight",
            "{documents}/MarioWiiSaveData",
            "{appdata}/WiiCompiled",
            "{appdata}/ttyd",
            "{appdata}/org.pkp-recomp.launcher",
            "{appdata}/sm64coopdx",
            "{appdata}/SMB1R",
            "{config}/SMS Launcher",
            "{appdata}/melee-pc",
            "{config}/dolphin-emu",
            "{localappdata}/boiii",
        ];
        for path in real {
            assert!(!crate::language::is_sensitive(path), "{path} doit passer");
        }
    }

    // Un lien refusé ne touche à rien : le dossier protégé reste un vrai dossier.
    #[test]
    fn sl4_refused_link_is_not_placed() {
        let root = std::env::temp_dir().join(format!("farfa-links-sl4-{}", uuid::Uuid::new_v4()));
        let startup = root.join("jeu").join("Startup");
        fs::create_dir_all(&startup).unwrap();
        fs::write(startup.join("a.txt"), "x").unwrap();
        let link = Link { system: None, path: "{exedir}/Startup".into(), target: "saves".into() };
        apply(&[link], &root.join("jeu"));
        assert!(!is_link(&startup));
        assert!(startup.join("a.txt").is_file());
        assert!(!root.join("jeu").join("saves").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    // Relecture 3 : lien impossible à retirer alors que le dossier d'avant est derrière (repère)
    // → erreur dédiée, avec le chemin ; sans repère, simple avertissement. Disque plein → DiskFull.
    #[test]
    fn locked_link_over_a_migrated_folder_stops_the_uninstall() {
        let root = std::env::temp_dir().join(format!("farfa-links-lock-{}", uuid::Uuid::new_v4()));
        let (before, game) = (root.join("appdata").join("boiii"), root.join("jeu"));
        fs::create_dir_all(&before).unwrap();
        fs::write(before.join("profil.json"), "p").unwrap();
        assert!(place(&before, &game.join("boiii")).unwrap());
        FAIL_UNLINK.with(|f| f.set(true));
        let err = remove_link_into(&before, &game).unwrap_err();
        assert_eq!(err.code(), "folder_not_restored");
        assert!(err.to_string().contains(&before.display().to_string()) && err.to_string().contains("rien n'a été effacé"));
        assert!(is_link(&before) && game.join("boiii").join("profil.json").exists());
        // Ensuite : remis à sa place, lien retiré.
        assert!(remove_link_into(&before, &game).unwrap());
        assert!(!is_link(&before) && before.join("profil.json").exists());
        assert_eq!(not_restored(&before, &std::io::Error::from_raw_os_error(if cfg!(windows) { 112 } else { 28 })).code(), "disk_full");
        fs::remove_dir_all(root).unwrap();
    }

    // Relecture 2 (B) : dossier créé par le launcher (pas de repère) : seul le lien part ;
    // dossier d'avant (repère) : il retourne à son chemin d'origine, sans perte.
    #[test]
    fn uninstall_gives_back_a_folder_that_existed_before() {
        let root = std::env::temp_dir().join(format!("farfa-links-back-{}", uuid::Uuid::new_v4()));
        let game = root.join("Emulateurs").join("dolphin");
        let (fresh, before) = (root.join("config").join("neuf"), root.join("config").join("dolphin-emu"));
        assert!(place(&fresh, &game.join("neuf")).unwrap());
        fs::create_dir_all(&before).unwrap();
        fs::write(before.join("Dolphin.ini"), "à moi").unwrap();
        assert!(place(&before, &game.join("config")).unwrap());
        assert!(game.join("config").join(MIGRATED).is_file());
        fs::write(before.join("nouveau.sav"), "joué avec le launcher").unwrap();
        let links = [
            Link { system: None, path: fresh.to_string_lossy().replace('\\', "/"), target: "neuf".into() },
            Link { system: None, path: before.to_string_lossy().replace('\\', "/"), target: "config".into() },
        ];
        let game_c = fs::canonicalize(&game).unwrap();
        for l in &links {
            assert!(remove_if_inside(Path::new(&l.path), &game_c).unwrap());
        }
        assert!(!fresh.exists());
        assert!(!is_link(&before));
        assert_eq!(fs::read_to_string(before.join("Dolphin.ini")).unwrap(), "à moi");
        assert_eq!(fs::read_to_string(before.join("nouveau.sav")).unwrap(), "joué avec le launcher");
        assert!(!before.join(MIGRATED).exists() && !game.join("config").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Relecture 2 (C) : lien d'essai refusé → le vrai dossier n'est pas touché ; lien final
    // refusé après le déplacement → le dossier est remis à sa place ; aucun essai ne traîne.
    #[test]
    fn failed_link_never_leaves_an_empty_folder() {
        let root = std::env::temp_dir().join(format!("farfa-links-fail-{}", uuid::Uuid::new_v4()));
        let (outside, target) = (root.join("config").join("emu"), root.join("jeu").join("config"));
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("reglages.ini"), "x").unwrap();
        FAIL_LINKS.with(|p| *p.borrow_mut() = [true].into());
        assert!(place(&outside, &target).is_err());
        assert_eq!(fs::read_to_string(outside.join("reglages.ini")).unwrap(), "x");
        assert!(!is_link(&outside));
        FAIL_LINKS.with(|p| *p.borrow_mut() = [false, true].into());
        assert!(place(&outside, &target).is_err());
        assert_eq!(fs::read_to_string(outside.join("reglages.ini")).unwrap(), "x");
        assert!(!is_link(&outside) && !target.exists());
        FAIL_LINKS.with(|p| p.borrow_mut().clear());
        assert!(place(&outside, &target).unwrap());
        assert_eq!(fs::read_to_string(target.join("reglages.ini")).unwrap(), "x");
        let restes: Vec<_> = fs::read_dir(root.join("config")).unwrap().chain(fs::read_dir(root.join("jeu")).unwrap())
            .filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().contains("farfa-essai")).collect();
        assert!(restes.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

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

        // Désinstallation d'un autre jeu : le lien reste ; de celui-ci : il part, et le dossier
        // qui existait avant (repère .farfa-migre) retourne à son chemin d'origine, au complet.
        assert!(!remove_if_inside(&outside, &fs::canonicalize(&game).unwrap()).unwrap());
        assert!(is_link(&outside));
        assert!(remove_if_inside(&outside, &fs::canonicalize(root.join("autre-jeu")).unwrap()).unwrap());
        assert!(!is_link(&outside) && outside.join("c.txt").exists() && outside.join("d.txt").exists());
        assert!(!outside.join(MIGRATED).exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Écart E20 : écrire au chemin d'origine (manettes, langue, sauvegardes en `{config}/…`,
    // comme controllers.rs) passe par le lien et arrive dans le dossier de l'émulateur.
    #[test]
    fn writing_at_the_original_path_lands_in_the_emulator() {
        let root = std::env::temp_dir().join(format!("farfa-links-write-{}", uuid::Uuid::new_v4()));
        let config = root.join("config").join("dolphin-emu");
        let target = root.join("Emulateurs").join("dolphin").join("config").join("dolphin-emu");
        assert!(place(&config, &target).unwrap());
        let file = config.join("Config").join("GCPadNew.ini");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "[GCPad1]").unwrap();
        assert_eq!(fs::read_to_string(target.join("Config").join("GCPadNew.ini")).unwrap(), "[GCPad1]");
        fs::remove_dir_all(root).unwrap();
    }

    // Copie interrompue : le reste `.farfa-part` est effacé au prochain essai et ne prend jamais
    // la place du vrai fichier ; une copie réussie n'en laisse aucun.
    #[test]
    fn interrupted_copy_never_becomes_the_save() {
        let root = std::env::temp_dir().join(format!("farfa-links-part-{}", uuid::Uuid::new_v4()));
        let (src, dst) = (root.join("src"), root.join("dst"));
        fs::create_dir_all(src.join("saves")).unwrap();
        fs::write(src.join("saves").join("jeu.sav"), "complet").unwrap();
        fs::create_dir_all(dst.join("saves")).unwrap();
        fs::write(dst.join("autre"), "x").unwrap(); // jeu déjà rempli : chemin de la copie
        // Reste d'un essai coupé en pleine copie.
        fs::write(dst.join("saves").join("jeu.sav.farfa-part"), "comp").unwrap();
        move_into(&src, &dst).unwrap();
        assert_eq!(fs::read_to_string(dst.join("saves").join("jeu.sav")).unwrap(), "complet");
        assert!(!dst.join("saves").join("jeu.sav.farfa-part").exists());
        assert!(!dst.join("saves").join("jeu.sav.avant-farfa").exists());
        fs::remove_dir_all(root).unwrap();
    }

    // Comparaison par blocs : identique, différence dans le dernier bloc, tailles différentes.
    #[test]
    fn same_content_by_blocks() {
        let root = std::env::temp_dir().join(format!("farfa-links-same-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let mut other = big.clone();
        *other.last_mut().unwrap() ^= 1;
        fs::write(root.join("a"), &big).unwrap();
        fs::write(root.join("b"), &big).unwrap();
        fs::write(root.join("c"), &other).unwrap();
        fs::write(root.join("d"), &big[..1000]).unwrap();
        assert!(same_content(&root.join("a"), &root.join("b")));
        assert!(!same_content(&root.join("a"), &root.join("c")));
        assert!(!same_content(&root.join("a"), &root.join("d")));
        assert!(!same_content(&root.join("a"), &root.join("absent")));
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
