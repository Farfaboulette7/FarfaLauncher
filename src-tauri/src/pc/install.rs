//! Installation d'un jeu PC : téléchargement des fichiers retenus, puis selon
//! le cas extraction des archives, installateurs GOG en silencieux, ou
//! clients Call of Duty ; Proton préparé d'abord sous Linux.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures_util::{stream, StreamExt};
use tokio_util::sync::CancellationToken;

use super::cod::{self, ClientDirs, CodGame};
use super::proton::{self, ProtonSetup, Runner};
use super::{gog, steam, PcGame, PcInstall, PcKind, PcOs};
use crate::error::{log_err, AppError, AppResult};
use crate::install::extract::{self, INTERNAL_DIR};
use crate::install::state::{self, InstalledGame};
use crate::install::{blocking, detect, download, ensure_space, finalize, path_to_rel, rel_to_path, InstallJob, InstallOutcome};
use crate::launch::{self, Launch};
use crate::platform::{ArchiveKind, Target};
use crate::romm::library::{Game, RemoteFile};

/// Téléchargements simultanés.
const PARALLEL_DOWNLOADS: usize = 6;

/// Place prise sur le disque par rapport aux fichiers téléchargés.
fn space_factor(kind: PcKind) -> f64 {
    match kind {
        PcKind::Portable => 1.05,
        // Installateurs ou archive, plus le jeu installé.
        PcKind::GogInstaller => 2.3,
        PcKind::Archive | PcKind::Cod => 3.2,
    }
}

/// `%LOCALAPPDATA%` vu par le jeu (celui du préfixe sous Proton).
pub fn local_appdata(runner: Option<&Runner>) -> PathBuf {
    match runner {
        Some(r) => r.local_appdata(),
        None => std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default(),
    }
}

/// Préfixe Wine d'un jeu (gardé à la désinstallation, comme ses sauvegardes).
pub fn prefix_for(config_dir: &Path, game_dir: &Path, rom_id: i64) -> PathBuf {
    let name = game_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("jeu-{rom_id}"));
    proton::prefixes_dir(config_dir).join(name)
}

/// Le programme de `name` à la racine du jeu, quelle que soit sa casse.
fn find_top_ci(dir: &Path, name: &str) -> Option<String> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.eq_ignore_ascii_case(name))
}

/// Binaires ELF et scripts d'un jeu Linux : les droits d'exécution ne
/// survivent pas au téléchargement.
#[cfg(unix)]
fn make_all_executable(dir: &Path) {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            make_all_executable(&path);
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        let mut magic = [0u8; 4];
        let elf = std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"\x7fELF";
        if elf || name.ends_with(".sh") || name.ends_with(".x86_64") || name.ends_with(".x86") {
            if let Ok(m) = std::fs::metadata(&path) {
                let mut p = m.permissions();
                p.set_mode(p.mode() | 0o755);
                let _ = std::fs::set_permissions(&path, p);
            }
        }
    }
}

#[cfg(not(unix))]
fn make_all_executable(_dir: &Path) {}

/// Progression cumulée de téléchargements simultanés.
struct DownloadProgress {
    rom_id: i64,
    total: u64,
    received: AtomicU64,
    last: Mutex<Instant>,
}

impl DownloadProgress {
    fn report(&self, job: &InstallJob, verifying: bool) {
        let mut last = self.last.lock().unwrap();
        if verifying || last.elapsed() >= Duration::from_millis(200) {
            *last = Instant::now();
            let done = self.received.load(Ordering::Relaxed);
            if verifying {
                job.emit_verifying(self.rom_id, done, self.total);
            } else {
                job.emit(self.rom_id, "download", done, self.total);
            }
        }
    }
}

async fn fetch(job: &InstallJob, file: &RemoteFile, dest: PathBuf, progress: &DownloadProgress) -> AppResult<()> {
    let mut prev = 0u64;
    download::download(&job.client, file, &dest, &job.cancel, |got| {
        progress.received.fetch_add(got.saturating_sub(prev), Ordering::Relaxed);
        prev = got;
        progress.report(job, false);
    }, || progress.report(job, true))
    .await
}

/// Octets posés dans le dossier du jeu (hors dossier interne du launcher).
async fn installed_size(game_dir: &Path) -> u64 {
    fn size(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
        rd.filter_map(Result::ok)
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() && e.file_name() != INTERNAL_DIR => size(&e.path()),
                Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    let dir = game_dir.to_path_buf();
    tokio::task::spawn_blocking(move || size(&dir)).await.unwrap_or(0)
}

/// Lance un installateur GOG dans `game_dir` et attend sa fin.
async fn run_installer(setup: &Path, game_dir: &Path, runner: Option<&Runner>) -> AppResult<()> {
    let name = setup.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    tracing::info!("installateur {name}");
    match runner {
        Some(r) => {
            let args = gog::installer_args(&proton::windows_path(game_dir));
            let rel = setup.strip_prefix(game_dir).map_err(|_| AppError::Internal)?.to_path_buf();
            let running = launch::spawn(game_dir, Launch { exe: rel, args: &args, env: &[], cwd: None, learn: false, runner: Some(r) })?;
            blocking(move || {
                running.wait(|| {});
                Ok(())
            })
            .await
        }
        None => {
            #[cfg(windows)]
            {
                let args = gog::installer_args(&game_dir.to_string_lossy());
                let exe = setup.to_path_buf();
                let code = blocking(move || gog::run_elevated(&exe, &args).map_err(|e| log_err(AppError::InstallerFailed, "lancement de l'installateur", e))).await?;
                // Certains installateurs GOG plantent en se fermant (0xC000041D) alors
                // que le jeu est bien posé : c'est sa description qui fait foi.
                if code != 0 && gog::play_task(game_dir).is_none() {
                    tracing::error!("{name} terminé avec le code {code}");
                    return Err(AppError::InstallerFailed);
                }
                if code != 0 {
                    tracing::warn!("{name} terminé avec le code {code:#x}, mais le jeu est installé");
                }
                Ok(())
            }
            #[cfg(not(windows))]
            {
                tracing::error!("installateur Windows sans Proton: {name}");
                Err(AppError::NotAvailable)
            }
        }
    }
}

pub async fn run(job: &InstallJob, game: Game, pc: PcGame) -> AppResult<InstallOutcome> {
    let rom_id = game.id;
    if !pc.runnable {
        return Err(AppError::NotAvailable);
    }
    let previous = state::find(&job.config_dir, rom_id);
    let game_dir = match &previous {
        Some(p) => p.game_dir.clone(),
        None => job.install_root.join(state::safe_folder_name(&game.folder_name, rom_id)),
    };
    let downloads = game_dir.join(INTERNAL_DIR).join("downloads");
    tracing::info!("installation de « {} » (jeu PC {:?}/{:?}) dans {}", game.name, pc.kind, pc.os, game_dir.display());
    ensure_space(&game_dir, (pc.size as f64 * space_factor(pc.kind)) as u64)?;
    std::fs::create_dir_all(game_dir.join(INTERNAL_DIR)).map_err(|e| download::disk_error("création du dossier du jeu", e))?;

    // 1. Proton d'abord : s'il manque, inutile de télécharger le jeu.
    let previous_pc = previous.as_ref().and_then(|p| p.pc.clone());
    let proton_setup = pc.proton.then(|| ProtonSetup {
        version: job.proton.clone().or_else(|| previous_pc.as_ref().and_then(|p| p.proton.as_ref()?.version.clone())),
        prefix: prefix_for(&job.config_dir, &game_dir, rom_id),
    });
    let mut runner = match &proton_setup {
        Some(setup) => {
            let mut last = Instant::now() - Duration::from_secs(1);
            let r = proton::runner(&job.config_dir, setup, None, &job.cancel, |got, total| {
                if last.elapsed() >= Duration::from_millis(200) || got == total {
                    last = Instant::now();
                    job.emit(rom_id, "proton", got, total);
                }
            })
            .await?;
            Some(r)
        }
        None => None,
    };

    // 2. Téléchargements. Un jeu prêt à lancer va directement à sa place.
    let in_place = pc.kind == PcKind::Portable;
    let dest_of = |rel: &str| if in_place { game_dir.join(rel_to_path(rel)) } else { downloads.join(rel_to_path(rel)) };
    // Plusieurs fichiers à la fois : un jeu en milliers de petits fichiers
    // passe sinon son temps à attendre chaque requête.
    let total: u64 = pc.files.iter().map(|f| f.size).sum();
    let progress = DownloadProgress { rom_id, total, received: AtomicU64::new(0), last: Mutex::new(Instant::now() - Duration::from_secs(1)) };
    let mut fetches = Vec::new();
    for file in &pc.files {
        fetches.push(fetch(job, file, dest_of(&file.rel_path), &progress));
    }
    let mut left = stream::iter(fetches).buffer_unordered(PARALLEL_DOWNLOADS);
    while let Some(r) = left.next().await {
        r?;
    }
    drop(left);
    job.emit(rom_id, "download", total, total);

    // 3. Mise en place.
    match pc.kind {
        PcKind::Portable => {}
        PcKind::Archive | PcKind::Cod => {
            let zips: Vec<PathBuf> = pc.files.iter().map(|f| dest_of(&f.rel_path)).collect();
            let (gd, cancel, cb) = (game_dir.clone(), job.cancel.clone(), job.on_progress.clone());
            blocking(move || -> AppResult<()> {
                let staging = gd.join(INTERNAL_DIR).join("staging");
                for zip in zips {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    extract::extract(&zip, ArchiveKind::Zip, &staging, &cancel, |done, total| {
                        if last.elapsed() >= Duration::from_millis(200) || done == total {
                            last = Instant::now();
                            cb(crate::install::Progress { rom_id, phase: "extract", done, total, verifying: false });
                        }
                    })?;
                    let root = extract::content_root(&staging)?;
                    let n = extract::merge_into(&root, &gd)?;
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = std::fs::remove_file(&zip);
                    tracing::info!("{} : {n} fichier(s) posés", zip.display());
                }
                Ok(())
            })
            .await?;
        }
        PcKind::GogInstaller => {
            let setups: Vec<PathBuf> = gog::setup_files(&pc.files).iter().map(|f| dest_of(&f.rel_path)).collect();
            for setup in &setups {
                if job.cancel.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                // Taille installée ≈ taille des installateurs : sert d'estimation.
                job.emit(rom_id, "installer", installed_size(&game_dir).await, pc.size);
                // Pendant l'installation : place occupée par le jeu, toutes les 2 s.
                let install = run_installer(setup, &game_dir, runner.as_ref());
                tokio::pin!(install);
                let mut tick = tokio::time::interval(Duration::from_secs(2));
                loop {
                    tokio::select! {
                        r = &mut install => break r?,
                        _ = tick.tick() => job.emit(rom_id, "installer", installed_size(&game_dir).await, pc.size),
                    }
                }
            }
            // Les installateurs ne servent plus : place rendue tout de suite.
            let _ = std::fs::remove_dir_all(&downloads);
        }
    }
    if job.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }

    // 4. Client Call of Duty.
    if let Some(cod) = pc.cod {
        if let Some(r) = runner.as_ref() {
            let r = r.clone();
            blocking(move || r.ensure_prefix()).await?;
        }
        let dirs = ClientDirs { game_dir: game_dir.clone(), local_appdata: local_appdata(runner.as_ref()) };
        cod::sync_clients(cod, &dirs, &job.cancel, |done, total| job.emit(rom_id, "client", done, total)).await?;
    }

    // 5. Exécutable et manière de lancer.
    job.emit(rom_id, "finalize", 0, 0);
    if pc.os == PcOs::Linux {
        let gd = game_dir.clone();
        blocking(move || {
            make_all_executable(&gd);
            Ok(())
        })
        .await?;
    }
    let steam_app_id = match pc.kind {
        PcKind::Portable | PcKind::Archive => steam::ensure_app_id(&game_dir, &game.name).await,
        _ => None,
    };
    if let Some(r) = runner.as_mut() {
        r.game_id = steam_app_id.map(|id| format!("umu-{id}")).unwrap_or_else(|| "umu-default".into());
    }
    let task = (pc.kind == PcKind::GogInstaller).then(|| gog::play_task(&game_dir)).flatten();
    let run_target = if pc.os == PcOs::Windows { Target::Windows } else { job.target };
    let candidates: Vec<String> = match (&task, pc.cod) {
        (Some(t), _) => vec![t.exe.clone()],
        (None, Some(cod)) => find_top_ci(&game_dir, cod.main_exe()).into_iter().collect(),
        (None, None) => detect::find_candidates(&game_dir, run_target).iter().map(|p| path_to_rel(p)).collect(),
    };
    tracing::info!("exécutables candidats: {candidates:?}");
    let kept = previous
        .as_ref()
        .and_then(|p| p.executable.clone())
        .filter(|e| game_dir.join(rel_to_path(e)).exists() && (task.is_none() || candidates.contains(e)));
    let now = state::now();
    let mut st = InstalledGame {
        rom_id,
        name: game.name.clone(),
        version: game.server_version.clone(),
        version_raw: game.server_version_raw.clone(),
        target: job.target,
        archive_name: pc.files.first().map(|f| f.file_name.clone()).unwrap_or_default(),
        executable: None,
        pending_candidates: Vec::new(),
        roms: Vec::new(),
        rom_sources: Vec::new(),
        mods_downloaded: previous.as_ref().map(|p| p.mods_downloaded.clone()).unwrap_or_default(),
        saves: previous.as_ref().map(|p| p.saves.clone()).unwrap_or_default(),
        direct_launch: None,
        prefer_launcher: false,
        installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
        updated_at: now,
        game_dir: game_dir.clone(),
        pc: Some(PcInstall {
            kind: pc.kind,
            os: pc.os,
            cod: pc.cod,
            args: task.as_ref().map(|t| t.args.clone()).unwrap_or_default(),
            cwd: task.as_ref().and_then(|t| t.cwd.clone()),
            proton: proton_setup,
            steam_app_id,
        }),
    };
    let chosen = match (kept, candidates.len()) {
        (Some(e), _) => e,
        (None, 0) => return Err(AppError::NoExecutable),
        (None, 1) => candidates[0].clone(),
        (None, _) => {
            st.pending_candidates = candidates;
            state::write_state(&job.config_dir, &st)?;
            return Ok(InstallOutcome::NeedsChoice { game: st });
        }
    };
    let st = finalize(&job.config_dir, st, &chosen, &game).await?;
    Ok(InstallOutcome::Installed { game: st })
}

/// Avant une partie d'un Call of Duty : clients à jour (au mieux), puis
/// programme et arguments du mode choisi.
pub async fn prepare_cod(
    cod: CodGame,
    mode: cod::CodMode,
    online: bool,
    game_dir: &Path,
    runner: Option<&Runner>,
    player: &str,
) -> AppResult<(PathBuf, Vec<String>, Option<PathBuf>)> {
    let launch = cod.launch(mode).ok_or(AppError::Internal)?;
    if let Some(r) = runner {
        let r = r.clone();
        blocking(move || r.ensure_prefix()).await?;
    }
    let appdata = local_appdata(runner);
    let dirs = ClientDirs { game_dir: game_dir.to_path_buf(), local_appdata: appdata.clone() };
    if let Err(e) = cod::sync_clients(cod, &dirs, &CancellationToken::new(), |_, _| {}).await {
        // Hors ligne : on joue avec le client en place.
        tracing::warn!("clients de {cod:?} non vérifiés: {}", e.code());
    }
    match launch {
        cod::ModeLaunch::Exe { exe, args } => Ok((PathBuf::from(exe), args, None)),
        cod::ModeLaunch::Plutonium { game, path_key } => {
            let root = cod::plutonium_root(&appdata);
            update_plutonium(game_dir, &root, runner).await;
            let game_path = proton::windows_path(game_dir);
            cod::set_game_path(&root, path_key, &game_path)?;
            if online {
                let launcher = cod::launcher_exe(&root);
                if !launcher.is_file() {
                    return Err(AppError::PlutoniumMissing);
                }
                // Pas encore connecté : la fenêtre de Plutonium s'ouvre pour se connecter.
                let args = if cod::has_token(&root) { vec![format!("plutonium://play/{game}")] } else { Vec::new() };
                Ok((launcher, args, Some(root)))
            } else {
                let boot = cod::bootstrapper_exe(&root);
                if !boot.is_file() {
                    return Err(AppError::PlutoniumMissing);
                }
                Ok((boot, vec![game.to_string(), game_path, "-lan".into(), "+name".into(), player.to_string()], Some(root)))
            }
        }
    }
}

/// Met Plutonium à jour avec son installateur (`plutonium.exe`, posé dans le
/// jeu), comme CB Servers : sa fenêtre reste ouverte après coup, c'est la
/// révision écrite dans `info.json` qui signale la fin.
async fn update_plutonium(game_dir: &Path, root: &Path, runner: Option<&Runner>) {
    let Some(remote) = cod::remote_revision().await else {
        tracing::warn!("révision de Plutonium inconnue (hors ligne ?)");
        return;
    };
    if cod::local_revision(root) == Some(remote) {
        return;
    }
    let Some(updater) = find_top_ci(game_dir, "plutonium.exe").map(|n| game_dir.join(n)) else {
        tracing::warn!("plutonium.exe absent du jeu");
        return;
    };
    tracing::info!("mise à jour de Plutonium → révision {remote}");
    let args = vec!["-update-only".to_string(), "-no-self-update".to_string()];
    let mut cmd = match runner {
        Some(r) => r.command(&updater, &args),
        None => {
            let mut c = std::process::Command::new(&updater);
            c.args(&args);
            c
        }
    };
    cmd.current_dir(game_dir);
    crate::external::use_system_env(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("installateur de Plutonium non lancé: {e}");
            return;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    while Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if cod::local_revision(root) == Some(remote) {
            tracing::info!("Plutonium à jour");
            break;
        }
        if matches!(child.try_wait(), Ok(Some(_))) && runner.is_none() {
            break;
        }
    }
    kill_tree(&mut child);
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill").args(["-TERM", &format!("-{}", child.id())]).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
