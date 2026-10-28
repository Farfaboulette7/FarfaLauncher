//! Manettes, lues ici plutôt que par le moteur web, dont la prise en charge
//! varie : gilrs sous Linux et macOS, XInput sous Windows. Les évènements
//! partent vers l'interface (navigation, page « Contrôles ») sauf pendant
//! qu'un jeu tourne.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(not(windows))]
use gilrs::{Axis, Button, EventType, Gamepad, Gilrs};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EVENT: &str = "gamepad";

/// Nouvelle recherche des manettes tant qu'aucune n'est vue : sous Linux, une
/// manette branchée après coup n'est pas toujours annoncée (droits d'accès
/// posés un instant après son apparition).
#[cfg(not(windows))]
const RESCAN_EVERY: Duration = Duration::from_secs(3);

static GAMES: AtomicUsize = AtomicUsize::new(0);
/// Recherche demandée (fin d'un jeu : il a pu garder la manette pour lui).
static RESCAN: AtomicBool = AtomicBool::new(false);
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
}

#[derive(Clone, PartialEq, Serialize)]
pub struct PadInfo {
    pub pad: usize,
    pub name: String,
    /// Disposition des boutons : `xbox`, `playstation`, `nintendo` ou `generic`.
    pub layout: &'static str,
}

pub fn connected() -> Vec<PadInfo> {
    PADS.lock().unwrap().values().cloned().collect()
}

/// Famille de la manette, d'après le fabricant puis le nom.
#[cfg_attr(windows, allow(dead_code))]
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

#[cfg(not(windows))]
fn info(pad: usize, gamepad: Gamepad<'_>) -> PadInfo {
    let name = gamepad.name().to_string();
    PadInfo { pad, layout: layout(gamepad.vendor_id(), &name), name }
}

#[cfg(not(windows))]
fn button_name(b: Button) -> Option<&'static str> {
    Some(match b {
        Button::South => "south",
        Button::East => "east",
        Button::West => "west",
        Button::North => "north",
        Button::LeftTrigger => "lb",
        Button::RightTrigger => "rb",
        Button::LeftTrigger2 => "lt",
        Button::RightTrigger2 => "rt",
        Button::Select => "select",
        Button::Start => "start",
        Button::Mode => "mode",
        Button::LeftThumb => "ls",
        Button::RightThumb => "rs",
        Button::DPadUp => "up",
        Button::DPadDown => "down",
        Button::DPadLeft => "left",
        Button::DPadRight => "right",
        _ => return None,
    })
}

#[cfg(not(windows))]
fn axis(a: Axis, value: f32) -> Option<(&'static str, f32)> {
    Some(match a {
        Axis::LeftStickX => ("lx", value),
        Axis::LeftStickY => ("ly", -value),
        Axis::RightStickX => ("rx", value),
        Axis::RightStickY => ("ry", -value),
        _ => return None,
    })
}

/// Recrée la lecture des manettes et annonce les changements à l'interface.
#[cfg(not(windows))]
fn rescan(app: &AppHandle) -> Option<Gilrs> {
    let gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("manettes indisponibles: {e}");
            return None;
        }
    };
    let found: BTreeMap<usize, PadInfo> = gilrs.gamepads().map(|(id, pad)| (id.into(), info(id.into(), pad))).collect();
    let mut pads = PADS.lock().unwrap();
    for (id, _) in pads.iter().filter(|(id, p)| found.get(id) != Some(p)) {
        let _ = app.emit(EVENT, PadEvent::Disconnected { pad: *id });
    }
    for pad in found.iter().filter(|(id, p)| pads.get(id) != Some(p)).map(|(_, p)| p) {
        tracing::info!("manette « {} » ({})", pad.name, pad.layout);
        let _ = app.emit(EVENT, PadEvent::Connected(pad.clone()));
    }
    *pads = found;
    Some(gilrs)
}

/// Démarre la lecture des manettes sur un fil dédié.
#[cfg(not(windows))]
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("manettes".into())
        .spawn(move || {
            let Some(mut gilrs) = rescan(&app) else { return };
            let mut last_scan = Instant::now();
            // Dernière valeur envoyée par axe, pour n'envoyer que les vrais changements.
            let mut sent: BTreeMap<(usize, &'static str), f32> = BTreeMap::new();
            loop {
                let idle = PADS.lock().unwrap().is_empty() && last_scan.elapsed() >= RESCAN_EVERY;
                if (idle || RESCAN.swap(false, Ordering::Relaxed)) && GAMES.load(Ordering::Relaxed) == 0 {
                    if let Some(g) = rescan(&app) {
                        gilrs = g;
                        sent.clear();
                    }
                    last_scan = Instant::now();
                }
                let Some(ev) = gilrs.next_event_blocking(Some(Duration::from_millis(500))) else { continue };
                let pad: usize = ev.id.into();
                let out = match ev.event {
                    EventType::Connected => {
                        let pad_info = info(pad, gilrs.gamepad(ev.id));
                        PADS.lock().unwrap().insert(pad, pad_info.clone());
                        tracing::info!("manette « {} » ({})", pad_info.name, pad_info.layout);
                        Some(PadEvent::Connected(pad_info))
                    }
                    EventType::Disconnected => {
                        PADS.lock().unwrap().remove(&pad);
                        Some(PadEvent::Disconnected { pad })
                    }
                    _ if GAMES.load(Ordering::Relaxed) > 0 => None,
                    EventType::ButtonPressed(b, _) => button_name(b).map(|button| PadEvent::Button { pad, button, pressed: true }),
                    EventType::ButtonReleased(b, _) => button_name(b).map(|button| PadEvent::Button { pad, button, pressed: false }),
                    EventType::AxisChanged(a, v, _) => axis(a, v).and_then(|(axis, value)| {
                        let value = (value * 20.0).round() / 20.0;
                        let last = sent.insert((pad, axis), value).unwrap_or(0.0);
                        (value != last).then_some(PadEvent::Axis { pad, axis, value })
                    }),
                    _ => None,
                };
                if let Some(out) = out {
                    let _ = app.emit(EVENT, out);
                }
            }
        })
        .expect("fil des manettes");
}

/// Windows : manettes Xbox (et compatibles) lues par XInput, sans condition de
/// focus. Windows.Gaming.Input (gilrs) ne transmet rien quand le focus est
/// dans le moteur web, qui tourne dans un autre processus. Les autres
/// manettes (PlayStation, Nintendo) sont lues par l'interface.
#[cfg(windows)]
pub fn start(app: AppHandle) {
    use windows_sys::Win32::UI::Input::XboxController::*;

    const BUTTONS: [(u16, &str); 14] = [
        (XINPUT_GAMEPAD_A, "south"),
        (XINPUT_GAMEPAD_B, "east"),
        (XINPUT_GAMEPAD_X, "west"),
        (XINPUT_GAMEPAD_Y, "north"),
        (XINPUT_GAMEPAD_LEFT_SHOULDER, "lb"),
        (XINPUT_GAMEPAD_RIGHT_SHOULDER, "rb"),
        (XINPUT_GAMEPAD_BACK, "select"),
        (XINPUT_GAMEPAD_START, "start"),
        (XINPUT_GAMEPAD_LEFT_THUMB, "ls"),
        (XINPUT_GAMEPAD_RIGHT_THUMB, "rs"),
        (XINPUT_GAMEPAD_DPAD_UP, "up"),
        (XINPUT_GAMEPAD_DPAD_DOWN, "down"),
        (XINPUT_GAMEPAD_DPAD_LEFT, "left"),
        (XINPUT_GAMEPAD_DPAD_RIGHT, "right"),
    ];
    // Emplacements vides : interrogés moins souvent (appel lent sans manette).
    const PROBE_EVERY: Duration = Duration::from_secs(2);
    const POLL: Duration = Duration::from_millis(16);

    /// État réduit d'une manette : boutons (gâchettes comprises) et axes arrondis.
    #[derive(Default, Clone, Copy, PartialEq)]
    struct Snapshot {
        buttons: u32,
        axes: [f32; 4],
    }

    fn read(slot: u32) -> Option<Snapshot> {
        let mut state = XINPUT_STATE::default();
        // SAFETY : `state` est une structure valide, écrite par XInput.
        if unsafe { XInputGetState(slot, &mut state) } != 0 {
            return None;
        }
        let g = state.Gamepad;
        let mut buttons = g.wButtons as u32;
        // Gâchettes : bits au-delà des boutons XInput.
        if g.bLeftTrigger > 100 {
            buttons |= 1 << 16;
        }
        if g.bRightTrigger > 100 {
            buttons |= 1 << 17;
        }
        let norm = |v: i16, dead: u16| {
            if v.unsigned_abs() < dead {
                0.0
            } else {
                ((v as f32 / 32767.0).clamp(-1.0, 1.0) * 20.0).round() / 20.0
            }
        };
        let (dl, dr) = (XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE, XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE);
        Some(Snapshot {
            buttons,
            axes: [norm(g.sThumbLX, dl), -norm(g.sThumbLY, dl), norm(g.sThumbRX, dr), -norm(g.sThumbRY, dr)],
        })
    }

    std::thread::Builder::new()
        .name("manettes".into())
        .spawn(move || {
            let mut last: [Option<Snapshot>; 4] = [None; 4];
            let mut probed = [Instant::now() - PROBE_EVERY; 4];
            loop {
                for slot in 0..4u32 {
                    let i = slot as usize;
                    if last[i].is_none() && probed[i].elapsed() < PROBE_EVERY {
                        continue;
                    }
                    probed[i] = Instant::now();
                    let now = read(slot);
                    let pad = i;
                    match (last[i], now) {
                        (None, Some(_)) => {
                            let info = PadInfo { pad, name: format!("Manette Xbox {}", slot + 1), layout: "xbox" };
                            tracing::info!("manette « {} » (xinput)", info.name);
                            PADS.lock().unwrap().insert(pad, info.clone());
                            let _ = app.emit(EVENT, PadEvent::Connected(info));
                        }
                        (Some(_), None) => {
                            PADS.lock().unwrap().remove(&pad);
                            let _ = app.emit(EVENT, PadEvent::Disconnected { pad });
                        }
                        _ => {}
                    }
                    if let (Some(before), Some(after)) = (last[i], now) {
                        if before != after && GAMES.load(Ordering::Relaxed) == 0 {
                            let changed = before.buttons ^ after.buttons;
                            let named = BUTTONS.iter().map(|&(bit, name)| (bit as u32, name)).chain([(1 << 16, "lt"), (1 << 17, "rt")]);
                            for (bit, button) in named.filter(|(bit, _)| changed & bit != 0) {
                                let _ = app.emit(EVENT, PadEvent::Button { pad, button, pressed: after.buttons & bit != 0 });
                            }
                            for (k, axis) in ["lx", "ly", "rx", "ry"].into_iter().enumerate() {
                                if before.axes[k] != after.axes[k] {
                                    let _ = app.emit(EVENT, PadEvent::Axis { pad, axis, value: after.axes[k] });
                                }
                            }
                        }
                    }
                    last[i] = now;
                }
                std::thread::sleep(POLL);
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
}
