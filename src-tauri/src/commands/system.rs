//! Informations sur l'ordinateur pour la page Paramètres : système, processeur,
//! mémoire, disques, et place occupée par les jeux et le cache. La carte
//! graphique est lue côté interface (WebGL), sans lancer de programme externe.

use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use sysinfo::{Disks, System};
use tauri::State;

use crate::error::{log_err, AppError, AppResult};
use crate::install::{dir_size, state};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub os: String,
    pub kernel: Option<String>,
    pub host_name: Option<String>,
    pub cpu: Option<String>,
    pub cpu_cores: Option<usize>,
    pub cpu_threads: usize,
    pub memory_total: u64,
    pub memory_available: u64,
    /// Secondes depuis le démarrage de l'ordinateur.
    pub uptime: u64,
    pub disks: Vec<DiskInfo>,
    pub games_count: usize,
    pub games_size: u64,
    pub cache_size: u64,
    pub logs_size: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub available: u64,
    /// Disque où s'installent les nouveaux jeux.
    pub holds_games: bool,
}

#[tauri::command]
pub async fn system_info(state: State<'_, AppState>) -> AppResult<SystemInfo> {
    let install_root = state.install_root();
    let games: Vec<_> = state::list_installed(&state.config_dir).into_iter().map(|g| g.game_dir).collect();
    let (cache_dir, log_dir) = (state.cache_dir.clone(), state.log_dir.clone());
    tokio::task::spawn_blocking(move || {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let cpu = sys.cpus().first().map(|c| c.brand().trim().to_string()).filter(|b| !b.is_empty());

        let disks = Disks::new_with_refreshed_list();
        // Disque des jeux : celui dont le point de montage est le plus long préfixe.
        let games_mount = disks
            .list()
            .iter()
            .filter(|d| install_root.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
            .map(|d| d.mount_point().to_path_buf());
        let mut disk_list: Vec<DiskInfo> = disks
            .list()
            .iter()
            .filter(|d| d.total_space() > 0)
            .map(|d| DiskInfo {
                name: d.name().to_string_lossy().trim().to_string(),
                mount: d.mount_point().to_string_lossy().into_owned(),
                total: d.total_space(),
                available: d.available_space(),
                holds_games: games_mount.as_deref() == Some(d.mount_point()),
            })
            .collect();
        disk_list.sort_by(|a, b| a.mount.cmp(&b.mount));
        disk_list.dedup_by(|a, b| a.mount == b.mount);

        SystemInfo {
            os: System::long_os_version().or_else(System::name).unwrap_or_else(|| std::env::consts::OS.to_string()),
            kernel: System::kernel_version(),
            host_name: System::host_name(),
            cpu,
            cpu_cores: System::physical_core_count(),
            cpu_threads: sys.cpus().len(),
            memory_total: sys.total_memory(),
            memory_available: sys.available_memory(),
            uptime: System::uptime(),
            disks: disk_list,
            games_count: games.len(),
            games_size: games.iter().map(|g| dir_size(g)).sum(),
            cache_size: dir_size(Path::new(&cache_dir)),
            logs_size: dir_size(Path::new(&log_dir)),
        }
    })
    .await
    .map_err(|e| log_err(AppError::Internal, "informations système", e))
}

/// Temps de réponse du serveur RomM (millisecondes).
#[tauri::command]
pub async fn server_ping(state: State<'_, AppState>) -> AppResult<u64> {
    let client = state.client()?;
    let started = Instant::now();
    client.send(client.get("api/heartbeat")).await?;
    Ok(started.elapsed().as_millis() as u64)
}
