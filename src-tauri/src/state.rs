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
use crate::error::LockOrRecover;

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
    /// Serveur injoignable (NAS éteint…) : jeux installés jouables, envois mis en file
    /// d'attente, sans attendre la fin du délai de connexion à chaque requête.
    pub offline: AtomicBool,
    /// Steam Deck : pas de question possible, en cas de conflit de sauvegardes la plus
    /// récente l'emporte (l'autre est gardée). Le launcher, lui, demande au joueur.
    pub newest_wins: AtomicBool,
    /// Installations en cours (jeu → jeton d'annulation).
    pub installs: Mutex<HashMap<i64, CancellationToken>>,
    /// Installations en attente de leur tour ou en pause (voir `commands::install`).
    pub queue: Mutex<Vec<crate::install::QueuedInstall>>,
    /// Pause demandée pour l'installation en cours : elle retourne dans la file.
    pub pause_requested: Mutex<HashSet<i64>>,
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

/// Jeux installés sans recomp choisie : ils gardent celle de leur archive.
fn keep_installed_recomps(catalog: &mut crate::manager::Catalog, config_dir: &std::path::Path) {
    let installed = crate::install::state::list_installed(config_dir);
    catalog.keep_installed_recomps(installed.iter().map(|g| (g.rom_id, g.archive_name.as_str())));
}

impl AppState {
    pub fn new(config_dir: PathBuf, log_dir: PathBuf, cache_dir: PathBuf, default_install_root: PathBuf) -> Self {
        crate::safe_file::remove_stale_tmp(&config_dir);
        let settings = settings::load(&config_dir);
        let mut catalog = crate::manager::Catalog::load_cached(&config_dir);
        catalog.choices = settings.recomp_choice.clone();
        keep_installed_recomps(&mut catalog, &config_dir);
        let manifest = crate::runtimes::Manifest::load_cached(&config_dir).with_catalog(&catalog);
        // Persiste immédiatement le device_id généré au premier lancement. Seulement si le
        // fichier n'existe pas : un settings.json qu'on n'a pas pu lire n'est pas écrasé par les
        // valeurs par défaut (un fichier abîmé a déjà été mis de côté par `load`).
        let missing = !settings::settings_path(&config_dir).exists();
        if missing {
            if let Err(e) = settings::save(&config_dir, &settings) {
                tracing::error!("impossible d'enregistrer les paramètres initiaux: {e}");
            }
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
            offline: AtomicBool::new(false),
            newest_wins: AtomicBool::new(false),
            installs: Mutex::new(HashMap::new()),
            queue: Mutex::new(Vec::new()),
            pause_requested: Mutex::new(HashSet::new()),
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
    /// Vérifié et inscrit sous les mêmes verrous que `start_running`, pris dans le même ordre
    /// (installs, puis running, puis busy) : une désinstallation et un lancement du même jeu
    /// ne peuvent plus se croiser (L13).
    pub fn mark_busy(&self, rom_id: i64) -> AppResult<BusyGuard<'_>> {
        let installs = self.installs.lock_or_recover();
        let running = self.running.lock_or_recover();
        if running.contains(&rom_id) {
            return Err(AppError::AlreadyRunning);
        }
        if installs.contains_key(&rom_id) || !self.busy.lock_or_recover().insert(rom_id) {
            return Err(AppError::Busy);
        }
        Ok(BusyGuard { state: self, rom_id })
    }

    /// Inscrit le jeu comme lancé, s'il n'est ni en installation, ni occupé (désinstallation,
    /// déplacement…), ni déjà lancé. Mêmes verrous et même ordre que `mark_busy` (L13).
    pub fn start_running(&self, rom_id: i64) -> AppResult<()> {
        let installs = self.installs.lock_or_recover();
        let mut running = self.running.lock_or_recover();
        if installs.contains_key(&rom_id) || self.is_busy(rom_id) {
            return Err(AppError::Busy);
        }
        if !running.insert(rom_id) {
            return Err(AppError::AlreadyRunning);
        }
        Ok(())
    }

    pub fn manifest(&self) -> crate::runtimes::Manifest {
        self.manifest.lock_or_recover().clone()
    }

    /// Catalogue du gestionnaire, avec les recomps choisies par le joueur.
    pub fn catalog(&self) -> crate::manager::Catalog {
        let mut catalog = self.catalog.lock_or_recover().clone();
        catalog.choices = self.settings.lock_or_recover().recomp_choice.clone();
        keep_installed_recomps(&mut catalog, &self.config_dir);
        catalog
    }

    /// Refuse si un jeu qui partage l'installation de `rom_id` tourne ou s'installe.
    pub fn check_family_idle(&self, rom_id: i64) -> AppResult<()> {
        let mut family = self.manifest().family(rom_id);
        // Émulateur : ses jeux sont ceux installés sur son dossier.
        if let Some(st) = crate::install::state::find(&self.config_dir, rom_id) {
            family.extend(crate::install::state::pages_in(&self.config_dir, &st.game_dir));
        }
        let others: Vec<i64> = family.into_iter().filter(|id| *id != rom_id).collect();
        if others.iter().any(|id| self.running.lock_or_recover().contains(id)) {
            return Err(AppError::AlreadyRunning);
        }
        if others.iter().any(|id| self.installs.lock_or_recover().contains_key(id) || self.is_busy(*id)) {
            return Err(AppError::Busy);
        }
        Ok(())
    }

    pub fn is_busy(&self, rom_id: i64) -> bool {
        self.busy.lock_or_recover().contains(&rom_id)
    }

    pub fn install_root(&self) -> PathBuf {
        self.settings().install_dir.unwrap_or_else(|| self.default_install_root.clone())
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock_or_recover().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> AppResult<Settings> {
        let mut guard = self.settings.lock_or_recover();
        f(&mut guard);
        settings::save(&self.config_dir, &guard)?;
        Ok(guard.clone())
    }

    pub fn server_url(&self) -> AppResult<String> {
        self.settings().server_url.ok_or(AppError::NoServer)
    }

    /// Client authentifié courant, ou `AuthInvalid` si déconnecté.
    pub fn client(&self) -> AppResult<RommClient> {
        self.client.lock_or_recover().clone().ok_or(AppError::AuthInvalid)
    }

    pub fn set_client(&self, client: Option<RommClient>) {
        *self.client.lock_or_recover() = client;
    }

    /// Le serveur était injoignable au dernier essai.
    pub fn is_offline(&self) -> bool {
        self.offline.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Note l'état du serveur ; rend `true` s'il vient de changer.
    pub fn set_offline(&self, offline: bool) -> bool {
        let before = self.offline.swap(offline, std::sync::atomic::Ordering::Relaxed);
        if before != offline {
            tracing::info!("serveur {}", if offline { "injoignable : mode hors ligne" } else { "de nouveau joignable" });
        }
        before != offline
    }

    /// Client du serveur, refusé tout de suite hors ligne (pas d'attente de connexion).
    pub fn online_client(&self) -> AppResult<RommClient> {
        if self.is_offline() {
            return Err(AppError::ServerUnreachable);
        }
        self.client()
    }
}

/// Garde de [`AppState::mark_busy`] : le jeu redevient libre à sa fin.
pub struct BusyGuard<'a> {
    state: &'a AppState,
    rom_id: i64,
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.state.busy.lock_or_recover().remove(&self.rom_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // L13 : un lancement et une désinstallation du même jeu, en même temps : jamais les deux.
    #[test]
    fn launch_and_uninstall_never_both_start() {
        let dir = std::env::temp_dir().join(format!("farfa-etat-{}", uuid::Uuid::new_v4()));
        let state = AppState::new(dir.clone(), dir.join("logs"), dir.join("cache"), dir.join("Jeux"));
        for rom_id in 0..300 {
            let (launched, busy) = std::thread::scope(|s| {
                let launch = s.spawn(|| state.start_running(rom_id).is_ok());
                let uninstall = s.spawn(|| state.mark_busy(rom_id).map(std::mem::forget).is_ok());
                (launch.join().unwrap(), uninstall.join().unwrap())
            });
            assert!(launched != busy, "jeu {rom_id} : lancé {launched}, occupé {busy}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
