mod cache;
mod commands;
mod error;
mod install;
mod launch;
mod logging;
mod platform;
mod romm;
mod secrets;
mod settings;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = app.path();
            let config_dir = paths.app_config_dir()?;
            let log_dir = paths.app_log_dir()?;
            let cache_dir = paths.app_cache_dir()?;
            let default_install_root = paths.home_dir()?.join("FarfaLauncher").join("Jeux");
            if let Some(guard) = logging::init(&log_dir) {
                // Garde le writer de logs vivant pendant toute l'exécution.
                app.manage(guard);
            }
            app.manage(state::AppState::new(config_dir, log_dir, cache_dir, default_install_root));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_logs,
            commands::open_url,
            commands::log_frontend,
            commands::session::session_status,
            commands::session::set_server,
            commands::session::start_device_auth,
            commands::session::poll_device_auth,
            commands::session::cancel_device_auth,
            commands::session::login_with_pair_code,
            commands::session::login_with_token,
            commands::session::logout,
            commands::library::system_target,
            commands::library::library_cached,
            commands::library::library_refresh,
            commands::library::image_path,
            commands::library::manual_path,
            commands::library::game_playtime,
            commands::library::clear_cache,
            commands::install::install_info,
            commands::install::install_game,
            commands::install::cancel_install,
            commands::install::executable_candidates,
            commands::install::choose_executable,
            commands::install::uninstall_game,
            commands::install::open_game_folder,
            commands::install::set_install_dir,
            commands::launch::running_games,
            commands::launch::play_game,
            commands::launch::flush_play_sessions,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de FarfaLauncher");
}
