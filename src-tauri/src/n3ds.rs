//! Jeux 3DS : déchiffrement d'une ROM (`.3ds` / `.cci` / `.cxi`) avant de la donner à
//! Azahar, qui ne lit que les jeux déchiffrés, et identifiant du jeu (sauvegardes).
//!
//! Les clés viennent d'un `aes_keys.txt` (format de Citra : `generator=…`,
//! `slot0x2CKeyX=…`…) servi par FarfaRomManager (`dechiffrer:` de la fiche). Le jeu est
//! déchiffré sur place, morceau par morceau : pas de copie du fichier.
//! Référence : 3dbrew, pages NCSD et NCCH.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use aes::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use sha2::{Digest, Sha256};

type Ctr = ctr::Ctr128BE<aes::Aes128>;

const MEDIA: u64 = 0x200;
const CHUNK: usize = 4 * 1024 * 1024;

/// Clés lues dans `aes_keys.txt`.
#[derive(Debug, Clone, Default)]
pub struct Keys {
    generator: Option<u128>,
    key_x: HashMap<u8, u128>,
}

impl Keys {
    pub fn parse(text: &str) -> Keys {
        let mut keys = Keys::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ':', ';']) {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else { continue };
            let Ok(value) = u128::from_str_radix(value.trim(), 16) else { continue };
            let name = name.trim().to_ascii_lowercase();
            if name == "generator" || name == "generatorconstant" {
                keys.generator = Some(value);
            } else if let Some(slot) = name.strip_prefix("slot0x").and_then(|s| s.strip_suffix("keyx")) {
                if let Ok(slot) = u8::from_str_radix(slot, 16) {
                    keys.key_x.insert(slot, value);
                }
            }
        }
        keys
    }

    /// Clé normale d'un emplacement : ((X <<< 2) ^ Y) + constante, puis <<< 87.
    fn normal(&self, slot: u8, key_y: u128) -> Result<u128, String> {
        let generator = self.generator.ok_or("clés 3DS : « generator » manque dans aes_keys.txt")?;
        let x = *self.key_x.get(&slot).ok_or_else(|| format!("clés 3DS : « slot0x{slot:02X}KeyX » manque dans aes_keys.txt"))?;
        Ok((x.rotate_left(2) ^ key_y).wrapping_add(generator).rotate_left(87))
    }
}

/// Une ROM que ce module sait lire, d'après son extension.
pub fn is_rom(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "3ds" | "cci" | "cxi"))
}

/// Début de chaque NCCH du fichier : les partitions d'un NCSD (`.3ds`, `.cci`), ou le
/// fichier lui-même (`.cxi`).
fn partitions(file: &mut File) -> Option<Vec<u64>> {
    let mut head = [0u8; 0x200];
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_exact(&mut head).ok()?;
    match &head[0x100..0x104] {
        b"NCSD" => Some(
            (0..8)
                .map(|i| u64::from(u32::from_le_bytes(head[0x120 + i * 8..0x124 + i * 8].try_into().unwrap())) * MEDIA)
                .zip((0..8).map(|i| u32::from_le_bytes(head[0x124 + i * 8..0x128 + i * 8].try_into().unwrap())))
                .filter(|(_, size)| *size > 0)
                .map(|(offset, _)| offset)
                .collect(),
        ),
        b"NCCH" => Some(vec![0]),
        _ => None,
    }
}

fn ncch_header(file: &mut File, at: u64) -> Option<[u8; 0x200]> {
    let mut h = [0u8; 0x200];
    file.seek(SeekFrom::Start(at)).ok()?;
    file.read_exact(&mut h).ok()?;
    (&h[0x100..0x104] == b"NCCH").then_some(h)
}

fn u32_at(h: &[u8], at: usize) -> u64 {
    u64::from(u32::from_le_bytes(h[at..at + 4].try_into().unwrap()))
}

/// ID de titre du jeu (`000400000006a900`), lu dans son premier NCCH.
pub fn title_id(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let first = *partitions(&mut file)?.first()?;
    let h = ncch_header(&mut file, first)?;
    Some(u64::from_le_bytes(h[0x118..0x120].try_into().unwrap()))
}

/// Dossier des sauvegardes du jeu dans Azahar : la moitié basse de son ID de titre
/// (`sdmc/Nintendo 3DS/…/title/00040000/0006a900`).
pub fn save_id(path: &Path) -> Option<String> {
    title_id(path).map(|id| format!("{:08x}", id & 0xffff_ffff))
}

/// La ROM a-t-elle encore une partie chiffrée ?
pub fn is_encrypted(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else { return false };
    let Some(parts) = partitions(&mut file) else { return false };
    parts.into_iter().any(|at| ncch_header(&mut file, at).is_some_and(|h| h[0x18F] & 0x04 == 0))
}

/// Compteur AES-CTR d'une section (1 : en-tête étendu, 2 : ExeFS, 3 : RomFS).
fn counter(h: &[u8], kind: u8, offset: u64) -> u128 {
    let mut ctr = [0u8; 16];
    let partition = &h[0x108..0x110];
    if u16::from_le_bytes([h[0x112], h[0x113]]) == 1 {
        ctr[..8].copy_from_slice(partition);
        ctr[12..].copy_from_slice(&(offset as u32).to_be_bytes());
    } else {
        for (i, b) in partition.iter().rev().enumerate() {
            ctr[i] = *b;
        }
        ctr[8] = kind;
    }
    u128::from_be_bytes(ctr)
}

/// Déchiffre `len` octets à `at` (dans le fichier), avec le compteur du début de la section
/// `start` : la section commence à `start` et le morceau à `at`.
fn apply(file: &mut File, key: u128, ctr: u128, start: u64, at: u64, len: u64, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let mut cipher = Ctr::new(&key.to_be_bytes().into(), &ctr.to_be_bytes().into());
    cipher.seek(at - start);
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(CHUNK as u64) as usize;
        file.seek(SeekFrom::Start(at + done)).map_err(|e| e.to_string())?;
        file.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
        cipher.apply_keystream(&mut buf[..n]);
        file.seek(SeekFrom::Start(at + done)).map_err(|e| e.to_string())?;
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(n as u64);
    }
    Ok(())
}

/// Emplacement de la clé secondaire (ExeFS `.code`, RomFS).
fn secondary_slot(method: u8) -> Result<u8, String> {
    match method {
        0x00 => Ok(0x2C),
        0x01 => Ok(0x25),
        0x0A => Ok(0x18),
        0x0B => Ok(0x1B),
        m => Err(format!("chiffrement 3DS inconnu ({m:#04x})")),
    }
}

/// Contenu déjà en clair malgré le drapeau : certains dumps « Decrypted » ont déchiffré le
/// jeu sans poser « NoCrypto » (le manuel, lui, est resté chiffré). La RomFS en clair commence
/// par `IVFC`, l'en-tête de l'ExeFS par le nom de son premier fichier (`.code`…).
fn already_plain(file: &mut File, at: u64, h: &[u8]) -> bool {
    let mut read = |offset: u64, len: usize| -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(at + offset)).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    if u32_at(h, 0x1B4) > 0 {
        return read(u32_at(h, 0x1B0) * MEDIA, 4).is_some_and(|m| m == b"IVFC");
    }
    if u32_at(h, 0x1A4) > 0 {
        return read(u32_at(h, 0x1A0) * MEDIA, 8).is_some_and(|n| {
            let name = n.split(|b| *b == 0).next().unwrap_or(&[]);
            matches!(name, b".code" | b"icon" | b"banner" | b"logo")
        });
    }
    false
}

/// Taille à déchiffrer, pour la progression.
fn encrypted_size(file: &mut File, parts: &[u64]) -> u64 {
    let mut total = 0;
    for &at in parts {
        if let Some(h) = ncch_header(file, at).filter(|h| h[0x18F] & 0x04 == 0) {
            if !already_plain(file, at, &h) {
                total += (u32_at(&h, 0x1A4) + u32_at(&h, 0x1B4)) * MEDIA + 0x800;
            }
        }
    }
    total
}

/// Déchiffre la ROM sur place. `progress(fait, total)` en octets. Ne fait rien si elle
/// est déjà déchiffrée. En cas d'échec au milieu, la ROM est à retélécharger.
pub fn decrypt(path: &Path, keys: &Keys, progress: &mut dyn FnMut(u64, u64)) -> Result<bool, String> {
    let mut file = File::options().read(true).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let parts = partitions(&mut file).ok_or("ce n'est pas une ROM 3DS (ni NCSD ni NCCH)")?;
    if !parts.iter().any(|&at| ncch_header(&mut file, at).is_some_and(|h| h[0x18F] & 0x04 == 0)) {
        return Ok(false);
    }
    // Avant toute écriture : chaque en-tête étendu doit retrouver son empreinte, sinon le dump est
    // inutilisable (Azahar planterait) et la ROM reste telle quelle.
    for &at in &parts {
        if let Some(h) = ncch_header(&mut file, at).filter(|h| h[0x18F] & 0x04 == 0) {
            check_exheader(&mut file, at, &h, keys)?;
        }
    }
    let total = encrypted_size(&mut file, &parts).max(1);
    let mut done = 0u64;
    for at in parts {
        let Some(mut h) = ncch_header(&mut file, at) else { continue };
        let flags = h[0x18F];
        if flags & 0x04 != 0 {
            continue;
        }
        if already_plain(&mut file, at, &h) {
            tracing::info!("partition 3DS à {at:#x} déjà en clair : seulement marquée déchiffrée");
        } else {
            decrypt_ncch(&mut file, at, &h, keys, &mut |n| {
                done += n;
                progress(done, total);
            })?;
        }
        // Marquée déchiffrée : plus de clé secondaire ni de clé fixe, « NoCrypto ».
        h[0x18B] = 0;
        h[0x18F] = (flags & !0x21) | 0x04;
        file.seek(SeekFrom::Start(at + 0x188)).map_err(|e| e.to_string())?;
        file.write_all(&h[0x188..0x190]).map_err(|e| e.to_string())?;
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(true)
}

/// Message de l'erreur d'un dump abîmé (repérée par l'appelant).
pub const DAMAGED: &str = "dump 3DS abîmé";

/// L'en-tête étendu (en clair, ou déchiffré en mémoire) a-t-il l'empreinte notée dans l'en-tête ?
fn check_exheader(file: &mut File, at: u64, h: &[u8; 0x200], keys: &Keys) -> Result<(), String> {
    if u32_at(h, 0x180) == 0 {
        return Ok(());
    }
    let mut exh = vec![0u8; 0x400];
    file.seek(SeekFrom::Start(at + 0x200)).map_err(|e| e.to_string())?;
    file.read_exact(&mut exh).map_err(|e| e.to_string())?;
    if Sha256::digest(&exh)[..] == h[0x160..0x180] {
        return Ok(());
    }
    if already_plain(file, at, h) {
        return Err(format!("{DAMAGED} : en-tête étendu illisible"));
    }
    let key = if h[0x18F] & 0x01 != 0 { 0 } else { keys.normal(0x2C, u128::from_be_bytes(h[..16].try_into().unwrap()))? };
    let mut cipher = Ctr::new(&key.to_be_bytes().into(), &counter(h, 1, 0x200).to_be_bytes().into());
    cipher.apply_keystream(&mut exh);
    if Sha256::digest(&exh)[..] == h[0x160..0x180] {
        Ok(())
    } else {
        Err(format!("{DAMAGED} : en-tête étendu indéchiffrable (empreinte fausse)"))
    }
}

/// Déchiffre un NCCH : en-tête étendu, ExeFS et RomFS.
fn decrypt_ncch(file: &mut File, at: u64, h: &[u8; 0x200], keys: &Keys, step: &mut dyn FnMut(u64)) -> Result<(), String> {
    let flags = h[0x18F];
    if flags & 0x20 != 0 {
        return Err("ce jeu demande une graine (seeddb), pas encore prise en charge".into());
    }
    {
        let key_y = u128::from_be_bytes(h[..16].try_into().unwrap());
        let (primary, secondary) = if flags & 0x01 != 0 {
            (0, 0) // clé fixe (jeux de développement) : clé nulle
        } else {
            (keys.normal(0x2C, key_y)?, keys.normal(secondary_slot(h[0x18B])?, key_y)?)
        };
        // En-tête étendu et descripteur d'accès.
        if u32_at(h, 0x180) > 0 {
            let ex = at + 0x200;
            apply(file, primary, counter(h, 1, 0x200), ex, ex, 0x800, step)?;
        }
        // ExeFS : son en-tête, l'icône et la bannière avec la clé principale, le reste avec la secondaire.
        let (exefs_off, exefs_len) = (u32_at(h, 0x1A0) * MEDIA, u32_at(h, 0x1A4) * MEDIA);
        if exefs_len > 0 {
            let start = at + exefs_off;
            let ctr = counter(h, 2, exefs_off);
            apply(file, primary, ctr, start, start, 0x200, step)?;
            let mut header = [0u8; 0x200];
            file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
            file.read_exact(&mut header).map_err(|e| e.to_string())?;
            for entry in header[..0xA0].chunks(16) {
                let name = std::str::from_utf8(&entry[..8]).unwrap_or("").trim_end_matches('\0');
                let (offset, size) = (u32_at(entry, 8), u32_at(entry, 12));
                if name.is_empty() || size == 0 {
                    continue;
                }
                let key = if matches!(name, "icon" | "banner") { primary } else { secondary };
                let len = size.div_ceil(MEDIA) * MEDIA;
                apply(file, key, ctr, start, start + 0x200 + offset, len.min(exefs_len.saturating_sub(0x200 + offset)), step)?;
            }
        }
        // RomFS : clé secondaire.
        let (romfs_off, romfs_len) = (u32_at(h, 0x1B0) * MEDIA, u32_at(h, 0x1B4) * MEDIA);
        if romfs_len > 0 {
            let start = at + romfs_off;
            apply(file, secondary, counter(h, 3, romfs_off), start, start, romfs_len, step)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEN: u128 = 0x0123_4567_89ab_cdef_0011_2233_4455_6677;

    fn keys() -> Keys {
        Keys::parse(&format!(
            "# test\n:AES\ngenerator={GEN:032X}\nslot0x2CKeyX=000102030405060708090A0B0C0D0E0F\nslot0x25KeyX=F0E0D0C0B0A090807060504030201000\n"
        ))
    }

    /// NCSD d'une partition : en-tête étendu, ExeFS (icon + .code) et RomFS, en clair.
    fn plain_rom() -> Vec<u8> {
        let ncch = 0x4000usize;
        let mut d = vec![0u8; ncch + 0x200 + 0x800 + 0x600 + 0x400];
        d[0x100..0x104].copy_from_slice(b"NCSD");
        d[0x120..0x124].copy_from_slice(&((ncch as u32) / 0x200).to_le_bytes());
        let parts = ((d.len() - ncch) as u32) / 0x200;
        d[0x124..0x128].copy_from_slice(&parts.to_le_bytes());
        let h = &mut d[ncch..ncch + 0x200];
        for (i, b) in h[..0x100].iter_mut().enumerate() {
            *b = i as u8 ^ 0x5a;
        }
        h[0x100..0x104].copy_from_slice(b"NCCH");
        h[0x108..0x110].copy_from_slice(&0x0004_0000_0006_a900u64.to_le_bytes());
        h[0x118..0x120].copy_from_slice(&0x0004_0000_0006_a900u64.to_le_bytes());
        h[0x180..0x184].copy_from_slice(&0x400u32.to_le_bytes());
        h[0x18B] = 0x01;
        h[0x18F] = 0x04;
        h[0x1A0..0x1A4].copy_from_slice(&5u32.to_le_bytes()); // 0xA00
        h[0x1A4..0x1A8].copy_from_slice(&3u32.to_le_bytes());
        h[0x1B0..0x1B4].copy_from_slice(&8u32.to_le_bytes()); // 0x1000
        h[0x1B4..0x1B8].copy_from_slice(&2u32.to_le_bytes());
        for (i, b) in d[ncch + 0x200..].iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let exefs = ncch + 0xA00;
        let entries: [(&[u8], u32, u32); 2] = [(b"icon", 0, 0x200), (b".code", 0x200, 0x1f0)];
        d[exefs..exefs + 0x200].fill(0);
        for (i, (name, off, size)) in entries.iter().enumerate() {
            let e = exefs + i * 16;
            d[e..e + name.len()].copy_from_slice(name);
            d[e + 8..e + 12].copy_from_slice(&off.to_le_bytes());
            d[e + 12..e + 16].copy_from_slice(&size.to_le_bytes());
        }
        let hash = Sha256::digest(&d[ncch + 0x200..ncch + 0x600]);
        d[ncch + 0x160..ncch + 0x180].copy_from_slice(&hash);
        d
    }

    /// Chiffre comme le ferait une vraie cartouche (le CTR est symétrique).
    fn encrypt(plain: &[u8], path: &Path) {
        std::fs::write(path, plain).unwrap();
        let mut f = File::options().read(true).write(true).open(path).unwrap();
        let h = ncch_header(&mut f, 0x4000).unwrap();
        let k = keys();
        let y = u128::from_be_bytes(h[..16].try_into().unwrap());
        let (p, s) = (k.normal(0x2C, y).unwrap(), k.normal(0x25, y).unwrap());
        let mut nop = |_| {};
        apply(&mut f, p, counter(&h, 1, 0x200), 0x4200, 0x4200, 0x800, &mut nop).unwrap();
        let start = 0x4000 + 0xA00;
        let ctr = counter(&h, 2, 0xA00);
        apply(&mut f, p, ctr, start, start + 0x200, 0x200, &mut nop).unwrap(); // icon
        apply(&mut f, s, ctr, start, start + 0x400, 0x200, &mut nop).unwrap(); // .code
        apply(&mut f, p, ctr, start, start, 0x200, &mut nop).unwrap(); // en-tête
        apply(&mut f, s, counter(&h, 3, 0x1000), 0x5000, 0x5000, 0x400, &mut nop).unwrap();
        f.seek(SeekFrom::Start(0x4000 + 0x18F)).unwrap();
        f.write_all(&[0x00]).unwrap();
    }

    #[test]
    fn decrypts_a_cartridge_in_place() {
        let dir = std::env::temp_dir().join(format!("n3ds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jeu.3ds");
        let plain = plain_rom();
        encrypt(&plain, &path);
        assert!(is_encrypted(&path));
        assert_ne!(std::fs::read(&path).unwrap()[0x4200..], plain[0x4200..]);
        let mut last = (0, 0);
        assert!(decrypt(&path, &keys(), &mut |d, t| last = (d, t)).unwrap());
        let out = std::fs::read(&path).unwrap();
        assert_eq!(out[0x4200..], plain[0x4200..], "contenu déchiffré");
        assert_eq!((out[0x4000 + 0x18B], out[0x4000 + 0x18F]), (0, 0x04));
        assert!(!is_encrypted(&path) && last.0 > 0);
        assert!(!decrypt(&path, &keys(), &mut |_, _| {}).unwrap(), "déjà en clair : rien à faire");
        assert_eq!(save_id(&path).as_deref(), Some("0006a900"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_dump_without_its_flag_is_only_marked() {
        let dir = std::env::temp_dir().join(format!("n3ds-plain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jeu.3ds");
        let mut plain = plain_rom();
        plain[0x5000..0x5004].copy_from_slice(b"IVFC");
        plain[0x4000 + 0x18F] = 0; // « Decrypted » mal fait : en clair, sans « NoCrypto »
        std::fs::write(&path, &plain).unwrap();
        assert!(is_encrypted(&path));
        assert!(decrypt(&path, &Keys::default(), &mut |_, _| {}).unwrap(), "pas besoin des clés");
        let out = std::fs::read(&path).unwrap();
        assert_eq!(out[0x4200..], plain[0x4200..], "contenu intact");
        assert_eq!((out[0x4000 + 0x18B], out[0x4000 + 0x18F]), (0, 0x04));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_exheader_is_refused_untouched() {
        let dir = std::env::temp_dir().join(format!("n3ds-damaged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jeu.3ds");
        let plain = plain_rom();
        encrypt(&plain, &path);
        // En-tête étendu abîmé, comme dans certains dumps « Decrypted ».
        let mut bad = std::fs::read(&path).unwrap();
        bad[0x4210] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        let err = decrypt(&path, &keys(), &mut |_, _| {}).unwrap_err();
        assert!(err.contains(DAMAGED), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), bad, "rien n'est écrit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Vraie ROM et vraies clés : `N3DS_ROM=… N3DS_KEYS=… cargo test --lib real_rom -- --ignored`.
    /// La ROM est déchiffrée sur place.
    #[test]
    #[ignore]
    fn real_rom() {
        let rom = std::path::PathBuf::from(std::env::var("N3DS_ROM").unwrap());
        let keys = Keys::parse(&std::fs::read_to_string(std::env::var("N3DS_KEYS").unwrap()).unwrap());
        assert!(is_encrypted(&rom));
        let t = std::time::Instant::now();
        assert!(decrypt(&rom, &keys, &mut |_, _| {}).unwrap());
        println!("déchiffrée en {:?}, ID {:?}", t.elapsed(), save_id(&rom));
        assert!(!is_encrypted(&rom));
    }

    #[test]
    fn missing_keys_are_named() {
        let k = Keys::parse("generator=00\n");
        assert!(k.normal(0x2C, 0).unwrap_err().contains("slot0x2CKeyX"));
        assert!(Keys::default().normal(0x2C, 0).unwrap_err().contains("generator"));
        assert_eq!(keys().key_x.len(), 2);
    }
}
