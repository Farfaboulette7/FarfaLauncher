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
use crate::error::LockOrRecover;
// Arrêt d'un programme et de ses sous-programmes : une seule fonction pour tout le launcher.
use crate::launch::kill_tree;

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

/// Place nécessaire avant de télécharger. Les archives sont extraites une à une et chacune est
/// supprimée aussitôt : il faut tous les fichiers, plus l'extraction de la plus grosse.
fn needed_space(pc: &PcGame) -> u64 {
    let factor = space_factor(pc.kind);
    match pc.kind {
        PcKind::Archive | PcKind::Cod => {
            let largest = pc.files.iter().map(|f| f.size).max().unwrap_or(0);
            pc.size + (largest as f64 * (factor - 1.0)) as u64
        }
        _ => (pc.size as f64 * factor) as u64,
    }
}

/// Accès à FarfaRomManager, d'où vient le client de Black Ops III.
pub struct ManagerAccess<'a> {
    pub client: &'a crate::romm::client::RommClient,
    pub base: Option<&'a str>,
    pub catalog: &'a crate::manager::Catalog,
}

/// Fichier qui retient le client boiii en place (son SHA-1).
const BOIII_MARKER: &str = ".farfa-client";

/// Black Ops III : boiii (exe + `boiii/data`) depuis FarfaRomManager, posé dans le jeu s'il a
/// changé ; puis `%LOCALAPPDATA%\boiii` (celui du préfixe sous Proton) relié à `jeu/boiii`,
/// car boiii y écrit toujours. Gestionnaire injoignable : le client en place est gardé.
pub async fn sync_boiii(
    access: &ManagerAccess<'_>,
    game_dir: &Path,
    local_appdata: &Path,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64),
) -> AppResult<()> {
    let installed = game_dir.join("boiii.exe").is_file();
    let file = access.catalog.recomp(cod::BOIII_CLIENT).and_then(|r| r.archives(Target::Windows).1).map(|a| a.file);
    let marker = game_dir.join("boiii").join(BOIII_MARKER);
    match file {
        None if installed => tracing::warn!("boiii absent du catalogue : client en place gardé"),
        None => {
            tracing::error!("boiii absent du catalogue de FarfaRomManager (fiche « {} »)", cod::BOIII_CLIENT);
            return Err(AppError::NotAvailable);
        }
        Some(f) if std::fs::read_to_string(&marker).ok().as_deref() == f.sha1.as_deref() && installed => {}
        Some(f) => {
            let zip = game_dir.join(INTERNAL_DIR).join("downloads").join("boiii-client.zip");
            let size = f.size;
            if let Err(e) = download::download_any(access.client, access.base, &f, &zip, cancel, |got| on_progress(got, size), || {}).await {
                if installed {
                    tracing::warn!("boiii non mis à jour ({}), client en place gardé", e.code());
                } else {
                    return Err(e);
                }
            } else {
                let (gd, z, sha1, cancel) = (game_dir.to_path_buf(), zip.clone(), f.sha1.clone().unwrap_or_default(), cancel.clone());
                blocking(move || -> AppResult<()> {
                    let staging = gd.join(INTERNAL_DIR).join("staging-client");
                    let _ = std::fs::remove_dir_all(&staging);
                    extract::extract(&z, ArchiveKind::Zip, &staging, &cancel, |_, _| {})?;
                    let root = extract::content_root(&staging)?;
                    // Sans casse : les profils et clés du joueur (boiii/user, boiii_players) restent.
                    extract::merge_into(&root, &gd)?;
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = std::fs::remove_file(&z);
                    std::fs::write(gd.join("boiii").join(BOIII_MARKER), sha1).map_err(|e| download::disk_error("client boiii", e))?;
                    Ok(())
                })
                .await?;
                tracing::info!("boiii {} posé", f.file_name);
            }
        }
    }
    let (link, target) = (local_appdata.join("boiii"), game_dir.join("boiii"));
    blocking(move || {
        match crate::links::place(&link, &target) {
            Ok(true) => tracing::info!("lien {} -> {}", link.display(), target.display()),
            Ok(false) => {}
            Err(e) => tracing::warn!("lien {} non posé: {e}", link.display()),
        }
        Ok(())
    })
    .await
}

/// `%LOCALAPPDATA%` vu par le jeu (celui du préfixe sous Proton).
pub fn local_appdata(runner: Option<&Runner>) -> PathBuf {
    match runner {
        Some(r) => r.local_appdata(),
        None => std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default(),
    }
}

/// Racine d'un morceau d'archive : dans le dossier enveloppe du premier morceau
/// s'il en avait un et que ce morceau l'a aussi, sinon le morceau tel quel.
fn chunk_root(staging: &Path, wrapper: &Path) -> PathBuf {
    let inside = staging.join(wrapper);
    if !wrapper.as_os_str().is_empty() && inside.is_dir() {
        inside
    } else {
        staging.to_path_buf()
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
        let mut last = self.last.lock_or_recover();
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
    ensure_space(&game_dir, needed_space(&pc))?;
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
                // Dossier enveloppe décidé par le premier morceau seulement : un morceau qui ne
                // contient que `zone/` (Call of Duty en morceaux) ne doit pas être aplati.
                let mut wrapper: Option<PathBuf> = None;
                for zip in zips {
                    let mut last = Instant::now() - Duration::from_secs(1);
                    extract::extract(&zip, ArchiveKind::Zip, &staging, &cancel, |done, total| {
                        if last.elapsed() >= Duration::from_millis(200) || done == total {
                            last = Instant::now();
                            cb(crate::install::Progress { rom_id, phase: "extract", done, total, verifying: false });
                        }
                    })?;
                    let root = match &wrapper {
                        None => {
                            let root = extract::content_root(&staging)?;
                            wrapper = Some(root.strip_prefix(&staging).map(Path::to_path_buf).unwrap_or_default());
                            root
                        }
                        Some(w) => chunk_root(&staging, w),
                    };
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
        if cod == CodGame::Bo3 {
            let access = ManagerAccess { client: &job.client, base: job.manager_base.as_deref(), catalog: &job.catalog };
            sync_boiii(&access, &game_dir, &dirs.local_appdata, &job.cancel, |done, total| job.emit(rom_id, "client", done, total)).await?;
        } else {
            cod::sync_clients(cod, &dirs, &job.cancel, |done, total| job.emit(rom_id, "client", done, total)).await?;
        }
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
        players: Vec::new(),
        installed_at: previous.as_ref().map(|p| p.installed_at).unwrap_or(now),
        updated_at: now,
        game_dir: game_dir.clone(),
        shared: None,
        ways: Vec::new(),
        pc: Some(PcInstall {
            kind: pc.kind,
            os: pc.os,
            cod: pc.cod,
            args: task.as_ref().map(|t| t.args.clone()).unwrap_or_default(),
            cwd: task.as_ref().and_then(|t| t.cwd.clone()),
            proton: proton_setup,
            steam_app_id,
            keys: previous_pc.map(|p| p.keys).unwrap_or_default(),
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
    access: &ManagerAccess<'_>,
) -> AppResult<(PathBuf, Vec<String>, Option<PathBuf>)> {
    let launch = cod.launch(mode).ok_or(AppError::Internal)?;
    if let Some(r) = runner {
        let (r, gd) = (r.clone(), game_dir.to_path_buf());
        blocking(move || {
            r.ensure_prefix()?;
            // boiii exige Steam dans le préfixe, que Proton ne pose pas sous umu.
            if cod == CodGame::Bo3 {
                if let Err(e) = r.ensure_steam_marker(&gd) {
                    tracing::warn!("Steam non préparé dans le préfixe: {}", e.code());
                }
            }
            Ok(())
        })
        .await?;
    }
    let appdata = local_appdata(runner);
    let dirs = ClientDirs { game_dir: game_dir.to_path_buf(), local_appdata: appdata.clone() };
    let synced = if cod == CodGame::Bo3 {
        sync_boiii(access, game_dir, &appdata, &CancellationToken::new(), |_, _| {}).await
    } else {
        cod::sync_clients(cod, &dirs, &CancellationToken::new(), |_, _| {}).await
    };
    if let Err(e) = synced {
        // Hors ligne : on joue avec le client en place.
        tracing::warn!("clients de {cod:?} non vérifiés: {}", e.code());
    }
    if cod == CodGame::Bo3 {
        if let Err(e) = set_boiii_name(game_dir, player) {
            tracing::warn!("pseudo non écrit pour boiii: {}", e.code());
        }
        // Installations aplaties (avant la 0.9.8) : zone/ rangé avant de jouer.
        let gd = game_dir.to_path_buf();
        blocking(move || -> AppResult<()> {
            if let Err(e) = cod::repair_bo3_zone(&gd) {
                tracing::warn!("zone/ de Black Ops III non rangé: {e}");
            }
            // Zones renommées par les versions 1.0 et 1.1 bêta : le jeu plantait en français.
            if let Err(e) = cod::restore_bo3_zones(&gd) {
                tracing::warn!("zones de Black Ops III non remises en place: {e}");
            }
            Ok(())
        })
        .await?;
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

/// Pseudo du joueur du launcher pour boiii : `playerName` de
/// `boiii_players/properties.json` (lu au démarrage, sinon boiii prend le nom du
/// compte Windows, « steamuser » sous Proton). Les autres réglages restent.
fn set_boiii_name(game_dir: &Path, player: &str) -> AppResult<()> {
    // boiii ne garde que l'ASCII imprimable.
    let name: String = player.chars().filter(|c| (' '..='~').contains(c)).collect();
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let file = game_dir.join("boiii_players").join("properties.json");
    let mut props = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    if props.get("playerName").and_then(|v| v.as_str()) == Some(name) {
        return Ok(());
    }
    props["playerName"] = serde_json::Value::String(name.to_string());
    std::fs::create_dir_all(file.parent().unwrap_or(game_dir)).map_err(|e| log_err(AppError::Disk, "dossier boiii_players", e))?;
    let text = serde_json::to_string_pretty(&props).map_err(|_| AppError::Internal)?;
    std::fs::write(&file, text).map_err(|e| log_err(AppError::Disk, "properties.json de boiii", e))?;
    tracing::info!("pseudo « {name} » écrit pour boiii");
    Ok(())
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
        // Installateur fermé : on arrête d'attendre. Sous Proton, le processus lancé (le script
        // de Proton) peut finir avant le programme Windows : on attend alors que tout son groupe
        // de processus soit fini (L1 de docs/audit-2026-11.md).
        if matches!(child.try_wait(), Ok(Some(_))) && !group_alive(child.id()) {
            tracing::info!("installateur de Plutonium fermé avant la fin de la mise à jour");
            break;
        }
    }
    kill_tree(&mut child);
}

/// Un processus du groupe `pgid` tourne encore (Linux, Mac : le groupe créé par
/// `process_group(0)`). Toujours faux sous Windows, où Proton n'existe pas.
fn group_alive(pgid: u32) -> bool {
    // Linux : lecture de /proc, en ignorant les zombies (comme le suivi des parties).
    #[cfg(target_os = "linux")]
    {
        crate::launch::group_alive(pgid)
    }
    // Mac : `kill -0` n'envoie rien, il dit seulement si un processus du groupe existe. Le `--`
    // est indispensable : sans lui, `-1234` serait lu comme une option.
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        std::process::Command::new("kill")
            .args(["-0", "--", &format!("-{pgid}")])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pgid;
        false
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_their_folders() {
        let staging = std::env::temp_dir().join(format!("farfa-morceau-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(staging.join("zone")).unwrap();
        // Premier morceau sans enveloppe : un morceau « zone/ » seul reste dans zone/.
        assert_eq!(chunk_root(&staging, Path::new("")), staging);
        // Premier morceau dans « Jeu/ » : les suivants aussi, s'ils l'ont.
        assert_eq!(chunk_root(&staging, Path::new("Jeu")), staging);
        std::fs::create_dir_all(staging.join("Jeu")).unwrap();
        assert_eq!(chunk_root(&staging, Path::new("Jeu")), staging.join("Jeu"));
        std::fs::remove_dir_all(staging).unwrap();
    }

    // Fin de l'installateur de Plutonium (L1) : un groupe de processus vivant est vu vivant,
    // puis fini une fois tous ses processus arrêtés.
    #[cfg(unix)]
    #[test]
    fn group_alive_follows_the_process_group() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("sleep").arg("30").process_group(0).spawn().unwrap();
        assert!(group_alive(child.id()));
        kill_tree(&mut child);
        assert!(!group_alive(child.id()));
    }

    // Sous Windows, pas de groupe à suivre : la fin du processus suffit.
    #[cfg(windows)]
    #[test]
    fn group_alive_is_false_on_windows() {
        assert!(!group_alive(std::process::id()));
    }

    #[test]
    fn boiii_name_keeps_other_settings() {
        let dir = std::env::temp_dir().join(format!("farfa-boiii-{}", uuid::Uuid::new_v4()));
        let file = dir.join("boiii_players").join("properties.json");
        set_boiii_name(&dir, "Farfa").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&file).unwrap()).unwrap()["playerName"], "Farfa");
        std::fs::write(&file, r#"{"playerName":"steamuser","fov":90}"#).unwrap();
        set_boiii_name(&dir, "Éloïse 7").unwrap(); // hors ASCII retiré, comme boiii
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["playerName"], "lose 7");
        assert_eq!(v["fov"], 90);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
