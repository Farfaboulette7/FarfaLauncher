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
use crate::manager::{self, Emulator};
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

/// Émulateur en bundle `.app` (Mac) : lancé depuis le dossier qui contient le bundle, où
/// les dérivés de yuzu cherchent `user/` et mGBA son `portable.ini`.
fn bundle_cwd(exe: &str) -> Option<String> {
    if !cfg!(target_os = "macos") || !exe.to_ascii_lowercase().ends_with(".app") {
        return None;
    }
    Some(exe.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default())
}

/// Identifiant d'un disque : GameCube ou Wii (`GZ2P01`, dans son en-tête), PSP
/// (`ULES00151`, dans `UMD_DATA.BIN`) ou PS2 (`SLES-50330`, dans `SYSTEM.CNF`),
/// aussi au format CHD.
pub fn disc_id(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
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
    let short = &id[..4];
    let region = match id.as_bytes()[3] {
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
        let previous = state::find(&self.config_dir, rom_id);
        let emulator = self.pick_emulator(&game.console, previous.as_ref()).ok_or(AppError::NotAvailable)?;
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
        let total: u64 = files.iter().map(|f| f.size).sum();
        ensure_space(&rom_dir, total)?;
        let mut offset = 0;
        let mut last = Instant::now() - Duration::from_secs(1);
        // Jeu joué jusqu'ici avec sa recomp : la ROM posée à côté d'elle est reprise.
        let mut reused: Vec<(PathBuf, String)> = Vec::new();
        for file in files {
            let dest = rom_dir.join(rom_rel(file));
            // Même ROM déjà en place (changement d'émulateur) : ni retéléchargée, ni relue en entier.
            if already_placed(previous.as_ref(), file, &dest) {
                offset += file.size;
                self.emit(rom_id, "download", offset, total);
                continue;
            }
            if let Some(old) = recomp_copy(previous.as_ref(), file) {
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

        self.emit(rom_id, "finalize", 0, 0);
        let main = main_rom(&game.rom_files).ok_or(AppError::NotAvailable)?;
        let rom_path = rom_dir.join(rom_rel(main));
        let rom_stem = stem(&main.file_name);
        let disc = disc_id(&rom_path);
        let (save_dir, save_patterns) = emulator.saves.for_console(&game.console);
        let args = emulator.launch.args.iter().map(|a| expand(a, &rom_path, &rom_dir, rom_stem)).collect();
        let env = emulator.launch.env.iter().map(|(k, v)| (k.clone(), expand(v, &rom_path, &rom_dir, rom_stem))).collect();
        let now = state::now();
        let placed: Vec<String> = game.rom_files.iter().map(|f| path_to_rel(&rom_rel(f))).collect();
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
                save_patterns: save_patterns.iter().filter_map(|p| expand_disc(&expand(p, &rom_path, &rom_dir, rom_stem), disc.as_deref())).collect(),
            }),
        };
        state::write_state(&self.config_dir, &st)?;
        super::remove_reused(&reused);
        tracing::info!("« {} » installé avec {} dans {}", st.name, emulator.name, rom_dir.display());
        let st = state::find(&self.config_dir, rom_id).ok_or(AppError::Internal)?;
        Ok(InstallOutcome::Installed { game: st })
    }

    /// Émulateur à utiliser : celui choisi, sinon celui du jeu déjà installé, sinon le premier de la console.
    fn pick_emulator(&self, console: &str, previous: Option<&InstalledGame>) -> Option<Emulator> {
        let of_console = |id: &str| self.catalog.emulator(id).filter(|e| e.consoles.iter().any(|c| c == console));
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
