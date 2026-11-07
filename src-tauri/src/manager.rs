//! FarfaRomManager : fournit les émulateurs et leurs éléments de configuration,
//! tenus à jour sur le serveur. Le launcher ne contacte jamais GitHub pour eux.
//!
//! Le catalogue est gardé sur le disque : hors ligne, les jeux déjà installés
//! restent jouables et les pages gardent leurs informations.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::assets;
use crate::error::{log_err, AppError};
use crate::platform::{ArchiveKind, Target, ALL_TARGETS};
use crate::romm::library::{PlatformArchive, RemoteFile};
use crate::settings::Settings;
use crate::state::AppState;

const SUBDOMAIN: &str = "manager";
const CACHE_FILE: &str = "catalog.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Catalog {
    pub recomps: Vec<CatalogRecomp>,
    pub emulators: Vec<Emulator>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogRecomp {
    pub id: String,
    pub name: String,
    pub repo: String,
    /// Launcher à plusieurs jeux : ID de son entrée historique dans RomM, clé de l'installation.
    pub entry_rom_id: Option<i64>,
    pub version: EmulatorVersion,
    pub files: Vec<ManagerFile>,
    pub mods: Vec<CatalogMod>,
    pub launch: RecompLaunch,
    pub games: Vec<CatalogRecompGame>,
    /// Comment la mettre en français.
    pub language: Vec<crate::language::LanguageRule>,
    /// Format d'attribution des manettes aux joueurs (controllers.rs).
    #[serde(default)]
    pub controllers: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogMod {
    pub name: String,
    pub files: Vec<ManagerFile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecompLaunch {
    pub exe: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub data_dir: Option<String>,
    pub save_dir: Option<String>,
    pub cache_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogRecompGame {
    pub rom_id: Option<i64>,
    /// Identifiant du jeu pour un launcher à plusieurs jeux.
    pub game: Option<String>,
}

impl CatalogRecomp {
    /// Un launcher qui lance plusieurs jeux : il s'installe une fois, à part de ses pages.
    pub fn is_shared(&self) -> bool {
        self.games.iter().any(|g| g.game.is_some()) && self.launch.data_dir.is_some()
    }

    /// Clé de l'installation : l'entrée historique si elle existe, sinon un ID propre au launcher.
    pub fn runtime_rom_id(&self) -> i64 {
        self.entry_rom_id.unwrap_or_else(|| runtime_id(&format!("recomp:{}", self.id)))
    }

    pub fn archives(&self, me: Target) -> (Vec<PlatformArchive>, Option<PlatformArchive>) {
        let mut archives: Vec<PlatformArchive> = ALL_TARGETS
            .iter()
            .filter_map(|t| {
                let f = self.files.iter().find(|f| f.system.as_deref() == Some(t.suffix()))?;
                Some(PlatformArchive { target: *t, kind: ArchiveKind::Zip, score: u8::MAX, file: f.remote() })
            })
            .collect();
        archives.sort_by_key(|a| ALL_TARGETS.iter().position(|t| *t == a.target));
        let mine = me.candidates().iter().find_map(|t| archives.iter().find(|a| a.target == *t)).cloned();
        (archives, mine)
    }

    pub fn mod_files(&self) -> Vec<RemoteFile> {
        self.mods
            .iter()
            .flat_map(|m| {
                m.files.iter().map(move |f| RemoteFile { rel_path: format!("MODS/{}/{}", m.name, f.name), ..f.remote() })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Emulator {
    pub id: String,
    pub name: String,
    pub consoles: Vec<String>,
    #[serde(default)]
    pub repo: Option<String>,
    pub version: EmulatorVersion,
    #[serde(default)]
    pub files: Vec<ManagerFile>,
    #[serde(default)]
    pub launch: EmulatorLaunch,
    #[serde(default)]
    pub saves: EmulatorSaves,
    #[serde(default)]
    pub config: Vec<ManagerFile>,
    #[serde(default)]
    pub config_missing: Vec<String>,
    /// Comment le mettre en français.
    #[serde(default)]
    pub language: Vec<crate::language::LanguageRule>,
    /// Format d'attribution des manettes aux joueurs (controllers.rs).
    #[serde(default)]
    pub controllers: Option<String>,
    /// Questions posées au lancement et touche qui y répond (dialogs.rs).
    #[serde(default)]
    pub dialogs: Vec<crate::dialogs::DialogAnswer>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmulatorVersion {
    /// Identifiant exact de la version, comparé pour détecter une mise à jour.
    pub cle: String,
    /// Version affichée.
    pub nom: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmulatorLaunch {
    pub exe: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Fichiers vides à créer à côté de l'exécutable (mode portable).
    pub portable: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EmulatorSaves {
    /// `{romdir}` ou relatif au dossier de l'exécutable.
    pub dir: Option<String>,
    /// Autre dossier pour un système (`windows`, `linux`) : `{appdata}/…` et ses équivalents.
    pub by_system: std::collections::HashMap<String, String>,
    pub patterns: Vec<String>,
    /// Dossier et fichiers propres à une console (et éventuellement un système), avant les autres.
    pub variants: Vec<SaveVariant>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SaveVariant {
    pub system: Option<String>,
    pub console: Option<String>,
    pub dir: Option<String>,
    pub patterns: Vec<String>,
}

impl EmulatorSaves {
    /// Dossier pour ce système.
    pub fn dir_here(&self) -> Option<&str> {
        self.by_system.get(crate::language::current_system()).or(self.dir.as_ref()).map(String::as_str)
    }

    /// Dossier et motifs pour un jeu de cette console, sur ce système.
    pub fn for_console(&self, console: &str) -> (Option<&str>, &[String]) {
        let here = crate::language::current_system();
        let variant = self.variants.iter().find(|v| {
            v.system.as_deref().is_none_or(|s| s == here) && v.console.as_deref().is_none_or(|c| c == console)
        });
        match variant {
            Some(v) => (v.dir.as_deref().or(self.dir_here()), if v.patterns.is_empty() { &self.patterns } else { &v.patterns }),
            None => (self.dir_here(), &self.patterns),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerFile {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    pub sha1: String,
    #[serde(default)]
    pub system: Option<String>,
    /// Élément de configuration : où le poser, relatif à l'exécutable.
    #[serde(default)]
    pub dest: Option<String>,
}

impl ManagerFile {
    pub fn remote(&self) -> RemoteFile {
        // Identifiant stable tiré du SHA-1 : l'onglet Mods s'en sert pour désigner un fichier.
        let id = self.sha1.get(..12).and_then(|h| i64::from_str_radix(h, 16).ok()).unwrap_or(0);
        RemoteFile { id, file_name: self.name.clone(), rel_path: self.name.clone(), size: self.size, sha1: Some(self.sha1.clone()), md5: None, from_manager: true }
    }

    pub fn download_path(&self) -> String {
        format!("files/{}", self.sha1)
    }
}

/// Ce que la page d'un jeu doit savoir de l'émulateur de sa console.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorOffer {
    pub id: String,
    pub name: String,
    pub version: String,
    pub version_key: String,
    /// Identifiant de l'émulateur dans les installations.
    pub runtime_rom_id: i64,
    /// Build pour ce système ; absente : « Pas d'émulateur disponible ».
    pub available: bool,
    /// Taille à télécharger pour ce système.
    pub size: u64,
    pub systems: Vec<String>,
    pub repo: Option<String>,
    /// Éléments de configuration que le serveur n'a pas encore (ex. prod.keys).
    pub config_missing: Vec<String>,
}

/// Identifiant d'un émulateur dans les installations : les ID RomM sont positifs.
pub fn runtime_id(emulator_id: &str) -> i64 {
    // FNV-1a : stable d'une version à l'autre, contrairement au hachage de la bibliothèque standard.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in emulator_id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    -((h >> 2) as i64) - 1
}

impl Emulator {
    /// Build pour ce système, par ordre de préférence.
    pub fn file_for(&self, target: Target) -> Option<&ManagerFile> {
        target.candidates().iter().find_map(|t| self.files.iter().find(|f| f.system.as_deref() == Some(t.suffix())))
    }
}

impl Catalog {
    /// Émulateur d'une console : celui qui a une build pour ce système d'abord.
    pub fn emulator_for_console(&self, console: &str, target: Target) -> Option<&Emulator> {
        let mut pour_la_console = self.emulators.iter().filter(|e| e.consoles.iter().any(|c| c == console));
        let premier = pour_la_console.clone().next();
        pour_la_console.find(|e| e.file_for(target).is_some()).or(premier)
    }

    pub fn offer(&self, console: &str, target: Target) -> Option<EmulatorOffer> {
        let e = self.emulator_for_console(console, target)?;
        let file = e.file_for(target);
        Some(EmulatorOffer {
            id: e.id.clone(),
            name: e.name.clone(),
            version: e.version.nom.clone(),
            version_key: e.version.cle.clone(),
            runtime_rom_id: runtime_id(&e.id),
            available: file.is_some(),
            size: file.map(|f| f.size).unwrap_or(0),
            systems: e.files.iter().filter_map(|f| f.system.clone()).collect(),
            repo: e.repo.clone(),
            config_missing: e.config_missing.clone(),
        })
    }

    pub fn recomp(&self, id: &str) -> Option<&CatalogRecomp> {
        self.recomps.iter().find(|r| r.id == id)
    }

    /// Réglages de langue du programme qui lance ce jeu : sa recomp ou son émulateur.
    pub fn language_for_install(&self, st: &crate::install::state::InstalledGame) -> Vec<crate::language::LanguageRule> {
        match &st.shared {
            Some(s) if s.is_emulator() => {
                self.emulators.iter().find(|e| runtime_id(&e.id) == s.runtime_rom_id).map(|e| e.language.clone()).unwrap_or_default()
            }
            _ => self.recomp_for_install(st).map(|r| r.language.clone()).unwrap_or_default(),
        }
    }

    /// Émulateur qui lance ce jeu installé.
    pub fn emulator_for_install(&self, st: &crate::install::state::InstalledGame) -> Option<&Emulator> {
        let s = st.shared.as_ref().filter(|s| s.is_emulator())?;
        self.emulators.iter().find(|e| runtime_id(&e.id) == s.runtime_rom_id)
    }

    /// Format d'attribution des manettes du programme qui lance ce jeu.
    pub fn controllers_for_install(&self, st: &crate::install::state::InstalledGame) -> Option<String> {
        match &st.shared {
            Some(s) if s.is_emulator() => self.emulators.iter().find(|e| runtime_id(&e.id) == s.runtime_rom_id).and_then(|e| e.controllers.clone()),
            _ => self.recomp_for_install(st).and_then(|r| r.controllers.clone()),
        }
    }

    /// Réponses automatiques de l'émulateur de ce jeu installé.
    pub fn dialogs_for_install(&self, st: &crate::install::state::InstalledGame) -> Vec<crate::dialogs::DialogAnswer> {
        self.emulator_for_install(st).map(|e| e.dialogs.clone()).unwrap_or_default()
    }

    /// Recomp qui fait tourner ce jeu installé (pas lancé par un émulateur).
    pub fn recomp_for_install(&self, st: &crate::install::state::InstalledGame) -> Option<&CatalogRecomp> {
        match &st.shared {
            Some(s) if s.is_emulator() => None,
            Some(s) => self.recomps.iter().find(|r| r.is_shared() && r.runtime_rom_id() == s.runtime_rom_id),
            None => self.recomp_for_rom(st.rom_id).or_else(|| self.recomps.iter().find(|r| r.is_shared() && r.runtime_rom_id() == st.rom_id)),
        }
    }

    /// Recomp à un seul jeu avec `argument:` : lancée directement sur sa ROM
    /// (`{rom}`), sans son écran de choix. Relu sur le catalogue à chaque lancement.
    pub fn recomp_launch(&self, st: &crate::install::state::InstalledGame) -> Option<crate::install::state::DirectLaunch> {
        if st.shared.is_some() {
            return None;
        }
        let recomp = self.recomp_for_rom(st.rom_id).filter(|r| !r.is_shared())?;
        if recomp.launch.args.is_empty() && recomp.launch.env.is_empty() {
            return None;
        }
        let exe = st.executable.clone()?;
        let sep = std::path::MAIN_SEPARATOR.to_string();
        let rom = st
            .roms
            .first()
            .map(|r| format!("{}{sep}{}", crate::launch::direct::GAME_DIR_VAR, r.replace('/', &sep)))
            .unwrap_or_default();
        let expand = |t: &str| t.replace("{rom}", &rom);
        Some(crate::install::state::DirectLaunch {
            exe,
            args: recomp.launch.args.iter().map(|a| expand(a)).collect(),
            cwd: None,
            env: recomp.launch.env.iter().map(|(k, v)| (k.clone(), expand(v))).collect(),
        })
    }

    /// Exécutable imposé par `exe:` pour une recomp à un seul jeu.
    pub fn recomp_exe(&self, rom_id: i64) -> Option<&str> {
        self.recomp_for_rom(rom_id).filter(|r| !r.is_shared())?.launch.exe.as_deref()
    }

    pub fn recomp_for_rom(&self, rom_id: i64) -> Option<&CatalogRecomp> {
        self.recomps.iter().find(|r| r.games.iter().any(|g| g.rom_id == Some(rom_id)))
    }

    /// Jeux qui ont une recomp au catalogue.
    pub fn recomp_rom_ids(&self) -> HashSet<i64> {
        self.recomps.iter().flat_map(|r| r.games.iter().filter_map(|g| g.rom_id)).collect()
    }

    pub fn knows_repo(&self, url: &str) -> bool {
        self.recomps.iter().any(|r| r.repo == url) || self.emulators.iter().any(|e| e.repo.as_deref() == Some(url))
    }

    pub fn load_cached(config_dir: &Path) -> Self {
        std::fs::read_to_string(config_dir.join(CACHE_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
}

/// Adresse effective : celle saisie dans les paramètres, sinon celle déduite de RomM.
pub fn base_url(settings: &Settings) -> Option<String> {
    settings.manager_url.clone().or_else(|| settings.server_url.as_deref().and_then(|u| assets::derive_subdomain(u, SUBDOMAIN)))
}

/// Relit le catalogue ; garde le précédent si le gestionnaire ne répond pas.
pub async fn refresh(state: &AppState) {
    let Some(base) = base_url(&state.settings()) else { return };
    let Ok(client) = state.client().map(|c| c.with_base(&base)) else { return };
    let text = match client.send(client.get("api/catalog")).await {
        Ok(resp) => resp.text().await.map_err(|e| log_err(AppError::Server, "lecture du catalogue", e)),
        Err(e) => Err(e),
    };
    let parsed = text.and_then(|t| {
        serde_json::from_str::<Catalog>(&t).map(|c| (c, t)).map_err(|e| log_err(AppError::Server, "catalogue invalide", e))
    });
    match parsed {
        Ok((catalog, text)) => {
            if let Err(e) = std::fs::write(state.config_dir.join(CACHE_FILE), text) {
                tracing::warn!("catalogue non mis en cache: {e}");
            }
            tracing::info!("catalogue : {} recomp(s), {} émulateur(s)", catalog.recomps.len(), catalog.emulators.len());
            *state.catalog.lock().unwrap() = catalog;
        }
        Err(e) => tracing::warn!("FarfaRomManager indisponible ({}), catalogue précédent gardé", e.code()),
    }
}

/// Installations déjà à la version du catalogue : `VERSION.txt` n'étant plus la référence, leur
/// texte de version est remplacé par la clé du catalogue pour ne pas les retélécharger.
pub fn align_versions(config_dir: &Path, catalog: &Catalog) -> usize {
    let mut aligned = 0;
    for mut st in crate::install::state::list_installed(config_dir) {
        if st.shared.is_some() {
            continue;
        }
        let recomp = catalog.recomp_for_rom(st.rom_id).or_else(|| catalog.recomps.iter().find(|r| r.is_shared() && r.runtime_rom_id() == st.rom_id));
        let Some(r) = recomp else { continue };
        if st.version_raw.as_deref() == Some(r.version.cle.as_str()) || st.version.as_deref() != Some(r.version.nom.as_str()) {
            continue;
        }
        st.version_raw = Some(r.version.cle.clone());
        match crate::install::state::write_state(config_dir, &st) {
            Ok(()) => aligned += 1,
            Err(e) => tracing::warn!("version de « {} » non alignée: {}", st.name, e.code()),
        }
    }
    aligned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_recomp_files_for_the_system() {
        let text = r#"{"recomps":[{"id":"p","name":"Gen1recomp","repo":"https://github.com/a/b","entryRomId":10550,
          "version":{"cle":"k","nom":"v1"},
          "files":[{"name":"p_Windows.zip","size":3,"sha1":"00000000000a00000000000000000000000000aa","system":"Windows"},{"name":"p_Linux.zip","size":4,"sha1":"bb","system":"Linux"}],
          "mods":[{"name":"M","files":[{"name":"m.zip","size":1,"sha1":"00000000000b00000000000000000000000000cc"}]}],
          "launch":{"exe":"gen1recomp","args":["--game={game}"],"env":[],"dataDir":"d","saveDir":"saves/{game}","cacheDir":null},
          "games":[{"romId":7,"game":"red"},{"romId":null,"game":"blue"}]},
          {"id":"seul","name":"S","repo":"","version":{"cle":"k2","nom":"1"},"files":[],"mods":[],"launch":{"args":[],"env":[]},"games":[{"romId":9}]}]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        let r = c.recomp("p").unwrap();
        assert!(r.is_shared() && r.runtime_rom_id() == 10550);
        assert!(!c.recomp("seul").unwrap().is_shared());
        assert!(c.recomp("seul").unwrap().runtime_rom_id() < 0);
        assert_eq!(c.recomp_for_rom(9).unwrap().id, "seul");
        let (all, mine) = r.archives(Target::SteamOS);
        assert_eq!(all.len(), 2);
        let mine = mine.unwrap();
        assert_eq!(mine.file.file_name, "p_Linux.zip");
        assert!(mine.file.from_manager);
        let mods = r.mod_files();
        assert_eq!(mods[0].rel_path, "MODS/M/m.zip");
        assert_eq!(mods[0].id, 0xb);
        assert!(r.archives(Target::MacArm).1.is_none());
    }

    fn emu(id: &str, consoles: &[&str], systems: &[&str]) -> Emulator {
        Emulator {
            id: id.into(),
            name: id.into(),
            consoles: consoles.iter().map(|c| c.to_string()).collect(),
            repo: Some(format!("https://github.com/x/{id}")),
            version: EmulatorVersion { cle: "k".into(), nom: "1.0".into() },
            files: systems.iter().map(|s| ManagerFile { name: format!("{id}_{s}.zip"), size: 5, sha1: "a".repeat(40), system: Some(s.to_string()), dest: None }).collect(),
            launch: EmulatorLaunch::default(),
            saves: EmulatorSaves::default(),
            config: vec![],
            config_missing: vec![],
            language: vec![],
            controllers: None,
            dialogs: vec![],
        }
    }

    #[test]
    fn single_recomp_launches_its_rom_directly() {
        let text = r#"{"recomps":[{"id":"paper-cut","repo":"https://github.com/LLCOOLT/paper-cut","games":[{"name":"TTYD","romId":3406}],
          "launch":{"exe":"ttyd-eu","args":["{rom}"]}}],"emulators":[]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        assert_eq!(c.recomp_exe(3406), Some("ttyd-eu"));
        assert_eq!(c.recomp_exe(1), None);
        let st = crate::install::state::InstalledGame {
            rom_id: 3406,
            name: "TTYD".into(),
            version: None,
            version_raw: None,
            target: Target::Windows,
            archive_name: "a.zip".into(),
            executable: Some("ttyd-eu.exe".into()),
            pending_candidates: vec![],
            roms: vec!["Paper Mario.rvz".into()],
            rom_sources: vec![],
            mods_downloaded: vec![],
            saves: Default::default(),
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 1,
            updated_at: 1,
            game_dir: std::path::PathBuf::from("x"),
            pc: None,
            shared: None,
        };
        let d = c.recomp_launch(&st).unwrap();
        assert_eq!(d.exe, "ttyd-eu.exe");
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(d.args, [format!("{}{sep}Paper Mario.rvz", crate::launch::direct::GAME_DIR_VAR)]);
        // Sans argument : le programme s'ouvre comme avant.
        let plain: Catalog = serde_json::from_str(r#"{"recomps":[{"id":"m","repo":"r","games":[{"name":"M","romId":3406}]}],"emulators":[]}"#).unwrap();
        assert!(plain.recomp_launch(&st).is_none());
    }

    #[test]
    fn reads_the_real_catalog_format() {
        let text = r#"{"format":1,"generatedAt":1,"recomps":[{"id":"p","repo":"https://github.com/a/b","games":[{"name":"X","romId":7},{"name":"Y","romId":null}]}],
          "emulators":[{"id":"mgba","name":"mGBA","consoles":["gba"],"repo":"https://github.com/mgba-emu/mgba","version":{"cle":"0.10.5|d","nom":"0.10.5","type":"release"},
          "files":[{"name":"mgba_Linux.zip","path":"x","size":3,"sha1":"s","system":"Linux"}],
          "launch":{"exe":"mGBA","args":["{rom}"],"env":[["A","b"]],"portable":["portable.ini"]},"saves":{"dir":"{romdir}","patterns":["{romstem}.sav"]},
          "config":[{"dest":"keys/prod.keys","name":"prod.keys","path":"y","size":1,"sha1":"c"}],"configMissing":["z"]}]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        assert_eq!(c.recomp_rom_ids(), HashSet::from([7]));
        let e = &c.emulators[0];
        assert_eq!(e.launch.env, [("A".to_string(), "b".to_string())]);
        assert_eq!(e.config[0].dest.as_deref(), Some("keys/prod.keys"));
        assert!(c.knows_repo("https://github.com/mgba-emu/mgba") && c.knows_repo("https://github.com/a/b") && !c.knows_repo("https://evil"));
    }

    #[test]
    fn offers_the_emulator_of_a_console() {
        let c = Catalog { recomps: vec![], emulators: vec![emu("mgba", &["gba"], &["Linux", "Windows"]), emu("mac", &["ngc"], &["macOS_ARM"]), emu("a", &["psp"], &["macOS_ARM"]), emu("b", &["psp"], &["Windows"])] };
        let o = c.offer("gba", Target::Windows).unwrap();
        assert!(o.available);
        assert_eq!(o.size, 5);
        assert_eq!(o.systems, ["Linux", "Windows"]);
        // SteamOS se rabat sur la build Linux, comme pour les recomps.
        assert!(c.offer("gba", Target::SteamOS).unwrap().available);
        // Un émulateur existe, mais pas pour ce système.
        assert!(!c.offer("ngc", Target::Windows).unwrap().available);
        // Aucun pour cette console.
        assert!(c.offer("snes", Target::Windows).is_none());
        // Deux pour une console : celui qui tourne ici.
        assert_eq!(c.offer("psp", Target::Windows).unwrap().id, "b");
    }

    #[test]
    fn runtime_ids_are_stable_and_never_a_romm_id() {
        assert!(runtime_id("mgba") < 0);
        assert_eq!(runtime_id("mgba"), runtime_id("mgba"));
        assert_ne!(runtime_id("mgba"), runtime_id("melonds"));
    }
}

#[cfg(test)]
mod saves_tests {
    use super::*;

    #[test]
    fn emulator_saves_and_language_by_system() {
        let e: Emulator = serde_json::from_str(
            r#"{"id":"punes","name":"puNES","consoles":["nes"],"version":{"cle":"k","nom":"v0.111"},
              "saves":{"dir":"prb","bySystem":{"linux":"{appdata}/puNES/prb"},"patterns":["{romstem}.prb"]},
              "language":[{"system":"windows","format":"ini","file":"{exedir}/puNES.cfg","key":"language","value":"french"}]}"#,
        )
        .unwrap();
        let expected = if cfg!(windows) { "prb" } else { "{appdata}/puNES/prb" };
        assert_eq!(e.saves.dir_here(), Some(expected));
        assert_eq!(e.language.len(), 1);
        // Catalogue d'avant : ni bySystem ni language.
        let old: Emulator = serde_json::from_str(r#"{"id":"m","name":"m","consoles":[],"version":{"cle":"","nom":""},"saves":{"dir":"{romdir}","patterns":[]}}"#).unwrap();
        assert_eq!(old.saves.dir_here(), Some("{romdir}"));
        assert!(old.language.is_empty());
    }
}
