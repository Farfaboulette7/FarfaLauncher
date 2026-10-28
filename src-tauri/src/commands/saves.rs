//! Synchronisation des sauvegardes avec le serveur :
//! - avant de jouer, `save_check` compare la sauvegarde locale à celle du
//!   serveur ; en cas de différence, l'utilisateur choisit (locale par défaut) ;
//! - après la partie, la sauvegarde locale est envoyée si elle a changé.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::install::state::{self, InstalledGame};
use crate::install::extract::INTERNAL_DIR;
use crate::install::rel_to_path;
use crate::romm::client::RommClient;
use crate::romm::saves as server;
use crate::saves::{self, SaveLocation};
use crate::state::AppState;

/// Copies de secours gardées avant d'écraser une sauvegarde locale.
const KEEP_BACKUPS: usize = 5;

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum SaveCheck {
    /// Synchronisation désactivée pour ce jeu.
    Off,
    /// Dossier des sauvegardes introuvable : à demander à l'utilisateur.
    Unknown { suggestions: Vec<String> },
    /// Rien à demander (identiques, rien sur le serveur, ou serveur injoignable).
    Ready,
    /// Sauvegarde locale et serveur différentes.
    Conflict {
        /// Secondes Unix ; `None` s'il n'y a pas de sauvegarde locale.
        local_modified: Option<u64>,
        /// Date RFC 3339 de l'envoi sur le serveur.
        server_modified: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SaveChoice {
    /// Garder la locale (elle sera envoyée après la partie).
    Local,
    /// Remplacer la locale par celle du serveur avant de jouer.
    Server,
    /// Pas de question posée (déjà synchronisées, ou synchronisation coupée).
    Unchanged,
}

pub fn location(st: &InstalledGame) -> Option<SaveLocation> {
    st.saves.dir.clone().map(|dir| SaveLocation { dir, patterns: st.saves.patterns.clone() })
}

/// Exécutables du jeu, relatifs à son dossier : celui choisi, puis le vrai
/// jeu démarré par son launcher s'il a été relevé.
fn exes(st: &InstalledGame) -> Vec<PathBuf> {
    st.executable.iter().chain(st.direct_launch.iter().map(|d| &d.exe)).map(|e| rel_to_path(e)).collect()
}

/// Emplacement connu ou deviné (et mémorisé) des sauvegardes d'un jeu.
fn resolve(config_dir: &std::path::Path, st: &mut InstalledGame) -> Option<SaveLocation> {
    if let Some(loc) = location(st) {
        return Some(loc);
    }
    let exes = exes(st);
    if exes.is_empty() {
        return None;
    }
    let loc = match st.pc.as_ref().and_then(|p| Some((p.cod?, p.proton.is_some()))) {
        // Call of Duty : dossier du client communautaire (Plutonium…).
        Some((cod, proton)) => {
            let local = if proton {
                crate::pc::install::prefix_for(config_dir, &st.game_dir, st.rom_id).join("drive_c/users/steamuser/AppData/Local")
            } else {
                crate::pc::install::local_appdata(None)
            };
            SaveLocation { dir: cod.save_dir(&st.game_dir, &local), patterns: Vec::new() }
        }
        None => saves::detect(&st.game_dir, &exes, &st.name)?,
    };
    tracing::info!("sauvegardes de « {} » trouvées dans {} {:?}", st.name, loc.dir.display(), loc.patterns);
    st.saves.dir = Some(loc.dir.clone());
    st.saves.patterns = loc.patterns.clone();
    st.saves.searched_after_play = false;
    if let Err(e) = state::write_state(config_dir, st) {
        tracing::error!("emplacement des sauvegardes non mémorisé: {}", e.code());
    }
    Some(loc)
}

/// Synchronisation coupée sans dossier : la réactive si les sauvegardes sont
/// désormais trouvées.
fn reenable_if_found(config_dir: &std::path::Path, st: &mut InstalledGame) -> bool {
    st.saves.off = false;
    if resolve(config_dir, st).is_some() {
        tracing::info!("« {} » : sauvegardes trouvées, synchronisation réactivée", st.name);
        return true;
    }
    st.saves.off = true;
    false
}

fn server_unavailable(e: &AppError) -> bool {
    matches!(e, AppError::ServerUnreachable | AppError::Server | AppError::AuthInvalid)
}

#[tauri::command]
pub async fn save_check(state: State<'_, AppState>, rom_id: i64) -> AppResult<SaveCheck> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    if st.saves.off {
        // Coupée faute de dossier trouvé : on retente, la détection a pu
        // progresser (ou le jeu a créé ses sauvegardes depuis).
        if st.saves.dir.is_some() || !reenable_if_found(&state.config_dir, &mut st) {
            return Ok(SaveCheck::Off);
        }
    }
    let Some(loc) = resolve(&state.config_dir, &mut st) else {
        // Jamais cherchées après une partie : le jeu les crée souvent au premier
        // lancement. On joue, et on cherche à nouveau à la sortie — sauf s'il
        // y a une sauvegarde sur le serveur à poser avant de jouer.
        if !st.saves.searched_after_play {
            let on_server = match state.client() {
                Ok(client) => matches!(server::find(&client, rom_id).await, Ok(Some(_))),
                Err(_) => false,
            };
            if !on_server {
                tracing::info!("« {} » : sauvegardes pas encore trouvées, nouvelle recherche après la partie", st.name);
                return Ok(SaveCheck::Ready);
            }
        }
        let suggestions = saves::suggestions(&st.game_dir, &exes(&st), &st.name).iter().map(|p| p.to_string_lossy().into_owned()).collect();
        return Ok(SaveCheck::Unknown { suggestions });
    };
    let Ok(client) = state.client() else { return Ok(SaveCheck::Ready) };
    let remote = match server::find(&client, rom_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(SaveCheck::Ready),
        Err(e) if server_unavailable(&e) => {
            tracing::warn!("sauvegardes: serveur indisponible ({}), on joue avec la locale", e.code());
            return Ok(SaveCheck::Ready);
        }
        Err(e) => return Err(e),
    };
    let local_hash = {
        let loc = loc.clone();
        tauri::async_runtime::spawn_blocking(move || saves::hash_local(&loc)).await.map_err(|_| AppError::Internal)??
    };
    let remote_hash = if st.saves.synced_server.as_deref() == Some(remote.updated_at.as_str()) {
        st.saves.synced_hash.clone()
    } else {
        saves::hash_zip(&server::download(&client, &remote).await?)?
    };
    if local_hash == remote_hash {
        st.saves.synced_hash = local_hash;
        st.saves.synced_server = Some(remote.updated_at);
        state::write_state(&state.config_dir, &st)?;
        return Ok(SaveCheck::Ready);
    }
    tracing::info!("« {} » : sauvegarde locale différente de celle du serveur", st.name);
    Ok(SaveCheck::Conflict { local_modified: saves::latest_modified(&loc), server_modified: remote.updated_at })
}

/// Choix du dossier des sauvegardes par l'utilisateur (`dir = None` +
/// `off = true` : jouer sans synchroniser ; `dir = None` + `off = false` :
/// redemander / redeviner).
#[tauri::command]
pub fn set_save_location(state: State<'_, AppState>, rom_id: i64, dir: Option<String>, off: bool) -> AppResult<InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    if let Some(d) = &dir {
        let path = PathBuf::from(d);
        saves::check_choice(&path, &st.game_dir)?;
        saves::collect(&SaveLocation { dir: path, patterns: Vec::new() })?;
    }
    st.saves.dir = dir.map(PathBuf::from);
    st.saves.patterns = Vec::new();
    st.saves.off = off;
    st.saves.synced_hash = None;
    st.saves.synced_server = None;
    st.saves.searched_after_play = false;
    state::write_state(&state.config_dir, &st)?;
    tracing::info!("sauvegardes de « {} » : {:?} (désactivé: {off})", st.name, st.saves.dir);
    Ok(st)
}

#[tauri::command]
pub fn open_save_folder(app: tauri::AppHandle, state: State<'_, AppState>, rom_id: i64) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let dir = st.saves.dir.filter(|d| d.is_dir()).ok_or(AppError::SaveDirInvalid)?;
    crate::external::open_path(&app, &dir.to_string_lossy(), "ouverture du dossier des sauvegardes")
}

/// Avant de lancer le jeu : applique le choix de l'utilisateur.
pub async fn before_play(state: &AppState, rom_id: i64, choice: SaveChoice) -> AppResult<()> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let Some(loc) = location(&st).filter(|_| !st.saves.off) else { return Ok(()) };
    match choice {
        SaveChoice::Unchanged => {}
        // La locale l'emporte : elle sera envoyée après la partie quoi qu'il arrive.
        SaveChoice::Local => {
            st.saves.synced_hash = None;
            state::write_state(&state.config_dir, &st)?;
        }
        SaveChoice::Server => {
            let client = state.client()?;
            let remote = server::find(&client, rom_id).await?.ok_or(AppError::Server)?;
            let bytes = server::download(&client, &remote).await?;
            let backups = st.game_dir.join(INTERNAL_DIR).join("backups");
            let backup = backups.join(format!("saves-{}", state::now()));
            let (loc2, bytes2) = (loc.clone(), bytes.clone());
            tauri::async_runtime::spawn_blocking(move || saves::restore(&loc2, &bytes2, &backup))
                .await
                .map_err(|_| AppError::Internal)??;
            prune_backups(&backups);
            st.saves.synced_hash = saves::hash_zip(&bytes)?;
            st.saves.synced_server = Some(remote.updated_at);
            state::write_state(&state.config_dir, &st)?;
            tracing::info!("« {} » : sauvegarde du serveur restaurée", st.name);
        }
    }
    Ok(())
}

fn prune_backups(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut saves: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("saves-")))
        .collect();
    saves.sort();
    while saves.len() > KEEP_BACKUPS {
        let _ = std::fs::remove_dir_all(saves.remove(0));
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveUpload {
    /// Pas de synchronisation pour ce jeu.
    Off,
    /// Rien n'a changé (ou aucune sauvegarde).
    Unchanged,
    Uploaded,
    Failed,
}

/// Après la partie : envoie la sauvegarde si elle a changé depuis le dernier échange.
pub async fn after_play(state: &AppState, client: Option<RommClient>, rom_id: i64, started: SystemTime) -> (SaveUpload, Option<AppError>) {
    let Some(mut st) = state::find(&state.config_dir, rom_id) else { return (SaveUpload::Off, None) };
    // Coupée faute de dossier (pas par choix d'un dossier) : on cherche quand même.
    if st.saves.off && st.saves.dir.is_some() {
        return (SaveUpload::Off, None);
    }
    // Pas encore trouvées : le jeu vient peut-être de les créer, sous un nom
    // reconnaissable ou non (fichiers écrits pendant la partie).
    let found = match resolve(&state.config_dir, &mut st) {
        Some(loc) => {
            if st.saves.off {
                st.saves.off = false;
                tracing::info!("« {} » : sauvegardes trouvées, synchronisation réactivée", st.name);
                if let Err(e) = state::write_state(&state.config_dir, &st) {
                    tracing::error!("synchronisation non réactivée: {}", e.code());
                }
            }
            Some(loc)
        }
        None => {
            let (dir, exes, name) = (st.game_dir.clone(), exes(&st), st.name.clone());
            let loc = tauri::async_runtime::spawn_blocking(move || saves::detect_by_activity(&dir, &exes, &name, started)).await.ok().flatten();
            if let Some(loc) = &loc {
                tracing::info!("« {} » : sauvegardes repérées pendant la partie dans {} {:?}", st.name, loc.dir.display(), loc.patterns);
                st.saves.dir = Some(loc.dir.clone());
                st.saves.patterns = loc.patterns.clone();
                st.saves.off = false;
                st.saves.searched_after_play = false;
                if let Err(e) = state::write_state(&state.config_dir, &st) {
                    tracing::error!("emplacement des sauvegardes non mémorisé: {}", e.code());
                }
            }
            loc
        }
    };
    let Some(loc) = found else {
        if !st.saves.off && !st.saves.searched_after_play {
            st.saves.searched_after_play = true;
            if let Err(e) = state::write_state(&state.config_dir, &st) {
                tracing::error!("recherche des sauvegardes non mémorisée: {}", e.code());
            }
            tracing::info!("« {} » : sauvegardes toujours introuvables après la partie", st.name);
        }
        return (SaveUpload::Off, None);
    };
    let res: AppResult<SaveUpload> = async {
        let loc2 = loc.clone();
        let (hash, zip) = tauri::async_runtime::spawn_blocking(move || -> AppResult<(Option<String>, Vec<u8>)> {
            let hash = saves::hash_local(&loc2)?;
            let zip = if hash.is_some() { saves::zip_local(&loc2)? } else { Vec::new() };
            Ok((hash, zip))
        })
        .await
        .map_err(|_| AppError::Internal)??;
        if hash.is_none() || hash == st.saves.synced_hash {
            return Ok(SaveUpload::Unchanged);
        }
        let client = client.ok_or(AppError::AuthInvalid)?;
        let size = zip.len();
        let uploaded = server::upload(&client, rom_id, zip).await?;
        st.saves.synced_hash = hash;
        st.saves.synced_server = Some(uploaded.updated_at);
        state::write_state(&state.config_dir, &st)?;
        tracing::info!("« {} » : sauvegarde envoyée ({size} octets)", st.name);
        Ok(SaveUpload::Uploaded)
    }
    .await;
    match res {
        Ok(r) => (r, None),
        Err(e) => {
            tracing::error!("« {} » : sauvegarde non envoyée ({})", st.name, e.code());
            (SaveUpload::Failed, Some(e))
        }
    }
}
