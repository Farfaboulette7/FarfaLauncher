//! Jeux Windows sous Linux, comme Heroic Games Launcher : GE-Proton lancé par
//! umu-launcher (environnement Steam Runtime + correctifs par jeu).
//!
//! - Versions de Proton : celles déjà installées (Steam, `compatibilitytools.d`,
//!   les nôtres) sont réutilisées ; les autres sont téléchargées depuis les
//!   releases de GE-Proton. La plus récente de GE-Proton est recommandée.
//!   L'archive téléchargée est vérifiée par le `.sha512sum` publié avec elle (SL13).
//! - umu-launcher : celui du système s'il existe, sinon son « zipapp »
//!   (quelques centaines de Ko, demande python3), version épinglée et vérifiée
//!   par son SHA-256 (SL13). Sans python3, Proton est lancé directement
//!   (`proton waitforexitandrun`).
//! - Un préfixe Wine par jeu, dans le dossier de config du launcher (il survit
//!   à une désinstallation, comme chez Heroic).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tokio_util::sync::CancellationToken;

use super::net;
use crate::error::{log_err, AppError, AppResult};

const GE_RELEASES: &str = "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases?per_page=8";
/// umu-launcher épinglé (SL13) : une version précise et l'empreinte SHA-256 de son zipapp,
/// relevée sur sa release GitHub (champ `digest` de l'API, release 1.4.4 du 2026-07-25).
/// Pour passer à une version plus récente : changer l'adresse et l'empreinte ensemble.
/// Seul un umu pas encore installé est concerné : celui du système ou déjà téléchargé est gardé.
const UMU_ZIPAPP: &str = "https://github.com/Open-Wine-Components/umu-launcher/releases/download/1.4.4/umu-launcher-1.4.4-zipapp.tar";
const UMU_SHA256: &str = "eb590691841f7fad3fc3ad8fd5db4ccb87849fe7948e62b28ece7a4ee48cc851";

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
    /// Adresse du `.sha512sum` publié par GE avec l'archive (versions à télécharger).
    #[serde(skip)]
    pub checksum_url: Option<String>,
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
            // L'archive de GE-Proton11-11 donne le dossier GE-Proton11-11-x86_64.
            let name = name.strip_suffix("-x86_64").map(str::to_string).unwrap_or(name);
            if out.iter().any(|v| v.name == name) {
                continue;
            }
            out.push(ProtonVersion { name, path: Some(path), source, size: None, url: None, checksum_url: None, recommended: false });
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

/// `.sha512sum` de l'archive `tar` : `GE-Proton10-25.sha512sum` pour `GE-Proton10-25.tar.gz`.
fn checksum_asset<'a>(r: &'a Release, tar: &Asset) -> Option<&'a Asset> {
    let wanted = format!("{}.sha512sum", tar.name.strip_suffix(".tar.gz").unwrap_or(&tar.name));
    r.assets.iter().find(|a| a.name == wanted)
}

/// Empreinte attendue pour `file_name`, lue dans un fichier au format de `sha512sum`
/// (`<empreinte hexadécimale>  <nom>` par ligne, `*` devant le nom en mode binaire).
/// Une ligne seule sans nom est acceptée. `None` : pas d'empreinte valable de `len` caractères.
fn expected_digest(sums: &str, file_name: &str, len: usize) -> Option<String> {
    let valid = |h: &str| h.len() == len && h.bytes().all(|b| b.is_ascii_hexdigit());
    let lines: Vec<&str> = sums.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    for line in &lines {
        let mut parts = line.split_whitespace();
        let (Some(hash), name) = (parts.next(), parts.next()) else { continue };
        let name = name.map(|n| n.trim_start_matches('*'));
        // Le nom peut être précédé d'un dossier (`./GE-Proton10-25.tar.gz`).
        let same = name.is_some_and(|n| n.rsplit('/').next() == Some(file_name));
        if valid(hash) && (same || (name.is_none() && lines.len() == 1)) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

/// Empreinte hexadécimale d'un fichier, lue par blocs (archives de plusieurs centaines de Mo).
fn file_digest<D: Digest>(path: &Path) -> AppResult<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| log_err(AppError::Disk, "lecture pour vérification", e))?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| log_err(AppError::Disk, "lecture pour vérification", e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare l'empreinte de `path` à `expected` ; un fichier qui ne correspond pas est effacé.
fn check_digest<D: Digest>(path: &Path, expected: &str, what: &str) -> AppResult<()> {
    let actual = file_digest::<D>(path)?;
    if actual != expected.to_ascii_lowercase() {
        let _ = std::fs::remove_file(path);
        return Err(log_err(AppError::Integrity, "vérification", format!("{what} : empreinte {actual} ≠ {expected}")));
    }
    Ok(())
}

/// Versions installées et téléchargeables ; la recommandée en premier.
pub async fn list(tools: &Path) -> Vec<ProtonVersion> {
    let mut all = installed(tools);
    match releases().await {
        Ok(rs) => {
            for r in &rs {
                let Some(a) = tarball(r) else { continue };
                let checksum_url = checksum_asset(r, a).map(|c| c.browser_download_url.clone());
                if let Some(v) = all.iter_mut().find(|v| v.name == r.tag_name) {
                    v.url = Some(a.browser_download_url.clone());
                    v.checksum_url = checksum_url;
                } else {
                    all.push(ProtonVersion {
                        name: r.tag_name.clone(),
                        path: None,
                        source: "remote",
                        size: Some(a.size),
                        url: Some(a.browser_download_url.clone()),
                        checksum_url,
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
    // SL13 : sans `.sha512sum` publié par GE, on ne télécharge pas (archive invérifiable).
    let Some(checksum_url) = pick.checksum_url else {
        tracing::error!("{} : pas de .sha512sum publié, téléchargement refusé", pick.name);
        return Err(AppError::ProtonUnavailable);
    };
    let http = net::client();
    let dir = tools.join("proton");
    let archive = dir.join(format!("{}.tar.gz", pick.name));
    // Empreinte lue avant l'archive : inutile de télécharger des centaines de Mo sans elle.
    // Le nom cherché est celui de l'asset publié (`GE-Proton10-25.tar.gz`).
    let published_name = url.rsplit('/').next().unwrap_or_default().to_string();
    let sums = net::get_text(&http, &checksum_url).await?;
    let Some(expected) = expected_digest(&sums, &published_name, 128) else {
        tracing::error!("{} : .sha512sum illisible", pick.name);
        return Err(AppError::ProtonUnavailable);
    };
    tracing::info!("téléchargement de {}", pick.name);
    net::download(&http, &url, &archive, cancel, on_progress).await?;
    let (a, d) = (archive.clone(), dir.clone());
    tokio::task::spawn_blocking(move || {
        // Vérifiée avant toute extraction ; effacée si elle ne correspond pas.
        check_digest::<Sha512>(&a, &expected, "archive de GE-Proton")?;
        untar_gz(&a, &d)
    })
    .await
    .map_err(|_| AppError::Internal)??;
    let _ = std::fs::remove_file(&archive);
    let Some(path) = installed(tools).into_iter().find(|v| v.name == pick.name).and_then(|v| v.path) else {
        tracing::error!("{} : archive inattendue", pick.name);
        return Err(AppError::ProtonUnavailable);
    };
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
        let archive = tools.join("umu-zipapp.tar");
        net::download(&http, UMU_ZIPAPP, &archive, cancel, |_, _| {}).await?;
        let (a, d) = (archive.clone(), tools.to_path_buf());
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            // Version épinglée : l'archive doit avoir exactement l'empreinte notée (SL13).
            check_digest::<Sha256>(&a, UMU_SHA256, "zipapp d'umu")?;
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

    /// Seulement pour un client dont la fiche a `proton-steam: oui` (écart E5, voir
    /// `Catalog::pc_client_needs_steam`).
    /// Steam « présent » dans le préfixe : `C:\Program Files (x86)\Steam\steam.exe` et la
    /// clé `HKLM\Software\Wow6432Node\Valve\Steam\InstallPath`. Proton ne pose `steam.exe`
    /// qu'à partir du Steam de Linux (`STEAM_COMPAT_CLIENT_INSTALL_PATH/legacycompat`),
    /// absent sous umu ; boiii refuse alors de démarrer (« Steam must be installed… »).
    /// Le fichier posé n'est qu'un repère : Proton force son `steam.exe` intégré (`steam.exe=b`).
    /// boiii charge aussi `steamclient64.dll` et ses voisines depuis ce dossier : sans
    /// elles, le jeu plante à l'intro (boiii-free, PR #204). Elles sont copiées depuis le jeu.
    pub fn ensure_steam_marker(&self, game_dir: &Path) -> AppResult<()> {
        let drive_c = self.drive_c();
        let steam = drive_c.join("Program Files (x86)").join("Steam");
        let exe = steam.join("steam.exe");
        std::fs::create_dir_all(&steam).map_err(|e| log_err(AppError::Disk, "dossier Steam du préfixe", e))?;
        if !exe.exists() {
            std::fs::write(&exe, b"FarfaLauncher : Steam pour boiii\n").map_err(|e| log_err(AppError::Disk, "steam.exe du préfixe", e))?;
            tracing::info!("repère {} posé", exe.display());
        }
        for dll in ["steamclient64.dll", "tier0_s64.dll", "vstdlib_s64.dll", "gameoverlayrenderer64.dll"] {
            let (src, dst) = (game_dir.join(dll), steam.join(dll));
            if src.is_file() && !dst.exists() {
                match std::fs::copy(&src, &dst) {
                    Ok(_) => tracing::info!("{dll} copiée dans le Steam du préfixe"),
                    Err(e) => tracing::warn!("{dll} non copiée dans le Steam du préfixe: {e}"),
                }
            }
        }
        let Some(reg) = drive_c.parent().map(|p| p.join("system.reg")) else { return Ok(()) };
        let text = match std::fs::read_to_string(&reg) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!("{} illisible ({e}) : clé de Steam non posée", reg.display());
                return Ok(());
            }
        };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Some(new) = with_steam_install_path(&text, now) {
            std::fs::write(&reg, new).map_err(|e| log_err(AppError::Disk, "system.reg du préfixe", e))?;
            tracing::info!("clé InstallPath de Steam ajoutée à {}", reg.display());
        }
        Ok(())
    }
}

/// `system.reg` (registre Wine, HKLM) avec `InstallPath` de Steam dans la vue 32 bits,
/// ou `None` si la valeur y est déjà.
fn with_steam_install_path(reg: &str, now: u64) -> Option<String> {
    const KEY: &str = r"[Software\\Wow6432Node\\Valve\\Steam]";
    const VALUE: &str = r#""InstallPath"="C:\\Program Files (x86)\\Steam""#;
    let lower = reg.to_ascii_lowercase();
    let Some(start) = lower.find(&KEY.to_ascii_lowercase()) else {
        let mut out = reg.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&format!("\n{KEY} {now}\n{VALUE}\n"));
        return Some(out);
    };
    let line_end = reg[start..].find('\n').map(|i| start + i + 1).unwrap_or(reg.len());
    let block_end = reg[line_end..].find("\n[").map(|i| line_end + i + 1).unwrap_or(reg.len());
    if lower[line_end..block_end].contains("\"installpath\"=") {
        return None;
    }
    // Valeur ajoutée après la dernière ligne de la clé.
    let body_end = line_end + reg[line_end..block_end].trim_end().len();
    let mut out = String::with_capacity(reg.len() + VALUE.len() + 2);
    if body_end == line_end {
        out.push_str(&reg[..line_end]);
        out.push_str(VALUE);
        out.push('\n');
        out.push_str(&reg[line_end..]);
    } else {
        out.push_str(&reg[..body_end]);
        out.push('\n');
        out.push_str(VALUE);
        out.push_str(&reg[body_end..]);
    }
    Some(out)
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

    #[test]
    fn steam_install_path_in_registry() {
        let value = r#""InstallPath"="C:\\Program Files (x86)\\Steam""#;
        // Clé absente : ajoutée à la fin.
        let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wine] 1\n\"A\"=\"b\"\n";
        let out = with_steam_install_path(reg, 42).unwrap();
        assert!(out.starts_with(reg));
        assert!(out.ends_with(&format!("\n[Software\\\\Wow6432Node\\\\Valve\\\\Steam] 42\n{value}\n")));
        // Déjà là : rien à faire.
        assert!(with_steam_install_path(&out, 43).is_none());
        // Clé présente sans la valeur : ajoutée dans la clé, pas dans la suivante.
        let reg = "[Software\\\\Wow6432Node\\\\Valve\\\\Steam] 1\n#time=1\n\"Other\"=\"x\"\n\n[Software\\\\Z] 2\n\"InstallPath\"=\"y\"\n";
        let out = with_steam_install_path(reg, 3).unwrap();
        assert_eq!(
            out,
            format!("[Software\\\\Wow6432Node\\\\Valve\\\\Steam] 1\n#time=1\n\"Other\"=\"x\"\n{value}\n\n[Software\\\\Z] 2\n\"InstallPath\"=\"y\"\n")
        );
        // Clé vide.
        let out = with_steam_install_path("[Software\\\\Wow6432Node\\\\Valve\\\\Steam] 1\n", 3).unwrap();
        assert_eq!(out, format!("[Software\\\\Wow6432Node\\\\Valve\\\\Steam] 1\n{value}\n"));
    }

    #[test]
    fn sha512sum_of_ge_proton() {
        let tar = "GE-Proton10-25.tar.gz";
        let h = "a".repeat(128);
        // Format publié par GE : « <empreinte>  <nom> ».
        assert_eq!(expected_digest(&format!("{h}  {tar}\n"), tar, 128), Some(h.clone()));
        // Mode binaire, dossier devant le nom, majuscules.
        let up = "B".repeat(128);
        assert_eq!(expected_digest(&format!("{up} *./{tar}\n"), tar, 128), Some("b".repeat(128)));
        // Une seule empreinte sans nom.
        assert_eq!(expected_digest(&format!("{h}\n"), tar, 128), Some(h.clone()));
        // Autre fichier, mauvaise longueur, caractères non hexadécimaux, vide : refusé.
        assert_eq!(expected_digest(&format!("{h}  GE-Proton10-24.tar.gz"), tar, 128), None);
        assert_eq!(expected_digest(&format!("{}  {tar}", "a".repeat(64)), tar, 128), None);
        assert_eq!(expected_digest(&format!("{}  {tar}", "z".repeat(128)), tar, 128), None);
        assert_eq!(expected_digest("", tar, 128), None);
        // Plusieurs empreintes sans nom : laquelle ? Refusé.
        assert_eq!(expected_digest(&format!("{h}\n{h}\n"), tar, 128), None);
    }

    #[test]
    fn checksum_asset_matches_the_tarball() {
        let asset = |name: &str| Asset { name: name.into(), browser_download_url: format!("https://x/{name}"), size: 1 };
        let r = Release {
            tag_name: "GE-Proton10-25".into(),
            assets: vec![asset("GE-Proton10-25.sha512sum"), asset("GE-Proton10-25.tar.gz"), asset("GE-Proton10-25-aarch64.tar.gz")],
        };
        let tar = tarball(&r).unwrap();
        assert_eq!(tar.name, "GE-Proton10-25.tar.gz");
        assert_eq!(checksum_asset(&r, tar).unwrap().name, "GE-Proton10-25.sha512sum");
        // Pas de .sha512sum : rien (le téléchargement sera refusé).
        let r = Release { tag_name: "x".into(), assets: vec![asset("GE-Proton10-25.tar.gz")] };
        assert!(checksum_asset(&r, tarball(&r).unwrap()).is_none());
    }

    #[test]
    fn downloaded_archive_digest_is_checked() {
        let dir = std::env::temp_dir().join(format!("farfa-sl13-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("umu-zipapp.tar");
        std::fs::write(&path, b"abc").unwrap();
        // SHA-256 connu de « abc ».
        let good = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(check_digest::<Sha256>(&path, good, "essai").is_ok());
        assert!(matches!(check_digest::<Sha256>(&path, &"0".repeat(64), "essai"), Err(AppError::Integrity)));
        // Archive refusée : effacée, jamais extraite.
        assert!(!path.exists());
        // L'empreinte épinglée d'umu a la bonne forme.
        assert!(UMU_SHA256.len() == 64 && UMU_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(UMU_ZIPAPP.starts_with("https://github.com/Open-Wine-Components/umu-launcher/releases/download/"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn windows_paths() {
        assert_eq!(windows_path(Path::new("/home/a/Jeux/BO2")), r"Z:\home\a\Jeux\BO2");
    }
}
