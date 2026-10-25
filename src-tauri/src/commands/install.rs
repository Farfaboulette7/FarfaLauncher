//! Commandes d'installation. Une installation tourne en arrière-plan et pousse
//! `install-progress` puis `install-finished` à l'interface.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::install::{self, detect, path_to_rel, state, InstallJob, InstallOutcome};
use crate::platform;
use crate::romm::library::{self, Game};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub install_root: String,
    pub games: Vec<state::InstalledGame>,
    /// Jeux en cours d'installation.
    pub active: Vec<i64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallFinished {
    rom_id: i64,
    outcome: Option<InstallOutcome>,
    error: Option<AppError>,
}

#[tauri::command]
pub fn install_info(state: State<'_, AppState>) -> InstallInfo {
    InstallInfo {
        install_root: state.install_root().to_string_lossy().into_owned(),
        games: state::list_installed(&state.config_dir),
        active: state.installs.lock().unwrap().keys().copied().collect(),
    }
}

/// `dest` : dossier où créer celui du jeu, pour cette installation seulement
/// (`None` = dossier des jeux des paramètres).
#[tauri::command]
pub fn install_game(app: AppHandle, state: State<'_, AppState>, rom_id: i64, dest: Option<String>) -> AppResult<()> {
    let install_root = match dest.map(PathBuf::from) {
        Some(d) if d.is_absolute() => d,
        Some(_) => return Err(AppError::Internal),
        None => state.install_root(),
    };
    let client = state.client()?;
    let cancel = CancellationToken::new();
    {
        let mut jobs = state.installs.lock().unwrap();
        if state.running.lock().unwrap().contains(&rom_id) {
            return Err(AppError::AlreadyRunning);
        }
        if jobs.contains_key(&rom_id) {
            return Err(AppError::Busy);
        }
        jobs.insert(rom_id, cancel.clone());
    }
    let emitter = app.clone();
    let job = InstallJob {
        client,
        config_dir: state.config_dir.clone(),
        install_root,
        target: platform::detect(),
        cancel,
        on_progress: Arc::new(move |p| {
            let _ = emitter.emit("install-progress", p);
        }),
    };
    tauri::async_runtime::spawn(async move {
        let result = job.run(rom_id).await;
        let state = app.state::<AppState>();
        state.installs.lock().unwrap().remove(&rom_id);
        let payload = match result {
            Ok(outcome) => InstallFinished { rom_id, outcome: Some(outcome), error: None },
            Err(e) => {
                if !matches!(e, AppError::Cancelled) {
                    tracing::error!("installation du jeu {rom_id} échouée: {}", e.code());
                }
                InstallFinished { rom_id, outcome: None, error: Some(e) }
            }
        };
        let _ = app.emit("install-finished", payload);
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_install(state: State<'_, AppState>, rom_id: i64) {
    if let Some(token) = state.installs.lock().unwrap().get(&rom_id) {
        tracing::info!("annulation de l'installation du jeu {rom_id}");
        token.cancel();
    }
}

/// Candidats exécutables d'un jeu installé (pour le changer depuis la fiche).
#[tauri::command]
pub fn executable_candidates(state: State<'_, AppState>, rom_id: i64) -> AppResult<Vec<String>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    Ok(detect::find_candidates(&st.game_dir, platform::detect()).iter().map(|p| path_to_rel(p)).collect())
}

/// Choix de l'exécutable : termine une installation en attente, ou change
/// l'exécutable d'un jeu déjà installé.
#[tauri::command]
pub async fn choose_executable(state: State<'_, AppState>, rom_id: i64, executable: String) -> AppResult<state::InstalledGame> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let candidates: Vec<String> = detect::find_candidates(&st.game_dir, platform::detect()).iter().map(|p| path_to_rel(p)).collect();
    if !candidates.contains(&executable) {
        return Err(AppError::NoExecutable);
    }
    if st.executable.is_none() {
        let game = game_details(&state, rom_id).await?;
        install::finalize(&state.config_dir, st, &executable, &game).await
    } else {
        install::change_executable(&state.config_dir, st, &executable).await
    }
}

async fn game_details(state: &AppState, rom_id: i64) -> AppResult<Game> {
    let cached = state
        .library
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.games.iter().find(|g| g.id == rom_id && g.details_loaded).cloned());
    match cached {
        Some(g) => Ok(g),
        None => library::fetch_game_details(&state.client()?, rom_id, platform::detect()).await,
    }
}

#[tauri::command]
pub async fn uninstall_game(state: State<'_, AppState>, rom_id: i64, keep_saves: bool) -> AppResult<()> {
    if state.running.lock().unwrap().contains(&rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    if state.installs.lock().unwrap().contains_key(&rom_id) {
        return Err(AppError::Busy);
    }
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    install::uninstall(&state.config_dir, st, keep_saves).await
}

/// Déplace un jeu installé : `dest` est le dossier qui contiendra le sien.
#[tauri::command]
pub async fn move_game(state: State<'_, AppState>, rom_id: i64, dest: String) -> AppResult<state::InstalledGame> {
    let dest = PathBuf::from(dest);
    if !dest.is_absolute() {
        return Err(AppError::Internal);
    }
    if state.running.lock().unwrap().contains(&rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    if state.installs.lock().unwrap().contains_key(&rom_id) {
        return Err(AppError::Busy);
    }
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    install::move_game(&state.config_dir, st, dest).await
}

#[tauri::command]
pub fn open_game_folder(app: AppHandle, state: State<'_, AppState>, rom_id: i64, sub: Option<String>) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let mut dir: PathBuf = st.game_dir.clone();
    if let Some(sub) = sub.as_deref() {
        // Seul sous-dossier autorisé : celui des mods téléchargés.
        if sub != install::extract::MODS_DIR {
            return Err(AppError::Internal);
        }
        dir = dir.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| log_err(AppError::Disk, "création du dossier des mods", e))?;
    }
    crate::external::open_path(&app, &dir.to_string_lossy(), "ouverture du dossier du jeu")
}

#[tauri::command]
pub fn set_install_dir(state: State<'_, AppState>, path: Option<String>) -> AppResult<String> {
    let dir = path.map(PathBuf::from).filter(|p| p.is_absolute());
    state.update_settings(|s| s.install_dir = dir)?;
    Ok(state.install_root().to_string_lossy().into_owned())
}
