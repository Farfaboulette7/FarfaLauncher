// Sans la feature `gui` (programme du Steam Deck), une partie du moteur ne sert pas.
#![cfg_attr(not(feature = "gui"), allow(dead_code, unused_imports))]

mod assets;
mod cache;
#[cfg(feature = "gui")]
mod commands;
mod controllers;
pub mod deck;
mod dialogs;
mod error;
mod external;
mod gamepad;
mod hardware;
#[cfg(feature = "gui")]
mod ingame;
mod install;
mod language;
mod links;
mod launch;
mod logging;
mod manager;
mod optimize;
mod pc;
mod play;
mod report;
mod platform;
mod presence;
mod romm;
mod runtimes;
mod save_store;
mod save_sync;
mod saves;
#[cfg(feature = "gui")]
mod secrets;
mod settings;
mod shaders;
mod stats;
mod steam;
mod state;
mod switch_content;
mod switch_saves;

/// Au-delà, les jaquettes les plus anciennes sont retirées au démarrage (elles se retéléchargent).
#[cfg(feature = "gui")]
const IMAGE_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[cfg(feature = "gui")]
use tauri::Manager;

#[cfg(feature = "gui")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    external::prepare_linux();
    tauri::Builder::default()
        // En premier : un second lancement (raccourci Steam) passe la main au launcher ouvert.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| commands::steam::on_second_instance(app, &args)))
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
            let default_install_root = install::default_root(&paths.home_dir()?);
            if let Some(guard) = logging::init(&log_dir) {
                // Garde le writer de logs vivant pendant toute l'exécution.
                app.manage(guard);
            }
            saves::set_prefixes_dir(pc::proton::prefixes_dir(&config_dir));
            let images = cache_dir.join("images");
            std::thread::spawn(move || {
                let removed = cache::prune_dir(&images, IMAGE_CACHE_MAX_BYTES);
                if removed > 0 {
                    tracing::info!("cache des images : {} Mo libérés", removed / (1024 * 1024));
                }
            });
            let state = state::AppState::new(config_dir, log_dir, cache_dir, default_install_root);
            // Fenêtre créée cachée : affichée directement au bon format.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_fullscreen(state.settings().fullscreen);
                let _ = window.show();
            }
            app.manage(state);
            app.manage(commands::update::PendingUpdate::default());
            gamepad::start(app.handle().clone());
            commands::presence::start(app.handle());
            commands::steam::on_start(&std::env::args().collect::<Vec<_>>());
            commands::stats::sync_later(app.handle(), 20);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Fenêtre principale fermée : les amis voient tout de suite le joueur hors ligne.
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                commands::presence::leave(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_logs,
            commands::update::check_update,
            commands::requests::my_game_requests,
            commands::requests::send_game_request,
            commands::requests::all_game_requests,
            commands::requests::update_game_request,
            commands::presence::friends,
            commands::presence::presence_hidden,
            commands::presence::set_presence_hidden,
            commands::presence::friend_avatar,
            commands::stats::player_stats,
            commands::steam::take_play_request,
            commands::steam::steam_info,
            commands::steam::add_to_steam,
            commands::stats::stats_ranking,
            commands::stats::game_ranking,
            commands::stats::set_stats_hidden,
            commands::update::install_update,
            commands::update::install_test_version,
            commands::update::beta_channel,
            commands::update::set_beta_channel,
            commands::open_url,
            commands::system::system_info,
            commands::system::server_ping,
            commands::system::optimizations,
            commands::system::set_optimized,
            commands::report::report_files,
            commands::report::send_report,
            commands::report::my_reports,
            commands::report::reports_seen,
            commands::report::mark_reports_seen,
            commands::report::reply_report,
            commands::report::support_status,
            commands::log_frontend,
            commands::window::is_fullscreen,
            commands::window::set_fullscreen,
            commands::window::gamepads,
            commands::session::session_status,
            commands::session::check_connection,
            commands::saves::flush_pending_saves,
            commands::saves::save_history,
            commands::saves::restore_save_version,
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
            commands::library::game_changelog,
            commands::library::clear_cache,
            commands::library::console_games,
            commands::library::console_games_range,
            commands::library::console_char_index,
            commands::library::search_games,
            commands::library::recent_games,
            commands::library::games_by_ids,
            commands::library::game_details,
            commands::library::manager_info,
            commands::library::set_manager_url,
            commands::library::favorites_list,
            commands::library::favorite_set,
            commands::sounds::sound_paths,
            commands::sounds::assets_info,
            commands::sounds::set_assets_url,
            commands::music::music_tracks,
            commands::music::music_files,
            commands::saves::save_check,
            commands::saves::shader_info,
            commands::saves::set_save_location,
            commands::saves::open_save_folder,
            commands::install::install_info,
            commands::install::install_game,
            commands::install::cancel_install,
            commands::install::pause_install,
            commands::install::resume_install,
            commands::install::move_install,
            commands::install::set_download_limit,
            commands::install::executable_candidates,
            commands::install::choose_executable,
            commands::install::uninstall_game,
            commands::install::use_install,
            commands::install::open_game_folder,
            commands::install::has_mods_folder,
            commands::install::open_mods_folder,
            commands::install::set_install_dir,
            commands::install::move_game,
            commands::install::proton_versions,
            commands::install::set_proton_version,
            commands::install::cod_modes,
            commands::install::bo3_language,
            commands::install::set_bo3_language,
            commands::install::game_language,
            commands::install::set_game_language,
            commands::install::set_key_binds,
            commands::install::controller_ports,
            commands::install::set_players,
            commands::launch::running_games,
            commands::launch::play_game,
            ingame::ingame_resume,
            ingame::ingame_state,
            ingame::quit_game,
            commands::launch::set_prefer_launcher,
            commands::launch::flush_play_sessions,
            commands::launch::set_crash_prompt,
            commands::mods::downloaded_mods,
            commands::mods::download_mod,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de FarfaLauncher");
}
