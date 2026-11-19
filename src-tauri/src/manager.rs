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

/// Fichiers à joindre aux rapports de bug (`journaux:` du gestionnaire) : chemin relatif au
/// dossier de l'émulateur ou de la recomp, ou commençant par `{exedir}`, `{appdata}`…, avec
/// `*` dans le dernier segment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPattern {
    #[serde(default)]
    pub system: Option<String>,
    pub pattern: String,
}

/// Dossier des mods d'après la documentation de la recomp ou de l'émulateur : relatif au dossier
/// de l'exécutable, ou commençant par `{exedir}`, `{appdata}`… ; `{titleid}` : ID de titre Switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModsDir {
    #[serde(default)]
    pub system: Option<String>,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Catalog {
    pub recomps: Vec<CatalogRecomp>,
    pub emulators: Vec<Emulator>,
    /// Recomp choisie par le joueur pour un jeu qui en a plusieurs (réglages), pas au catalogue.
    #[serde(skip)]
    pub choices: std::collections::BTreeMap<i64, String>,
}

/// Une recomp proposée pour un jeu, au choix quand il en a plusieurs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecompOffer {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Build pour ce système.
    pub available: bool,
    /// Systèmes qui ont une build (vide : recomp partagée, tous ceux du runtime).
    #[serde(default)]
    pub systems: Vec<Target>,
    /// Dépôt du projet (lien « Sources »).
    #[serde(default)]
    pub repo: Option<String>,
    /// Taille de sa build pour ce système (0 : aucune).
    #[serde(default)]
    pub size: u64,
    /// « Conseillé » : le petit message de la fiche.
    pub advice: Option<String>,
}

/// Émulateur conseillé pour une console, avec son message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleAdvice {
    pub console: String,
    pub text: String,
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
    /// Réglages optimisés selon le PC (optimize.rs).
    #[serde(default)]
    pub optimize: Vec<crate::optimize::OptimizeRule>,
    /// Journaux à joindre aux rapports de bug (report.rs).
    #[serde(default)]
    pub logs: Vec<LogPattern>,
    /// Où le joueur met ses mods (`dossier-mods:` du gestionnaire).
    #[serde(default)]
    pub mods_dirs: Vec<ModsDir>,
    /// Format d'attribution des manettes aux joueurs (controllers.rs).
    #[serde(default)]
    pub controllers: Option<String>,
    /// Données du jeu tirées de sa ROM (romfs…), servies par le gestionnaire à la place
    /// des fichiers de son entrée RomM.
    #[serde(default)]
    pub game_files: Vec<ManagerFile>,
    /// Version conseillée pour ses jeux quand ils ont plusieurs recomps ou émulateurs.
    #[serde(default)]
    pub advice: Option<String>,
    /// Dossiers hors du jeu reliés à un dossier du jeu (`lien:`).
    #[serde(default)]
    pub links: Vec<crate::links::Link>,
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
    /// Construction du jeu après l'installation : programme(s) puis arguments (`preparer:`).
    pub prepare: Vec<String>,
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
    /// Ancienne fiche `portage-*` du jeu : clé de son installation et de sa sauvegarde RomM d'avant.
    pub previous_rom_id: Option<i64>,
}

/// Argument qui ne fait que passer le launcher de la recomp : sans lui, elle ouvre son menu.
fn skips_menu(arg: &str) -> bool {
    arg == "--play" || arg.starts_with("--game=")
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
    /// Réglages optimisés selon le PC (optimize.rs).
    #[serde(default)]
    pub optimize: Vec<crate::optimize::OptimizeRule>,
    /// Journaux à joindre aux rapports de bug (report.rs).
    #[serde(default)]
    pub logs: Vec<LogPattern>,
    /// Où le joueur met ses mods (`dossier-mods:` du gestionnaire).
    #[serde(default)]
    pub mods_dirs: Vec<ModsDir>,
    /// Format d'attribution des manettes aux joueurs (controllers.rs).
    #[serde(default)]
    pub controllers: Option<String>,
    /// Questions posées au lancement et touche qui y répond (dialogs.rs).
    #[serde(default)]
    pub dialogs: Vec<crate::dialogs::DialogAnswer>,
    /// Archives posées dans le dossier de l'émulateur pour un jeu (mods, mise à jour, sauvegarde…).
    #[serde(default)]
    pub packs: Vec<EmulatorPack>,
    /// Consoles pour lesquelles il est l'émulateur conseillé, avec le message.
    #[serde(default)]
    pub advice: Vec<ConsoleAdvice>,
}

/// Archive de FarfaRomManager extraite dans le dossier de l'émulateur (`dest`, relatif à
/// l'exécutable) pour un jeu RomM, ou pour tous ses jeux.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorPack {
    #[serde(flatten)]
    pub file: ManagerFile,
    /// Jeu RomM ; absent : tous les jeux de l'émulateur.
    #[serde(default)]
    pub rom_id: Option<i64>,
    /// Posée seulement si ce chemin (relatif à `dest`) manque : jamais réécrite ensuite.
    #[serde(default)]
    pub if_missing: Option<String>,
}

impl Emulator {
    /// Message de conseil de cet émulateur pour une console.
    pub fn advice_for(&self, console: &str) -> Option<String> {
        self.advice.iter().find(|a| a.console == console).map(|a| a.text.clone())
    }

    /// Archives de ce jeu.
    pub fn packs_for(&self, rom_id: i64) -> impl Iterator<Item = &EmulatorPack> {
        self.packs.iter().filter(move |p| p.rom_id.is_none_or(|r| r == rom_id))
    }

    /// Version installée pour ce jeu : celle de l'émulateur, plus ses archives tenues à jour.
    pub fn version_for(&self, rom_id: i64) -> String {
        let packs: Vec<&str> = self.packs_for(rom_id).filter(|p| p.if_missing.is_none()).map(|p| p.file.sha1.get(..8).unwrap_or("")).collect();
        if packs.is_empty() {
            self.version.cle.clone()
        } else {
            format!("{}+{}", self.version.cle, packs.join("."))
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmulatorVersion {
    /// Identifiant exact de la version, comparé pour détecter une mise à jour.
    pub cle: String,
    /// Version affichée.
    pub nom: String,
    /// Date de publication (ISO 8601).
    #[serde(default)]
    pub date: Option<String>,
    /// Page de la release.
    #[serde(default)]
    pub url: Option<String>,
    /// Notes de version (changelog de la release GitHub).
    #[serde(default)]
    pub notes: String,
}

/// Dernières nouveautés du programme qui fait tourner un jeu (page « Informations »).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Changelog {
    /// « Recomp » ou « Émulateur ».
    pub kind: &'static str,
    pub name: String,
    pub version: String,
    pub date: Option<String>,
    pub url: Option<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EmulatorLaunch {
    pub exe: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Fichiers vides à créer à côté de l'exécutable (mode portable).
    pub portable: Vec<String>,
    /// Programmes lancés avant le jeu (ex. synchronisation de shaders).
    pub before: Vec<BeforeLaunch>,
    /// Chemins effacés avant chaque partie, relatifs à l'exécutable (cache à refaire…).
    pub clean: Vec<String>,
    /// Sur Mac, dossier des données de l'émulateur relatif à son bundle `.app`
    /// (`Contents/MacOS` pour DuckStation) : `{exedir}`, le mode portable, la configuration
    /// et les sauvegardes s'y rapportent au lieu du dossier qui contient le `.app`.
    #[serde(rename = "macData")]
    pub mac_data: Option<String>,
}

/// Programme lancé avant le jeu, relatif à l'exécutable de l'émulateur.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BeforeLaunch {
    /// `windows` ou `linux` ; absent : tous les systèmes.
    pub system: Option<String>,
    pub program: String,
    /// `{exedir}` : dossier de l'exécutable de l'émulateur.
    pub args: Vec<String>,
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
    /// Version à installer pour ce jeu : celle de l'émulateur, plus ses archives (mods…).
    #[serde(default)]
    pub game_version_key: String,
    /// Taille des archives de ce jeu (mods, mise à jour…).
    #[serde(default)]
    pub packs_size: u64,
    /// Émulateur conseillé pour cette console : le petit message de sa fiche.
    #[serde(default)]
    pub advice: Option<String>,
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
    /// Dossier de référence de l'émulateur (`{exedir}`) : celui de l'exécutable, ou sur Mac
    /// celui que désigne `macData` dans le bundle.
    pub fn data_dir(&self, game_dir: &std::path::Path, exe: &str) -> std::path::PathBuf {
        let program = game_dir.join(crate::install::rel_to_path(exe));
        if cfg!(target_os = "macos") && exe.to_ascii_lowercase().ends_with(".app") {
            if let Some(rel) = self.launch.mac_data.as_deref().filter(|r| !r.is_empty() && !r.split('/').any(|p| p == "..")) {
                return program.join(crate::install::rel_to_path(rel));
            }
        }
        program.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| game_dir.to_path_buf())
    }

    /// Build pour ce système, par ordre de préférence.
    pub fn file_for(&self, target: Target) -> Option<&ManagerFile> {
        target.candidates().iter().find_map(|t| self.files.iter().find(|f| f.system.as_deref() == Some(t.suffix())))
    }
}

impl Catalog {
    /// Émulateur d'une console : celui qui a une build pour ce système d'abord.
    /// Émulateur proposé d'abord : le conseillé pour la console s'il a une build pour ce système,
    /// sinon le premier qui en a une, sinon le premier.
    pub fn emulator_for_console(&self, console: &str, target: Target) -> Option<&Emulator> {
        let mut pour_la_console = self.emulators.iter().filter(|e| e.consoles.iter().any(|c| c == console));
        let premier = pour_la_console.clone().next();
        let conseille = pour_la_console.clone().find(|e| e.advice_for(console).is_some() && e.file_for(target).is_some());
        conseille.or_else(|| pour_la_console.find(|e| e.file_for(target).is_some())).or(premier)
    }

    pub fn offer(&self, console: &str, target: Target) -> Option<EmulatorOffer> {
        let e = self.emulator_for_console(console, target)?;
        Some(EmulatorOffer { advice: e.advice_for(console), ..Self::offer_of(e, target) })
    }

    /// Tous les émulateurs d'une console, dans l'ordre du catalogue (le premier est proposé d'abord).
    pub fn offers(&self, console: &str, target: Target) -> Vec<EmulatorOffer> {
        self.emulators
            .iter()
            .filter(|e| e.consoles.iter().any(|c| c == console))
            .map(|e| EmulatorOffer { advice: e.advice_for(console), ..Self::offer_of(e, target) })
            .collect()
    }

    /// Recomps d'un jeu, dans l'ordre du catalogue.
    pub fn recomps_for_rom(&self, rom_id: i64) -> Vec<&CatalogRecomp> {
        self.recomps.iter().filter(|r| r.games.iter().any(|g| g.rom_id == Some(rom_id))).collect()
    }

    /// Recomps proposées pour un jeu (au moins deux : le joueur choisit).
    pub fn recomp_offers(&self, rom_id: i64, target: Target) -> Vec<RecompOffer> {
        let all = self.recomps_for_rom(rom_id);
        if all.len() < 2 {
            return Vec::new();
        }
        all.into_iter().map(|r| Self::recomp_offer(r, target)).collect()
    }

    /// Une recomp telle que proposée au joueur, avec les systèmes qui ont une build.
    pub fn recomp_offer(r: &CatalogRecomp, target: Target) -> RecompOffer {
        let (archives, mine) = r.archives(target);
        RecompOffer {
            id: r.id.clone(),
            name: r.name.clone(),
            version: r.version.nom.clone(),
            available: r.is_shared() || mine.is_some(),
            systems: archives.iter().map(|a| a.target).collect(),
            repo: Some(r.repo.clone()).filter(|u| !u.is_empty()),
            size: mine.map(|a| a.file.size).unwrap_or(0),
            advice: r.advice.clone(),
        }
    }

    pub fn emulator(&self, id: &str) -> Option<&Emulator> {
        self.emulators.iter().find(|e| e.id == id)
    }

    fn offer_of(e: &Emulator, target: Target) -> EmulatorOffer {
        let file = e.file_for(target);
        EmulatorOffer {
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
            game_version_key: e.version.cle.clone(),
            packs_size: 0,
            advice: None,
        }
    }

    /// Offre d'un émulateur pour un jeu précis : ses archives comptent dans sa version et sa taille.
    pub fn for_game(mut offer: EmulatorOffer, catalog: &Catalog, rom_id: i64) -> EmulatorOffer {
        if let Some(e) = catalog.emulator(&offer.id) {
            offer.game_version_key = e.version_for(rom_id);
            offer.packs_size = e.packs_for(rom_id).map(|p| p.file.size).sum();
        }
        offer
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

    /// Réglages optimisés du programme qui lance ce jeu (émulateur ou recomp).
    pub fn optimize_for_install(&self, st: &crate::install::state::InstalledGame) -> Vec<crate::optimize::OptimizeRule> {
        match &st.shared {
            Some(s) if s.is_emulator() => self.emulator_for_install(st).map(|e| e.optimize.clone()).unwrap_or_default(),
            _ => self.recomp_for_install(st).map(|r| r.optimize.clone()).unwrap_or_default(),
        }
    }

    /// Dossier des mods de la fiche pour ce jeu installé, sur ce système.
    pub fn mods_dir_for_install(&self, st: &crate::install::state::InstalledGame) -> Option<String> {
        let dirs = match &st.shared {
            Some(s) if s.is_emulator() => self.emulator_for_install(st).map(|e| e.mods_dirs.clone()).unwrap_or_default(),
            _ => self.recomp_for_install(st).map(|r| r.mods_dirs.clone()).unwrap_or_default(),
        };
        let me = crate::language::current_system();
        dirs.iter().find(|d| d.system.as_deref() == Some(me)).or_else(|| dirs.iter().find(|d| d.system.is_none())).map(|d| d.path.clone())
    }

    /// Journaux à joindre à un rapport de bug pour ce jeu installé.
    pub fn logs_for_install(&self, st: &crate::install::state::InstalledGame) -> Vec<LogPattern> {
        match &st.shared {
            Some(s) if s.is_emulator() => self.emulator_for_install(st).map(|e| e.logs.clone()).unwrap_or_default(),
            _ => self.recomp_for_install(st).map(|r| r.logs.clone()).unwrap_or_default(),
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
        let expand = |t: &str| t.replace("{rom}", &rom).replace("{gamedir}", crate::launch::direct::GAME_DIR_VAR);
        Some(crate::install::state::DirectLaunch {
            exe,
            args: recomp.launch.args.iter().map(|a| expand(a)).collect(),
            cwd: None,
            env: recomp.launch.env.iter().map(|(k, v)| (k.clone(), expand(v))).collect(),
        })
    }

    /// Construction du jeu après l'installation de sa recomp (`preparer:`).
    pub fn recomp_prepare(&self, rom_id: i64) -> &[String] {
        self.recomp_for_rom(rom_id).filter(|r| !r.is_shared()).map(|r| r.launch.prepare.as_slice()).unwrap_or(&[])
    }

    /// Variables de la recomp (`variable:`), passées aussi à son outil de préparation
    /// (`HOOKSHOT_SETUP_BASE={gamedir}/…` : ses outils dans le dossier du jeu, pas dans AppData).
    pub fn recomp_env(&self, rom_id: i64) -> &[(String, String)] {
        self.recomp_for_rom(rom_id).filter(|r| !r.is_shared()).map(|r| r.launch.env.as_slice()).unwrap_or(&[])
    }

    /// Exécutable imposé par `exe:` pour une recomp à un seul jeu.
    pub fn recomp_exe(&self, rom_id: i64) -> Option<&str> {
        self.recomp_for_rom(rom_id).filter(|r| !r.is_shared())?.launch.exe.as_deref()
    }

    /// La recomp de ce jeu installé a un launcher à elle, autre chose que « Jouer » : recomp
    /// partagée, arguments qui ne font que passer son launcher (`--play`, `--game=…`), ou
    /// launcher relevé devant le vrai jeu. Bouton « Ouvrir la recomp ».
    pub fn recomp_menu(&self, st: &crate::install::state::InstalledGame) -> bool {
        if st.pc.is_some() || st.shared.as_ref().is_some_and(|s| s.is_emulator()) {
            return false;
        }
        if st.shared.is_some() {
            return true;
        }
        if st.direct_launch.as_ref().is_some_and(|d| st.executable.as_deref() != Some(d.exe.as_str())) {
            return true;
        }
        self.recomp_for_rom(st.rom_id)
            .filter(|r| !r.is_shared())
            .is_some_and(|r| !r.launch.args.is_empty() && r.launch.args.iter().all(|a| skips_menu(a)))
    }

    /// Notes de la dernière version d'une recomp ou d'un émulateur précis (mode choisi sur la
    /// page du jeu, installé ou non).
    pub fn changelog_of(&self, kind: &str, id: &str) -> Option<Changelog> {
        let (kind, name, v) = match kind {
            "recomp" => self.recomps.iter().find(|r| r.id == id).map(|r| ("Recomp", &r.name, &r.version))?,
            "emulator" => self.emulator(id).map(|e| ("Émulateur", &e.name, &e.version))?,
            _ => return None,
        };
        Some(Changelog { kind, name: name.clone(), version: v.nom.clone(), date: v.date.clone(), url: v.url.clone(), notes: v.notes.clone() })
    }

    /// Notes de la dernière version de la recomp du jeu, sinon de l'émulateur de sa console.
    pub fn changelog(&self, rom_id: i64, console: &str, target: Target) -> Option<Changelog> {
        let (kind, name, v) = match self.recomp_for_rom(rom_id) {
            Some(r) => ("Recomp", &r.name, &r.version),
            None => {
                let e = self.emulator_for_console(console, target)?;
                ("Émulateur", &e.name, &e.version)
            }
        };
        Some(Changelog { kind, name: name.clone(), version: v.nom.clone(), date: v.date.clone(), url: v.url.clone(), notes: v.notes.clone() })
    }

    /// Recomp d'un jeu : celle choisie par le joueur, sinon la conseillée, sinon la première.
    /// Jeu installé avant le choix des recomps (ou sans choix enregistré) : il garde la recomp
    /// dont vient son archive (`<id>_<Système>.zip`), pas la conseillée. Sinon une mise à jour
    /// passerait à une autre recomp, parfois sans build pour ce système (Longshot sous Linux).
    pub fn keep_installed_recomps<'a>(&mut self, installed: impl IntoIterator<Item = (i64, &'a str)>) {
        for (rom_id, archive) in installed {
            if self.choices.contains_key(&rom_id) {
                continue;
            }
            let all = self.recomps_for_rom(rom_id);
            if all.len() < 2 {
                continue;
            }
            if let Some(r) = all.iter().find(|r| archive.starts_with(&format!("{}_", r.id))) {
                let id = r.id.clone();
                self.choices.insert(rom_id, id);
            }
        }
    }

    pub fn recomp_for_rom(&self, rom_id: i64) -> Option<&CatalogRecomp> {
        let all = self.recomps_for_rom(rom_id);
        // Sans choix : la conseillée, puis la première, parmi celles qui ont une build pour ce
        // système (Longshot n'en a pas sous Linux : Hookshot y reste proposée).
        let me = crate::platform::detect();
        let here = |r: &&&CatalogRecomp| r.is_shared() || r.archives(me).1.is_some();
        // Un choix enregistré sans build pour ce système (Longshot retenu sous Linux par une
        // ancienne version) est ignoré : sinon la mise à jour échoue toujours (« not_available »).
        let chosen = self.choices.get(&rom_id).and_then(|id| all.iter().filter(here).find(|r| &r.id == id));
        chosen
            .or_else(|| all.iter().filter(here).find(|r| r.advice.is_some()))
            .or_else(|| all.iter().find(here))
            .or_else(|| all.iter().find(|r| r.advice.is_some()))
            .or(all.first())
            .copied()
    }

    /// Jeux passés de leur fiche `portage-*` à leur vraie console : (ancien ID, nouvel ID).
    pub fn moved_games(&self) -> Vec<(i64, i64)> {
        self.recomps.iter().flat_map(|r| r.games.iter().filter_map(|g| Some((g.previous_rom_id?, g.rom_id?)))).filter(|(a, b)| a != b).collect()
    }

    /// Ancienne fiche `portage-*` d'un jeu de recomp passé dans sa vraie console : elle garde
    /// la sauvegarde faite avec la recomp. La ROM de la console, elle, n'a sur RomM que celles
    /// des émulateurs.
    pub fn old_save_id(&self, rom_id: i64) -> Option<i64> {
        self.moved_games().into_iter().find(|(_, new)| *new == rom_id).map(|(old, _)| old)
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
            // Jeux Switch installés sous émulateur : chacun doit savoir où sont ses sauvegardes,
            // y compris ceux installés avant que la règle existe.
            let mut found = crate::switch_saves::fill_all(&catalog, &state.config_dir);
            // ID introuvable sur ce PC (XCI sans ID dans son nom) : celui qu'un autre joueur a
            // déjà appris, d'après les shaders partagés du jeu.
            for mut st in crate::install::state::list_installed(&state.config_dir) {
                if !crate::switch_saves::waiting(&catalog, &st) {
                    continue;
                }
                let Some(emu) = catalog.emulator_for_install(&st).map(|e| e.id.clone()) else { continue };
                let Ok(info) = client.get_json::<serde_json::Value>(&format!("api/shaders/rom/{}?systeme={}", st.rom_id, crate::language::current_system())).await else { continue };
                let title = info.as_object().into_iter().flat_map(|m| m.values()).find_map(|v| v.get("titleId")?.as_str().map(str::to_string));
                if let Some(title) = title {
                    crate::shaders::remember_title(&state.config_dir, &emu, st.rom_id, &title);
                    if crate::switch_saves::fill(&catalog, &state.config_dir, &mut st, None) {
                        found += 1;
                    }
                }
            }
            if found > 0 {
                tracing::info!("sauvegardes Switch : dossier trouvé pour {found} jeu(x) au démarrage");
            }
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

    #[test]
    fn recomp_choice_then_advice_then_first() {
        // Builds pour le système qui lance les tests (la CI tourne aussi sous Linux et Mac).
        let me = crate::platform::detect();
        let text = r#"{"recomps":[
          {"id":"hookshot","name":"Hookshot","repo":"r","version":{"cle":"a","nom":"2.1"},"games":[{"romId":7}],"files":[{"name":"h_Windows.zip","size":1,"sha1":"a","system":"Windows"}]},
          {"id":"longshot","name":"Longshot","repo":"r","version":{"cle":"b","nom":"v3.0.0-rc.3"},"games":[{"romId":7}],"advice":"Suite de Hookshot","files":[{"name":"l_Windows.zip","size":1,"sha1":"b","system":"Windows"}]},
          {"id":"seule","name":"Seule","repo":"r","version":{"cle":"c","nom":"1"},"games":[{"romId":8}]}],"emulators":[]}"#
            .replace("Windows", me.suffix());
        let mut c: Catalog = serde_json::from_str(&text).unwrap();
        assert_eq!(c.recomp_for_rom(7).unwrap().id, "longshot"); // conseillée
        c.choices.insert(7, "hookshot".into());
        assert_eq!(c.recomp_for_rom(7).unwrap().id, "hookshot"); // choisie
        c.choices.insert(7, "disparue".into());
        assert_eq!(c.recomp_for_rom(7).unwrap().id, "longshot"); // choix qui n'existe plus
        let offers = c.recomp_offers(7, me);
        assert_eq!(offers.iter().map(|o| (o.id.as_str(), o.available, o.advice.is_some())).collect::<Vec<_>>(),
                   vec![("hookshot", true, false), ("longshot", true, true)]);
        assert_eq!(offers[1].systems, vec![me]); // systèmes qui ont une build
        // Conseillée sans build pour ce système : celle qui en a une.
        let sans: Catalog = serde_json::from_str(&text.replace(&format!(r#"[{{"name":"l_{0}.zip","size":1,"sha1":"b","system":"{0}"}}]"#, me.suffix()), "[]")).unwrap();
        assert_eq!(sans.recomp_for_rom(7).unwrap().id, "hookshot");
        // Choix enregistré d'une recomp sans build pour ce système : ignoré.
        let mut sans = sans;
        sans.choices.insert(7, "longshot".into());
        assert_eq!(sans.recomp_for_rom(7).unwrap().id, "hookshot");
        assert!(c.recomp_offers(8, Target::Windows).is_empty()); // une seule : pas de choix
        assert_eq!(c.recomp_for_rom(8).unwrap().id, "seule");

        // Installé avec Hookshot, sans choix enregistré : il reste sur Hookshot.
        c.choices.clear();
        c.keep_installed_recomps([(7, "hookshot_Linux.zip"), (8, "seule_Linux.zip")]);
        assert_eq!(c.recomp_for_rom(7).unwrap().id, "hookshot");
        assert!(!c.choices.contains_key(&8));
        // Un choix du joueur passe avant.
        c.choices.insert(7, "longshot".into());
        c.keep_installed_recomps([(7, "hookshot_Linux.zip")]);
        assert_eq!(c.recomp_for_rom(7).unwrap().id, "longshot");
    }

    #[test]
    fn advised_emulator_comes_first() {
        let mut a = emu("a", &["switch-2"], &["Windows"]);
        let mut b = emu("b", &["switch-2"], &["Windows"]);
        let c0 = Catalog { emulators: vec![a.clone(), b.clone()], ..Default::default() };
        assert_eq!(c0.offer("switch-2", Target::Windows).unwrap().id, "a");
        b.advice = vec![ConsoleAdvice { console: "switch-2".into(), text: "Le plus rapide".into() }];
        a.advice = vec![ConsoleAdvice { console: "switch".into(), text: "autre console".into() }];
        let c = Catalog { emulators: vec![a, b], ..Default::default() };
        let o = c.offer("switch-2", Target::Windows).unwrap();
        assert_eq!((o.id.as_str(), o.advice.as_deref()), ("b", Some("Le plus rapide")));
        assert_eq!(c.offers("switch-2", Target::Windows).iter().map(|o| o.advice.is_some()).collect::<Vec<_>>(), vec![false, true]);
        // Conseillé sans build pour ce système : le premier qui en a une.
        assert_eq!(c.offer("switch-2", Target::MacArm).unwrap().id, "a");
    }

    fn emu(id: &str, consoles: &[&str], systems: &[&str]) -> Emulator {
        Emulator {
            id: id.into(),
            name: id.into(),
            consoles: consoles.iter().map(|c| c.to_string()).collect(),
            repo: Some(format!("https://github.com/x/{id}")),
            version: EmulatorVersion { cle: "k".into(), nom: "1.0".into(), ..Default::default() },
            files: systems.iter().map(|s| ManagerFile { name: format!("{id}_{s}.zip"), size: 5, sha1: "a".repeat(40), system: Some(s.to_string()), dest: None }).collect(),
            launch: EmulatorLaunch::default(),
            saves: EmulatorSaves::default(),
            config: vec![],
            config_missing: vec![],
            language: vec![],
            optimize: vec![],
            logs: vec![],
            mods_dirs: vec![],
            controllers: None,
            dialogs: vec![],
            packs: vec![],
            advice: vec![],
        }
    }

    #[test]
    fn packs_of_a_game_count_in_its_version() {
        let text = r#"{"recomps":[],"emulators":[{"id":"eden","name":"Eden","consoles":["switch"],"version":{"cle":"v1","nom":"v0.2.1"},
          "packs":[{"romId":null,"name":"fw.zip","dest":"user","sha1":"aaaaaaaaaaaa","size":10},
                   {"romId":192,"name":"mods.zip","dest":"user","sha1":"bbbbbbbbbbbb","size":20},
                   {"romId":192,"name":"save.zip","dest":"user","sha1":"cccccccccccc","size":5,"ifMissing":"nand/x"}]}]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        let e = c.emulator("eden").unwrap();
        assert_eq!(e.packs_for(192).count(), 3);
        assert_eq!(e.packs_for(7).count(), 1);
        // Une sauvegarde de départ (« si-absent ») ne change jamais la version.
        assert_eq!(e.version_for(192), "v1+aaaaaaaa.bbbbbbbb");
        let o = Catalog::for_game(c.offer("switch", Target::Windows).unwrap(), &c, 192);
        assert_eq!((o.game_version_key.as_str(), o.packs_size), ("v1+aaaaaaaa.bbbbbbbb", 35));
    }

    #[test]
    fn changelog_of_the_recomp_or_the_emulator() {
        let text = r###"{"recomps":[{"id":"pc","name":"Paper Cut","repo":"r","version":{"cle":"v1","nom":"v0.4.1","date":"2026-11-01T00:00:00Z","url":"u","notes":"## Nouveau\n- truc"},
          "games":[{"romId":42}]}],"emulators":[{"id":"mgba","name":"mGBA","consoles":["gba"],"version":{"cle":"k","nom":"0.10.5"}}]}"###;
        let c: Catalog = serde_json::from_str(text).unwrap();
        let r = c.changelog(42, "ngc", Target::Windows).unwrap();
        assert_eq!((r.kind, r.name.as_str(), r.version.as_str(), r.notes.as_str()), ("Recomp", "Paper Cut", "v0.4.1", "## Nouveau\n- truc"));
        let e = c.changelog(7, "gba", Target::Windows).unwrap();
        assert_eq!((e.kind, e.name.as_str(), e.notes.as_str()), ("Émulateur", "mGBA", ""));
        assert!(c.changelog(7, "snes", Target::Windows).is_none());
        // Mode choisi sur la page : l'émulateur même si le jeu a une recomp.
        assert_eq!(c.changelog_of("emulator", "mgba").unwrap().name, "mGBA");
        assert_eq!(c.changelog_of("recomp", "pc").unwrap().kind, "Recomp");
        assert!(c.changelog_of("emulator", "inconnu").is_none());
    }

    #[test]
    fn skipping_arguments_open_the_recomp_menu() {
        assert!(skips_menu("--play"));
        assert!(skips_menu("--game=yellow"));
        assert!(!skips_menu("{rom}"));
        assert!(!skips_menu("--data-dir"));
    }

    #[test]
    fn games_moved_from_portage_keep_their_old_id() {
        let text = r#"{"recomps":[{"id":"mh","repo":"r","games":[{"name":"MH","romId":10532,"previousRomId":10557},{"name":"P","romId":3406}],
          "gameFiles":[{"name":"MHGU-data.extract.zip","path":"jeux/mh/MHGU-data.extract.zip","size":5,"sha1":"abcdef0123456789abcdef0123456789abcdef01"}]}],"emulators":[]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        assert_eq!(c.moved_games(), [(10557, 10532)]);
        assert_eq!(c.old_save_id(10532), Some(10557));
        assert_eq!(c.old_save_id(3406), None);
        let mut g = crate::romm::library::Game::from_catalog(&c.recomps[0], Target::Windows);
        g.rom_files = vec![RemoteFile { id: 1, file_name: "x.nsp".into(), rel_path: "x.nsp".into(), size: 1, sha1: None, md5: None, from_manager: false }];
        g.use_catalog(&c.recomps[0], Target::Windows);
        assert_eq!(g.rom_files.len(), 1);
        assert_eq!(g.rom_files[0].rel_path, "ROM/MHGU-data.extract.zip");
        assert!(g.rom_files[0].from_manager);
    }

    #[test]
    fn single_recomp_launches_its_rom_directly() {
        let text = r#"{"recomps":[{"id":"paper-cut","repo":"https://github.com/LLCOOLT/paper-cut","games":[{"name":"TTYD","romId":3406}],
          "launch":{"exe":"ttyd-eu","args":["{rom}"]}}],"emulators":[]}"#;
        let c: Catalog = serde_json::from_str(text).unwrap();
        assert_eq!(c.recomp_exe(3406), Some("ttyd-eu"));
        assert_eq!(c.recomp_exe(1), None);
        let st = crate::install::state::InstalledGame {
            ways: Vec::new(),
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
        // {gamedir} dans les arguments et les variables (ReRAC : --data-dir {gamedir}/data).
        let rerac: Catalog = serde_json::from_str(
            r#"{"recomps":[{"id":"r","repo":"r","games":[{"name":"R","romId":3406}],
              "launch":{"exe":"rerac","args":["--data-dir","{gamedir}/data"],"env":[["RC_SAVE_DIR","{gamedir}/memcard"]]}}],"emulators":[]}"#,
        )
        .unwrap();
        let d = rerac.recomp_launch(&st).unwrap();
        let var = crate::launch::direct::GAME_DIR_VAR;
        assert_eq!(d.args, ["--data-dir".to_string(), format!("{var}/data")]);
        assert_eq!(d.env, [("RC_SAVE_DIR".to_string(), format!("{var}/memcard"))]);
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
        let c = Catalog { recomps: vec![], emulators: vec![emu("mgba", &["gba"], &["Linux", "Windows"]), emu("mac", &["ngc"], &["macOS_ARM"]), emu("a", &["psp"], &["macOS_ARM"]), emu("b", &["psp"], &["Windows"])], ..Default::default() };
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
        let expected = if cfg!(target_os = "linux") { "{appdata}/puNES/prb" } else { "prb" };
        assert_eq!(e.saves.dir_here(), Some(expected));
        assert_eq!(e.language.len(), 1);
        // Catalogue d'avant : ni bySystem ni language.
        let old: Emulator = serde_json::from_str(r#"{"id":"m","name":"m","consoles":[],"version":{"cle":"","nom":""},"saves":{"dir":"{romdir}","patterns":[]}}"#).unwrap();
        assert_eq!(old.saves.dir_here(), Some("{romdir}"));
        assert!(old.language.is_empty());
    }
}
