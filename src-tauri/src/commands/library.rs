//! Commandes de la bibliothèque : liste des jeux, cache, médias, temps de jeu.
//!
//! Rafraîchissement en deux temps, pour rester réactif même si le serveur est
//! lent : la liste légère est renvoyée tout de suite, puis les détails de
//! chaque jeu sont chargés en arrière-plan et poussés à l'interface par
//! l'événement `library-game` (`library-done` à la fin).

use std::collections::{HashMap, HashSet};
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
use crate::romm::library::{self, Game, Manual};
use crate::romm::playtime;
use crate::runtimes;
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
    runtimes::refresh(state).await;
    crate::manager::refresh(state).await;
    runtimes::combine(state);
    let manifest = state.manifest();
    let catalog = state.catalog();
    let aligned = crate::manager::align_versions(&state.config_dir, &catalog);
    if aligned > 0 {
        tracing::info!("{aligned} installation(s) déjà à la version du catalogue");
    }
    let recomps = catalog.recomp_rom_ids();
    let moves = catalog.moved_games();
    let moved = install_state::rekey_ids(&state.config_dir, &moves);
    if moved > 0 {
        tracing::info!("{moved} installation(s) passée(s) de portage-* à leur vraie console");
    }
    let hidden: HashSet<i64> = moves.iter().map(|(old, _)| *old).collect();
    // Chargés d'avance : les recomps et les jeux installés (filtre « Installés »), même
    // rangés dans une console qui ne se charge qu'à l'ouverture de sa liste.
    let mut eager = recomps.clone();
    eager.extend(install_state::list_installed(&state.config_dir).iter().map(|g| g.rom_id).filter(|id| *id > 0));
    let (platforms, mut games) = runtimes::list_library(&client, &manifest, &eager, &hidden, target, |ids| install_state::rekey_moved(&state.config_dir, ids)).await?;

    for g in games.iter_mut() {
        runtimes::annotate(g, &catalog, &recomps, target);
    }
    let mut platforms = platforms;
    for p in platforms.iter_mut() {
        p.has_emulator = catalog.offer(&p.slug, target).is_some_and(|o| o.available);
    }
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
        // Chaque runtime une seule fois, quel que soit son nombre de jeux.
        let mut shared = HashMap::new();
        for runtime in &manifest.runtimes {
            match runtimes::fetch_runtime(&client, &catalog, runtime, target).await {
                Ok(game) => {
                    shared.insert(runtime.rom_id, game);
                }
                Err(e) => tracing::warn!("runtime {} non chargé: {}", runtime.id, e.code()),
            }
        }
        let (shared, manifest, detail_catalog) = (&shared, &manifest, &catalog);
        let mut results = stream::iter(ids)
            .map(|id| {
                let client = client.clone();
                async move { (id, runtimes::fetch_details_with(&client, manifest, detail_catalog, shared, id, target).await) }
            })
            .buffer_unordered(DETAIL_CONCURRENCY);
        while let Some((id, res)) = results.next().await {
            match res {
                Ok(mut game) => {
                    runtimes::annotate(&mut game, &catalog, &recomps, target);
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
        crate::save_store::import_all(&state.config_dir, &catalog, &state.settings(), &client).await;
    });
    Ok(snapshot)
}

/// Chemin à demander au serveur pour une image : ressource statique (`/assets/…`) ou avatar
/// d'un compte (`/api/users/<id>/avatar`) ; `None` pour toute autre adresse.
fn server_image_path(base: &str, url: &str) -> Option<String> {
    let rest = url.strip_prefix(base)?.strip_prefix('/')?;
    if rest.starts_with("assets/") {
        return Some(rest.to_string());
    }
    let id = rest.strip_prefix("api/users/")?.split_once("/avatar")?.0;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).then(|| format!("api/users/{id}/avatar"))
}

#[cfg(test)]
mod image_tests {
    use super::server_image_path;

    #[test]
    fn only_server_images() {
        let b = "https://romm.example.org";
        assert_eq!(server_image_path(b, "https://romm.example.org/assets/romm/resources/a.png").as_deref(), Some("assets/romm/resources/a.png"));
        assert_eq!(server_image_path(b, "https://romm.example.org/api/users/3/avatar?v=2026").as_deref(), Some("api/users/3/avatar"));
        assert_eq!(server_image_path(b, "https://romm.example.org/api/users/me"), None);
        assert_eq!(server_image_path(b, "https://romm.example.org/api/users/../x/avatar"), None);
        assert_eq!(server_image_path(b, "https://ailleurs.org/assets/a.png"), None);
    }
}

/// Chemin local (en cache) d'une image du serveur.
#[tauri::command]
pub async fn image_path(state: State<'_, AppState>, url: String) -> AppResult<String> {
    let client = state.client()?;
    // N'accepte que des ressources statiques du serveur configuré, ou l'avatar d'un compte.
    let Some(path) = server_image_path(client.base(), &url) else {
        tracing::warn!("image hors serveur refusée");
        return Err(AppError::Internal);
    };
    let path = cache::cached_download(&client, &state.cache_dir.join("images"), &url, &path, None).await?;
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

/// Dernières nouveautés de la recomp ou de l'émulateur du jeu (catalogue de FarfaRomManager).
/// `kind` et `id` : le mode choisi sur la page du jeu, même pas installé.
#[tauri::command]
pub fn game_changelog(state: State<'_, AppState>, rom_id: i64, console: String, kind: Option<String>, id: Option<String>) -> Option<crate::manager::Changelog> {
    let catalog = state.catalog();
    match (kind.as_deref(), id.as_deref()) {
        (Some(k), Some(i)) => catalog.changelog_of(k, i).or_else(|| (k == "recomp").then(|| catalog.changelog(rom_id, &console, crate::platform::detect())).flatten()),
        _ => catalog.changelog(rom_id, &console, crate::platform::detect()),
    }
}

/// La recomp du jeu installé a son propre launcher (bouton « Ouvrir la recomp »).
#[tauri::command]
pub fn recomp_menu(state: State<'_, AppState>, rom_id: i64) -> bool {
    crate::install::state::find(&state.config_dir, rom_id).is_some_and(|st| state.catalog().recomp_menu(&st))
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

/// Jeux par page pour les consoles chargées à la demande.
const CONSOLE_PAGE: usize = 60;
/// Jeux au plus d'une tranche (une lettre d'une grosse console).
const CONSOLE_RANGE_MAX: usize = 1000;
const SEARCH_LIMIT: usize = 60;
const RECENT_LIMIT: usize = 24;

fn annotated(state: &AppState, mut games: Vec<Game>, me: Target) -> Vec<Game> {
    let manifest = state.manifest();
    let catalog = state.catalog();
    let recomps = catalog.recomp_rom_ids();
    games.retain(|g| !manifest.is_runtime(g.id));
    for g in games.iter_mut() {
        if let Some((runtime, def)) = manifest.page(g.id) {
            g.runtime = Some(runtimes::link(runtime, def));
        }
        runtimes::annotate(g, &catalog, &recomps, me);
    }
    games
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamesPage {
    pub games: Vec<Game>,
    pub has_more: bool,
}

/// Une page des jeux d'une console.
#[tauri::command]
pub async fn console_games(state: State<'_, AppState>, platform_ids: Vec<i64>, offset: usize) -> AppResult<GamesPage> {
    let me = platform::detect();
    let (games, has_more) = library::list_games_page(&state.client()?, &platform_ids, me, offset, CONSOLE_PAGE).await?;
    Ok(GamesPage { games: annotated(&state, games, me), has_more })
}

/// Tranche des jeux d'une console triés par nom (filtre par lettre : de la position de la
/// lettre dans l'index alphabétique à celle de la suivante).
#[tauri::command]
pub async fn console_games_range(state: State<'_, AppState>, platform_ids: Vec<i64>, offset: usize, limit: usize) -> AppResult<GamesPage> {
    let me = platform::detect();
    let limit = limit.clamp(1, CONSOLE_RANGE_MAX);
    let (games, has_more) = library::list_games_page(&state.client()?, &platform_ids, me, offset, limit).await?;
    Ok(GamesPage { games: annotated(&state, games, me), has_more })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharIndex {
    pub index: std::collections::BTreeMap<String, usize>,
    pub total: usize,
}

/// Index alphabétique d'une console (voir `library::char_index`).
#[tauri::command]
pub async fn console_char_index(state: State<'_, AppState>, platform_ids: Vec<i64>) -> AppResult<CharIndex> {
    let (index, total) = library::char_index(&state.client()?, &platform_ids).await?;
    Ok(CharIndex { index, total })
}

#[tauri::command]
pub async fn search_games(state: State<'_, AppState>, term: String) -> AppResult<Vec<Game>> {
    let me = platform::detect();
    let games = library::search_games(&state.client()?, &term, me, SEARCH_LIMIT).await?;
    Ok(annotated(&state, games, me))
}

#[tauri::command]
pub async fn recent_games(state: State<'_, AppState>) -> AppResult<Vec<Game>> {
    let me = platform::detect();
    let games = library::recent_games(&state.client()?, me, RECENT_LIMIT).await?;
    Ok(annotated(&state, games, me))
}

/// Favoris hors de la liste déjà chargée. Un jeu disparu du serveur est ignoré.
#[tauri::command]
pub async fn games_by_ids(state: State<'_, AppState>, ids: Vec<i64>) -> AppResult<Vec<Game>> {
    let me = platform::detect();
    let client = state.client()?;
    let results = stream::iter(ids).map(|id| { let client = client.clone(); async move { library::fetch_game_details(&client, id, me).await } }).buffer_unordered(DETAIL_CONCURRENCY);
    let games: Vec<Game> = results.filter_map(|r| async move { r.ok() }).collect().await;
    Ok(annotated(&state, games, me))
}

/// Fiche complète d'un jeu, à l'ouverture de sa page.
#[tauri::command]
pub async fn game_details(state: State<'_, AppState>, rom_id: i64) -> AppResult<Game> {
    let me = platform::detect();
    let game = runtimes::fetch_details(&state.client()?, &state.manifest(), &state.catalog(), rom_id, me).await?;
    Ok(annotated(&state, vec![game], me).remove(0))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerInfo {
    pub url: Option<String>,
    pub custom: bool,
}

fn manager_info_of(state: &AppState) -> ManagerInfo {
    let settings = state.settings();
    ManagerInfo { url: crate::manager::base_url(&settings), custom: settings.manager_url.is_some() }
}

#[tauri::command]
pub fn manager_info(state: State<'_, AppState>) -> ManagerInfo {
    manager_info_of(&state)
}

/// Adresse de FarfaRomManager ; vide = déduite de RomM. Le token y est envoyé :
/// mêmes règles que pour RomM (https obligatoire hors réseau local).
#[tauri::command]
pub fn set_manager_url(state: State<'_, AppState>, url: Option<String>) -> AppResult<ManagerInfo> {
    let url = match url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => Some(crate::romm::client::normalize_server_url(u)?),
        None => None,
    };
    state.update_settings(|s| s.manager_url = url)?;
    Ok(manager_info_of(&state))
}

#[tauri::command]
pub fn clear_cache(state: State<'_, AppState>) -> AppResult<()> {
    *state.library.lock().unwrap() = None;
    cache::clear(&state.cache_dir)
}
