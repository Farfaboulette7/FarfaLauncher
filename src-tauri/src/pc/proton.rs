//! Jeux Windows sous Linux, comme Heroic Games Launcher : GE-Proton lancé par
//! umu-launcher (environnement Steam Runtime + correctifs par jeu).
//!
//! - Versions de Proton : celles déjà installées (Steam, `compatibilitytools.d`,
//!   les nôtres) sont réutilisées ; les autres sont téléchargées depuis les
//!   releases de GE-Proton. La plus récente de GE-Proton est recommandée.
//! - umu-launcher : celui du système s'il existe, sinon son « zipapp »
//!   (quelques centaines de Ko, demande python3). Sans python3, Proton est
//!   lancé directement (`proton waitforexitandrun`).
//! - Un préfixe Wine par jeu, dans le dossier de config du launcher (il survit
//!   à une désinstallation, comme chez Heroic).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::net;
use crate::error::{log_err, AppError, AppResult};

const GE_RELEASES: &str = "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases?per_page=8";
const UMU_LATEST: &str = "https://api.github.com/repos/Open-Wine-Components/umu-launcher/releases/latest";

/// Une version de Proton, installée ou téléchargeable.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonVersion {
    pub name: String,
    /// Dossier, si elle est déjà sur cet ordinateur.
    pub path: Option<PathBuf>,
    /// `steam` : fournie par Steam ; `farfa` : téléchargée par le launcher ;
    /// `remote` : à télécharger.
    pub source: &'static str,
    /// Taille du téléchargement (versions à télécharger).
    pub size: Option<u64>,
    #[serde(skip)]
    pub url: Option<String>,
    pub recommended: bool,
}

/// Réglage Proton d'un jeu installé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtonSetup {
    /// Nom de la version (`GE-Proton11-11`) ; `None` : la recommandée.
    #[serde(default)]
    pub version: Option<String>,
    /// Dossier du préfixe Wine (données passées à `STEAM_COMPAT_DATA_PATH`).
    pub prefix: PathBuf,
}

/// De quoi lancer un programme Windows.
#[derive(Debug, Clone)]
pub struct Runner {
    pub proton: PathBuf,
    pub umu: Option<PathBuf>,
    pub prefix: PathBuf,
    /// `umu-<appid Steam>` ou `umu-default` (correctifs propres au jeu).
    pub game_id: String,
}

pub fn tools_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("tools")
}

pub fn prefixes_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("prefixes")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Dossiers de Steam où chercher des Proton déjà installés.
fn steam_dirs() -> Vec<(PathBuf, &'static str)> {
    let Some(h) = home() else { return Vec::new() };
    let mut out = Vec::new();
    for steam in [
        h.join(".steam/root"),
        h.join(".steam/steam"),
        h.join(".local/share/Steam"),
        h.join(".var/app/com.valvesoftware.Steam/data/Steam"),
    ] {
        out.push((steam.join("compatibilitytools.d"), "steam"));
        out.push((steam.join("steamapps/common"), "steam"));
    }
    out
}

/// Ordre des versions : `GE-Proton11-11` > `GE-Proton11-9` > `GE-Proton10-30`.
fn version_key(name: &str) -> Vec<u64> {
    name.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

fn is_ge(name: &str) -> bool {
    name.starts_with("GE-Proton")
}

/// Versions de Proton présentes sur l'ordinateur (dossiers qui ont un script `proton`).
pub fn installed(tools: &Path) -> Vec<ProtonVersion> {
    let mut dirs = vec![(tools.join("proton"), "farfa")];
    dirs.extend(steam_dirs());
    let mut out: Vec<ProtonVersion> = Vec::new();
    for (dir, source) in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(Result::ok) {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !path.join("proton").is_file() || (source == "steam" && !name.to_lowercase().contains("proton")) {
                continue;
            }
            if out.iter().any(|v| v.name == name) {
                continue;
            }
            out.push(ProtonVersion { name, path: Some(path), source, size: None, url: None, recommended: false });
        }
    }
    out
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

async fn releases() -> AppResult<Vec<Release>> {
    let text = net::get_text(&net::client(), GE_RELEASES).await?;
    serde_json::from_str(&text).map_err(|e| log_err(AppError::Download, "releases de GE-Proton", e))
}

/// Archive x86_64 d'une release de GE-Proton.
fn tarball(r: &Release) -> Option<&Asset> {
    r.assets.iter().find(|a| a.name.ends_with(".tar.gz") && !a.name.contains("aarch64"))
}

/// Versions installées et téléchargeables ; la recommandée en premier.
pub async fn list(tools: &Path) -> Vec<ProtonVersion> {
    let mut all = installed(tools);
    match releases().await {
        Ok(rs) => {
            for r in &rs {
                let Some(a) = tarball(r) else { continue };
                if let Some(v) = all.iter_mut().find(|v| v.name == r.tag_name) {
                    v.url = Some(a.browser_download_url.clone());
                } else {
                    all.push(ProtonVersion {
                        name: r.tag_name.clone(),
                        path: None,
                        source: "remote",
                        size: Some(a.size),
                        url: Some(a.browser_download_url.clone()),
                        recommended: false,
                    });
                }
            }
        }
        Err(e) => tracing::warn!("versions de GE-Proton indisponibles: {}", e.code()),
    }
    // La plus récente de GE-Proton, sinon la plus récente installée.
    all.sort_by_key(|v| std::cmp::Reverse((is_ge(&v.name), version_key(&v.name))));
    if let Some(first) = all.first_mut() {
        first.recommended = true;
    }
    all
}

/// Dossier de la version demandée (la recommandée si `None`), téléchargée si besoin.
pub async fn ensure(tools: &Path, wanted: Option<&str>, cancel: &CancellationToken, on_progress: impl FnMut(u64, u64)) -> AppResult<(String, PathBuf)> {
    // Déjà là : pas de réseau.
    if let Some(name) = wanted {
        if let Some(v) = installed(tools).into_iter().find(|v| v.name == name) {
            return Ok((v.name, v.path.unwrap_or_default()));
        }
    }
    let versions = list(tools).await;
    let pick = match wanted {
        Some(name) => versions.into_iter().find(|v| v.name == name),
        None => versions.into_iter().next(),
    }
    .ok_or(AppError::ProtonUnavailable)?;
    if let Some(path) = pick.path {
        return Ok((pick.name, path));
    }
    let url = pick.url.ok_or(AppError::ProtonUnavailable)?;
    let dir = tools.join("proton");
    let archive = dir.join(format!("{}.tar.gz", pick.name));
    tracing::info!("téléchargement de {}", pick.name);
    net::download(&net::client(), &url, &archive, cancel, on_progress).await?;
    let (a, d) = (archive.clone(), dir.clone());
    tokio::task::spawn_blocking(move || untar_gz(&a, &d)).await.map_err(|_| AppError::Internal)??;
    let _ = std::fs::remove_file(&archive);
    let path = dir.join(&pick.name);
    if !path.join("proton").is_file() {
        tracing::error!("{} : archive inattendue", pick.name);
        return Err(AppError::ProtonUnavailable);
    }
    Ok((pick.name, path))
}

fn untar_gz(archive: &Path, dest: &Path) -> AppResult<()> {
    let f = std::fs::File::open(archive).map_err(|e| log_err(AppError::Extract, "ouverture de l'archive", e))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    tar.set_preserve_permissions(true);
    tar.unpack(dest).map_err(|e| log_err(AppError::Extract, "extraction de l'archive", e))
}

fn on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

/// umu-run : celui du système, sinon le nôtre (téléchargé une fois). `None` :
/// pas de python3, Proton sera lancé directement.
pub async fn ensure_umu(tools: &Path, cancel: &CancellationToken) -> Option<PathBuf> {
    if let Some(p) = on_path("umu-run") {
        return Some(p);
    }
    let ours = tools.join("umu").join("umu-run");
    if ours.is_file() {
        return Some(ours);
    }
    if on_path("python3").is_none() {
        tracing::warn!("python3 absent : Proton sera lancé sans umu");
        return None;
    }
    let result: AppResult<PathBuf> = async {
        let http = net::client();
        let rel: Release = serde_json::from_str(&net::get_text(&http, UMU_LATEST).await?).map_err(|e| log_err(AppError::Download, "release d'umu", e))?;
        let asset = rel.assets.iter().find(|a| a.name.ends_with("-zipapp.tar")).ok_or(AppError::Download)?;
        let archive = tools.join("umu-zipapp.tar");
        net::download(&http, &asset.browser_download_url, &archive, cancel, |_, _| {}).await?;
        let (a, d) = (archive.clone(), tools.to_path_buf());
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let f = std::fs::File::open(&a).map_err(|e| log_err(AppError::Extract, "archive d'umu", e))?;
            let mut tar = tar::Archive::new(f);
            tar.set_preserve_permissions(true);
            tar.unpack(&d).map_err(|e| log_err(AppError::Extract, "extraction d'umu", e))
        })
        .await
        .map_err(|_| AppError::Internal)??;
        let _ = std::fs::remove_file(&archive);
        // L'archive contient `umu/umu-run`.
        crate::install::make_executable(&ours);
        if ours.is_file() { Ok(ours.clone()) } else { Err(AppError::Download) }
    }
    .await;
    match result {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("umu-launcher indisponible ({}), Proton lancé directement", e.code());
            None
        }
    }
}

/// Chemin vu par Windows (lecteur `Z:` = racine de Linux).
pub fn windows_path(p: &Path) -> String {
    if cfg!(windows) {
        return p.to_string_lossy().into_owned();
    }
    format!("Z:{}", p.to_string_lossy().replace('/', "\\"))
}

impl Runner {
    /// `drive_c` du préfixe : umu fait pointer `pfx` sur le préfixe lui-même,
    /// Proton seul le crée en sous-dossier `pfx/`.
    pub fn drive_c(&self) -> PathBuf {
        let nested = self.prefix.join("pfx").join("drive_c");
        if nested.is_dir() || self.umu.is_none() {
            nested
        } else {
            self.prefix.join("drive_c")
        }
    }

    /// `%LOCALAPPDATA%` du préfixe.
    pub fn local_appdata(&self) -> PathBuf {
        self.drive_c().join("users").join("steamuser").join("AppData").join("Local")
    }

    /// Commande qui lance `exe` (chemin absolu) avec Proton.
    pub fn command(&self, exe: &Path, args: &[String]) -> Command {
        let mut cmd;
        if let Some(umu) = &self.umu {
            cmd = Command::new(umu);
            cmd.arg(exe);
            cmd.env("WINEPREFIX", &self.prefix).env("GAMEID", &self.game_id).env("PROTONPATH", &self.proton);
        } else {
            cmd = Command::new(self.proton.join("proton"));
            cmd.arg("waitforexitandrun").arg(exe);
            let steam = steam_dirs().into_iter().map(|(d, _)| d.parent().map(Path::to_path_buf).unwrap_or(d)).find(|d| d.is_dir());
            cmd.env("STEAM_COMPAT_DATA_PATH", &self.prefix)
                .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam.unwrap_or_else(|| self.prefix.clone()));
        }
        cmd.args(args);
        cmd
    }

    /// Crée le préfixe s'il n'existe pas encore (fichiers à y poser avant la
    /// première partie, comme ceux des clients Call of Duty).
    pub fn ensure_prefix(&self) -> AppResult<()> {
        if self.drive_c().join("users").is_dir() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.prefix).map_err(|e| log_err(AppError::Disk, "dossier du préfixe", e))?;
        tracing::info!("création du préfixe {}", self.prefix.display());
        let mut cmd = if self.umu.is_some() { self.command(Path::new(""), &[]) } else { self.command(Path::new("wineboot"), &["-u".into()]) };
        crate::external::use_system_env(&mut cmd);
        let status = cmd.status().map_err(|e| log_err(AppError::LaunchFailed, "création du préfixe", e))?;
        if !self.drive_c().join("users").is_dir() {
            tracing::error!("préfixe non créé ({status})");
            return Err(AppError::LaunchFailed);
        }
        Ok(())
    }
}

/// Prépare Proton (et umu) pour un jeu : téléchargements si besoin.
pub async fn runner(
    config_dir: &Path,
    setup: &ProtonSetup,
    steam_app_id: Option<u32>,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u64, u64),
) -> AppResult<Runner> {
    let tools = tools_dir(config_dir);
    let (_, proton) = ensure(&tools, setup.version.as_deref(), cancel, on_progress).await?;
    let umu = ensure_umu(&tools, cancel).await;
    Ok(Runner {
        proton,
        umu,
        prefix: setup.prefix.clone(),
        game_id: steam_app_id.map(|id| format!("umu-{id}")).unwrap_or_else(|| "umu-default".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        let mut v = vec!["GE-Proton10-30", "Proton 9.0 (Beta)", "GE-Proton11-9", "GE-Proton11-11"];
        v.sort_by_key(|n| std::cmp::Reverse((is_ge(n), version_key(n))));
        assert_eq!(v, ["GE-Proton11-11", "GE-Proton11-9", "GE-Proton10-30", "Proton 9.0 (Beta)"]);
    }

    #[cfg(unix)]
    #[test]
    fn windows_paths() {
        assert_eq!(windows_path(Path::new("/home/a/Jeux/BO2")), r"Z:\home\a\Jeux\BO2");
    }
}
