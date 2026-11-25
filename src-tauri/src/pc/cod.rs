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
//! | Black Ops III       | boiii (Ezz)                 | boiii       | boiii   |
//!
//! Black Ops III est en plusieurs archives (`bo3_game_files-01.zip`…) et son client,
//! boiii-free, vient de FarfaRomManager (fiche `boiii`, version suivie et gardée là-bas),
//! pas de CB Servers : voir `install::sync_boiii`.
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

/// Dépôt de CB Servers, branche `main` : manifestes et fichiers des clients.
///
/// Risque connu et accepté (SL13) : chaque fichier est vérifié par le SHA-1 de son manifeste,
/// mais le manifeste vient du même dépôt que les fichiers. Quelqu'un qui prendrait la main sur
/// le compte GitHub de CB Servers changerait les deux ensemble : seul HTTPS protège alors les
/// clients. Rien de simple n'y remédie :
/// - épingler un commit figerait les clients, alors qu'ils doivent suivre les serveurs (règle
///   « toujours la dernière version », risque SL2 accepté) ;
/// - CB Servers ne publie ni signature ni empreinte ailleurs que dans ce manifeste.
///
/// Plutonium et AlterWare se mettent ensuite à jour seuls, hors du launcher.
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
    Bo3,
}

/// Fiche de FarfaRomManager qui fournit boiii (client de Black Ops III), quand aucune fiche du
/// catalogue ne déclare encore `pour:` (ancien gestionnaire) : voir `Catalog::pc_client`.
pub const BOIII_CLIENT: &str = "boiii";

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
    /// Motif de ses zips dans RomM (sans morceau ni `.zip`) : ce que déclare `pour:` dans la
    /// fiche de son client (écart E2).
    pub fn zip_motif(self) -> &'static str {
        match self {
            CodGame::Cod4 => "cod4_game_files",
            CodGame::Waw => "waw_game_files",
            CodGame::Mw2 => "mw2_game_files",
            CodGame::Bo1 => "bo1_game_files",
            CodGame::Bo2 => "bo2_game_files",
            CodGame::Mw3 => "mw3_game_files",
            CodGame::Bo3 => "bo3_game_files",
        }
    }

    /// `bo2_game_files.zip`, ou un morceau `bo3_game_files-01.zip`.
    pub fn from_zip_name(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        let base = lower.strip_suffix(".zip")?;
        let base = match base.rsplit_once('-') {
            Some((head, part)) if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) => head,
            _ => base,
        };
        let stem = base.strip_suffix("_game_files")?.to_string();
        Some(match stem.as_str() {
            "cod4" => CodGame::Cod4,
            "waw" => CodGame::Waw,
            "mw2" => CodGame::Mw2,
            "bo1" => CodGame::Bo1,
            "bo2" => CodGame::Bo2,
            "mw3" => CodGame::Mw3,
            "bo3" => CodGame::Bo3,
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
            // Client servi par FarfaRomManager (install::sync_boiii).
            CodGame::Bo3 => &[],
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
            CodGame::Bo3 => "BlackOps3.exe",
        }
    }

    pub fn modes(self) -> Vec<ModeInfo> {
        [CodMode::Sp, CodMode::Mp, CodMode::Zm]
            .into_iter()
            .filter_map(|mode| {
                let launch = self.launch(mode)?;
                let label = match mode {
                    // Un seul lancement : campagne, multijoueur et zombies se choisissent dans le jeu.
                    _ if self == CodGame::Bo3 => "Black Ops III",
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
            // -noupdate : la version de boiii est celle de FarfaRomManager ; -launch : sans son écran.
            (CodGame::Bo3, Mp) => exe("boiii.exe", &["-noupdate", "-launch"]),
            (CodGame::Bo3, Sp | Zm) => None,
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
            CodGame::Bo3 => game_dir.join("boiii_players").join("user"),
        }
    }
}

// ---------- Black Ops III : zones et langue ----------
//
// La langue du jeu vient de `localization.txt`, à la racine : c'est ce que fait le
// « Language Switcher » fourni avec le jeu (dossiers `French` et `English`). Les versions
// 1.0 et 1.1 bêta renommaient en plus les zones `fr_*` en `en_*` (et `snd/fr` en `snd/en`),
// en croyant le jeu bloqué en anglais par boiii : il cherchait alors les `fr_*` disparus et
// plantait. Ces renommages sont défaits (`restore_bo3_zones`), au changement de langue et
// avant chaque partie. Le choix se fait dans l'onglet Informations du jeu.
//
// Quand retirer la rustine (écart E3, noté le 24/11/2026) : `restore_bo3_zones` et
// `repair_bo3_zone` (installations d'avant la 0.9.8) ne servent qu'aux jeux installés ou passés
// en français par un launcher 1.0 ou 1.1 bêta. Les retirer quand plus aucun joueur n'a lancé
// Black Ops III avec un de ces launchers depuis : au plus tôt quand tous les amis ont lancé BO3
// au moins une fois avec la 1.2.0 ou plus (chaque partie répare le jeu), par exemple en
// vérifiant les rapports et les statistiques du gestionnaire (version du launcher par joueur).
// Garder alors `localization.txt` (le « Language Switcher ») : c'est la vraie façon de changer
// la langue, voulue à terme sous forme de `langue: ligne | localization.txt | … | fr | en`.

/// Langues proposées pour Black Ops III (le jeu les contient toutes les deux).
pub const BO3_LANGUAGES: [&str; 2] = ["fr", "en"];
/// Zones anglaises mises de côté pendant que le français est en place.
const BO3_KEPT: &str = "bo3-zones-en";
const SND_KEPT: &str = "snd-en";

fn bo3_kept(game_dir: &Path) -> PathBuf {
    game_dir.join(crate::install::extract::INTERNAL_DIR).join(BO3_KEPT)
}

/// `localization.txt` du « Language Switcher » fourni avec le jeu.
fn bo3_switcher(game_dir: &Path, lang: &str) -> PathBuf {
    let folder = if lang == "fr" { "French" } else { "English" };
    game_dir.join("_Language Switcher").join(folder).join("localization.txt")
}

/// Langue en place : `fr` si `localization.txt` est celui du français.
pub fn bo3_language(game_dir: &Path) -> &'static str {
    let read = |p: &Path| std::fs::read(p).ok();
    match (read(&game_dir.join("localization.txt")), read(&bo3_switcher(game_dir, "fr"))) {
        (Some(cur), Some(fr)) if cur == fr => "fr",
        _ => "en",
    }
}

/// `fr_` ou `en_` en tête de nom (sans tenir compte de la casse) : le reste du nom.
fn strip_lang<'a>(name: &'a str, lang: &str) -> Option<&'a str> {
    let head = name.get(..3)?;
    head.eq_ignore_ascii_case(&format!("{lang}_")).then(|| &name[3..])
}

/// Banques de sons d'un dossier `snd/<langue>` : `core.en.sabl` devient `core.fr.sabl`
/// (`from` → `to`). Ne remplace jamais un fichier déjà là.
fn rename_snd_banks(dir: &Path, from: &str, to: &str) -> std::io::Result<usize> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let (from, to) = (format!(".{from}."), format!(".{to}."));
    let mut n = 0;
    for (name, path) in files_of(dir)? {
        let lower = name.to_ascii_lowercase();
        let Some(at) = lower.find(&from) else { continue };
        let renamed = format!("{}{to}{}", &name[..at], &name[at + from.len()..]);
        let dest = dir.join(&renamed);
        if dest.exists() {
            continue;
        }
        std::fs::rename(&path, &dest)?;
        n += 1;
    }
    Ok(n)
}

fn files_of(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            out.push((entry.file_name().to_string_lossy().into_owned(), entry.path()));
        }
    }
    Ok(out)
}

/// Met Black Ops III en `fr` ou en `en` : seulement `localization.txt`, comme le
/// « Language Switcher » fourni avec le jeu. Défait d'abord les renommages de zones des
/// versions 1.0 et 1.1 bêta : le jeu, mis en français par ce fichier, y cherchait des `fr_*`
/// et `snd/fr` qui n'existaient plus, et plantait au démarrage (rapport 20261117-130619-1-10625).
pub fn set_bo3_language(game_dir: &Path, lang: &str) -> std::io::Result<()> {
    restore_bo3_zones(game_dir)?;
    let switcher = bo3_switcher(game_dir, lang);
    if switcher.is_file() {
        std::fs::copy(&switcher, game_dir.join("localization.txt"))?;
        tracing::info!("Black Ops III en « {lang} »");
    }
    Ok(())
}

/// Remet les zones et les sons sous leurs vrais noms (`fr_*`, `snd/fr/*.fr.*`) après un
/// passage au français fait par renommage. Rejouable sans risque : reprend un changement
/// interrompu, et accepte une réinstallation faite par-dessus (zones neuves des deux langues).
/// Appelé aussi avant chaque partie, pour réparer les jeux déjà passés en français.
pub fn restore_bo3_zones(game_dir: &Path) -> std::io::Result<()> {
    use std::fs;
    let zone = game_dir.join("zone");
    let kept = bo3_kept(game_dir);
    if !zone.is_dir() || !kept.is_dir() {
        return Ok(());
    }
    let (snd_fr, snd_en, snd_kept) = (zone.join("snd").join("fr"), zone.join("snd").join("en"), kept.join(SND_KEPT));
    let mut moved = 0;
    for (name, en_kept) in files_of(&kept)? {
        let Some(rest) = strip_lang(&name, "en") else { continue };
        let (en, fr) = (zone.join(format!("en_{rest}")), zone.join(format!("fr_{rest}")));
        if fr.exists() && en.exists() {
            // Réinstallé par-dessus : les deux langues sont déjà en place.
            fs::remove_file(&en_kept)?;
            continue;
        }
        if en.exists() {
            fs::rename(&en, &fr)?;
        }
        fs::rename(&en_kept, &en)?;
        moved += 1;
    }
    if snd_kept.is_dir() {
        if snd_fr.is_dir() && snd_en.is_dir() {
            fs::remove_dir_all(&snd_kept)?;
        } else {
            if snd_en.is_dir() {
                // Banques passées en `*.en.*` par la version de test : elles reprennent `*.fr.*`.
                moved += rename_snd_banks(&snd_en, "en", "fr")?;
                fs::rename(&snd_en, &snd_fr)?;
            }
            fs::rename(&snd_kept, &snd_en)?;
        }
    }
    let _ = fs::remove_dir(&kept);
    if moved > 0 {
        tracing::info!("Black Ops III : {moved} zone(s) remise(s) sous leur vrai nom");
    }
    Ok(())
}

/// Les installations faites avant la 0.9.8 ont posé le contenu de `zone/` à la racine du jeu
/// (morceaux 02 et suivants aplatis) : le jeu ne trouve alors aucune zone. On les range.
pub fn repair_bo3_zone(game_dir: &Path) -> std::io::Result<usize> {
    use std::fs;
    let zone = game_dir.join("zone");
    let mut repaired = 0;
    for entry in fs::read_dir(game_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let is_zone_file = entry.file_type()?.is_file() && [".ff", ".fd", ".xpak"].iter().any(|e| lower.ends_with(e));
        let is_snd = entry.file_type()?.is_dir() && lower == "snd";
        if !is_zone_file && !is_snd {
            continue;
        }
        fs::create_dir_all(&zone)?;
        repaired += move_into(&entry.path(), &zone.join(&name))?;
    }
    if repaired > 0 {
        tracing::warn!("Black Ops III : {repaired} fichier(s) de zone/ rangés (installation aplatie)");
    }
    Ok(repaired)
}

/// Déplace `src` vers `dst` ; deux dossiers sont fusionnés, un fichier déjà là est gardé.
fn move_into(src: &Path, dst: &Path) -> std::io::Result<usize> {
    if !dst.exists() {
        std::fs::rename(src, dst)?;
        return Ok(1);
    }
    if !(src.is_dir() && dst.is_dir()) {
        return Ok(0);
    }
    let mut n = 0;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        n += move_into(&entry.path(), &dst.join(entry.file_name()))?;
    }
    let _ = std::fs::remove_dir(src);
    Ok(n)
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
        assert_eq!(CodGame::from_zip_name("bo3_game_files-07.zip"), Some(CodGame::Bo3));
        assert_eq!(CodGame::from_zip_name("bo3_game_files-x.zip"), None);
    }

    #[test]
    fn modes() {
        let labels = |g: CodGame| g.modes().iter().map(|m| (m.label, m.plutonium)).collect::<Vec<_>>();
        assert_eq!(labels(CodGame::Bo2), [("Campagne", false), ("Multijoueur", true), ("Zombies", true)]);
        assert_eq!(labels(CodGame::Cod4), [("Campagne", false), ("Multijoueur", false)]);
        assert_eq!(labels(CodGame::Waw), [("Campagne", true), ("Multijoueur", true), ("Zombies", true)]);
        assert!(CodGame::Mw2.modes().iter().all(|m| !m.plutonium));
        assert_eq!(CodGame::Mw3.launch(CodMode::Mp), Some(ModeLaunch::Plutonium { game: "iw5mp", path_key: "iw5Path" }));
        assert_eq!(labels(CodGame::Bo3), [("Black Ops III", false)]);
        assert!(matches!(CodGame::Bo3.launch(CodMode::Mp), Some(ModeLaunch::Exe { exe: "boiii.exe", .. })));
    }

    fn put(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|_| format!("absent: {}", path.display()))
    }

    /// Dossier BO3 minimal tel qu'extrait des archives : deux langues, sons, Language Switcher.
    fn bo3_dir() -> PathBuf {
        let gd = std::env::temp_dir().join(format!("farfa-bo3-{}", uuid::Uuid::new_v4()));
        let z = gd.join("zone");
        for (lang, text) in [("en", "anglais"), ("fr", "français")] {
            put(&z.join(format!("{lang}_core_pre_gfx.ff")), text);
            put(&z.join(format!("{lang}_base.xpak")), text);
            put(&z.join("snd").join(lang).join(format!("core.{lang}.sabl")), text);
        }
        put(&z.join("mp_common.ff"), "commun");
        put(&gd.join("localization.txt"), "french\r\n");
        put(&gd.join("_Language Switcher/English/localization.txt"), "english\r\n");
        put(&gd.join("_Language Switcher/French/localization.txt"), "french\r\n");
        gd
    }

    /// Toutes les zones et banques sous leur vrai nom, rien de mis de côté.
    fn assert_bo3_untouched(gd: &Path) {
        let z = gd.join("zone");
        assert_eq!(read(&z.join("en_core_pre_gfx.ff")), "anglais");
        assert_eq!(read(&z.join("fr_core_pre_gfx.ff")), "français");
        assert_eq!(read(&z.join("en_base.xpak")), "anglais");
        assert_eq!(read(&z.join("fr_base.xpak")), "français");
        assert_eq!(read(&z.join("snd/en/core.en.sabl")), "anglais");
        assert_eq!(read(&z.join("snd/fr/core.fr.sabl")), "français");
        assert_eq!(read(&z.join("mp_common.ff")), "commun");
        assert!(!bo3_kept(gd).exists());
    }

    /// Passage au français tel que le faisaient la 1.0 (`banks` faux) et la version de test
    /// du rapport 20261117-130619-1-10625 (`banks` vrai).
    fn old_french_switch(gd: &Path, banks: bool) {
        let (z, kept) = (gd.join("zone"), bo3_kept(gd));
        std::fs::create_dir_all(&kept).unwrap();
        for rest in ["core_pre_gfx.ff", "base.xpak"] {
            std::fs::rename(z.join(format!("en_{rest}")), kept.join(format!("en_{rest}"))).unwrap();
            std::fs::rename(z.join(format!("fr_{rest}")), z.join(format!("en_{rest}"))).unwrap();
        }
        std::fs::rename(z.join("snd/en"), kept.join(SND_KEPT)).unwrap();
        std::fs::rename(z.join("snd/fr"), z.join("snd/en")).unwrap();
        if banks {
            std::fs::rename(z.join("snd/en/core.fr.sabl"), z.join("snd/en/core.en.sabl")).unwrap();
        }
    }

    #[test]
    fn bo3_switches_language_back_and_forth() {
        let gd = bo3_dir();
        put(&gd.join("localization.txt"), "english\r\n");
        assert_eq!(bo3_language(&gd), "en");
        set_bo3_language(&gd, "fr").unwrap();
        assert_eq!(bo3_language(&gd), "fr");
        assert_eq!(read(&gd.join("localization.txt")), "french\r\n");
        // Le jeu choisit seul ses zones `fr_*` : aucune n'est renommée.
        assert_bo3_untouched(&gd);
        set_bo3_language(&gd, "fr").unwrap();
        assert_eq!(bo3_language(&gd), "fr");

        set_bo3_language(&gd, "en").unwrap();
        assert_eq!(bo3_language(&gd), "en");
        assert_eq!(read(&gd.join("localization.txt")), "english\r\n");
        assert_bo3_untouched(&gd);
        std::fs::remove_dir_all(gd).unwrap();
    }

    #[test]
    fn bo3_zones_renamed_by_older_versions_are_restored() {
        for banks in [false, true] {
            // Changement de langue depuis l'ancien état.
            let gd = bo3_dir();
            old_french_switch(&gd, banks);
            set_bo3_language(&gd, "fr").unwrap();
            assert_bo3_untouched(&gd);
            assert_eq!(bo3_language(&gd), "fr");
            std::fs::remove_dir_all(gd).unwrap();

            // Ou simple lancement : `restore_bo3_zones`, sans toucher à la langue.
            let gd = bo3_dir();
            old_french_switch(&gd, banks);
            restore_bo3_zones(&gd).unwrap();
            assert_bo3_untouched(&gd);
            restore_bo3_zones(&gd).unwrap();
            assert_bo3_untouched(&gd);
            std::fs::remove_dir_all(gd).unwrap();
        }
    }

    #[test]
    fn bo3_restore_survives_a_reinstall_and_an_interruption() {
        let gd = bo3_dir();
        let z = gd.join("zone");
        old_french_switch(&gd, true);
        // Réinstallation par-dessus : les archives reposent les deux langues.
        put(&z.join("en_core_pre_gfx.ff"), "anglais");
        put(&z.join("fr_core_pre_gfx.ff"), "français");
        put(&z.join("en_base.xpak"), "anglais");
        put(&z.join("fr_base.xpak"), "français");
        put(&z.join("snd/en/core.en.sabl"), "anglais");
        put(&z.join("snd/fr/core.fr.sabl"), "français");
        restore_bo3_zones(&gd).unwrap();
        assert_bo3_untouched(&gd);

        // Coupé en plein passage au français : zone anglaise rangée, française pas encore renommée.
        std::fs::create_dir_all(bo3_kept(&gd)).unwrap();
        std::fs::rename(z.join("en_base.xpak"), bo3_kept(&gd).join("en_base.xpak")).unwrap();
        restore_bo3_zones(&gd).unwrap();
        assert_bo3_untouched(&gd);
        std::fs::remove_dir_all(gd).unwrap();
    }

    #[test]
    fn bo3_flattened_install_is_repaired() {
        let gd = std::env::temp_dir().join(format!("farfa-bo3-{}", uuid::Uuid::new_v4()));
        put(&gd.join("BlackOps3.exe"), "exe");
        put(&gd.join("video/intro.mkv"), "vidéo");
        put(&gd.join("zone/mp_common.ff"), "commun");
        // Morceaux aplatis par la 0.9.7.
        put(&gd.join("en_core_pre_gfx.ff"), "anglais");
        put(&gd.join("EN_BASE.XPAK"), "anglais");
        put(&gd.join("snd/en/core.en.sabl"), "anglais");
        assert_eq!(repair_bo3_zone(&gd).unwrap(), 3);
        assert_eq!(read(&gd.join("zone/en_core_pre_gfx.ff")), "anglais");
        assert_eq!(read(&gd.join("zone/EN_BASE.XPAK")), "anglais");
        assert_eq!(read(&gd.join("zone/snd/en/core.en.sabl")), "anglais");
        assert_eq!(read(&gd.join("zone/mp_common.ff")), "commun");
        assert_eq!(read(&gd.join("video/intro.mkv")), "vidéo");
        assert!(gd.join("BlackOps3.exe").exists() && !gd.join("snd").exists());
        assert_eq!(repair_bo3_zone(&gd).unwrap(), 0);
        std::fs::remove_dir_all(gd).unwrap();
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
