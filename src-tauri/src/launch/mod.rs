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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{log_err, AppError, AppResult};

/// Intervalle de surveillance des processus.
const POLL: Duration = Duration::from_secs(2);
/// Jeu fermé plus vite que ça : sa sortie (erreurs) est recopiée dans nos logs.
const QUICK_EXIT: Duration = Duration::from_secs(15);
/// Sortie du dernier lancement, dans le dossier interne du jeu.
const RUN_LOG: &str = "dernier-lancement.log";

/// Jeu lancé, à surveiller jusqu'à sa fermeture.
pub struct RunningGame {
    child: Child,
    started: Instant,
    run_log: Option<PathBuf>,
    #[cfg(windows)]
    job: windows_job::Job,
}

/// Lance `exe` (relatif à `game_dir`) dans son propre dossier.
pub fn spawn(game_dir: &Path, exe_rel: &Path) -> AppResult<RunningGame> {
    let exe = game_dir.join(exe_rel);
    if !exe.exists() {
        return Err(AppError::NoExecutable);
    }
    let work_dir = exe.parent().unwrap_or(game_dir).to_path_buf();
    let mut cmd = build_command(&exe);
    cmd.current_dir(&work_dir);
    crate::external::use_system_env(&mut cmd);
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
    tracing::info!("jeu lancé: {} (pid {})", exe.display(), child.id());

    #[cfg(windows)]
    {
        let job = windows_job::Job::create()?;
        if let Err(e) = job.assign(&child) {
            // Sans job, on suit au moins le processus principal.
            tracing::warn!("processus non rattaché au job: {e}");
        }
        Ok(RunningGame { child, started: Instant::now(), run_log, job })
    }
    #[cfg(not(windows))]
    Ok(RunningGame { child, started: Instant::now(), run_log })
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
    /// dans `spawn_blocking`).
    pub fn wait(mut self) {
        loop {
            // Récolte le processus principal s'il est terminé (évite un zombie).
            let main_alive = matches!(self.child.try_wait(), Ok(None));
            if !main_alive && !self.descendants_alive() {
                break;
            }
            std::thread::sleep(POLL);
        }
        let status = self.child.try_wait().ok().flatten();
        tracing::info!("jeu fermé (pid {}, {})", self.child.id(), status.map(|s| s.to_string()).unwrap_or_else(|| "état inconnu".into()));
        if self.started.elapsed() < QUICK_EXIT {
            self.log_output_tail();
        }
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
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, QueryInformationJobObject,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    };

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
        RunningGame { child, started: Instant::now(), run_log: None, job }.wait();
        assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
    }
}
