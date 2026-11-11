//! Construction du jeu après l'installation de sa recomp (`preparer:` sur FarfaRomManager).
//!
//! Certaines recomps (Hookshot) ne sont qu'un outil qui construit le jeu sur le PC à partir de
//! la copie du joueur. Le launcher le lance sans fenêtre, dans le protocole « RecompLauncher » :
//! des lignes `progress <0..1> <texte>` sur la sortie standard, une ligne `error <texte>` en cas
//! d'échec, et un code de sortie 0 quand le jeu est prêt. La progression s'affiche comme une
//! étape de l'installation (« build »), et Annuler arrête l'outil.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use super::{make_executable, rel_to_path};
use crate::error::{AppError, AppResult};

/// Programme de construction parmi les exécutables du jeu : le premier nom de la liste
/// (`HookshotBuilder, Hookshot-`) qui correspond, au nom exact ou par son début.
pub fn pick_program(candidates: &[String], names: &str) -> Option<String> {
    let file = |c: &str| c.rsplit('/').next().unwrap_or(c).to_lowercase();
    names.split(',').map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).find_map(|n| {
        candidates
            .iter()
            .find(|c| Path::new(&file(c)).file_stem().is_some_and(|s| s.to_string_lossy() == n))
            .or_else(|| candidates.iter().find(|c| file(c).starts_with(&n)))
            .cloned()
    })
}

/// Arguments avec `{rom}` (la copie du jeu) et `{gamedir}` (le dossier du jeu).
pub fn expand(args: &[String], rom: &Path, game_dir: &Path) -> Vec<String> {
    args.iter().map(|a| a.replace("{rom}", &rom.to_string_lossy()).replace("{gamedir}", &game_dir.to_string_lossy())).collect()
}

/// Ligne `progress 0.420 Compiling` → 0.42.
pub fn parse_progress(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("progress ")?;
    rest.split_whitespace().next()?.parse::<f64>().ok().map(|f| f.clamp(0.0, 1.0))
}

/// Lance la construction et attend sa fin. `on_progress` reçoit 0..1000.
pub fn run(game_dir: &Path, program: &str, args: &[String], cancel: &CancellationToken, on_progress: impl Fn(u64) + Send + Sync + 'static) -> AppResult<()> {
    let exe: PathBuf = game_dir.join(rel_to_path(program));
    make_executable(&exe);
    let dir = exe.parent().unwrap_or(game_dir).to_path_buf();
    tracing::info!("construction du jeu : {} {args:?}", exe.display());
    let mut cmd = Command::new(&exe);
    cmd.args(args).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        tracing::error!("construction non lancée : {e}");
        AppError::InstallerFailed
    })?;
    let error: Arc<Mutex<Option<String>>> = Arc::default();
    let out = child.stdout.take().map(|o| {
        let error = error.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(o).lines().map_while(Result::ok) {
                if let Some(f) = parse_progress(&line) {
                    on_progress((f * 1000.0) as u64);
                } else if let Some(e) = line.strip_prefix("error ") {
                    *error.lock().unwrap() = Some(e.to_string());
                }
                tracing::debug!("construction : {line}");
            }
        })
    });
    // La sortie d'erreur est gardée (fin seulement) pour le journal en cas d'échec.
    let err_tail: Arc<Mutex<Vec<String>>> = Arc::default();
    let err = child.stderr.take().map(|e| {
        let tail = err_tail.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                let mut t = tail.lock().unwrap();
                t.push(line);
                if t.len() > 30 {
                    t.remove(0);
                }
            }
        })
    });
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(500)),
            Err(e) => {
                tracing::error!("construction : {e}");
                return Err(AppError::InstallerFailed);
            }
        }
    };
    if let Some(t) = out {
        let _ = t.join();
    }
    if let Some(t) = err {
        let _ = t.join();
    }
    if status.success() {
        tracing::info!("construction du jeu terminée");
        return Ok(());
    }
    tracing::error!(
        "construction échouée ({status}) : {} | {}",
        error.lock().unwrap().clone().unwrap_or_default(),
        err_tail.lock().unwrap().join(" / ")
    );
    Err(AppError::InstallerFailed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_progress_and_arguments() {
        let win = ["HookshotBuilder.exe".to_string(), "jeu/hookshot.exe".into(), "jeu/bin/hookshot.exe".into()];
        assert_eq!(pick_program(&win, "HookshotBuilder, Hookshot-").as_deref(), Some("HookshotBuilder.exe"));
        let linux = ["Hookshot-v2.1.0-x86_64.AppImage".to_string(), "jeu/bin/hookshot".into()];
        assert_eq!(pick_program(&linux, "HookshotBuilder, Hookshot-").as_deref(), Some("Hookshot-v2.1.0-x86_64.AppImage"));
        assert_eq!(parse_progress("progress 0.425 Compiling the game"), Some(0.425));
        assert_eq!(parse_progress("progress 1.000 Done"), Some(1.0));
        assert_eq!(parse_progress("Warning: hard drive"), None);
        let args = expand(&["--game".into(), "{rom}".into(), "--out".into(), "{gamedir}/jeu".into()], Path::new("/j/oot.nup"), Path::new("/j"));
        assert_eq!(args, ["--game", "/j/oot.nup", "--out", "/j/jeu"]);
    }
}
