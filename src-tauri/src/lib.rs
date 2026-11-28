// Sans la feature `gui` (programme du Steam Deck), une partie du moteur ne sert pas.
#![cfg_attr(not(feature = "gui"), allow(dead_code, unused_imports))]

mod achievements;
mod assets;
mod cache;
mod community;
#[cfg(feature = "gui")]
mod commands;
mod controllers;
mod controls;
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
mod n3ds;
mod optimize;
mod pad_rate;
mod pc;
mod play;
mod play_status;
mod report;
mod platform;
mod presence;
mod romm;
mod runtimes;
mod save_store;
mod save_sync;
mod saves;
mod safe_file;
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
            // C'est aussi le seul dossier que l'interface lit par `asset:` (tauri.conf.json, SL16).
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
                fit_to_screen(&window);
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
                // La colonne cachée du menu en jeu (créée au premier lancement d'un jeu) est une
                // fenêtre : sans cet arrêt, le launcher restait ouvert, invisible, et bloquait
                // tout nouveau lancement (une seule instance).
                window.app_handle().exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_logs,
            commands::update::check_update,
            commands::requests::my_game_requests,
            commands::requests::send_game_request,
            commands::requests::all_game_requests,
            commands::requests::update_game_request,
            commands::fiches::fiches_list,
            commands::fiches::fiche_read,
            commands::fiches::fiche_check,
            commands::fiches::fiche_save,
            commands::fiches::fiche_rename,
            commands::service::service_status,
            commands::support::support_info,
            commands::ajouts::ajouts_list,
            commands::ajouts::ajouts_suivi,
            commands::ajouts::ajout_create,
            commands::presence::friends,
            commands::presence::presence_hidden,
            commands::presence::set_presence_hidden,
            commands::presence::friend_avatar,
            commands::stats::player_stats,
            commands::steam::take_play_request,
            commands::steam::steam_info,
            commands::steam::add_to_steam,
            commands::stats::stats_ranking,
            commands::stats::stats_sync,
            commands::community::game_reviews,
            commands::community::save_game_review,
            commands::community::delete_game_review,
            commands::community::game_reports,
            commands::stats::game_ranking,
            commands::stats::game_achievements,
            commands::stats::achievements_friends,
            commands::stats::ra_account,
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
            commands::window::pad_rates,
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
            commands::library::recomp_menu,
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
            commands::install::mods_folder,
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
            commands::install::controls_info,
            commands::install::set_controls,
            commands::install::set_controls_scope,
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

/// Taille par défaut de la fenêtre (`tauri.conf.json`, 1440 × 880) : assez large pour que les
/// onglets de la page d'un jeu tiennent sur une ligne. Sur un écran plus petit (Steam Deck en
/// mode bureau, 1280 × 800 ; petit portable ; écran à 125 %), on la réduit pour qu'elle tienne
/// dans la zone utile de l'écran (sans la barre des tâches), barre de titre comprise, sans
/// passer sous la taille minimale, puis on la recentre.
///
/// Appelée avant le premier affichage. Pièges (relecture du 2026-11-26) :
/// - `set_size` fixe la taille **intérieure** (sans la barre de titre) : on compare donc la
///   taille intérieure à la zone utile moins les décorations ;
/// - sous Linux, la taille extérieure d'une fenêtre encore cachée n'est pas fiable (GTK) : on
///   y prend une marge fixe pour la barre de titre et les bords.
#[cfg(feature = "gui")]
fn fit_to_screen(window: &tauri::WebviewWindow) {
    let monitor = match window.current_monitor() {
        Ok(Some(m)) => m,
        _ => match window.primary_monitor() {
            Ok(Some(m)) => m,
            _ => return, // écran inconnu (Wayland…) : on garde la taille par défaut
        },
    };
    let scale = monitor.scale_factor();
    let area = monitor.work_area().size.to_logical::<f64>(scale);
    let Ok(inner) = window.inner_size() else { return };
    let inner = inner.to_logical::<f64>(scale);
    // Décorations (barre de titre et bords), en pixels logiques.
    let (deco_w, deco_h) = decorations(window, scale, inner);
    let max_w = area.width - deco_w;
    let max_h = area.height - deco_h;
    if inner.width <= max_w && inner.height <= max_h {
        return;
    }
    // 960 × 600 : taille minimale de la fenêtre (`tauri.conf.json`).
    let width = inner.width.min(max_w).max(960.0);
    let height = inner.height.min(max_h).max(600.0);
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
    let _ = window.center();
}

/// Taille des décorations de la fenêtre : extérieure moins intérieure sous Windows (juste avant
/// l'affichage), marge fixe ailleurs (taille extérieure pas encore connue sous Linux).
#[cfg(feature = "gui")]
fn decorations(window: &tauri::WebviewWindow, scale: f64, inner: tauri::LogicalSize<f64>) -> (f64, f64) {
    const MARGE: (f64, f64) = (16.0, 48.0);
    if !cfg!(windows) {
        return MARGE;
    }
    match window.outer_size() {
        Ok(outer) => {
            let outer = outer.to_logical::<f64>(scale);
            let (w, h) = (outer.width - inner.width, outer.height - inner.height);
            if (0.0..200.0).contains(&w) && (0.0..200.0).contains(&h) && h > 0.0 {
                (w, h)
            } else {
                MARGE
            }
        }
        Err(_) => MARGE,
    }
}
