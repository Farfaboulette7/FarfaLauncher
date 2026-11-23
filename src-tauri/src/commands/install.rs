//! Commandes d'installation. Une installation tourne en arrière-plan et pousse
//! `install-progress` puis `install-finished` à l'interface.
//!
//! File de téléchargements : une seule installation tourne à la fois ; les autres attendent
//! (`queued`, avec leur place) ou sont en pause (`paused`). Une pause annule l'installation en
//! cours sans effacer ce qui est téléchargé : elle reprend là où elle s'était arrêtée.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::install::{self, detect, path_to_rel, state, InstallJob, InstallOutcome, Progress, QueuedInstall};
use crate::pc::{cod, dosbox, proton};
use crate::platform;
use crate::romm::library::Game;
use crate::runtimes;
use crate::state::AppState;
use crate::error::LockOrRecover;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInfo {
    pub install_root: String,
    pub games: Vec<state::InstalledGame>,
    /// Jeux en cours d'installation.
    pub active: Vec<i64>,
    /// File d'attente, dans l'ordre.
    pub queue: Vec<QueueEntry>,
    /// Limite de débit des téléchargements en Mo/s (0 : illimité).
    pub download_limit_mb: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub rom_id: i64,
    pub paused: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallFinished {
    rom_id: i64,
    outcome: Option<InstallOutcome>,
    error: Option<AppError>,
}

#[tauri::command]
pub fn install_info(state: State<'_, AppState>) -> InstallInfo {
    InstallInfo {
        install_root: state.install_root().to_string_lossy().into_owned(),
        games: state::list_installed(&state.config_dir),
        active: state.installs.lock_or_recover().keys().copied().collect(),
        queue: state.queue.lock_or_recover().iter().map(|q| QueueEntry { rom_id: q.rom_id, paused: q.paused }).collect(),
        download_limit_mb: state.settings().download_limit_mb,
    }
}

/// `dest` : dossier où créer celui du jeu, pour cette installation seulement
/// (`None` = dossier des jeux des paramètres). `proton` : version de Proton
/// pour un jeu Windows sous Linux (`None` = la recommandée). `with_emulator` :
/// installer avec l'émulateur de la console plutôt que la recomp ; `emulator` : lequel (sinon
/// celui déjà utilisé, ou le premier). Changer d'émulateur garde la ROM déjà téléchargée.
#[tauri::command]
pub fn install_game(
    app: AppHandle,
    state: State<'_, AppState>,
    rom_id: i64,
    dest: Option<String>,
    proton: Option<String>,
    with_emulator: Option<bool>,
    emulator: Option<String>,
    recomp: Option<String>,
) -> AppResult<()> {
    // Jeu à plusieurs recomps : celle choisie (sinon celle proposée) reste la sienne, même si le
    // conseil du catalogue change plus tard.
    if with_emulator != Some(true) {
        let catalog = state.catalog();
        let choices = catalog.recomps_for_rom(rom_id);
        let wanted = recomp.filter(|id| choices.iter().any(|r| &r.id == id)).or_else(|| (choices.len() > 1).then(|| catalog.recomp_for_rom(rom_id).map(|r| r.id.clone())).flatten());
        if let Some(id) = wanted {
            if state.settings().recomp_choice.get(&rom_id) != Some(&id) {
                tracing::info!("recomp choisie pour le jeu {rom_id} : {id}");
                state.update_settings(|s| {
                    s.recomp_choice.insert(rom_id, id);
                })?;
                crate::runtimes::combine(&state);
            }
        }
    }
    let install_root = match dest.map(PathBuf::from) {
        Some(d) if d.is_absolute() => d,
        Some(_) => return Err(AppError::Internal),
        None => state.install_root(),
    };
    let manifest = state.manifest();
    let with_emulator = with_emulator == Some(true);
    if manifest.page(rom_id).is_some() && !with_emulator {
        // Le programme partagé peut être mis à jour : aucun de ses jeux ne doit tourner.
        if manifest.family(rom_id).iter().any(|id| state.running.lock_or_recover().contains(id)) {
            return Err(AppError::AlreadyRunning);
        }
    }
    if with_emulator {
        // Même chose pour tous les jeux installés sur des émulateurs : il peut être mis à jour.
        let running = state.running.lock_or_recover().clone();
        let joue_sur_emulateur = running.iter().any(|id| state::find(&state.config_dir, *id).and_then(|s| s.shared).is_some_and(|s| s.is_emulator()));
        if joue_sur_emulateur {
            return Err(AppError::AlreadyRunning);
        }
    }
    state.client()?;
    {
        let jobs = state.installs.lock_or_recover();
        if state.running.lock_or_recover().contains(&rom_id) {
            return Err(AppError::AlreadyRunning);
        }
        let mut queue = state.queue.lock_or_recover();
        if jobs.contains_key(&rom_id) || queue.iter().any(|q| q.rom_id == rom_id) || state.is_busy(rom_id) {
            return Err(AppError::Busy);
        }
        queue.push(QueuedInstall { rom_id, install_root, proton, with_emulator, emulator_id: emulator, paused: false });
    }
    emit_queue(&app);
    pump(&app);
    Ok(())
}

/// Place de chaque jeu de la file, envoyée à l'interface (`queued` ou `paused`).
fn emit_queue(app: &AppHandle) {
    let state = app.state::<AppState>();
    let queue = state.queue.lock_or_recover().clone();
    let total = queue.len() as u64;
    for (i, q) in queue.iter().enumerate() {
        let phase = if q.paused { "paused" } else { "queued" };
        let _ = app.emit("install-progress", Progress { rom_id: q.rom_id, phase, done: i as u64 + 1, total, verifying: false });
    }
}

/// Lance la prochaine installation de la file si aucune ne tourne.
fn pump(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(client) = state.client() else { return };
    let running = state.running.lock_or_recover().clone();
    let (entry, cancel) = {
        let mut jobs = state.installs.lock_or_recover();
        if !jobs.is_empty() {
            return;
        }
        let mut queue = state.queue.lock_or_recover();
        let Some(i) = install::queue_next(&queue, |id| running.contains(&id) || state.is_busy(id)) else { return };
        let entry = queue.remove(i);
        let cancel = CancellationToken::new();
        jobs.insert(entry.rom_id, cancel.clone());
        (entry, cancel)
    };
    emit_queue(app);
    let settings = state.settings();
    install::download::set_limit_mb(settings.download_limit_mb);
    let rom_id = entry.rom_id;
    tracing::info!("installation du jeu {rom_id} : début");
    let emitter = app.clone();
    let job = InstallJob {
        client,
        config_dir: state.config_dir.clone(),
        install_root: entry.install_root.clone(),
        target: platform::detect(),
        cancel,
        proton: entry.proton.clone(),
        manifest: state.manifest(),
        catalog: state.catalog(),
        manager_base: crate::manager::base_url(&settings),
        with_emulator: entry.with_emulator,
        emulator_id: entry.emulator_id.clone(),
        runtime_lock: state.runtime_lock.clone(),
        on_progress: Arc::new(move |p| {
            let _ = emitter.emit("install-progress", p);
        }),
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = job.run(rom_id).await;
        let state = app.state::<AppState>();
        state.installs.lock_or_recover().remove(&rom_id);
        let paused = state.pause_requested.lock_or_recover().remove(&rom_id);
        if paused && matches!(result, Err(AppError::Cancelled)) {
            // En pause : retour en tête de file, les fichiers déjà reçus sont gardés.
            tracing::info!("installation du jeu {rom_id} en pause");
            state.queue.lock_or_recover().insert(0, QueuedInstall { paused: true, ..entry });
            emit_queue(&app);
        } else {
            let payload = match result {
                Ok(outcome) => InstallFinished { rom_id, outcome: Some(outcome), error: None },
                Err(e) => {
                    if !matches!(e, AppError::Cancelled) {
                        tracing::error!("installation du jeu {rom_id} échouée: {}", e.code());
                    }
                    InstallFinished { rom_id, outcome: None, error: Some(e) }
                }
            };
            let _ = app.emit("install-finished", payload);
        }
        pump(&app);
    });
}

/// Retire un jeu de la file d'attente ; l'interface le voit comme une installation annulée.
fn drop_from_queue(app: &AppHandle, rom_id: i64) -> bool {
    let state = app.state::<AppState>();
    let removed = {
        let mut queue = state.queue.lock_or_recover();
        let before = queue.len();
        queue.retain(|q| q.rom_id != rom_id);
        queue.len() != before
    };
    if removed {
        let _ = app.emit("install-finished", InstallFinished { rom_id, outcome: None, error: Some(AppError::Cancelled) });
        emit_queue(app);
    }
    removed
}

#[tauri::command]
pub fn cancel_install(app: AppHandle, state: State<'_, AppState>, rom_id: i64) {
    let token = state.installs.lock_or_recover().get(&rom_id).cloned();
    if let Some(token) = token {
        tracing::info!("annulation de l'installation du jeu {rom_id}");
        state.pause_requested.lock_or_recover().remove(&rom_id);
        token.cancel();
    } else if drop_from_queue(&app, rom_id) {
        tracing::info!("jeu {rom_id} retiré de la file des téléchargements");
    }
}

/// Met en pause : l'installation en cours s'arrête (ce qui est téléchargé reste), une
/// installation en attente garde sa place sans démarrer.
#[tauri::command]
pub fn pause_install(app: AppHandle, state: State<'_, AppState>, rom_id: i64) {
    let token = state.installs.lock_or_recover().get(&rom_id).cloned();
    if let Some(token) = token {
        state.pause_requested.lock_or_recover().insert(rom_id);
        token.cancel();
        return;
    }
    if let Some(q) = state.queue.lock_or_recover().iter_mut().find(|q| q.rom_id == rom_id) {
        q.paused = true;
    }
    emit_queue(&app);
}

/// Reprend une installation en pause (à son tour dans la file).
#[tauri::command]
pub fn resume_install(app: AppHandle, state: State<'_, AppState>, rom_id: i64) {
    if let Some(q) = state.queue.lock_or_recover().iter_mut().find(|q| q.rom_id == rom_id) {
        q.paused = false;
    }
    emit_queue(&app);
    pump(&app);
}

/// Monte (`up`) ou descend un jeu d'une place dans la file d'attente.
#[tauri::command]
pub fn move_install(app: AppHandle, state: State<'_, AppState>, rom_id: i64, up: bool) {
    let moved = install::queue_move(&mut state.queue.lock_or_recover(), rom_id, up);
    if moved {
        emit_queue(&app);
    }
}

/// Limite de débit des téléchargements en Mo/s (0 : illimité), appliquée tout de suite.
#[tauri::command]
pub fn set_download_limit(state: State<'_, AppState>, mb: u32) -> AppResult<()> {
    state.update_settings(|s| s.download_limit_mb = mb)?;
    install::download::set_limit_mb(mb);
    tracing::info!("limite de débit des téléchargements : {} Mo/s (0 = aucune)", mb);
    Ok(())
}

/// Système des exécutables d'un jeu : un jeu Windows sous Proton a des `.exe`.
fn exe_target(st: &state::InstalledGame) -> platform::Target {
    match &st.pc {
        Some(pc) if pc.os == crate::pc::PcOs::Windows => platform::Target::Windows,
        _ => platform::detect(),
    }
}

/// Candidats exécutables d'un jeu installé (pour le changer depuis la fiche).
#[tauri::command]
pub fn executable_candidates(state: State<'_, AppState>, rom_id: i64) -> AppResult<Vec<String>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    Ok(detect::find_candidates(&st.game_dir, exe_target(&st)).iter().map(|p| path_to_rel(p)).collect())
}

/// Choix de l'exécutable : termine une installation en attente, ou change
/// l'exécutable d'un jeu déjà installé.
#[tauri::command]
pub async fn choose_executable(state: State<'_, AppState>, rom_id: i64, executable: String) -> AppResult<state::InstalledGame> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let candidates: Vec<String> = detect::find_candidates(&st.game_dir, exe_target(&st)).iter().map(|p| path_to_rel(p)).collect();
    if !candidates.contains(&executable) {
        return Err(AppError::NoExecutable);
    }
    if st.executable.is_none() {
        let game = game_details(&state, rom_id).await?;
        install::finalize(&state.config_dir, st, &executable, &game).await
    } else {
        install::change_executable(&state.config_dir, st, &executable).await
    }
}

async fn game_details(state: &AppState, rom_id: i64) -> AppResult<Game> {
    let cached = state
        .library
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.games.iter().find(|g| g.id == rom_id && g.details_loaded).cloned());
    match cached {
        Some(g) => Ok(g),
        None => runtimes::fetch_details(&state.client()?, &state.manifest(), &state.catalog(), rom_id, platform::detect()).await,
    }
}

#[tauri::command]
pub async fn uninstall_game(app: AppHandle, state: State<'_, AppState>, rom_id: i64, keep_saves: bool) -> AppResult<()> {
    let _busy = state.mark_busy(rom_id)?;
    // Mise à jour en attente : elle n'a plus lieu d'être.
    drop_from_queue(&app, rom_id);
    state.check_family_idle(rom_id)?;
    state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    // Toutes les façons dont le jeu est installé partent : celle en service, puis celles en
    // réserve, qui passent en service l'une après l'autre.
    while let Some(st) = state::find(&state.config_dir, rom_id) {
        // Recomp qui avait relié un dossier hors du jeu (boiii) : le lien part avec le jeu.
        if let Some(r) = state.catalog().recomp_for_install(&st) {
            crate::links::remove_into(&r.links, &st.game_dir);
        }
        install::uninstall(&state.config_dir, st, keep_saves).await?;
    }
    Ok(())
}

/// Joue désormais avec une autre façon déjà installée (recomp ou émulateur), sans rien
/// retélécharger. Celle d'avant reste sur le disque, en réserve.
#[tauri::command]
pub fn use_install(state: State<'_, AppState>, rom_id: i64, kind: String, id: String) -> AppResult<state::InstalledGame> {
    let _busy = state.mark_busy(rom_id)?;
    if state.running.lock_or_recover().contains(&rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    let st = state::activate(&state.config_dir, rom_id, &kind, &id)?;
    // Recomp : elle devient celle du jeu (mises à jour, sauvegardes, lancement).
    if kind == "recomp" && state.catalog().recomps_for_rom(rom_id).iter().any(|r| r.id == id) && state.settings().recomp_choice.get(&rom_id) != Some(&id) {
        state.update_settings(|s| {
            s.recomp_choice.insert(rom_id, id.clone());
        })?;
        runtimes::combine(&state);
    }
    tracing::info!("« {} » se joue maintenant avec {kind} {id}", st.name);
    Ok(st)
}

/// Déplace un jeu installé : `dest` est le dossier qui contiendra le sien.
#[tauri::command]
pub async fn move_game(state: State<'_, AppState>, rom_id: i64, dest: String) -> AppResult<state::InstalledGame> {
    let dest = PathBuf::from(dest);
    if !dest.is_absolute() {
        return Err(AppError::Internal);
    }
    let _busy = state.mark_busy(rom_id)?;
    state.check_family_idle(rom_id)?;
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    // Un émulateur partage ses chemins de ROM et de sauvegardes avec ses jeux : il ne se déplace pas.
    if st.shared.as_ref().is_some_and(|s| s.is_emulator()) {
        return Err(AppError::Internal);
    }
    install::move_game(&state.config_dir, st, dest).await
}

#[tauri::command]
pub fn open_game_folder(app: AppHandle, state: State<'_, AppState>, rom_id: i64, sub: Option<String>) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let mut dir: PathBuf = st.game_dir.clone();
    if let Some(sub) = sub.as_deref() {
        // Seul sous-dossier autorisé : celui des mods téléchargés.
        if sub != install::extract::MODS_DIR {
            return Err(AppError::Internal);
        }
        dir = dir.join(sub);
        std::fs::create_dir_all(&dir).map_err(|e| log_err(AppError::Disk, "création du dossier des mods", e))?;
    }
    crate::external::open_path(&app, &dir.to_string_lossy(), "ouverture du dossier du jeu")
}

/// Dossier des mods du jeu installé (`mods` ou `mod`, au plus près de la racine), s'il existe.
fn find_mods_dir(game_dir: &std::path::Path) -> Option<PathBuf> {
    let mut level = vec![game_dir.to_path_buf()];
    for _ in 0..4 {
        let mut next = Vec::new();
        for dir in &level {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            let mut subs: Vec<PathBuf> = rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
            subs.sort();
            for sub in subs {
                let name = sub.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                if name == "mods" || name == "mod" {
                    return Some(sub);
                }
                // Dossier interne du launcher : jamais exploré.
                if !name.starts_with('.') {
                    next.push(sub);
                }
            }
        }
        level = next;
    }
    None
}

/// Dossier des mods d'un jeu installé : celui de la fiche (documentation de la recomp ou de
/// l'émulateur, `dossier-mods:`), sinon un dossier `mods` trouvé dans le jeu.
fn mods_dir_of(state: &AppState, st: &state::InstalledGame) -> Option<PathBuf> {
    let catalog = state.catalog();
    let Some(pattern) = catalog.mods_dir_for_install(st) else { return find_mods_dir(&st.game_dir) };
    let exe_dir = crate::report::exe_dir(&catalog, st);
    resolve_mods_dir(&pattern, &exe_dir, || crate::switch_saves::title_id(&catalog, &state.config_dir, st))
}

/// `user/load/{titleid}` → `<dossier de l'exe>/user/load/0100…` ; sans ID de titre connu, le
/// dossier qui le contient (`user/load`).
fn resolve_mods_dir(pattern: &str, exe_dir: &std::path::Path, title: impl FnOnce() -> Option<String>) -> Option<PathBuf> {
    let mut pattern = pattern.to_string();
    if pattern.contains("{titleid}") {
        pattern = match title() {
            Some(t) => pattern.replace("{titleid}", &t.to_uppercase()),
            None => pattern.split('/').take_while(|p| *p != "{titleid}").collect::<Vec<_>>().join("/"),
        };
    }
    if pattern.starts_with('{') {
        crate::language::expand(&pattern, exe_dir)
    } else if pattern.is_empty() || pattern.split('/').any(|p| p.is_empty() || p == "..") {
        None
    } else {
        Some(pattern.split('/').fold(exe_dir.to_path_buf(), |p, part| p.join(part)))
    }
}

/// Le jeu installé a-t-il un dossier de mods ?
#[tauri::command]
pub fn has_mods_folder(state: State<'_, AppState>, rom_id: i64) -> bool {
    mods_folder(state, rom_id).is_some()
}

/// Dossier des mods du jeu installé (affiché dans Informations › Installation).
#[tauri::command]
pub fn mods_folder(state: State<'_, AppState>, rom_id: i64) -> Option<String> {
    let st = state::find(&state.config_dir, rom_id)?;
    mods_dir_of(&state, &st).map(|d| d.to_string_lossy().into_owned())
}

/// Ouvre le dossier des mods du jeu installé (créé s'il n'existe pas encore).
#[tauri::command]
pub fn open_mods_folder(app: AppHandle, state: State<'_, AppState>, rom_id: i64) -> AppResult<()> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let dir = mods_dir_of(&state, &st).ok_or(AppError::NotInstalled)?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("dossier des mods non créé ({}): {e}", dir.display());
    }
    crate::external::open_path(&app, &dir.to_string_lossy(), "ouverture du dossier des mods")
}

#[cfg(test)]
mod mods_dir_tests {
    use super::resolve_mods_dir;
    use std::path::Path;

    #[test]
    fn mods_dir_of_the_documentation() {
        let exe = Path::new("jeu");
        assert_eq!(resolve_mods_dir("mods", exe, || None), Some(Path::new("jeu").join("mods")));
        assert_eq!(resolve_mods_dir("user/load/{titleid}", exe, || Some("0100f2c0115b6000".into())), Some(exe.join("user").join("load").join("0100F2C0115B6000")));
        assert_eq!(resolve_mods_dir("user/load/{titleid}", exe, || None), Some(exe.join("user").join("load")));
        assert_eq!(resolve_mods_dir("../x", exe, || None), None);
        assert_eq!(resolve_mods_dir("{exedir}/mods", exe, || None), Some(exe.join("mods")));
    }
}

/// Versions de Proton installées et téléchargeables (la recommandée en premier).
#[tauri::command]
pub async fn proton_versions(state: State<'_, AppState>) -> AppResult<Vec<proton::ProtonVersion>> {
    Ok(proton::list(&proton::tools_dir(&state.config_dir)).await)
}

/// Change la version de Proton d'un jeu installé (téléchargée au prochain lancement).
#[tauri::command]
pub fn set_proton_version(state: State<'_, AppState>, rom_id: i64, version: Option<String>) -> AppResult<state::InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let setup = st.pc.as_mut().and_then(|p| p.proton.as_mut()).ok_or(AppError::Internal)?;
    setup.version = version;
    state::write_state(&state.config_dir, &st)?;
    Ok(st)
}

/// Touches personnalisées d'un jeu DOS (appliquées au prochain lancement).
#[tauri::command]
pub fn set_key_binds(state: State<'_, AppState>, rom_id: i64, keys: Vec<dosbox::KeyBind>) -> AppResult<state::InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    st.pc.as_mut().ok_or(AppError::Internal)?.keys = keys;
    state::write_state(&state.config_dir, &st)?;
    Ok(st)
}

/// Nombre de joueurs dont le launcher sait poser la manette pour ce jeu installé
/// (0 : l'émulateur ou la recomp ne le permet pas).
#[tauri::command]
pub fn controller_ports(state: State<'_, AppState>, rom_id: i64) -> AppResult<usize> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    Ok(state.catalog().controllers_for_install(&st).map(|f| crate::controllers::ports(&f)).unwrap_or(0))
}

/// Manette de chaque joueur (appliquée au prochain lancement).
#[tauri::command]
pub fn set_players(
    state: State<'_, AppState>,
    rom_id: i64,
    players: Vec<Option<crate::controllers::PlayerPad>>,
) -> AppResult<state::InstalledGame> {
    let mut st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let mut players = players;
    while players.last().is_some_and(Option::is_none) {
        players.pop();
    }
    st.players = players;
    state::write_state(&state.config_dir, &st)?;
    Ok(st)
}

/// Modes de jeu d'un Call of Duty installé.
#[tauri::command]
pub fn cod_modes(state: State<'_, AppState>, rom_id: i64) -> AppResult<Vec<cod::ModeInfo>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    Ok(st.pc.and_then(|p| p.cod).map(|c| c.modes()).unwrap_or_default())
}

/// Black Ops III installé : son dossier.
fn bo3_dir(state: &AppState, rom_id: i64) -> AppResult<std::path::PathBuf> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    match st.pc.and_then(|p| p.cod) {
        Some(cod::CodGame::Bo3) => Ok(st.game_dir),
        _ => Err(AppError::Internal),
    }
}

/// Langue en place de Black Ops III (`fr` ou `en`).
#[tauri::command]
pub fn bo3_language(state: State<'_, AppState>, rom_id: i64) -> AppResult<&'static str> {
    Ok(cod::bo3_language(&bo3_dir(&state, rom_id)?))
}

/// Met Black Ops III en `fr` ou en `en` (renommages des zones, instantané).
#[tauri::command]
pub async fn set_bo3_language(state: State<'_, AppState>, rom_id: i64, lang: String) -> AppResult<&'static str> {
    if !cod::BO3_LANGUAGES.contains(&lang.as_str()) {
        return Err(AppError::Internal);
    }
    if state.running.lock_or_recover().contains(&rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    let dir = bo3_dir(&state, rom_id)?;
    tokio::task::spawn_blocking(move || -> AppResult<&'static str> {
        cod::repair_bo3_zone(&dir).map_err(|e| crate::error::log_err(AppError::Disk, "zone/ de Black Ops III", e))?;
        cod::set_bo3_language(&dir, &lang).map_err(|e| crate::error::log_err(AppError::Disk, "langue de Black Ops III", e))?;
        Ok(cod::bo3_language(&dir))
    })
    .await
    .map_err(|_| AppError::Internal)?
}

/// Langue choisie pour un jeu installé dont la recomp ou l'émulateur propose l'anglais
/// (`langue:` avec une valeur en anglais, OpenSmash) : `fr` ou `en`. `None` : pas de choix.
#[tauri::command]
pub fn game_language(state: State<'_, AppState>, rom_id: i64) -> AppResult<Option<&'static str>> {
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    let rules = state.catalog().language_for_install(&st);
    Ok(crate::language::offers_english(&rules).then(|| crate::language::choice(&st.game_dir)))
}

/// Choisit `fr` ou `en` ; la langue est posée au prochain lancement.
#[tauri::command]
pub fn set_game_language(state: State<'_, AppState>, rom_id: i64, lang: String) -> AppResult<&'static str> {
    if !matches!(lang.as_str(), "fr" | "en") {
        return Err(AppError::Internal);
    }
    if state.running.lock_or_recover().contains(&rom_id) {
        return Err(AppError::AlreadyRunning);
    }
    let st = state::find(&state.config_dir, rom_id).ok_or(AppError::NotInstalled)?;
    crate::language::set_choice(&st.game_dir, &lang).map_err(|e| crate::error::log_err(AppError::Disk, "langue du jeu", e))?;
    Ok(crate::language::choice(&st.game_dir))
}

#[tauri::command]
pub fn set_install_dir(state: State<'_, AppState>, path: Option<String>) -> AppResult<String> {
    let dir = path.map(PathBuf::from).filter(|p| p.is_absolute());
    state.update_settings(|s| s.install_dir = dir)?;
    Ok(state.install_root().to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_the_mods_folder_nearest_the_root() {
        let dir = std::env::temp_dir().join(format!("farfa-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(super::find_mods_dir(&dir).is_none());
        std::fs::create_dir_all(dir.join("a/b/Mods")).unwrap();
        std::fs::create_dir_all(dir.join(".farfalauncher/mods")).unwrap();
        assert_eq!(super::find_mods_dir(&dir).unwrap(), dir.join("a/b/Mods"));
        std::fs::create_dir_all(dir.join("game/mods")).unwrap();
        assert_eq!(super::find_mods_dir(&dir).unwrap(), dir.join("game/mods"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
