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
    /// Serveur des musiques et bruitages, normalisé ; `None` = déduit de
    /// `server_url` (voir `assets::derive_url`).
    pub assets_url: Option<String>,
    /// FarfaRomManager, normalisé ; `None` = déduit de `server_url`.
    pub manager_url: Option<String>,
    /// Identifiant stable de cette installation, envoyé à RomM lors de
    /// l'appairage (`client_device_identifier`).
    pub device_id: String,
    /// Fenêtre en plein écran au démarrage.
    pub fullscreen: bool,
    /// Jeux (romId) dont le joueur a coupé les réglages optimisés.
    pub optimize_off: Vec<i64>,
    /// Rapports de bug déjà lus : identifiant → date de la dernière nouvelle vue (`updatedAt`).
    pub reports_seen: std::collections::BTreeMap<String, u64>,
    /// Recomp choisie (identifiant du catalogue) pour les jeux qui en ont plusieurs : romId → recomp.
    pub recomp_choice: std::collections::BTreeMap<i64, String>,
    /// Jeux (romId) pour lesquels le joueur a choisi « Ne plus demander » après un plantage.
    pub crash_prompt_off: Vec<i64>,
    /// Limite de débit des téléchargements en Mo/s (0 : illimité).
    pub download_limit_mb: u32,
    /// « Recevoir les versions de test » : mises à jour du canal bêta.
    pub beta_channel: bool,
    /// « Apparaître hors ligne » : aucun signal de présence aux amis.
    pub presence_hidden: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server_url: None,
            install_dir: None,
            assets_url: None,
            manager_url: None,
            device_id: uuid::Uuid::new_v4().to_string(),
            fullscreen: true,
            optimize_off: Vec::new(),
            reports_seen: Default::default(),
            recomp_choice: Default::default(),
            crash_prompt_off: Vec::new(),
            download_limit_mb: 0,
            beta_channel: false,
            presence_hidden: false,
        }
    }
}

pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("settings.json")
}

/// Lit les réglages. Absent : valeurs par défaut. Illisible (JSON abîmé) : le fichier est mis
/// de côté (`settings.json.illisible-<date>`, voir `safe_file`) avant de repartir des valeurs
/// par défaut, pour qu'elles ne l'écrasent jamais.
pub fn load(config_dir: &Path) -> Settings {
    let path = settings_path(config_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            crate::safe_file::set_aside(&path, &e.to_string());
            Settings::default()
        }),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::error!("settings.json impossible à lire, valeurs par défaut: {e}");
            }
            Settings::default()
        }
    }
}

pub fn save(config_dir: &Path, settings: &Settings) -> AppResult<()> {
    std::fs::create_dir_all(config_dir)
        .map_err(|e| log_err(AppError::Internal, "création du dossier de config", e))?;
    let text = serde_json::to_string_pretty(settings)
        .map_err(|e| log_err(AppError::Internal, "sérialisation des paramètres", e))?;
    // Écriture atomique : fichier temporaire au nom unique puis renommage.
    crate::safe_file::write_atomic(&settings_path(config_dir), &text)
        .map_err(|e| log_err(AppError::Internal, "écriture de settings.json", e))
}

/// Lecture et écriture de `settings.json`.
#[cfg(test)]
mod tests {
    use super::*;

    /// Dossier de config temporaire propre à un test.
    fn temp_config(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("farfa-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Ce qui est enregistré par save se relit à l'identique par load.
    #[test]
    fn save_then_load_gives_the_same_settings() {
        let dir = temp_config("reglages-relus");
        let mut s = Settings::default();
        s.server_url = Some("https://romm.example.org".into());
        s.install_dir = Some(PathBuf::from("D:/Jeux"));
        s.fullscreen = false;
        s.optimize_off = vec![3, 7];
        s.recomp_choice.insert(7, "longshot".into());
        s.reports_seen.insert("20261117-1".into(), 42);
        s.download_limit_mb = 25;
        s.beta_channel = true;
        save(&dir, &s).unwrap();

        let back = load(&dir);
        assert_eq!(back.server_url, s.server_url);
        assert_eq!(back.install_dir, s.install_dir);
        assert_eq!(back.device_id, s.device_id);
        assert!(!back.fullscreen);
        assert_eq!(back.optimize_off, [3, 7]);
        assert_eq!(back.recomp_choice.get(&7).map(String::as_str), Some("longshot"));
        assert_eq!(back.reports_seen.get("20261117-1"), Some(&42));
        assert_eq!(back.download_limit_mb, 25);
        assert!(back.beta_channel);
        // Écriture atomique : pas de fichier temporaire qui traîne.
        assert!(!dir.join("settings.json.tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Premier lancement (pas de fichier) : valeurs par défaut, avec un identifiant d'appareil.
    #[test]
    fn missing_file_gives_defaults() {
        let dir = temp_config("reglages-absents");
        let s = load(&dir);
        assert!(s.server_url.is_none() && s.install_dir.is_none());
        assert!(s.fullscreen && !s.beta_channel && !s.presence_hidden);
        assert_eq!(s.download_limit_mb, 0);
        assert!(!s.device_id.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Fichier d'une ancienne version (champs manquants) : les champs présents sont gardés, les
    // autres prennent leur valeur par défaut ; un champ inconnu (version plus récente) est ignoré.
    #[test]
    fn old_file_keeps_its_values() {
        let dir = temp_config("reglages-anciens");
        std::fs::write(
            settings_path(&dir),
            r#"{"server_url":"https://romm.example.org","device_id":"appareil-1","fullscreen":false,"champ_du_futur":1}"#,
        )
        .unwrap();
        let s = load(&dir);
        assert_eq!(s.server_url.as_deref(), Some("https://romm.example.org"));
        assert_eq!(s.device_id, "appareil-1");
        assert!(!s.fullscreen);
        assert!(!s.beta_channel && s.recomp_choice.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Comportement actuel : fichier illisible = valeurs par défaut, sans panique (voir L3).
    #[test]
    fn unreadable_file_gives_defaults() {
        let dir = temp_config("reglages-illisibles");
        std::fs::write(settings_path(&dir), "{ tronqué").unwrap();
        let s = load(&dir);
        assert!(s.server_url.is_none() && s.fullscreen);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // save crée le dossier de config s'il n'existe pas encore.
    #[test]
    fn save_creates_the_config_folder() {
        let dir = temp_config("reglages-dossier").join("nouveau");
        save(&dir, &Settings::default()).unwrap();
        assert!(settings_path(&dir).is_file());
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    // Au démarrage, un settings.json illisible n'est pas remplacé par les valeurs par défaut :
    // son contenu (adresse du serveur, dossier des jeux…) reste dans le dossier de config.
    #[test]
    fn startup_does_not_overwrite_an_unreadable_file() {
        let dir = temp_config("reglages-ecrases");
        let broken = r#"{"server_url":"https://romm.example.org","install_dir":"D:/Jeux""#; // tronqué
        std::fs::write(settings_path(&dir), broken).unwrap();
        let _state = crate::state::AppState::new(dir.clone(), dir.join("logs"), dir.join("cache"), dir.join("Jeux"));
        let kept = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains(broken)));
        assert!(kept, "le contenu de l'ancien settings.json a été perdu");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
