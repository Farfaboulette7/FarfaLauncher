//! Installateurs GOG hors ligne (Inno Setup).
//!
//! Un dossier GOG contient l'installateur du jeu (`setup_<jeu>_<version>_(<build>).exe`
//! et ses parties `…-1.bin`, `…-2.bin`), ceux des DLC, des patchs entre
//! versions et parfois plusieurs éditions. Les installateurs sont déjà à la
//! dernière version : les patchs sont ignorés, et seule l'édition au numéro
//! de build le plus récent est gardée (jeu + DLC partagent ce numéro).
//!
//! Une fois installé, le jeu décrit lui-même comment le lancer dans
//! `goggame-<id>.info` (`playTasks`), comme le lit GOG Galaxy.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::romm::library::RemoteFile;

/// Arguments Inno Setup : installation sans questions dans `dir`, sans
/// raccourcis ni fenêtre (la progression est suivie par le launcher).
pub fn installer_args(dir: &str) -> Vec<String> {
    ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-", "/NOICONS", "/LANG=french"]
        .into_iter()
        .map(String::from)
        .chain(std::iter::once(format!("/DIR={dir}")))
        .collect()
}

/// Arguments du premier essai sous Windows, sans droits administrateur (faille SL9) :
/// ceux d'`installer_args` plus `/CURRENTUSER`, qui demande à Inno Setup 6 d'installer pour
/// ce compte seulement (pas d'écriture dans Program Files ni dans HKLM). Inno Setup 5 ignore
/// ce paramètre inconnu.
#[cfg_attr(not(windows), allow(dead_code))] // seulement sous Windows (Proton sinon)
pub fn installer_args_current_user(dir: &str) -> Vec<String> {
    let mut args = installer_args(dir);
    args.insert(0, "/CURRENTUSER".to_string());
    args
}

/// Codes de sortie d'Inno Setup qui disent que l'installation n'a pas eu lieu (1 : démarrage
/// impossible, 2 et 5 : annulée, 3 et 4 : erreur grave, 6 : tué, 7 : préparation refusée,
/// 8 : redémarrage nécessaire avant de continuer).
#[cfg_attr(not(windows), allow(dead_code))]
const INNO_FAILED: std::ops::RangeInclusive<u32> = 1..=8;

/// Après le premier essai sans droits administrateur : le jeu est-il bien installé ?
/// - code 0 : oui ;
/// - code d'échec d'Inno (1 à 8) : non, il faut réessayer en administrateur ;
/// - autre code (plantage à la fermeture, 0xC000041D chez certains installateurs GOG) : oui
///   seulement si le jeu décrit son lancement (`goggame-*.info`), comme avant.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn user_install_succeeded(code: u32, game_described: bool) -> bool {
    code == 0 || (!INNO_FAILED.contains(&code) && game_described)
}

/// Numéro de build GOG : le dernier `(nombre)` du nom.
fn build_id(stem: &str) -> Option<u64> {
    let end = stem.rfind(')')?;
    let start = stem[..end].rfind('(')?;
    stem[start + 1..end].parse().ok()
}

/// Installateurs à télécharger (jeu d'abord, puis DLC), avec leurs parties `.bin`.
/// `None` : pas un dossier d'installateurs GOG.
pub fn pick_installers(files: &[&RemoteFile]) -> Option<Vec<RemoteFile>> {
    let setups: Vec<&RemoteFile> = files
        .iter()
        .copied()
        .filter(|f| {
            let n = f.file_name.to_ascii_lowercase();
            !f.rel_path.contains('/') && n.starts_with("setup_") && n.ends_with(".exe")
        })
        .collect();
    if setups.is_empty() {
        return None;
    }
    let stem = |f: &RemoteFile| f.file_name[..f.file_name.len() - 4].to_string();
    let newest = setups.iter().filter_map(|f| build_id(&stem(f))).max();
    let parts = |s: &str| -> Vec<RemoteFile> {
        let prefix = format!("{}-", s.to_ascii_lowercase());
        files
            .iter()
            .filter(|f| {
                let n = f.file_name.to_ascii_lowercase();
                !f.rel_path.contains('/') && n.ends_with(".bin") && n.starts_with(&prefix)
            })
            .map(|f| (*f).clone())
            .collect()
    };
    let mut groups: Vec<(u64, Vec<RemoteFile>)> = setups
        .into_iter()
        .filter(|f| newest.is_none() || build_id(&stem(f)) == newest || build_id(&stem(f)).is_none())
        .map(|f| {
            let mut group = vec![f.clone()];
            group.extend(parts(&stem(f)));
            (group.iter().map(|g| g.size).sum(), group)
        })
        .collect();
    // Le jeu (le plus gros) avant ses DLC.
    groups.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    Some(groups.into_iter().flat_map(|(_, g)| g).collect())
}

/// Installateurs (`.exe`) parmi les fichiers retenus, dans l'ordre d'exécution.
pub fn setup_files(files: &[RemoteFile]) -> Vec<&RemoteFile> {
    files.iter().filter(|f| f.file_name.to_ascii_lowercase().ends_with(".exe")).collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GogInfo {
    #[serde(default)]
    play_tasks: Vec<RawTask>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawTask {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    path: String,
    #[serde(default)]
    arguments: String,
    #[serde(default)]
    working_dir: String,
}

/// Lancement du jeu décrit par GOG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayTask {
    /// Programme, relatif au dossier du jeu (séparateurs `/`).
    pub exe: String,
    pub args: Vec<String>,
    /// Dossier de travail relatif (`None` : celui du programme).
    pub cwd: Option<String>,
}

fn to_rel(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string()
}

/// Tâche de jeu principale des fichiers `goggame-*.info` du dossier.
pub fn play_task(game_dir: &Path) -> Option<PlayTask> {
    let mut infos: Vec<PathBuf> = std::fs::read_dir(game_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            n.starts_with("goggame-") && n.ends_with(".info")
        })
        .collect();
    infos.sort();
    let tasks: Vec<RawTask> = infos
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|t| serde_json::from_str::<GogInfo>(t.trim_start_matches('\u{feff}')).ok())
        .flat_map(|i| i.play_tasks)
        .filter(|t| t.kind == "FileTask" && !t.path.is_empty())
        .collect();
    // La tâche principale, comme GOG Galaxy : parfois le menu d'une compilation
    // (Devil May Cry HD Collection : dmcLauncher.exe → DMC 1, 2 ou 3).
    let game = |t: &&RawTask| t.category.is_empty() || t.category == "game";
    let playable = |t: &&RawTask| t.category != "document" && t.category != "tool";
    let task = tasks.iter().filter(playable).find(|t| t.is_primary).or_else(|| tasks.iter().find(game))?;
    // Casse des chemins de GOG pas toujours celle du disque (DOSBOX/dosbox.exe
    // pour DOSBOX/DOSBox.exe) : sans importance sous Windows, pas sous Linux.
    let Some(exe) = resolve_ci(game_dir, &to_rel(&task.path)).filter(|e| game_dir.join(e).is_file()) else {
        tracing::warn!("goggame: programme introuvable: {}", task.path);
        return None;
    };
    let cwd = Some(to_rel(&task.working_dir)).filter(|d| !d.is_empty()).map(|d| resolve_ci(game_dir, &d).unwrap_or(d));
    Some(PlayTask { exe, args: split_args(&task.arguments), cwd })
}

/// Chemin relatif retrouvé sur le disque sans tenir compte de la casse.
fn resolve_ci(root: &Path, rel: &str) -> Option<String> {
    let mut dir = root.to_path_buf();
    let mut parts = Vec::new();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        // Nom réel sur le disque : identique si possible, sinon à la casse près.
        let names: Vec<String> = std::fs::read_dir(&dir).ok()?.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        let name = names.iter().find(|n| *n == part).or_else(|| names.iter().find(|n| n.eq_ignore_ascii_case(part)))?.clone();
        dir.push(&name);
        parts.push(name);
    }
    Some(parts.join("/"))
}

/// Découpe une ligne d'arguments Windows (guillemets doubles).
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Lance un installateur Windows et attend sa fin ; rend son code de sortie.
/// - `elevate = false` : lancé comme un double-clic, avec les droits du joueur. Si le
///   programme exige l'administrateur (son manifeste), Windows affiche quand même l'UAC.
/// - `elevate = true` : verbe `runas`, droits administrateur demandés (fenêtre UAC). Utilisé
///   seulement en repli, quand le premier essai a échoué (faille SL9).
#[cfg(windows)]
pub fn run_installer(exe: &Path, args: &[String], elevate: bool) -> std::io::Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let params: String = args
        .iter()
        .map(|a| match a.split_once('=') {
            // /DIR="C:\Mes jeux\X" : guillemets autour de la valeur, comme Inno l'attend.
            Some((k, v)) if v.contains(' ') => format!("{k}=\"{v}\""),
            _ if a.contains(' ') => format!("\"{a}\""),
            _ => a.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let file = wide(exe.as_os_str());
    let params = wide(std::ffi::OsStr::new(&params));
    let dir = wide(exe.parent().unwrap_or(Path::new(".")).as_os_str());
    // « open » : comme un double-clic ; « runas » : en administrateur.
    let verb = wide(std::ffi::OsStr::new(if elevate { "runas" } else { "open" }));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.lpDirectory = dir.as_ptr();
    info.nShow = 1; // SW_SHOWNORMAL
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if info.hProcess.is_null() {
        return Ok(0);
    }
    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc::file;

    fn names(v: &[RemoteFile]) -> Vec<&str> {
        v.iter().map(|f| f.file_name.as_str()).collect()
    }

    #[test]
    fn keeps_newest_edition_and_dlcs_without_patches() {
        let files = [
            file("setup_metro_exodus_1.0.8.39_(64bit)_(62346).exe", 1),
            file("setup_metro_exodus_1.0.8.39_(64bit)_(62346)-1.bin", 50),
            file("setup_metro_exodus_3.0.8.39_(64bit)_(62413).exe", 1),
            file("setup_metro_exodus_3.0.8.39_(64bit)_(62413)-1.bin", 60),
            file("setup_metro_exodus_3.0.8.39_(64bit)_(62413)-2.bin", 60),
            file("setup_metro_exodus_-_the_two_colonels_3.0.8.39_(64bit)_(62413).exe", 3),
            file("setup_metro_exodus_-_the_two_colonels_1.0.8.39_(64bit)_(62346).exe", 3),
            file("patch_metro_exodus_1_to_3_(62413).exe", 9),
        ];
        let refs: Vec<&RemoteFile> = files.iter().collect();
        let picked = pick_installers(&refs).unwrap();
        assert_eq!(
            names(&picked),
            [
                "setup_metro_exodus_3.0.8.39_(64bit)_(62413).exe",
                "setup_metro_exodus_3.0.8.39_(64bit)_(62413)-1.bin",
                "setup_metro_exodus_3.0.8.39_(64bit)_(62413)-2.bin",
                "setup_metro_exodus_-_the_two_colonels_3.0.8.39_(64bit)_(62413).exe",
            ]
        );
        assert_eq!(setup_files(&picked).len(), 2);
    }

    #[test]
    fn base_game_runs_before_dlc() {
        let files = [
            file("setup_blasphemous_2_-_mea_culpa_3.0.0-cs73341_(64bit)_(91763).exe", 5),
            file("setup_blasphemous_2_3.0.0-cs73341_(64bit)_(91763).exe", 2),
            file("setup_blasphemous_2_3.0.0-cs73341_(64bit)_(91763)-1.bin", 90),
        ];
        let refs: Vec<&RemoteFile> = files.iter().collect();
        let picked = pick_installers(&refs).unwrap();
        assert_eq!(picked[0].file_name, "setup_blasphemous_2_3.0.0-cs73341_(64bit)_(91763).exe");
        assert!(pick_installers(&[&file("Tunic.exe", 1)]).is_none());
    }

    #[test]
    fn reads_play_task() {
        let dir = std::env::temp_dir().join(format!("farfa-gog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("bin/x64")).unwrap();
        std::fs::write(dir.join("bin/x64/Game.exe"), b"").unwrap();
        assert_eq!(resolve_ci(&dir, "BIN/X64/game.EXE").as_deref(), Some("bin/x64/Game.exe"));
        std::fs::write(
            dir.join("goggame-1423049311.info"),
            r#"{"gameId":"1","playTasks":[{"category":"launcher","isPrimary":false,"path":"x.exe","type":"FileTask"},{"category":"document","isPrimary":true,"path":"m.pdf","type":"FileTask"},
               {"category":"game","isPrimary":true,"path":"bin\\x64\\Game.exe","arguments":"-skipStartScreen \"a b\"","type":"FileTask"},
               {"category":"document","path":"manual.pdf","type":"FileTask"}]}"#,
        )
        .unwrap();
        let t = play_task(&dir).unwrap();
        assert_eq!(t, PlayTask { exe: "bin/x64/Game.exe".into(), args: vec!["-skipStartScreen".into(), "a b".into()], cwd: None });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn args() {
        assert_eq!(split_args(r#"-conf "..\dosbox.conf" -c "mount C ..""#), ["-conf", r"..\dosbox.conf", "-c", "mount C .."]);
        assert!(split_args("  ").is_empty());
        assert_eq!(installer_args(r"C:\Jeux\X").last().unwrap(), r"/DIR=C:\Jeux\X");
    }

    #[test]
    fn first_try_without_admin() {
        let args = installer_args_current_user(r"C:\Jeux\X");
        assert_eq!(args[0], "/CURRENTUSER");
        assert_eq!(args.last().unwrap(), r"/DIR=C:\Jeux\X");
        // Le chemin Proton (Linux) garde les arguments d'avant.
        assert!(!installer_args("C:/x").iter().any(|a| a == "/CURRENTUSER"));
    }

    #[test]
    fn when_to_retry_as_admin() {
        // Réussi : pas d'UAC.
        assert!(user_install_succeeded(0, false));
        // Échec d'Inno Setup : réessai en administrateur, même si un ancien jeu est décrit.
        for code in 1..=8 {
            assert!(!user_install_succeeded(code, true), "code {code}");
        }
        // Plantage à la fermeture (0xC000041D) : la description du jeu fait foi.
        assert!(user_install_succeeded(0xC000_041D, true));
        assert!(!user_install_succeeded(0xC000_041D, false));
    }
}
