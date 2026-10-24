//! Commandes de la bibliothèque : liste des jeux, cache, médias, temps de jeu.
//!
//! Rafraîchissement en deux temps, pour rester réactif même si le serveur est
//! lent : la liste légère est renvoyée tout de suite, puis les détails de
//! chaque jeu sont chargés en arrière-plan et poussés à l'interface par
//! l'événement `library-game` (`library-done` à la fin).

use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{stream, StreamExt};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cache::{self, LibrarySnapshot};
use crate::error::{AppError, AppResult};
use crate::install::state as install_state;
use crate::platform::{self, Target};
use crate::romm::favorites;
use crate::romm::library::{self, Manual};
use crate::romm::playtime;
use crate::state::AppState;

/// Détails chargés simultanément (on ménage le serveur).
const DETAIL_CONCURRENCY: usize = 2;

#[tauri::command]
pub fn system_target() -> Target {
    platform::detect()
}

/// Bibliothèque en cache (affichage immédiat au démarrage).
#[tauri::command]
pub fn library_cached(state: State<'_, AppState>) -> AppResult<Option<LibrarySnapshot>> {
    let server = state.server_url()?;
    let mut current = state.library.lock().unwrap();
    // Déjà en mémoire (rafraîchissement en cours ou terminé) : ne pas l'écraser
    // avec un cache plus ancien, ou vide.
    if current.as_ref().is_some_and(|s| s.server_url == server) {
        return Ok(current.clone());
    }
    let snap = cache::load_library(&state.cache_dir, &server);
    *current = snap.clone();
    Ok(snap)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshStarted {
    /// `None` si un rafraîchissement déjà en cours n'a pas encore reçu la liste.
    pub snapshot: Option<LibrarySnapshot>,
    /// `false` si un rafraîchissement était déjà en cours (aucune requête relancée).
    pub started: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshDone {
    failed: Vec<String>,
}

/// Recharge la liste depuis le serveur, puis les détails en arrière-plan.
#[tauri::command]
pub async fn library_refresh(app: AppHandle, state: State<'_, AppState>) -> AppResult<RefreshStarted> {
    if state.refreshing.swap(true, Ordering::SeqCst) {
        let snapshot = state.library.lock().unwrap().clone();
        return Ok(RefreshStarted { snapshot, started: false });
    }
    match start_refresh(&app, &state).await {
        Ok(snapshot) => Ok(RefreshStarted { snapshot: Some(snapshot), started: true }),
        Err(e) => {
            state.refreshing.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

async fn start_refresh(app: &AppHandle, state: &AppState) -> AppResult<LibrarySnapshot> {
    let client = state.client()?;
    let target = platform::detect();
    let platforms = library::list_platforms(&client).await?;
    let mut games = library::list_games(&client, &platforms, target).await?;
    let ids: Vec<(i64, String)> = games.iter().map(|g| (g.id, g.folder_name.clone())).collect();
    install_state::rekey_moved(&state.config_dir, &ids);

    // En attendant les détails, on garde ceux du cache pour chaque jeu.
    if let Some(old) = state.library.lock().unwrap().as_ref() {
        for g in games.iter_mut() {
            if let Some(prev) = old.games.iter().find(|o| o.id == g.id) {
                g.keep_details_from(prev);
            }
        }
    }
    let snapshot = LibrarySnapshot {
        server_url: client.base().to_string(),
        fetched_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        platforms,
        games,
    };
    *state.library.lock().unwrap() = Some(snapshot.clone());
    tracing::info!("bibliothèque: {} jeux, chargement des détails…", snapshot.games.len());

    let ids: Vec<i64> = snapshot.games.iter().map(|g| g.id).collect();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut failed = Vec::new();
        let mut results = stream::iter(ids)
            .map(|id| {
                let client = client.clone();
                async move { (id, library::fetch_game_details(&client, id, target).await) }
            })
            .buffer_unordered(DETAIL_CONCURRENCY);
        while let Some((id, res)) = results.next().await {
            match res {
                Ok(game) => {
                    if let Some(snap) = state.library.lock().unwrap().as_mut() {
                        if let Some(slot) = snap.games.iter_mut().find(|g| g.id == id) {
                            *slot = game.clone();
                        }
                    }
                    let _ = app.emit("library-game", &game);
                }
                Err(e) => {
                    let name = state
                        .library
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|s| s.games.iter().find(|g| g.id == id).map(|g| g.name.clone()))
                        .unwrap_or_else(|| id.to_string());
                    tracing::warn!("détails de « {name} » non chargés: {}", e.code());
                    failed.push(name);
                }
            }
        }
        let snap = state.library.lock().unwrap().clone();
        if let Some(snap) = snap {
            if let Err(e) = cache::save_library(&state.cache_dir, &snap) {
                tracing::error!("cache non enregistré: {}", e.code());
            }
        }
        state.refreshing.store(false, Ordering::SeqCst);
        let _ = app.emit("library-done", RefreshDone { failed });
    });
    Ok(snapshot)
}

/// Chemin local (en cache) d'une image du serveur.
#[tauri::command]
pub async fn image_path(state: State<'_, AppState>, url: String) -> AppResult<String> {
    let client = state.client()?;
    // N'accepte que des ressources statiques du serveur configuré.
    let prefix = format!("{}/assets/", client.base());
    let Some(rest) = url.strip_prefix(&prefix) else {
        tracing::warn!("image hors serveur refusée");
        return Err(AppError::Internal);
    };
    let path = cache::cached_download(&client, &state.cache_dir.join("images"), &url, &format!("assets/{rest}"), None).await?;
    Ok(path.to_string_lossy().into_owned())
}

/// Chemin local (en cache) d'un livret PDF.
#[tauri::command]
pub async fn manual_path(state: State<'_, AppState>, rom_id: i64, manual: Manual) -> AppResult<String> {
    let client = state.client()?;
    let dir = state.cache_dir.join("manuals");
    let path = match &manual {
        Manual::Resource { url } => {
            let prefix = format!("{}/assets/", client.base());
            let rest = url.strip_prefix(&prefix).ok_or(AppError::Internal)?;
            cache::cached_download(&client, &dir, url, &format!("assets/{rest}"), Some("pdf")).await?
        }
        Manual::File { file } => {
            let key = format!("{rom_id}:{}:{}:{}", file.id, file.size, file.sha1.as_deref().unwrap_or(""));
            cache::cached_download(&client, &dir, &key, &library::file_content_path(file), Some("pdf")).await?
        }
    };
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn game_playtime(state: State<'_, AppState>, rom_id: i64) -> AppResult<Option<u64>> {
    playtime::total_ms(&state.client()?, rom_id).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoritesInfo {
    pub rom_ids: Vec<i64>,
    /// Liste du cache : serveur injoignable.
    pub offline: bool,
}

/// Favoris de l'utilisateur (collection RomM), ou les derniers connus si le
/// serveur ne répond pas.
#[tauri::command]
pub async fn favorites_list(state: State<'_, AppState>) -> AppResult<FavoritesInfo> {
    let client = state.client()?;
    match favorites::list(&client).await {
        Ok(rom_ids) => {
            cache::save_favorites(&state.cache_dir, client.base(), &rom_ids);
            Ok(FavoritesInfo { rom_ids, offline: false })
        }
        Err(e @ (AppError::ServerUnreachable | AppError::Server)) => match cache::load_favorites(&state.cache_dir, client.base()) {
            Some(rom_ids) => Ok(FavoritesInfo { rom_ids, offline: true }),
            None => Err(e),
        },
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn favorite_set(state: State<'_, AppState>, rom_id: i64, favorite: bool) -> AppResult<Vec<i64>> {
    let client = state.client()?;
    let rom_ids = favorites::set(&client, rom_id, favorite).await?;
    cache::save_favorites(&state.cache_dir, client.base(), &rom_ids);
    Ok(rom_ids)
}

#[tauri::command]
pub fn clear_cache(state: State<'_, AppState>) -> AppResult<()> {
    *state.library.lock().unwrap() = None;
    cache::clear(&state.cache_dir)
}
