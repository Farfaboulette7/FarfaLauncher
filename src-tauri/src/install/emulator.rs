//! Jeu lancé par un émulateur : l'émulateur vient de FarfaRomManager, la ROM de RomM.
//!
//! ```text
//! <dossier des jeux>/Emulateurs/<id>/      installé une fois, partagé par ses jeux
//! <dossier des jeux>/Roms/<console>/<jeu>/ la ROM, et ses sauvegardes si l'émulateur les y range
//! ```
//!
//! Les ROM sont hors du dossier de l'émulateur : le retirer ne touche ni à elles,
//! ni aux sauvegardes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::state::{self, DirectLaunch, InstalledGame, RomSource, SaveSync, SharedGame};
use super::{detect, download, ensure_space, extract, make_executable, path_to_rel, rel_to_path, blocking, InstallJob, InstallOutcome};
use crate::error::{AppError, AppResult};
use crate::install::download::disk_error;
use crate::install::extract::INTERNAL_DIR;
use crate::error::log_err;
use crate::manager::{self, Emulator, RomPrep};
use crate::platform::ArchiveKind;
use crate::romm::library::{self, RemoteFile};
use crate::runtimes;

pub const KIND_EMULATOR: &str = "emulator";

/// Fichier à donner à l'émulateur quand un jeu en compte plusieurs.
fn disc_index(files: &[RemoteFile]) -> Option<&RemoteFile> {
    let ext = |f: &RemoteFile| f.file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    ["m3u", "cue", "gdi", "ccd"].iter().find_map(|e| files.iter().find(|f| ext(f) == *e))
}

/// Note d'une version de cartouche d'après son nom (GoodTools, No-Intro) : vérifiée,
/// française, européenne puis américaine ; jamais un hack, une bêta ou un dump abîmé.
pub fn cartridge_score(name: &str) -> i32 {
    let n = name.to_lowercase();
    let mut score = 0;
    for bad in ["[b", "[h", "[t", "[f", "[o", "[p", "hack", "(beta", "(proto", "(sample", "(demo", "(pd)", "(unl)"] {
        if n.contains(bad) {
            score -= 100;
        }
    }
    if n.contains("[a") {
        score -= 10;
    }
    if n.contains("[!]") {
        score += 20;
    }
    score += if n.contains("(f)") || n.contains("(france)") || n.contains("(fr") || n.contains(",fr") {
        12
    } else if n.contains("(e)") || n.contains("(europe") || n.contains(", europe") {
        8
    } else if n.contains("(u)") || n.contains("(usa") {
        4
    } else if n.contains("(m") {
        // Multilingue sans région connue (GoodTools « (M3) »).
        6
    } else {
        0
    };
    score
}

/// ROM à lancer : l'index d'un disque (cue, m3u…), sinon la meilleure version de la
/// cartouche (une entrée RomM « GoodTools » range toutes les versions d'un jeu ensemble).
fn main_rom(files: &[RemoteFile]) -> Option<&RemoteFile> {
    disc_index(files).or_else(|| {
        files.iter().max_by_key(|f| (is_rom_like(f), switch_base(&f.file_name), !f.rel_path.contains('/'), cartridge_score(&f.file_name), std::cmp::Reverse(f.file_name.len()), f.size))
    })
}

/// Contenu Switch : jeu, mise à jour ou DLC.
fn is_switch_content(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "nsp" | "nsz" | "xci" | "xcz")
}

/// Jeu de base plutôt que mise à jour ou DLC posés à côté. L'ID de titre le dit
/// (`[0100…000]` : le jeu ; `…800` : sa mise à jour ; le reste : des DLC), sinon le nom
/// (« update », « dlc », ou une version de mise à jour comme `v131072`).
fn switch_base(name: &str) -> u8 {
    if !is_switch_content(name) {
        return 1;
    }
    let n = name.to_ascii_lowercase();
    let title_id = n.split(['[', ']', '(', ')', ' ', '_']).find(|w| w.len() == 16 && w.starts_with("01") && w.chars().all(|c| c.is_ascii_hexdigit()));
    if let Some(id) = title_id {
        return if id.ends_with("000") { 2 } else { 0 };
    }
    let update_version = n.match_indices('v').any(|(i, _)| {
        let digits = n[i + 1..].chars().take_while(char::is_ascii_digit).count();
        digits >= 5 && (i == 0 || !n.as_bytes()[i - 1].is_ascii_alphanumeric())
    });
    if update_version || ["update", "upd]", "[upd", "dlc"].iter().any(|w| n.contains(w)) {
        0
    } else {
        1
    }
}

/// Fichier de jeu plutôt qu'annexe : un dossier RomM peut aussi ranger des archives de mods,
/// de DLC ou de sauvegarde (Switch : jeu.nsp, patch/maj.nsp, dlc.zip…).
fn is_rom_like(f: &RemoteFile) -> bool {
    let ext = f.file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    !["zip", "7z", "rar", "txt", "nfo", "jpg", "jpeg", "png", "pdf", "md", "sfv", "torrent"].contains(&ext.as_str())
}

/// Fichiers à télécharger : tout le disque, ou la seule cartouche retenue.
pub fn emulator_files(files: &[RemoteFile]) -> Vec<&RemoteFile> {
    if disc_index(files).is_some() {
        return files.iter().collect();
    }
    let main = main_rom(files);
    // Switch : le jeu avec ses mises à jour et DLC, que Citron et Eden lisent dans son dossier
    // (voir `switch_content`).
    if main.is_some_and(|m| is_switch_content(&m.file_name)) {
        return files.iter().filter(|f| is_switch_content(&f.file_name)).collect();
    }
    main.into_iter().collect()
}

/// Chemin de la ROM dans son dossier : `ROM/x.gba` → `x.gba` (entrée portage-*).
fn rom_rel(file: &RemoteFile) -> PathBuf {
    let rel = file.rel_path.strip_prefix("ROM/").or_else(|| file.rel_path.strip_prefix("rom/")).unwrap_or(&file.rel_path);
    rel_to_path(rel)
}

/// Nom du dossier d'un jeu : sans l'extension quand l'entrée RomM est un fichier seul.
fn rom_folder<'a>(entry: &'a str, files: &[RemoteFile]) -> &'a str {
    if files.len() == 1 && files[0].file_name == entry {
        stem(entry)
    } else {
        entry
    }
}

fn stem(file_name: &str) -> &str {
    Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name)
}

fn expand(text: &str, rom: &Path, rom_dir: &Path, rom_stem: &str) -> String {
    text.replace("{rom}", &rom.to_string_lossy()).replace("{romdir}", &rom_dir.to_string_lossy()).replace("{romstem}", rom_stem)
}

/// Lancement d'un jeu d'émulateur avec les arguments actuels du catalogue :
/// un `argument:` ajouté sur le gestionnaire (plein écran…) vaut sans réinstaller.
pub fn refreshed_launch(emulator: &Emulator, st: &InstalledGame) -> Option<DirectLaunch> {
    let old = st.direct_launch.as_ref()?;
    let rom_dir = st.shared.as_ref()?.rom_dir.clone()?;
    // La ROM est l'argument qui désigne un fichier du dossier du jeu.
    let rom = old.args.iter().map(PathBuf::from).find(|p| p.starts_with(&rom_dir) && p.is_file())?;
    let rom_stem = rom.file_stem()?.to_str()?.to_string();
    let args = emulator.launch.args.iter().map(|a| expand(a, &rom, &rom_dir, &rom_stem)).collect();
    let env = emulator.launch.env.iter().map(|(k, v)| (k.clone(), expand(v, &rom, &rom_dir, &rom_stem))).collect();
    Some(DirectLaunch { exe: old.exe.clone(), args, cwd: old.cwd.clone(), env })
}

/// ROM qui n'a pas été préparée pour l'émulateur (installée par un launcher qui ne savait pas
/// le faire, 1.2.0 et avant : le zip a été donné tel quel à Azahar) : préparée au lancement,
/// et l'installation est mise à jour. `None` : rien à faire.
pub async fn heal_rom(config_dir: &Path, emulator: &Emulator, st: &InstalledGame) -> AppResult<Option<InstalledGame>> {
    let Some(prep) = emulator.rom_prep.clone().filter(|p| p.unzip || p.decrypt.is_some()) else { return Ok(None) };
    let Some(shared) = st.shared.as_ref().filter(|s| s.is_emulator()) else { return Ok(None) };
    let (Some(rom_dir), Some(launch), Some(exe)) = (shared.rom_dir.clone(), st.direct_launch.as_ref(), st.executable.as_deref()) else { return Ok(None) };
    let Some(rom) = launch.args.iter().map(PathBuf::from).find(|p| p.starts_with(&rom_dir) && p.is_file()) else { return Ok(None) };
    let is_zip = rom.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let three_ds = prep.decrypt.as_deref() == Some("3ds");
    let (r, three) = (rom.clone(), three_ds);
    let needed = blocking(move || Ok(crate::n3ds::is_rom(&r) && three && crate::n3ds::is_encrypted(&r))).await? || (prep.unzip && is_zip);
    if !needed {
        return Ok(None);
    }
    tracing::info!("« {} » : ROM pas encore préparée pour {}, préparée avant le lancement", st.name, emulator.name);
    let keys = prep.keys.as_deref().map(|k| emulator.data_dir(&st.game_dir, exe).join(rel_to_path(k)));
    let (r, dir) = (rom.clone(), rom_dir.clone());
    let rel = blocking(move || prepare_rom(&r, &dir, &prep, keys.as_deref(), &|_, _, _| {})).await?;
    let new_rom = rom_dir.join(&rel);
    let (old_rel, new_rel) = (rom.strip_prefix(&rom_dir).map(path_to_rel).unwrap_or_default(), path_to_rel(&rel));
    let mut st = st.clone();
    for r in st.roms.iter_mut().filter(|r| **r == old_rel) {
        *r = new_rel.clone();
    }
    for s in &mut st.rom_sources {
        for p in s.placed.iter_mut().filter(|p| **p == old_rel) {
            *p = new_rel.clone();
        }
    }
    if let Some(d) = st.direct_launch.as_mut() {
        let (old, new) = (rom.to_string_lossy().into_owned(), new_rom.to_string_lossy().into_owned());
        for a in d.args.iter_mut().filter(|a| **a == old) {
            *a = new.clone();
        }
    }
    // Sauvegardes : l'identifiant du jeu se lit maintenant dans la ROM (3DS : ID de titre).
    // La console est le dossier des ROM de ce jeu (`Roms/<console>/<jeu>`).
    let console = rom_dir.parent().and_then(Path::file_name).and_then(|c| c.to_str()).unwrap_or("").to_string();
    let disc = disc_id(&new_rom);
    let stem_name = new_rom.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let (save_dir, patterns) = emulator.saves.for_console(&console);
    let exe_dir = emulator.data_dir(&st.game_dir, exe);
    if let Some(sh) = st.shared.as_mut() {
        sh.saves_dir = saves_dir(save_dir, &rom_dir, &exe_dir, disc.as_deref());
        sh.save_patterns = patterns
            .iter()
            .filter(|p| !crate::switch_saves::is_deferred(p))
            .filter_map(|p| expand_disc(&expand(p, &new_rom, &rom_dir, &stem_name), disc.as_deref()))
            .collect();
    }
    state::write_state(config_dir, &st)?;
    tracing::info!("« {} » : ROM préparée, {}", st.name, new_rom.display());
    Ok(Some(st))
}

/// Émulateur en bundle `.app` (Mac) : lancé depuis le dossier qui contient le bundle, où
/// les dérivés de yuzu cherchent `user/` et mGBA son `portable.ini`.
fn bundle_cwd(exe: &str) -> Option<String> {
    if !cfg!(target_os = "macos") || !exe.to_ascii_lowercase().ends_with(".app") {
        return None;
    }
    Some(exe.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default())
}

/// Identifiant d'un jeu 3DS (voir `n3ds::save_id`) ou d'un disque : GameCube ou Wii (`GZ2P01`, dans son en-tête), PSP
/// (`ULES00151`, dans `UMD_DATA.BIN`) ou PS2 (`SLES-50330`, dans `SYSTEM.CNF`),
/// aussi au format CHD.
pub fn disc_id(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    // 3DS : moitié basse de l'ID de titre (`0006a900`), le dossier de ses sauvegardes dans Azahar.
    if crate::n3ds::is_rom(path) {
        return crate::n3ds::save_id(path);
    }
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext == "chd" {
        let mut disc = ChdSectors::open(path)?;
        return psp_id(&mut disc).or_else(|| ps2_id(&mut disc));
    }
    // RVZ / WIA : copie de l'en-tête du disque à 0x58 ; CISO : 0x8000 ; WBFS : 0x200.
    let offset = match ext.as_str() {
        "iso" | "gcm" => 0,
        "rvz" | "wia" => 0x58,
        "ciso" => 0x8000,
        "wbfs" => 0x200,
        _ => return None,
    };
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut id = [0u8; 6];
    file.read_exact(&mut id).ok()?;
    if id.iter().all(u8::is_ascii_alphanumeric) {
        return Some(String::from_utf8_lossy(&id).into_owned());
    }
    if ext != "iso" {
        return None;
    }
    let mut disc = IsoSectors(file);
    psp_id(&mut disc).or_else(|| ps2_id(&mut disc))
}

const SECTOR: usize = 2048;

/// Secteurs de 2048 octets d'une image de disque.
trait Sectors {
    fn sector(&mut self, lba: u64) -> Option<Vec<u8>>;
}

struct IsoSectors(std::fs::File);

impl Sectors for IsoSectors {
    fn sector(&mut self, lba: u64) -> Option<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        self.0.seek(SeekFrom::Start(lba * SECTOR as u64)).ok()?;
        let mut buf = vec![0; SECTOR];
        self.0.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

/// Image CHD : de DVD (`chdman createdvd`, blocs de secteurs de 2048 octets) ou de
/// CD (`createcd`, trames de 2352 octets suivies de 96 octets de sous-code).
struct ChdSectors {
    chd: chd::Chd<std::io::BufReader<std::fs::File>>,
    hunk_size: u64,
    /// Taille d'une trame dans un bloc : 2048 (DVD) ou 2448 (CD).
    frame: u64,
    compressed: Vec<u8>,
}

/// Trame de CD avec sous-code, telle que la range un CHD de CD.
const CD_FRAME: u64 = 2448;

impl ChdSectors {
    fn open(path: &Path) -> Option<Self> {
        let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        let chd = chd::Chd::open(file, None).ok()?;
        let hunk_size = u64::from(chd.header().hunk_size());
        let frame = if hunk_size % CD_FRAME == 0 {
            CD_FRAME
        } else if hunk_size >= SECTOR as u64 && hunk_size % SECTOR as u64 == 0 {
            SECTOR as u64
        } else {
            return None;
        };
        Some(Self { chd, hunk_size, frame, compressed: Vec::new() })
    }
}

impl Sectors for ChdSectors {
    fn sector(&mut self, lba: u64) -> Option<Vec<u8>> {
        let offset = lba * self.frame;
        let mut out = self.chd.get_hunksized_buffer();
        let mut hunk = self.chd.hunk(u32::try_from(offset / self.hunk_size).ok()?).ok()?;
        hunk.read_hunk_in(&mut self.compressed, &mut out).ok()?;
        let mut start = (offset % self.hunk_size) as usize;
        // Trame brute (piste MODE1_RAW, MODE2_RAW : motif de synchronisation en tête) : données
        // après l'en-tête, 16 octets en mode 1, 24 en mode 2 (forme 1). Piste MODE1 « cuite » :
        // les 2048 octets de données d'emblée.
        const SYNC: [u8; 12] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
        if self.frame == CD_FRAME && out.get(start..start + 12) == Some(&SYNC[..]) {
            start += if out.get(start + 15) == Some(&2) { 24 } else { 16 };
        }
        out.get(start..start + SECTOR).map(<[u8]>::to_vec)
    }
}

/// Fichier à la racine d'un disque ISO 9660 : premier secteur et taille.
fn root_file(disc: &mut dyn Sectors, wanted: &[u8]) -> Option<(u64, u32)> {
    let pvd = disc.sector(16)?;
    if &pvd[1..6] != b"CD001" {
        return None;
    }
    let root = &pvd[156..190];
    let (lba, size) = (u32::from_le_bytes(root[2..6].try_into().ok()?), u32::from_le_bytes(root[10..14].try_into().ok()?));
    for i in 0..u64::from(size).div_ceil(SECTOR as u64).min(16) {
        let dir = disc.sector(u64::from(lba) + i)?;
        let mut pos = 0;
        while pos + 33 < dir.len() && dir[pos] != 0 {
            let len = dir[pos] as usize;
            let name_len = dir[pos + 32] as usize;
            let name = dir.get(pos + 33..pos + 33 + name_len)?;
            let bare = name.strip_suffix(b";1").unwrap_or(name);
            if bare.eq_ignore_ascii_case(wanted) {
                let at = u64::from(u32::from_le_bytes(dir[pos + 2..pos + 6].try_into().ok()?));
                return Some((at, u32::from_le_bytes(dir[pos + 10..pos + 14].try_into().ok()?)));
            }
            pos += len.max(1);
        }
    }
    None
}

/// `UMD_DATA.BIN` à la racine du disque PSP (ISO 9660) : « ULES-00151|… ».
fn psp_id(disc: &mut dyn Sectors) -> Option<String> {
    let (at, _) = root_file(disc, b"UMD_DATA.BIN")?;
    let data = disc.sector(at)?;
    let text = String::from_utf8_lossy(&data[..data.iter().position(|&b| b == b'|').unwrap_or(16).min(16)]).replace('-', "");
    (text.len() == 9 && text.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(text)
}

/// `SYSTEM.CNF` d'un disque PS2 : « BOOT2 = cdrom0:\SLES_503.30;1 » → `SLES-50330`,
/// l'identifiant que PCSX2 donne au jeu (et que portent ses sauvegardes).
fn ps2_id(disc: &mut dyn Sectors) -> Option<String> {
    let (at, _) = root_file(disc, b"SYSTEM.CNF")?;
    ps2_serial(&String::from_utf8_lossy(&disc.sector(at)?))
}

fn ps2_serial(cnf: &str) -> Option<String> {
    let line = cnf.lines().find(|l| l.trim_start().to_ascii_uppercase().starts_with("BOOT2"))?;
    let file = line.rsplit(['\\', ':']).next()?.split(';').next()?.trim();
    let (prefix, number) = file.split_once('_')?;
    let number: String = number.chars().filter(char::is_ascii_digit).collect();
    (prefix.len() == 4 && prefix.bytes().all(|b| b.is_ascii_alphabetic()) && !number.is_empty())
        .then(|| format!("{}-{number}", prefix.to_ascii_uppercase()))
}

/// Variables d'un disque : `{discid}` (identifiant entier), `{gameid}` (4 caractères),
/// `{gameidhex}` (dossier Wii) et `{region}` (cartes mémoire GameCube).
fn expand_disc(text: &str, id: Option<&str>) -> Option<String> {
    if !text.contains("{gameid") && !text.contains("{region}") && !text.contains("{discid}") {
        return Some(text.to_string());
    }
    let id = id?;
    let text = text.replace("{discid}", id);
    // ID de moins de 4 octets (ou coupé au milieu d'un caractère) : pas de variable, sans
    // paniquer (L12).
    let short = id.get(..4)?;
    let region = match short.as_bytes()[3] {
        b'E' | b'N' => "USA",
        b'J' => "JAP",
        b'K' | b'Q' | b'T' => "KOR",
        _ => "EUR",
    };
    let hex: String = short.bytes().map(|b| format!("{b:02x}")).collect();
    Some(text.replace("{gameidhex}", &hex).replace("{gameid}", short).replace("{region}", region))
}

/// Dossier des sauvegardes d'un jeu : à côté de la ROM, relatif à l'exécutable, ou dans
/// les dossiers de l'utilisateur (`{appdata}/…`).
fn saves_dir(dir: Option<&str>, rom_dir: &Path, exe_dir: &Path, disc: Option<&str>) -> Option<PathBuf> {
    // Switch : profil et ID du jeu connus au lancement seulement (voir `switch_saves`).
    if dir.is_some_and(crate::switch_saves::is_deferred) {
        return None;
    }
    let exe_dir = exe_dir.to_path_buf();
    match expand_disc(dir?, disc)?.as_str() {
        "{romdir}" => Some(rom_dir.to_path_buf()),
        dir if dir.starts_with('{') => crate::language::expand(dir, &exe_dir),
        rel => Some(exe_dir.join(rel_to_path(rel))),
    }
}

impl InstallJob {
    /// Installe l'émulateur s'il manque ou n'est plus à jour, puis la ROM de ce jeu.
    pub(super) async fn install_with_emulator(&self, rom_id: i64) -> AppResult<InstallOutcome> {
        let game = library::fetch_game_details(&self.client, rom_id, self.target).await?;
        // Le jeu peut déjà être installé d'autres façons : elles restent sur le disque, en
        // réserve. Les émulateurs partagent le dossier des ROM et les sauvegardes (RomM) ; la ROM
        // posée à côté d'une recomp est reprise par lien physique (une seule copie).
        let installs = state::all_installs(&self.config_dir, rom_id);
        let on_emulator = |p: &&InstalledGame| p.shared.as_ref().is_some_and(|s| s.is_emulator());
        let active = installs.first().filter(on_emulator).cloned();
        let emulator = self.pick_emulator(&game.console, rom_id, active.as_ref().or(installs.iter().find(on_emulator))).ok_or(AppError::NotAvailable)?;
        // Cet émulateur-là d'abord (mise à jour), sinon un autre : même dossier des ROM.
        let previous = installs
            .iter()
            .filter(on_emulator)
            .find(|p| p.shared.as_ref().is_some_and(|s| s.game == emulator.id))
            .or_else(|| installs.iter().find(on_emulator))
            .cloned();
        let recomps: Vec<InstalledGame> = installs.iter().filter(|p| p.shared.is_none() && p.pc.is_none()).cloned().collect();
        if game.rom_files.is_empty() {
            return Err(AppError::NotAvailable);
        }
        let installed = {
            let _lock = self.runtime_lock.lock().await;
            self.ensure_emulator(rom_id, &emulator).await?
        };
        let exe = installed.executable.clone().ok_or(AppError::NoExecutable)?;
        self.apply_packs(rom_id, &emulator, &installed).await?;

        // Dossier des jeux déduit de l'émulateur : il peut avoir été installé ailleurs.
        let root = installed.game_dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| self.install_root.clone());
        let rom_dir = previous
            .as_ref()
            .and_then(|p| p.shared.as_ref())
            .and_then(|s| s.rom_dir.clone())
            .unwrap_or_else(|| root.join("Roms").join(state::safe_folder_name(&game.console, rom_id)).join(state::safe_folder_name(rom_folder(&game.folder_name, &game.rom_files), rom_id)));

        let files = emulator_files(&game.rom_files);
        let main = main_rom(&game.rom_files).ok_or(AppError::NotAvailable)?;
        // ROM à préparer (zip extrait, jeu 3DS déchiffré) : déjà faite pour cette archive, elle reste.
        let prep = emulator.rom_prep.clone().filter(|p| p.unzip || p.decrypt.is_some());
        let prepared = prep.as_ref().and_then(|p| prepared_before(previous.as_ref(), main, &rom_dir, p));
        let total: u64 = files.iter().map(|f| f.size).sum();
        ensure_space(&rom_dir, total)?;
        let mut offset = 0;
        let mut last = Instant::now() - Duration::from_secs(1);
        // Jeu joué jusqu'ici avec sa recomp : la ROM posée à côté d'elle est reprise.
        let mut reused: Vec<(PathBuf, String)> = Vec::new();
        for file in files {
            let dest = rom_dir.join(rom_rel(file));
            // Même ROM déjà en place (changement d'émulateur) : ni retéléchargée, ni relue en entier.
            if already_placed(previous.as_ref(), file, &dest) || (prepared.is_some() && file.rel_path == main.rel_path) {
                offset += file.size;
                self.emit(rom_id, "download", offset, total);
                continue;
            }
            if let Some(old) = recomps.iter().find_map(|r| recomp_copy(Some(r), file)) {
                let (from, to) = (old.clone(), dest.clone());
                match blocking(move || super::reuse_file(&from, &to).map_err(|e| disk_error("reprise de la ROM de la recomp", e))).await {
                    Ok(()) => {
                        tracing::info!("ROM reprise de la recomp : {}", old.display());
                        reused.push((old, file.rel_path.clone()));
                        offset += file.size;
                        self.emit(rom_id, "download", offset, total);
                        continue;
                    }
                    Err(e) => tracing::warn!("ROM de la recomp non reprise ({}): {}", old.display(), e.code()),
                }
            }
            download::download(&self.client, file, &dest, &self.cancel, |got| {
                if last.elapsed() >= Duration::from_millis(200) || got == file.size {
                    last = Instant::now();
                    self.emit(rom_id, "download", offset + got, total);
                }
            }, || self.emit_verifying(rom_id, offset + file.size, total))
            .await?;
            offset += file.size;
        }

        // ROM préparée pour l'émulateur : c'est elle qui est lancée (posée à la place du zip).
        let prepared = match (prep, prepared) {
            (_, Some(rel)) => Some(rel),
            (Some(prep), None) => {
                let archive = rom_dir.join(rom_rel(main));
                let keys = prep.keys.as_deref().map(|k| emulator.data_dir(&installed.game_dir, &exe).join(rel_to_path(k)));
                let progress = self.on_progress.clone();
                let dir = rom_dir.clone();
                let rel = blocking(move || {
                    let report = |phase: &'static str, done: u64, total: u64| progress(super::Progress { rom_id, phase, done, total, verifying: false });
                    prepare_rom(&archive, &dir, &prep, keys.as_deref(), &report)
                })
                .await?;
                Some(path_to_rel(&rel))
            }
            (None, None) => None,
        };
        self.emit(rom_id, "finalize", 0, 0);
        let main_placed = path_to_rel(&rom_rel(main));
        let rom_path = rom_dir.join(rel_to_path(prepared.as_deref().unwrap_or(&main_placed)));
        let rom_file_name = rom_path.file_name().and_then(|n| n.to_str()).unwrap_or(&main.file_name).to_string();
        let rom_stem = stem(&rom_file_name);
        let disc = disc_id(&rom_path);
        let (save_dir, save_patterns) = emulator.saves.for_console(&game.console);
        let args = emulator.launch.args.iter().map(|a| expand(a, &rom_path, &rom_dir, rom_stem)).collect();
        let env = emulator.launch.env.iter().map(|(k, v)| (k.clone(), expand(v, &rom_path, &rom_dir, rom_stem))).collect();
        let now = state::now();
        let placed: Vec<String> = game
            .rom_files
            .iter()
            .map(|f| match &prepared {
                Some(rel) if f.rel_path == main.rel_path => rel.clone(),
                _ => path_to_rel(&rom_rel(f)),
            })
            .collect();
        let st = InstalledGame {
            rom_id,
            name: game.name.clone(),
            version: installed.version.clone(),
            // Version de l'émulateur plus celle des archives du jeu : une archive mise à jour
            // sur le serveur (mods…) se signale comme une mise à jour du jeu.
            version_raw: Some(emulator.version_for(rom_id)),
            target: installed.target,
            archive_name: installed.archive_name.clone(),
            executable: Some(exe.clone()),
            pending_candidates: Vec::new(),
            roms: placed.clone(),
            rom_sources: game
                .rom_files
                .iter()
                .zip(&placed)
                .map(|(f, p)| RomSource { rel_path: f.rel_path.clone(), size: f.size, sha1: f.sha1.clone(), md5: f.md5.clone(), placed: vec![p.clone()] })
                .collect(),
            mods_downloaded: Vec::new(),
            // Sauvegardes d'une recomp à part (sur FarfaRomManager) : l'émulateur cherche les siennes.
            saves: previous.as_ref().filter(|p| p.shared.as_ref().is_some_and(|s| s.is_emulator())).map(|p| p.saves.clone()).unwrap_or_else(SaveSync::default),
            direct_launch: Some(DirectLaunch { exe: exe.clone(), args, cwd: bundle_cwd(&exe), env }),
            prefer_launcher: false,
            players: previous.as_ref().map(|p| p.players.clone()).unwrap_or_default(),
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir: installed.game_dir.clone(),
            pc: None,
            ways: Vec::new(),
            shared: Some(SharedGame {
                runtime_rom_id: installed.rom_id,
                game: emulator.id.clone(),
                runtime_name: emulator.name.clone(),
                data_dir: String::new(),
                save_dir: String::new(),
                cache_dir: None,
                kind: Some(KIND_EMULATOR.to_string()),
                rom_dir: Some(rom_dir.clone()),
                saves_dir: saves_dir(save_dir, &rom_dir, &emulator.data_dir(&installed.game_dir, &exe), disc.as_deref()),
                save_patterns: save_patterns.iter().filter(|p| !crate::switch_saves::is_deferred(p)).filter_map(|p| expand_disc(&expand(p, &rom_path, &rom_dir, rom_stem), disc.as_deref())).collect(),
            }),
        };
        state::write_state(&self.config_dir, &st)?;
        // La ROM reprise reste aussi à côté de la recomp : on peut y revenir sans retélécharger.
        drop(reused);
        tracing::info!("« {} » installé avec {} dans {}", st.name, emulator.name, rom_dir.display());
        let st = state::find(&self.config_dir, rom_id).ok_or(AppError::Internal)?;
        Ok(InstallOutcome::Installed { game: st })
    }

    /// Émulateur à utiliser : celui choisi, sinon celui du jeu déjà installé, sinon le premier de la console.
    /// Un émulateur réservé à d'autres jeux (`seulement:`) n'est jamais pris.
    fn pick_emulator(&self, console: &str, rom_id: i64, previous: Option<&InstalledGame>) -> Option<Emulator> {
        let of_console =
            |id: &str| self.catalog.emulator(id).filter(|e| e.consoles.iter().any(|c| c == console) && e.serves(rom_id));
        self.emulator_id
            .as_deref()
            .and_then(of_console)
            .or_else(|| previous.and_then(|p| p.shared.as_ref()).filter(|s| s.is_emulator()).and_then(|s| of_console(&s.game)))
            .or_else(|| self.catalog.emulator_for_console(console, self.target))
            .cloned()
    }

    /// Émulateur installé et à jour. Sa progression s'affiche sur la page `page_id`.
    async fn ensure_emulator(&self, page_id: i64, emulator: &Emulator) -> AppResult<InstalledGame> {
        let file = emulator.file_for(self.target).ok_or(AppError::NotAvailable)?;
        let runtime_rom_id = manager::runtime_id(&emulator.id);
        let previous = state::find(&self.config_dir, runtime_rom_id);
        if let Some(current) = &previous {
            if current.executable.is_some() && current.version_raw.as_deref() == Some(emulator.version.cle.as_str()) {
                self.fetch_configs(page_id, emulator, current).await?;
                return Ok(current.clone());
            }
        }
        let game_dir = previous
            .as_ref()
            .map(|p| p.game_dir.clone())
            .unwrap_or_else(|| self.install_root.join("Emulateurs").join(state::safe_folder_name(&emulator.id, runtime_rom_id)));
        tracing::info!("installation de l'émulateur {} {} dans {}", emulator.name, emulator.version.nom, game_dir.display());
        let manager_client = self.manager_client()?;
        let internal = game_dir.join(INTERNAL_DIR);
        let archive = internal.join("downloads").join(&file.name);
        ensure_space(&game_dir, file.size * 4)?;
        let remote = file.remote();
        let mut last = Instant::now() - Duration::from_secs(1);
        download::download_from(&manager_client, &file.download_path(), &remote, &archive, &self.cancel, |got| {
            if last.elapsed() >= Duration::from_millis(200) || got == remote.size {
                last = Instant::now();
                self.emit(page_id, "download", got, remote.size);
            }
        }, || self.emit_verifying(page_id, remote.size, remote.size))
        .await?;

        let staging = internal.join("staging");
        {
            let (a, s, gd, cancel, cb) = (archive.clone(), staging.clone(), game_dir.clone(), self.cancel.clone(), self.on_progress.clone());
            blocking(move || -> AppResult<u64> {
                let mut last = Instant::now() - Duration::from_secs(1);
                extract::extract(&a, ArchiveKind::Zip, &s, &cancel, |done, total| {
                    if last.elapsed() >= Duration::from_millis(200) || done == total {
                        last = Instant::now();
                        cb(super::Progress { rom_id: page_id, phase: "extract", done, total, verifying: false });
                    }
                })?;
                let root = extract::content_root(&s)?;
                let n = extract::merge_into(&root, &gd)?;
                let _ = std::fs::remove_dir_all(&s);
                Ok(n)
            })
            .await?;
        }
        let _ = std::fs::remove_dir_all(internal.join("downloads"));
        if self.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        self.emit(page_id, "finalize", 0, 0);
        let candidates: Vec<String> = detect::find_candidates(&game_dir, self.target).iter().map(|p| path_to_rel(p)).collect();
        let hint = emulator.launch.exe.as_deref().unwrap_or(&emulator.id);
        let exe = runtimes::pick_exe(&candidates, hint, Some(&emulator.version.nom)).or_else(|| candidates.first().cloned()).ok_or(AppError::NoExecutable)?;
        let exe_rel = rel_to_path(&exe);
        make_executable(&game_dir.join(&exe_rel));
        super::remove_quarantine(&game_dir);
        let exe_dir = emulator.data_dir(&game_dir, &exe);
        for rel in &emulator.launch.portable {
            let marker = exe_dir.join(rel_to_path(rel));
            if !marker.exists() {
                if let Some(parent) = marker.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| disk_error("mode portable", e))?;
                }
                std::fs::write(&marker, b"").map_err(|e| disk_error("mode portable", e))?;
            }
        }
        let now = state::now();
        let st = InstalledGame {
            ways: Vec::new(),
            rom_id: runtime_rom_id,
            name: emulator.name.clone(),
            version: Some(emulator.version.nom.clone()),
            version_raw: Some(emulator.version.cle.clone()),
            target: self.target,
            archive_name: file.name.clone(),
            executable: Some(exe),
            pending_candidates: Vec::new(),
            roms: Vec::new(),
            rom_sources: Vec::new(),
            mods_downloaded: Vec::new(),
            saves: SaveSync::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir,
            pc: None,
            shared: None,
        };
        state::write_state(&self.config_dir, &st)?;
        self.fetch_configs(page_id, emulator, &st).await?;
        Ok(st)
    }

    /// Archives de ce jeu (mods, mise à jour, sauvegarde de départ…), extraites dans le dossier
    /// de l'émulateur. Une archive déjà posée dans la même version n'est pas reprise ; une archive
    /// « si-absent » n'est posée que si son chemin témoin manque.
    async fn apply_packs(&self, rom_id: i64, emulator: &Emulator, st: &InstalledGame) -> AppResult<()> {
        let exe = st.executable.as_deref().ok_or(AppError::NoExecutable)?;
        let exe_dir = emulator.data_dir(&st.game_dir, exe);
        let internal = st.game_dir.join(INTERNAL_DIR);
        let markers = internal.join("packs");
        for pack in emulator.packs_for(rom_id) {
            let dest = exe_dir.join(rel_to_path(pack.file.dest.as_deref().unwrap_or("")));
            if let Some(witness) = &pack.if_missing {
                if dest.join(rel_to_path(witness)).exists() {
                    continue;
                }
            }
            let marker = markers.join(&pack.file.name);
            if std::fs::read_to_string(&marker).is_ok_and(|s| s.trim() == pack.file.sha1) && pack.if_missing.is_none() {
                continue;
            }
            tracing::info!("{} : pose de {} dans {}", emulator.name, pack.file.name, dest.display());
            let archive = internal.join("downloads").join(&pack.file.name);
            ensure_space(&exe_dir, pack.file.size * 2)?;
            let remote = pack.file.remote();
            let client = self.manager_client()?;
            let mut last = Instant::now() - Duration::from_secs(1);
            download::download_from(&client, &pack.file.download_path(), &remote, &archive, &self.cancel, |got| {
                if last.elapsed() >= Duration::from_millis(200) || got == remote.size {
                    last = Instant::now();
                    self.emit(rom_id, "download", got, remote.size);
                }
            }, || self.emit_verifying(rom_id, remote.size, remote.size))
            .await?;
            let staging = internal.join("staging-pack");
            let (a, s, d, cancel, cb) = (archive.clone(), staging.clone(), dest.clone(), self.cancel.clone(), self.on_progress.clone());
            let n = blocking(move || -> AppResult<u64> {
                let mut last = Instant::now() - Duration::from_secs(1);
                extract::extract(&a, ArchiveKind::Zip, &s, &cancel, |done, total| {
                    if last.elapsed() >= Duration::from_millis(200) || done == total {
                        last = Instant::now();
                        cb(super::Progress { rom_id, phase: "extract", done, total, verifying: false });
                    }
                })?;
                let n = extract::merge_all(&s, &d)?;
                let _ = std::fs::remove_dir_all(&s);
                Ok(n)
            })
            .await?;
            let _ = std::fs::remove_file(&archive);
            if self.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            std::fs::create_dir_all(&markers).map_err(|e| disk_error("archives de l'émulateur", e))?;
            std::fs::write(&marker, &pack.file.sha1).map_err(|e| disk_error("archives de l'émulateur", e))?;
            tracing::info!("{} : {} posée ({n} fichiers)", emulator.name, pack.file.name);
        }
        Ok(())
    }

    /// Éléments de configuration (prod.keys…), posés à côté de l'exécutable.
    async fn fetch_configs(&self, page_id: i64, emulator: &Emulator, st: &InstalledGame) -> AppResult<()> {
        if emulator.config.is_empty() {
            return Ok(());
        }
        let exe = st.executable.as_deref().ok_or(AppError::NoExecutable)?;
        let exe_dir = emulator.data_dir(&st.game_dir, exe);
        let client = self.manager_client()?;
        for file in &emulator.config {
            let dest = exe_dir.join(rel_to_path(file.dest.as_deref().unwrap_or(&file.name)));
            let remote = file.remote();
            download::download_from(&client, &file.download_path(), &remote, &dest, &self.cancel, |got| self.emit(page_id, "download", got, remote.size), || {}).await?;
        }
        Ok(())
    }

    fn manager_client(&self) -> AppResult<crate::romm::client::RommClient> {
        self.manager_base.as_deref().map(|b| self.client.with_base(b)).ok_or(AppError::NotAvailable)
    }
}

/// ROM préparée lors d'une installation précédente, depuis la même archive, et toujours en
/// place (et déchiffrée) : ni retéléchargée, ni refaite.
fn prepared_before(previous: Option<&InstalledGame>, main: &RemoteFile, rom_dir: &Path, prep: &RomPrep) -> Option<String> {
    let sha1 = main.sha1.as_deref()?;
    let src = previous?.rom_sources.iter().find(|s| s.sha1.as_deref() == Some(sha1) && s.size == main.size && s.placed.len() == 1)?;
    let placed = &src.placed[0];
    if *placed == path_to_rel(&rom_rel(main)) {
        return None;
    }
    let path = rom_dir.join(rel_to_path(placed));
    let ok = path.is_file() && !(prep.decrypt.as_deref() == Some("3ds") && crate::n3ds::is_encrypted(&path));
    ok.then(|| placed.clone())
}

/// Prépare la ROM téléchargée pour l'émulateur : le zip est remplacé par le plus gros de ses
/// fichiers (la ROM, pas le readme), puis le jeu 3DS est déchiffré sur place.
/// Renvoie le chemin de la ROM, relatif au dossier des ROM.
fn prepare_rom(archive: &Path, rom_dir: &Path, prep: &RomPrep, keys: Option<&Path>, progress: &dyn Fn(&'static str, u64, u64)) -> AppResult<PathBuf> {
    let three_ds = prep.decrypt.as_deref() == Some("3ds");
    let is_zip = archive.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let rom = if prep.unzip && is_zip { unzip_rom(archive, three_ds, progress)? } else { archive.to_path_buf() };
    if three_ds && crate::n3ds::is_rom(&rom) && crate::n3ds::is_encrypted(&rom) {
        let text = keys
            .and_then(|k| std::fs::read_to_string(k).ok())
            .ok_or_else(|| log_err(AppError::KeysMissing, "déchiffrement 3DS", "aes_keys.txt absent du dossier de l'émulateur"))?;
        let keys = crate::n3ds::Keys::parse(&text);
        let started = Instant::now();
        let mut last = Instant::now() - Duration::from_secs(1);
        crate::n3ds::decrypt(&rom, &keys, &mut |done, total| {
            if last.elapsed() >= Duration::from_millis(200) || done == total {
                last = Instant::now();
                progress("decrypt", done, total);
            }
        })
        .map_err(|e| {
            let missing = e.contains("aes_keys.txt");
            // ROM à moitié déchiffrée : inutilisable, elle sera retéléchargée.
            if !missing {
                let _ = std::fs::remove_file(&rom);
            }
            let kind = if missing {
                AppError::KeysMissing
            } else if e.contains(crate::n3ds::DAMAGED) {
                AppError::RomDamaged
            } else {
                AppError::Decrypt
            };
            log_err(kind, "déchiffrement 3DS", e)
        })?;
        tracing::info!("ROM 3DS déchiffrée en {:?} : {}", started.elapsed(), rom.display());
    }
    Ok(rom.strip_prefix(rom_dir).map(Path::to_path_buf).unwrap_or_else(|_| rom.file_name().map(PathBuf::from).unwrap_or_default()))
}

/// Remplace le zip par le plus gros de ses fichiers (une ROM 3DS d'abord, si on en cherche une).
fn unzip_rom(archive: &Path, want_3ds: bool, progress: &dyn Fn(&'static str, u64, u64)) -> AppResult<PathBuf> {
    use std::io::{Read, Write};
    let bad = |e: String| log_err(AppError::Extract, "ROM en zip", e);
    let file = std::fs::File::open(archive).map_err(|e| disk_error("ouverture de la ROM", e))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| bad(e.to_string()))?;
    let mut best: Option<((bool, u64), usize, String)> = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
        if !entry.is_file() {
            continue;
        }
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string)) else { continue };
        let rank = (want_3ds && crate::n3ds::is_rom(Path::new(&name)), entry.size());
        if best.as_ref().is_none_or(|b| rank > b.0) {
            best = Some((rank, i, name));
        }
    }
    let ((_, size), index, name) = best.ok_or_else(|| bad("archive vide".into()))?;
    let dir = archive.parent().unwrap_or(Path::new("."));
    let dest = dir.join(&name);
    let part = dir.join(format!("{name}.part"));
    let mut entry = zip.by_index(index).map_err(|e| bad(e.to_string()))?;
    let mut out = std::fs::File::create(&part).map_err(|e| disk_error("extraction de la ROM", e))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    let mut last = Instant::now() - Duration::from_secs(1);
    loop {
        let n = entry.read(&mut buf).map_err(|e| bad(e.to_string()))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| disk_error("extraction de la ROM", e))?;
        done += n as u64;
        if last.elapsed() >= Duration::from_millis(200) || done == size {
            last = Instant::now();
            progress("extract", done, size);
        }
    }
    drop(out);
    std::fs::rename(&part, &dest).map_err(|e| disk_error("extraction de la ROM", e))?;
    std::fs::remove_file(archive).map_err(|e| disk_error("suppression du zip de la ROM", e))?;
    tracing::info!("ROM extraite de {} : {}", archive.display(), dest.display());
    Ok(dest)
}

/// Fichier déjà téléchargé pour ce jeu : même empreinte, même taille sur le disque.
fn already_placed(previous: Option<&InstalledGame>, file: &RemoteFile, dest: &Path) -> bool {
    let Some(sha1) = file.sha1.as_deref() else { return false };
    let known = previous.is_some_and(|p| p.rom_sources.iter().any(|s| s.sha1.as_deref() == Some(sha1) && s.size == file.size));
    known && std::fs::metadata(dest).is_ok_and(|m| m.len() == file.size)
}

/// ROM posée à côté de la recomp du jeu (installation précédente, sans émulateur).
fn recomp_copy(previous: Option<&InstalledGame>, file: &RemoteFile) -> Option<PathBuf> {
    let p = previous.filter(|p| p.shared.is_none() && p.pc.is_none())?;
    let src = p.rom_sources.iter().find(|s| super::same_rom(s, file) && s.placed.len() == 1)?;
    let old = p.game_dir.join(rel_to_path(&src.placed[0]));
    std::fs::metadata(&old).is_ok_and(|m| m.is_file() && m.len() == file.size).then_some(old)
}

/// Retire la ROM d'un jeu. Ses sauvegardes restent à côté si elles sont conservées.
pub fn remove_rom_files(st: &InstalledGame, shared: &SharedGame, keep_saves: bool) -> AppResult<()> {
    let Some(rom_dir) = shared.rom_dir.as_ref() else { return Ok(()) };
    if !keep_saves {
        if let Err(e) = std::fs::remove_dir_all(rom_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(disk_error("suppression de la ROM", e));
            }
        }
        return Ok(());
    }
    for rom in &st.roms {
        let _ = std::fs::remove_file(rom_dir.join(rel_to_path(rom)));
    }
    // Dossier vide (pas de sauvegarde) : retiré ; sinon il garde les sauvegardes.
    let _ = std::fs::remove_dir(rom_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, size: u64) -> RemoteFile {
        RemoteFile { id: 1, file_name: name.into(), rel_path: name.into(), size, sha1: None, md5: None, from_manager: false }
    }

    #[test]
    fn best_cartridge_of_a_goodtools_folder() {
        let yoshi = [
            f("Super Mario - Yoshi Island (J) (V1.0).smc", 2),
            f("Super Mario - Yoshi Island (J) (V1.1) [!].smc", 2),
            f("Super Mario World 2 - Yoshi's Island (E) (M3) (V1.0) [f1].smc", 2),
            f("Super Mario World 2 - Yoshi's Island (E) (M3) (V1.0) [!].smc", 2),
            f("Super Mario World 2 - Yoshi's Island (U) (M3) (V1.0) [!].smc", 2),
            f("Super Mario World 2 - Yoshi's Island (E) (M3) (V1.0) [h1C].smc", 2),
        ];
        let got = emulator_files(&yoshi);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].file_name, "Super Mario World 2 - Yoshi's Island (E) (M3) (V1.0) [!].smc");
        let smw = [f("Archie 2K's World (SMW1 Hack).smc", 9), f("Super Mario World (U) [!].smc", 1), f("Super Mario World (F) [!].smc", 1), f("Super Mario World (E) [!].smc", 1)];
        assert_eq!(emulator_files(&smw)[0].file_name, "Super Mario World (F) [!].smc");
        assert_eq!(cartridge_score("New Super Mario Bros. (Europe) (En,Fr,De,Es,It).nds"), 12);
        // Un disque garde tous ses fichiers.
        let disc = [f("Jeu (Track 1).bin", 5), f("Jeu (Track 2).bin", 5), f("Jeu.cue", 1)];
        assert_eq!(emulator_files(&disc).len(), 3);
        assert_eq!(main_rom(&disc).unwrap().file_name, "Jeu.cue");
    }

    #[test]
    fn picks_the_game_among_its_extras() {
        let mut patch = f("Super Smash Bros Ultimate [01006A800016E800][v1966080][US].nsp", 3_900);
        patch.rel_path = "patch/Super Smash Bros Ultimate [01006A800016E800][v1966080][US].nsp".into();
        let files = vec![
            f("CitronSSBU.v1 [linux].zip", 5_000),
            f("Super Smash Bros Ultimate[01006A800016E000][US][v0].nsp", 14_000),
            f("dlc.zip", 2),
            f("mods_suyu_backup.zip", 5_000),
            patch,
        ];
        assert_eq!(main_rom(&files).unwrap().file_name, "Super Smash Bros Ultimate[01006A800016E000][US][v0].nsp");
    }

    #[test]
    fn picks_the_file_given_to_the_emulator() {
        assert_eq!(main_rom(&[f("a.gba", 1)]).unwrap().file_name, "a.gba");
        assert_eq!(main_rom(&[f("a.bin", 900), f("a.cue", 1)]).unwrap().file_name, "a.cue");
        assert_eq!(main_rom(&[f("d1.cue", 1), f("d2.cue", 1), f("g.m3u", 1)]).unwrap().file_name, "g.m3u");
        assert_eq!(main_rom(&[f("readme.txt", 1), f("g.iso", 9)]).unwrap().file_name, "g.iso");
        assert!(main_rom(&[]).is_none());
    }

    #[test]
    fn rom_paths_lose_the_portage_folder() {
        let mut x = f("x.gba", 1);
        x.rel_path = "ROM/x.gba".into();
        assert_eq!(rom_rel(&x), PathBuf::from("x.gba"));
        x.rel_path = "Disc 1/x.bin".into();
        assert_eq!(rom_rel(&x), PathBuf::from("Disc 1").join("x.bin"));
    }

    #[test]
    fn switch_base_game_with_its_update_and_dlc() {
        let files = [
            f("Castlevania Belmonts Curse [0100BA4026E94800][v65536][Up v1.1.0].nsp", 3182299415),
            f("Castlevania Belmonts Curse [0100BA4026E94000][v0].nsp", 560321310),
            f("Castlevania Belmonts Curse [Alucard Style Costume DLC][0100BA4026E95001].nsp", 124112),
            f("notes.nfo", 10),
        ];
        assert_eq!(main_rom(&files).unwrap().file_name, files[1].file_name);
        let names: Vec<&str> = emulator_files(&files).iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(names.len(), 3, "jeu, mise à jour et DLC, sans le .nfo : {names:?}");
        // Sans ID de titre : la mise à jour se reconnaît à sa version.
        let pika = [f("Pokemon Lets Go Pikachu v131072.nsp", 65125332), f("Pokemon Lets Go Pikachu.xci", 7985954816)];
        assert_eq!(main_rom(&pika).unwrap().file_name, "Pokemon Lets Go Pikachu.xci");
        let botw = [
            f("The Legend of Zelda Breath of the Wild 1.9.0 [01007EF00011E800][v1114112].nsp", 991994208),
            f("The Legend of Zelda Breath of the Wild [01007EF00011E000][v0].nsp", 14476256004),
            f("The Legend of Zelda Breath of the Wild DLC Pack [2 DLCs].zip", 1393309644),
        ];
        assert_eq!(main_rom(&botw).unwrap().file_name, botw[1].file_name);
        assert_eq!(emulator_files(&botw).len(), 2, "le zip n'est pas lisible par l'émulateur");
        // Autres consoles : la seule cartouche retenue, comme avant.
        let gba = [f("Jeu (Europe).gba", 4), f("Jeu (USA).gba", 4)];
        assert_eq!(emulator_files(&gba).len(), 1);
    }

    #[test]
    fn zipped_3ds_rom_replaces_its_zip() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("farfa-rom-prep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zpath = dir.join("Jeu (Europe).zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zpath).unwrap());
            let o: zip::write::SimpleFileOptions = Default::default();
            // Le readme est plus gros : c'est quand même la ROM qui est prise.
            w.start_file("readme.txt", o).unwrap();
            w.write_all(&[b'x'; 64]).unwrap();
            w.start_file("Jeu (Europe) Decrypted.3ds", o).unwrap();
            w.write_all(b"pas une vraie ROM").unwrap();
            w.finish().unwrap();
        }
        let prep = RomPrep { unzip: true, decrypt: Some("3ds".into()), keys: None };
        let rel = prepare_rom(&zpath, &dir, &prep, None, &|_, _, _| {}).unwrap();
        assert_eq!(rel, PathBuf::from("Jeu (Europe) Decrypted.3ds"));
        assert!(!zpath.exists() && dir.join(&rel).is_file() && !dir.join("readme.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rom_folders_have_no_extension() {
        assert_eq!(rom_folder("Jeu v1.2 (USA).gba", &[f("Jeu v1.2 (USA).gba", 1)]), "Jeu v1.2 (USA)");
        // Dossier de plusieurs fichiers : son nom est gardé tel quel, même avec un point.
        assert_eq!(rom_folder("Jeu v1.2", &[f("a.bin", 1), f("a.cue", 1)]), "Jeu v1.2");
        assert_eq!(rom_folder("Pokemon Red", &[f("r.gb", 1)]), "Pokemon Red");
    }

    #[test]
    fn expands_launch_values() {
        let rom = Path::new("/r/Mon Jeu.gba");
        assert_eq!(expand("{rom}", rom, Path::new("/r"), "Mon Jeu"), "/r/Mon Jeu.gba");
        assert_eq!(expand("{romstem}.sav", rom, Path::new("/r"), stem("Mon Jeu.gba")), "Mon Jeu.sav");
    }

    #[test]
    fn keeping_saves_leaves_them_next_to_the_rom() {
        let root = std::env::temp_dir().join(format!("farfa-emu-{}", uuid::Uuid::new_v4()));
        let rom_dir = root.join("Roms/gba/Jeu");
        std::fs::create_dir_all(&rom_dir).unwrap();
        std::fs::write(rom_dir.join("Jeu.gba"), b"r").unwrap();
        std::fs::write(rom_dir.join("Jeu.sav"), b"s").unwrap();
        let shared = SharedGame {
            runtime_rom_id: -1, game: "m".into(), runtime_name: "m".into(), data_dir: String::new(), save_dir: String::new(), cache_dir: None,
            kind: Some(KIND_EMULATOR.into()), rom_dir: Some(rom_dir.clone()), saves_dir: Some(rom_dir.clone()), save_patterns: vec!["Jeu.sav".into()],
        };
        let st = InstalledGame {
            ways: Vec::new(),
            rom_id: 1, name: "J".into(), version: None, version_raw: None, target: crate::platform::Target::Windows, archive_name: String::new(),
            executable: None, pending_candidates: vec![], roms: vec!["Jeu.gba".into()], rom_sources: vec![], mods_downloaded: vec![], saves: SaveSync::default(),
            direct_launch: None, prefer_launcher: false, players: Vec::new(), installed_at: 0, updated_at: 0, pc: None, shared: Some(shared.clone()), game_dir: root.clone(),
        };
        remove_rom_files(&st, &shared, true).unwrap();
        assert!(!rom_dir.join("Jeu.gba").exists() && rom_dir.join("Jeu.sav").exists());
        remove_rom_files(&st, &shared, false).unwrap();
        assert!(!rom_dir.exists());
        // Déjà parti : pas une erreur.
        remove_rom_files(&st, &shared, false).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Jeu installé par un launcher qui ne préparait pas la ROM (1.2.0) : zip lancé tel quel.
    #[tokio::test]
    async fn zip_left_by_an_old_launcher_is_prepared_at_launch() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("farfa-heal-{}", uuid::Uuid::new_v4()));
        let rom_dir = root.join("Roms/3ds/Jeu");
        let emu_dir = root.join("Emulateurs/azahar");
        std::fs::create_dir_all(&rom_dir).unwrap();
        std::fs::create_dir_all(&emu_dir).unwrap();
        std::fs::write(emu_dir.join("azahar.exe"), b"x").unwrap();
        let zpath = rom_dir.join("Jeu.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zpath).unwrap());
            w.start_file("Jeu Decrypted.3ds", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"rom").unwrap();
            w.finish().unwrap();
        }
        let emulator: Emulator = serde_json::from_value(serde_json::json!({
            "id": "azahar", "name": "Azahar", "consoles": ["3ds"], "version": {"cle": "1", "nom": "1"},
            "romPrep": {"unzip": true, "decrypt": "3ds", "keys": "user/sysdata/aes_keys.txt"},
            "saves": {"dir": "user/sdmc", "patterns": ["{discid}"]}
        }))
        .unwrap();
        let shared = SharedGame {
            runtime_rom_id: -1, game: "azahar".into(), runtime_name: "Azahar".into(), data_dir: String::new(), save_dir: String::new(), cache_dir: None,
            kind: Some(KIND_EMULATOR.into()), rom_dir: Some(rom_dir.clone()), saves_dir: None, save_patterns: vec![],
        };
        let zip_arg = zpath.to_string_lossy().into_owned();
        let st = InstalledGame {
            ways: Vec::new(),
            rom_id: 7, name: "Jeu".into(), version: None, version_raw: None, target: crate::platform::Target::Windows, archive_name: String::new(),
            executable: Some("azahar.exe".into()), pending_candidates: vec![], roms: vec!["Jeu.zip".into()],
            rom_sources: vec![RomSource { rel_path: "Jeu.zip".into(), size: 1, sha1: Some("ab".into()), md5: None, placed: vec!["Jeu.zip".into()] }],
            mods_downloaded: vec![], saves: SaveSync::default(),
            direct_launch: Some(DirectLaunch { exe: "azahar.exe".into(), args: vec!["-f".into(), zip_arg], cwd: None, env: vec![] }),
            prefer_launcher: false, players: Vec::new(), installed_at: 0, updated_at: 0, pc: None, shared: Some(shared), game_dir: emu_dir.clone(),
        };
        let config = root.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let healed = heal_rom(&config, &emulator, &st).await.unwrap().expect("ROM préparée");
        let rom = rom_dir.join("Jeu Decrypted.3ds");
        assert!(rom.is_file() && !zpath.exists());
        assert_eq!(healed.roms, vec!["Jeu Decrypted.3ds".to_string()]);
        assert_eq!(healed.rom_sources[0].placed, vec!["Jeu Decrypted.3ds".to_string()]);
        assert_eq!(healed.direct_launch.as_ref().unwrap().args, vec!["-f".to_string(), rom.to_string_lossy().into_owned()]);
        // Déjà préparée : plus rien à faire au lancement suivant.
        assert!(heal_rom(&config, &emulator, &healed).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod disc_tests {
    use super::*;

    #[test]
    fn reads_disc_ids_and_names_dolphin_folders() {
        let dir = std::env::temp_dir().join(format!("farfa-disc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rvz = b"RVZ\x01".to_vec();
        rvz.resize(0x58, 0);
        rvz.extend_from_slice(b"GZ2P01\0\0");
        std::fs::write(dir.join("tp.rvz"), &rvz).unwrap();
        std::fs::write(dir.join("sms.iso"), b"GMSE01\0\0rest").unwrap();
        std::fs::write(dir.join("x.nes"), b"NES\x1a").unwrap();
        assert_eq!(disc_id(&dir.join("tp.rvz")).as_deref(), Some("GZ2P01"));
        assert_eq!(disc_id(&dir.join("sms.iso")).as_deref(), Some("GMSE01"));
        assert_eq!(disc_id(&dir.join("x.nes")), None);
        assert_eq!(expand_disc("GC/{region}/Card A", Some("GZ2P01")).as_deref(), Some("GC/EUR/Card A"));
        assert_eq!(expand_disc("*-{gameid}-*.gci", Some("GMSE01")).as_deref(), Some("*-GMSE-*.gci"));
        assert_eq!(expand_disc("{gameidhex}", Some("SMNP01")).as_deref(), Some("534d4e50"));
        // Sans identifiant lisible : pas de dossier deviné au hasard.
        assert_eq!(expand_disc("GC/{region}", None), None);
        assert_eq!(expand_disc("prb", None).as_deref(), Some("prb"));
        // ID trop court ou coupé dans un caractère : rien, sans paniquer (L12).
        assert_eq!(expand_disc("GC/{region}", Some("GZ")), None);
        assert_eq!(expand_disc("{gameid}", Some("GZXé1")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn picks_the_saves_of_the_console() {
        let saves: crate::manager::EmulatorSaves = serde_json::from_str(
            r#"{"dir":"User","patterns":[],"variants":[
              {"system":"windows","console":"ngc","dir":"User/GC/{region}/Card A","patterns":["*-{gameid}-*.gci"]},
              {"system":"linux","console":"ngc","dir":"{appdata}/dolphin-emu/GC/{region}/Card A","patterns":["*-{gameid}-*.gci"]},
              {"system":"windows","console":"wii","dir":"User/Wii/title/00010000","patterns":["{gameidhex}"]},
              {"system":"linux","console":"wii","dir":"{appdata}/dolphin-emu/Wii/title/00010000","patterns":["{gameidhex}"]}]}"#,
        )
        .unwrap();
        let (dir, patterns) = saves.for_console("wii");
        if cfg!(windows) {
            assert_eq!(dir, Some("User/Wii/title/00010000"));
            assert_eq!(patterns, ["{gameidhex}"]);
        } else if cfg!(target_os = "linux") {
            assert_eq!(dir, Some("{appdata}/dolphin-emu/Wii/title/00010000"));
            assert_eq!(patterns, ["{gameidhex}"]);
        } else {
            // Mac sans ligne « macos » : le dossier commun.
            assert_eq!(dir, Some("User"));
        }
        assert_eq!(saves.for_console("gba").0, Some("User"));
    }
}

#[cfg(test)]
mod psp_tests {
    /// `FARFA_E2E_CHD=<jeu PSP .chd> cargo test -- --ignored psp_chd_id --nocapture`.
    #[test]
    #[ignore]
    fn psp_chd_id() {
        let path = std::path::PathBuf::from(std::env::var("FARFA_E2E_CHD").unwrap());
        let id = super::disc_id(&path);
        println!("{} → {id:?}", path.display());
        assert!(id.is_some_and(|i| i.len() == 9));
    }

    /// `FARFA_E2E_PS2=<jeu .chd>;<autre .chd>… cargo test -- --ignored ps2_chd_ids --nocapture`.
    #[test]
    #[ignore]
    fn ps2_chd_ids() {
        for p in std::env::var("FARFA_E2E_PS2").unwrap().split(';') {
            let id = super::disc_id(std::path::Path::new(p));
            println!("{p} → {id:?}");
            assert!(id.is_some_and(|i| i.len() >= 9 && i.as_bytes()[4] == b'-'));
        }
    }

    #[test]
    fn ps2_serials() {
        assert_eq!(super::ps2_serial("BOOT2 = cdrom0:\\SLES_503.30;1\r\nVER = 1.00\r\n").as_deref(), Some("SLES-50330"));
        assert_eq!(super::ps2_serial("boot2=cdrom0:\\SCUS_971.13;1").as_deref(), Some("SCUS-97113"));
        assert_eq!(super::ps2_serial("BOOT = cdrom:\\SLUS_012.34;1"), None);
    }

    #[test]
    fn psp_iso_id() {
        // Disque ISO 9660 minimal : PVD au secteur 16, racine au 18, UMD_DATA.BIN au 19.
        let mut iso = vec![0u8; 20 * 2048];
        let pvd = 16 * 2048;
        iso[pvd] = 1;
        iso[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        iso[pvd + 156 + 2..pvd + 156 + 6].copy_from_slice(&18u32.to_le_bytes());
        iso[pvd + 156 + 10..pvd + 156 + 14].copy_from_slice(&2048u32.to_le_bytes());
        let dir = 18 * 2048;
        let name = b"UMD_DATA.BIN;1";
        iso[dir] = (33 + name.len()) as u8;
        iso[dir + 2..dir + 6].copy_from_slice(&19u32.to_le_bytes());
        iso[dir + 32] = name.len() as u8;
        iso[dir + 33..dir + 33 + name.len()].copy_from_slice(name);
        iso[19 * 2048..19 * 2048 + 15].copy_from_slice(b"ULES-00151|0001");
        let path = std::env::temp_dir().join(format!("farfa-psp-{}.iso", uuid::Uuid::new_v4()));
        std::fs::write(&path, &iso).unwrap();
        assert_eq!(super::disc_id(&path).as_deref(), Some("ULES00151"));
        assert_eq!(super::expand_disc("{discid}*", Some("ULES00151")).as_deref(), Some("ULES00151*"));
        std::fs::remove_file(&path).unwrap();
    }
}
