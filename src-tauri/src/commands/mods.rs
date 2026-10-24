//! Onglet « Mods » : téléchargement des archives de MODS/ dans
//! `<dossier du jeu>/mods_downloads/`, sans extraction ni installation
//! (l'utilisateur place lui-même le mod). Événements `mod-progress` et
//! `mod-finished`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::install::{download, extract::MODS_DIR, state};
use crate::platform;
use crate::romm::library;
use crate::state::AppState;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModProgress {
    rom_id: i64,
    file_id: i64,
    done: u64,
    total: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModFinished {
    rom_id: i64,
    file_id: i64,
    file_name: String,
    error: Option<AppError>,
}

/// Noms des archives présentes dans `mods_downloads/`.
#[tauri::command]
pub fn downloaded_mods(state: State<'_, AppState>, rom_id: i64) -> Vec<String> {
    let Some(st) = state::find(&state.config_dir, rom_id) else { return Vec::new() };
    std::fs::read_dir(st.game_dir.join(MODS_DIR))
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.ends_with(".part"))
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
pub async fn download_mod(app: AppHandle, state: State<'_, AppState>, rom_id: i64, file_id: i64) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let client = state.client()?;
    // Fichier à jour côté serveur (identifiants actuels).
    let game = library::fetch_game_details(&client, rom_id, platform::detect()).await?;
    let file = game.mods.iter().find(|m| m.id == file_id).cloned().ok_or(AppError::NotAvailable)?;
    let key = (rom_id, file_id);
    let cancel = CancellationToken::new();
    {
        let mut jobs = state.mod_downloads.lock().unwrap();
        if jobs.contains(&key) {
            return Err(AppError::Busy);
        }
        jobs.insert(key);
    }
    let dest = st.game_dir.join(MODS_DIR).join(&file.file_name);
    tracing::info!("téléchargement du mod {} pour « {} »", file.file_name, st.name);

    let emitter = Arc::new(app.clone());
    tauri::async_runtime::spawn(async move {
        let mut last = Instant::now() - Duration::from_secs(1);
        let total = file.size;
        let result = download::download(
            &client,
            &file,
            &dest,
            &cancel,
            |done| {
                if last.elapsed() >= Duration::from_millis(200) || done == total {
                    last = Instant::now();
                    let _ = emitter.emit("mod-progress", ModProgress { rom_id, file_id, done, total });
                }
            },
            || {},
        )
        .await;
        let state = app.state::<AppState>();
        state.mod_downloads.lock().unwrap().remove(&key);
        if result.is_ok() {
            if let Some(mut st) = state::find(&state.config_dir, rom_id) {
                if !st.mods_downloaded.contains(&file.file_name) {
                    st.mods_downloaded.push(file.file_name.clone());
                    let _ = state::write_state(&state.config_dir, &st);
                }
            }
        }
        let _ = app.emit("mod-finished", ModFinished { rom_id, file_id, file_name: file.file_name.clone(), error: result.err() });
    });
    Ok(())
}
