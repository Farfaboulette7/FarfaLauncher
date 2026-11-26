//! Synchronisation des sauvegardes avec le serveur (moteur, sans Tauri : partagé
//! par le launcher et le programme du Steam Deck) :
//! - avant de jouer, `save_check` compare la sauvegarde locale à celle du
//!   serveur ; en cas de différence, l'utilisateur choisit (locale par défaut) ;
//! - après la partie, la sauvegarde locale est envoyée si elle a changé.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::install::state::{self, InstalledGame};
use crate::install::extract::INTERNAL_DIR;
use crate::install::rel_to_path;
use crate::romm::client::RommClient;
use crate::save_store::SaveStore;
use crate::saves::{self, SaveLocation};
use crate::state::AppState;
use crate::error::LockOrRecover;

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
    // Jeu d'émulateur : le dossier vient de FarfaRomManager (« sauvegardes: »). Deviné dans le
    // dossier de l'émulateur, il tomberait sur ses fichiers système (Switch : nand/system/save).
    if st.shared.as_ref().is_some_and(|s| s.is_emulator()) {
        return None;
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

fn store_of(state: &AppState, client: RommClient, st: &InstalledGame) -> SaveStore {
    SaveStore::of(&state.catalog(), &state.settings(), client, st)
}

fn server_unavailable(e: &AppError) -> bool {
    matches!(e, AppError::ServerUnreachable | AppError::Server | AppError::AuthInvalid)
}

pub async fn check(state: &AppState, rom_id: i64) -> AppResult<SaveCheck> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    crate::switch_saves::fill(&state.catalog(), &state.config_dir, &mut st, None);
    if crate::switch_saves::waiting(&state.catalog(), &st) {
        tracing::info!("« {} » : ID du jeu Switch pas encore connu, appris après une partie qui sauvegarde", st.name);
        return Ok(SaveCheck::Ready);
    }
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
            let on_server = match state.online_client() {
                Ok(client) => matches!(store_of(&state, client, &st).find().await, Ok(Some(_))),
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
    // Hors ligne : on joue avec la locale, envoyée au retour du serveur.
    let Ok(client) = state.online_client() else { return Ok(SaveCheck::Ready) };
    let store = store_of(&state, client, &st);
    let remote = match store.find().await {
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
        tokio::task::spawn_blocking(move || saves::hash_local(&loc)).await.map_err(|_| AppError::Internal)??
    };
    // Partie jouée hors ligne, serveur inchangé depuis : la locale est la plus récente, elle
    // partira après cette partie (son empreinte diffère de celle du dernier échange).
    if st.saves.pending_upload && st.saves.synced_server.as_deref() == Some(remote.updated_at.as_str()) {
        return Ok(SaveCheck::Ready);
    }
    let remote_hash = if st.saves.synced_server.as_deref() == Some(remote.updated_at.as_str()) {
        st.saves.synced_hash.clone()
    } else {
        saves::hash_zip(&store.download(&remote).await?)?
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
pub fn set_location(state: &AppState, rom_id: i64, dir: Option<String>, off: bool) -> AppResult<InstalledGame> {
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
            let store = store_of(state, state.client()?, &st);
            let remote = store.find().await?.ok_or(AppError::Server)?;
            let bytes = store.download(&remote).await?;
            let backups = st.game_dir.join(INTERNAL_DIR).join("backups");
            let backup = backups.join(format!("saves-{}", state::now()));
            let (loc2, bytes2) = (loc.clone(), bytes.clone());
            tokio::task::spawn_blocking(move || saves::restore(&loc2, &bytes2, &backup))
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveUpload {
    /// Pas de synchronisation pour ce jeu.
    Off,
    /// Rien n'a changé (ou aucune sauvegarde).
    Unchanged,
    Uploaded,
    Failed,
    /// Serveur injoignable : gardée sur ce PC, envoyée à son retour.
    Pending,
}

/// Après la partie : envoie la sauvegarde si elle a changé depuis le dernier échange.
pub async fn after_play(state: &AppState, client: Option<RommClient>, rom_id: i64, started: SystemTime) -> (SaveUpload, Option<AppError>) {
    let Some(mut st) = state::find(&state.config_dir, rom_id) else { return (SaveUpload::Off, None) };
    // Jeu Switch : dossier trouvé maintenant que la partie l'a créé.
    crate::switch_saves::fill(&state.catalog(), &state.config_dir, &mut st, Some(started));
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
        // Jeu d'émulateur : rien n'est deviné dans le dossier de l'émulateur (voir resolve).
        None if st.shared.as_ref().is_some_and(|s| s.is_emulator()) => None,
        None => {
            let (dir, exes, name) = (st.game_dir.clone(), exes(&st), st.name.clone());
            let loc = tokio::task::spawn_blocking(move || saves::detect_by_activity(&dir, &exes, &name, started)).await.ok().flatten();
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
        let (hash, zip) = tokio::task::spawn_blocking(move || -> AppResult<(Option<String>, Vec<u8>)> {
            let hash = saves::hash_local(&loc2)?;
            let zip = if hash.is_some() { saves::zip_local(&loc2)? } else { Vec::new() };
            Ok((hash, zip))
        })
        .await
        .map_err(|_| AppError::Internal)??;
        if hash.is_none() || hash == st.saves.synced_hash {
            if st.saves.pending_upload {
                st.saves.pending_upload = false;
                state::write_state(&state.config_dir, &st)?;
            }
            return Ok(SaveUpload::Unchanged);
        }
        if state.is_offline() {
            return Err(AppError::ServerUnreachable);
        }
        let client = client.ok_or(AppError::AuthInvalid)?;
        let size = zip.len();
        let updated_at = store_of(state, client, &st).upload(zip).await?;
        st.saves.synced_hash = hash;
        st.saves.synced_server = Some(updated_at);
        st.saves.pending_upload = false;
        state::write_state(&state.config_dir, &st)?;
        tracing::info!("« {} » : sauvegarde envoyée ({size} octets)", st.name);
        Ok(SaveUpload::Uploaded)
    }
    .await;
    match res {
        Ok(r) => (r, None),
        // Serveur injoignable : la sauvegarde attend son retour (`flush_pending`).
        Err(e) if matches!(e, AppError::ServerUnreachable | AppError::Server) => {
            tracing::warn!("« {} » : serveur injoignable, sauvegarde gardée pour plus tard ({})", st.name, e.code());
            if !st.saves.pending_upload {
                st.saves.pending_upload = true;
                if let Err(e) = state::write_state(&state.config_dir, &st) {
                    tracing::error!("sauvegarde en attente non mémorisée: {}", e.code());
                }
            }
            (SaveUpload::Pending, None)
        }
        Err(e) => {
            tracing::error!("« {} » : sauvegarde non envoyée ({})", st.name, e.code());
            (SaveUpload::Failed, Some(e))
        }
    }
}

/// Versions précédentes de la sauvegarde du jeu sur le serveur ; `None` : pas d'historique
/// (aucun gestionnaire, ou gestionnaire pas encore à jour pour les jeux gardés par RomM).
/// Recomps et jeux RomM (émulateurs, jeux PC) : même réponse, voir `save_store`.
pub async fn history(state: &AppState, rom_id: i64) -> AppResult<Option<Vec<crate::save_store::SaveVersion>>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let store = store_of(state, state.online_client()?, &st);
    store.history().await
}

/// Remet une version précédente en place : sur le serveur (l'actuelle passe d'abord dans
/// l'historique), puis sur ce PC (la locale est d'abord copiée dans les copies de secours du jeu).
pub async fn restore_version(state: &AppState, rom_id: i64, version: &str) -> AppResult<()> {
    let _busy = state.mark_busy(rom_id)?;
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let store = store_of(state, state.online_client()?, &st);
    store.restore(version).await?;
    tracing::info!("« {} » : version {version} de la sauvegarde remise en place sur le serveur", st.name);
    before_play(state, rom_id, SaveChoice::Server).await
}

/// Que faire d'une sauvegarde gardée hors ligne, selon le serveur à son retour.
#[derive(Debug, PartialEq)]
enum PendingAction {
    /// Serveur inchangé depuis le dernier échange : la locale part.
    Upload,
    /// Une autre machine a envoyé la sienne entre-temps : la question sera posée au
    /// prochain lancement (`check`), rien n'est écrasé.
    AskAtNextLaunch,
}

fn pending_action(synced_server: Option<&str>, remote_updated: Option<&str>) -> PendingAction {
    if synced_server == remote_updated {
        PendingAction::Upload
    } else {
        PendingAction::AskAtNextLaunch
    }
}

/// Envoie les sauvegardes gardées hors ligne (au retour du serveur). Rend le nombre de
/// sauvegardes encore en attente.
pub async fn flush_pending(state: &AppState) -> usize {
    let pending: Vec<InstalledGame> = state::list_installed(&state.config_dir).into_iter().filter(|st| st.saves.pending_upload).collect();
    let mut left = 0;
    for mut st in pending {
        // Jeu en cours : il enverra lui-même à la fin de la partie.
        if state.running.lock_or_recover().contains(&st.rom_id) {
            left += 1;
            continue;
        }
        let Ok(client) = state.online_client() else { return left + 1 };
        let remote = match store_of(state, client.clone(), &st).find().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("« {} » : sauvegarde en attente non envoyée ({})", st.name, e.code());
                left += 1;
                continue;
            }
        };
        match pending_action(st.saves.synced_server.as_deref(), remote.as_ref().map(|r| r.updated_at.as_str())) {
            PendingAction::Upload => {
                let (save, _) = after_play(state, Some(client), st.rom_id, SystemTime::now()).await;
                tracing::info!("« {} » : sauvegarde gardée hors ligne → {save:?}", st.name);
                if matches!(save, SaveUpload::Pending | SaveUpload::Failed) {
                    left += 1;
                }
            }
            PendingAction::AskAtNextLaunch => {
                tracing::info!("« {} » : sauvegarde changée aussi sur le serveur, question au prochain lancement", st.name);
                st.saves.pending_upload = false;
                if let Err(e) = state::write_state(&state.config_dir, &st) {
                    tracing::error!("sauvegarde en attente non mise à jour: {}", e.code());
                }
            }
        }
    }
    left
}

#[cfg(test)]
mod pending_tests {
    use super::{pending_action, PendingAction};

    #[test]
    fn uploads_only_if_the_server_did_not_change() {
        assert_eq!(pending_action(Some("2026-11-16T02:00:00Z"), Some("2026-11-16T02:00:00Z")), PendingAction::Upload);
        // Rien sur le serveur, ni avant ni maintenant : première sauvegarde du jeu.
        assert_eq!(pending_action(None, None), PendingAction::Upload);
        // Envoyée depuis le Deck pendant la nuit : on ne l'écrase pas.
        assert_eq!(pending_action(Some("2026-11-16T02:00:00Z"), Some("2026-11-16T03:00:00Z")), PendingAction::AskAtNextLaunch);
        assert_eq!(pending_action(None, Some("2026-11-16T03:00:00Z")), PendingAction::AskAtNextLaunch);
    }
}
