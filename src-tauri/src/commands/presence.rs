//! Présence des amis : panneau « Amis » et « Apparaître hors ligne » (voir `crate::presence`).

use tauri::{AppHandle, Manager, State};

use crate::error::AppResult;
use crate::presence::{self, Friends};
use crate::state::AppState;

/// Signal toutes les 60 s tant que le launcher est ouvert (rien hors ligne ou si le joueur
/// apparaît hors ligne).
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            presence::signal(&app.state::<AppState>()).await;
            tokio::time::sleep(std::time::Duration::from_secs(presence::INTERVAL_SECS)).await;
        }
    });
}

/// Signal tout de suite (jeu lancé ou arrêté).
pub fn notify(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { presence::signal(&app.state::<AppState>()).await });
}

/// Fermeture du launcher : hors ligne tout de suite, sans retenir la fermeture plus de 2 s.
pub fn leave(app: &AppHandle) {
    let state = app.state::<AppState>();
    tauri::async_runtime::block_on(async {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), presence::leave(&state)).await;
    });
}

/// Amis et leur présence ; `None` : gestionnaire sans la présence (rien à montrer).
#[tauri::command]
pub async fn friends(state: State<'_, AppState>) -> AppResult<Option<Friends>> {
    presence::friends(&state).await
}

#[tauri::command]
pub fn presence_hidden(state: State<'_, AppState>) -> bool {
    state.settings().presence_hidden
}

#[tauri::command]
pub async fn set_presence_hidden(state: State<'_, AppState>, hidden: bool) -> AppResult<bool> {
    presence::set_hidden(&state, hidden).await?;
    Ok(hidden)
}

/// Avatar d'un ami (servi par le gestionnaire), en cache local.
#[tauri::command]
pub async fn friend_avatar(state: State<'_, AppState>, id: i64) -> AppResult<String> {
    let client = presence::manager(&state)?;
    // Une version par jour : un nouvel avatar apparaît au plus tard le lendemain.
    let day = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0);
    let path = crate::cache::cached_download(
        &client,
        &state.cache_dir.join("images"),
        &format!("ami-{id}-{day}"),
        &format!("api/presence/avatar/{id}"),
        Some("png"),
    )
    .await?;
    Ok(path.to_string_lossy().into_owned())
}
