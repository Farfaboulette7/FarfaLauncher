//! Installation (et réinstallation / mise à jour) d'un jeu en un clic :
//! espace disque → téléchargements (archive RECOMP + ROMs) → sauvegarde des
//! saves (si déjà installé) → extraction et fusion sans casse → détection de
//! l'exécutable → copie des ROMs à côté de l'exécutable → droits d'exécution →
//! fichier d'état.

pub mod detect;
pub mod download;
pub mod extract;
pub mod state;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::platform::Target;
use crate::romm::client::RommClient;
use crate::romm::library::{self, Game, RemoteFile};
use download::disk_error;
use extract::INTERNAL_DIR;
use state::InstalledGame;

/// Marge gardée libre sur le disque en plus du nécessaire.
const SPACE_MARGIN: u64 = 200 * 1024 * 1024;
/// Estimation de la taille décompressée avant d'avoir l'archive.
const UNZIP_RATIO_ESTIMATE: f64 = 2.5;
/// Nombre de sauvegardes de saves conservées par jeu.
const BACKUPS_KEPT: usize = 5;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub rom_id: i64,
    /// `prepare` | `download` | `backup` | `extract` | `finalize`
    pub phase: &'static str,
    pub done: u64,
    pub total: u64,
}

pub struct InstallJob {
    pub client: RommClient,
    pub config_dir: PathBuf,
    pub install_root: PathBuf,
    pub target: Target,
    pub cancel: CancellationToken,
    pub on_progress: Arc<dyn Fn(Progress) + Send + Sync>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum InstallOutcome {
    Installed { game: InstalledGame },
    /// Plusieurs exécutables possibles : l'utilisateur doit choisir.
    NeedsChoice { game: InstalledGame },
}

impl InstallJob {
    fn emit(&self, rom_id: i64, phase: &'static str, done: u64, total: u64) {
        (self.on_progress)(Progress { rom_id, phase, done, total });
    }

    pub async fn run(&self, rom_id: i64) -> AppResult<InstallOutcome> {
        self.emit(rom_id, "prepare", 0, 0);
        // Toujours les informations fraîches du serveur (identifiants de fichiers à jour).
        let game = library::fetch_game_details(&self.client, rom_id, self.target).await?;
        let archive = game.my_archive.clone().ok_or(AppError::NotAvailable)?;
        let previous = state::find(&self.config_dir, rom_id);
        let game_dir = match &previous {
            Some(p) => p.game_dir.clone(),
            None => self.install_root.join(state::safe_folder_name(&game.folder_name, rom_id)),
        };
        let internal = game_dir.join(INTERNAL_DIR);
        let downloads = internal.join("downloads");
        tracing::info!(
            "installation de « {} » ({:?}, {}) dans {}",
            game.name,
            archive.target,
            archive.file.file_name,
            game_dir.display()
        );

        // 1. Espace disque (estimation avant téléchargement).
        let rom_bytes: u64 = game.rom_files.iter().map(|f| f.size).sum();
        let estimate = archive.file.size + rom_bytes * 2 + (archive.file.size as f64 * UNZIP_RATIO_ESTIMATE) as u64;
        ensure_space(&game_dir, estimate)?;

        // 2. Téléchargements : archive puis ROMs, progression en octets cumulés.
        let mut files: Vec<(RemoteFile, PathBuf)> = vec![(archive.file.clone(), downloads.join("archive").join(&archive.file.file_name))];
        for f in &game.rom_files {
            files.push((f.clone(), downloads.join(rel_to_path(&f.rel_path))));
        }
        let total: u64 = files.iter().map(|(f, _)| f.size).sum();
        let mut offset = 0u64;
        let mut last = Instant::now() - Duration::from_secs(1);
        for (file, dest) in &files {
            download::download(&self.client, file, dest, &self.cancel, |got| {
                if last.elapsed() >= Duration::from_millis(200) || got == file.size {
                    last = Instant::now();
                    self.emit(rom_id, "download", offset + got, total);
                }
            }, || self.emit(rom_id, "verify", 0, 0))
            .await?;
            offset += file.size;
        }
        let archive_path = files[0].1.clone();

        // 3. Espace exact pour l'extraction.
        let kind = archive.kind;
        let unpacked = {
            let p = archive_path.clone();
            blocking(move || extract::uncompressed_size(&p, kind)).await?
        };
        ensure_space(&game_dir, unpacked + rom_bytes)?;

        // 4. Mise à jour d'un jeu installé : sauvegarde des saves d'abord.
        if previous.is_some() {
            self.emit(rom_id, "backup", 0, 0);
            let gd = game_dir.clone();
            let backup = internal.join("backups").join(state::now().to_string());
            let n = blocking(move || extract::backup_saves(&gd, &backup)).await?;
            tracing::info!("{n} dossier(s) de sauvegarde copiés avant mise à jour");
            prune_backups(&internal.join("backups"));
        }

        // 5. Extraction dans un dossier temporaire puis fusion sans casse.
        let staging = internal.join("staging");
        {
            let (a, s, gd, cancel) = (archive_path.clone(), staging.clone(), game_dir.clone(), self.cancel.clone());
            let cb = self.on_progress.clone();
            let n = blocking(move || -> AppResult<u64> {
                let mut last = Instant::now() - Duration::from_secs(1);
                extract::extract(&a, kind, &s, &cancel, |done, total| {
                    if last.elapsed() >= Duration::from_millis(200) || done == total {
                        last = Instant::now();
                        cb(Progress { rom_id, phase: "extract", done, total });
                    }
                })?;
                let root = extract::content_root(&s)?;
                let n = extract::merge_into(&root, &gd)?;
                let _ = std::fs::remove_dir_all(&s);
                Ok(n)
            })
            .await?;
            tracing::info!("{n} fichier(s) du jeu posés");
        }
        if self.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        // 6. Exécutable : choix précédent conservé s'il existe encore.
        self.emit(rom_id, "finalize", 0, 0);
        let candidates: Vec<String> = detect::find_candidates(&game_dir, self.target).iter().map(|p| path_to_rel(p)).collect();
        tracing::info!("exécutables candidats: {candidates:?}");
        let kept = previous
            .as_ref()
            .and_then(|p| p.executable.clone())
            .filter(|e| game_dir.join(rel_to_path(e)).exists());
        let now = state::now();
        let mut st = InstalledGame {
            rom_id,
            name: game.name.clone(),
            version: game.server_version.clone(),
            version_raw: game.server_version_raw.clone(),
            target: archive.target,
            archive_name: archive.file.file_name.clone(),
            executable: None,
            pending_candidates: Vec::new(),
            roms: previous.as_ref().map(|p| p.roms.clone()).unwrap_or_default(),
            mods_downloaded: previous.as_ref().map(|p| p.mods_downloaded.clone()).unwrap_or_default(),
            saves: previous.as_ref().map(|p| p.saves.clone()).unwrap_or_default(),
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir: game_dir.clone(),
        };
        let chosen = match (kept, candidates.len()) {
            (Some(e), _) => e,
            (None, 0) => return Err(AppError::NoExecutable),
            (None, 1) => candidates[0].clone(),
            (None, _) => {
                st.pending_candidates = candidates;
                state::write_state(&self.config_dir, &st)?;
                return Ok(InstallOutcome::NeedsChoice { game: st });
            }
        };
        let st = finalize(&self.config_dir, st, &chosen, &game).await?;
        Ok(InstallOutcome::Installed { game: st })
    }
}

/// Termine l'installation avec l'exécutable choisi : ROMs, droits, état, nettoyage.
pub async fn finalize(config_dir: &Path, mut st: InstalledGame, executable: &str, game: &Game) -> AppResult<InstalledGame> {
    let game_dir = st.game_dir.clone();
    let exe_rel = rel_to_path(executable);
    if !game_dir.join(&exe_rel).exists() {
        return Err(AppError::NoExecutable);
    }
    let rom_dir_rel = exe_parent(&exe_rel);
    let downloads = game_dir.join(INTERNAL_DIR).join("downloads");
    let rom_files: Vec<RemoteFile> = game.rom_files.clone();
    let st_roms_old = st.roms.clone();
    let gd = game_dir.clone();
    let exe_for_perm = exe_rel.clone();
    let roms = blocking(move || -> AppResult<Vec<String>> {
        let mut placed = Vec::new();
        for f in &rom_files {
            let staged = downloads.join(rel_to_path(&f.rel_path));
            // ROM/sous/dossier/x.z64 → <dossier de l'exe>/sous/dossier/x.z64
            let inner = f.rel_path.strip_prefix("ROM/").or_else(|| f.rel_path.strip_prefix("rom/")).unwrap_or(&f.file_name);
            let dest_rel = rom_dir_rel.join(rel_to_path(inner));
            let dest = gd.join(&dest_rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| disk_error("dossier des ROMs", e))?;
            }
            std::fs::copy(&staged, &dest).map_err(|e| disk_error("copie d'une ROM", e))?;
            placed.push(path_to_rel(&dest_rel));
        }
        // ROMs d'une installation précédente restées à un autre endroit : conservées
        // (on ne supprime jamais de fichiers du jeu).
        let _ = st_roms_old;
        make_executable(&gd.join(&exe_for_perm));
        remove_quarantine(&gd);
        let _ = std::fs::remove_dir_all(&downloads);
        Ok(placed)
    })
    .await?;
    st.roms = roms;
    st.executable = Some(path_to_rel(&exe_rel));
    st.pending_candidates.clear();
    state::write_state(config_dir, &st)?;
    tracing::info!("« {} » installé, exécutable {}", st.name, executable);
    Ok(st)
}

/// Change l'exécutable d'un jeu déjà installé et recopie les ROMs à côté.
pub async fn change_executable(config_dir: &Path, mut st: InstalledGame, executable: &str) -> AppResult<InstalledGame> {
    let exe_rel = rel_to_path(executable);
    let gd = st.game_dir.clone();
    if !gd.join(&exe_rel).exists() {
        return Err(AppError::NoExecutable);
    }
    let new_dir = exe_parent(&exe_rel);
    let old_roms = st.roms.clone();
    let exe_for_perm = exe_rel.clone();
    let roms = blocking(move || -> AppResult<Vec<String>> {
        let mut placed = Vec::new();
        for old in &old_roms {
            let old_rel = rel_to_path(old);
            let name = old_rel.file_name().map(PathBuf::from).unwrap_or_default();
            let dest_rel = new_dir.join(name);
            if dest_rel != old_rel && gd.join(&old_rel).exists() {
                std::fs::copy(gd.join(&old_rel), gd.join(&dest_rel)).map_err(|e| disk_error("copie d'une ROM", e))?;
            }
            placed.push(path_to_rel(&dest_rel));
        }
        make_executable(&gd.join(&exe_for_perm));
        Ok(placed)
    })
    .await?;
    st.roms = roms;
    st.executable = Some(path_to_rel(&exe_rel));
    st.pending_candidates.clear();
    state::write_state(config_dir, &st)?;
    Ok(st)
}

/// Désinstalle un jeu ; avec `keep_saves`, les dossiers save/saves restent en place.
pub async fn uninstall(config_dir: &Path, st: InstalledGame, keep_saves: bool) -> AppResult<()> {
    let gd = st.game_dir.clone();
    blocking(move || -> AppResult<()> {
        if !gd.exists() {
            return Ok(());
        }
        if keep_saves {
            remove_all_but_saves(&gd, &gd)?;
        } else {
            std::fs::remove_dir_all(&gd).map_err(|e| log_err(AppError::Disk, "suppression du jeu", e))?;
        }
        Ok(())
    })
    .await?;
    state::forget(config_dir, st.rom_id)?;
    tracing::info!("« {} » désinstallé (saves conservées: {keep_saves})", st.name);
    Ok(())
}

/// Dossier des jeux par défaut : `Jeux` à côté du logiciel installé, s'il est
/// accessible en écriture ; sinon `~/FarfaLauncher/Jeux` (et toujours en
/// développement, pour ne pas installer dans `target/`).
pub fn default_root(home: &Path) -> PathBuf {
    let fallback = home.join("FarfaLauncher").join("Jeux");
    if cfg!(debug_assertions) {
        return fallback;
    }
    match app_location() {
        Some(dir) if is_writable(&dir) => dir.join("Jeux"),
        Some(dir) => {
            tracing::info!("{} n'est pas modifiable : jeux dans {}", dir.display(), fallback.display());
            fallback
        }
        None => fallback,
    }
}

/// Dossier où le logiciel est installé.
fn app_location() -> Option<PathBuf> {
    // AppImage : l'exécutable tourne depuis un montage temporaire en lecture seule.
    if let Some(p) = std::env::var_os("APPIMAGE") {
        return PathBuf::from(p).parent().map(Path::to_path_buf);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        // …/FarfaLauncher.app/Contents/MacOS → dossier qui contient le .app
        return dir
            .ancestors()
            .find(|a| a.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
            .and_then(Path::parent)
            .map(Path::to_path_buf);
    }
    Some(dir.to_path_buf())
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".farfa-ecriture-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Déplace un jeu installé dans `dest_parent` (son dossier garde son nom).
/// Renommage direct sur le même disque, sinon copie puis suppression.
pub async fn move_game(config_dir: &Path, mut st: InstalledGame, dest_parent: PathBuf) -> AppResult<InstalledGame> {
    let old = st.game_dir.clone();
    let name = old.file_name().ok_or(AppError::Internal)?.to_owned();
    let target = dest_parent.join(&name);
    if target == old {
        return Ok(st);
    }
    if target.starts_with(&old) {
        return Err(AppError::MoveIntoItself);
    }
    if target.exists() {
        return Err(AppError::DestinationExists);
    }
    let (from, to) = (old.clone(), target.clone());
    blocking(move || -> AppResult<()> {
        std::fs::create_dir_all(&dest_parent).map_err(|e| disk_error("création du dossier de destination", e))?;
        match std::fs::rename(&from, &to) {
            Ok(()) => return Ok(()),
            Err(e) if !crosses_devices(&e) => return Err(disk_error("déplacement du jeu", e)),
            Err(_) => {}
        }
        // Autre disque : copie complète, puis suppression de l'original.
        ensure_space(&to, dir_size(&from))?;
        if let Err(e) = copy_dir(&from, &to) {
            let _ = std::fs::remove_dir_all(&to);
            return Err(e);
        }
        if let Err(e) = std::fs::remove_dir_all(&from) {
            tracing::warn!("copie réussie mais l'ancien dossier {} n'a pas pu être supprimé: {e}", from.display());
        }
        Ok(())
    })
    .await?;
    // Sauvegardes rangées dans le dossier du jeu : elles ont suivi.
    if let Some(rest) = st.saves.dir.as_ref().and_then(|d| d.strip_prefix(&old).ok()).map(Path::to_path_buf) {
        st.saves.dir = Some(target.join(rest));
    }
    st.game_dir = target;
    state::write_state(config_dir, &st)?;
    tracing::info!("« {} » déplacé de {} vers {}", st.name, old.display(), st.game_dir.display());
    Ok(st)
}

fn crosses_devices(e: &std::io::Error) -> bool {
    // Windows : ERROR_NOT_SAME_DEVICE (17) ; Unix : EXDEV (18).
    e.kind() == std::io::ErrorKind::CrossesDevices || (cfg!(windows) && e.raw_os_error() == Some(17)) || (cfg!(unix) && e.raw_os_error() == Some(18))
}

pub fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.filter_map(Result::ok)
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn copy_dir(from: &Path, to: &Path) -> AppResult<()> {
    std::fs::create_dir_all(to).map_err(|e| disk_error("création d'un dossier", e))?;
    for entry in std::fs::read_dir(from).map_err(|e| disk_error("lecture du dossier du jeu", e))?.filter_map(Result::ok) {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| disk_error("copie d'un fichier", e))?;
        }
    }
    Ok(())
}

/// Supprime tout sauf les dossiers save/saves (et leurs dossiers parents).
fn remove_all_but_saves(root: &Path, dir: &Path) -> AppResult<bool> {
    let mut kept_any = false;
    for entry in std::fs::read_dir(dir).map_err(|e| log_err(AppError::Disk, "lecture du dossier du jeu", e))?.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && (name == "save" || name == "saves") {
            kept_any = true;
        } else if is_dir && name != INTERNAL_DIR {
            // Le sous-dossier se supprime lui-même s'il ne garde rien.
            kept_any |= remove_all_but_saves(root, &path)?;
        } else if is_dir {
            std::fs::remove_dir_all(&path).map_err(|e| log_err(AppError::Disk, "suppression d'un dossier", e))?;
        } else {
            std::fs::remove_file(&path).map_err(|e| log_err(AppError::Disk, "suppression d'un fichier", e))?;
        }
    }
    if !kept_any && dir != root {
        std::fs::remove_dir(dir).map_err(|e| log_err(AppError::Disk, "suppression d'un dossier", e))?;
    }
    Ok(kept_any)
}

fn exe_parent(exe_rel: &Path) -> PathBuf {
    // Pour un bundle `.app`, la « position » de l'exécutable est celle du bundle.
    exe_rel.parent().map(Path::to_path_buf).unwrap_or_default()
}

pub fn rel_to_path(rel: &str) -> PathBuf {
    rel.split('/').filter(|s| !s.is_empty() && *s != "..").collect()
}

pub fn path_to_rel(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn ensure_space(dir: &Path, needed: u64) -> AppResult<()> {
    let probe = dir.ancestors().find(|p| p.exists()).unwrap_or(dir);
    let available = match fs4::available_space(probe) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("espace disque inconnu pour {}: {e}", probe.display());
            return Ok(());
        }
    };
    if available < needed + SPACE_MARGIN {
        return Err(AppError::NotEnoughSpace { needed: human(needed + SPACE_MARGIN), available: human(available) });
    }
    Ok(())
}

fn human(bytes: u64) -> String {
    let gb = bytes as f64 / 1024f64.powi(3);
    if gb >= 1.0 {
        format!("{gb:.1} Go").replace('.', ",")
    } else {
        format!("{} Mo", bytes / (1024 * 1024))
    }
}

fn prune_backups(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    while entries.len() > BACKUPS_KEPT {
        let old = entries.remove(0);
        let _ = std::fs::remove_dir_all(old);
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let set = |p: &Path| {
        if let Ok(m) = std::fs::metadata(p) {
            let mut perm = m.permissions();
            perm.set_mode(perm.mode() | 0o755);
            let _ = std::fs::set_permissions(p, perm);
        }
    };
    if path.is_dir() {
        // Bundle .app : binaires dans Contents/MacOS.
        if let Ok(rd) = std::fs::read_dir(path.join("Contents").join("MacOS")) {
            for e in rd.filter_map(Result::ok) {
                set(&e.path());
            }
        }
    } else {
        set(path);
    }
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn remove_quarantine(game_dir: &Path) {
    if cfg!(target_os = "macos") {
        match std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(game_dir).status() {
            Ok(s) if s.success() => tracing::info!("attribut de quarantaine retiré"),
            Ok(s) => tracing::warn!("xattr a échoué ({s})"),
            Err(e) => tracing::warn!("xattr introuvable: {e}"),
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| log_err(AppError::Internal, "tâche interrompue", e))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn move_keeps_registry_and_saves() {
        let base = std::env::temp_dir().join(format!("farfa-move-{}", uuid::Uuid::new_v4()));
        let (config, old) = (base.join("config"), base.join("a").join("Zelda"));
        std::fs::create_dir_all(old.join("saves")).unwrap();
        std::fs::write(old.join("saves/slot.sav"), b"x").unwrap();
        let st = InstalledGame {
            rom_id: 7,
            name: "Zelda".into(),
            version: None,
            version_raw: None,
            target: Target::Windows,
            archive_name: "z.zip".into(),
            executable: Some("z.exe".into()),
            pending_candidates: vec![],
            roms: vec![],
            mods_downloaded: vec![],
            saves: state::SaveSync { dir: Some(old.join("saves")), ..Default::default() },
            installed_at: 0,
            updated_at: 0,
            game_dir: old.clone(),
        };
        state::write_state(&config, &st).unwrap();

        let moved = move_game(&config, st, base.join("b")).await.unwrap();
        let new = base.join("b").join("Zelda");
        assert_eq!(moved.game_dir, new);
        assert_eq!(moved.saves.dir.as_deref(), Some(new.join("saves").as_path()));
        assert!(new.join("saves/slot.sav").exists() && !old.exists());
        assert_eq!(state::find(&config, 7).unwrap().game_dir, new);
        // Déjà occupé / dans son propre dossier : refusé.
        std::fs::create_dir_all(base.join("c").join("Zelda")).unwrap();
        assert!(matches!(move_game(&config, moved.clone(), base.join("c")).await, Err(AppError::DestinationExists)));
        assert!(matches!(move_game(&config, moved, new.join("sub")).await, Err(AppError::MoveIntoItself)));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn uninstall_keeps_only_saves() {
        let root = std::env::temp_dir().join(format!("farfa-uninstall-{}", uuid::Uuid::new_v4()));
        for dir in ["res/rml", "sdk", "data/saves", INTERNAL_DIR] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in ["game.exe", "res/rml/a.rcss", "sdk/x.lib", "data/saves/slot1.sav", "data/config.ini"] {
            std::fs::write(root.join(file), b"x").unwrap();
        }
        assert!(remove_all_but_saves(&root, &root).unwrap());
        assert!(root.join("data/saves/slot1.sav").exists());
        for gone in ["game.exe", "res", "sdk", "data/config.ini", INTERNAL_DIR] {
            assert!(!root.join(gone).exists(), "{gone} aurait dû être supprimé");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
