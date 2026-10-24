mod assets;
mod cache;
mod commands;
mod error;
mod install;
mod launch;
mod logging;
mod platform;
mod romm;
mod saves;
mod secrets;
mod settings;
mod state;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let paths = app.path();
            let config_dir = paths.app_config_dir()?;
            let log_dir = paths.app_log_dir()?;
            // Sous-dossier dédié : le dossier de cache de l'application contient
            // aussi les logs et les données du WebView (Windows), à ne pas vider.
            let app_cache = paths.app_cache_dir()?;
            let cache_dir = app_cache.join("cache");
            cache::remove_legacy_entries(&app_cache);
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
            commands::library::favorites_list,
            commands::library::favorite_set,
            commands::sounds::sound_paths,
            commands::sounds::assets_info,
            commands::sounds::set_assets_url,
            commands::music::music_tracks,
            commands::music::music_files,
            commands::saves::save_check,
            commands::saves::set_save_location,
            commands::saves::open_save_folder,
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
            commands::mods::downloaded_mods,
            commands::mods::download_mod,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de FarfaLauncher");
}
