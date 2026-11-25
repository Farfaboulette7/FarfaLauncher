//! Réglages de la page « Contrôles » : boutons de la console réassignés pour chaque joueur, et
//! leur portée (ce jeu, tous les jeux du même émulateur, tous les jeux).
//!
//! Gardés dans `controles.json` du dossier de config. Seuls les changements du joueur y sont :
//! un bouton absent garde le réglage par défaut ([`gc_buttons`]). Un joueur sans entrée n'a
//! jamais été touché : la configuration de l'émulateur reste alors telle quelle (réglages faits
//! dans Dolphin gardés) ; « Par défaut » laisse une entrée vide, qui remet les boutons du launcher.
//!
//! Aujourd'hui, seul Dolphin (GameCube) sait recevoir ces réglages (controllers.rs).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::LockOrRecover;

/// Clé de `PlayerPad` pour « Clavier et souris ».
pub const KEYBOARD: &str = "clavier-souris";

const FILE: &str = "controles.json";

/// Boutons réassignés d'un joueur : bouton de la console → source (`south`, `lx-` pour une
/// manette ; `key:KeyW`, `mouse:left`, `mouse:x-` pour le clavier et la souris).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerBinds {
    #[serde(default)]
    pub pad: BTreeMap<String, String>,
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

/// Portée des réglages d'un jeu.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Jeu,
    Emulateur,
    Tous,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// Portée choisie, par jeu (numéro RomM).
    #[serde(default)]
    scopes: BTreeMap<String, Scope>,
    /// Réglages par portée et par famille de boutons (`jeu:12|gc`, `emu:dolphin|gc`, `tous|gc`).
    #[serde(default)]
    profiles: BTreeMap<String, Vec<Option<PlayerBinds>>>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn load(dir: &Path) -> Store {
    let path = dir.join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else { return Store::default() };
    match serde_json::from_str(&text) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("{FILE} illisible ({e}) : mis de côté");
            crate::safe_file::set_aside(&path, "illisible");
            Store::default()
        }
    }
}

fn save(dir: &Path, store: &Store) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(store).map_err(std::io::Error::other)?;
    crate::safe_file::write_atomic(&dir.join(FILE), &text)
}

/// Où se trouvent les réglages d'un jeu.
pub struct Target<'a> {
    pub rom_id: i64,
    /// Identifiant de l'émulateur dans le catalogue.
    pub emulator: &'a str,
    /// Famille de boutons ([`layout_of`]).
    pub layout: &'a str,
}

impl Target<'_> {
    fn key(&self, scope: Scope) -> String {
        match scope {
            Scope::Jeu => format!("jeu:{}|{}", self.rom_id, self.layout),
            Scope::Emulateur => format!("emu:{}|{}", self.emulator, self.layout),
            Scope::Tous => format!("tous|{}", self.layout),
        }
    }
}

/// Famille de boutons qu'un format de manettes sait recevoir (`None` : boutons réglés dans
/// l'émulateur lui-même).
pub fn layout_of(format: &str) -> Option<&'static str> {
    match format.split('|').next().unwrap_or("").trim() {
        "dolphin" => Some("gc"),
        _ => None,
    }
}

/// Portée et boutons en vigueur pour ce jeu.
pub fn current(dir: &Path, t: &Target) -> (Scope, Vec<Option<PlayerBinds>>) {
    let _guard = LOCK.lock_or_recover();
    let store = load(dir);
    let scope = store.scopes.get(&t.rom_id.to_string()).copied().unwrap_or_default();
    (scope, store.profiles.get(&t.key(scope)).cloned().unwrap_or_default())
}

/// Enregistre les boutons, dans la portée en vigueur pour ce jeu.
pub fn set_binds(dir: &Path, t: &Target, mut binds: Vec<Option<PlayerBinds>>) -> std::io::Result<()> {
    let _guard = LOCK.lock_or_recover();
    let mut store = load(dir);
    let scope = store.scopes.get(&t.rom_id.to_string()).copied().unwrap_or_default();
    while binds.last().is_some_and(Option::is_none) {
        binds.pop();
    }
    store.profiles.insert(t.key(scope), binds);
    save(dir, &store)
}

/// Change la portée. Une portée encore vide reprend les boutons en vigueur (rien ne change
/// pour le joueur au moment où il choisit).
pub fn set_scope(dir: &Path, t: &Target, scope: Scope) -> std::io::Result<()> {
    let _guard = LOCK.lock_or_recover();
    let mut store = load(dir);
    let old = store.scopes.get(&t.rom_id.to_string()).copied().unwrap_or_default();
    if !store.profiles.contains_key(&t.key(scope)) {
        if let Some(binds) = store.profiles.get(&t.key(old)).cloned() {
            store.profiles.insert(t.key(scope), binds);
        }
    }
    if scope == Scope::Jeu {
        store.scopes.remove(&t.rom_id.to_string());
    } else {
        store.scopes.insert(t.rom_id.to_string(), scope);
    }
    save(dir, &store)
}

/// Bouton de la console émulée, avec ses réglages par défaut.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleButton {
    pub id: &'static str,
    pub label: &'static str,
    /// Source par défaut à la manette.
    pub pad: &'static str,
    /// Source par défaut au clavier et à la souris.
    pub keys: &'static str,
    /// Clés de Dolphin (GCPadNew.ini) qui reçoivent ce bouton.
    #[serde(skip)]
    pub dolphin: &'static [&'static str],
}

/// Manette GameCube. Par défaut : comme avant (controllers.rs), et au clavier le préréglage
/// Espace, Maj, E, R, Q, clic droit, clic gauche, Entrée, ZQSD (WASD en QWERTY), souris.
pub fn gc_buttons() -> Vec<ConsoleButton> {
    let b = |id, label, pad, keys, dolphin| ConsoleButton { id, label, pad, keys, dolphin };
    vec![
        b("a", "A", "south", "key:Space", &["Buttons/A"]),
        b("b", "B", "east", "key:ShiftLeft", &["Buttons/B"]),
        b("x", "X", "west", "key:KeyE", &["Buttons/X"]),
        b("y", "Y", "north", "key:KeyR", &["Buttons/Y"]),
        b("z", "Z", "rb", "key:KeyQ", &["Buttons/Z"]),
        b("l", "L", "lt", "mouse:right", &["Triggers/L", "Triggers/L-Analog"]),
        b("r", "R", "rt", "mouse:left", &["Triggers/R", "Triggers/R-Analog"]),
        b("start", "Start", "start", "key:Enter", &["Buttons/Start"]),
        b("up", "Stick ↑", "ly-", "key:KeyW", &["Main Stick/Up"]),
        b("down", "Stick ↓", "ly+", "key:KeyS", &["Main Stick/Down"]),
        b("left", "Stick ←", "lx-", "key:KeyA", &["Main Stick/Left"]),
        b("right", "Stick →", "lx+", "key:KeyD", &["Main Stick/Right"]),
        b("c-up", "Stick C ↑", "ry-", "mouse:y-", &["C-Stick/Up"]),
        b("c-down", "Stick C ↓", "ry+", "mouse:y+", &["C-Stick/Down"]),
        b("c-left", "Stick C ←", "rx-", "mouse:x-", &["C-Stick/Left"]),
        b("c-right", "Stick C →", "rx+", "mouse:x+", &["C-Stick/Right"]),
        b("d-up", "Croix ↑", "up", "key:ArrowUp", &["D-Pad/Up"]),
        b("d-down", "Croix ↓", "down", "key:ArrowDown", &["D-Pad/Down"]),
        b("d-left", "Croix ←", "left", "key:ArrowLeft", &["D-Pad/Left"]),
        b("d-right", "Croix →", "right", "key:ArrowRight", &["D-Pad/Right"]),
    ]
}

/// Boutons d'une famille (vide : famille inconnue).
pub fn buttons_of(layout: &str) -> Vec<ConsoleButton> {
    match layout {
        "gc" => gc_buttons(),
        _ => Vec::new(),
    }
}

/// Clavier et souris possibles ici (Dolphin : DInput sous Windows, XInput2 sous Linux).
pub fn keyboard_here() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// Périphérique « clavier et souris » de Dolphin sur ce système.
pub fn dolphin_keyboard_device() -> &'static str {
    if cfg!(windows) {
        "DInput/0/Keyboard Mouse"
    } else {
        "XInput2/0/Virtual core pointer"
    }
}

/// Expression de Dolphin pour une source de manette (noms communs à XInput et SDL).
pub fn dolphin_pad(source: &str) -> Option<&'static str> {
    Some(match source {
        "south" => "`Button A`",
        "east" => "`Button B`",
        "west" => "`Button X`",
        "north" => "`Button Y`",
        "lb" => "`Shoulder L`",
        "rb" => "`Shoulder R`",
        "lt" => "`Trigger L`",
        "rt" => "`Trigger R`",
        "start" => "Start",
        "select" => "Back",
        "mode" => "Guide",
        "ls" => "`Thumb L`",
        "rs" => "`Thumb R`",
        "up" => "`Pad N`",
        "down" => "`Pad S`",
        "left" => "`Pad W`",
        "right" => "`Pad E`",
        // SDL : vers le haut = négatif ; Dolphin : vers le haut = Y+.
        "lx-" => "`Left X-`",
        "lx+" => "`Left X+`",
        "ly-" => "`Left Y+`",
        "ly+" => "`Left Y-`",
        "rx-" => "`Right X-`",
        "rx+" => "`Right X+`",
        "ry-" => "`Right Y+`",
        "ry+" => "`Right Y-`",
        _ => return None,
    })
}

/// Expression de Dolphin pour une touche ou la souris. Les touches suivent leur position
/// (`KeyboardEvent.code`, celle du QWERTY), comme DirectInput : `KeyW` est le Z d'un AZERTY.
pub fn dolphin_keys(source: &str) -> Option<String> {
    let win = cfg!(windows);
    if let Some(m) = source.strip_prefix("mouse:") {
        let name = match (m, win) {
            ("left", true) => "Click 0",
            ("right", true) => "Click 1",
            ("middle", true) => "Click 2",
            ("left", false) => "Click 1",
            ("right", false) => "Click 3",
            ("middle", false) => "Click 2",
            ("x-", _) => "Cursor X-",
            ("x+", _) => "Cursor X+",
            ("y-", _) => "Cursor Y-",
            ("y+", _) => "Cursor Y+",
            _ => return None,
        };
        return Some(format!("`{name}`"));
    }
    let code = source.strip_prefix("key:")?;
    let name = if win { dinput_key(code)? } else { x11_key(code)? };
    Some(if name.chars().all(|c| c.is_ascii_alphanumeric()) { name } else { format!("`{name}`") })
}

/// Nom DirectInput (DIK_…) d'une touche, comme Dolphin l'écrit.
fn dinput_key(code: &str) -> Option<String> {
    if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1) {
        return Some(c.to_string());
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return Some(d.to_string());
    }
    if let Some(d) = code.strip_prefix("Numpad").filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) {
        return Some(format!("NUMPAD{d}"));
    }
    if code.starts_with('F') && code[1..].parse::<u8>().is_ok() {
        return Some(code.to_string());
    }
    Some(
        match code {
            "Space" => "SPACE",
            "Enter" => "RETURN",
            "Escape" => "ESCAPE",
            "Tab" => "TAB",
            "Backspace" => "BACK",
            "CapsLock" => "CAPITAL",
            "ShiftLeft" => "LSHIFT",
            "ShiftRight" => "RSHIFT",
            "ControlLeft" => "LCONTROL",
            "ControlRight" => "RCONTROL",
            "AltLeft" => "LMENU",
            "AltRight" => "RMENU",
            "ArrowUp" => "UP",
            "ArrowDown" => "DOWN",
            "ArrowLeft" => "LEFT",
            "ArrowRight" => "RIGHT",
            "Minus" => "MINUS",
            "Equal" => "EQUALS",
            "BracketLeft" => "LBRACKET",
            "BracketRight" => "RBRACKET",
            "Semicolon" => "SEMICOLON",
            "Quote" => "APOSTROPHE",
            "Backquote" => "GRAVE",
            "Backslash" => "BACKSLASH",
            "IntlBackslash" => "OEM_102",
            "Comma" => "COMMA",
            "Period" => "PERIOD",
            "Slash" => "SLASH",
            "NumpadAdd" => "ADD",
            "NumpadSubtract" => "SUBTRACT",
            "NumpadMultiply" => "MULTIPLY",
            "NumpadDivide" => "DIVIDE",
            "NumpadDecimal" => "DECIMAL",
            "NumpadEnter" => "NUMPADENTER",
            "Insert" => "INSERT",
            "Delete" => "DELETE",
            "Home" => "HOME",
            "End" => "END",
            "PageUp" => "PRIOR",
            "PageDown" => "NEXT",
            _ => return None,
        }
        .to_string(),
    )
}

/// Nom X11 (keysym) d'une touche, comme Dolphin l'écrit sous Linux. Disposition américaine :
/// sur un autre clavier, les lettres sont celles du QWERTY à la même position.
fn x11_key(code: &str) -> Option<String> {
    if let Some(c) = code.strip_prefix("Key").filter(|c| c.len() == 1) {
        return Some(c.to_ascii_lowercase());
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return Some(d.to_string());
    }
    if let Some(d) = code.strip_prefix("Numpad").filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) {
        return Some(format!("KP_{d}"));
    }
    if code.starts_with('F') && code[1..].parse::<u8>().is_ok() {
        return Some(code.to_string());
    }
    Some(
        match code {
            "Space" => "space",
            "Enter" => "Return",
            "Escape" => "Escape",
            "Tab" => "Tab",
            "Backspace" => "BackSpace",
            "CapsLock" => "Caps_Lock",
            "ShiftLeft" => "Shift_L",
            "ShiftRight" => "Shift_R",
            "ControlLeft" => "Control_L",
            "ControlRight" => "Control_R",
            "AltLeft" => "Alt_L",
            "AltRight" => "ISO_Level3_Shift",
            "ArrowUp" => "Up",
            "ArrowDown" => "Down",
            "ArrowLeft" => "Left",
            "ArrowRight" => "Right",
            "Minus" => "minus",
            "Equal" => "equal",
            "BracketLeft" => "bracketleft",
            "BracketRight" => "bracketright",
            "Semicolon" => "semicolon",
            "Quote" => "apostrophe",
            "Backquote" => "grave",
            "Backslash" => "backslash",
            "IntlBackslash" => "less",
            "Comma" => "comma",
            "Period" => "period",
            "Slash" => "slash",
            "NumpadAdd" => "KP_Add",
            "NumpadSubtract" => "KP_Subtract",
            "NumpadMultiply" => "KP_Multiply",
            "NumpadDivide" => "KP_Divide",
            "NumpadDecimal" => "KP_Decimal",
            "NumpadEnter" => "KP_Enter",
            "Insert" => "Insert",
            "Delete" => "Delete",
            "Home" => "Home",
            "End" => "End",
            "PageUp" => "Prior",
            "PageDown" => "Next",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_and_binds() {
        let dir = std::env::temp_dir().join(format!("farfa-controles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t = Target { rom_id: 12, emulator: "dolphin", layout: "gc" };
        assert_eq!(current(&dir, &t), (Scope::Jeu, vec![]));
        let mut p = PlayerBinds::default();
        p.pad.insert("a".into(), "east".into());
        set_binds(&dir, &t, vec![Some(p.clone()), None]).unwrap();
        assert_eq!(current(&dir, &t).1, vec![Some(p.clone())]);
        // Nouvelle portée vide : reprend les boutons en vigueur.
        set_scope(&dir, &t, Scope::Emulateur).unwrap();
        assert_eq!(current(&dir, &t), (Scope::Emulateur, vec![Some(p.clone())]));
        // Un autre jeu du même émulateur, réglé sur « émulateur », les voit aussi.
        let other = Target { rom_id: 13, emulator: "dolphin", layout: "gc" };
        set_scope(&dir, &other, Scope::Emulateur).unwrap();
        assert_eq!(current(&dir, &other).1, vec![Some(p)]);
        set_binds(&dir, &other, vec![Some(PlayerBinds::default())]).unwrap();
        assert_eq!(current(&dir, &t).1, vec![Some(PlayerBinds::default())]);
        // Retour à « ce jeu » : ses anciens réglages.
        set_scope(&dir, &t, Scope::Jeu).unwrap();
        assert_eq!(current(&dir, &t).0, Scope::Jeu);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dolphin_names() {
        assert_eq!(dolphin_pad("ly-"), Some("`Left Y+`"));
        if cfg!(windows) {
            assert_eq!(dolphin_keys("key:KeyW").as_deref(), Some("W"));
            assert_eq!(dolphin_keys("key:ShiftLeft").as_deref(), Some("LSHIFT"));
            assert_eq!(dolphin_keys("key:Numpad4").as_deref(), Some("NUMPAD4"));
            assert_eq!(dolphin_keys("mouse:right").as_deref(), Some("`Click 1`"));
        } else {
            assert_eq!(dolphin_keys("key:KeyW").as_deref(), Some("w"));
            assert_eq!(dolphin_keys("key:ShiftLeft").as_deref(), Some("`Shift_L`"));
        }
        assert_eq!(dolphin_keys("mouse:y-").as_deref(), Some("`Cursor Y-`"));
        assert_eq!(dolphin_keys("key:Inconnue"), None);
        // Chaque défaut se traduit.
        for b in gc_buttons() {
            assert!(dolphin_pad(b.pad).is_some() && dolphin_keys(b.keys).is_some(), "{}", b.id);
        }
    }
}
