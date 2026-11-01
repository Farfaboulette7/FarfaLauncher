//! Jeu lancé par un émulateur : l'émulateur vient de FarfaRomManager, la ROM de RomM.
//!
//! ```text
//! <dossier des jeux>/Emulateurs/<id>/      installé une fois, partagé par ses jeux
//! <dossier des jeux>/Roms/<console>/<jeu>/ la ROM, et ses sauvegardes si l'émulateur les y range
//! ```
//!
//! Les ROM sont hors du dossier de l'émulateur : le retirer ne touche ni à elles,
//! ni aux sauvegardes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::state::{self, DirectLaunch, InstalledGame, RomSource, SaveSync, SharedGame};
use super::{detect, download, ensure_space, extract, make_executable, path_to_rel, rel_to_path, blocking, InstallJob, InstallOutcome};
use crate::error::{AppError, AppResult};
use crate::install::download::disk_error;
use crate::install::extract::INTERNAL_DIR;
use crate::manager::{self, Emulator};
use crate::platform::ArchiveKind;
use crate::romm::library::{self, RemoteFile};
use crate::runtimes;

pub const KIND_EMULATOR: &str = "emulator";

/// Fichier à donner à l'émulateur quand un jeu en compte plusieurs.
fn main_rom(files: &[RemoteFile]) -> Option<&RemoteFile> {
    let ext = |f: &RemoteFile| f.file_name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    ["m3u", "cue", "gdi", "ccd"]
        .iter()
        .find_map(|e| files.iter().find(|f| ext(f) == *e))
        .or_else(|| files.iter().max_by_key(|f| f.size))
}

/// Chemin de la ROM dans son dossier : `ROM/x.gba` → `x.gba` (entrée portage-*).
fn rom_rel(file: &RemoteFile) -> PathBuf {
    let rel = file.rel_path.strip_prefix("ROM/").or_else(|| file.rel_path.strip_prefix("rom/")).unwrap_or(&file.rel_path);
    rel_to_path(rel)
}

/// Nom du dossier d'un jeu : sans l'extension quand l'entrée RomM est un fichier seul.
fn rom_folder<'a>(entry: &'a str, files: &[RemoteFile]) -> &'a str {
    if files.len() == 1 && files[0].file_name == entry {
        stem(entry)
    } else {
        entry
    }
}

fn stem(file_name: &str) -> &str {
    Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name)
}

fn expand(text: &str, rom: &Path, rom_dir: &Path, rom_stem: &str) -> String {
    text.replace("{rom}", &rom.to_string_lossy()).replace("{romdir}", &rom_dir.to_string_lossy()).replace("{romstem}", rom_stem)
}

/// Dossier des sauvegardes d'un jeu : à côté de la ROM, ou relatif à l'exécutable.
fn saves_dir(emulator: &Emulator, rom_dir: &Path, game_dir: &Path, exe: &str) -> Option<PathBuf> {
    match emulator.saves.dir.as_deref()? {
        "{romdir}" => Some(rom_dir.to_path_buf()),
        rel => {
            let exe_dir = game_dir.join(rel_to_path(exe)).parent().map(Path::to_path_buf).unwrap_or_else(|| game_dir.to_path_buf());
            Some(exe_dir.join(rel_to_path(rel)))
        }
    }
}

impl InstallJob {
    /// Installe l'émulateur s'il manque ou n'est plus à jour, puis la ROM de ce jeu.
    pub(super) async fn install_with_emulator(&self, rom_id: i64) -> AppResult<InstallOutcome> {
        let game = library::fetch_game_details(&self.client, rom_id, self.target).await?;
        let emulator = self.catalog.emulator_for_console(&game.console, self.target).cloned().ok_or(AppError::NotAvailable)?;
        if game.rom_files.is_empty() {
            return Err(AppError::NotAvailable);
        }
        let installed = {
            let _lock = self.runtime_lock.lock().await;
            self.ensure_emulator(rom_id, &emulator).await?
        };
        let exe = installed.executable.clone().ok_or(AppError::NoExecutable)?;

        // Dossier des jeux déduit de l'émulateur : il peut avoir été installé ailleurs.
        let root = installed.game_dir.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_else(|| self.install_root.clone());
        let previous = state::find(&self.config_dir, rom_id);
        let rom_dir = previous
            .as_ref()
            .and_then(|p| p.shared.as_ref())
            .and_then(|s| s.rom_dir.clone())
            .unwrap_or_else(|| root.join("Roms").join(state::safe_folder_name(&game.console, rom_id)).join(state::safe_folder_name(rom_folder(&game.folder_name, &game.rom_files), rom_id)));

        let total: u64 = game.rom_files.iter().map(|f| f.size).sum();
        ensure_space(&rom_dir, total)?;
        let mut offset = 0;
        let mut last = Instant::now() - Duration::from_secs(1);
        for file in &game.rom_files {
            let dest = rom_dir.join(rom_rel(file));
            download::download(&self.client, file, &dest, &self.cancel, |got| {
                if last.elapsed() >= Duration::from_millis(200) || got == file.size {
                    last = Instant::now();
                    self.emit(rom_id, "download", offset + got, total);
                }
            }, || self.emit_verifying(rom_id, offset + file.size, total))
            .await?;
            offset += file.size;
        }

        self.emit(rom_id, "finalize", 0, 0);
        let main = main_rom(&game.rom_files).ok_or(AppError::NotAvailable)?;
        let rom_path = rom_dir.join(rom_rel(main));
        let rom_stem = stem(&main.file_name);
        let args = emulator.launch.args.iter().map(|a| expand(a, &rom_path, &rom_dir, rom_stem)).collect();
        let env = emulator.launch.env.iter().map(|(k, v)| (k.clone(), expand(v, &rom_path, &rom_dir, rom_stem))).collect();
        let now = state::now();
        let placed: Vec<String> = game.rom_files.iter().map(|f| path_to_rel(&rom_rel(f))).collect();
        let st = InstalledGame {
            rom_id,
            name: game.name.clone(),
            version: installed.version.clone(),
            version_raw: installed.version_raw.clone(),
            target: installed.target,
            archive_name: installed.archive_name.clone(),
            executable: Some(exe.clone()),
            pending_candidates: Vec::new(),
            roms: placed.clone(),
            rom_sources: game
                .rom_files
                .iter()
                .zip(&placed)
                .map(|(f, p)| RomSource { rel_path: f.rel_path.clone(), size: f.size, sha1: f.sha1.clone(), md5: f.md5.clone(), placed: vec![p.clone()] })
                .collect(),
            mods_downloaded: Vec::new(),
            saves: previous.as_ref().map(|p| p.saves.clone()).unwrap_or_else(SaveSync::default),
            direct_launch: Some(DirectLaunch { exe: exe.clone(), args, cwd: None, env }),
            prefer_launcher: false,
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir: installed.game_dir.clone(),
            pc: None,
            shared: Some(SharedGame {
                runtime_rom_id: installed.rom_id,
                game: emulator.id.clone(),
                runtime_name: emulator.name.clone(),
                data_dir: String::new(),
                save_dir: String::new(),
                cache_dir: None,
                kind: Some(KIND_EMULATOR.to_string()),
                rom_dir: Some(rom_dir.clone()),
                saves_dir: saves_dir(&emulator, &rom_dir, &installed.game_dir, &exe),
                save_patterns: emulator.saves.patterns.iter().map(|p| expand(p, &rom_path, &rom_dir, rom_stem)).collect(),
            }),
        };
        state::write_state(&self.config_dir, &st)?;
        tracing::info!("« {} » installé avec {} dans {}", st.name, emulator.name, rom_dir.display());
        let st = state::find(&self.config_dir, rom_id).ok_or(AppError::Internal)?;
        Ok(InstallOutcome::Installed { game: st })
    }

    /// Émulateur installé et à jour. Sa progression s'affiche sur la page `page_id`.
    async fn ensure_emulator(&self, page_id: i64, emulator: &Emulator) -> AppResult<InstalledGame> {
        let file = emulator.file_for(self.target).ok_or(AppError::NotAvailable)?;
        let runtime_rom_id = manager::runtime_id(&emulator.id);
        let previous = state::find(&self.config_dir, runtime_rom_id);
        if let Some(current) = &previous {
            if current.executable.is_some() && current.version_raw.as_deref() == Some(emulator.version.cle.as_str()) {
                self.fetch_configs(page_id, emulator, current).await?;
                return Ok(current.clone());
            }
        }
        let game_dir = previous
            .as_ref()
            .map(|p| p.game_dir.clone())
            .unwrap_or_else(|| self.install_root.join("Emulateurs").join(state::safe_folder_name(&emulator.id, runtime_rom_id)));
        tracing::info!("installation de l'émulateur {} {} dans {}", emulator.name, emulator.version.nom, game_dir.display());
        let manager_client = self.manager_client()?;
        let internal = game_dir.join(INTERNAL_DIR);
        let archive = internal.join("downloads").join(&file.name);
        ensure_space(&game_dir, file.size * 4)?;
        let remote = file.remote();
        let mut last = Instant::now() - Duration::from_secs(1);
        download::download_from(&manager_client, &file.download_path(), &remote, &archive, &self.cancel, |got| {
            if last.elapsed() >= Duration::from_millis(200) || got == remote.size {
                last = Instant::now();
                self.emit(page_id, "download", got, remote.size);
            }
        }, || self.emit_verifying(page_id, remote.size, remote.size))
        .await?;

        let staging = internal.join("staging");
        {
            let (a, s, gd, cancel, cb) = (archive.clone(), staging.clone(), game_dir.clone(), self.cancel.clone(), self.on_progress.clone());
            blocking(move || -> AppResult<u64> {
                let mut last = Instant::now() - Duration::from_secs(1);
                extract::extract(&a, ArchiveKind::Zip, &s, &cancel, |done, total| {
                    if last.elapsed() >= Duration::from_millis(200) || done == total {
                        last = Instant::now();
                        cb(super::Progress { rom_id: page_id, phase: "extract", done, total, verifying: false });
                    }
                })?;
                let root = extract::content_root(&s)?;
                let n = extract::merge_into(&root, &gd)?;
                let _ = std::fs::remove_dir_all(&s);
                Ok(n)
            })
            .await?;
        }
        let _ = std::fs::remove_dir_all(internal.join("downloads"));
        if self.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        self.emit(page_id, "finalize", 0, 0);
        let candidates: Vec<String> = detect::find_candidates(&game_dir, self.target).iter().map(|p| path_to_rel(p)).collect();
        let hint = emulator.launch.exe.as_deref().unwrap_or(&emulator.id);
        let exe = runtimes::pick_exe(&candidates, hint, Some(&emulator.version.nom)).or_else(|| candidates.first().cloned()).ok_or(AppError::NoExecutable)?;
        let exe_rel = rel_to_path(&exe);
        make_executable(&game_dir.join(&exe_rel));
        let exe_dir = game_dir.join(&exe_rel).parent().map(Path::to_path_buf).unwrap_or_else(|| game_dir.clone());
        for rel in &emulator.launch.portable {
            let marker = exe_dir.join(rel_to_path(rel));
            if !marker.exists() {
                if let Some(parent) = marker.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| disk_error("mode portable", e))?;
                }
                std::fs::write(&marker, b"").map_err(|e| disk_error("mode portable", e))?;
            }
        }
        let now = state::now();
        let st = InstalledGame {
            rom_id: runtime_rom_id,
            name: emulator.name.clone(),
            version: Some(emulator.version.nom.clone()),
            version_raw: Some(emulator.version.cle.clone()),
            target: self.target,
            archive_name: file.name.clone(),
            executable: Some(exe),
            pending_candidates: Vec::new(),
            roms: Vec::new(),
            rom_sources: Vec::new(),
            mods_downloaded: Vec::new(),
            saves: SaveSync::default(),
            direct_launch: None,
            prefer_launcher: false,
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir,
            pc: None,
            shared: None,
        };
        state::write_state(&self.config_dir, &st)?;
        self.fetch_configs(page_id, emulator, &st).await?;
        Ok(st)
    }

    /// Éléments de configuration (prod.keys…), posés à côté de l'exécutable.
    async fn fetch_configs(&self, page_id: i64, emulator: &Emulator, st: &InstalledGame) -> AppResult<()> {
        if emulator.config.is_empty() {
            return Ok(());
        }
        let exe = st.executable.as_deref().ok_or(AppError::NoExecutable)?;
        let exe_dir = st.game_dir.join(rel_to_path(exe)).parent().map(Path::to_path_buf).unwrap_or_else(|| st.game_dir.clone());
        let client = self.manager_client()?;
        for file in &emulator.config {
            let dest = exe_dir.join(rel_to_path(file.dest.as_deref().unwrap_or(&file.name)));
            let remote = file.remote();
            download::download_from(&client, &file.download_path(), &remote, &dest, &self.cancel, |got| self.emit(page_id, "download", got, remote.size), || {}).await?;
        }
        Ok(())
    }

    fn manager_client(&self) -> AppResult<crate::romm::client::RommClient> {
        self.manager_base.as_deref().map(|b| self.client.with_base(b)).ok_or(AppError::NotAvailable)
    }
}

/// Retire la ROM d'un jeu. Ses sauvegardes restent à côté si elles sont conservées.
pub fn remove_rom_files(st: &InstalledGame, shared: &SharedGame, keep_saves: bool) -> AppResult<()> {
    let Some(rom_dir) = shared.rom_dir.as_ref() else { return Ok(()) };
    if !keep_saves {
        if let Err(e) = std::fs::remove_dir_all(rom_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(disk_error("suppression de la ROM", e));
            }
        }
        return Ok(());
    }
    for rom in &st.roms {
        let _ = std::fs::remove_file(rom_dir.join(rel_to_path(rom)));
    }
    // Dossier vide (pas de sauvegarde) : retiré ; sinon il garde les sauvegardes.
    let _ = std::fs::remove_dir(rom_dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, size: u64) -> RemoteFile {
        RemoteFile { id: 1, file_name: name.into(), rel_path: name.into(), size, sha1: None, md5: None, from_manager: false }
    }

    #[test]
    fn picks_the_file_given_to_the_emulator() {
        assert_eq!(main_rom(&[f("a.gba", 1)]).unwrap().file_name, "a.gba");
        assert_eq!(main_rom(&[f("a.bin", 900), f("a.cue", 1)]).unwrap().file_name, "a.cue");
        assert_eq!(main_rom(&[f("d1.cue", 1), f("d2.cue", 1), f("g.m3u", 1)]).unwrap().file_name, "g.m3u");
        assert_eq!(main_rom(&[f("readme.txt", 1), f("g.iso", 9)]).unwrap().file_name, "g.iso");
        assert!(main_rom(&[]).is_none());
    }

    #[test]
    fn rom_paths_lose_the_portage_folder() {
        let mut x = f("x.gba", 1);
        x.rel_path = "ROM/x.gba".into();
        assert_eq!(rom_rel(&x), PathBuf::from("x.gba"));
        x.rel_path = "Disc 1/x.bin".into();
        assert_eq!(rom_rel(&x), PathBuf::from("Disc 1").join("x.bin"));
    }

    #[test]
    fn rom_folders_have_no_extension() {
        assert_eq!(rom_folder("Jeu v1.2 (USA).gba", &[f("Jeu v1.2 (USA).gba", 1)]), "Jeu v1.2 (USA)");
        // Dossier de plusieurs fichiers : son nom est gardé tel quel, même avec un point.
        assert_eq!(rom_folder("Jeu v1.2", &[f("a.bin", 1), f("a.cue", 1)]), "Jeu v1.2");
        assert_eq!(rom_folder("Pokemon Red", &[f("r.gb", 1)]), "Pokemon Red");
    }

    #[test]
    fn expands_launch_values() {
        let rom = Path::new("/r/Mon Jeu.gba");
        assert_eq!(expand("{rom}", rom, Path::new("/r"), "Mon Jeu"), "/r/Mon Jeu.gba");
        assert_eq!(expand("{romstem}.sav", rom, Path::new("/r"), stem("Mon Jeu.gba")), "Mon Jeu.sav");
    }

    #[test]
    fn keeping_saves_leaves_them_next_to_the_rom() {
        let root = std::env::temp_dir().join(format!("farfa-emu-{}", uuid::Uuid::new_v4()));
        let rom_dir = root.join("Roms/gba/Jeu");
        std::fs::create_dir_all(&rom_dir).unwrap();
        std::fs::write(rom_dir.join("Jeu.gba"), b"r").unwrap();
        std::fs::write(rom_dir.join("Jeu.sav"), b"s").unwrap();
        let shared = SharedGame {
            runtime_rom_id: -1, game: "m".into(), runtime_name: "m".into(), data_dir: String::new(), save_dir: String::new(), cache_dir: None,
            kind: Some(KIND_EMULATOR.into()), rom_dir: Some(rom_dir.clone()), saves_dir: Some(rom_dir.clone()), save_patterns: vec!["Jeu.sav".into()],
        };
        let st = InstalledGame {
            rom_id: 1, name: "J".into(), version: None, version_raw: None, target: crate::platform::Target::Windows, archive_name: String::new(),
            executable: None, pending_candidates: vec![], roms: vec!["Jeu.gba".into()], rom_sources: vec![], mods_downloaded: vec![], saves: SaveSync::default(),
            direct_launch: None, prefer_launcher: false, installed_at: 0, updated_at: 0, pc: None, shared: Some(shared.clone()), game_dir: root.clone(),
        };
        remove_rom_files(&st, &shared, true).unwrap();
        assert!(!rom_dir.join("Jeu.gba").exists() && rom_dir.join("Jeu.sav").exists());
        remove_rom_files(&st, &shared, false).unwrap();
        assert!(!rom_dir.exists());
        // Déjà parti : pas une erreur.
        remove_rom_files(&st, &shared, false).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
