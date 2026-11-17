//! « Ajouter à Steam » (voir `crate::steam`) et lancement d'un jeu par `--jouer <romId>`.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::steam;

/// Jeu demandé par un raccourci Steam, pas encore pris par l'interface.
static PENDING: Mutex<Option<i64>> = Mutex::new(None);
/// Le launcher a été ouvert par un raccourci Steam pour ce jeu : il se ferme à la fin de la
/// partie (Steam compte alors le bon temps de jeu).
static OPENED_FOR: Mutex<Option<i64>> = Mutex::new(None);

/// `--jouer <romId>` dans les arguments.
pub fn play_arg(args: &[String]) -> Option<i64> {
    let i = args.iter().position(|a| a == steam::PLAY_ARG)?;
    args.get(i + 1)?.parse().ok().filter(|id| *id > 0)
}

/// Au démarrage : retient le jeu demandé par Steam.
pub fn on_start(args: &[String]) {
    if let Some(id) = play_arg(args) {
        tracing::info!("lancé par Steam pour le jeu {id}");
        *PENDING.lock().unwrap() = Some(id);
        *OPENED_FOR.lock().unwrap() = Some(id);
    }
}

/// Raccourci Steam alors que le launcher est déjà ouvert : il passe devant et lance le jeu.
pub fn on_second_instance(app: &AppHandle, args: &[String]) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if let Some(id) = play_arg(args) {
        tracing::info!("raccourci Steam : jeu {id}");
        *PENDING.lock().unwrap() = Some(id);
        let _ = app.emit("play-request", id);
    }
}

/// Fin d'une partie : fermer le launcher s'il n'a été ouvert que pour elle (et sans plantage
/// à signaler).
pub fn after_game(app: &AppHandle, rom_id: i64, crashed: bool) {
    let mut opened = OPENED_FOR.lock().unwrap();
    if *opened != Some(rom_id) {
        return;
    }
    *opened = None;
    if crashed {
        return;
    }
    tracing::info!("partie lancée depuis Steam terminée : fermeture du launcher");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Le temps de laisser l'interface recevoir la fin de partie.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        app.exit(0);
    });
}

/// Jeu à lancer demandé par Steam (une seule fois).
#[tauri::command]
pub fn take_play_request() -> Option<i64> {
    PENDING.lock().unwrap().take()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamInfo {
    /// Steam est installé (un compte a déjà été utilisé).
    pub available: bool,
    /// Le jeu a déjà son raccourci.
    pub added: bool,
}

#[tauri::command]
pub fn steam_info(rom_id: i64) -> SteamInfo {
    let available = steam::steam_root().and_then(|r| steam::user_config_dir(&r)).is_some();
    SteamInfo { available, added: available && steam::has_shortcut(rom_id) }
}

/// Image du serveur en cache local (jaquette, capture), ou rien.
async fn image(state: &State<'_, AppState>, url: Option<String>) -> Option<std::path::PathBuf> {
    let url = url?;
    crate::commands::library::image_path(state.clone(), url).await.ok().map(std::path::PathBuf::from)
}

#[tauri::command]
pub async fn add_to_steam(state: State<'_, AppState>, rom_id: i64) -> AppResult<steam::Added> {
    let config = steam::steam_root().and_then(|r| steam::user_config_dir(&r)).ok_or(AppError::NotAvailable)?;
    let installed = crate::install::state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let game = state.library.lock().unwrap().as_ref().and_then(|l| l.games.iter().find(|g| g.id == rom_id).cloned());
    let name = game.as_ref().map(|g| g.name.clone()).unwrap_or(installed.name);
    let cover = image(&state, game.as_ref().and_then(|g| g.cover_url.clone())).await;
    let shot = image(&state, game.as_ref().and_then(|g| g.screenshot_urls.first().cloned())).await;
    let exe = steam::launcher_exe()?;
    let images = steam::Images { cover: cover.as_deref(), screenshot: shot.as_deref() };
    tokio::task::block_in_place(|| steam::add_to(&config, rom_id, &name, &exe, &images))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_play_argument() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(play_arg(&a(&["farfalauncher.exe", "--jouer", "42"])), Some(42));
        assert_eq!(play_arg(&a(&["farfalauncher.exe"])), None);
        assert_eq!(play_arg(&a(&["x", "--jouer"])), None);
        assert_eq!(play_arg(&a(&["x", "--jouer", "abc"])), None);
        assert_eq!(play_arg(&a(&["x", "--jouer", "-3"])), None);
    }
}
