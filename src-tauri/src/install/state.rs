//! État local des jeux installés.
//!
//! - Un fichier `farfalauncher.json` dans chaque dossier de jeu (version
//!   installée = contenu de VERSION.txt, plateforme, exécutable, dates, mods).
//! - Un registre `installed.json` (dossier de config) qui associe chaque jeu
//!   RomM à son dossier, car le dossier d'installation peut changer.
//! - Un jeu peut être installé de plusieurs façons à la fois (sa recomp, un ou plusieurs
//!   émulateurs, une autre recomp) : une seule est en service (« Jouer »), les autres restent
//!   sur le disque, en réserve (`alternates`), et on bascule sans rien retélécharger.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{log_err, AppError, AppResult};
use crate::install::extract::{INTERNAL_DIR, STATE_FILE};
use crate::platform::Target;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledGame {
    pub rom_id: i64,
    pub name: String,
    /// Contenu de VERSION.txt au moment de l'installation.
    pub version: Option<String>,
    /// Contenu complet de VERSION.txt à l'installation (comparé au serveur).
    #[serde(default)]
    pub version_raw: Option<String>,
    pub target: Target,
    pub archive_name: String,
    /// Exécutable choisi, relatif au dossier du jeu (séparateurs `/`).
    /// `None` tant que l'utilisateur n'a pas choisi parmi plusieurs candidats.
    pub executable: Option<String>,
    /// Candidats proposés quand le choix est en attente.
    #[serde(default)]
    pub pending_candidates: Vec<String>,
    /// ROMs copiées à côté de l'exécutable (relatives au dossier du jeu).
    #[serde(default)]
    pub roms: Vec<String>,
    /// Fichiers de `ROM/` déjà posés, pour ne pas les retélécharger s'ils n'ont pas changé.
    #[serde(default)]
    pub rom_sources: Vec<RomSource>,
    /// Archives de mods téléchargées dans `mods_downloads/`.
    #[serde(default)]
    pub mods_downloaded: Vec<String>,
    /// Synchronisation des sauvegardes avec le serveur.
    #[serde(default)]
    pub saves: SaveSync,
    /// Vrai jeu démarré par le launcher de l'exécutable (relevé pendant une
    /// partie) : « Jouer » le lance directement. Oublié à chaque mise à jour.
    #[serde(default)]
    pub direct_launch: Option<DirectLaunch>,
    /// Passer quand même par le launcher (réglages, mise à jour du port…).
    #[serde(default)]
    pub prefer_launcher: bool,
    /// Manette de chaque joueur (case vide : aucune). Vide = ordre de branchement.
    #[serde(default)]
    pub players: Vec<Option<crate::controllers::PlayerPad>>,
    /// Horodatages Unix (secondes).
    pub installed_at: u64,
    pub updated_at: u64,
    /// Jeu de la plateforme « Ordinateur ».
    #[serde(default)]
    pub pc: Option<crate::pc::PcInstall>,
    /// Jeu d'un runtime partagé : `game_dir` est alors le dossier du runtime.
    #[serde(default)]
    pub shared: Option<SharedGame>,
    /// Dossier du jeu (non stocké dans le fichier, déduit de son emplacement).
    #[serde(skip_deserializing, default)]
    pub game_dir: PathBuf,
    /// Façons dont ce jeu est installé, celle en service comprise (rempli à la lecture,
    /// jamais écrit dans le dossier du jeu).
    #[serde(skip_deserializing, skip_serializing_if = "Vec::is_empty", default)]
    pub ways: Vec<InstallWay>,
}

/// Une façon de jouer installée : une recomp ou un émulateur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallWay {
    /// `recomp` ou `emulator`.
    pub kind: String,
    /// Identifiant de la recomp ou de l'émulateur dans le catalogue.
    pub id: String,
    pub version: Option<String>,
    /// C'est elle que « Jouer » lance.
    pub active: bool,
    /// Son dossier (celui de l'émulateur pour un émulateur).
    pub game_dir: PathBuf,
}

impl InstalledGame {
    /// Façon de jouer de cette installation (`None` : jeu PC).
    pub fn way(&self, active: bool) -> Option<InstallWay> {
        if self.pc.is_some() {
            return None;
        }
        let (kind, id) = match &self.shared {
            Some(s) if s.is_emulator() => ("emulator", s.game.clone()),
            _ => ("recomp", recomp_id_of(&self.archive_name)?),
        };
        Some(InstallWay { kind: kind.into(), id, version: self.version.clone(), active, game_dir: self.game_dir.clone() })
    }
}

/// Recomp d'une archive installée : `<id>_<Système>.zip`.
pub fn recomp_id_of(archive_name: &str) -> Option<String> {
    archive_name.rsplit_once('_').map(|(id, _)| id.to_string()).filter(|id| !id.is_empty())
}

/// Lien entre un jeu installé et le runtime qui le fait tourner. Version et
/// exécutable ne sont pas gardés ici : ils sont relus sur le runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedGame {
    pub runtime_rom_id: i64,
    /// Identifiant du jeu pour la recomp (`red`, `gold`…).
    pub game: String,
    pub runtime_name: String,
    /// Dossier de données du runtime, sous `%APPDATA%` et ses équivalents.
    pub data_dir: String,
    /// Sauvegardes de ce jeu, relatives à `data_dir`.
    pub save_dir: String,
    /// Données tirées de la ROM, relatives à `data_dir`.
    #[serde(default)]
    pub cache_dir: Option<String>,
    /// `emulator` : le programme vient de FarfaRomManager, la ROM est dans `rom_dir`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub rom_dir: Option<PathBuf>,
    /// Émulateur : dossier des sauvegardes de ce jeu et fichiers à y prendre.
    #[serde(default)]
    pub saves_dir: Option<PathBuf>,
    #[serde(default)]
    pub save_patterns: Vec<String>,
}

impl SharedGame {
    pub fn is_emulator(&self) -> bool {
        self.kind.as_deref() == Some(crate::install::emulator::KIND_EMULATOR)
    }
}

/// Un fichier de `ROM/` du serveur et ce qu'il a donné dans le dossier du jeu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RomSource {
    /// Chemin sur le serveur, relatif au dossier du jeu (`ROM/x.z64`).
    pub rel_path: String,
    pub size: u64,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub md5: Option<String>,
    /// Ce qui a été posé, relatif au dossier du jeu : la copie de la ROM, ou
    /// les dossiers/fichiers de premier niveau d'une archive `.extract.zip`.
    pub placed: Vec<String>,
}

/// Lancement direct du jeu, tel que son launcher le fait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectLaunch {
    /// Programme du jeu, relatif au dossier du jeu (séparateurs `/`).
    pub exe: String,
    /// Arguments passés par le launcher (`{GAME_DIR}` à la place du dossier du jeu).
    #[serde(default)]
    pub args: Vec<String>,
    /// Dossier de travail, relatif au dossier du jeu (`None` : celui du programme).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Variables d'environnement ajoutées ou changées par le launcher (réglages,
    /// chemins de bibliothèques…), `{GAME_DIR}` à la place du dossier du jeu.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSync {
    /// Dossier des sauvegardes (absolu). `None` : pas encore trouvé.
    pub dir: Option<PathBuf>,
    /// Fichiers à prendre dans `dir` (motifs `*`, premier niveau) ; vide = tout le dossier.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// L'utilisateur a choisi de jouer sans synchroniser.
    #[serde(default)]
    pub off: bool,
    /// Empreinte du contenu au dernier échange avec le serveur.
    #[serde(default)]
    pub synced_hash: Option<String>,
    /// Date (`updated_at`) de la sauvegarde serveur à ce moment-là.
    #[serde(default)]
    pub synced_server: Option<String>,
    /// Introuvables même après une partie : on demande à l'utilisateur.
    /// (Avant la première partie, le jeu n'a souvent encore rien créé.)
    #[serde(default)]
    pub searched_after_play: bool,
    /// Sauvegarde changée pendant que le serveur était injoignable : envoyée à son retour.
    #[serde(default)]
    pub pending_upload: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    /// Installation en service de chaque jeu.
    games: Vec<RegistryEntry>,
    /// Autres installations des mêmes jeux, gardées sur le disque.
    #[serde(default)]
    alternates: Vec<RegistryEntry>,
}

impl RegistryEntry {
    fn same_place(&self, other: &RegistryEntry) -> bool {
        self.rom_id == other.rom_id && self.game_dir == other.game_dir && self.page == other.page
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    rom_id: i64,
    game_dir: PathBuf,
    /// Jeu d'un runtime partagé : son état est dans `<dossier>/.farfalauncher/pages/`.
    #[serde(default)]
    page: bool,
}

fn registry_path(config_dir: &Path) -> PathBuf {
    config_dir.join("installed.json")
}

/// Verrou des modifications du registre : lire, changer et réécrire `installed.json` se fait
/// d'un bloc, sinon deux installations finies en même temps pourraient s'effacer l'une l'autre.
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Prend le verrou du registre (un verrou empoisonné par une panique reste utilisable : il ne
/// protège aucune donnée en mémoire).
fn lock_registry() -> std::sync::MutexGuard<'static, ()> {
    REGISTRY_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Lit le registre.
/// - Absent : registre vide.
/// - Illisible (JSON abîmé) : le fichier est mis de côté (`installed.json.illisible-<date>`,
///   voir `safe_file`) et on repart d'un registre vide : il n'est jamais écrasé.
/// - Impossible à lire (droits, disque) : erreur, pour ne pas le réécrire vide par-dessus.
fn read_registry(config_dir: &Path) -> AppResult<Registry> {
    let path = registry_path(config_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::default()),
        Err(e) => return Err(log_err(AppError::Disk, "lecture de installed.json", e)),
    };
    match serde_json::from_str(&text) {
        Ok(reg) => Ok(reg),
        Err(e) => {
            crate::safe_file::set_aside(&path, &e.to_string());
            Ok(Registry::default())
        }
    }
}

/// Lecture seule du registre : vide s'il ne peut pas être lu.
fn load_registry(config_dir: &Path) -> Registry {
    read_registry(config_dir).unwrap_or_default()
}

fn save_registry(config_dir: &Path, reg: &Registry) -> AppResult<()> {
    std::fs::create_dir_all(config_dir).map_err(|e| log_err(AppError::Internal, "dossier de config", e))?;
    let text = serde_json::to_string_pretty(reg).map_err(|e| log_err(AppError::Internal, "registre", e))?;
    write_atomic(&registry_path(config_dir), &text)
}

fn write_atomic(path: &Path, text: &str) -> AppResult<()> {
    crate::safe_file::write_atomic(path, text).map_err(|e| log_err(AppError::Disk, "écriture d'un fichier d'état", e))
}

pub fn read_state(game_dir: &Path) -> Option<InstalledGame> {
    let text = std::fs::read_to_string(game_dir.join(STATE_FILE)).ok()?;
    let mut st: InstalledGame = serde_json::from_str(&text)
        .map_err(|e| tracing::warn!("état illisible dans {}: {e}", game_dir.display()))
        .ok()?;
    st.game_dir = game_dir.to_path_buf();
    migrate_bo3(&mut st);
    Some(st)
}

/// Black Ops III installé en 0.9.6 comme une « recomp » (boiii) : c'est un jeu PC Call of Duty
/// (Proton sous Linux, client boiii du gestionnaire). Les fichiers en place restent, sans rien
/// retélécharger ; boiii est revérifié au prochain lancement.
fn migrate_bo3(st: &mut InstalledGame) {
    if st.pc.is_some() || st.shared.is_some() || st.archive_name != "call-of-duty-black-ops-3_Windows.zip" {
        return;
    }
    tracing::info!("« {} » : ancienne installation en recomp, reprise comme jeu PC Call of Duty", st.name);
    st.version = None;
    st.version_raw = None;
    st.archive_name = "bo3_game_files-01.zip".into();
    st.executable = Some("BlackOps3.exe".into());
    st.rom_sources.clear();
    st.roms.clear();
    st.direct_launch = None;
    st.pc = Some(crate::pc::PcInstall {
        kind: crate::pc::PcKind::Cod,
        os: crate::pc::PcOs::Windows,
        cod: Some(crate::pc::cod::CodGame::Bo3),
        args: Vec::new(),
        cwd: None,
        proton: None,
        steam_app_id: None,
        keys: Vec::new(),
    });
}

fn page_path(game_dir: &Path, rom_id: i64) -> PathBuf {
    game_dir.join(INTERNAL_DIR).join("pages").join(format!("{rom_id}.json"))
}

/// État d'un jeu d'un runtime : ce qui concerne le programme vient du runtime.
fn read_page(game_dir: &Path, rom_id: i64) -> Option<InstalledGame> {
    let text = std::fs::read_to_string(page_path(game_dir, rom_id)).ok()?;
    let mut st: InstalledGame = serde_json::from_str(&text)
        .map_err(|e| tracing::warn!("état illisible pour le jeu {rom_id}: {e}"))
        .ok()?;
    let runtime = read_state(game_dir)?;
    let shared = st.shared.clone()?;
    st.game_dir = game_dir.to_path_buf();
    st.version = runtime.version;
    // Jeu d'émulateur avec des archives (mods…) : sa version est celle de l'émulateur suivie
    // de leurs empreintes, gardée tant que l'émulateur n'a pas changé.
    let with_packs = matches!((&st.version_raw, &runtime.version_raw), (Some(own), Some(rt)) if own.starts_with(&format!("{rt}+")));
    if !with_packs {
        st.version_raw = runtime.version_raw;
    }
    st.target = runtime.target;
    st.archive_name = runtime.archive_name;
    st.executable = runtime.executable.clone();
    if let (Some(direct), Some(exe)) = (st.direct_launch.as_mut(), runtime.executable) {
        direct.exe = exe;
    }
    // Le dossier n'existe qu'après le premier lancement : relu à chaque fois.
    if shared.is_emulator() {
        st.saves.dir = shared.saves_dir.clone();
        st.saves.patterns = shared.save_patterns.clone();
    } else {
        st.saves.dir = crate::runtimes::data_dir(&shared.data_dir).map(|d| d.join(crate::install::rel_to_path(&shared.save_dir)));
        st.saves.patterns = Vec::new();
    }
    Some(st)
}

fn read_entry(e: &RegistryEntry) -> Option<InstalledGame> {
    if e.page {
        read_page(&e.game_dir, e.rom_id)
    } else {
        read_state(&e.game_dir).filter(|s| s.rom_id == e.rom_id)
    }
}

/// Écrit l'état dans le dossier du jeu et l'inscrit au registre.
pub fn write_state(config_dir: &Path, st: &InstalledGame) -> AppResult<()> {
    let mut stored = st.clone();
    stored.ways.clear();
    let text = serde_json::to_string_pretty(&stored).map_err(|e| log_err(AppError::Internal, "état du jeu", e))?;
    if st.shared.is_some() {
        let path = page_path(&st.game_dir, st.rom_id);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| log_err(AppError::Disk, "dossier d'état", e))?;
        }
        write_atomic(&path, &text)?;
    } else {
        write_atomic(&st.game_dir.join(STATE_FILE), &text)?;
    }
    let _verrou = lock_registry();
    let mut reg = read_registry(config_dir)?;
    let entry = RegistryEntry { rom_id: st.rom_id, game_dir: st.game_dir.clone(), page: st.shared.is_some() };
    // Installé d'une autre façon jusqu'ici : l'ancienne installation reste, en réserve.
    for old in reg.games.iter().filter(|g| g.rom_id == st.rom_id && !g.same_place(&entry)) {
        if !reg.alternates.iter().any(|a| a.same_place(old)) {
            reg.alternates.push(old.clone());
        }
    }
    reg.alternates.retain(|a| !a.same_place(&entry));
    reg.games.retain(|g| g.rom_id != st.rom_id);
    reg.games.push(entry);
    save_registry(config_dir, &reg)
}

/// Les installations en réserve d'un jeu (encore lisibles sur le disque).
pub fn alternates(config_dir: &Path, rom_id: i64) -> Vec<InstalledGame> {
    load_registry(config_dir).alternates.iter().filter(|a| a.rom_id == rom_id).filter_map(read_entry).collect()
}

/// Toutes les installations d'un jeu : celle en service d'abord.
pub fn all_installs(config_dir: &Path, rom_id: i64) -> Vec<InstalledGame> {
    find(config_dir, rom_id).into_iter().chain(alternates(config_dir, rom_id)).collect()
}

/// Met en service l'installation en réserve qui a cette façon de jouer ; celle d'avant passe
/// en réserve à son tour.
pub fn activate(config_dir: &Path, rom_id: i64, kind: &str, id: &str) -> AppResult<InstalledGame> {
    let _verrou = lock_registry();
    let mut reg = read_registry(config_dir)?;
    let pos = reg
        .alternates
        .iter()
        .position(|a| a.rom_id == rom_id && read_entry(a).and_then(|s| s.way(false)).is_some_and(|w| w.kind == kind && w.id == id))
        .ok_or(AppError::NotInstalled)?;
    let chosen = reg.alternates.remove(pos);
    let previous: Vec<RegistryEntry> = reg.games.iter().filter(|g| g.rom_id == rom_id).cloned().collect();
    reg.games.retain(|g| g.rom_id != rom_id);
    reg.alternates.extend(previous);
    reg.games.push(chosen);
    save_registry(config_dir, &reg)?;
    find(config_dir, rom_id).ok_or(AppError::NotInstalled)
}

/// Jeux installés sur le runtime du dossier `game_dir`.
pub fn pages_in(config_dir: &Path, game_dir: &Path) -> Vec<i64> {
    // Les installations en réserve comptent : l'émulateur reste tant qu'un jeu peut y revenir.
    let reg = load_registry(config_dir);
    let mut ids: Vec<i64> = reg.games.iter().chain(&reg.alternates).filter(|g| g.page && g.game_dir == game_dir).map(|g| g.rom_id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Dossier déplacé : tout ce qui y est inscrit le suit.
pub fn relocate(config_dir: &Path, old: &Path, new: &Path) -> AppResult<()> {
    let _verrou = lock_registry();
    let mut reg = read_registry(config_dir)?;
    let Registry { games, alternates } = &mut reg;
    for g in games.iter_mut().chain(alternates.iter_mut()).filter(|g| g.game_dir == old) {
        g.game_dir = new.to_path_buf();
    }
    save_registry(config_dir, &reg)
}

/// Oublie l'installation en service d'un jeu. S'il en a une autre en réserve, elle passe en
/// service.
pub fn forget(config_dir: &Path, rom_id: i64) -> AppResult<()> {
    let _verrou = lock_registry();
    let mut reg = read_registry(config_dir)?;
    for g in reg.games.iter().filter(|g| g.rom_id == rom_id && g.page) {
        let _ = std::fs::remove_file(page_path(&g.game_dir, rom_id));
    }
    reg.games.retain(|g| g.rom_id != rom_id);
    if let Some(pos) = reg.alternates.iter().position(|a| a.rom_id == rom_id && read_entry(a).is_some()) {
        let next = reg.alternates.remove(pos);
        reg.games.push(next);
    }
    save_registry(config_dir, &reg)
}

/// Jeux installés dont le dossier et le fichier d'état existent encore.
pub fn list_installed(config_dir: &Path) -> Vec<InstalledGame> {
    let reg = load_registry(config_dir);
    reg.games.iter().filter_map(read_entry).map(|st| with_ways(&reg, st)).collect()
}

pub fn find(config_dir: &Path, rom_id: i64) -> Option<InstalledGame> {
    let reg = load_registry(config_dir);
    let entry = reg.games.iter().find(|g| g.rom_id == rom_id)?;
    read_entry(entry).map(|st| with_ways(&reg, st))
}

/// Ajoute à l'installation en service les façons de jouer installées (elle comprise).
fn with_ways(reg: &Registry, mut st: InstalledGame) -> InstalledGame {
    let others = reg.alternates.iter().filter(|a| a.rom_id == st.rom_id).filter_map(read_entry).filter_map(|a| a.way(false));
    st.ways = st.way(true).into_iter().chain(others).collect();
    st
}

/// Jeux déplacés sur le serveur (ex. de `farfa-portage` vers `portage-n64`) :
/// RomM leur donne un nouvel identifiant. Une installation dont l'identifiant
/// a disparu est rattachée au jeu qui a le même nom de dossier.
/// `games` : (identifiant, nom de dossier) des jeux du serveur.
pub fn rekey_moved(config_dir: &Path, games: &[(i64, String)]) -> usize {
    let installed = list_installed(config_dir);
    let mut moved = 0;
    for mut st in installed.iter().cloned() {
        // Un émulateur (ID négatif) n'est pas une entrée de RomM.
        if st.rom_id < 0 || st.shared.is_some() || games.iter().any(|(id, _)| *id == st.rom_id) {
            continue;
        }
        let dir_name = st.game_dir.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some((new_id, _)) = games.iter().find(|(id, folder)| {
            dir_name.as_deref() == Some(safe_folder_name(folder, *id).as_str()) && !installed.iter().any(|i| i.rom_id == *id)
        }) else {
            continue;
        };
        let old_id = st.rom_id;
        st.rom_id = *new_id;
        match write_state(config_dir, &st).and_then(|_| forget(config_dir, old_id)) {
            Ok(()) => {
                tracing::info!("« {} » déplacé sur le serveur : jeu {old_id} → {new_id}", st.name);
                moved += 1;
            }
            Err(e) => tracing::error!("rattachement de « {} » impossible: {}", st.name, e.code()),
        }
    }
    moved
}

/// Jeux passés de leur fiche `portage-*` à leur vraie console : l'installation suit
/// son nouvel ID (rien n'est retéléchargé). `moves` : (ancien ID, nouvel ID).
pub fn rekey_ids(config_dir: &Path, moves: &[(i64, i64)]) -> usize {
    let installed = list_installed(config_dir);
    let mut moved = 0;
    for (old_id, new_id) in moves {
        let Some(mut st) = installed.iter().find(|i| i.rom_id == *old_id).cloned() else { continue };
        if installed.iter().any(|i| i.rom_id == *new_id) {
            continue;
        }
        st.rom_id = *new_id;
        match write_state(config_dir, &st).and_then(|_| forget(config_dir, *old_id)) {
            Ok(()) => {
                tracing::info!("« {} » passe sur sa vraie console : jeu {old_id} → {new_id}", st.name);
                moved += 1;
            }
            Err(e) => tracing::error!("rattachement de « {} » impossible: {}", st.name, e.code()),
        }
    }
    moved
}

/// Nom de dossier sûr sur tous les systèmes.
pub fn safe_folder_name(name: &str, rom_id: i64) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    let reserved = ["con", "prn", "aux", "nul", "com1", "lpt1"];
    if cleaned.is_empty() || cleaned.starts_with('.') || reserved.contains(&cleaned.to_lowercase().as_str()) {
        format!("jeu-{rom_id}")
    } else {
        cleaned
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names() {
        assert_eq!(safe_folder_name("Super Mario 64", 1), "Super Mario 64");
        assert_eq!(safe_folder_name("Zelda: Twilight Princess?", 1), "Zelda_ Twilight Princess_");
        assert_eq!(safe_folder_name("  ..  ", 7), "jeu-7");
        assert_eq!(safe_folder_name("CON", 3), "jeu-3");
        assert_eq!(safe_folder_name("Jeu.", 3), "Jeu");
    }

    #[test]
    fn moved_games_keep_their_install() {
        let root = std::env::temp_dir().join(format!("farfa-rekey-{}", uuid::Uuid::new_v4()));
        let config = root.join("config");
        let game_dir = root.join("Jeux").join("Super Mario 64");
        std::fs::create_dir_all(&game_dir).unwrap();
        let st = InstalledGame {
            ways: Vec::new(),
            rom_id: 3,
            name: "Super Mario 64".into(),
            version: None,
            version_raw: None,
            target: Target::Windows,
            archive_name: "a.zip".into(),
            executable: Some("game.exe".into()),
            pending_candidates: vec![],
            roms: vec![],
            rom_sources: vec![],
            mods_downloaded: vec![],
            saves: SaveSync::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 1,
            updated_at: 1,
            game_dir: game_dir.clone(),
            pc: None,
            shared: None,
        };
        write_state(&config, &st).unwrap();

        let games = vec![(40, "Super Mario 64".to_string()), (41, "Autre".to_string())];
        assert_eq!(rekey_moved(&config, &games), 1);
        assert!(find(&config, 3).is_none());
        assert_eq!(find(&config, 40).unwrap().executable.as_deref(), Some("game.exe"));
        // Déjà rattaché : plus rien à faire.
        assert_eq!(rekey_moved(&config, &games), 0);
        // Passé de sa fiche portage-* à sa vraie console, même si l'ancienne fiche existe encore.
        assert_eq!(rekey_ids(&config, &[(40, 19)]), 1);
        assert!(find(&config, 40).is_none());
        assert_eq!(find(&config, 19).unwrap().executable.as_deref(), Some("game.exe"));
        assert_eq!(rekey_ids(&config, &[(40, 19)]), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn installed(rom_id: i64, archive: &str, game_dir: PathBuf) -> InstalledGame {
        InstalledGame {
            ways: Vec::new(),
            rom_id,
            name: "Zelda".into(),
            version: Some("1".into()),
            version_raw: None,
            target: Target::Windows,
            archive_name: archive.into(),
            executable: Some("jeu.exe".into()),
            pending_candidates: vec![],
            roms: vec![],
            rom_sources: vec![],
            mods_downloaded: vec![],
            saves: SaveSync::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 1,
            updated_at: 1,
            game_dir,
            pc: None,
            shared: None,
        }
    }

    /// Recomp puis émulateur sur le même jeu : les deux restent, on bascule de l'une à l'autre,
    /// et la désinstallation les retire l'une après l'autre.
    #[test]
    fn recomp_and_emulator_live_side_by_side() {
        let root = std::env::temp_dir().join(format!("farfa-ways-{}", uuid::Uuid::new_v4()));
        let config = root.join("config");
        let (recomp_dir, emu_dir) = (root.join("Jeux").join("Zelda"), root.join("Jeux").join("Citron"));
        std::fs::create_dir_all(&recomp_dir).unwrap();
        std::fs::create_dir_all(&emu_dir).unwrap();
        write_state(&config, &installed(7, "zelda_Windows.zip", recomp_dir.clone())).unwrap();
        write_state(&config, &installed(-5, "citron_Windows.zip", emu_dir.clone())).unwrap();
        let mut page = installed(7, "citron_Windows.zip", emu_dir.clone());
        page.shared = Some(SharedGame {
            runtime_rom_id: -5,
            game: "citron".into(),
            runtime_name: "Citron".into(),
            data_dir: String::new(),
            save_dir: String::new(),
            cache_dir: None,
            kind: Some(crate::install::emulator::KIND_EMULATOR.into()),
            rom_dir: Some(root.join("Roms")),
            saves_dir: None,
            save_patterns: vec![],
        });
        write_state(&config, &page).unwrap();

        let ways = |c: &Path| find(c, 7).unwrap().ways.iter().map(|w| (w.kind.clone(), w.id.clone(), w.active)).collect::<Vec<_>>();
        assert_eq!(ways(&config), vec![("emulator".into(), "citron".into(), true), ("recomp".into(), "zelda".into(), false)]);
        assert_eq!(pages_in(&config, &emu_dir), vec![7]);
        // L'état gardé sur le disque ne contient pas la liste (elle est relue à chaque fois).
        assert!(!std::fs::read_to_string(recomp_dir.join(STATE_FILE)).unwrap().contains("\"ways\""));

        // Retour à la recomp, sans rien réinstaller.
        let st = activate(&config, 7, "recomp", "zelda").unwrap();
        assert_eq!(st.game_dir, recomp_dir);
        assert_eq!(ways(&config), vec![("recomp".into(), "zelda".into(), true), ("emulator".into(), "citron".into(), false)]);
        assert!(activate(&config, 7, "emulator", "eden").is_err());
        // L'émulateur reste compté : il n'est pas désinstallé tant qu'un jeu peut y revenir.
        assert_eq!(pages_in(&config, &emu_dir), vec![7]);

        // Oublier celle en service fait passer l'autre en service, puis plus rien.
        forget(&config, 7).unwrap();
        assert_eq!(ways(&config), vec![("emulator".into(), "citron".into(), true)]);
        forget(&config, 7).unwrap();
        assert!(find(&config, 7).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod bo3_tests {
    use super::*;

    #[test]
    fn bo3_recomp_install_becomes_a_cod_pc_game() {
        let dir = std::env::temp_dir().join(format!("farfa-bo3-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = InstalledGame {
            ways: Vec::new(),
            rom_id: 10625,
            name: "Call of Duty: Black Ops III".into(),
            version: Some("v3.0.0".into()),
            version_raw: None,
            target: crate::platform::Target::Windows,
            archive_name: "call-of-duty-black-ops-3_Windows.zip".into(),
            executable: Some("boiii.exe".into()),
            pending_candidates: vec![],
            roms: vec![],
            rom_sources: vec![RomSource { rel_path: "ROM/BO3-data-01.extract.zip".into(), size: 1, sha1: None, md5: None, placed: vec!["zone".into()] }],
            mods_downloaded: vec![],
            saves: Default::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 1,
            updated_at: 1,
            game_dir: dir.clone(),
            pc: None,
            shared: None,
        };
        std::fs::write(dir.join(STATE_FILE), serde_json::to_string(&old).unwrap()).unwrap();
        let st = read_state(&dir).unwrap();
        assert_eq!(st.pc.as_ref().and_then(|p| p.cod), Some(crate::pc::cod::CodGame::Bo3));
        assert_eq!(st.executable.as_deref(), Some("BlackOps3.exe"));
        assert!(st.rom_sources.is_empty() && st.version.is_none());
        // Une installation normale ne bouge pas.
        let other = InstalledGame { archive_name: "x_Windows.zip".into(), ..old };
        std::fs::write(dir.join(STATE_FILE), serde_json::to_string(&other).unwrap()).unwrap();
        assert!(read_state(&dir).unwrap().pc.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Registre `installed.json` et fichier d'état de chaque jeu (`farfalauncher.json`) : lecture,
/// écriture, compatibilité avec les fichiers des anciennes versions.
#[cfg(test)]
mod registry_tests {
    use super::*;

    /// Dossier temporaire propre à un test, avec `config/` (registre) et `Jeux/` (jeux).
    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("farfa-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("config")).unwrap();
        root
    }

    /// Un jeu installé minimal (recomp Windows) dans `game_dir`, dossier créé.
    fn game(rom_id: i64, game_dir: PathBuf) -> InstalledGame {
        std::fs::create_dir_all(&game_dir).unwrap();
        InstalledGame {
            ways: Vec::new(),
            rom_id,
            name: format!("Jeu {rom_id}"),
            version: Some("1.0".into()),
            version_raw: Some("1.0\n".into()),
            target: Target::Windows,
            archive_name: "jeu_Windows.zip".into(),
            executable: Some("bin/jeu.exe".into()),
            pending_candidates: vec![],
            roms: vec!["rom.z64".into()],
            rom_sources: vec![],
            mods_downloaded: vec![],
            saves: SaveSync::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 10,
            updated_at: 20,
            game_dir,
            pc: None,
            shared: None,
        }
    }

    /// Registre écrit à la main, au format des anciennes versions : une seule entrée.
    fn old_registry(config: &Path, rom_id: i64, game_dir: &Path) {
        let reg = format!(r#"{{"games":[{{"romId":{rom_id},"gameDir":{}}}]}}"#, serde_json::to_string(game_dir).unwrap());
        std::fs::write(config.join("installed.json"), reg).unwrap();
    }

    // Ce qui est écrit par write_state se relit à l'identique par find et list_installed.
    #[test]
    fn write_then_read_back() {
        let root = temp_root("registre-relu");
        let config = root.join("config");
        let st = game(5, root.join("Jeux").join("Zelda"));
        write_state(&config, &st).unwrap();

        let found = find(&config, 5).unwrap();
        assert_eq!(found.name, "Jeu 5");
        assert_eq!(found.version.as_deref(), Some("1.0"));
        assert_eq!(found.version_raw.as_deref(), Some("1.0\n"));
        assert_eq!(found.executable.as_deref(), Some("bin/jeu.exe"));
        assert_eq!(found.roms, ["rom.z64"]);
        assert_eq!((found.installed_at, found.updated_at), (10, 20));
        assert_eq!(found.game_dir, st.game_dir);
        assert_eq!(list_installed(&config).len(), 1);
        assert!(find(&config, 6).is_none());
        // Le registre est dans installed.json, l'état du jeu dans son dossier.
        assert!(config.join("installed.json").is_file());
        assert!(st.game_dir.join(STATE_FILE).is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Réécrire un jeu remplace son entrée au registre sans la dupliquer.
    #[test]
    fn rewriting_a_game_keeps_one_entry() {
        let root = temp_root("registre-reecrit");
        let config = root.join("config");
        let mut st = game(5, root.join("Jeux").join("Zelda"));
        write_state(&config, &st).unwrap();
        st.version = Some("2.0".into());
        write_state(&config, &st).unwrap();
        let all = list_installed(&config);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].version.as_deref(), Some("2.0"));
        std::fs::remove_dir_all(root).unwrap();
    }

    // Un jeu dont le dossier a été supprimé à la main n'est plus listé.
    #[test]
    fn deleted_game_folder_is_not_listed() {
        let root = temp_root("registre-supprime");
        let config = root.join("config");
        let st = game(5, root.join("Jeux").join("Zelda"));
        write_state(&config, &st).unwrap();
        write_state(&config, &game(6, root.join("Jeux").join("Mario"))).unwrap();
        std::fs::remove_dir_all(&st.game_dir).unwrap();
        assert!(find(&config, 5).is_none());
        assert_eq!(list_installed(&config).iter().map(|g| g.rom_id).collect::<Vec<_>>(), [6]);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Une entrée du registre qui pointe vers le dossier d'un autre jeu est ignorée.
    #[test]
    fn registry_entry_must_match_the_game_in_the_folder() {
        let root = temp_root("registre-autre-jeu");
        let config = root.join("config");
        let st = game(6, root.join("Jeux").join("Mario"));
        write_state(&config, &st).unwrap();
        old_registry(&config, 5, &st.game_dir);
        assert!(find(&config, 5).is_none());
        assert!(list_installed(&config).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Registre des anciennes versions (ni `alternates` ni `page`) : toujours lu.
    #[test]
    fn old_registry_format_is_still_read() {
        let root = temp_root("registre-ancien");
        let config = root.join("config");
        let st = game(5, root.join("Jeux").join("Zelda"));
        write_state(&config, &st).unwrap();
        old_registry(&config, 5, &st.game_dir);
        assert_eq!(find(&config, 5).unwrap().name, "Jeu 5");
        std::fs::remove_dir_all(root).unwrap();
    }

    // Fichier d'état d'une ancienne version (seulement les champs obligatoires) : lu, le
    // reste prend sa valeur par défaut, et un champ inconnu (version plus récente) est ignoré.
    #[test]
    fn old_state_file_is_still_read() {
        let root = temp_root("etat-ancien");
        let dir = root.join("Jeux").join("Zelda");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(STATE_FILE),
            r#"{"romId":5,"name":"Zelda","version":null,"target":"Linux","archiveName":"z_Linux.zip",
                "executable":"z","installedAt":1,"updatedAt":2,"champDuFutur":true}"#,
        )
        .unwrap();
        let st = read_state(&dir).unwrap();
        assert_eq!((st.rom_id, st.target, st.executable.as_deref()), (5, Target::Linux, Some("z")));
        assert!(st.roms.is_empty() && st.saves.dir.is_none() && !st.saves.pending_upload && st.pc.is_none());
        // Le dossier n'est pas stocké : c'est celui où le fichier a été lu.
        assert_eq!(st.game_dir, dir);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Fichier d'état illisible ou absent : pas de jeu (ni panique, ni valeurs inventées).
    #[test]
    fn unreadable_state_file_gives_nothing() {
        let root = temp_root("etat-illisible");
        let dir = root.join("Jeux").join("Zelda");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(STATE_FILE), "{ pas du json").unwrap();
        assert!(read_state(&dir).is_none());
        assert!(read_state(&root.join("absent")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Registre illisible = aucun jeu installé, sans panique (il est mis de côté, voir L2).
    #[test]
    fn unreadable_registry_lists_nothing() {
        let root = temp_root("registre-illisible");
        let config = root.join("config");
        write_state(&config, &game(5, root.join("Jeux").join("Zelda"))).unwrap();
        std::fs::write(config.join("installed.json"), "{ tronqué").unwrap();
        assert!(list_installed(&config).is_empty());
        assert!(find(&config, 5).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Un registre illisible n'est pas écrasé par la prochaine écriture : son contenu (les jeux
    // installés) reste quelque part dans le dossier de config.
    #[test]
    fn unreadable_registry_is_not_overwritten() {
        let root = temp_root("registre-ecrase");
        let config = root.join("config");
        let broken = r#"{"games":[{"romId":5,"gameDir":"C:/Jeux/Zelda"}"#; // fichier tronqué
        std::fs::write(config.join("installed.json"), broken).unwrap();
        write_state(&config, &game(6, root.join("Jeux").join("Mario"))).unwrap();
        let kept = std::fs::read_dir(&config)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains(broken)));
        assert!(kept, "le contenu de l'ancien installed.json a été perdu");
        std::fs::remove_dir_all(root).unwrap();
    }

    // Plusieurs installations finies en même temps : aucune ne disparaît du registre (verrou).
    #[test]
    fn simultaneous_writes_keep_every_game() {
        let root = temp_root("registre-simultane");
        let config = root.join("config");
        let threads: Vec<_> = (1..=16)
            .map(|id| {
                let (config, dir) = (config.clone(), root.join("Jeux").join(format!("Jeu{id}")));
                std::thread::spawn(move || write_state(&config, &game(id, dir)).unwrap())
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(list_installed(&config).len(), 16);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Dossier du jeu déplacé : le registre suit, le jeu est retrouvé au nouvel endroit.
    #[test]
    fn relocate_follows_the_moved_folder() {
        let root = temp_root("registre-deplace");
        let config = root.join("config");
        let old = root.join("Jeux").join("Zelda");
        write_state(&config, &game(5, old.clone())).unwrap();
        let new = root.join("Autre").join("Zelda");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::rename(&old, &new).unwrap();
        assert!(find(&config, 5).is_none());
        relocate(&config, &old, &new).unwrap();
        assert_eq!(find(&config, 5).unwrap().game_dir, new);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Le premier write_state crée le dossier de config s'il n'existe pas encore.
    #[test]
    fn first_write_creates_the_config_folder() {
        let root = temp_root("registre-premier");
        let config = root.join("config").join("nouveau");
        write_state(&config, &game(5, root.join("Jeux").join("Zelda"))).unwrap();
        assert!(find(&config, 5).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    // L'identifiant de recomp se lit dans le nom de l'archive (`<id>_<Système>.zip`).
    #[test]
    fn recomp_id_comes_from_the_archive_name() {
        assert_eq!(recomp_id_of("zelda_Windows.zip").as_deref(), Some("zelda"));
        assert_eq!(recomp_id_of("ship_of_harkinian_Linux.zip").as_deref(), Some("ship_of_harkinian"));
        assert_eq!(recomp_id_of("_Windows.zip"), None);
        assert_eq!(recomp_id_of("sanssysteme.zip"), None);
    }
}
