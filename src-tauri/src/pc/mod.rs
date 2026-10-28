//! Jeux PC de la plateforme « Ordinateur » (dossier `ordinateur` de RomM).
//!
//! Contrairement aux portages (`RECOMP/<nom>_<Plateforme>.zip`), le dossier
//! d'un jeu PC contient le jeu tel qu'il a été récupéré :
//! - installateurs GOG (`setup_*.exe` + `*.bin`, patchs, `EXTRA/`) ;
//! - jeu prêt à lancer (dossier copié tel quel, Windows ou Linux) ;
//! - archive `.zip` du jeu ;
//! - Call of Duty : `<jeu>_game_files.zip` de CB Servers, plus un client
//!   (Plutonium, IW4x…) téléchargé par le launcher.
//!
//! Les étiquettes du nom de dossier (`[windows]`, `[linux]`, `[GOG]`)
//! départagent deux versions d'un même jeu ; le contenu du dossier prime
//! (un jeu étiqueté `[windows]` qui ne contient que des binaires Linux est
//! un jeu Linux).

pub mod cod;
pub mod dosbox;
pub mod gog;
pub mod install;
pub mod net;
pub mod proton;
pub mod steam;

use serde::{Deserialize, Serialize};

use crate::install::detect::is_annex_name;
use crate::platform::Target;
use crate::romm::library::RemoteFile;
use cod::CodGame;

/// Dossiers RomM des jeux PC (`fs_slug`).
const PC_FOLDERS: &[&str] = &["ordinateur", "pc", "win", "windows"];

pub fn is_pc_folder(fs_slug: &str) -> bool {
    PC_FOLDERS.contains(&fs_slug.to_ascii_lowercase().as_str())
}

/// Système pour lequel les fichiers du jeu sont faits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PcOs {
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PcKind {
    /// Dossier du jeu prêt à lancer.
    Portable,
    /// Archive(s) `.zip` du jeu, à extraire.
    Archive,
    /// Installateurs GOG, lancés en mode silencieux.
    GogInstaller,
    /// Fichiers d'un Call of Duty + client de CB Servers.
    Cod,
}

/// Ce qu'il faut savoir pour installer un jeu PC.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcGame {
    pub kind: PcKind,
    pub os: PcOs,
    #[serde(default)]
    pub cod: Option<CodGame>,
    /// Fichiers à télécharger (installateurs retenus, archive, ou tout le jeu).
    pub files: Vec<RemoteFile>,
    pub size: u64,
    /// Jouable sur ce système (directement ou avec Proton).
    pub runnable: bool,
    /// Jeu Windows sur Linux : lancé avec Proton.
    pub proton: bool,
}

/// Ce que le launcher retient d'un jeu PC installé.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PcInstall {
    pub kind: PcKind,
    pub os: PcOs,
    #[serde(default)]
    pub cod: Option<CodGame>,
    /// Arguments du jeu (tâche de lancement GOG).
    #[serde(default)]
    pub args: Vec<String>,
    /// Dossier de travail relatif (`None` : celui de l'exécutable).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Lancé avec Proton (jeu Windows sous Linux).
    #[serde(default)]
    pub proton: Option<proton::ProtonSetup>,
    #[serde(default)]
    pub steam_app_id: Option<u32>,
    /// Touches personnalisées (jeux DOS, voir `dosbox`).
    #[serde(default)]
    pub keys: Vec<dosbox::KeyBind>,
}

/// Étiquettes `[…]` du nom de dossier, en minuscules.
pub fn folder_tags(fs_name: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = fs_name;
    while let Some(start) = rest.find('[') {
        let Some(len) = rest[start..].find(']') else { break };
        tags.push(rest[start + 1..start + len].trim().to_ascii_lowercase());
        rest = &rest[start + len + 1..];
    }
    tags
}

/// Système indiqué par les étiquettes du dossier.
pub fn tagged_os(fs_name: &str) -> Option<PcOs> {
    let tags = folder_tags(fs_name);
    if tags.iter().any(|t| t == "linux") {
        Some(PcOs::Linux)
    } else if tags.iter().any(|t| t == "windows" || t == "win" || t == "gog") {
        Some(PcOs::Windows)
    } else {
        None
    }
}

/// Nom du jeu sans étiquettes `[…]` ni `(…)`, pour rapprocher les versions
/// Windows et Linux d'un même jeu.
pub fn base_name(fs_name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for c in fs_name.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Proton est-il utilisable sur ce système ?
pub fn proton_capable(me: Target) -> bool {
    matches!(me, Target::Linux | Target::LinuxArch | Target::SteamOS)
}

/// Ce système fait-il tourner directement des jeux de `os` ?
pub fn native(os: PcOs, me: Target) -> bool {
    match os {
        PcOs::Windows => me == Target::Windows,
        PcOs::Linux => proton_capable(me),
    }
}

fn lower_name(f: &RemoteFile) -> String {
    f.file_name.to_ascii_lowercase()
}

fn is_top_level(f: &RemoteFile) -> bool {
    !f.rel_path.contains('/')
}

/// Fichier qui ne sert pas à jouer : bonus GOG, sauvegardes d'origine…
fn is_extra(f: &RemoteFile) -> bool {
    let first = f.rel_path.split('/').next().unwrap_or("").to_ascii_lowercase();
    f.rel_path.contains('/') && matches!(first.as_str(), "extra" | "extras" | "bonus" | "goodies")
}

/// Programme Windows qui peut être le jeu (pas un désinstalleur, un crash handler…).
fn is_game_exe(f: &RemoteFile) -> bool {
    lower_name(f).strip_suffix(".exe").is_some_and(|stem| !is_annex_name(stem))
}

/// Binaire ou script de lancement Linux reconnaissable à son nom.
fn is_linux_launcher(f: &RemoteFile) -> bool {
    let n = lower_name(f);
    n.ends_with(".x86_64") || n.ends_with(".x86") || n.ends_with(".appimage") || (n.ends_with(".sh") && is_top_level(f))
}

/// Décrit un jeu PC d'après les fichiers de son dossier. `None` : dossier vide
/// (détails pas encore chargés) ou rien d'utilisable.
pub fn classify(fs_name: &str, files: &[RemoteFile], me: Target) -> Option<PcGame> {
    let files: Vec<&RemoteFile> = files.iter().filter(|f| !is_extra(f)).collect();
    if files.is_empty() {
        return None;
    }
    let (kind, os, cod, picked): (PcKind, PcOs, Option<CodGame>, Vec<RemoteFile>) =
        if let Some((cod, zip)) = files.iter().find_map(|f| CodGame::from_zip_name(&f.file_name).map(|c| (c, *f))) {
            (PcKind::Cod, PcOs::Windows, Some(cod), vec![zip.clone()])
        } else if let Some(installers) = gog::pick_installers(&files) {
            (PcKind::GogInstaller, PcOs::Windows, None, installers)
        } else if files.iter().all(|f| lower_name(f).ends_with(".zip")) {
            let os = tagged_os(fs_name).unwrap_or(PcOs::Windows);
            (PcKind::Archive, os, None, files.iter().map(|f| (*f).clone()).collect())
        } else {
            let has_exe = files.iter().any(|f| is_game_exe(f));
            let has_linux = files.iter().any(|f| is_linux_launcher(f));
            let os = match (has_exe, has_linux) {
                (true, false) => PcOs::Windows,
                (false, true) => PcOs::Linux,
                _ => tagged_os(fs_name).unwrap_or(PcOs::Windows),
            };
            (PcKind::Portable, os, None, files.iter().map(|f| (*f).clone()).collect())
        };
    let proton = os == PcOs::Windows && proton_capable(me);
    Some(PcGame {
        kind,
        os,
        cod,
        size: picked.iter().map(|f| f.size).sum(),
        files: picked,
        runnable: native(os, me) || proton,
        proton,
    })
}

/// Parmi plusieurs versions d'un même jeu (`[windows]` et `[linux]`), garde
/// la version native de ce système. `games` : (identifiant, nom de dossier) ;
/// renvoie les identifiants à masquer.
pub fn hidden_duplicates(games: &[(i64, String)], me: Target) -> Vec<i64> {
    let mut hidden = Vec::new();
    for (id, folder) in games {
        let key = base_name(folder);
        let mine = tagged_os(folder);
        let better = games.iter().any(|(other, f)| {
            other != id && base_name(f) == key && {
                let theirs = tagged_os(f);
                // L'autre est native ici et pas celle-ci.
                theirs.is_some_and(|o| native(o, me)) && !mine.is_some_and(|o| native(o, me))
            }
        });
        if better {
            hidden.push(*id);
        }
    }
    hidden
}

#[cfg(test)]
pub(crate) fn file(rel: &str, size: u64) -> RemoteFile {
    RemoteFile {
        id: 1,
        file_name: rel.rsplit('/').next().unwrap().to_string(),
        rel_path: rel.to_string(),
        size,
        sha1: None,
        md5: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_and_names() {
        assert_eq!(folder_tags("Blasphemous [windows] [GOG]"), ["windows", "gog"]);
        assert_eq!(tagged_os("Iconoclasts [linux]"), Some(PcOs::Linux));
        assert_eq!(tagged_os("Soma [windows] [GOG]"), Some(PcOs::Windows));
        assert_eq!(tagged_os("Call of duty black ops"), None);
        assert_eq!(base_name("Ender Lilies - Quietus of the Knights (igbd-138858) [linux]"), "enderliliesquietusoftheknights");
        assert_eq!(base_name("Iconoclasts [windows]"), base_name("Iconoclasts [linux]"));
    }

    #[test]
    fn portable_os_comes_from_content() {
        // Étiqueté [windows] mais ne contient que la version Linux.
        let broforce = [file("Broforce.x86_64", 10), file("Broforce_Data/level0", 5)];
        let g = classify("Broforce [windows]", &broforce, Target::Windows).unwrap();
        assert_eq!((g.kind, g.os, g.runnable), (PcKind::Portable, PcOs::Linux, false));
        assert!(classify("Broforce [windows]", &broforce, Target::SteamOS).unwrap().runnable);

        let hk = [file("hollow_knight.exe", 1), file("UnityCrashHandler64.exe", 1), file("steam_appid.txt", 1)];
        let g = classify("Hollow Knight [windows]", &hk, Target::Linux).unwrap();
        assert_eq!((g.os, g.proton, g.runnable, g.size), (PcOs::Windows, true, true, 3));
        let g = classify("Hollow Knight [windows]", &hk, Target::MacArm).unwrap();
        assert!(!g.runnable);
    }

    #[test]
    fn cod_and_archives() {
        let g = classify("Call of duty black ops 2", &[file("bo2_game_files.zip", 9)], Target::Windows).unwrap();
        assert_eq!((g.kind, g.cod), (PcKind::Cod, Some(CodGame::Bo2)));
        let g = classify("Jeu [linux]", &[file("jeu.zip", 9)], Target::Linux).unwrap();
        assert_eq!((g.kind, g.os, g.proton), (PcKind::Archive, PcOs::Linux, false));
    }

    #[test]
    fn extras_are_ignored() {
        let files = [file("setup_soma_1.61_(64bit)_(50360).exe", 5), file("EXTRA/artbook.zip", 50)];
        let g = classify("Soma [windows] [GOG]", &files, Target::Windows).unwrap();
        assert_eq!((g.kind, g.size), (PcKind::GogInstaller, 5));
    }

    #[test]
    fn native_version_wins() {
        let games = vec![(1, "Iconoclasts [linux]".to_string()), (2, "Iconoclasts [windows]".to_string()), (3, "Hades [windows]".to_string())];
        assert_eq!(hidden_duplicates(&games, Target::Linux), [2]);
        assert_eq!(hidden_duplicates(&games, Target::Windows), [1]);
        assert!(hidden_duplicates(&games, Target::MacArm).is_empty());
    }
}

