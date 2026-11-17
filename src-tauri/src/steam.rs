//! « Ajouter à Steam » : raccourci non-Steam d'un jeu, qui lance le jeu par le launcher
//! (`farfalauncher --jouer <romId>`), avec sa jaquette et une capture comme images.
//!
//! Steam garde ces raccourcis dans `userdata/<compte>/config/shortcuts.vdf` (VDF binaire) et
//! leurs images dans `config/grid/`. Il ne relit le fichier qu'à son démarrage, et le réécrit en
//! quittant : il faut donc le redémarrer pour voir le raccourci. Une copie `.bak` est faite avant
//! chaque écriture. Windows et Linux de bureau ; sur le Steam Deck, NieRDeck crée déjà les siens.

use std::path::{Path, PathBuf};

use crate::error::{log_err, AppError, AppResult};

/// Argument de lancement d'un jeu par le launcher (raccourci Steam).
pub const PLAY_ARG: &str = "--jouer";

// ------------------------------------------------------------------ VDF binaire

#[derive(Debug, Clone, PartialEq)]
pub enum Vdf {
    Map(Vec<(String, Vdf)>),
    Str(Vec<u8>),
    Int(i32),
    Float(f32),
    U64(u64),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Map(items) => items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Vdf> {
        match self {
            Vdf::Map(items) => items.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Vdf::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    /// Remplace (ou ajoute) une clé, en gardant sa place.
    fn set(&mut self, key: &str, value: Vdf) {
        if let Some(v) = self.get_mut(key) {
            *v = value;
        } else if let Vdf::Map(items) = self {
            items.push((key.to_string(), value));
        }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }

    fn cstr(&mut self) -> Option<Vec<u8>> {
        let end = self.data[self.pos..].iter().position(|&b| b == 0)?;
        let s = self.data[self.pos..self.pos + end].to_vec();
        self.pos += end + 1;
        Some(s)
    }

    fn map(&mut self) -> Option<Vec<(String, Vdf)>> {
        let mut items = Vec::new();
        loop {
            let kind = self.byte()?;
            if kind == 0x08 {
                return Some(items);
            }
            let key = String::from_utf8_lossy(&self.cstr()?).into_owned();
            let value = match kind {
                0x00 => Vdf::Map(self.map()?),
                0x01 => Vdf::Str(self.cstr()?),
                0x02 => Vdf::Int(i32::from_le_bytes(self.take(4)?.try_into().ok()?)),
                0x03 => Vdf::Float(f32::from_le_bytes(self.take(4)?.try_into().ok()?)),
                0x07 => Vdf::U64(u64::from_le_bytes(self.take(8)?.try_into().ok()?)),
                _ => return None,
            };
            items.push((key, value));
        }
    }
}

/// Lit un VDF binaire (`None` s'il est illisible : on n'y touche pas).
pub fn parse(data: &[u8]) -> Option<Vdf> {
    let mut r = Reader { data, pos: 0 };
    let items = r.map()?;
    Some(Vdf::Map(items))
}

fn write_map(out: &mut Vec<u8>, items: &[(String, Vdf)]) {
    for (k, v) in items {
        let kind = match v {
            Vdf::Map(_) => 0x00,
            Vdf::Str(_) => 0x01,
            Vdf::Int(_) => 0x02,
            Vdf::Float(_) => 0x03,
            Vdf::U64(_) => 0x07,
        };
        out.push(kind);
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        match v {
            Vdf::Map(m) => write_map(out, m),
            Vdf::Str(s) => {
                out.extend_from_slice(s);
                out.push(0);
            }
            Vdf::Int(i) => out.extend_from_slice(&i.to_le_bytes()),
            Vdf::Float(f) => out.extend_from_slice(&f.to_le_bytes()),
            Vdf::U64(u) => out.extend_from_slice(&u.to_le_bytes()),
        }
    }
    out.push(0x08);
}

pub fn serialize(root: &Vdf) -> Vec<u8> {
    let mut out = Vec::new();
    if let Vdf::Map(items) = root {
        write_map(&mut out, items);
    }
    out
}

// ------------------------------------------------------------------ raccourcis

/// Identifiant d'un raccourci non-Steam, comme Steam le calcule : CRC-32 de l'exécutable (entre
/// guillemets) suivi du nom, bit de poids fort levé. Les images de `grid/` portent ce numéro.
pub fn shortcut_appid(exe_quoted: &str, name: &str) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(exe_quoted.as_bytes());
    h.update(name.as_bytes());
    h.finalize() | 0x8000_0000
}

fn quote(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy())
}

fn launch_options(rom_id: i64) -> String {
    format!("{PLAY_ARG} {rom_id}")
}

/// Ce raccourci lance-t-il ce jeu par le launcher ?
fn is_ours(entry: &Vdf, rom_id: i64) -> bool {
    entry.str("LaunchOptions").is_some_and(|o| o.trim() == launch_options(rom_id))
        && entry.str("Exe").is_some_and(|e| e.to_ascii_lowercase().contains("farfa"))
}

/// Ajoute ou met à jour le raccourci du jeu dans `root` (contenu de shortcuts.vdf). Renvoie son
/// identifiant Steam.
pub fn upsert(root: &mut Vdf, rom_id: i64, name: &str, exe: &Path, icon: Option<&Path>) -> u32 {
    if !matches!(root.get("shortcuts"), Some(Vdf::Map(_))) {
        *root = Vdf::Map(vec![("shortcuts".into(), Vdf::Map(Vec::new()))]);
    }
    let exe_quoted = quote(exe);
    let appid = shortcut_appid(&exe_quoted, name);
    let start_dir = exe.parent().map(quote).unwrap_or_default();
    let Some(Vdf::Map(list)) = root.get_mut("shortcuts") else { unreachable!() };
    let existing = list.iter().position(|(_, e)| is_ours(e, rom_id));
    let mut entry = match existing {
        Some(i) => list[i].1.clone(),
        None => Vdf::Map(vec![
            ("IsHidden".into(), Vdf::Int(0)),
            ("AllowDesktopConfig".into(), Vdf::Int(1)),
            ("AllowOverlay".into(), Vdf::Int(1)),
            ("OpenVR".into(), Vdf::Int(0)),
            ("Devkit".into(), Vdf::Int(0)),
            ("DevkitGameID".into(), Vdf::Str(Vec::new())),
            ("DevkitOverrideAppID".into(), Vdf::Int(0)),
            ("LastPlayTime".into(), Vdf::Int(0)),
            ("FlatpakAppID".into(), Vdf::Str(Vec::new())),
            ("tags".into(), Vdf::Map(Vec::new())),
        ]),
    };
    entry.set("appid", Vdf::Int(appid as i32));
    entry.set("AppName", Vdf::Str(name.as_bytes().to_vec()));
    entry.set("Exe", Vdf::Str(exe_quoted.into_bytes()));
    entry.set("StartDir", Vdf::Str(start_dir.into_bytes()));
    entry.set("icon", Vdf::Str(icon.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into_bytes()));
    entry.set("ShortcutPath", Vdf::Str(Vec::new()));
    entry.set("LaunchOptions", Vdf::Str(launch_options(rom_id).into_bytes()));
    // Rangement : l'identifiant reste en tête, comme dans les fichiers de Steam.
    if let Vdf::Map(items) = &mut entry {
        if let Some(i) = items.iter().position(|(k, _)| k == "appid") {
            let a = items.remove(i);
            items.insert(0, a);
        }
    }
    match existing {
        Some(i) => list[i].1 = entry,
        None => {
            let next = list.iter().filter_map(|(k, _)| k.parse::<usize>().ok()).max().map_or(0, |n| n + 1);
            list.push((next.to_string(), entry));
        }
    }
    appid
}

/// Identifiant du raccourci de ce jeu, s'il existe.
pub fn find(root: &Vdf, rom_id: i64) -> Option<u32> {
    let Some(Vdf::Map(list)) = root.get("shortcuts") else { return None };
    list.iter().find(|(_, e)| is_ours(e, rom_id)).map(|(_, e)| match (e.get("appid"), e.str("Exe"), e.str("AppName")) {
        (Some(Vdf::Int(id)), _, _) => *id as u32,
        (_, Some(exe), Some(name)) => shortcut_appid(&exe, &name),
        _ => 0,
    })
}

// ------------------------------------------------------------------ dossiers de Steam

/// Dossier d'installation de Steam.
pub fn steam_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(p) = windows_steam_path().filter(|p| p.join("userdata").is_dir()) {
            return Some(p);
        }
        let p = PathBuf::from(r"C:\Program Files (x86)\Steam");
        return p.join("userdata").is_dir().then_some(p);
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/data/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam", ".steam/root"]
            .iter()
            .map(|s| home.join(s))
            .find(|p| p.join("userdata").is_dir())
    }
}

#[cfg(windows)]
fn windows_steam_path() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(r"Software\Valve\Steam"), wide("SteamPath"));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY : tampon et taille valides, chaînes terminées par un zéro.
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if status != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len]).replace('/', "\\")))
}

/// Dossier `config` du compte Steam utilisé en dernier (celui dont la configuration a bougé en
/// dernier).
pub fn user_config_dir(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(root.join("userdata")).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "0" || !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let config = entry.path().join("config");
        let when = ["localconfig.vdf", "shortcuts.vdf"]
            .iter()
            .filter_map(|f| std::fs::metadata(config.join(f)).and_then(|m| m.modified()).ok())
            .max()
            .unwrap_or(std::time::UNIX_EPOCH);
        if config.is_dir() && best.as_ref().is_none_or(|(t, _)| when > *t) {
            best = Some((when, config));
        }
    }
    best.map(|(_, p)| p)
}

/// Exécutable que Steam doit lancer : l'AppImage elle-même sous Linux (pas son montage
/// temporaire), sinon le programme en cours.
pub fn launcher_exe() -> AppResult<PathBuf> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(appimage));
    }
    std::env::current_exe().map_err(|e| log_err(AppError::Internal, "chemin du launcher", e))
}

/// Steam est-il ouvert ? (il ne verra le raccourci qu'après un redémarrage)
pub fn steam_running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
    let mut sys = System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()));
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes().values().any(|p| {
        let n = p.name().to_string_lossy().to_ascii_lowercase();
        n == "steam.exe" || n == "steam" || n == "steamwebhelper.exe"
    })
}

// ------------------------------------------------------------------ ajout

pub struct Images<'a> {
    /// Jaquette (portrait) : bibliothèque de Steam.
    pub cover: Option<&'a Path>,
    /// Capture (paysage) : bandeau et image large.
    pub screenshot: Option<&'a Path>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Added {
    pub appid: u32,
    /// Steam est ouvert : le redémarrer pour voir le jeu.
    pub steam_running: bool,
    /// Le raccourci existait déjà (mis à jour).
    pub updated: bool,
}

fn copy_image(src: &Path, grid: &Path, stem: &str) {
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
    if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
        return;
    }
    // Une image d'une autre extension, laissée par un ajout précédent, passerait avant.
    for old in ["png", "jpg", "jpeg"] {
        let _ = std::fs::remove_file(grid.join(format!("{stem}.{old}")));
    }
    if let Err(e) = std::fs::copy(src, grid.join(format!("{stem}.{ext}"))) {
        tracing::warn!("image Steam {stem} non copiée : {e}");
    }
}

/// Ajoute (ou met à jour) le raccourci Steam d'un jeu dans `config` (dossier du compte Steam).
pub fn add_to(config: &Path, rom_id: i64, name: &str, exe: &Path, images: &Images<'_>) -> AppResult<Added> {
    let file = config.join("shortcuts.vdf");
    let mut root = match std::fs::read(&file) {
        Ok(data) => {
            // Copie de sécurité avant d'y toucher.
            std::fs::write(config.join("shortcuts.vdf.bak"), &data).map_err(|e| log_err(AppError::Internal, "copie de shortcuts.vdf", e))?;
            match parse(&data) {
                Some(root) => root,
                None => {
                    tracing::warn!("shortcuts.vdf illisible : rien n'est modifié");
                    return Err(AppError::Internal);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vdf::Map(vec![("shortcuts".into(), Vdf::Map(Vec::new()))]),
        Err(e) => return Err(log_err(AppError::Internal, "lecture de shortcuts.vdf", e)),
    };
    let updated = find(&root, rom_id).is_some();
    // Pas d'icône : Steam prend celle du launcher.
    let appid = upsert(&mut root, rom_id, name, exe, None);
    let tmp = config.join("shortcuts.vdf.tmp");
    std::fs::write(&tmp, serialize(&root))
        .and_then(|_| std::fs::rename(&tmp, &file))
        .map_err(|e| log_err(AppError::Internal, "écriture de shortcuts.vdf", e))?;
    let grid = config.join("grid");
    let _ = std::fs::create_dir_all(&grid);
    if let Some(cover) = images.cover {
        copy_image(cover, &grid, &format!("{appid}p"));
    }
    if let Some(shot) = images.screenshot {
        copy_image(shot, &grid, &appid.to_string());
        copy_image(shot, &grid, &format!("{appid}_hero"));
    }
    tracing::info!("raccourci Steam {} « {name} » (jeu {rom_id})", if updated { "mis à jour" } else { "ajouté" });
    Ok(Added { appid, steam_running: steam_running(), updated })
}

/// Le jeu a-t-il déjà son raccourci Steam ?
pub fn has_shortcut(rom_id: i64) -> bool {
    let Some(config) = steam_root().and_then(|r| user_config_dir(&r)) else { return false };
    std::fs::read(config.join("shortcuts.vdf")).ok().and_then(|d| parse(&d)).is_some_and(|root| find(&root, rom_id).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        // Un raccourci existant d'un autre programme, avec des tags et un entier 64 bits.
        let root = Vdf::Map(vec![(
            "shortcuts".into(),
            Vdf::Map(vec![(
                "0".into(),
                Vdf::Map(vec![
                    ("appid".into(), Vdf::Int(-123)),
                    ("AppName".into(), Vdf::Str(b"Autre jeu".to_vec())),
                    ("Exe".into(), Vdf::Str(b"\"C:\\autre.exe\"".to_vec())),
                    ("LastPlayTime".into(), Vdf::Int(1_700_000_000)),
                    ("Big".into(), Vdf::U64(u64::MAX)),
                    ("tags".into(), Vdf::Map(vec![("0".into(), Vdf::Str(b"favorite".to_vec()))])),
                ]),
            )]),
        )]);
        serialize(&root)
    }

    /// Vrai fichier de Steam, relu puis réécrit à l'identique (rien n'est modifié) :
    /// `FARFA_STEAM_VDF=<chemin> cargo test --lib real_shortcuts -- --ignored`.
    #[test]
    #[ignore]
    fn real_shortcuts_file_roundtrip() {
        let path = std::env::var("FARFA_STEAM_VDF").expect("FARFA_STEAM_VDF");
        let data = std::fs::read(path).unwrap();
        let root = parse(&data).expect("illisible");
        let Some(Vdf::Map(list)) = root.get("shortcuts") else { panic!("pas de liste") };
        println!("{} raccourcis : {:?}", list.len(), list.iter().map(|(_, e)| e.str("AppName").unwrap_or_default()).collect::<Vec<_>>());
        assert_eq!(serialize(&root), data);
    }

    #[test]
    fn binary_vdf_roundtrip() {
        let data = sample();
        let root = parse(&data).unwrap();
        assert_eq!(serialize(&root), data);
        assert!(parse(&data[..data.len() - 1]).is_none());
        assert!(parse(&[0x05, b'x', 0]).is_none());
    }

    #[test]
    fn add_keeps_other_shortcuts_and_updates_in_place() {
        let mut root = parse(&sample()).unwrap();
        let exe = Path::new(r"C:\Users\x\FarfaLauncher\farfalauncher.exe");
        let id = upsert(&mut root, 42, "Zelda", exe, None);
        assert!(id & 0x8000_0000 != 0);
        assert_eq!(id, shortcut_appid(&format!("\"{}\"", exe.display()), "Zelda"));
        let Some(Vdf::Map(list)) = root.get("shortcuts") else { panic!() };
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].1.str("AppName").unwrap(), "Autre jeu");
        assert_eq!(list[1].0, "1");
        let ours = &list[1].1;
        assert_eq!(ours.str("LaunchOptions").unwrap(), "--jouer 42");
        assert_eq!(ours.str("StartDir").unwrap(), r#""C:\Users\x\FarfaLauncher""#);
        assert_eq!(ours.get("appid"), Some(&Vdf::Int(id as i32)));
        assert_eq!(find(&root, 42), Some(id));
        assert_eq!(find(&root, 7), None);
        // Relu tel quel après écriture, et un second ajout met à jour au lieu de dupliquer.
        let mut again = parse(&serialize(&root)).unwrap();
        let id2 = upsert(&mut again, 42, "Zelda : édition", exe, None);
        let Some(Vdf::Map(list)) = again.get("shortcuts") else { panic!() };
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].1.str("AppName").unwrap(), "Zelda : édition");
        assert_eq!(find(&again, 42), Some(id2));
    }

    #[test]
    fn empty_or_missing_file_starts_a_list() {
        let mut root = Vdf::Map(Vec::new());
        upsert(&mut root, 5, "Jeu", Path::new("/home/x/FaRFa.AppImage"), None);
        let data = serialize(&root);
        assert_eq!(&data[..11], b"\x00shortcuts\x00");
        assert!(find(&parse(&data).unwrap(), 5).is_some());
    }

    #[test]
    fn writes_files_with_backup_and_images() {
        let dir = std::env::temp_dir().join(format!("farfa-steam-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shortcuts.vdf"), sample()).unwrap();
        let cover = dir.join("cover.jpg");
        std::fs::write(&cover, b"jpg").unwrap();
        let added = add_to(&dir, 9, "Mario", Path::new(r"C:\f\farfalauncher.exe"), &Images { cover: Some(&cover), screenshot: None }).unwrap();
        assert!(!added.updated);
        assert_eq!(std::fs::read(dir.join("shortcuts.vdf.bak")).unwrap(), sample());
        assert!(dir.join("grid").join(format!("{}p.jpg", added.appid)).is_file());
        let again = add_to(&dir, 9, "Mario", Path::new(r"C:\f\farfalauncher.exe"), &Images { cover: None, screenshot: None }).unwrap();
        assert!(again.updated);
        assert_eq!(again.appid, added.appid);
        // Un fichier illisible n'est jamais écrasé.
        std::fs::write(dir.join("shortcuts.vdf"), b"\x05nimporte").unwrap();
        assert!(add_to(&dir, 9, "Mario", Path::new("x"), &Images { cover: None, screenshot: None }).is_err());
        assert_eq!(std::fs::read(dir.join("shortcuts.vdf")).unwrap(), b"\x05nimporte");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
