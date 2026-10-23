//! État local des jeux installés.
//!
//! - Un fichier `farfalauncher.json` dans chaque dossier de jeu (version
//!   installée = contenu de VERSION.txt, plateforme, exécutable, dates, mods).
//! - Un registre `installed.json` (dossier de config) qui associe chaque jeu
//!   RomM à son dossier, car le dossier d'installation peut changer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{log_err, AppError, AppResult};
use crate::install::extract::STATE_FILE;
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
    /// Archives de mods téléchargées dans `mods_downloads/`.
    #[serde(default)]
    pub mods_downloaded: Vec<String>,
    /// Horodatages Unix (secondes).
    pub installed_at: u64,
    pub updated_at: u64,
    /// Dossier du jeu (non stocké dans le fichier, déduit de son emplacement).
    #[serde(skip_deserializing, default)]
    pub game_dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    games: Vec<RegistryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryEntry {
    rom_id: i64,
    game_dir: PathBuf,
}

fn registry_path(config_dir: &Path) -> PathBuf {
    config_dir.join("installed.json")
}

fn load_registry(config_dir: &Path) -> Registry {
    std::fs::read_to_string(registry_path(config_dir))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_registry(config_dir: &Path, reg: &Registry) -> AppResult<()> {
    std::fs::create_dir_all(config_dir).map_err(|e| log_err(AppError::Internal, "dossier de config", e))?;
    let text = serde_json::to_string_pretty(reg).map_err(|e| log_err(AppError::Internal, "registre", e))?;
    write_atomic(&registry_path(config_dir), &text)
}

fn write_atomic(path: &Path, text: &str) -> AppResult<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, path))
        .map_err(|e| log_err(AppError::Disk, "écriture d'un fichier d'état", e))
}

pub fn read_state(game_dir: &Path) -> Option<InstalledGame> {
    let text = std::fs::read_to_string(game_dir.join(STATE_FILE)).ok()?;
    let mut st: InstalledGame = serde_json::from_str(&text)
        .map_err(|e| tracing::warn!("état illisible dans {}: {e}", game_dir.display()))
        .ok()?;
    st.game_dir = game_dir.to_path_buf();
    Some(st)
}

/// Écrit l'état dans le dossier du jeu et l'inscrit au registre.
pub fn write_state(config_dir: &Path, st: &InstalledGame) -> AppResult<()> {
    let text = serde_json::to_string_pretty(st).map_err(|e| log_err(AppError::Internal, "état du jeu", e))?;
    write_atomic(&st.game_dir.join(STATE_FILE), &text)?;
    let mut reg = load_registry(config_dir);
    reg.games.retain(|g| g.rom_id != st.rom_id);
    reg.games.push(RegistryEntry { rom_id: st.rom_id, game_dir: st.game_dir.clone() });
    save_registry(config_dir, &reg)
}

pub fn forget(config_dir: &Path, rom_id: i64) -> AppResult<()> {
    let mut reg = load_registry(config_dir);
    reg.games.retain(|g| g.rom_id != rom_id);
    save_registry(config_dir, &reg)
}

/// Jeux installés dont le dossier et le fichier d'état existent encore.
pub fn list_installed(config_dir: &Path) -> Vec<InstalledGame> {
    load_registry(config_dir)
        .games
        .iter()
        .filter_map(|e| read_state(&e.game_dir).filter(|s| s.rom_id == e.rom_id))
        .collect()
}

pub fn find(config_dir: &Path, rom_id: i64) -> Option<InstalledGame> {
    let entry = load_registry(config_dir).games.into_iter().find(|g| g.rom_id == rom_id)?;
    read_state(&entry.game_dir).filter(|s| s.rom_id == rom_id)
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
}
