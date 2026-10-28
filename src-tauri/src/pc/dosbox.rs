//! Touches personnalisées des jeux DOS lancés par DOSBox (versions GOG).
//!
//! DOSBox 0.74 lit un fichier de correspondances (`mapperfile`) qui remplace
//! entièrement ses associations par défaut : on l'écrit donc complet, défauts
//! compris, puis on y applique les choix du joueur. Un fichier de config
//! supplémentaire, passé en dernier `-conf`, pointe dessus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAPPER_FILE: &str = "farfalauncher-touches.map";
const CONF_FILE: &str = "farfalauncher-touches.conf";

/// Une touche du jeu (`target`, évènement DOSBox comme `key_up`) déclenchée
/// par une touche ou un bouton du joueur (`source`, comme `key 119` ou
/// `stick_0 button 0`). La source ne garde pas son rôle d'origine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyBind {
    pub target: String,
    pub source: String,
    /// Noms affichés (touches telles que nommées sur le clavier du joueur).
    #[serde(default)]
    pub target_label: Option<String>,
    #[serde(default)]
    pub source_label: Option<String>,
}

/// Exécutable DOSBox (le jeu est alors un jeu DOS).
pub fn is_dosbox(exe: &str) -> bool {
    Path::new(exe)
        .file_stem()
        .is_some_and(|s| s.to_string_lossy().to_ascii_lowercase().starts_with("dosbox"))
}

// Codes des touches de SDL 1.2 (disposition américaine : DOSBox traduit la
// position physique de la touche en ces codes).
const KEYS: &[(&str, u32)] = &[
    ("f1", 282), ("f2", 283), ("f3", 284), ("f4", 285), ("f5", 286), ("f6", 287),
    ("f7", 288), ("f8", 289), ("f9", 290), ("f10", 291), ("f11", 292), ("f12", 293),
    ("1", 49), ("2", 50), ("3", 51), ("4", 52), ("5", 53), ("6", 54), ("7", 55), ("8", 56), ("9", 57), ("0", 48),
    ("a", 97), ("b", 98), ("c", 99), ("d", 100), ("e", 101), ("f", 102), ("g", 103), ("h", 104), ("i", 105),
    ("j", 106), ("k", 107), ("l", 108), ("m", 109), ("n", 110), ("o", 111), ("p", 112), ("q", 113), ("r", 114),
    ("s", 115), ("t", 116), ("u", 117), ("v", 118), ("w", 119), ("x", 120), ("y", 121), ("z", 122),
    ("space", 32), ("esc", 27), ("equals", 61), ("grave", 96), ("tab", 9), ("enter", 13), ("bspace", 8),
    ("lbracket", 91), ("rbracket", 93), ("minus", 45), ("capslock", 301), ("semicolon", 59), ("quote", 39),
    ("backslash", 92), ("lshift", 304), ("rshift", 303), ("lalt", 308), ("ralt", 307), ("lctrl", 306),
    ("rctrl", 305), ("comma", 44), ("period", 46), ("slash", 47), ("printscreen", 316), ("scrolllock", 302),
    ("pause", 19), ("pagedown", 281), ("pageup", 280), ("insert", 277), ("home", 278), ("delete", 127),
    ("end", 279), ("up", 273), ("left", 276), ("down", 274), ("right", 275),
    ("kp_0", 256), ("kp_1", 257), ("kp_2", 258), ("kp_3", 259), ("kp_4", 260), ("kp_5", 261), ("kp_6", 262),
    ("kp_7", 263), ("kp_8", 264), ("kp_9", 265), ("numlock", 300), ("kp_divide", 267), ("kp_multiply", 268),
    ("kp_minus", 269), ("kp_plus", 270), ("kp_period", 266), ("kp_enter", 271), ("lessthan", 60),
];

/// Raccourcis de DOSBox (mod1 = Ctrl, mod2 = Alt).
const HANDLERS: &[(&str, &str)] = &[
    ("hand_shutdown", "key 290 mod1"),
    ("hand_capmouse", "key 291 mod1"),
    ("hand_fullscr", "key 13 mod2"),
    ("hand_mapper", "key 282 mod1"),
    ("hand_pause", "key 19 mod2"),
    ("hand_speedlock", "key 293 mod2"),
    ("hand_cycledown", "key 292 mod1"),
    ("hand_cycleup", "key 293 mod1"),
    ("hand_fskipdown", "key 288 mod1"),
    ("hand_fskipup", "key 289 mod1"),
    ("hand_scrshot", "key 286 mod1"),
    ("hand_swapimg", "key 285 mod1"),
    ("mod_1", "key 306"),
    ("mod_2", "key 308"),
];

/// Manette émulée par défaut : premier stick et deux premiers boutons.
const STICK: &[(&str, &str)] = &[
    ("jaxis_0_0-", "stick_0 axis 0 0"),
    ("jaxis_0_0+", "stick_0 axis 0 1"),
    ("jaxis_0_1-", "stick_0 axis 1 0"),
    ("jaxis_0_1+", "stick_0 axis 1 1"),
    ("jbutton_0_0", "stick_0 button 0"),
    ("jbutton_0_1", "stick_0 button 1"),
];

/// Contenu du fichier de correspondances.
fn mapper(binds: &[KeyBind]) -> String {
    let mut events: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, sym) in KEYS {
        events.insert(format!("key_{name}"), vec![format!("key {sym}")]);
    }
    for (name, bind) in HANDLERS {
        events.entry(name.to_string()).or_default().push(bind.to_string());
    }
    events.get_mut("mod_1").unwrap().push("key 305".into());
    events.get_mut("mod_2").unwrap().push("key 307".into());
    for (name, bind) in STICK {
        events.insert(name.to_string(), vec![bind.to_string()]);
    }
    for b in binds {
        for list in events.values_mut() {
            list.retain(|s| s != &b.source);
        }
    }
    for b in binds {
        let list = events.entry(b.target.clone()).or_default();
        if !list.contains(&b.source) {
            list.push(b.source.clone());
        }
    }
    let mut out = String::new();
    for (event, list) in events {
        out.push_str(&event);
        for bind in list {
            out.push_str(&format!(" \"{bind}\""));
        }
        out.push('\n');
    }
    out
}

/// Écrit (ou supprime, sans touche personnalisée) les fichiers du jeu.
/// `path_for` : chemin vu par DOSBox (Proton : chemin Windows).
pub fn write(game_dir: &Path, binds: &[KeyBind], path_for: impl Fn(&Path) -> String) -> std::io::Result<Option<PathBuf>> {
    let map = game_dir.join(MAPPER_FILE);
    let conf = game_dir.join(CONF_FILE);
    if binds.is_empty() {
        for f in [&map, &conf] {
            if f.exists() {
                std::fs::remove_file(f)?;
            }
        }
        return Ok(None);
    }
    std::fs::write(&map, mapper(binds))?;
    let mut text = format!("[sdl]\nmapperfile={}\n", path_for(&map));
    // Sans manette déclarée, DOSBox ignore les boutons associés.
    if binds.iter().any(|b| b.source.starts_with("stick_")) {
        text.push_str("\n[joystick]\njoysticktype=auto\n");
    }
    std::fs::write(&conf, text)?;
    Ok(Some(conf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_dosbox() {
        assert!(is_dosbox("DOSBOX/DOSBox.exe"));
        assert!(is_dosbox("dosbox-x.exe"));
        assert!(!is_dosbox("DOOM II.exe"));
    }

    #[test]
    fn remapped_source_leaves_its_default() {
        let text = mapper(&[KeyBind { target: "key_up".into(), source: "key 119".into(), target_label: None, source_label: None }]);
        assert!(text.contains("key_up \"key 273\" \"key 119\"\n"));
        assert!(text.contains("key_w\n"));
        assert!(text.contains("hand_fullscr \"key 13 mod2\"\n"));
        assert!(text.contains("mod_1 \"key 306\" \"key 305\"\n"));
    }

    #[test]
    fn writes_and_removes_files() {
        let dir = std::env::temp_dir().join(format!("farfa-dosbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binds = [KeyBind { target: "key_space".into(), source: "stick_0 button 0".into(), target_label: None, source_label: None }];
        let conf = write(&dir, &binds, |p| p.to_string_lossy().into_owned()).unwrap().unwrap();
        let text = std::fs::read_to_string(&conf).unwrap();
        assert!(text.contains("mapperfile=") && text.contains("joysticktype=auto"));
        assert!(write(&dir, &[], |p| p.to_string_lossy().into_owned()).unwrap().is_none());
        assert!(!conf.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
