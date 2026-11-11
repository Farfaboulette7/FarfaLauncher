//! Tour du Mac (Apple Silicon) sur le vrai serveur : chaque émulateur et chaque recomp
//! qui a une build Mac est installé dans un dossier temporaire, lancé comme le fait le
//! launcher (langue, manettes, programme du .app), photographié, puis fermé.
//!
//! `FARFA_E2E_URL`, `FARFA_E2E_TOKEN`, `FARFA_E2E_MANAGER`, `FARFA_E2E_MAC_ROMS`
//! (« duckstation:77,pcsx2:10404… » : jeu RomM de chaque émulateur), `FARFA_E2E_OUT`
//! (dossier des captures et du compte rendu), puis
//! `cargo test --lib -- --ignored mac_tour_e2e --nocapture`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{rel_to_path, state, InstallJob, InstallOutcome};
use crate::manager::Catalog;
use crate::romm::client::RommClient;
use crate::runtimes::Manifest;

/// Temps laissé au jeu avant la capture.
const BOOT: Duration = Duration::from_secs(40);

struct Verdict {
    name: String,
    detail: String,
    ok: bool,
}

fn capture(out: &Path, name: &str) -> PathBuf {
    let file = out.join(format!("{}.png", name.replace(['/', ' ', ':'], "_")));
    let _ = std::process::Command::new("screencapture").arg("-x").arg(&file).status();
    file
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false)
}

/// Lance le jeu installé comme `play_game` (sans menu ni Proton), capture, ferme.
async fn play(catalog: &Catalog, st: &state::InstalledGame, out: &Path, label: &str) -> (bool, String) {
    let exe = st.executable.clone().unwrap_or_default();
    let direct = catalog
        .emulator_for_install(st)
        .and_then(|e| super::emulator::refreshed_launch(e, st))
        .or_else(|| catalog.recomp_launch(st))
        .or_else(|| st.direct_launch.clone());
    let program = direct.as_ref().map(|d| d.exe.clone()).unwrap_or(exe.clone());
    let exe_dir = match catalog.emulator_for_install(st) {
        Some(e) => e.data_dir(&st.game_dir, &program),
        None => st.game_dir.join(rel_to_path(&program)).parent().map(Path::to_path_buf).unwrap_or_else(|| st.game_dir.clone()),
    };
    let mut env: Vec<(String, String)> = direct.as_ref().map(|d| d.env.clone()).unwrap_or_default();
    let rules = catalog.language_for_install(st);
    env.extend(crate::language::prepare(&st.game_dir, &exe_dir, &rules));
    if let Some(e) = catalog.emulator_for_install(st) {
        crate::launch::before::clean(&e.launch.clean, &exe_dir);
    }
    let args = direct.as_ref().map(|d| d.args.clone()).unwrap_or_default();
    let launch = crate::launch::Launch {
        exe: rel_to_path(&program),
        args: &args,
        env: &env,
        cwd: direct.as_ref().and_then(|d| d.cwd.as_deref()).map(rel_to_path),
        learn: false,
        runner: None,
    };
    let running = match crate::launch::spawn(&st.game_dir, launch) {
        Ok(r) => r,
        Err(e) => return (false, format!("lancement impossible : {e:?}")),
    };
    let pid = running.pid();
    let stop = running.stop_flag();
    let waiter = tokio::task::spawn_blocking(move || running.wait(|| {}));
    tokio::time::sleep(BOOT).await;
    let up = alive(pid);
    let shot = capture(out, label);
    stop.store(true, Ordering::Relaxed);
    let _ = tokio::time::timeout(Duration::from_secs(30), waiter).await;
    let log = std::fs::read_to_string(st.game_dir.join(super::extract::INTERNAL_DIR).join("dernier-lancement.log")).unwrap_or_default();
    let tail: String = log.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ⏎ ");
    let detail = format!(
        "programme {program}, args {args:?}, {} au bout de {} s, capture {} ; fin du journal : {tail}",
        if up { "toujours ouvert" } else { "FERMÉ" },
        BOOT.as_secs(),
        shot.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    );
    (up, detail)
}

#[tokio::test]
#[ignore]
async fn mac_tour_e2e() {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    // Journal du launcher dans la sortie du test : les erreurs d'installation y sont détaillées.
    let _ = tracing_subscriber::fmt().with_env_filter("info").with_test_writer().try_init();
    let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
    let out = PathBuf::from(var("FARFA_E2E_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let base = std::env::temp_dir().join(format!("farfa-mac-{}", uuid::Uuid::new_v4()));
    let config = base.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
    let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
    let target = crate::platform::detect();
    println!("Mac détecté : {target:?}");
    let job = |emulator_id: Option<String>| InstallJob {
        client: client.clone(),
        config_dir: config.clone(),
        install_root: base.join("Jeux"),
        target,
        cancel: CancellationToken::new(),
        proton: None,
        manifest: Manifest::default().with_catalog(&catalog),
        catalog: catalog.clone(),
        manager_base: Some(var("FARFA_E2E_MANAGER")),
        with_emulator: emulator_id.is_some(),
        emulator_id,
        runtime_lock: Arc::new(tokio::sync::Mutex::new(())),
        on_progress: Arc::new(|_| {}),
    };
    let mut verdicts: Vec<Verdict> = Vec::new();

    // Émulateurs : ceux qui ont une build Mac et un jeu donné.
    let roms: Vec<(String, i64)> = var("FARFA_E2E_MAC_ROMS")
        .split(',')
        .filter_map(|p| p.split_once(':').and_then(|(e, r)| Some((e.trim().to_string(), r.trim().parse().ok()?))))
        .collect();
    // `FARFA_E2E_ONLY` : seulement les éléments dont le nom contient ce texte (« Recomp »…).
    let only = var("FARFA_E2E_ONLY");
    for e in catalog.emulators.iter().filter(|e| format!("Émulateur {}", e.name).contains(&only)) {
        let name = format!("Émulateur {}", e.name);
        if e.file_for(target).is_none() {
            verdicts.push(Verdict { name, detail: "pas de build Mac au catalogue".into(), ok: false });
            continue;
        }
        let Some(&(_, rom)) = roms.iter().find(|(id, _)| *id == e.id) else {
            verdicts.push(Verdict { name, detail: "build Mac présente, aucun jeu de test donné".into(), ok: true });
            continue;
        };
        println!("== {name} (jeu {rom})");
        match job(Some(e.id.clone())).run(rom).await {
            Ok(InstallOutcome::Installed { game }) => {
                let (ok, detail) = play(&catalog, &game, &out, &e.id).await;
                println!("   {detail}");
                verdicts.push(Verdict { name, detail, ok });
                let _ = super::uninstall(&config, game, false).await;
            }
            Ok(InstallOutcome::NeedsChoice { game }) => verdicts.push(Verdict { name, detail: format!("choix d'exécutable demandé : {:?}", game.pending_candidates), ok: false }),
            Err(err) => verdicts.push(Verdict { name, detail: format!("installation impossible : {err:?}"), ok: false }),
        }
    }

    // Recomps : leur premier jeu sur RomM.
    for r in catalog.recomps.iter().filter(|r| format!("Recomp {}", r.name).contains(&only)) {
        let name = format!("Recomp {}", r.name);
        if !r.files.iter().any(|f| target.candidates().iter().any(|t| f.system.as_deref() == Some(t.suffix()))) {
            verdicts.push(Verdict { name, detail: "pas de build Mac".into(), ok: false });
            continue;
        }
        let Some(rom) = r.games.iter().find_map(|g| g.rom_id).or(r.entry_rom_id) else {
            verdicts.push(Verdict { name, detail: "aucun jeu RomM".into(), ok: false });
            continue;
        };
        println!("== {name} (jeu {rom})");
        match job(None).run(rom).await {
            Ok(InstallOutcome::Installed { game }) => {
                let (ok, detail) = play(&catalog, &game, &out, &r.id).await;
                println!("   {detail}");
                verdicts.push(Verdict { name, detail, ok });
                let _ = super::uninstall(&config, game, false).await;
            }
            Ok(InstallOutcome::NeedsChoice { game }) => verdicts.push(Verdict { name, detail: format!("choix d'exécutable demandé : {:?}", game.pending_candidates), ok: false }),
            Err(err) => verdicts.push(Verdict { name, detail: format!("installation impossible : {err:?}"), ok: false }),
        }
    }

    let report: String = verdicts.iter().map(|v| format!("{} {} — {}\n", if v.ok { "OK " } else { "NON" }, v.name, v.detail)).collect();
    println!("\n{report}");
    std::fs::write(out.join("compte-rendu.txt"), report).unwrap();
    let _ = std::fs::remove_dir_all(&base);
}
