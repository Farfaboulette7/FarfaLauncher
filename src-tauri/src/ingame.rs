//! Menu en jeu : un appui maintenu sur Select + Start (lu par gamepad.rs pendant
//! la partie) fait glisser une colonne sur la gauche de l'écran, par-dessus le
//! jeu : durée de la partie, heure, nom du jeu, puis « Reprendre » et « Quitter
//! le jeu ». La colonne est une petite fenêtre à part, qui ne prend pas la main
//! au jeu ; le launcher, lui, reste où il est. Elle ne dépend ni de l'émulateur
//! ni de la recomp : le launcher ferme lui-même le jeu (voir launch::RunningGame).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

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
    /// Pause du jeu tant que le menu est ouvert.
    pub control: crate::launch::GameControl,
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

/// Prépare la colonne (cachée) dès le lancement d'un jeu : à l'ouverture, elle glisse déjà remplie.
pub fn prepare(app: &AppHandle) {
    if let Err(e) = column(app) {
        tracing::warn!("menu en jeu non préparé: {e}");
    }
}

/// Ouvre le menu sur la partie la plus récente (appelé par le fil des manettes).
pub fn open(app: &AppHandle) {
    let state = app.state::<AppState>();
    let latest = state.playing.lock().unwrap().iter().max_by_key(|(_, p)| p.started).map(|(id, p)| (*id, p.started.elapsed(), p.control.clone()));
    let Some((rom_id, elapsed, control)) = latest else { return };
    // Le jeu ne reçoit plus rien (manette, clavier) tant que le joueur choisit.
    control.pause();
    let (name, platform) = describe(&state, rom_id);
    tracing::info!("menu en jeu ouvert ({name})");
    let info = MenuInfo { rom_id, name, platform, session_ms: elapsed.as_millis() as u64, quitting: false };
    *SHOWN.lock().unwrap() = Some(info.clone());
    gamepad::set_menu(true);
    let _ = app.emit_to(LABEL, EVENT, Some(info));
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let w = match column(app) {
        Ok(w) => w,
        Err(e) => return tracing::error!("menu en jeu indisponible: {e}"),
    };
    let Some(rect) = screen_rect(app) else { return };
    // Hors de l'écran à gauche, puis glisse jusqu'au bord.
    show_at(&w, rect, rect.x - rect.w);
    std::thread::spawn(move || slide(&w, rect, generation, true));
}

/// Referme le menu : la colonne repart vers la gauche, la manette retourne au jeu.
pub fn close(app: &AppHandle) {
    gamepad::set_menu(false);
    for p in app.state::<AppState>().playing.lock().unwrap().values() {
        p.control.resume();
    }
    *SHOWN.lock().unwrap() = None;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(w) = app.get_webview_window(LABEL) else { return };
    let Some(rect) = *RECT.lock().unwrap() else { return };
    let app = app.clone();
    std::thread::spawn(move || {
        if slide(&w, rect, generation, false) {
            hide(&w);
            // Contenu vidé une fois la colonne partie (elle repart telle quelle).
            let _ = app.emit_to(LABEL, EVENT, None::<MenuInfo>);
        }
    });
}

/// Ouverture ou fermeture en cours : une plus récente interrompt l'animation.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// Place de la colonne ouverte, en pixels physiques.
static RECT: Mutex<Option<Rect>> = Mutex::new(None);
/// Durée du glissement.
const SLIDE: Duration = Duration::from_millis(220);

#[derive(Clone, Copy)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Glisse la colonne vers l'écran (`entering`) ou hors de l'écran. Faux si une autre
/// ouverture ou fermeture a pris la main entre-temps.
fn slide(w: &WebviewWindow, rect: Rect, generation: u64, entering: bool) -> bool {
    let started = Instant::now();
    loop {
        if GENERATION.load(Ordering::SeqCst) != generation {
            return false;
        }
        let t = (started.elapsed().as_secs_f64() / SLIDE.as_secs_f64()).min(1.0);
        // Arrivée qui ralentit, départ qui accélère.
        let shown = if entering { 1.0 - (1.0 - t).powi(3) } else { 1.0 - t.powi(3) };
        move_to(w, rect, rect.x - rect.w + (rect.w as f64 * shown).round() as i32);
        if t >= 1.0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// Bord gauche de l'écran du launcher (celui où le jeu est lancé), sur toute la hauteur.
fn screen_rect(app: &AppHandle) -> Option<Rect> {
    let m = app
        .get_webview_window("main")
        .and_then(|m| m.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let rect = Rect {
        x: m.position().x,
        y: m.position().y,
        w: (WIDTH * m.scale_factor()).round() as i32,
        h: m.size().height as i32,
    };
    *RECT.lock().unwrap() = Some(rect);
    Some(rect)
}

/// Déplace la colonne (sans la taille, sans prendre le focus).
fn move_to(w: &WebviewWindow, rect: Rect, x: i32) {
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE};
        // SAFETY : fenêtre de la colonne.
        unsafe { SetWindowPos(hwnd.0 as _, HWND_TOPMOST, x, rect.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE) };
        return;
    }
    let _ = w.set_position(PhysicalPosition::new(x, rect.y));
}

/// Affiche la colonne en `x`, au-dessus de tout, sans prendre le focus au jeu.
fn show_at(w: &WebviewWindow, rect: Rect, x: i32) {
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOWNOACTIVATE};
        // SAFETY : fenêtre de la colonne.
        unsafe {
            let h = hwnd.0 as _;
            SetWindowPos(h, HWND_TOPMOST, x, rect.y, rect.w, rect.h, SWP_NOACTIVATE);
            ShowWindow(h, SW_SHOWNOACTIVATE);
        }
        return;
    }
    let _ = w.set_size(PhysicalSize::new(rect.w as u32, rect.h as u32));
    let _ = w.set_position(PhysicalPosition::new(x, rect.y));
    let _ = w.show();
    let _ = w.set_always_on_top(true);
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
    let (stop, control) = state.playing.lock().unwrap().get(&rom_id).map(|p| (p.stop.clone(), p.control.clone())).ok_or(AppError::NotInstalled)?;
    // Relancé pour qu'il puisse se fermer proprement.
    control.resume();
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
