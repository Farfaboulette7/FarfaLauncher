//! État partagé entre les commandes Tauri.

use std::path::PathBuf;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::cache::LibrarySnapshot;
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
    pub cache_dir: PathBuf,
    pub settings: Mutex<Settings>,
    /// Client authentifié ; `None` tant que l'utilisateur n'est pas connecté.
    pub client: Mutex<Option<RommClient>>,
    pub pending_device: Mutex<Option<PendingDevice>>,
    /// Bibliothèque courante (cache puis serveur).
    pub library: Mutex<Option<LibrarySnapshot>>,
    /// Un seul rafraîchissement à la fois : pas de requêtes en double.
    pub refreshing: AtomicBool,
    /// Installations en cours (jeu → jeton d'annulation).
    pub installs: Mutex<HashMap<i64, CancellationToken>>,
    /// Jeux en cours d'exécution.
    pub running: Mutex<HashSet<i64>>,
    /// Parties lancées, de leur démarrage à leur fermeture (menu en jeu).
    #[cfg(feature = "gui")]
    pub playing: Mutex<HashMap<i64, crate::ingame::Playing>>,
    /// Jeux en cours de désinstallation ou de déplacement : ni lancés, ni installés.
    pub busy: Mutex<HashSet<i64>>,
    /// Téléchargements de mods en cours (jeu, fichier).
    pub mod_downloads: Mutex<HashSet<(i64, i64)>>,
    /// Recomps partagées par plusieurs jeux (dernier manifeste connu).
    pub manifest: Mutex<crate::runtimes::Manifest>,
    /// Catalogue de FarfaRomManager (dernier connu).
    pub catalog: Mutex<crate::manager::Catalog>,
    /// Un seul jeu à la fois installe ou met à jour un runtime.
    pub runtime_lock: Arc<tokio::sync::Mutex<()>>,
    /// Dossier d'installation par défaut (`~/FarfaLauncher/Jeux`).
    pub default_install_root: PathBuf,
}

impl AppState {
    pub fn new(config_dir: PathBuf, log_dir: PathBuf, cache_dir: PathBuf, default_install_root: PathBuf) -> Self {
        let settings = settings::load(&config_dir);
        let catalog = crate::manager::Catalog::load_cached(&config_dir);
        let manifest = crate::runtimes::Manifest::load_cached(&config_dir).with_catalog(&catalog);
        // Persiste immédiatement le device_id généré au premier lancement.
        if let Err(e) = settings::save(&config_dir, &settings) {
            tracing::error!("impossible d'enregistrer les paramètres initiaux: {e}");
        }
        Self {
            config_dir,
            log_dir,
            cache_dir,
            settings: Mutex::new(settings),
            client: Mutex::new(None),
            pending_device: Mutex::new(None),
            library: Mutex::new(None),
            refreshing: AtomicBool::new(false),
            installs: Mutex::new(HashMap::new()),
            running: Mutex::new(HashSet::new()),
            #[cfg(feature = "gui")]
            playing: Mutex::new(HashMap::new()),
            busy: Mutex::new(HashSet::new()),
            mod_downloads: Mutex::new(HashSet::new()),
            manifest: Mutex::new(manifest),
            catalog: Mutex::new(catalog),
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            default_install_root,
        }
    }

    /// Marque le jeu occupé jusqu'à la fin de la garde renvoyée.
    pub fn mark_busy(&self, rom_id: i64) -> AppResult<BusyGuard<'_>> {
        if self.running.lock().unwrap().contains(&rom_id) {
            return Err(AppError::AlreadyRunning);
        }
        if self.installs.lock().unwrap().contains_key(&rom_id) || !self.busy.lock().unwrap().insert(rom_id) {
            return Err(AppError::Busy);
        }
        Ok(BusyGuard { state: self, rom_id })
    }

    pub fn manifest(&self) -> crate::runtimes::Manifest {
        self.manifest.lock().unwrap().clone()
    }

    pub fn catalog(&self) -> crate::manager::Catalog {
        self.catalog.lock().unwrap().clone()
    }

    /// Refuse si un jeu qui partage l'installation de `rom_id` tourne ou s'installe.
    pub fn check_family_idle(&self, rom_id: i64) -> AppResult<()> {
        let mut family = self.manifest().family(rom_id);
        // Émulateur : ses jeux sont ceux installés sur son dossier.
        if let Some(st) = crate::install::state::find(&self.config_dir, rom_id) {
            family.extend(crate::install::state::pages_in(&self.config_dir, &st.game_dir));
        }
        let others: Vec<i64> = family.into_iter().filter(|id| *id != rom_id).collect();
        if others.iter().any(|id| self.running.lock().unwrap().contains(id)) {
            return Err(AppError::AlreadyRunning);
        }
        if others.iter().any(|id| self.installs.lock().unwrap().contains_key(id) || self.is_busy(*id)) {
            return Err(AppError::Busy);
        }
        Ok(())
    }

    pub fn is_busy(&self, rom_id: i64) -> bool {
        self.busy.lock().unwrap().contains(&rom_id)
    }

    pub fn install_root(&self) -> PathBuf {
        self.settings().install_dir.unwrap_or_else(|| self.default_install_root.clone())
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

/// Garde de [`AppState::mark_busy`] : le jeu redevient libre à sa fin.
pub struct BusyGuard<'a> {
    state: &'a AppState,
    rom_id: i64,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.state.busy.lock().unwrap().remove(&self.rom_id);
    }
}
