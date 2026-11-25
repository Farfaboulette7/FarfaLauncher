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

pub mod before;
pub mod direct;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
/// Fermeture demandée (menu en jeu) : délai laissé au jeu pour se fermer seul avant d'être arrêté.
const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// Pas de la surveillance : une demande de fermeture est prise en compte aussi vite.
const STEP: Duration = Duration::from_millis(200);
/// Fenêtre du jeu fermée mais programme encore là (Eden reste en arrière-plan) : passé ce
/// délai sans aucune fenêtre, le jeu est considéré comme fermé et ses processus arrêtés.
const WINDOW_GONE: Duration = Duration::from_secs(10);

/// Jeu lancé, à surveiller jusqu'à sa fermeture.
pub struct RunningGame {
    child: Child,
    started: Instant,
    run_log: Option<PathBuf>,
    /// Relevé des programmes lancés (`None` : lancement direct, rien à apprendre).
    watcher: Option<direct::Watcher>,
    /// Fermeture demandée par le joueur (menu en jeu).
    stop: Arc<AtomicBool>,
    /// Jeu suspendu par le menu en jeu.
    paused: Arc<AtomicBool>,
    #[cfg(windows)]
    job: Arc<windows_job::Job>,
}

/// Pause du jeu pendant le menu en jeu : tous ses processus sont suspendus
/// (ni image, ni son, ni lecture des manettes), puis relancés tels quels.
#[derive(Clone)]
pub struct GameControl {
    #[cfg(windows)]
    job: Arc<windows_job::Job>,
    #[cfg(unix)]
    pgid: u32,
    paused: Arc<AtomicBool>,
}

impl GameControl {
    pub fn pause(&self) {
        if self.paused.swap(true, Ordering::SeqCst) {
            return;
        }
        tracing::info!("jeu en pause (menu en jeu)");
        #[cfg(windows)]
        windows_job::suspend(&self.job.process_ids(), true);
        #[cfg(unix)]
        signal_group(self.pgid, "STOP");
    }

    pub fn resume(&self) {
        if !self.paused.swap(false, Ordering::SeqCst) {
            return;
        }
        tracing::info!("jeu repris");
        #[cfg(windows)]
        windows_job::suspend(&self.job.process_ids(), false);
        #[cfg(unix)]
        signal_group(self.pgid, "CONT");
    }
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
    // Bundle macOS : son programme est lancé directement (arguments, variables, arrêt et
    // pause comme ailleurs), depuis son dossier (Contents/MacOS) sauf `cwd` donné.
    let exe = app_binary(&exe).unwrap_or(exe);
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
    crate::language::linux_locale(&mut cmd);
    // Jeton du smoke test (moteur Deck) : le jeu n'en hérite jamais.
    cmd.env_remove("FARFALAUNCHERDECK_JETON");
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
        Ok(RunningGame { child, started: Instant::now(), run_log, watcher, stop: Arc::default(), paused: Arc::default(), job: Arc::new(job) })
    }
    #[cfg(not(windows))]
    Ok(RunningGame { child, started: Instant::now(), run_log, watcher, stop: Arc::default(), paused: Arc::default() })
}

/// Programme à l'intérieur d'un bundle macOS : celui que nomme `CFBundleExecutable`
/// dans son Info.plist (le dossier MacOS contient aussi des bibliothèques et des aides).
pub fn app_binary(exe: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") || !exe.to_string_lossy().to_lowercase().ends_with(".app") {
        return None;
    }
    let macos = exe.join("Contents").join("MacOS");
    let plist = exe.join("Contents").join("Info.plist");
    let named = std::fs::read_to_string(&plist)
        .ok()
        .and_then(|t| plist_string(&t, "CFBundleExecutable"))
        .or_else(|| {
            // Info.plist binaire : plutil le lit.
            let out = Command::new("plutil").args(["-extract", "CFBundleExecutable", "raw", "-o", "-"]).arg(&plist).output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        })
        .map(|name| macos.join(name))
        .filter(|p| p.is_file());
    named.or_else(|| {
        let stem = exe.file_stem()?.to_string_lossy().to_lowercase();
        let files: Vec<PathBuf> = std::fs::read_dir(&macos).ok()?.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect();
        let is_lib = |p: &PathBuf| p.extension().is_some_and(|e| e == "dylib" || e == "so");
        files.iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase() == stem)).or_else(|| files.iter().find(|p| !is_lib(p))).cloned()
    })
}

/// Valeur texte d'une clé dans un Info.plist au format XML.
fn plist_string(text: &str, key: &str) -> Option<String> {
    let after = &text[text.find(&format!("<key>{key}</key>"))? + key.len() + 11..];
    let start = after.find("<string>")? + 8;
    let end = after[start..].find("</string>")?;
    // Rien d'autre qu'un blanc entre la clé et sa valeur.
    after[..start - 8].trim().is_empty().then(|| after[start..start + end].trim().to_string())
}

fn build_command(exe: &Path) -> Command {
    let lower = exe.to_string_lossy().to_lowercase();
    if cfg!(target_os = "macos") && lower.ends_with(".app") {
        // Bundle dont le programme est introuvable : macOS le lance lui-même.
        let mut c = Command::new("open");
        // -W : attendre la fermeture ; -n : nouvelle instance ; --args : la suite va au programme.
        c.arg("-W").arg("-n").arg(exe).arg("--args");
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
    #[cfg(test)]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Drapeau à lever pour fermer le jeu : il est d'abord prié de se fermer
    /// (comme avec la croix de sa fenêtre), puis arrêté s'il ne l'a pas fait.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Pause et reprise du jeu (menu en jeu).
    pub fn control(&self) -> GameControl {
        GameControl {
            #[cfg(windows)]
            job: self.job.clone(),
            #[cfg(unix)]
            pgid: self.child.id(),
            paused: self.paused.clone(),
        }
    }

    /// Attend la fin du jeu et de tous ses processus (bloquant : à appeler
    /// dans `spawn_blocking`). Renvoie le jeu démarré par un launcher, s'il y en a eu un.
    /// `on_window` : appelé une fois, quand une fenêtre du jeu apparaît (Windows ;
    /// ailleurs, jamais).
    pub fn wait(self, on_window: impl FnMut()) -> Option<DirectLaunch> {
        self.wait_outcome(on_window).0
    }

    /// Comme `wait`, et dit si le jeu semble avoir planté : fermé tout seul en moins de
    /// 15 s, ou arrêté par une erreur grave (exception Windows, signal SIGSEGV…).
    pub fn wait_outcome(mut self, mut on_window: impl FnMut()) -> (Option<DirectLaunch>, Option<Crash>) {
        // Surveillance rapprochée tant que la fenêtre du jeu n'est pas là.
        let mut window_seen = !cfg!(windows);
        let mut close_asked: Option<Instant> = None;
        let mut killed = false;
        // Depuis quand la partie n'a plus de fenêtre (après en avoir eu une).
        let mut window_lost: Option<Instant> = None;
        // Arrêtée par le launcher parce que sa fenêtre était fermée : pas un plantage.
        let mut closed_without_window = false;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                match close_asked {
                    None => {
                        tracing::info!("fermeture du jeu demandée");
                        self.ask_close();
                        close_asked = Some(Instant::now());
                    }
                    Some(at) if !killed && at.elapsed() >= CLOSE_GRACE => {
                        tracing::info!("jeu toujours ouvert : arrêt forcé");
                        self.kill();
                        killed = true;
                    }
                    _ => {}
                }
            }
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
            // Programme lancé toujours là, mais plus aucune fenêtre (fermée par le joueur) :
            // certains émulateurs ne se terminent jamais. Pas pendant la pause du menu en jeu,
            // ni quand le programme lancé est déjà fini (un launcher qui passe la main au jeu).
            if cfg!(windows) && window_seen && main_alive && close_asked.is_none() && !killed && !self.paused.load(Ordering::Relaxed) {
                if self.has_window() {
                    window_lost = None;
                } else {
                    let since = *window_lost.get_or_insert_with(Instant::now);
                    if since.elapsed() >= WINDOW_GONE {
                        tracing::info!("plus aucune fenêtre du jeu depuis {} s : jeu fermé, processus arrêtés", WINDOW_GONE.as_secs());
                        self.kill();
                        killed = true;
                        closed_without_window = true;
                    }
                }
            }
            // Attente découpée : une demande de fermeture n'attend pas la fin de l'intervalle.
            let wait = if close_asked.is_some() { STEP } else if window_seen { POLL } else { WINDOW_POLL };
            let until = Instant::now() + wait;
            while Instant::now() < until && (close_asked.is_some() || !self.stop.load(Ordering::Relaxed)) {
                std::thread::sleep(STEP.min(until.saturating_duration_since(Instant::now())));
            }
        }
        let status = self.child.try_wait().ok().flatten();
        tracing::info!("jeu fermé (pid {}, {})", self.child.id(), status.map(|s| s.to_string()).unwrap_or_else(|| "état inconnu".into()));
        let elapsed = self.started.elapsed();
        let quick = elapsed < QUICK_EXIT;
        if quick {
            self.log_output_tail();
        }
        let fatal = status.and_then(fatal_error);
        // Fermé à la demande du launcher : pas un plantage.
        let crash = (close_asked.is_none() && !closed_without_window && (quick || fatal.is_some())).then(|| Crash {
            seconds: elapsed.as_secs(),
            error: fatal.or_else(|| status.and_then(exit_code_error)),
        });
        if let Some(c) = &crash {
            tracing::warn!("le jeu semble avoir planté après {} s ({}) : rapport de bug proposé", c.seconds, c.error.as_deref().unwrap_or("fermé trop vite"));
        }
        let learned = self.watcher.as_ref().and_then(direct::Watcher::learned);
        if let Some(d) = &learned {
            tracing::info!("jeu démarré par le launcher: {} {:?}", d.exe, d.args);
        }
        (learned, crash)
    }

    /// Demande au jeu de se fermer : ses fenêtres reçoivent WM_CLOSE (Windows),
    /// son groupe de processus SIGTERM (Linux).
    fn ask_close(&mut self) {
        // Un jeu suspendu ne lirait pas la demande.
        self.control().resume();
        #[cfg(windows)]
        windows_job::close_windows(&self.job.process_ids());
        #[cfg(unix)]
        signal_group(self.child.id(), "TERM");
    }

    /// Arrête le jeu et tous ses processus.
    fn kill(&mut self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        signal_group(self.child.id(), "KILL");
        let _ = self.child.kill();
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

/// Arrête un programme et tous ceux qu'il a lancés (annulation d'une construction, d'un
/// installateur). Sert à `install/prepare.rs` et `pc/install.rs` (L9).
/// - Linux, Mac : SIGKILL à tout son groupe de processus (il doit avoir été lancé avec
///   `process_group(0)`) ;
/// - Windows : `taskkill /T /F`, qui suit l'arbre des processus (enfants, petits-enfants…).
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    signal_group(child.id(), "KILL");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let r = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Err(e) = r {
            tracing::warn!("taskkill non lancé: {e}");
        }
    }
    // Le programme lui-même, au cas où (déjà fini : sans effet), puis on attend sa fin.
    let _ = child.kill();
    let _ = child.wait();
}

/// Envoie un signal à tout le groupe de processus du jeu.
#[cfg(unix)]
fn signal_group(pgid: u32, signal: &str) {
    let r = Command::new("kill").arg(format!("-{signal}")).arg("--").arg(format!("-{pgid}")).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if let Err(e) = r {
        tracing::warn!("signal {signal} non envoyé au jeu: {e}");
    }
}

/// Un processus non zombie appartient-il encore au groupe `pgid` ?
#[cfg(target_os = "linux")]
pub(crate) fn group_alive(pgid: u32) -> bool {
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
        QueryInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
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

    /// Suspend (ou relance) chacun de ces processus.
    pub fn suspend(pids: &[u32], pause: bool) {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};
        #[link(name = "ntdll")]
        extern "system" {
            fn NtSuspendProcess(process: HANDLE) -> i32;
            fn NtResumeProcess(process: HANDLE) -> i32;
        }
        for &pid in pids {
            // SAFETY : poignée ouverte puis refermée ici ; NtSuspendProcess/NtResumeProcess
            // n'agissent que sur ce processus.
            unsafe {
                let h = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
                if h.is_null() {
                    tracing::warn!("processus {pid} non suspendu: {}", std::io::Error::last_os_error());
                    continue;
                }
                let status = if pause { NtSuspendProcess(h) } else { NtResumeProcess(h) };
                if status < 0 {
                    tracing::warn!("processus {pid}: état {status:#x}");
                }
                CloseHandle(h);
            }
        }
    }

    /// Job Object sans limite : il sert seulement à compter les processus du
    /// jeu. Le fermer ne tue pas le jeu (pas de KILL_ON_JOB_CLOSE).
    pub struct Job(HANDLE);

    // Le handle n'est utilisé que par le thread de surveillance.
    unsafe impl Send for Job {}
    // SAFETY : les fonctions du job utilisées ici peuvent être appelées de plusieurs fils.
    unsafe impl Sync for Job {}

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

    impl Job {
        /// Arrête tous les processus du job.
        pub fn terminate(&self) {
            if unsafe { TerminateJobObject(self.0, 1) } == 0 {
                tracing::warn!("TerminateJobObject: {}", std::io::Error::last_os_error());
            }
        }
    }

    /// Envoie WM_CLOSE aux fenêtres visibles de ces processus (comme la croix de la fenêtre).
    pub fn close_windows(pids: &[u32]) {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE};

        unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
            let pids = unsafe { &*(data as *const Vec<u32>) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            if unsafe { IsWindowVisible(hwnd) } != 0 && pids.contains(&pid) {
                unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
            }
            1
        }
        let pids = pids.to_vec();
        unsafe { EnumWindows(Some(visit), &pids as *const Vec<u32> as LPARAM) };
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
        let learned = RunningGame { child, started: Instant::now(), run_log: None, watcher, stop: Arc::default(), paused: Arc::default(), job: Arc::new(job) }.wait(|| {});
        // ping hors du dossier et trop bref : ce n'est pas un jeu.
        assert_eq!(learned, None);
        assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
    }

    /// Quitter depuis le menu en jeu : un jeu sans fenêtre (ping de 60 s) ne
    /// se ferme pas seul ; il est arrêté, avec ses sous-processus, après le délai de grâce.
    #[cfg(windows)]
    #[test]
    fn stop_flag_ends_the_game() {
        let dir = std::env::temp_dir();
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "ping", "-n", "60", "127.0.0.1", ">", "NUL"]).current_dir(&dir);
        let child = cmd.spawn().unwrap();
        let job = windows_job::Job::create().unwrap();
        job.assign(&child).unwrap();
        let game = RunningGame { child, started: Instant::now(), run_log: None, watcher: None, stop: Arc::default(), paused: Arc::default(), job: Arc::new(job) };
        let stop = game.stop_flag();
        // En pause (menu en jeu) au moment de quitter : relancé pour être fermé.
        let control = game.control();
        control.pause();
        control.pause();
        let started = Instant::now();
        let waiter = std::thread::spawn(move || game.wait(|| {}));
        std::thread::sleep(Duration::from_secs(1));
        stop.store(true, Ordering::Relaxed);
        waiter.join().unwrap();
        let took = started.elapsed();
        assert!(took >= CLOSE_GRACE && took < CLOSE_GRACE + Duration::from_secs(5), "{took:?}");
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

/// Arrêt sur erreur grave : exception Windows (code 0xC0000000 et plus), ou signal de
/// plantage sous Linux et macOS.
/// Arrêt brusque d'un jeu : de quoi proposer et préremplir un rapport de bug.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    /// Durée de la partie, en secondes.
    pub seconds: u64,
    /// Erreur qui a arrêté le jeu (« 0xC0000005 (accès mémoire refusé) »), ou son code de
    /// sortie s'il s'est fermé trop vite ; `None` : fermé trop vite, sans code d'erreur.
    pub error: Option<String>,
}

/// Erreur grave qui a arrêté le jeu : exception Windows, signal sous Linux et macOS.
fn fatal_error(status: std::process::ExitStatus) -> Option<String> {
    #[cfg(windows)]
    {
        status.code().and_then(|c| windows_exception(c as u32))
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().and_then(fatal_signal)
    }
}

/// Code de sortie différent de 0 (jeu fermé trop vite sans erreur grave).
fn exit_code_error(status: std::process::ExitStatus) -> Option<String> {
    status.code().filter(|c| *c != 0).map(|c| format!("code de sortie {c}"))
}

/// Exception Windows (codes 0xC…), avec son nom quand il est connu.
fn windows_exception(code: u32) -> Option<String> {
    if code < 0xC000_0000 {
        return None;
    }
    let name = match code {
        0xC000_0005 => Some("accès mémoire refusé"),
        0xC000_001D => Some("instruction non prise en charge par le processeur"),
        0xC000_0094 => Some("division par zéro"),
        0xC000_00FD => Some("dépassement de pile"),
        0xC000_0135 => Some("DLL introuvable"),
        0xC000_0142 => Some("une DLL n'a pas pu démarrer"),
        0xC000_0374 => Some("mémoire du jeu corrompue"),
        0xC000_0409 => Some("arrêt d'urgence du jeu"),
        0xC06D_007E => Some("module introuvable"),
        _ => None,
    };
    Some(match name {
        Some(n) => format!("0x{code:08X} ({n})"),
        None => format!("0x{code:08X}"),
    })
}

/// Signal qui a arrêté le jeu, s'il signale un plantage.
#[cfg_attr(windows, allow(dead_code))]
fn fatal_signal(signal: i32) -> Option<String> {
    let name = match signal {
        4 => "SIGILL, instruction non prise en charge",
        6 => "SIGABRT, arrêt d'urgence du jeu",
        7 => "SIGBUS, erreur d'accès mémoire",
        8 => "SIGFPE, erreur de calcul",
        11 => "SIGSEGV, accès mémoire refusé",
        _ => return None,
    };
    Some(format!("signal {signal} ({name})"))
}

#[cfg(test)]
mod kill_tests {
    // L9 : annuler arrête aussi les sous-programmes (ici `ping`, lancé par `cmd`).
    #[cfg(windows)]
    #[test]
    fn kill_tree_stops_grandchildren() {
        use sysinfo::{Pid, ProcessesToUpdate, System};
        let mut child = std::process::Command::new("cmd").args(["/c", "ping -n 60 127.0.0.1 > nul"]).spawn().unwrap();
        let parent = Pid::from_u32(child.id());
        let mut sys = System::new();
        let mut grandchild = None;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            sys.refresh_processes(ProcessesToUpdate::All, true);
            grandchild = sys.processes().iter().find(|(_, p)| p.parent() == Some(parent) && p.name().eq_ignore_ascii_case("ping.exe")).map(|(pid, _)| *pid);
            if grandchild.is_some() {
                break;
            }
        }
        let grandchild = grandchild.expect("ping n'a pas démarré");
        super::kill_tree(&mut child);
        std::thread::sleep(std::time::Duration::from_millis(500));
        sys.refresh_processes(ProcessesToUpdate::All, true);
        assert!(sys.process(grandchild).is_none(), "le sous-programme tourne encore");
    }
}

#[cfg(test)]
mod crash_tests {
    use super::*;

    #[test]
    fn names_windows_exceptions() {
        assert_eq!(windows_exception(0xC000_0005).as_deref(), Some("0xC0000005 (accès mémoire refusé)"));
        assert_eq!(windows_exception(0xC123_4567).as_deref(), Some("0xC1234567"));
        // Codes de sortie ordinaires : pas une exception.
        assert_eq!(windows_exception(0), None);
        assert_eq!(windows_exception(1), None);
    }

    #[test]
    fn names_fatal_signals() {
        assert_eq!(fatal_signal(11).as_deref(), Some("signal 11 (SIGSEGV, accès mémoire refusé)"));
        // SIGTERM, SIGKILL : fermeture demandée, pas un plantage.
        assert_eq!(fatal_signal(15), None);
        assert_eq!(fatal_signal(9), None);
    }

    #[test]
    fn exit_code_only_when_not_zero() {
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        let (ok, err) = (std::process::ExitStatus::from_raw(0), std::process::ExitStatus::from_raw(3));
        // Sous Unix, le statut brut porte le code de sortie dans l'octet du haut.
        #[cfg(unix)]
        let (ok, err) = (std::process::ExitStatus::from_raw(0), std::process::ExitStatus::from_raw(3 << 8));
        assert_eq!(exit_code_error(ok), None);
        assert_eq!(exit_code_error(err).as_deref(), Some("code de sortie 3"));
        assert_eq!(fatal_error(err), None);
    }

    #[test]
    fn crash_serializes_for_the_interface() {
        let c = Crash { seconds: 8, error: Some("code de sortie 1".into()) };
        assert_eq!(serde_json::to_value(&c).unwrap(), serde_json::json!({ "seconds": 8, "error": "code de sortie 1" }));
    }
}

#[cfg(test)]
mod plist_tests {
    #[test]
    fn reads_bundle_executable() {
        let t = "<plist><dict>
	<key>CFBundleName</key>
	<string>Melee</string>
	<key>CFBundleExecutable</key>
	<string>melee</string>
</dict></plist>";
        assert_eq!(super::plist_string(t, "CFBundleExecutable").as_deref(), Some("melee"));
        assert_eq!(super::plist_string(t, "CFBundleName").as_deref(), Some("Melee"));
        assert_eq!(super::plist_string(t, "CFBundleIconFile"), None);
    }
}
