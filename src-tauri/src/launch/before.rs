//! Programmes lancés avant le jeu, déclarés par l'émulateur (`avant:` sur FarfaRomManager) :
//! par exemple lichii-sync, qui ajoute au cache de Lichii les shaders partagés du jeu.
//!
//! Sans fenêtre, avec une limite de temps : un échec est noté dans les logs, le jeu part quand même.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::manager::BeforeLaunch;

/// Temps laissé à chaque programme : la première synchronisation des shaders de Zelda
/// télécharge 1,2 Go (53 s sur une bonne connexion).
const LIMIT: Duration = Duration::from_secs(15 * 60);

fn this_system() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Programmes de ce système, à lancer depuis le dossier de l'exécutable de l'émulateur.
pub fn applicable(list: &[BeforeLaunch]) -> Vec<BeforeLaunch> {
    list.iter().filter(|b| b.system.as_deref().is_none_or(|s| s.eq_ignore_ascii_case(this_system()))).cloned().collect()
}

/// Efface ces chemins (relatifs au dossier de l'exécutable) avant la partie.
pub fn clean(paths: &[String], exe_dir: &Path) {
    for rel in paths {
        if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            continue;
        }
        let path = exe_dir.join(crate::install::rel_to_path(rel));
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else if path.exists() {
            std::fs::remove_file(&path)
        } else {
            continue;
        };
        match result {
            Ok(()) => tracing::info!("avant le jeu : {} effacé", path.display()),
            Err(e) => tracing::warn!("avant le jeu : {} non effacé: {e}", path.display()),
        }
    }
}

/// Lance chaque programme et attend sa fin (bloquant : à appeler hors du fil principal).
pub fn run_all(list: &[BeforeLaunch], exe_dir: &Path) {
    for b in list {
        let program = exe_dir.join(crate::install::rel_to_path(&b.program));
        if !program.is_file() {
            tracing::warn!("avant le jeu : {} introuvable", program.display());
            continue;
        }
        crate::install::make_executable(&program);
        let dir = exe_dir.to_string_lossy();
        let args: Vec<String> = b.args.iter().map(|a| a.replace("{exedir}", &dir)).collect();
        let mut cmd = Command::new(&program);
        cmd.args(&args).current_dir(exe_dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let started = Instant::now();
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("avant le jeu : {} non lancé: {e}", b.program);
                continue;
            }
        };
        // Sorties lues à part : un programme bavard ne se bloque pas sur un tube plein.
        let out = child.stdout.take().map(|o| std::thread::spawn(move || read_all(o)));
        let err = child.stderr.take().map(|o| std::thread::spawn(move || read_all(o)));
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if started.elapsed() < LIMIT => std::thread::sleep(Duration::from_millis(100)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let text = [out, err].into_iter().flatten().filter_map(|h| h.join().ok()).collect::<Vec<_>>().join("\n");
        let text = text.trim();
        match status {
            Some(s) if s.success() => tracing::info!("avant le jeu : {} fini en {:.1} s\n{text}", b.program, started.elapsed().as_secs_f32()),
            Some(s) => tracing::warn!("avant le jeu : {} a échoué ({s})\n{text}", b.program),
            None => tracing::warn!("avant le jeu : {} arrêté au bout de {} s\n{text}", b.program, LIMIT.as_secs()),
        }
    }
}

fn read_all(mut r: impl std::io::Read) -> String {
    let mut buf = Vec::new();
    let _ = r.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_programs_of_this_system() {
        let b = |s: Option<&str>, p: &str| BeforeLaunch { system: s.map(String::from), program: p.into(), args: vec![] };
        let list = vec![b(Some("windows"), "w.exe"), b(Some("linux"), "l"), b(None, "tous")];
        let names: Vec<String> = applicable(&list).into_iter().map(|b| b.program).collect();
        let mine = if cfg!(windows) { "w.exe" } else { "l" };
        assert_eq!(names, vec![mine.to_string(), "tous".to_string()]);
    }

    #[test]
    fn cleans_only_inside_the_emulator_folder() {
        let dir = std::env::temp_dir().join(format!("farfa-effacer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("user/cache/13.0.4")).unwrap();
        std::fs::write(dir.join("user/cache/13.0.4/share.lut"), b"x").unwrap();
        std::fs::write(dir.join("garde.txt"), b"x").unwrap();
        clean(&["user/cache".into(), "../garde.txt".into(), "absent".into()], &dir);
        assert!(!dir.join("user/cache").exists() && dir.join("user").exists() && dir.join("garde.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn runs_a_program_from_the_emulator_folder() {
        let dir = std::env::temp_dir().join(format!("farfa-avant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(r"C:\Windows\System32\cmd.exe", dir.join("outil.exe")).unwrap();
        let list = vec![BeforeLaunch { system: None, program: "outil.exe".into(), args: vec!["/c".into(), "echo".into(), "{exedir}".into(), ">".into(), "trace.txt".into()] }];
        run_all(&list, &dir);
        let trace = std::fs::read_to_string(dir.join("trace.txt")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(trace.contains(&*dir.to_string_lossy()), "{trace}");
    }
}
