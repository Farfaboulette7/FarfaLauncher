//! Shaders partagés des émulateurs Switch (Citron, Eden), via FarfaRomManager.
//!
//! Ces émulateurs gardent les shaders d'un jeu dans `user/shader/<ID du jeu>/vulkan.bin`, sous
//! une forme indépendante de la carte graphique : ils les recompilent pour elle au démarrage, et
//! le jeu ne saccade plus aux endroits déjà vus. Chaque joueur envoie son fichier après la partie
//! (le serveur réunit les shaders de tout le monde) et reçoit avant la partie la réunion de ceux
//! des autres. `vulkan_pipelines.bin`, propre à la carte graphique, n'est jamais touché.
//!
//! Rien ici n'empêche de jouer : une erreur est notée dans le journal et la partie continue.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::romm::client::RommClient;

const CACHE: &str = "vulkan.bin";
const MEMO: &str = "shaders.json";
const MAGIC: &[u8] = b"yuzucach";
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_UPLOAD: usize = 256 << 20;

/// Fichiers voisins du cache, jamais partagés : pipelines propres à la carte graphique, liste
/// des shaders utilisés, sauvegardes de l'émulateur.
fn is_cache_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("vulkan") && n.ends_with(".bin") && !["_used", "pipelines", "static"].iter().any(|k| n.contains(k))
}

/// Dossier des shaders de l'émulateur et nom de son cache, relevés sur le disque :
/// `user/shader/<jeu>/vulkan.bin` (Citron, Eden), `user/cache/shader/<jeu>/…` (Switch 2).
/// `None` : l'émulateur partage déjà ses shaders lui-même (Lichii et lichii-sync), ou son
/// cache n'a encore jamais été écrit (son nom est alors inconnu).
pub fn locate(exe_dir: &Path) -> Option<(PathBuf, String)> {
    let classic = exe_dir.join("user").join("shader");
    let switch2 = exe_dir.join("user").join("cache").join("shader");
    let is_switch2 = switch2.is_dir();
    let root = if is_switch2 { switch2 } else { classic };
    let files: Vec<String> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .flat_map(|e| std::fs::read_dir(e.path()).into_iter().flatten().filter_map(Result::ok))
        .map(|f| f.file_name().to_string_lossy().into_owned())
        .collect();
    if files.iter().any(|f| f.starts_with("lichii-cache")) {
        return None;
    }
    let name = files.into_iter().filter(|f| is_cache_name(f)).max_by_key(|f| f == CACHE).or_else(|| (!is_switch2).then(|| CACHE.to_string()))?;
    Some((root, name))
}

/// Dernier échange avec le serveur pour un jeu et un émulateur.
#[derive(Default, Clone, Serialize, Deserialize)]
struct Memo {
    title: Option<String>,
    /// SHA-1 du fichier local au dernier échange.
    local: Option<String>,
    /// SHA-1 de l'archive du serveur reçue la dernière fois.
    server: Option<String>,
}

#[derive(Deserialize)]
struct ServerInfo {
    version: u32,
    sha1: String,
    count: u64,
}

pub struct Job {
    pub manager: RommClient,
    pub emulator: String,
    pub rom_id: i64,
    /// `user/shader` de l'émulateur (`user/cache/shader` chez les émulateurs Switch 2).
    pub shader_root: PathBuf,
    /// Nom du cache dans le dossier d'un jeu (`vulkan.bin`, `vulkan-frizti.fork-v54.bin`…).
    pub cache_name: String,
    pub config_dir: PathBuf,
    /// Fichiers du jeu (pour y lire son ID).
    pub roms: Vec<PathBuf>,
}

fn sha1_hex(data: &[u8]) -> String {
    Sha1::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// ID de titre de base d'un ID de jeu, de mise à jour (`…800`) ; `None` pour un DLC.
fn base_title(id: &str) -> Option<String> {
    let id = id.to_ascii_lowercase();
    // Switch : 01… ; Switch 2 : 04…
    if id.len() != 16 || !(id.starts_with("01") || id.starts_with("04")) || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match &id[13..] {
        "000" | "800" => Some(format!("{}000", &id[..13])),
        _ => None,
    }
}

/// ID du jeu écrit dans le nom d'un fichier (`… [0100AE00096EA000][v0].nsp`).
pub fn title_from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    names.into_iter().find_map(|n| n.split(['[', ']', '(', ')', ' ', '_', '.']).find_map(base_title))
}

/// ID lu dans le nom du ticket (`<ID><génération>.tik`) d'un NSP.
pub fn title_from_nsp(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 16];
    f.read_exact(&mut head).ok()?;
    if &head[..4] != b"PFS0" {
        return None;
    }
    let count = u32::from_le_bytes(head[4..8].try_into().ok()?) as usize;
    let names_len = u32::from_le_bytes(head[8..12].try_into().ok()?) as usize;
    if count > 4096 || names_len > 1 << 20 {
        return None;
    }
    let mut rest = vec![0u8; count * 24 + names_len];
    f.read_exact(&mut rest).ok()?;
    let names = String::from_utf8_lossy(&rest[count * 24..]).into_owned();
    names.split('\0').filter(|n| n.ends_with(".tik")).find_map(|n| n.get(..16).and_then(base_title))
}

/// Dossier de shaders écrit pendant la partie : le jeu qui vient de tourner.
fn learned_title(root: &Path, cache: &str, since: SystemTime) -> Option<String> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            let modified = e.path().join(cache).metadata().and_then(|m| m.modified()).ok()?;
            (base_title(&name) == Some(name.clone()) && modified >= since).then_some((modified, name))
        })
        .max()
        .map(|(_, name)| name)
}

/// ID du jeu déjà appris par le partage des shaders (`shaders.json`).
pub fn known_title(config_dir: &Path, emulator: &str, rom_id: i64) -> Option<String> {
    let all: BTreeMap<String, Memo> = std::fs::read_to_string(config_dir.join(MEMO)).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    all.get(&format!("{emulator}:{rom_id}"))?.title.clone()
}

/// Retient l'ID d'un jeu appris ailleurs (serveur) pour cet émulateur.
pub fn remember_title(config_dir: &Path, emulator: &str, rom_id: i64, title: &str) {
    let path = config_dir.join(MEMO);
    let mut all: BTreeMap<String, Memo> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.entry(format!("{emulator}:{rom_id}")).or_default().title = Some(title.to_ascii_lowercase());
    if let Ok(text) = serde_json::to_string(&all) {
        let _ = std::fs::write(&path, text);
    }
}

fn cache_version(data: &[u8]) -> Option<u32> {
    (data.len() >= 12 && &data[..8] == MAGIC).then(|| u32::from_le_bytes(data[8..12].try_into().unwrap()))
}

fn zip_one(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut out);
        z.start_file(CACHE, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))?;
        z.write_all(data)?;
        z.finish()?;
    }
    Ok(out.into_inner())
}

fn unzip_one(zip: &[u8]) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).ok()?;
    let mut file = archive.by_name(CACHE).ok()?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).ok()?;
    Some(data)
}

impl Job {
    fn key(&self) -> String {
        format!("{}:{}", self.emulator, self.rom_id)
    }

    fn memo(&self) -> Memo {
        let all: BTreeMap<String, Memo> = std::fs::read_to_string(self.config_dir.join(MEMO)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        all.get(&self.key()).cloned().unwrap_or_default()
    }

    fn save_memo(&self, memo: &Memo) {
        let path = self.config_dir.join(MEMO);
        let mut all: BTreeMap<String, Memo> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        all.insert(self.key(), memo.clone());
        if let Ok(text) = serde_json::to_string(&all) {
            if let Err(e) = std::fs::write(&path, text) {
                tracing::warn!("shaders : échange non mémorisé ({e})");
            }
        }
    }

    fn title(&self, memo: &Memo) -> Option<String> {
        memo.title
            .clone()
            .or_else(|| title_from_names(self.roms.iter().filter_map(|p| p.file_name()?.to_str())))
            .or_else(|| self.roms.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("nsp"))).find_map(|p| title_from_nsp(p)))
    }

    fn cache_path(&self, title: &str) -> PathBuf {
        self.shader_root.join(title).join(&self.cache_name)
    }

    /// Envoie le fichier local s'il a changé depuis le dernier échange. `false` : envoi raté.
    async fn upload(&self, title: &str, memo: &mut Memo) -> bool {
        let Ok(data) = std::fs::read(self.cache_path(title)) else { return true };
        if cache_version(&data).is_none() {
            return true;
        }
        // Cache démesuré (ancien format de certains émulateurs, plusieurs Go) : pas envoyé.
        if data.len() > MAX_UPLOAD {
            tracing::info!("shaders {} {title} : cache de {} Mo, trop gros pour être partagé", self.emulator, data.len() >> 20);
            return true;
        }
        let sha = sha1_hex(&data);
        if memo.local.as_deref() == Some(sha.as_str()) {
            return true;
        }
        let Ok(body) = zip_one(&data) else { return false };
        let path = format!("api/shaders/{}/{title}?rom={}", self.emulator, self.rom_id);
        match self.manager.send(self.manager.put_bytes(&path, body).timeout(TIMEOUT)).await {
            Ok(resp) => {
                let added = resp.json::<serde_json::Value>().await.ok().and_then(|v| v.get("added")?.as_u64()).unwrap_or(0);
                tracing::info!("shaders {} {title} : {added} nouveau(x) envoyé(s) au serveur", self.emulator);
                memo.local = Some(sha);
                true
            }
            Err(e) => {
                tracing::warn!("shaders {} {title} non envoyés ({})", self.emulator, e.code());
                false
            }
        }
    }

    /// Reçoit la réunion du serveur si elle a changé depuis la dernière fois.
    async fn download(&self, title: &str, memo: &mut Memo) {
        let path = format!("api/shaders/{}/{title}", self.emulator);
        let info: ServerInfo = match self.manager.send(self.manager.get(&path)).await {
            Ok(resp) => match resp.json().await {
                Ok(i) => i,
                Err(_) => return,
            },
            Err(_) => return, // rien sur le serveur pour ce jeu (404), ou serveur absent
        };
        if memo.server.as_deref() == Some(info.sha1.as_str()) {
            return;
        }
        let local_path = self.cache_path(title);
        let local = std::fs::read(&local_path).ok();
        // Autre format que celui de l'émulateur installé : il jetterait tout le cache local.
        if let Some(v) = local.as_deref().and_then(cache_version) {
            if v != info.version {
                tracing::info!("shaders {} {title} : format {v} ici, {} sur le serveur, rien de changé", self.emulator, info.version);
                return;
            }
        }
        let zip = match self.manager.send(self.manager.get_stream(&format!("{path}/content")).timeout(TIMEOUT)).await {
            Ok(resp) => match resp.bytes().await {
                Ok(b) => b,
                Err(_) => return,
            },
            Err(e) => {
                tracing::warn!("shaders {} {title} non reçus ({})", self.emulator, e.code());
                return;
            }
        };
        let Some(data) = unzip_one(&zip).filter(|d| cache_version(d) == Some(info.version)) else {
            tracing::warn!("shaders {} {title} : archive du serveur illisible", self.emulator);
            return;
        };
        if let Some(dir) = local_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = local_path.with_extension("bin.farfa");
        if std::fs::write(&tmp, &data).and_then(|_| std::fs::rename(&tmp, &local_path)).is_err() {
            let _ = std::fs::remove_file(&tmp);
            tracing::warn!("shaders {} {title} : fichier local non remplacé", self.emulator);
            return;
        }
        tracing::info!("shaders {} {title} : {} reçus du serveur", self.emulator, info.count);
        memo.local = Some(sha1_hex(&data));
        memo.server = Some(info.sha1);
    }

    /// Avant la partie : d'abord ce qui n'a pas été envoyé, puis la réunion du serveur.
    pub async fn before(&self) {
        let mut memo = self.memo();
        let Some(title) = self.title(&memo) else { return };
        memo.title = Some(title.clone());
        // Le fichier local n'est remplacé que si ses shaders sont déjà sur le serveur.
        if self.upload(&title, &mut memo).await {
            self.download(&title, &mut memo).await;
        }
        self.save_memo(&memo);
    }

    /// Après la partie : les shaders vus pendant la partie partent sur le serveur.
    pub async fn after(&self, started: SystemTime) {
        let mut memo = self.memo();
        let Some(title) = self.title(&memo).or_else(|| learned_title(&self.shader_root, &self.cache_name, started)) else { return };
        memo.title = Some(title.clone());
        self.upload(&title, &mut memo).await;
        self.save_memo(&memo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_ids_of_switch_files() {
        assert_eq!(title_from_names(["Hyrule Warriors Definitive Edition [0100AE00096EA000][USA][v0].nsp"]).as_deref(), Some("0100ae00096ea000"));
        // Mise à jour : l'ID de son jeu ; DLC seul : rien.
        assert_eq!(title_from_names(["Castlevania [0100BA4026E94800][v65536].nsp"]).as_deref(), Some("0100ba4026e94000"));
        assert_eq!(title_from_names(["Castlevania [Alucard DLC][0100BA4026E95001].nsp"]), None);
        assert_eq!(title_from_names(["Pokemon Lets Go Pikachu.xci"]), None);

        let dir = std::env::temp_dir().join(format!("farfa-tik-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let names = b"a.nca\x000100ae00096ea8000000000000000004.tik\x00";
        let mut nsp = b"PFS0".to_vec();
        nsp.extend_from_slice(&2u32.to_le_bytes());
        nsp.extend_from_slice(&(names.len() as u32).to_le_bytes());
        nsp.extend_from_slice(&[0; 4]);
        nsp.extend_from_slice(&[0; 48]);
        nsp.extend_from_slice(names);
        std::fs::write(dir.join("maj.nsp"), &nsp).unwrap();
        assert_eq!(title_from_nsp(&dir.join("maj.nsp")).as_deref(), Some("0100ae00096ea000"));

        // Dossier de shaders écrit pendant la partie.
        let before = SystemTime::now() - Duration::from_secs(5);
        std::fs::create_dir_all(dir.join("shader/0100ba4026e94000")).unwrap();
        std::fs::write(dir.join("shader/0100ba4026e94000/vulkan.bin"), b"yuzucach").unwrap();
        std::fs::create_dir_all(dir.join("shader/nvidia")).unwrap();
        assert_eq!(learned_title(&dir.join("shader"), CACHE, before).as_deref(), Some("0100ba4026e94000"));
        assert_eq!(learned_title(&dir.join("shader"), CACHE, SystemTime::now() + Duration::from_secs(60)), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_of_each_emulator() {
        let base = std::env::temp_dir().join(format!("farfa-shaderloc-{}", uuid::Uuid::new_v4()));
        let put = |emu: &str, rel: &str| {
            let p = base.join(emu).join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"yuzucach").unwrap();
        };
        // Citron, Eden : user/shader, vulkan.bin même avant la première partie.
        std::fs::create_dir_all(base.join("citron")).unwrap();
        assert_eq!(locate(&base.join("citron")), Some((base.join("citron/user/shader"), "vulkan.bin".into())));
        // Églantine.
        put("eglantine", "user/cache/shader/040074a01bf12000/vulkan.bin");
        put("eglantine", "user/cache/shader/vulkan_static_pipelines.bin");
        assert_eq!(locate(&base.join("eglantine")), Some((base.join("eglantine/user/cache/shader"), "vulkan.bin".into())));
        // Fritchii : son propre nom, sans la liste des shaders utilisés.
        put("fritchii", "user/cache/shader/0400486024dec000/vulkan_used-frizti.fork-v54.bin");
        put("fritchii", "user/cache/shader/0400486024dec000/vulkan-frizti.fork-v54.bin");
        assert_eq!(locate(&base.join("fritchii")).map(|l| l.1), Some("vulkan-frizti.fork-v54.bin".into()));
        // Lichii partage déjà les siens (lichii-sync).
        put("lichii", "user/cache/shader/040074a01bf12000/lichii-cache.v2.bin");
        put("lichii", "user/cache/shader/0400486024dec000/vulkan.fork-v54.bin");
        assert_eq!(locate(&base.join("lichii")), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn zip_round_trip() {
        let data = b"yuzucach\x12\0\0\0record".to_vec();
        assert_eq!(unzip_one(&zip_one(&data).unwrap()), Some(data.clone()));
        assert_eq!(cache_version(&data), Some(18));
    }
}

/// Vrai gestionnaire : `FARFA_E2E_URL`, `_TOKEN`, `_MANAGER`, `_ROM` (jeu Switch) et
/// `FARFA_E2E_SHADERS` (dossier `user/shader` d'un Citron où ce jeu a déjà tourné).
/// Un joueur envoie son cache, un autre qui n'a rien le reçoit à l'identique.
#[cfg(test)]
mod e2e {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn shaders_shared_between_two_players() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let source = PathBuf::from(var("FARFA_E2E_SHADERS"));
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap().with_base(&var("FARFA_E2E_MANAGER"));
        let title = std::fs::read_dir(&source).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().to_string()).find(|n| base_title(n).as_deref() == Some(n.as_str())).unwrap();
        let original = std::fs::read(source.join(&title).join(CACHE)).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-shaders-e2e-{}", uuid::Uuid::new_v4()));
        let job = |who: &str| {
            let dir = base.join(who);
            std::fs::create_dir_all(dir.join("config")).unwrap();
            Job { manager: client.clone(), emulator: "citron".into(), rom_id: rom, shader_root: dir.join("shader"), cache_name: CACHE.into(), config_dir: dir.join("config"), roms: vec![PathBuf::from(format!("Jeu [{title}][v0].nsp"))] }
        };
        let a = job("a");
        std::fs::create_dir_all(a.shader_root.join(&title)).unwrap();
        std::fs::write(a.cache_path(&title), &original).unwrap();
        a.after(SystemTime::UNIX_EPOCH).await;
        let b = job("b");
        b.before().await;
        let received = std::fs::read(b.cache_path(&title)).expect("rien reçu");
        println!("{title} : {} octets envoyés, {} reçus", original.len(), received.len());
        assert_eq!(received, original);
        // Deuxième lancement : rien à refaire (déjà à jour).
        b.before().await;
        std::fs::remove_dir_all(&base).unwrap();
    }
}
