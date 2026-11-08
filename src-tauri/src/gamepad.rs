//! Manettes, lues par SDL3 (compilé dans le launcher) sur tous les systèmes.
//! SDL est aussi ce qu'utilisent Dolphin et la plupart des recomps : le
//! launcher voit chaque manette comme eux (nom, GUID, emplacement XInput), ce
//! qui permet d'attribuer les joueurs (voir controllers.rs).
//!
//! Les évènements partent vers l'interface (navigation, page « Contrôles »).
//! Pendant qu'un jeu tourne, SDL est rouvert en simple lecture (sans HIDAPI, qui
//! prendrait la main sur les manettes PlayStation et Nintendo) : rien ne part vers
//! l'interface, sauf Select + Start maintenus, qui ouvrent le menu en jeu (ingame.rs) ;
//! la manette pilote alors ce menu jusqu'à sa fermeture.

use std::collections::BTreeMap;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sdl3_sys::events::{SDL_Event, SDL_EventType, SDL_WaitEventTimeout};
use sdl3_sys::gamepad::*;
use sdl3_sys::guid::{SDL_GUIDToString, SDL_GUID};
use sdl3_sys::hints::*;
use sdl3_sys::init::{SDL_InitSubSystem, SDL_QuitSubSystem, SDL_INIT_GAMEPAD};
use sdl3_sys::joystick::SDL_JoystickID;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EVENT: &str = "gamepad";

static GAMES: AtomicUsize = AtomicUsize::new(0);
/// Relecture demandée (fin d'un jeu).
static RESCAN: AtomicBool = AtomicBool::new(false);
/// Menu en jeu ouvert : la manette le pilote.
static MENU: AtomicBool = AtomicBool::new(false);
/// Durée d'appui sur Select + Start pour ouvrir le menu en jeu.
const MENU_HOLD: Duration = Duration::from_millis(1000);

pub fn set_menu(open: bool) {
    MENU.store(open, Ordering::Relaxed);
}
static PADS: Mutex<BTreeMap<usize, PadInfo>> = Mutex::new(BTreeMap::new());

pub fn game_started() {
    GAMES.fetch_add(1, Ordering::Relaxed);
}

pub fn game_stopped() {
    if GAMES.load(Ordering::Relaxed) > 0 {
        GAMES.fetch_sub(1, Ordering::Relaxed);
    }
    RESCAN.store(true, Ordering::Relaxed);
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PadEvent {
    Button { pad: usize, button: &'static str, pressed: bool },
    /// Valeur de -1 à 1 ; vers le bas positif pour les axes verticaux.
    Axis { pad: usize, axis: &'static str, value: f32 },
    Connected(PadInfo),
    Disconnected { pad: usize },
    /// Pendant une partie : famille d'une manette, pour les boutons du menu en jeu.
    Layout { pad: usize, layout: &'static str },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PadInfo {
    /// Numéro de la manette pour cette session de SDL (change à chaque rebranchement).
    pub pad: usize,
    pub name: String,
    /// Disposition des boutons : `xbox`, `playstation`, `nintendo` ou `generic`.
    pub layout: &'static str,
    /// Identifiant durable : GUID SDL et numéro de série (sinon chemin du périphérique).
    pub key: String,
    /// GUID SDL (32 caractères hexadécimaux), comme dans gamecontrollerdb.txt.
    pub guid: String,
    pub vendor: u16,
    pub product: u16,
    /// Emplacement XInput (0 à 3) d'une manette Xbox sous Windows.
    pub xinput: Option<u8>,
    /// Numéro de série donné par SDL (manettes Bluetooth, PlayStation…).
    pub serial: Option<String>,
    /// Ordre d'arrivée, pour l'attribution par défaut des joueurs.
    #[serde(skip)]
    pub order: u64,
    /// Entrée brute de chaque élément de la manette, comme l'écrit Eden
    /// (`south` → `button:0`, `lefttrigger` → `axis:2`, `dpup` → `hat:0,direction:up`).
    #[serde(skip)]
    pub bindings: Vec<(&'static str, String)>,
    /// Nombre d'axes du joystick brut : Snes9x (Linux) numérote ses croix après eux.
    #[serde(skip)]
    pub axes: i32,
}

fn button_key(b: SDL_GamepadButton) -> Option<&'static str> {
    Some(match b {
        SDL_GamepadButton::SOUTH => "south",
        SDL_GamepadButton::EAST => "east",
        SDL_GamepadButton::WEST => "west",
        SDL_GamepadButton::NORTH => "north",
        SDL_GamepadButton::BACK => "back",
        SDL_GamepadButton::GUIDE => "guide",
        SDL_GamepadButton::START => "start",
        SDL_GamepadButton::LEFT_STICK => "leftstick",
        SDL_GamepadButton::RIGHT_STICK => "rightstick",
        SDL_GamepadButton::LEFT_SHOULDER => "leftshoulder",
        SDL_GamepadButton::RIGHT_SHOULDER => "rightshoulder",
        SDL_GamepadButton::DPAD_UP => "dpup",
        SDL_GamepadButton::DPAD_DOWN => "dpdown",
        SDL_GamepadButton::DPAD_LEFT => "dpleft",
        SDL_GamepadButton::DPAD_RIGHT => "dpright",
        SDL_GamepadButton::MISC1 => "misc1",
        _ => return None,
    })
}

fn axis_key(a: SDL_GamepadAxis) -> Option<&'static str> {
    Some(match a {
        SDL_GamepadAxis::LEFTX => "leftx",
        SDL_GamepadAxis::LEFTY => "lefty",
        SDL_GamepadAxis::RIGHTX => "rightx",
        SDL_GamepadAxis::RIGHTY => "righty",
        SDL_GamepadAxis::LEFT_TRIGGER => "lefttrigger",
        SDL_GamepadAxis::RIGHT_TRIGGER => "righttrigger",
        _ => return None,
    })
}

/// Correspondance de la manette (SDL_GetGamepadBindings), la première pour chaque élément.
///
/// # Safety
/// `pad` est une manette ouverte, sur le fil de SDL.
unsafe fn bindings(pad: *mut SDL_Gamepad) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut count = 0;
    let list = SDL_GetGamepadBindings(pad, &mut count);
    if list.is_null() {
        return out;
    }
    for i in 0..count.max(0) as usize {
        let b = &**list.add(i);
        let key = match b.output_type {
            SDL_GamepadBindingType::BUTTON => button_key(b.output.button),
            SDL_GamepadBindingType::AXIS => axis_key(b.output.axis.axis),
            _ => None,
        };
        let input = match b.input_type {
            SDL_GamepadBindingType::BUTTON => Some(format!("button:{}", b.input.button)),
            SDL_GamepadBindingType::AXIS => Some(format!("axis:{}", b.input.axis.axis)),
            SDL_GamepadBindingType::HAT => {
                let direction = match b.input.hat.hat_mask {
                    1 => "up",
                    2 => "right",
                    4 => "down",
                    8 => "left",
                    _ => "",
                };
                (!direction.is_empty()).then(|| format!("hat:{},direction:{direction}", b.input.hat.hat))
            }
            _ => None,
        };
        if let (Some(key), Some(input)) = (key, input) {
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, input));
            }
        }
    }
    sdl3_sys::stdinc::SDL_free(list.cast());
    out
}

pub fn connected() -> Vec<PadInfo> {
    let mut pads: Vec<PadInfo> = PADS.lock().unwrap().values().cloned().collect();
    pads.sort_by_key(|p| p.order);
    pads
}

/// Famille de la manette, d'après le fabricant puis le nom.
fn layout(vendor: Option<u16>, name: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    match vendor {
        Some(0x054c) => return "playstation",
        Some(0x057e) => return "nintendo",
        Some(0x045e) | Some(0x28de) => return "xbox",
        _ => {}
    }
    // « Wireless Controller » tout court : nom des manettes Sony.
    if ["xbox", "x-box", "xinput", "steam deck", "steam virtual"].iter().any(|k| name.contains(k)) {
        "xbox"
    } else if name == "wireless controller" || ["dualsense", "dualshock", "playstation", "ps4", "ps5"].iter().any(|k| name.contains(k)) {
        "playstation"
    } else if ["nintendo", "pro controller", "joy-con", "switch"].iter().any(|k| name.contains(k)) {
        "nintendo"
    } else {
        "generic"
    }
}

/// Le GUID SDL porte, en octet 14, le pilote qui lit la manette : `x` pour XInput.
fn is_xinput(guid: &SDL_GUID) -> bool {
    guid.data[14] == b'x'
}

fn guid_string(guid: SDL_GUID) -> String {
    let mut buf = [0 as c_char; 33];
    // SAFETY : tampon de 33 octets, la taille demandée par SDL.
    unsafe { SDL_GUIDToString(guid, buf.as_mut_ptr(), buf.len() as i32) };
    text(buf.as_ptr()).unwrap_or_default()
}

fn text(p: *const c_char) -> Option<String> {
    // SAFETY : chaîne terminée par un zéro, fournie par SDL, ou nulle.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
}

/// Ouvre la manette et relève son identité.
fn open(id: SDL_JoystickID, order: u64) -> Option<(*mut SDL_Gamepad, PadInfo)> {
    // SAFETY : appels SDL sur le fil qui l'a initialisé ; pointeur vérifié.
    unsafe {
        let pad = SDL_OpenGamepad(id);
        if pad.is_null() {
            return None;
        }
        let guid = SDL_GetGamepadGUIDForID(id);
        let name = text(SDL_GetGamepadName(pad)).unwrap_or_else(|| "Manette".into());
        let vendor = SDL_GetGamepadVendor(pad);
        let product = SDL_GetGamepadProduct(pad);
        let layout = match SDL_GetGamepadType(pad) {
            SDL_GamepadType::XBOX360 | SDL_GamepadType::XBOXONE => "xbox",
            SDL_GamepadType::PS3 | SDL_GamepadType::PS4 | SDL_GamepadType::PS5 => "playstation",
            SDL_GamepadType::NINTENDO_SWITCH_PRO
            | SDL_GamepadType::NINTENDO_SWITCH_JOYCON_LEFT
            | SDL_GamepadType::NINTENDO_SWITCH_JOYCON_RIGHT
            | SDL_GamepadType::NINTENDO_SWITCH_JOYCON_PAIR
            | SDL_GamepadType::GAMECUBE => "nintendo",
            _ => layout((vendor != 0).then_some(vendor), &name),
        };
        let xinput = if is_xinput(&guid) {
            u8::try_from(SDL_GetGamepadPlayerIndex(pad)).ok().filter(|i| *i < 4)
        } else {
            None
        };
        let guid_text = guid_string(guid);
        let serial = text(SDL_GetGamepadSerial(pad));
        let place = serial
            .clone()
            .or_else(|| text(SDL_GetGamepadPath(pad)))
            .or_else(|| xinput.map(|i| format!("xinput{i}")))
            .unwrap_or_default();
        let info = PadInfo {
            pad: id.0 as usize,
            name,
            layout,
            key: format!("{guid_text}/{place}"),
            guid: guid_text,
            vendor,
            product,
            xinput,
            serial,
            order,
            bindings: bindings(pad),
            axes: sdl3_sys::joystick::SDL_GetNumJoystickAxes(SDL_GetGamepadJoystick(pad)).max(0),
        };
        Some((pad, info))
    }
}

fn button_name(b: i32) -> Option<&'static str> {
    Some(match SDL_GamepadButton(b) {
        SDL_GamepadButton::SOUTH => "south",
        SDL_GamepadButton::EAST => "east",
        SDL_GamepadButton::WEST => "west",
        SDL_GamepadButton::NORTH => "north",
        SDL_GamepadButton::LEFT_SHOULDER => "lb",
        SDL_GamepadButton::RIGHT_SHOULDER => "rb",
        SDL_GamepadButton::BACK => "select",
        SDL_GamepadButton::START => "start",
        SDL_GamepadButton::GUIDE => "mode",
        SDL_GamepadButton::LEFT_STICK => "ls",
        SDL_GamepadButton::RIGHT_STICK => "rs",
        SDL_GamepadButton::DPAD_UP => "up",
        SDL_GamepadButton::DPAD_DOWN => "down",
        SDL_GamepadButton::DPAD_LEFT => "left",
        SDL_GamepadButton::DPAD_RIGHT => "right",
        _ => return None,
    })
}

/// Lecture des manettes ouverte : manettes SDL en cours et dernières valeurs envoyées.
struct Session {
    open: BTreeMap<u32, *mut SDL_Gamepad>,
    sent: BTreeMap<(usize, &'static str), f32>,
    triggers: BTreeMap<(usize, &'static str), bool>,
    /// Pendant une partie : rien vers l'interface hors du menu en jeu.
    watch: bool,
    /// Boutons Select et Start enfoncés, par manette.
    held: BTreeMap<usize, (bool, bool)>,
    layouts: BTreeMap<usize, &'static str>,
    combo_since: Option<Instant>,
    /// Menu déjà ouvert pour cet appui (il faut relâcher pour recommencer).
    combo_fired: bool,
    /// Familles des manettes envoyées à l'interface depuis l'ouverture du menu.
    layouts_sent: bool,
}

impl Session {
    fn start(watch: bool) -> Option<Session> {
        // SAFETY : réglages et initialisation sur le fil des manettes.
        unsafe {
            // Lire la manette même quand la fenêtre n'a pas le focus (moteur web
            // dans un autre processus sous Windows).
            SDL_SetHint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, c"1".as_ptr());
            // Ne pas changer les voyants des manettes PlayStation et Xbox.
            SDL_SetHint(SDL_HINT_JOYSTICK_HIDAPI_PS5_PLAYER_LED, c"0".as_ptr());
            SDL_SetHint(SDL_HINT_JOYSTICK_HIDAPI_XBOX_ONE_HOME_LED, c"0".as_ptr());
            // Pendant une partie, pas d'accès exclusif aux manettes (HIDAPI).
            SDL_SetHint(SDL_HINT_JOYSTICK_HIDAPI, if watch { c"0".as_ptr() } else { c"1".as_ptr() });
            if !SDL_InitSubSystem(SDL_INIT_GAMEPAD) {
                tracing::warn!("manettes indisponibles: {:?}", text(sdl3_sys::error::SDL_GetError()));
                return None;
            }
        }
        Some(Session {
            open: BTreeMap::new(),
            sent: BTreeMap::new(),
            triggers: BTreeMap::new(),
            watch,
            held: BTreeMap::new(),
            layouts: BTreeMap::new(),
            combo_since: None,
            combo_fired: false,
            layouts_sent: false,
        })
    }

    fn stop(self, app: &AppHandle) {
        // SAFETY : manettes ouvertes par cette session, fermées une seule fois.
        unsafe {
            for (_, pad) in self.open {
                SDL_CloseGamepad(pad);
            }
            SDL_QuitSubSystem(SDL_INIT_GAMEPAD);
        }
        if self.watch {
            return;
        }
        let old = std::mem::take(&mut *PADS.lock().unwrap());
        for pad in old.keys() {
            let _ = app.emit(EVENT, PadEvent::Disconnected { pad: *pad });
        }
    }

    fn handle(&mut self, app: &AppHandle, ev: &SDL_Event, order: &mut u64) {
        // SAFETY : le champ lu correspond au type de l'évènement.
        let kind = SDL_EventType(unsafe { ev.r#type });
        let out = match kind {
            SDL_EventType::GAMEPAD_ADDED => {
                let id = unsafe { ev.gdevice.which };
                if self.open.contains_key(&id.0) {
                    return;
                }
                *order += 1;
                let Some((handle, info)) = open(id, *order) else { return };
                self.open.insert(id.0, handle);
                if self.watch {
                    self.layouts.insert(info.pad, info.layout);
                    self.layouts_sent = false;
                    return;
                }
                tracing::info!("manette « {} » ({}, {}{})", info.name, info.layout, info.guid,
                    info.xinput.map(|i| format!(", XInput {i}")).unwrap_or_default());
                PADS.lock().unwrap().insert(info.pad, info.clone());
                Some(PadEvent::Connected(info))
            }
            SDL_EventType::GAMEPAD_REMOVED => {
                let id = unsafe { ev.gdevice.which };
                if let Some(handle) = self.open.remove(&id.0) {
                    // SAFETY : manette ouverte par cette session.
                    unsafe { SDL_CloseGamepad(handle) };
                }
                let pad = id.0 as usize;
                self.sent.retain(|(p, _), _| *p != pad);
                if self.watch {
                    self.held.remove(&pad);
                    self.layouts.remove(&pad);
                    return;
                }
                PADS.lock().unwrap().remove(&pad);
                Some(PadEvent::Disconnected { pad })
            }
            SDL_EventType::GAMEPAD_BUTTON_DOWN | SDL_EventType::GAMEPAD_BUTTON_UP => {
                let b = unsafe { ev.gbutton };
                button_name(b.button as i32).map(|button| PadEvent::Button { pad: b.which.0 as usize, button, pressed: b.down })
            }
            SDL_EventType::GAMEPAD_AXIS_MOTION => {
                let a = unsafe { ev.gaxis };
                let pad = a.which.0 as usize;
                let raw = a.value as f32 / 32767.0;
                match SDL_GamepadAxis(a.axis as i32) {
                    // Gâchettes : vues comme des boutons.
                    SDL_GamepadAxis::LEFT_TRIGGER | SDL_GamepadAxis::RIGHT_TRIGGER => {
                        let button = if SDL_GamepadAxis(a.axis as i32) == SDL_GamepadAxis::LEFT_TRIGGER { "lt" } else { "rt" };
                        let pressed = raw > 0.4;
                        let before = self.triggers.insert((pad, button), pressed).unwrap_or(false);
                        (pressed != before).then_some(PadEvent::Button { pad, button, pressed })
                    }
                    axis => {
                        let name = match axis {
                            SDL_GamepadAxis::LEFTX => "lx",
                            SDL_GamepadAxis::LEFTY => "ly",
                            SDL_GamepadAxis::RIGHTX => "rx",
                            SDL_GamepadAxis::RIGHTY => "ry",
                            _ => return,
                        };
                        let value = if raw.abs() < 0.24 { 0.0 } else { (raw.clamp(-1.0, 1.0) * 20.0).round() / 20.0 };
                        let last = self.sent.insert((pad, name), value).unwrap_or(0.0);
                        (value != last).then_some(PadEvent::Axis { pad, axis: name, value })
                    }
                }
            }
            _ => None,
        };
        let Some(out) = out else { return };
        if self.watch {
            if let PadEvent::Button { pad, button: b @ ("select" | "start"), pressed } = out {
                let h = self.held.entry(pad).or_default();
                if b == "select" {
                    h.0 = pressed
                } else {
                    h.1 = pressed
                }
            }
            if !MENU.load(Ordering::Relaxed) {
                self.layouts_sent = false;
                return;
            }
            if !self.layouts_sent {
                self.layouts_sent = true;
                for (&pad, &layout) in &self.layouts {
                    let _ = app.emit(EVENT, PadEvent::Layout { pad, layout });
                }
            }
        }
        let _ = app.emit(EVENT, out);
    }

    /// Pendant une partie : Select + Start maintenus ouvrent le menu en jeu.
    fn check_combo(&mut self, app: &AppHandle) {
        if !self.held.values().any(|&(select, start)| select && start) {
            self.combo_since = None;
            self.combo_fired = false;
            return;
        }
        let since = *self.combo_since.get_or_insert_with(Instant::now);
        if !self.combo_fired && since.elapsed() >= MENU_HOLD {
            self.combo_fired = true;
            if !MENU.load(Ordering::Relaxed) {
                crate::ingame::open(app);
            }
        }
    }
}

/// Démarre la lecture des manettes sur un fil dédié (SDL veut un seul fil).
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("manettes".into())
        .spawn(move || {
            let mut session: Option<Session> = None;
            let mut order = 0u64;
            loop {
                let playing = GAMES.load(Ordering::Relaxed) > 0;
                // Début ou fin de partie : SDL rouvert dans l'autre mode.
                let rescan = RESCAN.swap(false, Ordering::Relaxed);
                if rescan || session.as_ref().is_some_and(|s| s.watch != playing) {
                    if let Some(s) = session.take() {
                        s.stop(&app);
                    }
                }
                if session.is_none() {
                    session = Session::start(playing);
                    if session.is_none() {
                        std::thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                }
                let s = session.as_mut().unwrap();
                let mut ev = SDL_Event::default();
                // SAFETY : évènement écrit par SDL sur ce fil.
                if unsafe { SDL_WaitEventTimeout(&mut ev, if s.watch { 100 } else { 250 }) } {
                    s.handle(&app, &ev, &mut order);
                }
                if s.watch {
                    s.check_combo(&app);
                }
            }
        })
        .expect("fil des manettes");
}

#[cfg(test)]
mod tests {
    use super::layout;

    #[test]
    fn layouts() {
        assert_eq!(layout(Some(0x054c), "Wireless Controller"), "playstation");
        assert_eq!(layout(Some(0x057e), "Pro Controller"), "nintendo");
        assert_eq!(layout(None, "Xbox Wireless Controller"), "xbox");
        assert_eq!(layout(None, "DualSense Wireless Controller"), "playstation");
        assert_eq!(layout(Some(0x1234), "HHL GCU 1"), "generic");
    }

    /// Liste les manettes vues par SDL pendant 3 s (cargo test -- --ignored sdl_pads --nocapture).
    #[test]
    #[ignore]
    fn sdl_pads() {
        use super::*;
        let s = Session::start(false).expect("SDL");
        let mut order = 0;
        let mut ev = SDL_Event::default();
        let until = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < until {
            if unsafe { SDL_WaitEventTimeout(&mut ev, 100) } && SDL_EventType(unsafe { ev.r#type }) == SDL_EventType::GAMEPAD_ADDED {
                let id = unsafe { ev.gdevice.which };
                order += 1;
                if let Some((_, info)) = open(id, order) {
                    println!("{info:?}");
                }
            }
        }
        drop(s);
    }
}
