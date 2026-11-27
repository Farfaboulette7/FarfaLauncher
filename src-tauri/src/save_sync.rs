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

/// Date d'une sauvegarde du serveur, en secondes Unix. Tolérante : RFC 3339 (RomM :
/// `…+00:00`, gestionnaire : fractions de seconde), espace au lieu du `T`, et date sans
/// fuseau, lue en UTC. `None` : illisible.
pub fn parse_server_date(s: &str) -> Option<i64> {
    use time::format_description::well_known::Rfc3339;
    let s = s.trim();
    let mut iso = s.to_string();
    if iso.len() > 10 && iso.as_bytes()[10] == b' ' {
        iso.replace_range(10..11, "T");
    }
    if let Ok(d) = time::OffsetDateTime::parse(&iso, &Rfc3339) {
        return Some(d.unix_timestamp());
    }
    // Sans fuseau : UTC (les deux serveurs écrivent en UTC).
    time::OffsetDateTime::parse(&format!("{iso}Z"), &Rfc3339).ok().map(|d| d.unix_timestamp())
}

/// Date Unix lisible dans le journal (RFC 3339, UTC).
pub fn format_unix(t: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(t)
        .ok()
        .and_then(|d| d.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| t.to_string())
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
    SaveStore::of(&state.config_dir, &state.catalog(), &state.settings(), client, st)
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
    // Hors ligne : on joue avec la locale. Avant d'être envoyée, elle sera comparée au
    // serveur (`guard`) : rien n'est écrasé à l'aveugle.
    let Ok(client) = state.online_client() else {
        tracing::warn!("« {} » : serveur injoignable, sauvegardes non comparées, on joue avec la locale", st.name);
        return Ok(SaveCheck::Ready);
    };
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
        let old = saves.remove(0);
        // Copie de la locale (dossier) ou version du serveur gardée (`keep_server_copy`, zip).
        let _ = if old.is_dir() { std::fs::remove_dir_all(old) } else { std::fs::remove_file(old) };
    }
}

/// Steam Deck : version du serveur remplacée par une locale plus récente, gardée aussi sur
/// l'appareil (en plus de l'historique du serveur), dans les copies de secours du jeu.
fn keep_server_copy(st: &InstalledGame, bytes: &[u8]) {
    let backups = st.game_dir.join(INTERNAL_DIR).join("backups");
    let path = backups.join(format!("saves-{}-serveur.zip", state::now()));
    match std::fs::create_dir_all(&backups).and_then(|_| std::fs::write(&path, bytes)) {
        Ok(()) => {
            tracing::info!("« {} » : version du serveur gardée dans {}", st.name, path.display());
            prune_backups(&backups);
        }
        Err(e) => tracing::error!("« {} » : version du serveur non gardée sur l'appareil ({e}), elle reste dans l'historique", st.name),
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
    /// Changée aussi sur le serveur depuis le dernier échange : rien n'est écrasé, la locale
    /// reste sur cet appareil et la question est posée au prochain lancement (`check`).
    Conflict,
    /// Steam Deck : envoyée par-dessus une version plus ancienne d'un autre appareil, gardée
    /// (historique du serveur et copie dans `.farfalauncher/backups`).
    UploadedKeptOther,
    /// Steam Deck : celle du serveur était plus récente, elle est remise sur l'appareil (la
    /// locale est d'abord copiée dans `.farfalauncher/backups`).
    ServerNewer,
}

/// Ce que fait `after_play` d'une sauvegarde locale changée, selon le serveur.
#[derive(Debug, PartialEq)]
enum Guard {
    /// Serveur inchangé depuis le dernier échange (ou vide) : la locale part.
    Upload,
    /// Le serveur a déjà ce contenu : rien à envoyer.
    AlreadyThere,
    /// Les deux ont changé : rien n'est écrasé, question au prochain lancement.
    AskLater,
    /// Les deux ont changé, la locale est la plus récente (Deck) : elle part, l'autre est gardée.
    UploadKeepOther,
    /// Les deux ont changé, le serveur est le plus récent (Deck) : il revient sur l'appareil.
    TakeServer,
}

/// Décision du garde-fou, sans réseau ni disque (testée) :
/// - `same_date` : `updated_at` du serveur = `synced_server` (rien reçu d'ailleurs) ;
/// - empreintes du serveur, du dernier échange et de la locale ;
/// - dates (secondes Unix) de la locale et du serveur, pour le Deck (`newest_wins`) ;
/// - `during_play` : jeu encore ouvert, ses fichiers ne sont jamais remplacés.
#[allow(clippy::too_many_arguments)]
fn guard_decision(
    same_date: bool,
    remote_hash: Option<&str>,
    synced_hash: Option<&str>,
    local_hash: Option<&str>,
    local_modified: Option<i64>,
    server_modified: Option<i64>,
    newest_wins: bool,
    during_play: bool,
) -> Guard {
    if same_date || remote_hash.is_none() {
        return Guard::Upload;
    }
    if remote_hash == local_hash {
        return Guard::AlreadyThere;
    }
    // Même contenu qu'au dernier échange (date changée seulement : copie refaite…).
    if remote_hash == synced_hash {
        return Guard::Upload;
    }
    if !newest_wins {
        return Guard::AskLater;
    }
    // Date du serveur illisible : le serveur gagne (la locale est gardée en copie).
    match (local_modified, server_modified) {
        (Some(local), Some(server)) if local > server => Guard::UploadKeepOther,
        _ if during_play => Guard::AskLater,
        _ => Guard::TakeServer,
    }
}

/// Après la partie : envoie la sauvegarde si elle a changé depuis le dernier échange.
pub async fn after_play(state: &AppState, client: Option<RommClient>, rom_id: i64, started: SystemTime) -> (SaveUpload, Option<AppError>) {
    upload_changed(state, client, rom_id, started, false).await
}

/// Pendant la partie (mise en veille du Steam Deck) : envoie la sauvegarde si elle a changé,
/// avec le même garde-fou, sans jamais remplacer les fichiers du jeu ouvert.
pub async fn upload_during_play(state: &AppState, rom_id: i64, started: SystemTime) -> (SaveUpload, Option<AppError>) {
    upload_changed(state, state.client().ok(), rom_id, started, true).await
}

async fn upload_changed(state: &AppState, client: Option<RommClient>, rom_id: i64, started: SystemTime, during_play: bool) -> (SaveUpload, Option<AppError>) {
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
        } else {
            tracing::info!("« {} » : pas de dossier de sauvegarde connu, rien n'est envoyé", st.name);
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
            tracing::info!("« {} » : sauvegarde inchangée depuis le dernier échange", st.name);
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
        let store = store_of(state, client, &st);
        // Garde-fou : le serveur a-t-il reçu une autre sauvegarde depuis le dernier échange ?
        let (decision, remote) = match store.find().await? {
            None => (Guard::Upload, None),
            Some(remote) if st.saves.synced_server.as_deref() == Some(remote.updated_at.as_str()) => (Guard::Upload, None),
            Some(remote) => {
                let bytes = store.download(&remote).await?;
                let remote_hash = saves::hash_zip(&bytes)?;
                let local_modified = saves::latest_modified(&loc).map(|t| t as i64);
                let server_modified = parse_server_date(&remote.updated_at);
                let newest_wins = state.newest_wins.load(std::sync::atomic::Ordering::Relaxed);
                let d = guard_decision(false, remote_hash.as_deref(), st.saves.synced_hash.as_deref(), hash.as_deref(), local_modified, server_modified, newest_wins, during_play);
                if d != Guard::Upload && d != Guard::AlreadyThere {
                    tracing::warn!(
                        "« {} » : sauvegarde changée ici et sur le serveur depuis le dernier échange (locale : {}, serveur : {}{}, dernier échange : {}) → {d:?}",
                        st.name,
                        local_modified.map(format_unix).unwrap_or_else(|| "?".into()),
                        remote.updated_at,
                        if server_modified.is_none() { " (date illisible)" } else { "" },
                        st.saves.synced_server.as_deref().unwrap_or("jamais"),
                    );
                }
                (d, Some((remote.updated_at, bytes)))
            }
        };
        match (decision, remote) {
            (Guard::AlreadyThere, Some((updated_at, _))) => {
                st.saves.synced_hash = hash;
                st.saves.synced_server = Some(updated_at);
                st.saves.pending_upload = false;
                state::write_state(&state.config_dir, &st)?;
                tracing::info!("« {} » : le serveur a déjà cette sauvegarde", st.name);
                Ok(SaveUpload::Unchanged)
            }
            (Guard::AskLater, _) => {
                st.saves.pending_upload = false;
                state::write_state(&state.config_dir, &st)?;
                tracing::info!("« {} » : sauvegarde gardée sur cet appareil, question au prochain lancement", st.name);
                Ok(SaveUpload::Conflict)
            }
            (Guard::TakeServer, Some((updated_at, bytes))) => {
                let backups = st.game_dir.join(INTERNAL_DIR).join("backups");
                let backup = backups.join(format!("saves-{}", state::now()));
                let (loc2, bytes2) = (loc.clone(), bytes.clone());
                tokio::task::spawn_blocking(move || saves::restore(&loc2, &bytes2, &backup)).await.map_err(|_| AppError::Internal)??;
                prune_backups(&backups);
                st.saves.synced_hash = saves::hash_zip(&bytes)?;
                st.saves.synced_server = Some(updated_at);
                st.saves.pending_upload = false;
                state::write_state(&state.config_dir, &st)?;
                tracing::info!("« {} » : sauvegarde du serveur plus récente, remise sur l'appareil (la locale est dans les copies de secours)", st.name);
                Ok(SaveUpload::ServerNewer)
            }
            (decision, remote) => {
                let kept_other = decision == Guard::UploadKeepOther;
                if let (true, Some((_, bytes))) = (kept_other, &remote) {
                    keep_server_copy(&st, bytes);
                }
                let size = zip.len();
                let updated_at = store.upload(zip).await?;
                st.saves.synced_hash = hash;
                st.saves.synced_server = Some(updated_at);
                st.saves.pending_upload = false;
                state::write_state(&state.config_dir, &st)?;
                tracing::info!("« {} » : sauvegarde envoyée ({size} octets){}", st.name, if kept_other { ", l'autre version est gardée" } else { "" });
                Ok(if kept_other { SaveUpload::UploadedKeptOther } else { SaveUpload::Uploaded })
            }
        }
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

#[cfg(test)]
mod guard_tests {
    use super::{guard_decision, parse_server_date, Guard};

    #[test]
    fn reads_the_dates_of_both_servers() {
        // RomM.
        assert_eq!(parse_server_date("2026-07-16T15:53:39+00:00"), Some(1784217219));
        // Gestionnaire : fractions de seconde.
        assert_eq!(parse_server_date("2026-07-16T15:53:39.123456+00:00"), Some(1784217219));
        assert_eq!(parse_server_date("2026-07-16T15:53:39Z"), Some(1784217219));
        // Sans fuseau : UTC. Espace au lieu du T.
        assert_eq!(parse_server_date("2026-07-16T15:53:39"), Some(1784217219));
        assert_eq!(parse_server_date("2026-07-16 15:53:39.5"), Some(1784217219));
        assert_eq!(parse_server_date(" 2026-07-16T17:53:39+02:00 "), Some(1784217219));
        assert_eq!(parse_server_date("hier soir"), None);
        assert_eq!(parse_server_date(""), None);
    }

    fn d(same: bool, remote: Option<&str>, synced: Option<&str>, local: Option<&str>, lt: Option<i64>, st: Option<i64>, deck: bool, during: bool) -> Guard {
        guard_decision(same, remote, synced, local, lt, st, deck, during)
    }

    #[test]
    fn never_overwrites_a_save_sent_by_another_device() {
        // Serveur inchangé depuis le dernier échange, ou vide : la locale part.
        assert_eq!(d(true, Some("b"), Some("a"), Some("c"), None, None, false, false), Guard::Upload);
        assert_eq!(d(false, None, None, Some("c"), None, None, true, false), Guard::Upload);
        // Date changée, même contenu qu'au dernier échange : la locale part.
        assert_eq!(d(false, Some("a"), Some("a"), Some("c"), None, None, false, false), Guard::Upload);
        // Le serveur a déjà ce contenu.
        assert_eq!(d(false, Some("c"), Some("a"), Some("c"), None, None, false, false), Guard::AlreadyThere);
        // PC : les deux ont changé → rien n'est écrasé, question au prochain lancement.
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(200), Some(100), false, false), Guard::AskLater);
        // Jamais synchronisée ici (BotW) : compte comme un changement du serveur.
        assert_eq!(d(false, Some("b"), None, Some("c"), Some(200), Some(100), false, false), Guard::AskLater);
    }

    #[test]
    fn on_the_deck_the_newest_wins() {
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(200), Some(100), true, false), Guard::UploadKeepOther);
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(100), Some(200), true, false), Guard::TakeServer);
        // Égalité, date du serveur illisible ou locale sans date : le serveur gagne.
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(100), Some(100), true, false), Guard::TakeServer);
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(200), None, true, false), Guard::TakeServer);
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), None, Some(100), true, false), Guard::TakeServer);
        // Jeu encore ouvert (veille) : ses fichiers ne sont jamais remplacés.
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(100), Some(200), true, true), Guard::AskLater);
        assert_eq!(d(false, Some("b"), Some("a"), Some("c"), Some(200), Some(100), true, true), Guard::UploadKeepOther);
    }
}
