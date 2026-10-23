//! Paramètres persistants (fichier JSON dans le dossier de config de l'OS).
//! Ne contient jamais le token : celui-ci vit dans le trousseau (voir `secrets`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{log_err, AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Adresse du serveur RomM, normalisée (sans `/` final).
    pub server_url: Option<String>,
    /// Dossier d'installation des jeux ; `None` = dossier par défaut.
    pub install_dir: Option<PathBuf>,
    /// Identifiant stable de cette installation, envoyé à RomM lors de
    /// l'appairage (`client_device_identifier`).
    pub device_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: None,
            install_dir: None,
            device_id: uuid::Uuid::new_v4().to_string(),
        }
    }
}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

pub fn load(config_dir: &Path) -> Settings {
    let path = settings_path(config_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!("settings.json illisible, valeurs par défaut: {e}");
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(config_dir: &Path, settings: &Settings) -> AppResult<()> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| log_err(AppError::Internal, "création du dossier de config", e))?;
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| log_err(AppError::Internal, "sérialisation des paramètres", e))?;
    // Écriture atomique : fichier temporaire puis renommage.
    let path = settings_path(config_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| log_err(AppError::Internal, "écriture de settings.json", e))
}
