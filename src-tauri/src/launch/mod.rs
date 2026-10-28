//! Lancement d'un jeu et suivi de sa durée.
//!
//! Le jeu est lancé avec le dossier de son exécutable comme répertoire de
//! travail. La session dure tant que le processus lancé **ou l'un de ses
//! descendants** tourne (certains jeux relancent un autre processus) :
//! - Windows : le processus est placé dans un Job Object, qui englobe
//!   automatiquement tous les processus qu'il crée ;
//! - Linux : le jeu reçoit son propre groupe de processus, hérité par ses
//!   enfants ; on surveille ce groupe dans /proc ;
//! - macOS : le bundle `.app` est ouvert via `open -W`, qui ne rend la main
//!   qu'à la fermeture de l'application.
//!
//! Les programmes démarrés pendant la partie sont relevés : voir [`direct`].

pub mod direct;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{log_err, AppError, AppResult};
use crate::install::state::DirectLaunch;

/// Intervalle de surveillance des processus.
const POLL: Duration = Duration::from_secs(2);
/// Intervalle tant que la fenêtre du jeu n'est pas apparue.
const WINDOW_POLL: Duration = Duration::from_millis(300);
/// Jeu fermé plus vite que ça : sa sortie (erreurs) est recopiée dans nos logs.
const QUICK_EXIT: Duration = Duration::from_secs(15);
/// Sortie du dernier lancement, dans le dossier interne du jeu.
const RUN_LOG: &str = "dernier-lancement.log";

/// Jeu lancé, à surveiller jusqu'à sa fermeture.
pub struct RunningGame {
    child: Child,
    started: Instant,
    run_log: Option<PathBuf>,
    /// Relevé des programmes lancés (`None` : lancement direct, rien à apprendre).
    watcher: Option<direct::Watcher>,
    #[cfg(windows)]
    job: windows_job::Job,
}

/// Programme à lancer, relatif au dossier du jeu.
pub struct Launch<'a> {
    pub exe: PathBuf,
    pub args: &'a [String],
    /// Variables d'environnement en plus (`{GAME_DIR}` remplacé).
    pub env: &'a [(String, String)],
    /// Dossier de travail (`None` : celui du programme).
    pub cwd: Option<PathBuf>,
    /// Relever le jeu que ce programme démarre (c'est peut-être un launcher).
    pub learn: bool,
    /// Jeu Windows sous Linux : lancé avec Proton.
    pub runner: Option<&'a crate::pc::proton::Runner>,
}

/// Lance le programme dans le dossier du jeu (`launch.exe` peut aussi être
/// absolu : client installé hors du jeu, comme Plutonium).
pub fn spawn(game_dir: &Path, launch: Launch<'_>) -> AppResult<RunningGame> {
    let exe = game_dir.join(&launch.exe);
    if !exe.exists() {
        return Err(AppError::NoExecutable);
    }
    let work_dir = match &launch.cwd {
        Some(rel) if game_dir.join(rel).is_dir() => game_dir.join(rel),
        _ => exe.parent().unwrap_or(game_dir).to_path_buf(),
    };
    let args = direct::expand_args(launch.args, game_dir);
    let mut cmd = match launch.runner {
        Some(r) => r.command(&exe, &args),
        None => {
            let mut c = build_command(&exe);
            c.args(&args);
            c
        }
    };
    cmd.current_dir(&work_dir);
    crate::external::use_system_env(&mut cmd);
    for (k, v) in launch.env {
        cmd.env(k, direct::expand_args(std::slice::from_ref(v), game_dir).remove(0));
    }
    // Sortie du jeu gardée dans un fichier : utile s'il se ferme aussitôt.
    let run_log = game_dir.join(crate::install::extract::INTERNAL_DIR).join(RUN_LOG);
    let run_log = std::fs::File::create(&run_log).ok().and_then(|f| {
        let err = f.try_clone().ok()?;
        cmd.stdout(Stdio::from(f)).stderr(Stdio::from(err));
        Some(run_log)
    });
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Nouveau groupe : les enfants du jeu en héritent.
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| log_err(AppError::LaunchFailed, &format!("lancement de {}", exe.display()), e))?;
    tracing::info!("jeu lancé: {} {:?} (pid {})", exe.display(), launch.args, child.id());
    let watcher = launch.learn.then(|| direct::Watcher::new(game_dir, &exe));

    #[cfg(windows)]
    {
        let job = windows_job::Job::create()?;
        if let Err(e) = job.assign(&child) {
            // Sans job, on suit au moins le processus principal.
            tracing::warn!("processus non rattaché au job: {e}");
        }
        Ok(RunningGame { child, started: Instant::now(), run_log, watcher, job })
    }
    #[cfg(not(windows))]
    Ok(RunningGame { child, started: Instant::now(), run_log, watcher })
}

fn build_command(exe: &Path) -> Command {
    let lower = exe.to_string_lossy().to_lowercase();
    if cfg!(target_os = "macos") && lower.ends_with(".app") {
        let mut c = Command::new("open");
        // -W : attendre la fermeture ; -n : nouvelle instance.
        c.arg("-W").arg("-n").arg(exe);
        return c;
    }
    if cfg!(unix) && lower.ends_with(".sh") {
        let mut c = Command::new("sh");
        c.arg(exe);
        return c;
    }
    Command::new(exe)
}

impl RunningGame {
    /// Attend la fin du jeu et de tous ses processus (bloquant : à appeler
    /// dans `spawn_blocking`). Renvoie le jeu démarré par un launcher, s'il y en a eu un.
    /// `on_window` : appelé une fois, quand une fenêtre du jeu apparaît (Windows ;
    /// ailleurs, jamais).
    pub fn wait(mut self, mut on_window: impl FnMut()) -> Option<DirectLaunch> {
        // Surveillance rapprochée tant que la fenêtre du jeu n'est pas là.
        let mut window_seen = !cfg!(windows);
        loop {
            if !window_seen && self.has_window() {
                window_seen = true;
                on_window();
            }
            if self.watcher.is_some() {
                let pids = self.process_ids();
                if let Some(w) = self.watcher.as_mut() {
                    w.observe(&pids);
                }
            }
            // Récolte le processus principal s'il est terminé (évite un zombie).
            let main_alive = matches!(self.child.try_wait(), Ok(None));
            if !main_alive && !self.descendants_alive() {
                break;
            }
            std::thread::sleep(if window_seen { POLL } else { WINDOW_POLL });
        }
        let status = self.child.try_wait().ok().flatten();
        tracing::info!("jeu fermé (pid {}, {})", self.child.id(), status.map(|s| s.to_string()).unwrap_or_else(|| "état inconnu".into()));
        if self.started.elapsed() < QUICK_EXIT {
            self.log_output_tail();
        }
        let learned = self.watcher.as_ref().and_then(direct::Watcher::learned);
        if let Some(d) = &learned {
            tracing::info!("jeu démarré par le launcher: {} {:?}", d.exe, d.args);
        }
        learned
    }

    /// Processus de la partie (le programme lancé et ses descendants).
    #[cfg(windows)]
    fn process_ids(&self) -> Vec<u32> {
        self.job.process_ids()
    }

    /// Une fenêtre visible appartient-elle à la partie ?
    #[cfg(windows)]
    fn has_window(&self) -> bool {
        windows_job::visible_window(&self.job.process_ids())
    }

    #[cfg(not(windows))]
    fn has_window(&self) -> bool {
        false
    }

    #[cfg(target_os = "linux")]
    fn process_ids(&self) -> Vec<u32> {
        group_members(self.child.id())
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn process_ids(&self) -> Vec<u32> {
        Vec::new()
    }

    /// Fin de la sortie du jeu, pour comprendre une fermeture immédiate.
    fn log_output_tail(&self) {
        let Some(text) = self.run_log.as_ref().and_then(|p| std::fs::read(p).ok()) else { return };
        let tail = String::from_utf8_lossy(&text[text.len().saturating_sub(3000)..]).trim().to_string();
        if tail.is_empty() {
            tracing::warn!("jeu fermé aussitôt, sans rien afficher");
        } else {
            tracing::warn!("jeu fermé aussitôt ; fin de sa sortie :
{tail}");
        }
    }

    #[cfg(windows)]
    fn descendants_alive(&self) -> bool {
        self.job.active_processes() > 0
    }

    #[cfg(target_os = "linux")]
    fn descendants_alive(&self) -> bool {
        group_alive(self.child.id())
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    fn descendants_alive(&self) -> bool {
        // macOS : `open -W` couvre la durée de vie de l'application.
        false
    }
}

/// Un processus non zombie appartient-il encore au groupe `pgid` ?
#[cfg(target_os = "linux")]
fn group_alive(pgid: u32) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else { return false };
    rd.filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
        .any(|stat| parse_stat(&stat).map(|(state, pgrp)| state != 'Z' && pgrp == pgid).unwrap_or(false))
}

/// Processus non zombies du groupe `pgid`.
#[cfg(target_os = "linux")]
fn group_members(pgid: u32) -> Vec<u32> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
            parse_stat(&stat).filter(|&(state, pgrp)| state != 'Z' && pgrp == pgid).map(|_| pid)
        })
        .collect()
}

/// `pid (comm) state ppid pgrp …` → (state, pgrp). `comm` peut contenir des espaces.
#[cfg(any(target_os = "linux", test))]
fn parse_stat(stat: &str) -> Option<(char, u32)> {
    let rest = &stat[stat.rfind(')')? + 2..];
    let mut it = rest.split_whitespace();
    let state = it.next()?.chars().next()?;
    let _ppid = it.next()?;
    let pgrp = it.next()?.parse().ok()?;
    Some((state, pgrp))
}

#[cfg(windows)]
mod windows_job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, JobObjectBasicProcessIdList,
        QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    };

    /// Nombre maximal de processus relevés à la fois.
    const MAX_IDS: usize = 512;

    /// `JOBOBJECT_BASIC_PROCESS_ID_LIST` avec de la place pour [`MAX_IDS`] processus.
    #[repr(C)]
    struct IdList {
        assigned: u32,
        listed: u32,
        ids: [usize; MAX_IDS],
    }

    use crate::error::{AppError, AppResult};

    /// Job Object sans limite : il sert seulement à compter les processus du
    /// jeu. Le fermer ne tue pas le jeu (pas de KILL_ON_JOB_CLOSE).
    pub struct Job(HANDLE);

    // Le handle n'est utilisé que par le thread de surveillance.
    unsafe impl Send for Job {}

    impl Job {
        pub fn create() -> AppResult<Self> {
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                tracing::error!("CreateJobObjectW: {}", std::io::Error::last_os_error());
                return Err(AppError::LaunchFailed);
            }
            Ok(Self(h))
        }

        pub fn assign(&self, child: &Child) -> std::io::Result<()> {
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub fn active_processes(&self) -> u32 {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut _,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                0
            } else {
                info.ActiveProcesses
            }
        }

        pub fn process_ids(&self) -> Vec<u32> {
            let mut list = Box::new(IdList { assigned: 0, listed: 0, ids: [0; MAX_IDS] });
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicProcessIdList,
                    &mut *list as *mut IdList as *mut _,
                    std::mem::size_of::<IdList>() as u32,
                    std::ptr::null_mut(),
                )
            };
            // Liste trop longue : remplie en partie, ce qui suffit.
            if ok == 0 && list.listed == 0 {
                return Vec::new();
            }
            list.ids[..(list.listed as usize).min(MAX_IDS)].iter().map(|&id| id as u32).collect()
        }
    }

    /// Une fenêtre visible appartient-elle à l'un de ces processus ?
    pub fn visible_window(pids: &[u32]) -> bool {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};

        struct Search<'a> {
            pids: &'a [u32],
            found: bool,
        }
        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
            let search = unsafe { &mut *(data as *mut Search) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if unsafe { IsWindowVisible(hwnd) } != 0 && search.pids.contains(&pid) {
                search.found = true;
                return 0; // fin de l'énumération
            }
            1
        }
        if pids.is_empty() {
            return false;
        }
        let mut search = Search { pids, found: false };
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as LPARAM) };
        search.found
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat() {
        assert_eq!(parse_stat("1234 (my game (x64)) S 1 1234 1234 0 -1"), Some(('S', 1234)));
        assert_eq!(parse_stat("99 (zz) Z 5 77 77"), Some(('Z', 77)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[cfg(windows)]
    #[test]
    fn tracks_child_processes_on_windows() {
        // cmd lance un sous-processus (ping) puis se termine : la session doit
        // durer jusqu'à la fin de ping.
        let dir = std::env::temp_dir();
        let started = std::time::Instant::now();
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "/B", "ping", "-n", "4", "127.0.0.1", ">", "NUL"]).current_dir(&dir);
        let child = cmd.spawn().unwrap();
        let job = windows_job::Job::create().unwrap();
        job.assign(&child).unwrap();
        let watcher = Some(direct::Watcher::new(&dir, Path::new("cmd.exe")));
        let learned = RunningGame { child, started: Instant::now(), run_log: None, watcher, job }.wait(|| {});
        // ping hors du dossier et trop bref : ce n'est pas un jeu.
        assert_eq!(learned, None);
        assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
    }

    /// Un « launcher » (cmd) qui démarre le « jeu » (une copie de ping dans le
    /// dossier du jeu) : le jeu est relevé. Lent (~25 s) : `cargo test -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn learns_the_game_started_by_a_launcher() {
        let game_dir = std::env::temp_dir().join(format!("farfa-direct-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(game_dir.join("bin")).unwrap();
        std::fs::copy(r"C:\Windows\System32\PING.EXE", game_dir.join("bin/Jeu.exe")).unwrap();
        std::fs::write(game_dir.join("launcher.cmd"), "@set \"FARFA_TEST=%~dp0x\"\r\n@\"%~dp0bin\\Jeu.exe\" -n 25 127.0.0.1 > NUL\r\n").unwrap();
        let launch = Launch { exe: "launcher.cmd".into(), args: &[], env: &[], cwd: None, learn: true, runner: None };
        let learned = spawn(&game_dir, launch).unwrap().wait(|| {}).expect("jeu relevé");
        assert_eq!(learned.exe, "bin/Jeu.exe");
        assert_eq!(learned.args, ["-n", "25", "127.0.0.1"]);
        // Le launcher est resté à la racine du jeu : le jeu y sera lancé aussi.
        assert_eq!(learned.cwd.as_deref(), Some(""));
        // Variable ajoutée par le launcher, avec le dossier du jeu remplacé.
        let var = learned.env.iter().find(|(k, _)| k == "FARFA_TEST").expect("variable relevée");
        assert_eq!(var.1, format!("{}\\x", direct::GAME_DIR_VAR));
        assert!(learned.env.iter().all(|(k, _)| k != "PATH"), "{:?}", learned.env);
        let _ = std::fs::remove_dir_all(&game_dir);
    }
}
