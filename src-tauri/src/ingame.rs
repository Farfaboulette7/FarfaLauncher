//! Menu en jeu : un appui maintenu sur Select + Start (lu par gamepad.rs pendant
//! la partie) fait glisser une colonne sur la gauche de l'écran, par-dessus le
//! jeu : durée de la partie, heure, nom du jeu, puis « Reprendre » et « Quitter
//! le jeu ». La colonne est une petite fenêtre à part, qui ne prend pas la main
//! au jeu ; le launcher, lui, reste où il est. Elle ne dépend ni de l'émulateur
//! ni de la recomp : le launcher ferme lui-même le jeu (voir launch::RunningGame).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::{AppError, AppResult};
use crate::gamepad;
use crate::state::AppState;

/// Fenêtre de la colonne (aussi dans capabilities/default.json et main.tsx).
pub const LABEL: &str = "ingame";
pub const EVENT: &str = "ingame-menu";
/// Largeur de la colonne (pixels logiques).
const WIDTH: f64 = 420.0;

/// Partie en cours.
pub struct Playing {
    pub started: Instant,
    /// Levé pour fermer le jeu (voir `RunningGame::stop_flag`).
    pub stop: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuInfo {
    rom_id: i64,
    name: String,
    platform: Option<String>,
    /// Durée de la partie au moment de l'ouverture.
    session_ms: u64,
    /// Fermeture du jeu en cours.
    quitting: bool,
}

/// Menu affiché (relu par la colonne à son chargement).
static SHOWN: Mutex<Option<MenuInfo>> = Mutex::new(None);

/// Nom et console du jeu, d'après la bibliothèque (sinon l'installation).
fn describe(state: &AppState, rom_id: i64) -> (String, Option<String>) {
    if let Some(lib) = state.library.lock().unwrap().as_ref() {
        if let Some(g) = lib.games.iter().find(|g| g.id == rom_id) {
            let platform = lib.platforms.iter().find(|p| p.id == g.platform_id).map(|p| p.name.clone());
            return (g.name.clone(), platform);
        }
    }
    let name = crate::install::state::find(&state.config_dir, rom_id).map(|st| st.name).unwrap_or_else(|| "Jeu en cours".into());
    (name, None)
}

/// Ouvre le menu sur la partie la plus récente (appelé par le fil des manettes).
pub fn open(app: &AppHandle) {
    let state = app.state::<AppState>();
    let latest = state.playing.lock().unwrap().iter().max_by_key(|(_, p)| p.started).map(|(id, p)| (*id, p.started.elapsed()));
    let Some((rom_id, elapsed)) = latest else { return };
    let (name, platform) = describe(&state, rom_id);
    tracing::info!("menu en jeu ouvert ({name})");
    let info = MenuInfo { rom_id, name, platform, session_ms: elapsed.as_millis() as u64, quitting: false };
    *SHOWN.lock().unwrap() = Some(info.clone());
    gamepad::set_menu(true);
    match column(app) {
        Ok(w) => {
            place(app, &w);
            show_without_focus(&w);
        }
        Err(e) => tracing::error!("menu en jeu indisponible: {e}"),
    }
    let _ = app.emit_to(LABEL, EVENT, Some(info));
}

/// Referme le menu : la manette retourne au jeu.
pub fn close(app: &AppHandle) {
    gamepad::set_menu(false);
    *SHOWN.lock().unwrap() = None;
    let _ = app.emit_to(LABEL, EVENT, None::<MenuInfo>);
    if let Some(w) = app.get_webview_window(LABEL) {
        hide(&w);
    }
}

/// Cache la colonne (montrée par Windows directement : Tauri la croit déjà cachée).
fn hide(w: &WebviewWindow) {
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        // SAFETY : fenêtre de la colonne.
        unsafe { ShowWindow(hwnd.0 as _, SW_HIDE) };
        return;
    }
    let _ = w.hide();
}

/// La fenêtre de la colonne, créée cachée à la première ouverture.
fn column(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(LABEL) {
        return Ok(w);
    }
    let w = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Menu en jeu")
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .focused(false)
        .visible(false)
        .inner_size(WIDTH, 600.0)
        .build()?;
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
        // SAFETY : fenêtre que l'on vient de créer. Jamais activée, même au clic : le jeu garde la main.
        unsafe {
            let h = hwnd.0 as _;
            let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize);
        }
    }
    Ok(w)
}

/// Colonne sur toute la hauteur de l'écran du launcher (celui où le jeu est lancé), à gauche.
fn place(app: &AppHandle, w: &WebviewWindow) {
    let monitor = app
        .get_webview_window("main")
        .and_then(|m| m.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };
    let scale = m.scale_factor();
    let pos = m.position().to_logical::<f64>(scale);
    let size = m.size().to_logical::<f64>(scale);
    let _ = w.set_position(LogicalPosition::new(pos.x, pos.y));
    let _ = w.set_size(LogicalSize::new(WIDTH, size.height));
}

/// Affiche la colonne au-dessus de tout sans prendre le focus au jeu.
fn show_without_focus(w: &WebviewWindow) {
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
        };
        // SAFETY : fenêtre de la colonne.
        unsafe {
            let h = hwnd.0 as _;
            ShowWindow(h, SW_SHOWNOACTIVATE);
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        return;
    }
    let _ = w.show();
    let _ = w.set_always_on_top(true);
}

/// Menu affiché, pour la colonne qui vient de se charger.
#[tauri::command]
pub fn ingame_state() -> Option<MenuInfo> {
    SHOWN.lock().unwrap().clone()
}

/// Reprendre la partie : la colonne disparaît.
#[tauri::command]
pub fn ingame_resume(app: AppHandle) {
    close(&app);
}

/// Quitter la partie : le jeu est prié de se fermer, puis arrêté s'il ne le fait pas.
/// La colonne reste affichée (« Fermeture du jeu… ») jusqu'à la fin du jeu.
#[tauri::command]
pub fn quit_game(app: AppHandle, state: State<'_, AppState>, rom_id: i64) -> AppResult<()> {
    let stop = state.playing.lock().unwrap().get(&rom_id).map(|p| p.stop.clone()).ok_or(AppError::NotInstalled)?;
    tracing::info!("quitter le jeu {rom_id} depuis le menu en jeu");
    gamepad::set_menu(false);
    stop.store(true, Ordering::Relaxed);
    let info = SHOWN.lock().unwrap().as_mut().map(|i| {
        i.quitting = true;
        i.clone()
    });
    let _ = app.emit_to(LABEL, EVENT, info);
    Ok(())
}
