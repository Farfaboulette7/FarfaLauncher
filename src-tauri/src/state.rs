//! État partagé entre les commandes Tauri.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::error::{AppError, AppResult};
use crate::romm::client::RommClient;
use crate::settings::{self, Settings};

pub struct PendingDevice {
    pub server_url: String,
    pub device_code: String,
}

pub struct AppState {
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub settings: Mutex<Settings>,
    /// Client authentifié ; `None` tant que l'utilisateur n'est pas connecté.
    pub client: Mutex<Option<RommClient>>,
    pub pending_device: Mutex<Option<PendingDevice>>,
}

impl AppState {
    pub fn new(config_dir: PathBuf, log_dir: PathBuf) -> Self {
        let settings = settings::load(&config_dir);
        // Persiste immédiatement le device_id généré au premier lancement.
        if let Err(e) = settings::save(&config_dir, &settings) {
            tracing::error!("impossible d'enregistrer les paramètres initiaux: {e}");
        }
        Self {
            config_dir,
            log_dir,
            settings: Mutex::new(settings),
            client: Mutex::new(None),
            pending_device: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> AppResult<Settings> {
        let mut guard = self.settings.lock().unwrap();
        f(&mut guard);
        settings::save(&self.config_dir, &guard)?;
        Ok(guard.clone())
    }

    pub fn server_url(&self) -> AppResult<String> {
        self.settings().server_url.ok_or(AppError::NoServer)
    }

    /// Client authentifié courant, ou `AuthInvalid` si déconnecté.
    pub fn client(&self) -> AppResult<RommClient> {
        self.client.lock().unwrap().clone().ok_or(AppError::AuthInvalid)
    }

    pub fn set_client(&self, client: Option<RommClient>) {
        *self.client.lock().unwrap() = client;
    }
}
