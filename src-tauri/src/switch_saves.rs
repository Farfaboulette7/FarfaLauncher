//! Sauvegardes des jeux Switch (Citron, Eden) : `nand/user/save/0000000000000000/<profil>/<ID du jeu>`.
//!
//! La règle vient de FarfaRomManager (`sauvegardes: user/nand/user/save/0000000000000000/{profil}`,
//! `motif: {titleid}`), mais deux choses ne se connaissent qu'au lancement :
//! - `{profil}` : le profil Switch de l'émulateur, propre à chaque PC ;
//! - `{titleid}` : l'ID du jeu, lu dans le nom de ses fichiers ou son ticket, sinon le dossier
//!   que l'émulateur vient de créer pendant la partie.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::install::state::{self, InstalledGame};
use crate::manager::Catalog;

const PROFILE: &str = "{profil}";
const TITLE: &str = "{titleid}";

/// La règle de cet émulateur attend-elle d'être résolue au lancement ?
pub fn is_deferred(text: &str) -> bool {
    text.contains(PROFILE) || text.contains(TITLE)
}

fn is_hex(name: &str, len: usize) -> bool {
    name.len() == len && name.chars().all(|c| c.is_ascii_hexdigit())
}

fn modified(p: &Path) -> Option<SystemTime> {
    p.metadata().and_then(|m| m.modified()).ok()
}

/// Profils de l'émulateur (dossiers de 32 chiffres hexadécimaux).
fn profiles(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|r| r.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir() && p.file_name().is_some_and(|n| is_hex(&n.to_string_lossy(), 32))).collect())
        .unwrap_or_default()
}

/// Profil à utiliser : celui qui a déjà une sauvegarde de ce jeu, sinon le plus récent.
fn pick_profile(root: &Path, title: &str) -> Option<PathBuf> {
    let all = profiles(root);
    let with_game = all.iter().filter_map(|p| Some((modified(&p.join(title))?, p.clone()))).max().map(|(_, p)| p);
    with_game.or_else(|| all.into_iter().filter_map(|p| Some((modified(&p)?, p))).max().map(|(_, p)| p))
}

/// Dossier de jeu créé ou modifié pendant la partie, dans n'importe quel profil.
fn learned_title(root: &Path, since: SystemTime) -> Option<String> {
    profiles(root)
        .iter()
        .flat_map(|p| std::fs::read_dir(p).into_iter().flatten().filter_map(Result::ok))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_uppercase();
            let when = modified(&e.path())?;
            (is_hex(&name, 16) && (name.starts_with("01") || name.starts_with("04")) && name.ends_with("000") && when >= since).then_some((when, name))
        })
        .max()
        .map(|(_, n)| n)
}

/// Dossier et motifs une fois `{profil}` et `{titleid}` connus.
pub fn resolve(rule_dir: &str, patterns: &[String], exe_dir: &Path, title: &str) -> Option<(PathBuf, Vec<String>)> {
    let (before, after) = rule_dir.split_once(PROFILE)?;
    let root = exe_dir.join(crate::install::rel_to_path(before));
    let profile = pick_profile(&root, title)?;
    let rest = crate::install::rel_to_path(&after.replace(TITLE, title));
    let dir = if rest.as_os_str().is_empty() { profile } else { profile.join(rest) };
    Some((dir, patterns.iter().map(|p| p.replace(TITLE, title)).collect()))
}

/// Jeu Switch dont l'émulateur a une règle à compléter, encore sans dossier : il se joue sans
/// rien demander, et l'ID s'apprend à la première partie où le jeu sauvegarde.
pub fn waiting(catalog: &Catalog, st: &InstalledGame) -> bool {
    let Some(emulator) = catalog.emulator_for_install(st) else { return false };
    let Some(console) = emulator.consoles.iter().find(|c| *c == "switch" || *c == "switch-2") else { return false };
    let deferred = emulator.saves.for_console(console).0.is_some_and(is_deferred);
    deferred && !st.shared.as_ref().and_then(|s| s.saves_dir.as_ref()).is_some_and(|d| d.is_dir())
}

/// Complète l'installation d'un jeu Switch avec son dossier de sauvegardes, s'il manque.
/// `since` : début de la partie qui vient de finir (l'ID peut alors s'apprendre). `true` si changé.
pub fn fill(catalog: &Catalog, config_dir: &Path, st: &mut InstalledGame, since: Option<SystemTime>) -> bool {
    let Some(emulator) = catalog.emulator_for_install(st) else { return false };
    let Some(console) = emulator.consoles.iter().find(|c| *c == "switch" || *c == "switch-2") else { return false };
    let (Some(rule), patterns) = emulator.saves.for_console(console) else { return false };
    let patterns = patterns.to_vec();
    if !is_deferred(rule) {
        return false;
    }
    let Some(exe) = st.executable.clone() else { return false };
    let exe_dir = emulator.data_dir(&st.game_dir, &exe);
    let Some(shared) = st.shared.as_ref() else { return false };
    if shared.saves_dir.as_ref().is_some_and(|d| d.is_dir()) {
        return false;
    }
    let rom_dir = shared.rom_dir.clone().unwrap_or_default();
    let roms: Vec<PathBuf> = st.roms.iter().map(|r| rom_dir.join(crate::install::rel_to_path(r))).collect();
    let root = exe_dir.join(crate::install::rel_to_path(rule.split(PROFILE).next().unwrap_or_default()));
    let title = crate::shaders::known_title(config_dir, &emulator.id, st.rom_id)
        .or_else(|| crate::shaders::title_from_names(roms.iter().filter_map(|p| p.file_name()?.to_str())))
        .or_else(|| roms.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("nsp"))).find_map(|p| crate::shaders::title_from_nsp(p)))
        .map(|t| t.to_ascii_uppercase())
        .or_else(|| since.and_then(|s| learned_title(&root, s)));
    let Some(title) = title else { return false };
    let Some((dir, pats)) = resolve(rule, &patterns, &exe_dir, &title) else { return false };
    tracing::info!("sauvegardes Switch de « {} » : {} {:?}", st.name, dir.display(), pats);
    if let Some(shared) = st.shared.as_mut() {
        shared.saves_dir = Some(dir.clone());
        shared.save_patterns = pats.clone();
    }
    st.saves.dir = Some(dir);
    st.saves.patterns = pats;
    // Coupée faute de dossier : réactivée maintenant qu'il est connu.
    st.saves.off = false;
    st.saves.searched_after_play = false;
    if let Err(e) = state::write_state(config_dir, st) {
        tracing::error!("dossier des sauvegardes Switch non mémorisé: {}", e.code());
    }
    true
}

/// Tous les jeux Switch et Switch 2 installés sous émulateur : dossier complété s'il manque.
/// Renvoie le nombre de jeux complétés.
pub fn fill_all(catalog: &Catalog, config_dir: &Path) -> usize {
    let mut found = 0;
    for mut st in state::list_installed(config_dir) {
        if fill(catalog, config_dir, &mut st, None) {
            found += 1;
        } else if waiting(catalog, &st) {
            tracing::info!("sauvegardes Switch de « {} » : dossier pas encore créé par l'émulateur, appris à la prochaine partie", st.name);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_and_title_are_found() {
        let exe = std::env::temp_dir().join(format!("farfa-swsave-{}", uuid::Uuid::new_v4()));
        let root = exe.join("user/nand/user/save/0000000000000000");
        let rule = "user/nand/user/save/0000000000000000/{profil}";
        let pats = vec!["{titleid}".to_string()];
        // Aucun profil encore : rien.
        assert!(resolve(rule, &pats, &exe, "0100AE00096EA000").is_none());
        std::fs::create_dir_all(root.join("8B78E244D05F295F3A00000000000000/01006A800016E000")).unwrap();
        std::fs::create_dir_all(root.join("E950DDCEAF68288B78E244D05F295F3A/0100AE00096EA000")).unwrap();
        std::fs::create_dir_all(root.join("pas-un-profil")).unwrap();
        // Le profil qui a déjà ce jeu.
        let (dir, p) = resolve(rule, &pats, &exe, "0100AE00096EA000").unwrap();
        assert_eq!(dir, root.join("E950DDCEAF68288B78E244D05F295F3A"));
        assert_eq!(p, ["0100AE00096EA000"]);
        assert_eq!(resolve(rule, &pats, &exe, "01006A800016E000").unwrap().0, root.join("8B78E244D05F295F3A00000000000000"));
        // ID appris : le dossier de jeu modifié pendant la partie.
        let since = SystemTime::now() - std::time::Duration::from_secs(5);
        assert_eq!(learned_title(&root, since).as_deref().map(|t| t.len()), Some(16));
        assert_eq!(learned_title(&root, SystemTime::now() + std::time::Duration::from_secs(60)), None);
        assert!(is_deferred(rule) && !is_deferred("memcards"));
        // Switch 2 : ID en 04….
        std::fs::create_dir_all(root.join("E950DDCEAF68288B78E244D05F295F3A/0400245021D2C000")).unwrap();
        assert_eq!(learned_title(&root, since).as_deref(), Some("0400245021D2C000"));
        std::fs::remove_dir_all(&exe).unwrap();
    }
}
