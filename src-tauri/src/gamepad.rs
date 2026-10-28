//! Manettes : lues ici (gilrs) plutôt que par le moteur web, dont la prise en
//! charge varie (WebKitGTK sous Linux). Les évènements partent vers
//! l'interface (navigation, page « Contrôles ») seulement quand la fenêtre a
//! le focus : pas pendant qu'un jeu tourne.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use gilrs::{Axis, Button, EventType, Gilrs};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const EVENT: &str = "gamepad";

static FOCUSED: AtomicBool = AtomicBool::new(true);
static PADS: Mutex<BTreeMap<usize, String>> = Mutex::new(BTreeMap::new());

pub fn set_focused(focused: bool) {
    FOCUSED.store(focused, Ordering::Relaxed);
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PadEvent {
    Button { pad: usize, button: &'static str, pressed: bool },
    /// Valeur de -1 à 1 ; vers le bas positif pour les axes verticaux.
    Axis { pad: usize, axis: &'static str, value: f32 },
    Connected { pad: usize, name: String },
    Disconnected { pad: usize },
}

#[derive(Serialize)]
pub struct PadInfo {
    pub pad: usize,
    pub name: String,
}

pub fn connected() -> Vec<PadInfo> {
    PADS.lock().unwrap().iter().map(|(&pad, name)| PadInfo { pad, name: name.clone() }).collect()
}

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

fn axis(a: Axis, value: f32) -> Option<(&'static str, f32)> {
    Some(match a {
        Axis::LeftStickX => ("lx", value),
        Axis::LeftStickY => ("ly", -value),
        Axis::RightStickX => ("rx", value),
        Axis::RightStickY => ("ry", -value),
        _ => return None,
    })
}

/// Démarre la lecture des manettes sur un fil dédié.
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("manettes".into())
        .spawn(move || {
            let mut gilrs = match Gilrs::new() {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!("manettes indisponibles: {e}");
                    return;
                }
            };
            for (id, pad) in gilrs.gamepads() {
                PADS.lock().unwrap().insert(id.into(), pad.name().to_string());
            }
            // Dernière valeur envoyée par axe, pour n'envoyer que les vrais changements.
            let mut sent: BTreeMap<(usize, &'static str), f32> = BTreeMap::new();
            loop {
                let Some(ev) = gilrs.next_event_blocking(Some(Duration::from_secs(1))) else { continue };
                let pad: usize = ev.id.into();
                let out = match ev.event {
                    EventType::Connected => {
                        let name = gilrs.gamepad(ev.id).name().to_string();
                        PADS.lock().unwrap().insert(pad, name.clone());
                        Some(PadEvent::Connected { pad, name })
                    }
                    EventType::Disconnected => {
                        PADS.lock().unwrap().remove(&pad);
                        Some(PadEvent::Disconnected { pad })
                    }
                    _ if !FOCUSED.load(Ordering::Relaxed) => None,
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
