//! Lancement direct appris.
//!
//! Certains jeux s'ouvrent sur un launcher (choix de la ROM, réglages,
//! compilation du port…) qui démarre ensuite le vrai jeu, un autre programme
//! du dossier. Pendant une partie lancée par l'exécutable du jeu, on relève les
//! programmes démarrés ; si l'un d'eux est le jeu, « Jouer » le lancera
//! directement les fois suivantes, avec les mêmes arguments.
//!
//! Est retenu comme jeu le dernier programme démarré qui :
//! - se trouve dans le dossier du jeu (un launcher Electron ou AppImage se
//!   décompresse dans un dossier temporaire : il n'en fait pas partie) ;
//! - n'est pas l'exécutable lancé ni un outil (compilateur, git, updater…) ;
//! - a tourné au moins [`MIN_GAME`].
//!
//! On garde aussi ses arguments, son dossier de travail et les variables
//! d'environnement que le launcher lui a données.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::install::detect::is_annex_name;
use crate::install::path_to_rel;
use crate::install::state::DirectLaunch;

/// Durée minimale pour être pris pour le jeu.
const MIN_GAME: Duration = Duration::from_secs(20);
/// Remplace le dossier du jeu dans les arguments (le jeu peut être déplacé).
pub const GAME_DIR_VAR: &str = "{GAME_DIR}";

/// Outils qu'un launcher lance sans que ce soit le jeu.
const TOOLS: &[&str] = &[
    "git", "git-remote-https", "cmake", "ninja", "make", "mingw32-make", "gcc", "g++", "cc", "c++", "cc1", "cc1plus", "as",
    "ld", "collect2", "ar", "clang", "clang++", "lld", "lld-link", "cl", "link", "msbuild", "python", "python3", "pythonw",
    "node", "sh", "bash", "cmd", "powershell", "pwsh", "conhost", "curl", "wget", "tar", "unzip", "7z", "xdg-open",
];

/// Variables propres au launcher (Electron, AppImage…), jamais reprises.
const LAUNCHER_ENV: &[&str] = &["ELECTRON_", "CHROME_", "APPDIR", "APPIMAGE", "ARGV0", "OWD", "PORTABLE_EXECUTABLE_"];

struct Seen {
    /// Programme, tel que le système le donne.
    exe: PathBuf,
    /// Le même, comparable (voir [`normalize`]).
    key: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    /// `CLE=valeur`, tel que le système le donne.
    environ: Vec<String>,
    first: Instant,
    last: Instant,
    /// Relevés : les infos sont relues tant qu'il y en a moins de 2 (sous Linux,
    /// un processus tout juste créé porte encore le programme de son parent).
    reads: u8,
}

pub struct Watcher {
    sys: System,
    game_dir: PathBuf,
    launched: PathBuf,
    seen: HashMap<u32, Seen>,
}

impl Watcher {
    pub fn new(game_dir: &Path, launched: &Path) -> Self {
        // Chemins réels : le système donne ceux des processus liens résolus
        // (sous Bazzite, /home est un lien vers /var/home).
        let real = |p: &Path| normalize(&std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
        Self { sys: System::new(), game_dir: real(game_dir), launched: real(launched), seen: HashMap::new() }
    }

    /// Relève les processus de la partie (à appeler à chaque tour de surveillance).
    pub fn observe(&mut self, pids: &[u32]) {
        let now = Instant::now();
        let to_read: Vec<Pid> =
            pids.iter().filter(|p| self.seen.get(p).is_none_or(|s| s.reads < 2)).map(|&p| Pid::from_u32(p)).collect();
        if !to_read.is_empty() {
            let kind = ProcessRefreshKind::nothing()
                .with_exe(UpdateKind::Always)
                .with_cmd(UpdateKind::Always)
                .with_cwd(UpdateKind::Always)
                .with_environ(UpdateKind::Always);
            self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&to_read), true, kind);
            for pid in to_read {
                let Some(p) = self.sys.process(pid) else { continue };
                let Some(exe) = p.exe().map(Path::to_path_buf) else { continue };
                let key = normalize(&exe);
                let args = p.cmd().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
                let cwd = p.cwd().map(Path::to_path_buf);
                let environ = p.environ().iter().map(|v| v.to_string_lossy().into_owned()).collect();
                let entry = self.seen.entry(pid.as_u32()).or_insert_with(|| Seen {
                    exe: exe.clone(),
                    key: key.clone(),
                    args: Vec::new(),
                    cwd: None,
                    environ: Vec::new(),
                    first: now,
                    last: now,
                    reads: 0,
                });
                if entry.key != key {
                    // Programme remplacé (exec) : sa durée repart de zéro.
                    entry.first = now;
                }
                entry.exe = exe;
                entry.key = key;
                entry.args = args;
                entry.cwd = cwd;
                entry.environ = environ;
                entry.reads += 1;
            }
        }
        for pid in pids {
            if let Some(s) = self.seen.get_mut(pid) {
                s.last = now;
            }
        }
    }

    /// Le jeu démarré par le launcher, s'il y en a un.
    pub fn learned(&self) -> Option<DirectLaunch> {
        let game = self
            .seen
            .values()
            .filter(|s| s.key != self.launched && s.last - s.first >= MIN_GAME && !is_tool(&s.exe))
            .filter(|s| s.key.starts_with(&self.game_dir))
            .max_by_key(|s| s.first)?;
        let exe = self.rel(&game.exe)?;
        let args = game.args.iter().map(|a| self.hide_dir(a)).collect();
        let cwd = game.cwd.as_ref().and_then(|c| self.rel(c));
        // Dossier de travail = celui du programme : inutile de le retenir.
        let exe_dir = Path::new(&exe).parent().map(path_to_rel).unwrap_or_default();
        let cwd = cwd.filter(|c| *c != exe_dir);
        let env = self.launcher_env(&game.environ);
        Some(DirectLaunch { exe, args, cwd, env })
    }
}

impl Watcher {
    /// `path` relatif au dossier du jeu, avec sa casse d'origine.
    fn rel(&self, path: &Path) -> Option<String> {
        if !normalize(path).starts_with(&self.game_dir) {
            return None;
        }
        let plain = PathBuf::from(path.to_string_lossy().trim_start_matches(r"\\?\"));
        Some(path_to_rel(&plain.components().skip(self.game_dir.components().count()).collect::<PathBuf>()))
    }

    /// Variables que le launcher a ajoutées ou changées par rapport aux nôtres.
    fn launcher_env(&self, environ: &[String]) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = environ
            .iter()
            .filter_map(|kv| kv.split_once('='))
            // Sous Windows, `=C:=C:…` : dossiers courants par lecteur, pas des variables.
            .filter(|(k, _)| !k.is_empty() && !LAUNCHER_ENV.iter().any(|p| k.to_uppercase().starts_with(p)))
            .filter(|(k, v)| std::env::var(k).ok().as_deref() != Some(*v))
            .map(|(k, v)| (k.to_string(), self.hide_dir(v)))
            .collect();
        env.sort();
        env
    }

    /// Remplace le dossier du jeu par [`GAME_DIR_VAR`] dans un argument.
    fn hide_dir(&self, arg: &str) -> String {
        let dir = self.game_dir.to_string_lossy();
        let comparable = if cfg!(windows) { arg.replace('/', "\\").to_lowercase() } else { arg.to_string() };
        match comparable.find(dir.as_ref()) {
            // Même longueur : les positions correspondent dans l'argument d'origine.
            Some(i) if comparable.len() == arg.len() => format!("{}{GAME_DIR_VAR}{}", &arg[..i], &arg[i + dir.len()..]),
            _ => arg.to_string(),
        }
    }
}

/// Arguments à passer au jeu installé dans `game_dir`.
pub fn expand_args(args: &[String], game_dir: &Path) -> Vec<String> {
    let dir = game_dir.to_string_lossy();
    args.iter().map(|a| a.replace(GAME_DIR_VAR, &dir)).collect()
}

fn is_tool(exe: &Path) -> bool {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    TOOLS.contains(&stem.as_str()) || is_annex_name(&stem)
}

/// Chemin comparable : sans préfixe `\\?\` et, sous Windows, sans distinction de casse.
fn normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    if cfg!(windows) {
        PathBuf::from(s.replace('/', "\\").to_lowercase())
    } else {
        PathBuf::from(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(exe: &str, args: &[&str], cwd: Option<&str>, start: u64, secs: u64, t0: Instant) -> Seen {
        let first = t0 + Duration::from_secs(start);
        Seen {
            environ: vec![],
            exe: PathBuf::from(exe),
            key: normalize(Path::new(exe)),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: cwd.map(PathBuf::from),
            first,
            last: first + Duration::from_secs(secs),
            reads: 2,
        }
    }

    #[test]
    fn picks_the_game_started_by_the_launcher() {
        let root = if cfg!(windows) { r"C:\Jeux\Sunshine" } else { "/jeux/Sunshine" };
        let at = |rel: &str| Path::new(root).join(rel).to_string_lossy().into_owned();
        let t0 = Instant::now();
        let mut w = Watcher::new(Path::new(root), Path::new(&at("Launcher.exe")));
        let temp = if cfg!(windows) { r"C:\Temp\x\Launcher.exe" } else { "/tmp/.mount_x/launcher" };
        w.seen.insert(1, seen(&at("Launcher.exe"), &[], None, 0, 600, t0));
        w.seen.insert(2, seen(temp, &[], None, 1, 600, t0));
        w.seen.insert(3, seen(&at("tools/bin/ninja.exe"), &[], None, 10, 300, t0));
        w.seen.insert(4, seen(&at("tools/bin/cc1plus.exe"), &[], None, 20, 30, t0));
        let cfg = at("port/sms.ini");
        w.seen.insert(5, seen(&at("port/build/Sms.exe"), &["--config", &cfg], Some(&at("port")), 320, 200, t0));
        w.seen.insert(6, seen(&at("port/build/crashpad_handler.exe"), &[], None, 321, 200, t0));
        let d = w.learned().unwrap();
        assert_eq!(d.exe, "port/build/Sms.exe");
        assert_eq!(d.cwd.as_deref(), Some("port"));
        assert_eq!(d.args[0], "--config");
        assert!(d.args[1].starts_with(GAME_DIR_VAR), "{:?}", d.args);
        assert_eq!(expand_args(&d.args, Path::new(root))[1], cfg);
    }

    #[test]
    fn nothing_learned_when_the_executable_is_the_game() {
        let root = if cfg!(windows) { r"C:\Jeux\Mario" } else { "/jeux/Mario" };
        let exe = Path::new(root).join("sm64.exe");
        let t0 = Instant::now();
        let mut w = Watcher::new(Path::new(root), &exe);
        w.seen.insert(1, seen(&exe.to_string_lossy(), &[], None, 0, 900, t0));
        // Programme bref (moins de 20 s) : pas le jeu.
        w.seen.insert(2, seen(&Path::new(root).join("helper2.exe").to_string_lossy(), &[], None, 5, 3, t0));
        assert_eq!(w.learned(), None);
    }

    #[test]
    fn records_running_children() {
        let dir = std::env::temp_dir();
        let me = std::env::current_exe().unwrap();
        let mut w = Watcher::new(&dir, &dir.join("none"));
        w.observe(&[std::process::id()]);
        let s = w.seen.get(&std::process::id()).expect("processus relevé");
        assert_eq!(s.key, normalize(&me));
    }
}
