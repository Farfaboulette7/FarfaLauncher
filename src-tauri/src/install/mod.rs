//! Installation (et réinstallation / mise à jour) d'un jeu en un clic :
//! espace disque → téléchargements (archive RECOMP + ROMs) → sauvegarde des
//! saves (si déjà installé) → extraction et fusion sans casse → détection de
//! l'exécutable → copie des ROMs à côté de l'exécutable → droits d'exécution →
//! fichier d'état.

pub mod detect;
pub mod download;
pub mod emulator;
pub mod extract;
pub mod state;
#[cfg(test)]
mod mac_e2e;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::manager::Catalog;
use crate::platform::{ArchiveKind, Target};
use crate::romm::client::RommClient;
use crate::romm::library::{Game, RemoteFile};
use crate::runtimes::{self, Manifest, Runtime, RuntimeGame};
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
    /// Téléchargement : fichier reçu en cours de vérification (la progression reste affichée).
    pub verifying: bool,
}

pub struct InstallJob {
    pub client: RommClient,
    pub config_dir: PathBuf,
    pub install_root: PathBuf,
    pub target: Target,
    pub cancel: CancellationToken,
    /// Version de Proton choisie pour un jeu Windows sous Linux (`None` : la recommandée).
    pub proton: Option<String>,
    pub manifest: Manifest,
    pub catalog: Catalog,
    /// Adresse de FarfaRomManager (`None` : pas d'émulateur).
    pub manager_base: Option<String>,
    /// Installer ce jeu avec l'émulateur de sa console plutôt que sa recomp.
    pub with_emulator: bool,
    /// Émulateur choisi parmi ceux de la console (`None` : celui déjà utilisé par le jeu, sinon le premier).
    pub emulator_id: Option<String>,
    pub runtime_lock: Arc<tokio::sync::Mutex<()>>,
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
    pub(crate) fn emit(&self, rom_id: i64, phase: &'static str, done: u64, total: u64) {
        (self.on_progress)(Progress { rom_id, phase, done, total, verifying: false });
    }

    /// Fichier téléchargé en cours de vérification : on reste en phase de téléchargement.
    pub(crate) fn emit_verifying(&self, rom_id: i64, done: u64, total: u64) {
        (self.on_progress)(Progress { rom_id, phase: "download", done, total, verifying: true });
    }

    pub async fn run(&self, rom_id: i64) -> AppResult<InstallOutcome> {
        self.emit(rom_id, "prepare", 0, 0);
        if self.with_emulator {
            return self.install_with_emulator(rom_id).await;
        }
        if let Some((runtime, game)) = self.manifest.page(rom_id) {
            return self.install_page(rom_id, runtime, game).await;
        }
        // Toujours les informations fraîches du serveur (identifiants de fichiers à jour).
        let game = runtimes::fetch_details(&self.client, &self.manifest, &self.catalog, rom_id, self.target).await?;
        if let Some(pc) = game.pc.clone() {
            return crate::pc::install::run(self, game, pc).await;
        }
        self.install_recomp(rom_id, game, None).await
    }

    /// Jeu d'un runtime partagé : installe le runtime s'il manque ou n'est plus
    /// à jour, puis ajoute seulement la ROM de ce jeu.
    async fn install_page(&self, rom_id: i64, runtime: &Runtime, def: &RuntimeGame) -> AppResult<InstallOutcome> {
        let page = runtimes::fetch_page(&self.client, &self.manifest, rom_id, self.target).await?;
        let rom = page.rom_files.first().cloned().ok_or(AppError::NotAvailable)?;
        let installed = {
            let _lock = self.runtime_lock.lock().await;
            self.ensure_runtime(rom_id, runtime).await?
        };
        let exe = installed.executable.clone().ok_or(AppError::NoExecutable)?;

        // La ROM reste sur le disque : la recomp en a besoin à chaque nouvelle version.
        let game_dir = installed.game_dir.clone();
        let rom_rel = format!("{INTERNAL_DIR}/roms/{}", rom.file_name);
        ensure_space(&game_dir, rom.size)?;
        let mut last = Instant::now() - Duration::from_secs(1);
        download::download(&self.client, &rom, &game_dir.join(rel_to_path(&rom_rel)), &self.cancel, |got| {
            if last.elapsed() >= Duration::from_millis(200) || got == rom.size {
                last = Instant::now();
                self.emit(rom_id, "download", got, rom.size);
            }
        }, || self.emit_verifying(rom_id, rom.size, rom.size))
        .await?;

        self.emit(rom_id, "finalize", 0, 0);
        let sep = std::path::MAIN_SEPARATOR;
        let rom_arg = format!("{}{sep}{}", crate::launch::direct::GAME_DIR_VAR, path_to_rel(&rel_to_path(&rom_rel)).replace('/', &sep.to_string()));
        let (args, env) = runtime.launch_for(&def.game, &rom_arg);
        let previous = state::find(&self.config_dir, rom_id);
        let now = state::now();
        let st = InstalledGame {
            rom_id,
            name: page.name.clone(),
            version: installed.version.clone(),
            version_raw: installed.version_raw.clone(),
            target: installed.target,
            archive_name: installed.archive_name.clone(),
            executable: Some(exe.clone()),
            pending_candidates: Vec::new(),
            roms: vec![rom_rel.clone()],
            rom_sources: vec![state::RomSource { rel_path: rom.rel_path.clone(), size: rom.size, sha1: rom.sha1.clone(), md5: rom.md5.clone(), placed: vec![rom_rel] }],
            mods_downloaded: Vec::new(),
            saves: previous.as_ref().map(|p| p.saves.clone()).unwrap_or_default(),
            direct_launch: Some(state::DirectLaunch { exe, args, cwd: None, env }),
            prefer_launcher: previous.as_ref().is_some_and(|p| p.prefer_launcher),
            players: previous.as_ref().map(|p| p.players.clone()).unwrap_or_default(),
            installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir,
            pc: None,
            shared: Some(state::SharedGame {
                runtime_rom_id: runtime.rom_id,
                game: def.game.clone(),
                runtime_name: runtime.name.clone(),
                data_dir: runtime.data_dir.clone(),
                save_dir: runtime.save_dir.replace("{game}", &def.game),
                cache_dir: runtime.cache_dir.as_ref().map(|d| d.replace("{game}", &def.game)),
                kind: None,
                rom_dir: None,
                saves_dir: None,
                save_patterns: Vec::new(),
            }),
        };
        state::write_state(&self.config_dir, &st)?;
        tracing::info!("« {} » installé sur le runtime {}", st.name, runtime.id);
        // Relu pour avoir les informations venues du runtime et le dossier des sauvegardes.
        let st = state::find(&self.config_dir, rom_id).ok_or(AppError::Internal)?;
        Ok(InstallOutcome::Installed { game: st })
    }

    /// Runtime installé et à jour. Sa progression s'affiche sur la page `page_id`.
    async fn ensure_runtime(&self, page_id: i64, runtime: &Runtime) -> AppResult<InstalledGame> {
        // Sans ses ROMs : chaque jeu apporte la sienne.
        let mut game = runtimes::fetch_runtime(&self.client, &self.catalog, runtime, self.target).await?;
        game.rom_files.clear();
        if let Some(current) = state::find(&self.config_dir, runtime.rom_id) {
            let up_to_date = game.server_version_raw.is_none() || game.server_version_raw == current.version_raw;
            if current.executable.is_some() && up_to_date {
                tracing::info!("runtime {} déjà installé et à jour", runtime.id);
                return Ok(current);
            }
        }
        tracing::info!("installation du runtime {}", runtime.id);
        let forward = self.on_progress.clone();
        let job = InstallJob {
            client: self.client.clone(),
            config_dir: self.config_dir.clone(),
            install_root: self.install_root.clone(),
            target: self.target,
            cancel: self.cancel.clone(),
            proton: None,
            manifest: self.manifest.clone(),
            catalog: self.catalog.clone(),
            manager_base: self.manager_base.clone(),
            with_emulator: false,
            emulator_id: None,
            runtime_lock: self.runtime_lock.clone(),
            on_progress: Arc::new(move |p| forward(Progress { rom_id: page_id, ..p })),
        };
        match job.install_recomp(runtime.rom_id, game, Some(runtime)).await? {
            InstallOutcome::Installed { game } => Ok(game),
            InstallOutcome::NeedsChoice { .. } => Err(AppError::NoExecutable),
        }
    }

    /// Recomp : archive de la plateforme, ROMs à côté de l'exécutable, état.
    /// `runtime` : l'exécutable est celui du manifeste, sans question.
    async fn install_recomp(&self, rom_id: i64, game: Game, runtime: Option<&Runtime>) -> AppResult<InstallOutcome> {
        let archive = game.my_archive.clone().ok_or(AppError::NotAvailable)?;
        let previous = state::find(&self.config_dir, rom_id);
        // Jeu joué jusqu'ici avec un émulateur : la recomp prend son propre dossier, et la ROM
        // déjà sur le disque est reprise du dossier de l'émulateur plutôt que retéléchargée.
        let (previous, from_emulator) = match previous {
            Some(p) if p.shared.as_ref().is_some_and(|s| s.is_emulator()) => (None, Some(p)),
            p => (p, None),
        };
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

        // 1. Espace disque (estimation avant téléchargement). Les ROMs déjà
        //    posées et inchangées sur le serveur ne sont pas retéléchargées.
        let old_sources = previous.as_ref().map(|p| p.rom_sources.clone()).unwrap_or_default();
        let reused = match &from_emulator {
            Some(emu) => {
                let (emu, files, dl) = (emu.clone(), game.rom_files.clone(), downloads.clone());
                blocking(move || Ok(reuse_emulator_roms(&emu, &files, &dl))).await?
            }
            None => Vec::new(),
        };
        let rom_fetch: Vec<&RemoteFile> = game
            .rom_files
            .iter()
            .filter(|f| unchanged_rom(&old_sources, f, &game_dir).is_none() && !reused.iter().any(|(_, rel)| *rel == f.rel_path))
            .collect();
        if rom_fetch.len() < game.rom_files.len() {
            tracing::info!("{} ROM(s) inchangée(s), non retéléchargée(s)", game.rom_files.len() - rom_fetch.len());
        }
        let rom_bytes: u64 = rom_fetch.iter().map(|f| f.size).sum();
        let estimate = archive.file.size + rom_bytes * 2 + (archive.file.size as f64 * UNZIP_RATIO_ESTIMATE) as u64;
        ensure_space(&game_dir, estimate)?;

        // 2. Téléchargements : archive puis ROMs, progression en octets cumulés.
        let mut files: Vec<(RemoteFile, PathBuf)> = vec![(archive.file.clone(), downloads.join("archive").join(&archive.file.file_name))];
        for f in rom_fetch {
            files.push((f.clone(), downloads.join(rel_to_path(&f.rel_path))));
        }
        let total: u64 = files.iter().map(|(f, _)| f.size).sum();
        let mut offset = 0u64;
        let mut last = Instant::now() - Duration::from_secs(1);
        for (file, dest) in &files {
            download::download_any(&self.client, self.manager_base.as_deref(), file, dest, &self.cancel, |got| {
                if last.elapsed() >= Duration::from_millis(200) || got == file.size {
                    last = Instant::now();
                    self.emit(rom_id, "download", offset + got, total);
                }
            }, || self.emit_verifying(rom_id, offset + file.size, total))
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
                        cb(Progress { rom_id, phase: "extract", done, total, verifying: false });
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
            rom_sources: old_sources,
            mods_downloaded: previous.as_ref().map(|p| p.mods_downloaded.clone()).unwrap_or_default(),
            // Sauvegardes de l'émulateur à part (sur RomM) : la recomp cherche les siennes.
            saves: previous.as_ref().map(|p| p.saves.clone()).unwrap_or_default(),
            direct_launch: None,
            prefer_launcher: previous.as_ref().is_some_and(|p| p.prefer_launcher),
            players: previous.as_ref().or(from_emulator.as_ref()).map(|p| p.players.clone()).unwrap_or_default(),
            installed_at: previous.as_ref().or(from_emulator.as_ref()).map(|p| p.installed_at).unwrap_or(now),
            updated_at: now,
            game_dir: game_dir.clone(),
            pc: None,
            shared: None,
        };
        let kept = match runtime {
            Some(r) => runtimes::pick_exe(&candidates, &r.exe, game.server_version.as_deref()).or_else(|| {
                tracing::warn!("exécutable {} absent des candidats", r.exe);
                candidates.first().cloned()
            }),
            // `exe:` du gestionnaire : programme imposé (Paper Cut : ttyd-eu, pas son menu).
            None => match self.catalog.recomp_exe(rom_id) {
                Some(hint) => runtimes::pick_exe(&candidates, hint, None).or(kept),
                None => kept,
            },
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
        // La ROM est maintenant à côté de la recomp : la copie de l'émulateur est retirée.
        remove_reused(&reused);
        Ok(InstallOutcome::Installed { game: st })
    }
}

/// Même fichier : même taille, et même SHA-1 s'il est connu des deux côtés (sinon même chemin).
pub(crate) fn same_rom(s: &state::RomSource, f: &RemoteFile) -> bool {
    s.size == f.size
        && match (&s.sha1, &f.sha1) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => s.rel_path == f.rel_path,
        }
}

/// Reprend un fichier déjà sur le disque : lien physique (même disque, rien à copier), sinon copie.
pub(crate) fn reuse_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(to);
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

/// Pose un fichier téléchargé : déplacé (même disque), sinon copié.
fn place_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

/// ROMs du jeu installé avec un émulateur reprises dans les téléchargements de la recomp :
/// (ancien fichier, `rel_path` serveur).
fn reuse_emulator_roms(emu: &InstalledGame, files: &[RemoteFile], downloads: &Path) -> Vec<(PathBuf, String)> {
    let Some(rom_dir) = emu.shared.as_ref().and_then(|s| s.rom_dir.clone()) else { return Vec::new() };
    let mut reused = Vec::new();
    for f in files {
        let Some(src) = emu.rom_sources.iter().find(|s| same_rom(s, f) && s.placed.len() == 1) else { continue };
        let old = rom_dir.join(rel_to_path(&src.placed[0]));
        if !std::fs::metadata(&old).is_ok_and(|m| m.len() == f.size) {
            continue;
        }
        match reuse_file(&old, &downloads.join(rel_to_path(&f.rel_path))) {
            Ok(()) => {
                tracing::info!("ROM reprise de l'émulateur : {}", old.display());
                reused.push((old, f.rel_path.clone()));
            }
            Err(e) => tracing::warn!("ROM de l'émulateur non reprise ({}): {e}", old.display()),
        }
    }
    reused
}

/// Retire les anciennes copies des ROMs reprises (leur contenu est à sa nouvelle place).
pub(crate) fn remove_reused(reused: &[(PathBuf, String)]) {
    for (old, _) in reused {
        if let Err(e) = std::fs::remove_file(old) {
            tracing::warn!("ancienne ROM non retirée ({}): {e}", old.display());
        }
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
    let old_sources = st.rom_sources.clone();
    let gd = game_dir.clone();
    let exe_for_perm = exe_rel.clone();
    let (roms, sources) = blocking(move || -> AppResult<(Vec<String>, Vec<state::RomSource>)> {
        let mut placed = Vec::new();
        let mut sources = Vec::new();
        for f in &rom_files {
            let staged = downloads.join(rel_to_path(&f.rel_path));
            if !staged.exists() {
                // Pas retéléchargée car inchangée : ce qui est en place est gardé.
                match unchanged_rom(&old_sources, f, &gd) {
                    Some(s) => {
                        if !is_extract_archive(&f.file_name) {
                            placed.extend(s.placed.iter().cloned());
                        }
                        sources.push(s);
                    }
                    None => tracing::warn!("ROM absente des téléchargements, ignorée: {}", f.rel_path),
                }
                continue;
            }
            // ROM/sous/dossier/x.z64 → <dossier de l'exe>/sous/dossier/x.z64
            let inner = f.rel_path.strip_prefix("ROM/").or_else(|| f.rel_path.strip_prefix("rom/")).unwrap_or(&f.file_name);
            let dest_rel = rom_dir_rel.join(rel_to_path(inner));
            let dest = gd.join(&dest_rel);
            let out = if is_extract_archive(&f.file_name) {
                // Données en milliers de fichiers (romfs…) : extraites à côté de l'exécutable.
                let dir_rel = dest_rel.parent().map(Path::to_path_buf).unwrap_or_default();
                let staging = gd.join(INTERNAL_DIR).join("staging-rom");
                extract::extract(&staged, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {})?;
                let top: Vec<String> = std::fs::read_dir(&staging)
                    .map_err(|e| disk_error("lecture de l'archive extraite", e))?
                    .filter_map(Result::ok)
                    .map(|e| path_to_rel(&dir_rel.join(e.file_name())))
                    .collect();
                let n = extract::merge_into(&staging, &gd.join(&dir_rel))?;
                let _ = std::fs::remove_dir_all(&staging);
                let _ = std::fs::remove_file(&staged);
                tracing::info!("{} extraite : {n} fichier(s) posés", f.rel_path);
                top
            } else {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| disk_error("dossier des ROMs", e))?;
                }
                place_file(&staged, &dest).map_err(|e| disk_error("copie d'une ROM", e))?;
                placed.push(path_to_rel(&dest_rel));
                vec![path_to_rel(&dest_rel)]
            };
            sources.push(state::RomSource { rel_path: f.rel_path.clone(), size: f.size, sha1: f.sha1.clone(), md5: f.md5.clone(), placed: out });
        }
        // ROMs d'une installation précédente restées à un autre endroit : conservées
        // (on ne supprime jamais de fichiers du jeu).
        make_executable(&gd.join(&exe_for_perm));
        remove_quarantine(&gd);
        let _ = std::fs::remove_dir_all(&downloads);
        Ok((placed, sources))
    })
    .await?;
    st.roms = roms;
    st.rom_sources = sources;
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
    // Autre exécutable : le jeu qu'il lance sera relevé à nouveau.
    st.direct_launch = None;
    state::write_state(config_dir, &st)?;
    Ok(st)
}

/// Désinstalle un jeu ; avec `keep_saves`, les dossiers save/saves restent en place.
pub async fn uninstall(config_dir: &Path, st: InstalledGame, keep_saves: bool) -> AppResult<()> {
    match st.shared.clone() {
        Some(shared) => uninstall_page(config_dir, st, shared, keep_saves).await,
        None => uninstall_dir(config_dir, st, keep_saves).await,
    }
}

/// Jeu d'un runtime : retire sa ROM et ses données. Le runtime et le dossier de
/// données commun ne partent qu'avec le dernier jeu qui les utilise.
async fn uninstall_page(config_dir: &Path, st: InstalledGame, shared: state::SharedGame, keep_saves: bool) -> AppResult<()> {
    let others = state::pages_in(config_dir, &st.game_dir).into_iter().filter(|id| *id != st.rom_id).count();
    if shared.is_emulator() {
        let (page, sh) = (st.clone(), shared.clone());
        blocking(move || emulator::remove_rom_files(&page, &sh, keep_saves)).await?;
        state::forget(config_dir, st.rom_id)?;
        tracing::info!("« {} » désinstallé ({others} autre(s) jeu(x) sur l'émulateur, saves conservées: {keep_saves})", st.name);
        if others == 0 {
            if let Some(runtime) = state::find(config_dir, shared.runtime_rom_id) {
                // L'émulateur n'a aucune sauvegarde à garder : elles sont avec les ROM.
                uninstall_dir(config_dir, runtime, false).await?;
            }
        }
        return Ok(());
    }
    let (gd, roms, data) = (st.game_dir.clone(), st.roms.clone(), runtimes::existing_data_dir(&shared.data_dir));
    let (cache_dir, save_dir, whole_data) = (shared.cache_dir.clone(), shared.save_dir.clone(), others == 0 && !keep_saves);
    blocking(move || -> AppResult<()> {
        let remove_dir = |dir: PathBuf| match std::fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(log_err(AppError::Disk, "suppression des données du jeu", e)),
            _ => Ok(()),
        };
        for rom in &roms {
            let _ = std::fs::remove_file(gd.join(rel_to_path(rom)));
        }
        let Some(data) = data else { return Ok(()) };
        if whole_data {
            return remove_dir(data);
        }
        if let Some(cache) = cache_dir {
            remove_dir(data.join(rel_to_path(&cache)))?;
        }
        if !keep_saves {
            remove_dir(data.join(rel_to_path(&save_dir)))?;
        }
        Ok(())
    })
    .await?;
    state::forget(config_dir, st.rom_id)?;
    tracing::info!("« {} » désinstallé ({others} autre(s) jeu(x) sur le runtime, saves conservées: {keep_saves})", st.name);
    if others == 0 {
        if let Some(runtime) = state::find(config_dir, shared.runtime_rom_id) {
            uninstall_dir(config_dir, runtime, keep_saves).await?;
        }
    }
    Ok(())
}

async fn uninstall_dir(config_dir: &Path, st: InstalledGame, keep_saves: bool) -> AppResult<()> {
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
    // Runtime partagé : ses autres jeux ont suivi le dossier.
    state::relocate(config_dir, &old, &st.game_dir)?;
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

/// Archive de `ROM/` à extraire à côté de l'exécutable plutôt qu'à copier
/// telle quelle (ex. `exefs`/`romfs` d'un jeu Switch : des dizaines de milliers
/// de fichiers, qui rendraient RomM très lent s'ils étaient posés un par un).
pub fn is_extract_archive(file_name: &str) -> bool {
    file_name.to_ascii_lowercase().ends_with(".extract.zip")
}

/// ROM déjà posée lors d'une installation précédente, identique sur le serveur
/// (chemin, taille, empreintes) et toujours présente sur le disque.
fn unchanged_rom(sources: &[state::RomSource], f: &RemoteFile, game_dir: &Path) -> Option<state::RomSource> {
    sources
        .iter()
        .find(|s| {
            s.rel_path == f.rel_path
                && s.size == f.size
                && (f.sha1.is_none() || s.sha1 == f.sha1)
                && (f.md5.is_none() || s.md5 == f.md5)
                && !s.placed.is_empty()
                && s.placed.iter().all(|p| game_dir.join(rel_to_path(p)).exists())
        })
        .cloned()
}

fn exe_parent(exe_rel: &Path) -> PathBuf {
    // Bundle `.app` sur Mac : le dossier de son programme (Contents/MacOS), comme le
    // dossier de l'exécutable sous Windows ; c'est là que les portages cherchent leurs fichiers.
    if cfg!(target_os = "macos") && exe_rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
        return exe_rel.join("Contents").join("MacOS");
    }
    exe_rel.parent().map(Path::to_path_buf).unwrap_or_default()
}

pub fn rel_to_path(rel: &str) -> PathBuf {
    rel.split('/').filter(|s| !s.is_empty() && *s != "..").collect()
}

pub fn path_to_rel(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

pub(crate) fn ensure_space(dir: &Path, needed: u64) -> AppResult<()> {
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
pub(crate) fn make_executable(path: &Path) {
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
pub(crate) fn make_executable(_path: &Path) {}

pub(crate) fn remove_quarantine(game_dir: &Path) {
    if cfg!(target_os = "macos") {
        match std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(game_dir).status() {
            Ok(s) if s.success() => tracing::info!("attribut de quarantaine retiré"),
            Ok(s) => tracing::warn!("xattr a échoué ({s})"),
            Err(e) => tracing::warn!("xattr introuvable: {e}"),
        }
    }
}

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
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
            rom_sources: vec![],
            mods_downloaded: vec![],
            saves: state::SaveSync { dir: Some(old.join("saves")), ..Default::default() },
            direct_launch: None,
            prefer_launcher: false,
            players: Vec::new(),
            installed_at: 0,
            updated_at: 0,
            game_dir: old.clone(),
            pc: None,
            shared: None,
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
    fn unchanged_roms_are_not_fetched_again() {
        let root = std::env::temp_dir().join(format!("farfa-roms-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("data/romfs")).unwrap();
        let f = RemoteFile { id: 1, file_name: "MHGU-data.extract.zip".into(), rel_path: "ROM/MHGU-data.extract.zip".into(), size: 10, sha1: None, md5: None, from_manager: false };
        let src = state::RomSource { rel_path: f.rel_path.clone(), size: 10, sha1: None, md5: None, placed: vec!["data".into()] };
        assert!(is_extract_archive(&f.file_name) && !is_extract_archive("game.zip"));
        assert!(unchanged_rom(std::slice::from_ref(&src), &f, &root).is_some());
        // Autre taille sur le serveur, ou dossier supprimé : à retélécharger.
        assert!(unchanged_rom(&[state::RomSource { size: 9, ..src.clone() }], &f, &root).is_none());
        std::fs::remove_dir_all(root.join("data")).unwrap();
        assert!(unchanged_rom(&[src], &f, &root).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn emulator_rom_is_reused_by_the_recomp() {
        let root = std::env::temp_dir().join(format!("farfa-switch-{}", uuid::Uuid::new_v4()));
        let rom_dir = root.join("Roms/zelda");
        std::fs::create_dir_all(&rom_dir).unwrap();
        std::fs::write(rom_dir.join("oot.nup"), b"ROM!").unwrap();
        let f = RemoteFile { id: 1, file_name: "oot.nup".into(), rel_path: "oot.nup".into(), size: 4, sha1: Some("AB".into()), md5: None, from_manager: false };
        let other = RemoteFile { id: 2, file_name: "autre.bin".into(), rel_path: "autre.bin".into(), size: 4, sha1: Some("cd".into()), md5: None, from_manager: false };
        let mut emu: InstalledGame = serde_json::from_value(serde_json::json!({
            "romId": 1, "name": "Zelda", "version": null, "target": "Windows", "archiveName": "", "executable": "lichii.exe",
            "gameDir": root.join("Emulateurs/lichii"), "installedAt": 0, "updatedAt": 0
        }))
        .unwrap();
        emu.rom_sources = vec![state::RomSource { rel_path: "oot.nup".into(), size: 4, sha1: Some("ab".into()), md5: None, placed: vec!["oot.nup".into()] }];
        emu.shared = Some(state::SharedGame {
            runtime_rom_id: -1,
            game: "lichii".into(),
            runtime_name: "Lichii".into(),
            data_dir: String::new(),
            save_dir: String::new(),
            cache_dir: None,
            kind: Some(emulator::KIND_EMULATOR.into()),
            rom_dir: Some(rom_dir.clone()),
            saves_dir: None,
            save_patterns: Vec::new(),
        });
        let downloads = root.join("recomp/.farfalauncher/downloads");
        let reused = reuse_emulator_roms(&emu, &[f.clone(), other], &downloads);
        assert_eq!(reused.len(), 1, "seule la même ROM (SHA-1 sans tenir compte de la casse) est reprise");
        assert_eq!(std::fs::read(downloads.join("oot.nup")).unwrap(), b"ROM!");
        remove_reused(&reused);
        assert!(!rom_dir.join("oot.nup").exists());
        assert_eq!(std::fs::read(downloads.join("oot.nup")).unwrap(), b"ROM!", "le contenu survit à l'ancienne copie");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Vraie archive : `FARFA_EXTRACT_ZIP=chemin.extract.zip cargo test -- --ignored extract_real`.
    #[test]
    #[ignore]
    fn extract_real_archive() {
        let Some(zip) = std::env::var_os("FARFA_EXTRACT_ZIP").map(PathBuf::from) else { return };
        let root = std::env::temp_dir().join(format!("farfa-real-{}", uuid::Uuid::new_v4()));
        let staging = root.join(INTERNAL_DIR).join("staging-rom");
        extract::extract(&zip, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        let n = extract::merge_into(&staging, &root).unwrap();
        println!("{n} fichiers posés dans {}", root.display());
        assert!(n > 0);
    }

    /// Parcours complet contre un vrai serveur, sans toucher aux vraies données
    /// (POKEPORT_IDENTITY) : `FARFA_E2E_URL=… FARFA_E2E_TOKEN=… FARFA_E2E_YELLOW=<id>
    /// FARFA_E2E_GOLD=<id> FARFA_E2E_RUNTIME=<id> cargo test -- --ignored shared_runtime_e2e --nocapture`.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn shared_runtime_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let id = |k: &str| var(k).parse::<i64>().unwrap();
        let (yellow, gold, runtime_id) = (id("FARFA_E2E_YELLOW"), id("FARFA_E2E_GOLD"), id("FARFA_E2E_RUNTIME"));
        let identity = "farfa-test-g1-e2e";
        let data = PathBuf::from(var("APPDATA")).join(identity);
        let _ = std::fs::remove_dir_all(&data);
        let base = std::env::temp_dir().join(format!("farfa-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manifest: Manifest = serde_json::from_value(serde_json::json!({
            "runtimes": [{ "id": "gen1recomp", "name": "Gen1recomp", "repo": "https://github.com/bryanthaboi/gen1recomp",
                "romId": runtime_id, "exe": "gen1recomp", "dataDir": identity, "saveDir": "saves/{game}", "cacheDir": "{game}",
                "args": ["--game={game}"], "env": [["POKEPORT_IMPORT_ROM", "{rom}"], ["POKEPORT_IDENTITY", identity]] }],
            "games": [{ "runtime": "gen1recomp", "game": "yellow", "romId": yellow }, { "runtime": "gen1recomp", "game": "gold", "romId": gold }]
        }))
        .unwrap();
        let job = InstallJob {
            client: RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap(),
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest,
            catalog: Catalog::default(),
            manager_base: None,
            with_emulator: false,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let installed = |o: InstallOutcome| match o {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("le runtime ne doit rien demander"),
        };

        // Premier jeu : runtime + sa seule ROM.
        let st = installed(job.run(yellow).await.unwrap());
        let dir = st.game_dir.clone();
        let exe = dir.join(st.executable.clone().unwrap());
        println!("dossier {} ; version {:?} ; lancement {:?}", dir.display(), st.version, st.direct_launch);
        assert!(exe.exists() && state::find(&config, runtime_id).is_some());
        assert_eq!(std::fs::read_dir(dir.join(".farfalauncher/roms")).unwrap().count(), 1);
        assert!(st.saves.dir.as_ref().is_some_and(|d| d.ends_with("saves/yellow")));
        assert_eq!(st.direct_launch.as_ref().unwrap().args, ["--game=yellow"]);
        let stamp = std::fs::metadata(&exe).unwrap().modified().unwrap();

        // Lancement : import sans menu, puis le jeu s'ouvre.
        let d = st.direct_launch.clone().unwrap();
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        for _ in 0..120 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if data.join("yellow/rom-cache.complete").exists() {
                break;
            }
        }
        assert!(data.join("yellow/rom-cache.complete").exists(), "ROM non importée");
        tokio::time::sleep(Duration::from_secs(12)).await;
        std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status().unwrap();
        waiter.await.unwrap();
        println!("sortie du jeu :\n{}", std::fs::read_to_string(dir.join(".farfalauncher/dernier-lancement.log")).unwrap_or_default());

        // Deuxième jeu : le runtime n'est pas retouché.
        let st2 = installed(job.run(gold).await.unwrap());
        assert_eq!(st2.game_dir, dir);
        assert_eq!(std::fs::metadata(&exe).unwrap().modified().unwrap(), stamp, "runtime réinstallé");
        assert_eq!(std::fs::read_dir(dir.join(".farfalauncher/roms")).unwrap().count(), 2);
        assert_eq!(state::list_installed(&config).len(), 3);

        // Désinstaller Jaune sans ses sauvegardes : Or et le runtime ne bougent pas.
        for s in ["saves/yellow", "saves/gold"] {
            std::fs::create_dir_all(data.join(s)).unwrap();
            std::fs::write(data.join(s).join("slot1.lua"), b"x").unwrap();
        }
        uninstall(&config, state::find(&config, yellow).unwrap(), false).await.unwrap();
        assert!(state::find(&config, yellow).is_none() && state::find(&config, gold).is_some());
        assert!(exe.exists() && !data.join("saves/yellow").exists() && !data.join("yellow").exists());
        assert!(data.join("saves/gold/slot1.lua").exists() && data.join("options.lua").exists());
        assert_eq!(std::fs::read_dir(dir.join(".farfalauncher/roms")).unwrap().count(), 1);

        // Le dernier jeu emporte le runtime ; ses sauvegardes restent si on le demande.
        uninstall(&config, state::find(&config, gold).unwrap(), true).await.unwrap();
        assert!(!exe.exists() && state::find(&config, runtime_id).is_none());
        assert!(data.join("saves/gold/slot1.lua").exists());
        assert!(state::list_installed(&config).is_empty());

        std::fs::remove_dir_all(&data).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Recomps servies par FarfaRomManager, dans des dossiers temporaires : `FARFA_E2E_URL=… FARFA_E2E_MANAGER=…
    /// FARFA_E2E_TOKEN=… cargo test -- --ignored recomp_manager_e2e --nocapture`.
    /// Vrai serveur : reprise depuis RomM puis envois sur FarfaRomManager, sur un jeu sans
    /// sauvegarde ; tout est retiré à la fin. `FARFA_E2E_URL`, `_TOKEN`, `_MANAGER`.
    #[tokio::test]
    #[ignore]
    async fn recomp_saves_e2e() {
        use crate::romm::saves as romm;
        use crate::save_store::RecompSave;
        let var = |k: &str| std::env::var(k).unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = var("FARFA_E2E_MANAGER");
        let manager = client.with_base(&base);
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let mut free = None;
        for r in &catalog.recomps {
            for g in &r.games {
                let Some(rom_id) = g.rom_id else { continue };
                let game = g.game.clone().unwrap_or_else(|| r.id.clone());
                let save = RecompSave::new(client.clone(), &base, Some(rom_id), &r.id, &game);
                if save.find().await.unwrap().is_none() && romm::find(&client, rom_id).await.unwrap().is_none() {
                    free = Some((save, rom_id, format!("api/saves/{}/{game}", r.id)));
                }
            }
        }
        let (save, rom_id, path) = free.expect("aucun jeu de recomp sans sauvegarde");
        println!("essai sur {path} (ROM {rom_id})");
        let zip = |text: &str| {
            let mut out = std::io::Cursor::new(Vec::new());
            let mut w = zip::ZipWriter::new(&mut out);
            w.start_file("test.sav", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, text.as_bytes()).unwrap();
            w.finish().unwrap();
            out.into_inner()
        };

        let on_romm = romm::upload(&client, rom_id, zip("depuis romm")).await.unwrap();
        let imported = save.import_from_romm().await;
        let hash = crate::saves::hash_zip;
        let check = async {
            let imported = imported.unwrap().expect("sauvegarde reprise");
            assert_eq!(hash(&save.download().await.unwrap()).unwrap(), hash(&zip("depuis romm")).unwrap());
            assert_eq!(save.find().await.unwrap().unwrap().updated_at, imported.updated_at);
            // Sur RomM, elle reste en place.
            assert_eq!(romm::find(&client, rom_id).await.unwrap().unwrap().id, on_romm.id);
            let next = zip("depuis le launcher");
            let date = save.upload(next.clone()).await.unwrap();
            assert_ne!(date, imported.updated_at);
            assert_eq!(save.download().await.unwrap(), next);
        }
        .await;
        client.send(client.post("api/saves/delete", &serde_json::json!({ "saves": [on_romm.id] }))).await.unwrap();
        manager.send(manager.delete(&path, &())).await.unwrap();
        assert!(save.find().await.unwrap().is_none());
        assert!(romm::find(&client, rom_id).await.unwrap().is_none());
        check
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn recomp_manager_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-recomp-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let manifest = Manifest::default().with_catalog(&catalog);
        println!("{} recomps, runtime(s) partagé(s) : {:?}", catalog.recomps.len(), manifest.runtimes.iter().map(|r| (&r.id, r.rom_id)).collect::<Vec<_>>());
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: manifest.clone(),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: false,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let installed = |o: InstallOutcome| match o {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { game } => panic!("{} : choix demandé parmi {:?}", game.name, game.pending_candidates),
        };

        // Launcher à plusieurs jeux : programme du gestionnaire, ROM de RomM.
        let runtime = &manifest.runtimes[0];
        let recomp = catalog.recomp(&runtime.id).unwrap();
        let (premier, second) = (manifest.games[0].rom_id, manifest.games[1].rom_id);
        let st = installed(job.run(premier).await.unwrap());
        let exe = st.game_dir.join(rel_to_path(st.executable.as_deref().unwrap()));
        println!("{} sur {} : {:?} {:?}", st.name, st.version.as_deref().unwrap_or("?"), exe.file_name(), st.direct_launch.as_ref().map(|d| &d.args));
        assert!(exe.exists());
        assert_eq!(st.version_raw.as_deref(), Some(recomp.version.cle.as_str()));
        let stamp = std::fs::metadata(&exe).unwrap().modified().unwrap();
        let st2 = installed(job.run(second).await.unwrap());
        assert_eq!(std::fs::metadata(&exe).unwrap().modified().unwrap(), stamp, "runtime réinstallé");
        assert_eq!(st2.game_dir, st.game_dir);

        // Recomp à un seul jeu : même chemin qu'avant, seul le programme change de source.
        let seule = catalog.recomps.iter().filter(|r| !r.is_shared() && r.files.iter().any(|f| f.system.as_deref() == Some("Windows")) && r.games.iter().any(|g| g.rom_id.is_some())).min_by_key(|r| r.files.iter().map(|f| f.size).sum::<u64>()).unwrap();
        let rom = seule.games[0].rom_id.unwrap();
        println!("recomp à un jeu : {} (ROM {rom})", seule.name);
        match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => {
                assert_eq!(game.version_raw.as_deref(), Some(seule.version.cle.as_str()));
                assert!(game.game_dir.join(rel_to_path(game.executable.as_deref().unwrap())).exists());
                println!("  installée : {:?} {} ROM", game.executable, game.roms.len());
                assert!(!game.roms.is_empty(), "la ROM de RomM devait être posée");
            }
            InstallOutcome::NeedsChoice { game } => {
                println!("  plusieurs exécutables : {:?}", game.pending_candidates);
                assert_eq!(game.version_raw.as_deref(), Some(seule.version.cle.as_str()));
            }
        }

        // Déjà à la version du catalogue : pas de mise à jour. Autre version : une vraie mise à jour.
        let mut ancien = state::find(&config, recomp.runtime_rom_id()).unwrap();
        ancien.version_raw = Some("Pokemon\nVersion : texte de VERSION.txt".into());
        state::write_state(&config, &ancien).unwrap();
        assert_eq!(crate::manager::align_versions(&config, &catalog), 1);
        assert_eq!(state::find(&config, recomp.runtime_rom_id()).unwrap().version_raw.as_deref(), Some(recomp.version.cle.as_str()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Vrai serveur et vrai émulateur, dans des dossiers temporaires : `FARFA_E2E_URL=… FARFA_E2E_MANAGER=…
    /// FARFA_E2E_TOKEN=… FARFA_E2E_ROM_A=<id gba> FARFA_E2E_ROM_B=<id gba> cargo test -- --ignored emulator_e2e --nocapture`.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn emulator_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let (a, b): (i64, i64) = (var("FARFA_E2E_ROM_A").parse().unwrap(), var("FARFA_E2E_ROM_B").parse().unwrap());
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-emu-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        println!("{} émulateur(s) au catalogue", catalog.emulators.len());
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default(),
            catalog,
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: true,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let installed = |o: InstallOutcome| match o {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("l'émulateur ne doit rien demander"),
        };

        // Premier jeu : émulateur + ROM.
        let st = installed(job.run(a).await.unwrap());
        let shared = st.shared.clone().unwrap();
        let exe = st.game_dir.join(rel_to_path(st.executable.as_deref().unwrap()));
        let rom_dir = shared.rom_dir.clone().unwrap();
        println!("exe {} ; ROM dans {} ; lancement {:?} ; saves {:?} {:?}", exe.display(), rom_dir.display(), st.direct_launch, st.saves.dir, st.saves.patterns);
        assert!(exe.exists() && exe.with_file_name("portable.ini").exists());
        assert!(std::fs::read_dir(&rom_dir).unwrap().count() >= 1);
        assert_eq!(st.saves.dir.as_deref(), Some(rom_dir.as_path()));
        let stamp = std::fs::metadata(&exe).unwrap().modified().unwrap();

        // Lancement direct, sauvegarde repérée à côté de la ROM.
        let d = st.direct_launch.clone().unwrap();
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(8)).await;
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        let loc = crate::saves::SaveLocation { dir: st.saves.dir.clone().unwrap(), patterns: st.saves.patterns.clone() };
        let saves = crate::saves::collect(&loc).unwrap();
        println!("sauvegardes : {:?}", saves.iter().map(|(r, _)| r).collect::<Vec<_>>());
        assert_eq!(saves.len(), 1, "sauvegarde attendue à côté de la ROM");

        // Deuxième jeu : l'émulateur n'est pas retouché.
        let st2 = installed(job.run(b).await.unwrap());
        assert_eq!(st2.game_dir, st.game_dir);
        assert_eq!(std::fs::metadata(&exe).unwrap().modified().unwrap(), stamp, "émulateur réinstallé");

        // Désinstaller en gardant la sauvegarde : ROM partie, sauvegarde et autre jeu intacts.
        uninstall(&config, state::find(&config, a).unwrap(), true).await.unwrap();
        assert!(exe.exists() && state::find(&config, b).is_some());
        assert_eq!(crate::saves::collect(&loc).unwrap().len(), 1, "sauvegarde perdue");
        assert_eq!(std::fs::read_dir(&rom_dir).unwrap().count(), 1, "la ROM devait partir");

        // Le dernier jeu emporte l'émulateur.
        uninstall(&config, state::find(&config, b).unwrap(), false).await.unwrap();
        assert!(!exe.exists() && state::list_installed(&config).is_empty());
        assert!(rom_dir.join(&saves[0].0).exists(), "la sauvegarde du premier jeu devait rester");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Vrai serveur, puNES lancé : `FARFA_E2E_ROM` (jeu NES à pile), plus URL, TOKEN, MANAGER.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn punes_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-punes-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default(),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: true,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("l'émulateur ne doit rien demander"),
        };
        let shared = st.shared.clone().unwrap();
        let exe = st.game_dir.join(rel_to_path(st.executable.as_deref().unwrap()));
        let cfg = exe.with_file_name("puNES.cfg");
        println!("exe {} ; saves {:?} {:?}", exe.display(), shared.saves_dir, shared.save_patterns);
        assert_eq!(shared.runtime_name, "puNES");
        assert!(cfg.exists(), "puNES.cfg (mode portable) attendu");
        assert_eq!(shared.saves_dir.as_deref(), Some(exe.with_file_name("prb").as_path()));

        // Comme play_game : règles de langue, puis lancement direct.
        let rules = catalog.language_for_install(&st);
        let exe_dir = exe.parent().unwrap().to_path_buf();
        let env = crate::language::prepare(&st.game_dir, &exe_dir, &rules);
        assert!(env.is_empty());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap().trim(), "language=french");
        let d = st.direct_launch.clone().unwrap();
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(20)).await;
        // Fermeture normale (pas /F) : puNES écrit sa pile et ses réglages en partant.
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        let text = std::fs::read_to_string(&cfg).unwrap();
        println!("puNES.cfg après la partie : {} lignes, langue : {:?}", text.lines().count(), text.lines().find(|l| l.starts_with("language")));
        assert!(text.lines().any(|l| l.split('=').map(str::trim).collect::<Vec<_>>() == ["language", "french"]));
        let loc = crate::saves::SaveLocation { dir: shared.saves_dir.clone().unwrap(), patterns: shared.save_patterns.clone() };
        let saves = crate::saves::collect(&loc).unwrap();
        println!("sauvegardes : {:?}", saves.iter().map(|(r, _)| r).collect::<Vec<_>>());
        assert_eq!(saves.len(), 1, "sauvegarde .prb attendue dans prb/");
        println!("dossier de test : {}", base.display());
    }

    /// Vrai serveur, Dolphin lancé : `FARFA_E2E_ROM` (jeu GameCube), plus URL, TOKEN, MANAGER.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn dolphin_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-dolphin-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default(),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: true,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("l'émulateur ne doit rien demander"),
        };
        let shared = st.shared.clone().unwrap();
        let exe = st.game_dir.join(rel_to_path(st.executable.as_deref().unwrap()));
        let exe_dir = exe.parent().unwrap().to_path_buf();
        let rom_file = std::fs::read_dir(shared.rom_dir.as_ref().unwrap()).unwrap().filter_map(Result::ok).map(|e| e.path()).next().unwrap();
        let id = crate::install::emulator::disc_id(&rom_file);
        println!("exe {} ; disque {:?} ; saves {:?} {:?} ; lancement {:?}", exe.display(), id, shared.saves_dir, shared.save_patterns, st.direct_launch.as_ref().map(|d| &d.args));
        assert_eq!(shared.runtime_name, "Dolphin");
        assert!(exe_dir.join("portable.txt").exists());
        let id = id.expect("identifiant du disque lisible");
        let region = if id.as_bytes()[3] == b'E' { "USA" } else { "EUR" };
        assert_eq!(shared.saves_dir.as_deref(), Some(exe_dir.join("User").join("GC").join(region).join("Card A").as_path()));
        assert_eq!(shared.save_patterns, [format!("*-{}-*.gci", &id[..4])]);

        let rules = catalog.language_for_install(&st);
        crate::language::prepare(&st.game_dir, &exe_dir, &rules);
        let ini = exe_dir.join("User").join("Config").join("Dolphin.ini");
        println!("Dolphin.ini avant :\n{}", std::fs::read_to_string(&ini).unwrap());
        let d = st.direct_launch.clone().unwrap();
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(35)).await;
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        let text = std::fs::read_to_string(&ini).unwrap();
        let get = |k: &str| text.lines().find(|l| l.split('=').next().map(str::trim) == Some(k)).map(|l| l.split('=').nth(1).unwrap_or("").trim().to_string());
        println!("après : {} lignes ; LanguageCode {:?} ; SelectedLanguage {:?} ; PermissionAsked {:?}", text.lines().count(), get("LanguageCode"), get("SelectedLanguage"), get("PermissionAsked"));
        assert_eq!(get("LanguageCode").as_deref(), Some("fr"));
        assert_eq!(get("SelectedLanguage").as_deref(), Some("2"));
        println!("carte mémoire : {:?}", std::fs::read_dir(shared.saves_dir.as_ref().unwrap()).map(|r| r.filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>()));
        println!("dossier de test : {}", base.display());
    }

    /// Vrai serveur, n'importe quel émulateur : installe `FARFA_E2E_ROM` avec l'émulateur de sa
    /// console, pose la langue, lance le jeu 25 s puis liste les sauvegardes. Garde le dossier.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn emulator_play_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-play-e2e-{rom}"));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default(),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: true,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("l'émulateur ne doit rien demander"),
        };
        let d = st.direct_launch.clone().unwrap();
        let exe_dir = st.game_dir.join(rel_to_path(&d.exe)).parent().unwrap().to_path_buf();
        let rules = catalog.language_for_install(&st);
        crate::language::prepare(&st.game_dir, &exe_dir, &rules);
        println!("ROM {:?} ; lancement {:?} ; saves {:?} {:?}", st.roms, d.args, st.saves.dir, st.saves.patterns);
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(25)).await;
        // Fermeture normale d'abord : l'émulateur écrit alors sa configuration et la sauvegarde.
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        if let Some(dir) = &st.saves.dir {
            println!("dossier des sauvegardes : {:?}", std::fs::read_dir(dir).map(|r| r.filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>()));
        }
        println!("dossier de l'émulateur : {:?}", std::fs::read_dir(&exe_dir).map(|r| r.filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>()));
        println!("dossier de test : {}", base.display());
    }

    /// Vrai serveur, recomp dont le jeu est une archive à décompresser (NSMBW) : `FARFA_E2E_ROM`, plus URL, TOKEN, MANAGER.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn recomp_game_files_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-game-files-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default().with_catalog(&catalog),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: false,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { game } => {
                let details = runtimes::fetch_details(&job.client, &job.manifest, &catalog, rom, Target::Windows).await.unwrap();
                let exe = game.pending_candidates[0].clone();
                finalize(&config, game, &exe, &details).await.unwrap()
            }
        };
        let sources: Vec<&str> = st.rom_sources.iter().map(|s| s.rel_path.as_str()).collect();
        println!("exe {:?} ; candidats {:?} ; données {:?} ; posées {:?}", st.executable, st.pending_candidates, sources, st.rom_sources.iter().map(|s| s.placed.len()).sum::<usize>());
        assert!(!sources.is_empty() && sources.iter().all(|s| s.ends_with(".extract.zip")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Vrai serveur, recomp à un jeu lancée sur sa ROM (Paper Cut) : `FARFA_E2E_ROM`, plus URL, TOKEN, MANAGER.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn recomp_direct_rom_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-recomp-rom-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default().with_catalog(&catalog),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: false,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { game } => panic!("choix demandé parmi {:?}", game.pending_candidates),
        };
        let d = catalog.recomp_launch(&st).expect("lancement direct");
        println!("exe {:?} ; ROM {:?} ; lancement {:?}", st.executable, st.roms, d);
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(20)).await;
        let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/V", "/FO", "LIST"]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        println!("{text}");
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        assert!(text.contains("Paper Cut") || text.contains("ttyd"), "jeu fermé avant 20 s");
        println!("dossier de test : {}", base.display());
    }

    /// Vrai serveur, PPSSPP lancé : `FARFA_E2E_ROM` (jeu PSP en .chd), plus URL, TOKEN, MANAGER.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore]
    async fn ppsspp_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let rom: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let base = std::env::temp_dir().join(format!("farfa-ppsspp-e2e-{}", uuid::Uuid::new_v4()));
        let config = base.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let job = InstallJob {
            client,
            config_dir: config.clone(),
            install_root: base.join("Jeux"),
            target: Target::Windows,
            cancel: CancellationToken::new(),
            proton: None,
            manifest: Manifest::default(),
            catalog: catalog.clone(),
            manager_base: Some(var("FARFA_E2E_MANAGER")),
            with_emulator: true,
            emulator_id: None,
            runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
            on_progress: Arc::new(|_| {}),
        };
        let st = match job.run(rom).await.unwrap() {
            InstallOutcome::Installed { game } => game,
            InstallOutcome::NeedsChoice { .. } => panic!("l'émulateur ne doit rien demander"),
        };
        let shared = st.shared.clone().unwrap();
        let exe = st.game_dir.join(rel_to_path(st.executable.as_deref().unwrap()));
        let exe_dir = exe.parent().unwrap().to_path_buf();
        println!("exe {} ; saves {:?} {:?}", exe.display(), shared.saves_dir, shared.save_patterns);
        assert_eq!(shared.runtime_name, "PPSSPP");
        assert_eq!(shared.saves_dir.as_deref(), Some(exe_dir.join("memstick").join("PSP").join("SAVEDATA").as_path()));
        assert_eq!(shared.save_patterns.len(), 1);
        assert!(shared.save_patterns[0].len() == 10 && shared.save_patterns[0].ends_with('*'), "motif {{discid}}* attendu");

        let rules = catalog.language_for_install(&st);
        crate::language::prepare(&st.game_dir, &exe_dir, &rules);
        let ini = exe_dir.join("memstick").join("PSP").join("SYSTEM").join("ppsspp.ini");
        let d = st.direct_launch.clone().unwrap();
        let launch = crate::launch::Launch { exe: rel_to_path(&d.exe), args: &d.args, env: &d.env, cwd: None, learn: false, runner: None };
        let running = crate::launch::spawn(&st.game_dir, launch).unwrap();
        let pid = running.pid();
        let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
        tokio::time::sleep(Duration::from_secs(35)).await;
        std::process::Command::new("taskkill").args(["/PID", &pid.to_string()]).status().unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        let _ = std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).status();
        waiter.await.unwrap();
        let text = std::fs::read_to_string(&ini).unwrap();
        let get = |k: &str| text.lines().find(|l| l.split('=').next().map(str::trim) == Some(k)).map(|l| l.split('=').nth(1).unwrap_or("").trim().to_string());
        println!("ppsspp.ini après : {} lignes ; Language {:?} ; GameLanguage {:?} ; CheckForNewVersion {:?}", text.lines().count(), get("Language"), get("GameLanguage"), get("CheckForNewVersion"));
        assert_eq!(get("Language").as_deref(), Some("fr_FR"));
        assert_eq!(get("GameLanguage").as_deref(), Some("2"));
        println!("SAVEDATA : {:?}", std::fs::read_dir(shared.saves_dir.as_ref().unwrap()).map(|r| r.filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>()));
        println!("dossier de test : {}", base.display());
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
