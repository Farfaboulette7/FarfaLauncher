//! Attribution des manettes aux joueurs, posée dans la configuration de
//! l'émulateur ou de la recomp juste avant le lancement.
//!
//! Le format vient de la ligne `manettes:` du catalogue de FarfaRomManager
//! (`dolphin`, `punes`…). Les manettes sont celles que voit SDL (gamepad.rs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gamepad::PadInfo;
use crate::language::{expand, set_ini};

/// Manette choisie pour un joueur, gardée dans l'état du jeu installé.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerPad {
    /// Identifiant durable de la manette (voir `PadInfo::key`).
    pub key: String,
    /// Nom affiché quand elle n'est pas branchée.
    pub name: String,
}

/// Nombre de joueurs qu'un format sait attribuer (0 : format inconnu).
pub fn ports(format: &str) -> usize {
    match format_name(format) {
        "dolphin" | "punes" | "aurora" | "recharged" | "eden" => 4,
        "snes9x" => 2,
        "mgba" | "ppsspp" | "melonds" => 1,
        _ => 0,
    }
}

fn format_name(format: &str) -> &str {
    format.split('|').next().unwrap_or("").trim()
}

/// Réglages après le nom : `aurora | windows={documents}/X | linux={appdata}/Y`.
fn params(format: &str) -> Vec<(String, String)> {
    format
        .split('|')
        .skip(1)
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

/// Réglage pour ce système (`windows=`, `linux=`), sinon commun (`dir=`).
fn param_here<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let system = crate::language::current_system();
    params.iter().find(|(k, _)| k == system).or_else(|| params.iter().find(|(k, _)| k == name)).map(|(_, v)| v.as_str())
}

/// Manette de chaque port. Sans attribution enregistrée : ordre de branchement.
/// Une manette absente laisse sa case vide, sans faire remonter les suivantes.
pub fn resolve(players: &[Option<PlayerPad>], pads: &[PadInfo], ports: usize) -> Vec<Option<PadInfo>> {
    if players.iter().all(Option::is_none) {
        return pads.iter().take(ports).cloned().map(Some).collect();
    }
    let mut free: Vec<&PadInfo> = pads.iter().collect();
    let mut out: Vec<Option<PadInfo>> = vec![None; players.len().min(ports)];
    // D'abord la manette exacte, puis une du même modèle (autre prise, autre emplacement XInput).
    for same_model in [false, true] {
        for (i, wanted) in players.iter().take(ports).enumerate() {
            let Some(wanted) = wanted else { continue };
            if out[i].is_some() {
                continue;
            }
            let guid = wanted.key.split('/').next().unwrap_or("");
            if let Some(at) = free.iter().position(|p| if same_model { p.guid == guid } else { p.key == wanted.key }) {
                out[i] = Some(free.remove(at).clone());
            }
        }
    }
    out
}

/// Pose l'attribution pour ce format. `exe_dir` : dossier de l'exécutable.
pub fn apply(format: &str, players: &[Option<PlayerPad>], pads: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
    let ports = resolve(players, pads, ports(format));
    if ports.iter().all(Option::is_none) {
        return Ok(()); // aucune manette : la configuration du joueur reste telle quelle
    }
    match format_name(format) {
        "dolphin" => dolphin::apply(&ports, pads, exe_dir),
        "eden" => eden::apply(&ports, pads, exe_dir),
        "punes" => punes::apply(&ports, pads, exe_dir),
        "aurora" => aurora::apply(&ports, &params(format), exe_dir),
        "recharged" => recharged::apply(&ports, pads, exe_dir),
        "mgba" => mgba::apply(&ports, exe_dir),
        "ppsspp" => ppsspp::apply(&ports, exe_dir),
        "melonds" => melonds::apply(&ports, pads, exe_dir),
        "snes9x" => snes9x::apply(&ports, pads, exe_dir),
        other => Err(format!("format de manettes inconnu : {other}")),
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn write(path: &Path, before: &str, after: &str) -> Result<(), String> {
    if before == after {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, after).map_err(|e| format!("{}: {e}", path.display()))
}

fn path_for(windows: &str, linux: &str, exe_dir: &Path) -> Result<PathBuf, String> {
    let rel = if cfg!(windows) { windows } else { linux };
    expand(rel, exe_dir).ok_or_else(|| format!("dossier introuvable : {rel}"))
}

/// Rang d'une manette parmi celles qui lui ressemblent (`same`), dans l'ordre
/// où SDL et les émulateurs les énumèrent.
fn rank(pad: &PadInfo, all: &[PadInfo], same: impl Fn(&PadInfo) -> bool) -> usize {
    all.iter().filter(|p| same(p) && p.pad < pad.pad).count()
}

/// Rang XInput : Dolphin et puNES numérotent les emplacements occupés, sans trou.
fn xinput_rank(slot: u8, all: &[PadInfo]) -> usize {
    all.iter().filter(|p| p.xinput.is_some_and(|s| s < slot)).count()
}

/// Remplace (ou ajoute) toute une section d'un fichier INI.
fn replace_section(text: &str, section: &str, body: &[String]) -> String {
    let header = format!("[{section}]");
    let mut out: Vec<String> = Vec::new();
    let mut lines = text.lines().peekable();
    let mut done = false;
    while let Some(line) = lines.next() {
        if line.trim() == header && !done {
            out.push(header.clone());
            out.extend(body.iter().cloned());
            while lines.peek().is_some_and(|l| !l.trim().starts_with('[')) {
                lines.next();
            }
            done = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !done {
        out.push(header);
        out.extend(body.iter().cloned());
    }
    let mut s = out.join(if text.contains("\r\n") { "\r\n" } else { "\n" });
    s.push_str(if text.contains("\r\n") { "\r\n" } else { "\n" });
    s
}

fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut current = "";
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            current = &t[1..t.len() - 1];
        } else if current == section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim());
                }
            }
        }
    }
    None
}

mod dolphin {
    use super::*;

    /// Nom du périphérique tel que Dolphin l'écrit (`XInput/0/Gamepad`, `SDL/0/PS5 Controller`).
    pub fn device(pad: &PadInfo, all: &[PadInfo]) -> String {
        match pad.xinput {
            Some(slot) if cfg!(windows) => format!("XInput/{}/Gamepad", xinput_rank(slot, all)),
            _ => format!("SDL/{}/{}", rank(pad, all, |p| p.name == pad.name && !(cfg!(windows) && p.xinput.is_some())), pad.name),
        }
    }

    /// Manette GameCube complète. Noms de boutons communs à XInput et SDL
    /// (Dolphin fait correspondre « Button A » au bouton du bas des manettes SDL).
    fn gc(device: &str) -> Vec<String> {
        [
            ("Device", device),
            ("Buttons/A", "`Button A`"),
            ("Buttons/B", "`Button B`"),
            ("Buttons/X", "`Button X`"),
            ("Buttons/Y", "`Button Y`"),
            ("Buttons/Z", "`Shoulder R`"),
            ("Buttons/Start", "Start"),
            ("Main Stick/Up", "`Left Y+`"),
            ("Main Stick/Down", "`Left Y-`"),
            ("Main Stick/Left", "`Left X-`"),
            ("Main Stick/Right", "`Left X+`"),
            ("Main Stick/Calibration", "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42"),
            ("C-Stick/Up", "`Right Y+`"),
            ("C-Stick/Down", "`Right Y-`"),
            ("C-Stick/Left", "`Right X-`"),
            ("C-Stick/Right", "`Right X+`"),
            ("C-Stick/Calibration", "100.00 141.42 100.00 141.42 100.00 141.42 100.00 141.42"),
            ("Triggers/L", "`Trigger L`"),
            ("Triggers/R", "`Trigger R`"),
            ("Triggers/L-Analog", "`Trigger L`"),
            ("Triggers/R-Analog", "`Trigger R`"),
            ("D-Pad/Up", "`Pad N`"),
            ("D-Pad/Down", "`Pad S`"),
            ("D-Pad/Left", "`Pad W`"),
            ("D-Pad/Right", "`Pad E`"),
            ("Rumble/Motor", "`Motor L`|`Motor R`"),
        ]
        .iter()
        .map(|(k, v)| format!("{k} = {v}"))
        .collect()
    }

    /// Wiimote émulée tenue à l'horizontale (jeux de plateforme), sans extension.
    fn wiimote(device: &str) -> Vec<String> {
        [
            ("Device", device),
            ("Source", "1"),
            ("Buttons/A", "`Button B`"),
            ("Buttons/B", "`Shoulder R`"),
            ("Buttons/1", "`Button X`"),
            ("Buttons/2", "`Button A`"),
            ("Buttons/-", "Back"),
            ("Buttons/+", "Start"),
            ("Buttons/Home", "Guide"),
            ("D-Pad/Up", "`Pad N`"),
            ("D-Pad/Down", "`Pad S`"),
            ("D-Pad/Left", "`Pad W`"),
            ("D-Pad/Right", "`Pad E`"),
            ("IR/Up", "`Right Y+`"),
            ("IR/Down", "`Right Y-`"),
            ("IR/Left", "`Right X-`"),
            ("IR/Right", "`Right X+`"),
            ("Shake/X", "`Shoulder L`"),
            ("Shake/Y", "`Shoulder L`"),
            ("Shake/Z", "`Shoulder L`"),
            ("Options/Sideways Wiimote", "True"),
            ("Extension", "None"),
            ("Rumble/Motor", "`Motor L`|`Motor R`"),
        ]
        .iter()
        .map(|(k, v)| format!("{k} = {v}"))
        .collect()
    }

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        let dir = path_for("{exedir}/User/Config", "{config}/dolphin-emu", exe_dir)?;
        let (gc_path, wii_path, main_path) = (dir.join("GCPadNew.ini"), dir.join("WiimoteNew.ini"), dir.join("Dolphin.ini"));
        let (gc_before, wii_before, main_before) = (read(&gc_path), read(&wii_path), read(&main_path));
        let (mut gc_text, mut wii_text, mut main_text) = (gc_before.clone(), wii_before.clone(), main_before.clone());
        for i in 0..4 {
            let (gc_section, wii_section) = (format!("GCPad{}", i + 1), format!("Wiimote{}", i + 1));
            match ports.get(i).cloned().flatten() {
                Some(pad) => {
                    let dev = device(&pad, all);
                    // Même manette qu'avant : les réglages faits dans Dolphin sont gardés.
                    if ini_value(&gc_text, &gc_section, "Device") != Some(dev.as_str()) {
                        gc_text = replace_section(&gc_text, &gc_section, &gc(&dev));
                    }
                    if ini_value(&wii_text, &wii_section, "Device") != Some(dev.as_str()) {
                        wii_text = replace_section(&wii_text, &wii_section, &wiimote(&dev));
                    } else {
                        wii_text = set_ini(&wii_text, &format!("{wii_section}.Source"), "1");
                    }
                    main_text = set_ini(&main_text, &format!("Core.SIDevice{i}"), "6");
                }
                // Joueur 1 sans manette : le clavier par défaut de Dolphin reste.
                None if i == 0 => {}
                None => {
                    main_text = set_ini(&main_text, &format!("Core.SIDevice{i}"), "0");
                    if wii_text.contains(&format!("[{wii_section}]")) {
                        wii_text = set_ini(&wii_text, &format!("{wii_section}.Source"), "0");
                    }
                }
            }
        }
        write(&gc_path, &gc_before, &gc_text)?;
        write(&wii_path, &wii_before, &wii_text)?;
        write(&main_path, &main_before, &main_text)
    }
}

/// Eden et ses dérivés (Eglantine) : `user/config/qt-config.ini` en mode portable,
/// section [Controls], une entrée par bouton comme les écrit Eden quand on choisit
/// la manette dans ses réglages (SDL 3, comme le launcher).
mod eden {
    use super::*;

    /// Bouton Switch → élément de la manette SDL (disposition par position :
    /// A à droite, B en bas), comme `GetDefaultButtonBinding` d'Eden.
    const BUTTONS: [(&str, &str); 22] = [
        ("a", "east"),
        ("b", "south"),
        ("x", "north"),
        ("y", "west"),
        ("lstick", "leftstick"),
        ("rstick", "rightstick"),
        ("l", "leftshoulder"),
        ("r", "rightshoulder"),
        ("zl", "lefttrigger"),
        ("zr", "righttrigger"),
        ("plus", "start"),
        ("minus", "back"),
        ("dleft", "dpleft"),
        ("dup", "dpup"),
        ("dright", "dpright"),
        ("ddown", "dpdown"),
        ("slleft", "leftshoulder"),
        ("srleft", "rightshoulder"),
        ("home", "guide"),
        ("screenshot", "misc1"),
        ("slright", "leftshoulder"),
        ("srright", "rightshoulder"),
    ];

    /// GUID tel qu'Eden l'écrit : celui de SDL sans la somme du nom (octets 2 et 3).
    pub fn guid(pad: &PadInfo) -> String {
        let g = pad.guid.to_ascii_lowercase();
        if g.len() == 32 {
            format!("{}0000{}", &g[..4], &g[8..])
        } else {
            g
        }
    }

    fn source(pad: &PadInfo, all: &[PadInfo]) -> String {
        let g = guid(pad);
        format!("port:{},guid:{g}", rank(pad, all, |p| guid(p) == g))
    }

    fn input<'a>(pad: &'a PadInfo, element: &str) -> Option<&'a str> {
        pad.bindings.iter().find(|(k, _)| *k == element).map(|(_, v)| v.as_str())
    }

    fn axis(pad: &PadInfo, element: &str) -> Option<String> {
        input(pad, element)?.strip_prefix("axis:").map(str::to_string)
    }

    pub fn entries(pad: &PadInfo, all: &[PadInfo], player: usize) -> Vec<(String, String)> {
        let src = source(pad, all);
        let mut out = Vec::new();
        for (switch, element) in BUTTONS {
            let value = match input(pad, element) {
                // Gâchette analogique : appuyée à mi-course.
                Some(i) if i.starts_with("axis:") => format!("\"engine:sdl,{src},{i},threshold:0.500000,invert:+\""),
                Some(i) => format!("\"engine:sdl,{src},{i}\""),
                None => "\"\"".to_string(),
            };
            out.push((format!("player_{player}_button_{switch}"), value));
        }
        for (stick, x, y) in [("lstick", "leftx", "lefty"), ("rstick", "rightx", "righty")] {
            let value = match (axis(pad, x), axis(pad, y)) {
                (Some(ax), Some(ay)) => format!(
                    "\"engine:sdl,{src},axis_x:{ax},axis_y:{ay},offset_x:0.000000,offset_y:0.000000,invert_x:+,invert_y:+,deadzone:0.150000\""
                ),
                _ => "\"\"".to_string(),
            };
            out.push((format!("player_{player}_{stick}"), value));
        }
        out
    }

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        let path = path_for("{exedir}/user/config/qt-config.ini", "{exedir}/user/config/qt-config.ini", exe_dir)?;
        let before = read(&path);
        let mut text = before.clone();
        for i in 0..8 {
            let connected = format!("Controls.player_{i}_connected");
            match ports.get(i).cloned().flatten() {
                Some(pad) => {
                    // Même manette au même port qu'avant : les réglages faits dans Eden restent.
                    let same = ini_value(&text, "Controls", &format!("player_{i}_button_a"))
                        .is_some_and(|v| source(&pad, all).split(',').all(|part| v.split([',', '"']).any(|p| p == part)));
                    if !same {
                        for (key, value) in entries(&pad, all, i) {
                            text = set_ini(&text, &format!("Controls.{key}\\default"), "false");
                            text = set_ini(&text, &format!("Controls.{key}"), &value);
                        }
                    }
                    text = set_ini(&text, &format!("{connected}\\default"), "false");
                    text = set_ini(&text, &connected, "true");
                }
                // Joueur 1 sans manette : le clavier d'Eden reste.
                None if i == 0 => {}
                None => {
                    if ini_value(&text, "Controls", &format!("player_{i}_connected")).is_some_and(|v| v == "true") {
                        text = set_ini(&text, &format!("{connected}\\default"), "false");
                        text = set_ini(&text, &connected, "false");
                    }
                }
            }
        }
        write(&path, &before, &text)
    }
}

mod punes {
    use super::*;

    /// Identifiant puNES d'une manette sous Linux : bus, fabricant, produit et
    /// version tirés du GUID SDL, mêlés au numéro unique (js_guid_create de puNES).
    #[cfg_attr(windows, allow(dead_code))]
    pub fn linux_guid(pad: &PadInfo) -> Option<String> {
        let g: Vec<u8> = (0..16).map(|i| u8::from_str_radix(pad.guid.get(i * 2..i * 2 + 2)?, 16).ok()).collect::<Option<_>>()?;
        let word = |i: usize| u16::from_le_bytes([g[i], g[i + 1]]);
        let (bus, vendor, product, version) = (word(0), word(4), word(8), word(12));
        if vendor == 0 || product == 0 {
            return None;
        }
        let words = [
            bus.wrapping_sub(500),
            0u16.wrapping_sub(100),
            vendor,
            vendor.wrapping_sub(200),
            product,
            product.wrapping_sub(300),
            version,
            version.wrapping_sub(400),
        ];
        let mut bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        if let Some(uniq) = pad.key.split_once('/').map(|(_, u)| u).filter(|u| !u.starts_with('/') && !u.is_empty()) {
            let mut idx = 0;
            for c in uniq.bytes() {
                bytes[4 + idx] ^= c;
                idx = if idx >= 11 { 0 } else { idx + 1 };
            }
        }
        let w: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(format!("{{{:04X}{:04X}-{:04X}-{:04X}-{:04X}-{:04X}{:04X}{:04X}}}", w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]))
    }

    /// Identifiant puNES sous Windows : l'instance DirectInput de la manette.
    #[cfg(windows)]
    fn windows_guid(pad: &PadInfo, all: &[PadInfo]) -> Option<String> {
        let devices = super::dinput::devices();
        let xinput_ids: Vec<(u16, u16)> = all.iter().filter(|p| p.xinput.is_some()).map(|p| (p.vendor, p.product)).collect();
        let is_xinput = |d: &super::dinput::Device| xinput_ids.contains(&(d.vendor, d.product)) || d.name.to_ascii_lowercase().contains("xbox");
        let found = match pad.xinput {
            // puNES donne les emplacements XInput occupés, dans l'ordre, à ses manettes XInput.
            Some(slot) => devices.iter().filter(|d| is_xinput(d)).nth(xinput_rank(slot, all)),
            None => {
                let n = rank(pad, all, |p| p.xinput.is_none() && p.vendor == pad.vendor && p.product == pad.product);
                devices.iter().filter(|d| !is_xinput(d) && d.vendor == pad.vendor && d.product == pad.product).nth(n)
            }
        };
        found.map(|d| d.guid.clone())
    }

    fn guid(pad: &PadInfo, all: &[PadInfo]) -> Option<String> {
        #[cfg(windows)]
        return windows_guid(pad, all);
        #[cfg(not(windows))]
        {
            let _ = all;
            linux_guid(pad)
        }
    }

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        let path = path_for("{exedir}/input.cfg", "{config}/puNES/input.cfg", exe_dir)?;
        let before = read(&path);
        let mut text = before.clone();
        let mut players = 0;
        for i in 0..4 {
            let n = i + 1;
            let pad = ports.get(i).cloned().flatten();
            let id = pad.as_ref().and_then(|p| guid(p, all));
            if pad.is_some() && id.is_none() {
                tracing::warn!("puNES : manette du joueur {n} introuvable dans DirectInput");
            }
            if pad.is_some() {
                players = n;
            }
            text = set_ini(&text, &format!("port {n}.P{n}J GUID"), id.as_deref().unwrap_or("NULL"));
            // Joueur 1 : toujours branché (clavier sinon).
            let on = pad.is_some() || i == 0;
            text = set_ini(&text, &format!("port {n}.controller {n}"), if on { "standard" } else { "disable" });
        }
        text = set_ini(&text, "system.controller mode", if players > 2 { "fourscore" } else { "nes" });
        write(&path, &before, &text)
    }
}

/// PPSSPP : ses touches de manette visent un emplacement XInput (appareils
/// 20 à 23 de `controls.ini`). On les place sur celui de la manette choisie.
mod ppsspp {
    use super::*;

    /// Remplace l'appareil XInput (`20-96` → `21-96`) dans chaque association.
    pub fn retarget(text: &str, slot: u8) -> String {
        let device = 20 + u32::from(slot);
        let lines: Vec<String> = text
            .lines()
            .map(|line| {
                let Some((key, values)) = line.split_once('=') else { return line.to_string() };
                let mapped: Vec<String> = values
                    .trim()
                    .split(',')
                    .map(|entry| {
                        // Une touche, ou une combinaison « a:b » : chaque partie a son appareil.
                        entry
                            .split(':')
                            .map(|part| match part.split_once('-') {
                                Some(("20" | "21" | "22" | "23", code)) => format!("{device}-{code}"),
                                _ => part.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join(":")
                    })
                    .collect();
                if values.trim().is_empty() {
                    line.to_string()
                } else {
                    format!("{} = {}", key.trim_end(), mapped.join(","))
                }
            })
            .collect();
        join_lines_like(lines, text)
    }

    pub fn apply(ports: &[Option<PadInfo>], exe_dir: &Path) -> Result<(), String> {
        let Some(Some(pad)) = ports.first() else { return Ok(()) };
        // Manette hors XInput (PlayStation, Switch) : PPSSPP la lit déjà par ses autres voies.
        let Some(slot) = pad.xinput else { return Ok(()) };
        let path = path_for("{exedir}/memstick/PSP/SYSTEM/controls.ini", "{config}/ppsspp/PSP/SYSTEM/controls.ini", exe_dir)?;
        // Pas encore de controls.ini : PPSSPP l'écrit à sa première fermeture, avec l'emplacement 1.
        if slot == 0 && !path.is_file() {
            return Ok(());
        }
        let before = read(&path);
        if before.is_empty() {
            return Ok(());
        }
        write(&path, &before, &retarget(&before, slot))
    }
}

fn join_lines_like(lines: Vec<String>, original: &str) -> String {
    let nl = if original.contains("\r\n") { "\r\n" } else { "\n" };
    let mut s = lines.join(nl);
    if original.ends_with('\n') {
        s.push_str(nl);
    }
    s
}

/// mGBA : ses réglages par défaut n'associent à une manette que le stick et
/// la croix. On y ajoute les boutons, dans le profil de la manette (par nom).
mod mgba {
    use super::*;

    /// Numéros de boutons SDL 2 (API joystick, celle de mGBA) : manettes Xbox
    /// (XInput) d'un côté, manettes lues par HIDAPI (PlayStation, Switch) de l'autre.
    fn buttons(layout: &str) -> [(&'static str, &'static str); 6] {
        if layout == "xbox" || layout == "generic" {
            [("keyA", "1"), ("keyB", "0"), ("keyL", "4"), ("keyR", "5"), ("keySelect", "6"), ("keyStart", "7")]
        } else {
            [("keyA", "1"), ("keyB", "0"), ("keyL", "9"), ("keyR", "10"), ("keySelect", "4"), ("keyStart", "6")]
        }
    }

    pub fn apply(ports: &[Option<PadInfo>], exe_dir: &Path) -> Result<(), String> {
        let Some(Some(pad)) = ports.first() else { return Ok(()) };
        // Version portable (portable.ini) : config.ini à côté de l'exécutable.
        let path = if exe_dir.join("portable.ini").is_file() {
            exe_dir.join("config.ini")
        } else {
            path_for("{appdata}/mGBA/config.ini", "{config}/mgba/config.ini", exe_dir)?
        };
        let before = read(&path);
        let mut text = before.clone();
        // GBA d'un côté, Game Boy et Game Boy Color de l'autre : chacun ses sections.
        let gba_profile = format!("gba.input-profile.{}", pad.name);
        let gb_profile = format!("gb.input-profile.{}", pad.name);
        for section in ["gba.input.SDLB", gba_profile.as_str(), "gb.input.SDLB", gb_profile.as_str()] {
            // Boutons déjà choisis dans mGBA : on n'y touche pas.
            if ini_value(&text, section, "keyA").is_some_and(|v| v != "-1") {
                continue;
            }
            for (key, value) in buttons(pad.layout) {
                text = set_ini(&text, &format!("{section}.{key}"), value);
            }
        }
        write(&path, &before, &text)
    }
}

/// Bouton d'une console à la place du même bouton de la manette (A à droite, B en bas…).
const NINTENDO_FACE: [(&str, &str); 8] = [
    ("A", "east"),
    ("B", "south"),
    ("X", "north"),
    ("Y", "west"),
    ("L", "leftshoulder"),
    ("R", "rightshoulder"),
    ("Select", "back"),
    ("Start", "start"),
];
const DPAD: [(&str, &str); 4] = [("Up", "dpup"), ("Down", "dpdown"), ("Left", "dpleft"), ("Right", "dpright")];

fn binding<'a>(pad: &'a PadInfo, element: &str) -> Option<&'a str> {
    pad.bindings.iter().find(|(k, _)| *k == element).map(|(_, v)| v.as_str())
}

/// Croix lue comme un « hat » SDL : (numéro, direction).
fn hat(input: &str) -> Option<(i64, &str)> {
    let (h, d) = input.strip_prefix("hat:")?.split_once(",direction:")?;
    Some((h.parse().ok()?, d))
}

/// melonDS : `JoystickID` et `[Instance0.Joystick]` de melonDS.toml, en numéros SDL bruts
/// (bouton N ; croix 0x100 | hat << 4 | sens ; axe 0x10000 | axe << 24 | sens << 20).
mod melonds {
    use super::*;

    pub fn code(input: &str) -> Option<i64> {
        if let Some(b) = input.strip_prefix("button:") {
            return b.parse().ok();
        }
        if let Some((h, d)) = hat(input) {
            let dir = match d {
                "up" => 1,
                "right" => 2,
                "down" => 4,
                "left" => 8,
                _ => return None,
            };
            return Some(0x100 | (h << 4) | dir);
        }
        let axis: i64 = input.strip_prefix("axis:")?.split(',').next()?.parse().ok()?;
        // Gâchette : sens 2, « appuyée » dès qu'elle quitte le repos. 0xFFFF dans les 16 bits du
        // bas : aucun bouton (sinon melonDS lit aussi le bouton 0, A d'une manette Xbox).
        Some(0xFFFF | 0x10000 | (axis << 24) | (2 << 20))
    }

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        let Some(Some(pad)) = ports.first() else { return Ok(()) };
        // Dossier « portable » à côté de l'exécutable : la configuration y est.
        let path = if exe_dir.join("portable").is_dir() {
            exe_dir.join("portable").join("melonDS.toml")
        } else {
            path_for("{config}/melonDS/melonDS.toml", "{config}/melonDS/melonDS.toml", exe_dir)?
        };
        let before = read(&path);
        let mut text = set_ini(&before, "Instance0.JoystickID", &rank(pad, all, |_| true).to_string());
        for (key, element) in NINTENDO_FACE.iter().chain(DPAD.iter()) {
            if let Some(c) = binding(pad, element).and_then(code) {
                text = set_ini(&text, &format!("Instance0.Joystick.{key}"), &c.to_string());
            }
        }
        // Gâchette droite : les deux écrans échangent leur place et leur taille (le grand
        // reste à gauche). Deux raccourcis de melonDS sur la même gâchette.
        if let Some(c) = binding(pad, "righttrigger").and_then(code) {
            for hotkey in ["HK_SwapScreens", "HK_SwapScreenEmphasis"] {
                text = set_ini(&text, &format!("Instance0.Joystick.{hotkey}"), &c.to_string());
            }
        }
        write(&path, &before, &text)
    }
}

/// Numéros que donne l'interface manettes de Windows (winmm) : ils peuvent sauter des cases
/// (une manette Xbox seule peut être la n° 1). La manette y est reconnue à son fabricant et
/// son produit ; à défaut, son rang parmi les manettes branchées.
fn winmm_index(pad: &PadInfo, all: &[PadInfo]) -> usize {
    #[cfg(windows)]
    {
        let same: Vec<usize> = winmm::devices().into_iter().filter(|d| d.1 == pad.vendor && d.2 == pad.product).map(|d| d.0).collect();
        let n = rank(pad, all, |p| p.vendor == pad.vendor && p.product == pad.product);
        if let Some(id) = same.get(n) {
            return *id;
        }
    }
    rank(pad, all, |_| true)
}

#[cfg(windows)]
mod winmm {
    /// JOYCAPSW : seuls le fabricant et le produit servent ici.
    #[repr(C)]
    struct JoyCaps {
        mid: u16,
        pid: u16,
        name: [u16; 32],
        fields: [u32; 19],
        reg_key: [u16; 32],
        oem: [u16; 260],
    }

    #[link(name = "winmm")]
    extern "system" {
        fn joyGetDevCapsW(id: usize, caps: *mut JoyCaps, size: u32) -> u32;
    }

    /// (numéro, fabricant, produit) des manettes que voit Windows.
    pub fn devices() -> Vec<(usize, u16, u16)> {
        let mut out = Vec::new();
        for id in 0..16 {
            let mut caps = JoyCaps { mid: 0, pid: 0, name: [0; 32], fields: [0; 19], reg_key: [0; 32], oem: [0; 260] };
            // SAFETY : structure de la taille annoncée, remplie par winmm.
            let r = unsafe { joyGetDevCapsW(id, &mut caps, std::mem::size_of::<JoyCaps>() as u32) };
            if r == 0 {
                out.push((id, caps.mid, caps.pid));
            }
        }
        out
    }
}

/// Snes9x : manettes 1 et 2. Sous Windows, `[Controls\Win]` de snes9x.conf avec l'interface
/// manettes de Windows (`(J0)Button 1`, `(J0)POV Up`) ; sous Linux, `[Joypad N]` du
/// snes9x.conf de la version GTK avec les numéros SDL (`Joystick 1 Button 1`).
mod snes9x {
    use super::*;

    const SECTION: &str = "Controls\\Win";

    /// Numéro de bouton de l'interface manettes de Windows, selon la famille de la manette.
    pub fn winmm_button(layout: &str, element: &str) -> Option<u8> {
        let order: [&str; 10] = match layout {
            "playstation" => ["west", "south", "east", "north", "leftshoulder", "rightshoulder", "lt", "rt", "back", "start"],
            "nintendo" => ["south", "east", "west", "north", "leftshoulder", "rightshoulder", "lt", "rt", "back", "start"],
            _ => ["south", "east", "west", "north", "leftshoulder", "rightshoulder", "back", "start", "leftstick", "rightstick"],
        };
        order.iter().position(|e| *e == element).map(|i| i as u8)
    }

    /// Entrée SDL écrite comme la lit Snes9x GTK. La croix (hat) y est un axe virtuel
    /// placé après les vrais axes : haut/bas sur le premier, gauche/droite sur le second.
    pub fn gtk_input(pad: &PadInfo, device: usize, input: &str) -> Option<String> {
        if let Some(b) = input.strip_prefix("button:") {
            return Some(format!("Joystick {device} Button {b}"));
        }
        if let Some((h, d)) = hat(input) {
            let base = i64::from(pad.axes) + h * 2;
            let (axis, sign) = match d {
                "up" => (base, '+'),
                "down" => (base, '-'),
                "right" => (base + 1, '+'),
                "left" => (base + 1, '-'),
                _ => return None,
            };
            return Some(format!("Joystick {device} Axis {axis} {sign} 50%"));
        }
        let axis = input.strip_prefix("axis:")?.split(',').next()?;
        Some(format!("Joystick {device} Axis {axis} + 50%"))
    }

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        if cfg!(windows) {
            let path = exe_dir.join("snes9x.conf");
            let before = read(&path);
            let mut text = before.clone();
            for i in 0..2 {
                let n = i + 1;
                let Some(Some(pad)) = ports.get(i) else {
                    // Joueur 2 sans manette : son port reste fermé. Le joueur 1 garde le clavier.
                    if n > 1 {
                        text = set_ini(&text, &format!("{SECTION}.Joypad{n}:Enabled"), "FALSE");
                    }
                    continue;
                };
                let j = format!("(J{:x})", winmm_index(pad, all));
                text = set_ini(&text, &format!("{SECTION}.Joypad{n}:Enabled"), "TRUE");
                for (key, element) in NINTENDO_FACE {
                    if let Some(b) = winmm_button(pad.layout, element) {
                        text = set_ini(&text, &format!("{SECTION}.Joypad{n}:{key}"), &format!("{j}Button {b}"));
                    }
                }
                for dir in ["Up", "Down", "Left", "Right"] {
                    text = set_ini(&text, &format!("{SECTION}.Joypad{n}:{dir}"), &format!("{j}POV {dir}"));
                }
            }
            return write(&path, &before, &text);
        }
        let path = path_for("{config}/snes9x/snes9x.conf", "{config}/snes9x/snes9x.conf", exe_dir)?;
        let before = read(&path);
        let mut text = before.clone();
        for (i, port) in ports.iter().enumerate() {
            let Some(pad) = port else { continue };
            // Snes9x GTK numérote les manettes à partir de 1, dans l'ordre de SDL.
            let device = rank(pad, all, |_| true) + 1;
            for (key, element) in NINTENDO_FACE.iter().chain(DPAD.iter()) {
                if let Some(v) = binding(pad, element).and_then(|input| gtk_input(pad, device, input)) {
                    text = set_ini(&text, &format!("Joypad {i}.{key}"), &v);
                }
            }
        }
        write(&path, &before, &text)
    }
}

/// Recharged (Mario Strikers Charged) : `[controls] player1..4` de
/// `recharged.ini`, `gamepadN` étant la N-ième manette que liste SDL.
mod recharged {
    use super::*;

    pub fn apply(ports: &[Option<PadInfo>], all: &[PadInfo], exe_dir: &Path) -> Result<(), String> {
        let path = exe_dir.join("recharged.ini");
        // Pas encore de recharged.ini : le programme le crée à son premier lancement,
        // un fichier partiel le priverait de ses réglages par défaut.
        if !path.is_file() {
            return Ok(());
        }
        let before = read(&path);
        let mut text = before.clone();
        for i in 0..4 {
            let value = match ports.get(i).cloned().flatten() {
                Some(pad) => format!("gamepad{}", rank(&pad, all, |_| true) + 1),
                None if i == 0 => "keyboard".to_string(),
                None => "off".to_string(),
            };
            text = set_ini(&text, &format!("controls.player{}", i + 1), &value);
        }
        write(&path, &before, &text)
    }
}

/// Recomps bâties sur aurora (Melee, NSMBW…) : `controller_ports.dat` dans
/// le dossier utilisateur, une manette (GUID SDL et numéro de série) par port.
mod aurora {
    use super::*;

    const MAGIC: &[u8; 4] = b"CPRT";
    const UNSET: u8 = 0;
    const NONE: u8 = 1;
    const CONTROLLER: u8 = 2;

    /// Numéro de série tel qu'aurora le compare : minuscules, sans deux-points ni tirets.
    fn normalize(serial: &str) -> String {
        serial.chars().filter(|c| *c != ':' && *c != '-').map(|c| c.to_ascii_lowercase()).collect()
    }

    fn string(out: &mut Vec<u8>, s: &str) {
        let bytes = &s.as_bytes()[..s.len().min(256)];
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
    }

    /// Version et fin de fichier (réglages de vibration, version 3) d'un fichier existant.
    fn existing(data: &[u8]) -> Option<(u32, Vec<u8>)> {
        if data.len() < 8 || &data[..4] != MAGIC {
            return None;
        }
        let version = u32::from_le_bytes(data[4..8].try_into().ok()?);
        if version != 2 && version != 3 {
            return None;
        }
        let mut at = 8;
        for _ in 0..4 {
            at += 1;
            for _ in 0..2 {
                let len = u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
                at += 4 + len;
            }
        }
        Some((version, data.get(at..)?.to_vec()))
    }

    pub fn encode(ports: &[Option<PadInfo>], before: &[u8]) -> Vec<u8> {
        // Version 2 : lue par toutes les versions d'aurora vues jusqu'ici.
        let (version, tail) = existing(before).unwrap_or((2, Vec::new()));
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&version.to_le_bytes());
        for i in 0..4 {
            let (state, guid, serial) = match ports.get(i) {
                Some(Some(pad)) => (CONTROLLER, pad.guid.clone(), normalize(pad.serial.as_deref().unwrap_or(""))),
                // Joueur sans manette : port fermé, pour qu'une autre ne le prenne pas.
                Some(None) => (NONE, String::new(), String::new()),
                None => (UNSET, String::new(), String::new()),
            };
            out.push(state);
            string(&mut out, &guid);
            string(&mut out, &serial);
        }
        out.extend_from_slice(&tail);
        out
    }

    pub fn apply(ports: &[Option<PadInfo>], params: &[(String, String)], exe_dir: &Path) -> Result<(), String> {
        // Version portable (portable.txt à côté de l'exécutable) : tout reste à côté.
        let dir = if exe_dir.join("portable.txt").is_file() {
            exe_dir.to_path_buf()
        } else {
            let rel = param_here(params, "dir").ok_or("aurora : dossier utilisateur non indiqué")?;
            expand(rel, exe_dir).ok_or_else(|| format!("dossier introuvable : {rel}"))?
        };
        let path = dir.join("controller_ports.dat");
        let before = std::fs::read(&path).unwrap_or_default();
        let after = encode(ports, &before);
        if before == after {
            return Ok(());
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, after).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Manettes vues par DirectInput (puNES sous Windows), dans son ordre d'énumération.
#[cfg(windows)]
mod dinput {
    use windows::core::{Interface, BOOL};
    use windows::Win32::Devices::HumanInterfaceDevice::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    pub struct Device {
        /// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, comme l'écrit puNES.
        pub guid: String,
        pub vendor: u16,
        pub product: u16,
        pub name: String,
    }

    unsafe extern "system" fn found(dev: *mut DIDEVICEINSTANCEW, list: *mut core::ffi::c_void) -> BOOL {
        // SAFETY : pointeurs fournis par EnumDevices ; `list` est le Vec passé plus bas.
        let (dev, list) = unsafe { (&*dev, &mut *(list as *mut Vec<Device>)) };
        let g = dev.guidInstance;
        let d = g.data4;
        let name_len = dev.tszProductName.iter().position(|&c| c == 0).unwrap_or(dev.tszProductName.len());
        list.push(Device {
            guid: format!(
                "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
                g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            ),
            // guidProduct.Data1 = MAKELONG(fabricant, produit).
            vendor: (dev.guidProduct.data1 & 0xffff) as u16,
            product: (dev.guidProduct.data1 >> 16) as u16,
            name: String::from_utf16_lossy(&dev.tszProductName[..name_len]),
        });
        BOOL(1) // DIENUM_CONTINUE
    }

    pub fn devices() -> Vec<Device> {
        let mut list: Vec<Device> = Vec::new();
        // SAFETY : appels COM documentés ; l'interface est libérée à la fin du bloc.
        unsafe {
            let Ok(module) = GetModuleHandleW(None) else { return list };
            let mut raw = std::ptr::null_mut();
            if DirectInput8Create(module.into(), DIRECTINPUT_VERSION, &IDirectInput8W::IID, &mut raw, None).is_err() || raw.is_null() {
                return list;
            }
            let di = IDirectInput8W::from_raw(raw);
            let _ = di.EnumDevices(DI8DEVCLASS_GAMECTRL, Some(found), &mut list as *mut Vec<Device> as *mut _, DIEDFL_ALLDEVICES);
        }
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(pad: usize, name: &str, guid: &str, place: &str, xinput: Option<u8>) -> PadInfo {
        PadInfo {
            pad,
            name: name.into(),
            layout: "xbox",
            key: format!("{guid}/{place}"),
            guid: guid.into(),
            vendor: 0x045e,
            product: 0x028e,
            xinput,
            serial: None,
            order: pad as u64,
            bindings: Vec::new(),
            axes: 6,
        }
    }

    #[test]
    fn ppsspp_slot() {
        let t = "\u{feff}[ControlMapping]\r\nUp = 1-19,20-19,10-19\r\nCross = 1-54,20-96,10-189\r\nPause = 1-111,20-4034,20-3:20-4046\r\n";
        let got = ppsspp::retarget(t, 2);
        assert_eq!(got, "\u{feff}[ControlMapping]\r\nUp = 1-19,22-19,10-19\r\nCross = 1-54,22-96,10-189\r\nPause = 1-111,22-4034,22-3:22-4046\r\n");
        assert_eq!(ppsspp::retarget(&got, 0), t);
    }

    fn xbox_bindings() -> Vec<(&'static str, String)> {
        vec![
            ("south", "button:0".into()),
            ("east", "button:1".into()),
            ("west", "button:2".into()),
            ("north", "button:3".into()),
            ("leftshoulder", "button:4".into()),
            ("rightshoulder", "button:5".into()),
            ("back", "button:6".into()),
            ("start", "button:7".into()),
            ("dpup", "hat:0,direction:up".into()),
            ("dpdown", "hat:0,direction:down".into()),
            ("dpleft", "hat:0,direction:left".into()),
            ("dpright", "hat:0,direction:right".into()),
            ("lefttrigger", "axis:2".into()),
            ("righttrigger", "axis:5".into()),
        ]
    }

    #[test]
    fn melonds_codes() {
        assert_eq!(melonds::code("button:1"), Some(1));
        assert_eq!(melonds::code("hat:0,direction:up"), Some(0x101));
        assert_eq!(melonds::code("hat:0,direction:left"), Some(0x108));
        assert_eq!(melonds::code("axis:2"), Some(0xFFFF | 0x10000 | (2 << 24) | (2 << 20)));
        let dir = std::env::temp_dir().join(format!("farfa-melonds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("portable")).unwrap();
        std::fs::write(dir.join("portable").join("melonDS.toml"), "[Instance0.Firmware]\nLanguage = 2\n").unwrap();
        let other = pad(1, "Autre", "o", "o0", Some(0));
        let mut x = pad(2, "Xbox", "g", "x1", Some(1));
        x.bindings = xbox_bindings();
        apply("melonds", &[Some(PlayerPad { key: x.key.clone(), name: x.name.clone() })], &[other, x], &dir).unwrap();
        let toml = read(&dir.join("portable").join("melonDS.toml"));
        assert_eq!(ini_value(&toml, "Instance0", "JoystickID"), Some("1"));
        assert_eq!(ini_value(&toml, "Instance0.Joystick", "A"), Some("1"));
        assert_eq!(ini_value(&toml, "Instance0.Joystick", "B"), Some("0"));
        assert_eq!(ini_value(&toml, "Instance0.Joystick", "Up"), Some("257"));
        assert_eq!(ini_value(&toml, "Instance0.Firmware", "Language"), Some("2"));
        let rt = (0xFFFF | 0x10000 | (5 << 24) | (2 << 20)).to_string();
        assert_eq!(ini_value(&toml, "Instance0.Joystick", "HK_SwapScreens"), Some(rt.as_str()));
        assert_eq!(ini_value(&toml, "Instance0.Joystick", "HK_SwapScreenEmphasis"), Some(rt.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snes9x_formats() {
        // Windows : Xbox A en bas (Button 0) = B de la Super Nintendo.
        assert_eq!(snes9x::winmm_button("xbox", "south"), Some(0));
        assert_eq!(snes9x::winmm_button("xbox", "start"), Some(7));
        assert_eq!(snes9x::winmm_button("playstation", "south"), Some(1));
        assert_eq!(snes9x::winmm_button("playstation", "start"), Some(9));
        // Linux : la croix suit les 6 axes d'une manette Xbox.
        let mut x = pad(1, "Xbox", "g", "x0", Some(0));
        x.bindings = xbox_bindings();
        assert_eq!(snes9x::gtk_input(&x, 1, "hat:0,direction:up").as_deref(), Some("Joystick 1 Axis 6 + 50%"));
        assert_eq!(snes9x::gtk_input(&x, 1, "hat:0,direction:left").as_deref(), Some("Joystick 1 Axis 7 - 50%"));
        assert_eq!(snes9x::gtk_input(&x, 2, "button:1").as_deref(), Some("Joystick 2 Button 1"));
        if cfg!(windows) {
            let dir = std::env::temp_dir().join(format!("farfa-snes9x-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("snes9x.conf"), "[Controls\\Win]\n     Joypad1:Enabled              = TRUE\n     Joypad1:A                    = V\n").unwrap();
            apply("snes9x", &[], &[x], &dir).unwrap();
            let conf = read(&dir.join("snes9x.conf"));
            assert_eq!(ini_value(&conf, "Controls\\Win", "Joypad1:A"), Some("(J0)Button 1"));
            assert_eq!(ini_value(&conf, "Controls\\Win", "Joypad1:Up"), Some("(J0)POV Up"));
            assert_eq!(ini_value(&conf, "Controls\\Win", "Joypad2:Enabled"), Some("FALSE"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn mgba_buttons() {
        let dir = std::env::temp_dir().join(format!("farfa-mgba-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("portable.ini"), "").unwrap();
        std::fs::write(dir.join("config.ini"), "[gba.input.SDLB]\nkeyA=-1\nhat0Up=6\n\n[gba.input-profile.Xbox One Controller]\nkeyA=-1\nkeyB=-1\n\n[ports.qt]\nfullscreen=0\n").unwrap();
        let x = pad(1, "Xbox One Controller", "g", "x0", Some(0));
        apply("mgba", &[], &[x], &dir).unwrap();
        let ini = read(&dir.join("config.ini"));
        assert_eq!(ini_value(&ini, "gba.input-profile.Xbox One Controller", "keyA"), Some("1"));
        assert_eq!(ini_value(&ini, "gba.input-profile.Xbox One Controller", "keyStart"), Some("7"));
        assert_eq!(ini_value(&ini, "gba.input.SDLB", "keyB"), Some("0"));
        assert_eq!(ini_value(&ini, "gba.input.SDLB", "hat0Up"), Some("6"));
        // Choix fait dans mGBA : gardé.
        let tuned = ini.replace("[gba.input.SDLB]\nkeyA=1", "[gba.input.SDLB]\nkeyA=3");
        std::fs::write(dir.join("config.ini"), &tuned).unwrap();
        let x = pad(1, "Xbox One Controller", "g", "x0", Some(0));
        apply("mgba", &[], &[x], &dir).unwrap();
        assert_eq!(ini_value(&read(&dir.join("config.ini")), "gba.input.SDLB", "keyA"), Some("3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recharged_players() {
        let dir = std::env::temp_dir().join(format!("farfa-recharged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("recharged.ini"), "[game]\r\ndisc = game/R4QE01.rvz\r\n[controls]\r\nplayer1 = keyboard\r\nplayer2 = off\r\nplayer3 = off\r\nplayer4 = off\r\nrumble = true\r\n").unwrap();
        let a = pad(4, "Xbox", "g1", "x0", Some(0));
        let b = pad(9, "PS5", "g2", "s", None);
        let players = vec![Some(PlayerPad { key: b.key.clone(), name: String::new() }), Some(PlayerPad { key: a.key.clone(), name: String::new() })];
        apply("recharged", &players, &[a, b], &dir).unwrap();
        let ini = read(&dir.join("recharged.ini"));
        assert_eq!(ini_value(&ini, "controls", "player1"), Some("gamepad2"));
        assert_eq!(ini_value(&ini, "controls", "player2"), Some("gamepad1"));
        assert_eq!(ini_value(&ini, "controls", "player3"), Some("off"));
        assert_eq!(ini_value(&ini, "game", "disc"), Some("game/R4QE01.rvz"));
        assert_eq!(ini_value(&ini, "controls", "rumble"), Some("true"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aurora_file() {
        let mut ps5 = pad(2, "PS5 Controller", "0300aa", "AA:BB", None);
        ps5.serial = Some("AA:BB-CC".into());
        let xbox = pad(1, "Xbox", "0300bb", "xinput0", Some(0));
        let data = aurora::encode(&[Some(ps5), None, Some(xbox)], &[]);
        let mut want = b"CPRT".to_vec();
        want.extend_from_slice(&2u32.to_le_bytes());
        want.push(2);
        want.extend_from_slice(&6u32.to_le_bytes());
        want.extend_from_slice(b"0300aa");
        want.extend_from_slice(&6u32.to_le_bytes());
        want.extend_from_slice(b"aabbcc");
        want.push(1);
        want.extend_from_slice(&[0; 8]);
        want.push(2);
        want.extend_from_slice(&6u32.to_le_bytes());
        want.extend_from_slice(b"0300bb");
        want.extend_from_slice(&0u32.to_le_bytes());
        want.push(0);
        want.extend_from_slice(&[0; 8]);
        assert_eq!(data, want);
        // Fichier de Melee (version 3, vibration à la fin) : version et vibration gardées.
        let mut v3 = want.clone();
        v3[4] = 3;
        v3.extend_from_slice(&[1, 2, 3, 4]);
        let again = aurora::encode(&[None], &v3);
        assert_eq!(&again[4..8], &3u32.to_le_bytes());
        assert!(again.ends_with(&[1, 2, 3, 4]));
        assert_eq!(
            params("aurora | windows={documents}/M | dir={appdata}/x"),
            vec![("windows".to_string(), "{documents}/M".to_string()), ("dir".to_string(), "{appdata}/x".to_string())]
        );
    }

    #[test]
    fn resolve_by_key_then_model() {
        let a = pad(1, "Xbox 360 Controller", "g1", "xinput0", Some(0));
        let b = pad(2, "PS5 Controller", "g2", "SERIAL", None);
        let c = pad(3, "Xbox 360 Controller", "g1", "xinput1", Some(1));
        let pads = vec![a.clone(), b.clone(), c.clone()];
        // Sans attribution : ordre de branchement.
        assert_eq!(resolve(&[], &pads, 2), vec![Some(a.clone()), Some(b.clone())]);
        let want = |k: &str| Some(PlayerPad { key: k.into(), name: String::new() });
        // Joueur 1 = PS5, joueur 2 = absent, joueur 3 = Xbox branchée ailleurs.
        let got = resolve(&[want("g2/SERIAL"), want("g9/x"), want("g1/xinput3")], &pads, 4);
        assert_eq!(got, vec![Some(b), None, Some(a)]);
    }

    #[test]
    fn dolphin_devices() {
        let a = pad(5, "Xbox 360 Controller", "g1", "xinput2", Some(2));
        let b = pad(7, "PS5 Controller", "g2", "s1", None);
        let c = pad(9, "PS5 Controller", "g2", "s2", None);
        let all = vec![a.clone(), b.clone(), c.clone()];
        if cfg!(windows) {
            assert_eq!(dolphin::device(&a, &all), "XInput/0/Gamepad");
        }
        assert_eq!(dolphin::device(&b, &all), "SDL/0/PS5 Controller");
        assert_eq!(dolphin::device(&c, &all), "SDL/1/PS5 Controller");
    }

    #[test]
    fn dolphin_files() {
        let dir = std::env::temp_dir().join(format!("farfa-pads-{}", std::process::id()));
        let cfg = if cfg!(windows) { dir.join("User/Config") } else { return };
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("GCPadNew.ini"), "[GCPad1]\r\nDevice = DInput/0/Keyboard Mouse\r\nButtons/A = `X`\r\n[GCPad2]\r\nDevice = DInput/0/Keyboard Mouse\r\n").unwrap();
        std::fs::write(cfg.join("Dolphin.ini"), "[Interface]\r\nLanguageCode = fr\r\n").unwrap();
        let a = pad(1, "Xbox 360 Controller", "g1", "xinput0", Some(0));
        let b = pad(2, "PS5 Controller", "g2", "s", None);
        let players = vec![Some(PlayerPad { key: b.key.clone(), name: String::new() }), None, Some(PlayerPad { key: a.key.clone(), name: String::new() })];
        apply("dolphin", &players, &[a, b], &dir).unwrap();
        let gc = read(&cfg.join("GCPadNew.ini"));
        assert_eq!(ini_value(&gc, "GCPad1", "Device"), Some("SDL/0/PS5 Controller"));
        assert_eq!(ini_value(&gc, "GCPad1", "Buttons/A"), Some("`Button A`"));
        assert_eq!(ini_value(&gc, "GCPad2", "Device"), Some("DInput/0/Keyboard Mouse"));
        assert_eq!(ini_value(&gc, "GCPad3", "Device"), Some("XInput/0/Gamepad"));
        assert!(gc.contains("\r\n") && !gc.contains("`X`"));
        let main = read(&cfg.join("Dolphin.ini"));
        assert_eq!(ini_value(&main, "Interface", "LanguageCode"), Some("fr"));
        assert_eq!(
            ["SIDevice0", "SIDevice1", "SIDevice2", "SIDevice3"].map(|k| ini_value(&main, "Core", k)),
            [Some("6"), Some("0"), Some("6"), Some("0")]
        );
        let wii = read(&cfg.join("WiimoteNew.ini"));
        assert_eq!(ini_value(&wii, "Wiimote3", "Device"), Some("XInput/0/Gamepad"));
        assert_eq!(ini_value(&wii, "Wiimote1", "Source"), Some("1"));
        // Réglage fait dans Dolphin, même manette : gardé.
        let tuned = gc.replace("Buttons/Z = `Shoulder R`", "Buttons/Z = `Shoulder L`");
        std::fs::write(cfg.join("GCPadNew.ini"), &tuned).unwrap();
        let a = pad(1, "Xbox 360 Controller", "g1", "xinput0", Some(0));
        let b = pad(2, "PS5 Controller", "g2", "s", None);
        apply("dolphin", &players, &[a, b], &dir).unwrap();
        assert_eq!(read(&cfg.join("GCPadNew.ini")), tuned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manette Xbox (XInput), comparée à ce qu'écrit Suyu pour la même manette.
    #[test]
    fn eden_xbox() {
        let mut x = pad(3, "Xbox Series X Controller", "030003f05e040000ff02000000007801", "xinput0", Some(0));
        let mut b: Vec<(&'static str, String)> = ["south", "east", "west", "north", "leftshoulder", "rightshoulder", "back", "start", "leftstick", "rightstick", "guide"]
            .iter()
            .zip([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
            .map(|(k, n)| (*k, format!("button:{n}")))
            .collect();
        b.extend([("dpup", "hat:0,direction:up".to_string()), ("leftx", "axis:0".into()), ("lefty", "axis:1".into()), ("lefttrigger", "axis:2".into()), ("rightx", "axis:3".into()), ("righty", "axis:4".into()), ("righttrigger", "axis:5".into())]);
        x.bindings = b;
        let e: std::collections::HashMap<String, String> = eden::entries(&x, &[x.clone()], 0).into_iter().collect();
        let g = "port:0,guid:030000005e040000ff02000000007801";
        assert_eq!(e["player_0_button_a"], format!("\"engine:sdl,{g},button:1\""));
        assert_eq!(e["player_0_button_plus"], format!("\"engine:sdl,{g},button:7\""));
        assert_eq!(e["player_0_button_dup"], format!("\"engine:sdl,{g},hat:0,direction:up\""));
        assert_eq!(e["player_0_button_zl"], format!("\"engine:sdl,{g},axis:2,threshold:0.500000,invert:+\""));
        assert!(e["player_0_lstick"].contains("axis_x:0,axis_y:1"));
        assert_eq!(e["player_0_button_screenshot"], "\"\"");

        let dir = std::env::temp_dir().join(format!("farfa-eden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        apply("eden", &[], &[x.clone()], &dir).unwrap();
        let text = std::fs::read_to_string(dir.join("user/config/qt-config.ini")).unwrap();
        assert!(text.contains("[Controls]\n") && text.contains("player_0_connected=true") && text.contains(r"player_0_button_b\default=false"));
        // Réglage changé dans Eden avec la même manette : gardé.
        std::fs::write(dir.join("user/config/qt-config.ini"), text.replace(&format!("{g},button:1"), &format!("{g},button:0"))).unwrap();
        apply("eden", &[], &[x], &dir).unwrap();
        assert!(std::fs::read_to_string(dir.join("user/config/qt-config.ini")).unwrap().contains(&format!("player_0_button_a=\"engine:sdl,{g},button:0\"")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn punes_linux_guid() {
        // Manette Xbox 360 en USB : bus 3, 045e:028e, version 0x0114.
        let mut p = pad(1, "Xbox 360 Controller", "030000005e0400008e02000014010000", "/dev/input/event5", None);
        p.key = format!("{}//dev/input/event5", p.guid);
        let g = punes::linux_guid(&p).unwrap();
        // Mots : 3-500, -100, 045e, 045e-200, 028e, 028e-300, 0114, 0114-400.
        assert_eq!(g, "{FE0FFF9C-045E-0396-028E-01620114FF84}");
    }

    #[test]
    fn replace_keeps_others() {
        let t = "[A]\nx = 1\n[B]\ny = 2\n";
        assert_eq!(replace_section(t, "A", &["z = 3".into()]), "[A]\nz = 3\n[B]\ny = 2\n");
        assert_eq!(replace_section(t, "C", &["w = 4".into()]), "[A]\nx = 1\n[B]\ny = 2\n[C]\nw = 4\n");
    }
}
