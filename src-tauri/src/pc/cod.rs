//! Call of Duty jouables grâce aux clients communautaires, comme le fait le
//! launcher de CB Servers (github.com/CBServers/cb-launcher) :
//!
//! | Jeu                 | Campagne                    | Multijoueur | Zombies |
//! |---------------------|-----------------------------|-------------|---------|
//! | Modern Warfare      | IW3SP-Mod                   | IW3x        | —       |
//! | World at War        | Plutonium (t4sp)            | t4mp        | t4sp    |
//! | Modern Warfare 2    | IW4x-SP (AlterWare)         | IW4x        | —       |
//! | Black Ops           | Plutonium (t5sp)            | t5mp        | t5sp    |
//! | Black Ops 2         | T6SP-Mod                    | t6mp        | t6zm    |
//! | Modern Warfare 3    | IW5-Mod (AlterWare)         | iw5mp       | —       |
//!
//! Les fichiers des clients viennent du dépôt public de CB Servers
//! (manifeste `[chemin, taille, sha1, destination?]`) et sont revérifiés
//! avant chaque partie. Plutonium et AlterWare se mettent ensuite à jour seuls.
//! Les modes Plutonium se jouent en ligne (compte Plutonium, via son
//! launcher) ou hors ligne en LAN (sans compte).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio_util::sync::CancellationToken;

use super::net;
use crate::error::{log_err, AppError, AppResult};

const UPDATER: &str = "https://github.com/CBServers/updater/raw/main/updater";
const PLUTONIUM_PROD: &str = "https://cdn.plutonium.pw/updater/prod.json";
/// Gros fichiers optionnels des manifestes (rapporteur de plantage d'IW4x).
const SKIPPED_PREFIXES: &[&str] = &["crash-reporter/"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodGame {
    Cod4,
    Waw,
    Mw2,
    Bo1,
    Bo2,
    Mw3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodMode {
    Sp,
    Mp,
    Zm,
}

/// Mode proposé au clic sur « Jouer ».
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeInfo {
    pub mode: CodMode,
    pub label: &'static str,
    /// Passe par Plutonium : en ligne ou en LAN.
    pub plutonium: bool,
}

/// Fichiers d'un client, d'après un manifeste de CB Servers.
struct Client {
    manifest: &'static str,
    /// Fichiers à prendre (vide : tous).
    only: &'static [&'static str],
    /// Fichier présent quand le client est déjà installé.
    marker: &'static str,
}

/// Comment lancer un mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeLaunch {
    /// Programme du dossier du jeu.
    Exe { exe: &'static str, args: Vec<String> },
    /// Jeu Plutonium (`t6zm`…) ; `path_key` : sa clé dans `config.json`.
    Plutonium { game: &'static str, path_key: &'static str },
}

impl CodGame {
    pub fn from_zip_name(name: &str) -> Option<Self> {
        let stem = name.to_ascii_lowercase().strip_suffix("_game_files.zip")?.to_string();
        Some(match stem.as_str() {
            "cod4" => CodGame::Cod4,
            "waw" => CodGame::Waw,
            "mw2" => CodGame::Mw2,
            "bo1" => CodGame::Bo1,
            "bo2" => CodGame::Bo2,
            "mw3" => CodGame::Mw3,
            _ => return None,
        })
    }

    fn clients(self) -> &'static [Client] {
        match self {
            CodGame::Cod4 => &[
                Client { manifest: "iw3x", only: &[], marker: "miles32.dll" },
                Client { manifest: "iw3sp-mod", only: &[], marker: "game.dll" },
            ],
            CodGame::Waw | CodGame::Bo1 => &[Client { manifest: "alt-launchers", only: &["plutonium.exe"], marker: "plutonium.exe" }],
            CodGame::Mw2 => &[Client { manifest: "iw4x", only: &[], marker: "alterware-launcher.exe" }],
            CodGame::Bo2 => &[Client { manifest: "t6", only: &[], marker: "T6SP-Mod.exe" }],
            CodGame::Mw3 => &[Client {
                manifest: "alt-launchers",
                only: &["plutonium.exe", "alterware-launcher.exe"],
                marker: "alterware-launcher.exe",
            }],
        }
    }

    /// Dossier des fichiers « appdata » des clients (dans `%LOCALAPPDATA%`).
    fn appdata_folder(self) -> &'static str {
        "CallofDuty4MW"
    }

    /// Programme du jeu de base : sa présence vaut « installé ».
    pub fn main_exe(self) -> &'static str {
        match self {
            CodGame::Cod4 => "iw3mp.exe",
            CodGame::Waw => "CoDWaWmp.exe",
            CodGame::Mw2 => "iw4mp.exe",
            CodGame::Bo1 => "BlackOpsMP.exe",
            CodGame::Bo2 => "t6mp.exe",
            CodGame::Mw3 => "iw5mp.exe",
        }
    }

    pub fn modes(self) -> Vec<ModeInfo> {
        [CodMode::Sp, CodMode::Mp, CodMode::Zm]
            .into_iter()
            .filter_map(|mode| {
                let launch = self.launch(mode)?;
                let label = match mode {
                    CodMode::Sp => "Campagne",
                    CodMode::Mp => "Multijoueur",
                    CodMode::Zm => "Zombies",
                };
                Some(ModeInfo { mode, label, plutonium: matches!(launch, ModeLaunch::Plutonium { .. }) })
            })
            .collect()
    }

    pub fn launch(self, mode: CodMode) -> Option<ModeLaunch> {
        use CodMode::*;
        let exe = |exe: &'static str, args: &[&str]| Some(ModeLaunch::Exe { exe, args: args.iter().map(|a| a.to_string()).collect() });
        let pluto = |game: &'static str, path_key: &'static str| Some(ModeLaunch::Plutonium { game, path_key });
        match (self, mode) {
            (CodGame::Cod4, Sp) => exe("iw3sp_mod.exe", &["+set", "iw3sp_auto_update", "0"]),
            (CodGame::Cod4, Mp) => exe("iw3mp.exe", &[]),
            (CodGame::Waw, Sp | Zm) => pluto("t4sp", "t4Path"),
            (CodGame::Waw, Mp) => pluto("t4mp", "t4Path"),
            (CodGame::Mw2, Sp) => exe("alterware-launcher.exe", &["iw4x-sp", "--skip-launcher-update"]),
            (CodGame::Mw2, Mp) => exe("iw4x.exe", &[]),
            (CodGame::Bo1, Sp | Zm) => pluto("t5sp", "t5Path"),
            (CodGame::Bo1, Mp) => pluto("t5mp", "t5Path"),
            (CodGame::Bo2, Sp) => exe("T6SP-Mod.exe", &[]),
            (CodGame::Bo2, Mp) => pluto("t6mp", "t6Path"),
            (CodGame::Bo2, Zm) => pluto("t6zm", "t6Path"),
            (CodGame::Mw3, Sp) => exe("alterware-launcher.exe", &["iw5-mod", "--skip-launcher-update", "--pass", "-singleplayer"]),
            (CodGame::Mw3, Mp) => pluto("iw5mp", "iw5Path"),
            (CodGame::Cod4 | CodGame::Mw2 | CodGame::Mw3, Zm) => None,
        }
    }
}

/// Entrée d'un manifeste : `[chemin, taille, sha1, destination?]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: String,
    size: u64,
    sha1: String,
    appdata: bool,
}

fn parse_manifest(text: &str) -> AppResult<Vec<Entry>> {
    let values: Vec<serde_json::Value> = serde_json::from_str(text).map_err(|e| log_err(AppError::Download, "manifeste du client", e))?;
    Ok(values
        .iter()
        .filter_map(|v| {
            let a = v.as_array()?;
            Some(Entry {
                path: a.first()?.as_str()?.to_string(),
                size: a.get(1)?.as_u64()?,
                sha1: a.get(2)?.as_str()?.to_ascii_lowercase(),
                appdata: a.get(3).and_then(|d| d.as_str()) == Some("appdata"),
            })
        })
        .collect())
}

fn sha1_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}

/// Où poser les fichiers des clients.
pub struct ClientDirs {
    pub game_dir: PathBuf,
    /// `%LOCALAPPDATA%` (celui du préfixe Proton sous Linux).
    pub local_appdata: PathBuf,
}

/// Télécharge les fichiers manquants ou modifiés des clients du jeu.
/// Serveur injoignable : on garde ce qui est en place s'il y a déjà le client.
pub async fn sync_clients(
    game: CodGame,
    dirs: &ClientDirs,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> AppResult<()> {
    let http = net::client();
    let mut todo: Vec<(String, Entry, PathBuf)> = Vec::new();
    for client in game.clients() {
        let entries = match net::get_text(&http, &format!("{UPDATER}/{}.json", client.manifest)).await.and_then(|t| parse_manifest(&t)) {
            Ok(e) => e,
            Err(e) => {
                if dirs.game_dir.join(client.marker).exists() {
                    tracing::warn!("manifeste {} indisponible, client en place gardé", client.manifest);
                    continue;
                }
                return Err(e);
            }
        };
        let blocking_entries: Vec<(Entry, PathBuf)> = entries
            .into_iter()
            .filter(|e| client.only.is_empty() || client.only.iter().any(|o| o.eq_ignore_ascii_case(&e.path)))
            .filter(|e| !SKIPPED_PREFIXES.iter().any(|p| e.path.starts_with(p)))
            .map(|e| {
                let base = if e.appdata { dirs.local_appdata.join(game.appdata_folder()) } else { dirs.game_dir.clone() };
                let dest = base.join(crate::install::rel_to_path(&e.path));
                (e, dest)
            })
            .collect();
        let stale = tokio::task::spawn_blocking(move || {
            blocking_entries
                .into_iter()
                .filter(|(e, dest)| {
                    let same = std::fs::metadata(dest).map(|m| m.len() == e.size).unwrap_or(false)
                        && sha1_file(dest).as_deref() == Some(e.sha1.as_str());
                    !same
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|_| AppError::Internal)?;
        todo.extend(stale.into_iter().map(|(e, d)| (client.manifest.to_string(), e, d)));
    }
    let total: u64 = todo.iter().map(|(_, e, _)| e.size).sum();
    if todo.is_empty() {
        return Ok(());
    }
    tracing::info!("{} fichier(s) de client à télécharger ({total} octets)", todo.len());
    let mut done = 0u64;
    for (manifest, entry, dest) in todo {
        let url = format!("{UPDATER}/{manifest}/{}", net::encode_path(&entry.path));
        let tmp = dest.with_extension("farfa-dl");
        let _ = std::fs::remove_file(&tmp);
        net::download(&http, &url, &tmp, cancel, |got, _| on_progress(done + got, total)).await?;
        let hash = {
            let t = tmp.clone();
            tokio::task::spawn_blocking(move || sha1_file(&t)).await.map_err(|_| AppError::Internal)?
        };
        if hash.as_deref() != Some(entry.sha1.as_str()) {
            let _ = std::fs::remove_file(&tmp);
            tracing::error!("{}: empreinte inattendue", entry.path);
            return Err(AppError::Integrity);
        }
        std::fs::rename(&tmp, &dest).map_err(|e| crate::install::download::disk_error("pose d'un fichier du client", e))?;
        done += entry.size;
    }
    Ok(())
}

// ---------- Plutonium ----------

pub fn plutonium_root(local_appdata: &Path) -> PathBuf {
    local_appdata.join("Plutonium")
}

impl CodGame {
    /// Profils et sauvegardes : `storage/<jeu>/players` de Plutonium, sinon
    /// `players` à côté du jeu (clients IW3x, IW4x).
    pub fn save_dir(self, game_dir: &Path, local_appdata: &Path) -> PathBuf {
        let storage = |game: &str| plutonium_root(local_appdata).join("storage").join(game).join("players");
        match self {
            CodGame::Waw => storage("t4"),
            CodGame::Bo1 => storage("t5"),
            CodGame::Bo2 => storage("t6"),
            CodGame::Mw3 => storage("iw5"),
            CodGame::Cod4 | CodGame::Mw2 => game_dir.join("players"),
        }
    }
}

fn read_revision(text: &str) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(text).ok()?.get("revision")?.as_i64()
}

pub fn local_revision(root: &Path) -> Option<i64> {
    read_revision(&std::fs::read_to_string(root.join("info.json")).ok()?)
}

/// Révision publiée de Plutonium (`prod.json` → premier manifeste).
pub async fn remote_revision() -> Option<i64> {
    let http = net::client();
    let prod: serde_json::Value = serde_json::from_str(&net::get_text(&http, PLUTONIUM_PROD).await.ok()?).ok()?;
    let manifest = prod.get("manifests")?.as_array()?.first()?.as_str()?.to_string();
    read_revision(&net::get_text(&http, &manifest).await.ok()?)
}

pub fn launcher_exe(root: &Path) -> PathBuf {
    root.join("bin").join("plutonium-launcher-win32.exe")
}

pub fn bootstrapper_exe(root: &Path) -> PathBuf {
    root.join("bin").join("plutonium-bootstrapper-win32.exe")
}

/// Compte Plutonium connecté sur cet ordinateur.
pub fn has_token(root: &Path) -> bool {
    std::fs::read_to_string(root.join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("token")?.as_str().map(|s| !s.is_empty()))
        .unwrap_or(false)
}

/// Indique à Plutonium où est le jeu (`config.json`), sans toucher au reste
/// (le jeton de connexion y est aussi).
pub fn set_game_path(root: &Path, key: &str, path: &str) -> AppResult<()> {
    let file = root.join("config.json");
    let mut doc = match std::fs::read_to_string(&file) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) if v.is_object() => v,
            _ => {
                tracing::warn!("config.json de Plutonium illisible, laissé tel quel");
                return Ok(());
            }
        },
        Err(_) => serde_json::json!({}),
    };
    let obj = doc.as_object_mut().ok_or(AppError::Internal)?;
    if obj.get(key).and_then(|v| v.as_str()) == Some(path) {
        return Ok(());
    }
    for field in ["iw5Path", "t4Path", "t5Path", "t6Path", "token"] {
        obj.entry(field).or_insert_with(|| serde_json::Value::String(String::new()));
    }
    obj.insert(key.to_string(), serde_json::Value::String(path.to_string()));
    std::fs::create_dir_all(root).map_err(|e| log_err(AppError::Disk, "dossier de Plutonium", e))?;
    let text = serde_json::to_string_pretty(&doc).map_err(|e| log_err(AppError::Internal, "config.json", e))?;
    std::fs::write(&file, text).map_err(|e| log_err(AppError::Disk, "config.json de Plutonium", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_names() {
        assert_eq!(CodGame::from_zip_name("cod4_game_files.zip"), Some(CodGame::Cod4));
        assert_eq!(CodGame::from_zip_name("BO2_game_files.zip"), Some(CodGame::Bo2));
        assert_eq!(CodGame::from_zip_name("cod2_game_files.zip"), None);
        assert_eq!(CodGame::from_zip_name("mw2.zip"), None);
    }

    #[test]
    fn modes() {
        let labels = |g: CodGame| g.modes().iter().map(|m| (m.label, m.plutonium)).collect::<Vec<_>>();
        assert_eq!(labels(CodGame::Bo2), [("Campagne", false), ("Multijoueur", true), ("Zombies", true)]);
        assert_eq!(labels(CodGame::Cod4), [("Campagne", false), ("Multijoueur", false)]);
        assert_eq!(labels(CodGame::Waw), [("Campagne", true), ("Multijoueur", true), ("Zombies", true)]);
        assert!(CodGame::Mw2.modes().iter().all(|m| !m.plutonium));
        assert_eq!(CodGame::Mw3.launch(CodMode::Mp), Some(ModeLaunch::Plutonium { game: "iw5mp", path_key: "iw5Path" }));
    }

    #[test]
    fn manifest() {
        let m = r#"[["iw3mp.exe",3,"C160AE",  "game"],["bin/launcher.dll",6,"2ACC","appdata"],["x",1,"AB"],{"ManifestHash":"9D"}]"#;
        let e = parse_manifest(m).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(e[0], Entry { path: "iw3mp.exe".into(), size: 3, sha1: "c160ae".into(), appdata: false });
        assert!(e[1].appdata && !e[2].appdata);
    }

    #[test]
    fn plutonium_config_keeps_token() {
        let root = std::env::temp_dir().join(format!("farfa-pluto-{}", uuid::Uuid::new_v4()));
        set_game_path(&root, "t6Path", r"C:\Jeux\BO2").unwrap();
        assert!(!has_token(&root));
        std::fs::write(root.join("config.json"), r#"{"token":"abc","t6Path":"old"}"#).unwrap();
        set_game_path(&root, "t6Path", r"Z:\home\x\BO2").unwrap();
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("config.json")).unwrap()).unwrap();
        assert_eq!(v["token"], "abc");
        assert_eq!(v["t6Path"], r"Z:\home\x\BO2");
        assert_eq!(v["t4Path"], "");
        assert!(has_token(&root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
