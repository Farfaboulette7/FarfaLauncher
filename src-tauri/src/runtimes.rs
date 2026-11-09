//! Recomps qui font tourner plusieurs jeux (Gen1recomp : Rouge, Bleu, Or…).
//!
//! Le « runtime » est le programme installé, partagé et suivi une seule fois :
//! son entrée RomM (`RECOMP/`, `VERSION.txt`, `MODS/`) n'a pas de page dans le
//! launcher. Chaque jeu a sa propre entrée `portage-<console>/<Jeu>/ROM/`, qui donne sa page.
//!
//! Le manifeste qui les relie est la clé `recomps` de `assets.json`, sur le
//! serveur des assets (réservé aux utilisateurs connectés). Une copie reste sur
//! le disque pour démarrer sans serveur.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets;
use crate::error::{AppError, AppResult};
use crate::manager::Catalog;
use crate::platform::Target;
use crate::romm::client::RommClient;
use crate::romm::library::{self, Game, GameRuntime};
use crate::state::AppState;

const REMOTE_FILE: &str = "assets.json";
const CACHE_FILE: &str = "recomps.json";

/// Programme partagé par plusieurs jeux.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub id: String,
    pub name: String,
    /// Dépôt GitHub de la recomp (le suivi des versions reste à recomp-updater).
    pub repo: String,
    /// Entrée RomM qui porte `RECOMP/` et `VERSION.txt`.
    pub rom_id: i64,
    /// Début du nom de l'exécutable, sans extension (`gen1recomp`).
    pub exe: String,
    /// Dossier de données partagé, sous `%APPDATA%` et ses équivalents.
    pub data_dir: String,
    /// Sauvegardes d'un jeu, relatives à `data_dir` (`{game}` remplacé).
    pub save_dir: String,
    /// Données tirées de la ROM, relatives à `data_dir`, recréées si besoin.
    #[serde(default)]
    pub cache_dir: Option<String>,
    /// Arguments du lancement (`{game}`, `{rom}` remplacés). Vide : menu de la recomp.
    #[serde(default)]
    pub args: Vec<String>,
    /// Variables d'environnement du lancement (`{game}`, `{rom}` remplacés).
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Un jeu : sa page vient de son entrée `portage-*` de RomM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGame {
    pub runtime: String,
    /// Identifiant du jeu pour la recomp (`red`, `gold`…).
    pub game: String,
    pub rom_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Manifest {
    pub runtimes: Vec<Runtime>,
    pub games: Vec<RuntimeGame>,
}

#[derive(Deserialize)]
struct AssetsFile {
    #[serde(default)]
    recomps: Manifest,
}

/// Chemin relatif sans remontée : sert à supprimer des dossiers à la désinstallation.
fn valid_rel(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

fn valid_game_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

impl Runtime {
    fn is_valid(&self) -> bool {
        valid_game_id(&self.id)
            && !self.exe.is_empty()
            && valid_rel(&self.data_dir)
            && valid_rel(&expand(&self.save_dir, "x", ""))
            && self.cache_dir.as_deref().is_none_or(|d| valid_rel(&expand(d, "x", "")))
    }

    /// Arguments et variables de lancement d'un jeu.
    pub fn launch_for(&self, game: &str, rom: &str) -> (Vec<String>, Vec<(String, String)>) {
        (
            self.args.iter().map(|a| expand(a, game, rom)).collect(),
            self.env.iter().map(|(k, v)| (k.clone(), expand(v, game, rom))).collect(),
        )
    }
}

fn expand(text: &str, game: &str, rom: &str) -> String {
    text.replace("{game}", game).replace("{rom}", rom)
}

impl Manifest {
    /// Le catalogue de FarfaRomManager prime ; `assets.json` reste pour ce qu'il ne connaît pas.
    pub fn with_catalog(self, catalog: &Catalog) -> Manifest {
        let mut runtimes: Vec<Runtime> = Vec::new();
        let mut games: Vec<RuntimeGame> = Vec::new();
        for r in catalog.recomps.iter().filter(|r| r.is_shared()) {
            runtimes.push(Runtime {
                id: r.id.clone(),
                name: r.name.clone(),
                repo: r.repo.clone(),
                rom_id: r.runtime_rom_id(),
                exe: r.launch.exe.clone().unwrap_or_else(|| r.id.clone()),
                data_dir: r.launch.data_dir.clone().unwrap_or_default(),
                save_dir: r.launch.save_dir.clone().unwrap_or_default(),
                cache_dir: r.launch.cache_dir.clone(),
                args: r.launch.args.clone(),
                env: r.launch.env.clone(),
            });
            games.extend(r.games.iter().filter_map(|g| Some(RuntimeGame { runtime: r.id.clone(), game: g.game.clone()?, rom_id: g.rom_id? })));
        }
        let keep_old = |r: &Runtime| !runtimes.iter().any(|n| n.rom_id == r.rom_id);
        let old_runtimes: Vec<Runtime> = self.runtimes.iter().filter(|r| keep_old(r)).cloned().collect();
        let old_ids: Vec<String> = old_runtimes.iter().map(|r| r.id.clone()).collect();
        games.extend(self.games.into_iter().filter(|g| old_ids.contains(&g.runtime)));
        runtimes.extend(old_runtimes);
        Manifest { runtimes, games }.validated()
    }

    fn parse(text: &str) -> AppResult<Self> {
        let file: AssetsFile = serde_json::from_str(text).map_err(|e| crate::error::log_err(AppError::Server, "assets.json invalide", e))?;
        Ok(file.recomps.validated())
    }

    fn validated(mut self) -> Self {
        self.runtimes.retain(|r| {
            let ok = r.is_valid();
            if !ok {
                tracing::warn!("runtime {} ignoré : champs invalides", r.id);
            }
            ok
        });
        let known: Vec<String> = self.runtimes.iter().map(|r| r.id.clone()).collect();
        self.games.retain(|g| {
            let ok = known.contains(&g.runtime) && valid_game_id(&g.game);
            if !ok {
                tracing::warn!("jeu {} ignoré : runtime inconnu ou identifiant invalide", g.rom_id);
            }
            ok
        });
        self
    }

    pub fn load_cached(config_dir: &Path) -> Self {
        std::fs::read_to_string(config_dir.join(CACHE_FILE))
            .ok()
            .and_then(|t| serde_json::from_str::<Manifest>(&t).ok())
            .map(Manifest::validated)
            .unwrap_or_default()
    }

    fn save_cached(&self, config_dir: &Path) {
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(config_dir.join(CACHE_FILE), text) {
                    tracing::warn!("manifeste des recomps non mis en cache: {e}");
                }
            }
            Err(e) => tracing::warn!("manifeste des recomps non mis en cache: {e}"),
        }
    }

    pub fn is_runtime(&self, rom_id: i64) -> bool {
        self.runtimes.iter().any(|r| r.rom_id == rom_id)
    }

    /// Le jeu `rom_id` et son runtime, si la page en dépend.
    pub fn page(&self, rom_id: i64) -> Option<(&Runtime, &RuntimeGame)> {
        let game = self.games.iter().find(|g| g.rom_id == rom_id)?;
        Some((self.runtimes.iter().find(|r| r.id == game.runtime)?, game))
    }

    #[cfg(test)]
    pub fn page_ids(&self) -> Vec<i64> {
        self.games.iter().map(|g| g.rom_id).collect()
    }

    /// Le jeu ou runtime `rom_id` et tout ce qui partage son installation.
    pub fn family(&self, rom_id: i64) -> Vec<i64> {
        let runtime = self.page(rom_id).map(|(r, _)| r).or_else(|| self.runtimes.iter().find(|r| r.rom_id == rom_id));
        match runtime {
            Some(r) => std::iter::once(r.rom_id).chain(self.games.iter().filter(|g| g.runtime == r.id).map(|g| g.rom_id)).collect(),
            None => vec![rom_id],
        }
    }
}

/// Relit le manifeste sur le serveur des assets ; garde le précédent s'il ne répond pas.
pub async fn refresh(state: &AppState) {
    let Some(base) = assets::base_url(&state.settings()) else { return };
    let Ok(client) = state.client().map(|c| c.with_base(&base)) else { return };
    let text = match client.send(client.get(REMOTE_FILE)).await {
        Ok(resp) => resp.text().await.map_err(|e| crate::error::log_err(AppError::Server, "lecture de assets.json", e)),
        Err(e) => Err(e),
    };
    match text.and_then(|t| Manifest::parse(&t)) {
        Ok(manifest) => {
            manifest.save_cached(&state.config_dir);
            tracing::info!("{} runtime(s) partagé(s) dans assets.json, {} jeu(x)", manifest.runtimes.len(), manifest.games.len());
        }
        Err(e) => tracing::warn!("manifeste des recomps indisponible ({}), copie locale gardée", e.code()),
    }
}

pub fn link(runtime: &Runtime, game: &RuntimeGame) -> GameRuntime {
    GameRuntime { rom_id: runtime.rom_id, name: runtime.name.clone(), repo: runtime.repo.clone(), game: game.game.clone() }
}

/// Manifeste en vigueur, d'après les dernières copies d'`assets.json` et du catalogue.
pub fn combine(state: &AppState) {
    let manifest = Manifest::load_cached(&state.config_dir).with_catalog(&state.catalog());
    *state.manifest.lock().unwrap() = manifest;
}

/// Programme d'un runtime : décrit par le catalogue, sinon par son entrée RomM.
pub async fn fetch_runtime(client: &RommClient, catalog: &Catalog, runtime: &Runtime, me: Target) -> AppResult<Game> {
    match catalog.recomp(&runtime.id) {
        Some(r) => Ok(Game::from_catalog(r, me)),
        None => library::fetch_game_details(client, runtime.rom_id, me).await,
    }
}

/// Détails d'une recomp à un seul jeu : son programme vient du catalogue s'il la connaît.
async fn fetch_single(client: &RommClient, catalog: &Catalog, rom_id: i64, me: Target) -> AppResult<Game> {
    let mut game = library::fetch_game_details(client, rom_id, me).await?;
    if let Some(r) = catalog.recomp_for_rom(rom_id).filter(|r| !r.is_shared()) {
        game.use_catalog(r, me);
    }
    Ok(game)
}

/// Page d'un jeu sans les fichiers de son runtime : sa ROM est dans son `ROM/`.
pub async fn fetch_page(client: &RommClient, manifest: &Manifest, rom_id: i64, me: Target) -> AppResult<Game> {
    let (runtime, game) = manifest.page(rom_id).ok_or(AppError::NotAvailable)?;
    let mut page = library::fetch_game_details(client, rom_id, me).await?;
    page.runtime = Some(link(runtime, game));
    Ok(page)
}

/// Plateformes et jeux de la bibliothèque. Les runtimes n'ont pas de page. `on_listed` reçoit
/// tous les jeux du serveur, runtimes compris.
pub async fn list_library(
    client: &RommClient,
    manifest: &Manifest,
    recomps: &HashSet<i64>,
    hidden: &HashSet<i64>,
    me: Target,
    on_listed: impl FnOnce(&[(i64, String)]) -> usize,
) -> AppResult<(Vec<library::Platform>, Vec<Game>)> {
    let platforms = library::list_platforms(client).await?;
    let mut games = library::list_games(client, &platforms, me).await?;
    // Jeux de recomp et jeux installés hors `portage-*` : chargés d'avance eux aussi.
    let mut missing: Vec<i64> = recomps.iter().copied().filter(|id| !games.iter().any(|g| g.id == *id)).collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        games.extend(library::list_games_by_ids(client, &missing, me).await);
        games.sort_by_key(|g| g.name.to_lowercase());
    }
    let ids: Vec<(i64, String)> = games.iter().map(|g| (g.id, g.folder_name.clone())).collect();
    on_listed(&ids);
    // Ancienne fiche `portage-*` d'un jeu passé sur sa vraie console : plus affichée.
    games.retain(|g| !manifest.is_runtime(g.id) && !hidden.contains(&g.id));
    for game in games.iter_mut() {
        if let Some((runtime, def)) = manifest.page(game.id) {
            game.runtime = Some(link(runtime, def));
        }
    }
    Ok((platforms, games))
}

/// Ajoute à un jeu ce que le catalogue sait de lui : recomp et émulateur.
pub fn annotate(game: &mut Game, catalog: &Catalog, recomps: &HashSet<i64>, me: Target) {
    game.recomp = game.recomp || game.runtime.is_some() || recomps.contains(&game.id);
    let console = (game.pc.is_none() && !game.console.is_empty()).then(|| game.console.clone());
    let id = game.id;
    game.emulator = console.as_deref().and_then(|c| catalog.offer(c, me)).map(|o| Catalog::for_game(o, catalog, id));
    game.emulators = console.as_deref().map(|c| catalog.offers(c, me)).unwrap_or_default().into_iter().map(|o| Catalog::for_game(o, catalog, id)).collect();
}

/// Détails d'un jeu : ses propres fichiers, plus ceux de son runtime s'il en a un.
pub async fn fetch_details(client: &RommClient, manifest: &Manifest, catalog: &Catalog, rom_id: i64, me: Target) -> AppResult<Game> {
    let Some((runtime, _)) = manifest.page(rom_id) else {
        return fetch_single(client, catalog, rom_id, me).await;
    };
    let shared = fetch_runtime(client, catalog, runtime, me).await?;
    let mut page = fetch_page(client, manifest, rom_id, me).await?;
    page.use_runtime(&shared);
    Ok(page)
}

/// Détails d'un jeu pendant un rafraîchissement : chaque runtime n'y est lu qu'une fois.
pub async fn fetch_details_with(client: &RommClient, manifest: &Manifest, catalog: &Catalog, shared: &HashMap<i64, Game>, rom_id: i64, me: Target) -> AppResult<Game> {
    let Some((runtime, _)) = manifest.page(rom_id) else {
        return fetch_single(client, catalog, rom_id, me).await;
    };
    let rt = shared.get(&runtime.rom_id).ok_or(AppError::Server)?;
    let mut page = fetch_page(client, manifest, rom_id, me).await?;
    page.use_runtime(rt);
    Ok(page)
}

/// Dossier de données existant d'un runtime (rien tant qu'il n'a jamais tourné).
pub fn existing_data_dir(name: &str) -> Option<PathBuf> {
    crate::saves::data_roots().into_iter().map(|r| r.join(crate::install::rel_to_path(name))).find(|d| d.is_dir())
}

/// Dossier de données d'un runtime : celui qui existe, sinon le premier emplacement du système.
pub fn data_dir(name: &str) -> Option<PathBuf> {
    existing_data_dir(name).or_else(|| crate::saves::data_roots().first().map(|r| r.join(crate::install::rel_to_path(name))))
}

/// Exécutable d'un runtime parmi les candidats. Le nom peut contenir la version
/// (AppImage) et l'ancienne reste sur le disque : celle de la version installée d'abord.
pub fn pick_exe(candidates: &[String], exe: &str, version: Option<&str>) -> Option<String> {
    let exe = exe.to_lowercase();
    let name = |c: &str| c.rsplit('/').next().unwrap_or(c).to_lowercase();
    // Le nom exact d'abord : `mGBA.exe` plutôt que `mgba-sdl.exe`.
    let exact = |c: &str| Path::new(&name(c)).file_stem().is_some_and(|s| s.to_string_lossy() == exe);
    if let Some(c) = candidates.iter().find(|c| exact(c)) {
        return Some(c.clone());
    }
    let matching: Vec<&String> = candidates.iter().filter(|c| name(c).starts_with(&exe)).collect();
    let version = version.map(|v| v.trim_start_matches(['v', 'V'])).filter(|v| !v.is_empty());
    matching
        .iter()
        .find(|c| version.is_some_and(|v| name(c).contains(v)))
        .or(matching.first())
        .map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vrai serveur : les jeux des recomps du catalogue sont dans la liste de départ.
    #[tokio::test]
    #[ignore]
    async fn catalog_recomps_listed_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let manager = client.with_base(&var("FARFA_E2E_MANAGER"));
        let catalog: Catalog = serde_json::from_str(&manager.send(manager.get("api/catalog")).await.unwrap().text().await.unwrap()).unwrap();
        let ids = catalog.recomp_rom_ids();
        let manifest = Manifest::default().with_catalog(&catalog);
        let (_, mut games) = list_library(&client, &manifest, &ids, &HashSet::new(), Target::Windows, |_| 0).await.unwrap();
        for g in games.iter_mut() {
            annotate(g, &catalog, &ids, Target::Windows);
        }
        let recomps: Vec<(i64, &str, &str)> = games.iter().filter(|g| g.recomp).map(|g| (g.id, g.console.as_str(), g.name.as_str())).collect();
        println!("{recomps:?}");
        let id: i64 = var("FARFA_E2E_ROM").parse().unwrap();
        assert!(recomps.iter().any(|r| r.0 == id));
    }

    /// Vrai serveur, vrai manifeste : `FARFA_E2E_URL=…FARFA_E2E_TOKEN=… FARFA_E2E_ASSETS=… cargo test -- --ignored library_e2e --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn library_e2e() {
        let var = |k: &str| std::env::var(k).unwrap();
        let client = RommClient::new(&var("FARFA_E2E_URL"), Some(var("FARFA_E2E_TOKEN"))).unwrap();
        let assets = client.with_base(&var("FARFA_E2E_ASSETS"));
        let text = assets.send(assets.get(REMOTE_FILE)).await.unwrap().text().await.unwrap();
        let manifest = Manifest::parse(&text).unwrap();
        let (platforms, games) = list_library(&client, &manifest, &HashSet::new(), &HashSet::new(), Target::Windows, |_| 0).await.unwrap();
        for p in &platforms {
            let own: Vec<&str> = games.iter().filter(|g| g.platform_id == p.id).map(|g| g.name.as_str()).collect();
            println!("{} {} ({}): {own:?}", p.id, p.fs_slug, p.name);
        }
        assert!(!games.iter().any(|g| manifest.is_runtime(g.id)));
        assert_eq!(games.iter().filter(|g| g.runtime.is_some()).count(), manifest.games.len());
        let runtime = library::fetch_game_details(&client, 10550, Target::Windows).await.unwrap();
        let page = fetch_details(&client, &manifest, &Catalog::default(), manifest.page_ids()[0], Target::Windows).await.unwrap();
        let debut = std::time::Instant::now();
        let (console, plus) = library::list_games_page(&client, &[8], Target::Windows, 0, 60).await.unwrap();
        println!("console GBA : {} jeux en {:?}, suite : {plus}, premier : {} ({} fichier(s) dans la liste)", console.len(), debut.elapsed(), console[0].name, console[0].rom_files.len());
        assert_eq!(console.len(), 60);
        assert!(plus && console.iter().all(|g| g.console == "gba" && !g.recomp));
        let (suite, _) = library::list_games_page(&client, &[8], Target::Windows, 60, 60).await.unwrap();
        assert!(suite.iter().all(|g| console.iter().all(|c| c.id != g.id)), "pages qui se chevauchent");
        let debut = std::time::Instant::now();
        let trouves = library::search_games(&client, "Mario Kart", Target::Windows, 60).await.unwrap();
        println!("recherche : {:?} en {:?}", trouves.iter().map(|g| format!("{} [{}]", g.name, g.console)).collect::<Vec<_>>(), debut.elapsed());
        assert!(!trouves.is_empty());
        let recents = library::recent_games(&client, Target::Windows, 24).await.unwrap();
        println!("récents : {} jeux, premier {}", recents.len(), recents[0].name);
        let details = library::fetch_game_details(&client, 835, Target::Windows).await.unwrap();
        println!("entrée de console 835 : {} ROM {:?}", details.rom_files.len(), details.rom_files.iter().map(|f| f.rel_path.as_str()).collect::<Vec<_>>());
        assert_eq!(details.rom_files.len(), 1);
        assert_eq!(details.rom_files[0].rel_path, details.rom_files[0].file_name);
        println!("{} : {} ROM, archive {:?}, version {:?}, {} mod(s)", page.name, page.rom_files.len(), page.my_archive.as_ref().map(|a| &a.file.file_name), page.server_version, page.mods.len());
        assert_eq!(page.rom_files.len(), 1);
        assert_eq!(page.server_version_raw, runtime.server_version_raw);
        assert!(page.my_archive.is_some() && page.details_loaded);
    }

    const TEXT: &str = r#"{"music":[],"recomps":{
        "runtimes":[{"id":"gen1recomp","name":"Gen1recomp","repo":"https://github.com/x/y","romId":10,
            "exe":"gen1recomp","dataDir":"pokemon-love2d","saveDir":"saves/{game}","cacheDir":"{game}",
            "args":["--game={game}"],"env":[["ROM","{rom}"]]},
          {"id":"bad","name":"B","repo":"","romId":99,"exe":"b","dataDir":"../..","saveDir":"s"}],
        "games":[{"runtime":"gen1recomp","game":"red","romId":1},{"runtime":"gen1recomp","game":"blue","romId":2},
          {"runtime":"bad","game":"x","romId":3},{"runtime":"gen1recomp","game":"../x","romId":4}]}}"#;

    #[test]
    fn the_catalog_replaces_the_same_runtime_of_assets_json() {
        let catalog: Catalog = serde_json::from_str(r#"{"recomps":[{"id":"pokemon","name":"Gen1recomp","repo":"https://github.com/n/m","entryRomId":10,
            "version":{"cle":"k","nom":"v1"},"launch":{"exe":"gen1recomp","args":["--game={game}"],"dataDir":"pokemon-love2d","saveDir":"saves/{game}","cacheDir":"{game}"},
            "games":[{"romId":1,"game":"red"},{"romId":null,"game":"absent"},{"romId":2,"game":"blue"}]},
            {"id":"seul","name":"S","repo":"","version":{"cle":"k","nom":"1"},"games":[{"romId":50}]}]}"#).unwrap();
        let m = Manifest::parse(TEXT).unwrap().with_catalog(&catalog);
        assert_eq!(m.runtimes.len(), 1);
        assert_eq!(m.runtimes[0].id, "pokemon");
        assert_eq!(m.runtimes[0].repo, "https://github.com/n/m");
        assert_eq!(m.page_ids(), [1, 2]);
        assert!(m.is_runtime(10) && !m.is_runtime(50));
        // Rien au catalogue : assets.json reste en place.
        assert_eq!(Manifest::parse(TEXT).unwrap().with_catalog(&Catalog::default()).runtimes[0].id, "gen1recomp");
    }

    #[test]
    fn reads_and_validates_the_manifest() {
        let m = Manifest::parse(TEXT).unwrap();
        assert_eq!(m.runtimes.len(), 1);
        assert_eq!(m.page_ids(), [1, 2]);
        assert!(m.is_runtime(10) && !m.is_runtime(1));
        assert_eq!(m.page(2).unwrap().1.game, "blue");
        assert!(m.page(3).is_none());
        assert_eq!(m.family(1), [10, 1, 2]);
        assert_eq!(m.family(10), [10, 1, 2]);
        assert_eq!(m.family(77), [77]);
    }

    #[test]
    fn a_server_without_manifest_changes_nothing() {
        let m = Manifest::parse(r#"{"music":[]}"#).unwrap();
        assert!(m.runtimes.is_empty() && m.games.is_empty());
    }

    #[test]
    fn expands_the_launch_of_a_game() {
        let m = Manifest::parse(TEXT).unwrap();
        let (args, env) = m.runtimes[0].launch_for("red", "/r/x.gb");
        assert_eq!(args, ["--game=red"]);
        assert_eq!(env, [("ROM".to_string(), "/r/x.gb".to_string())]);
    }

    #[test]
    fn refuses_paths_that_leave_the_data_folder() {
        for bad in ["", "..", "a/../b", "/a", "a/", "C:/x", "a\\b"] {
            assert!(!valid_rel(bad), "{bad}");
        }
        assert!(valid_rel("saves/red") && valid_rel("pokemon-love2d"));
    }

    #[test]
    fn picks_the_executable_of_the_installed_version() {
        let list = vec!["gen1recomp-0.3.98-linux.AppImage".to_string(), "gen1recomp-0.3.99-linux.AppImage".to_string(), "autre.AppImage".to_string()];
        assert_eq!(pick_exe(&list, "gen1recomp", Some("v0.3.99")).unwrap(), list[1]);
        assert_eq!(pick_exe(&list, "gen1recomp", Some("v9")).unwrap(), list[0]);
        assert_eq!(pick_exe(&["sub/Gen1Recomp.exe".to_string()], "gen1recomp", None).unwrap(), "sub/Gen1Recomp.exe");
        assert!(pick_exe(&list, "zelda", None).is_none());
        // Un programme annexe au nom voisin n'est pas choisi, même s'il vient avant.
        let mgba = vec!["mgba-sdl.exe".to_string(), "mGBA.exe".to_string()];
        assert_eq!(pick_exe(&mgba, "mGBA", Some("0.10.5")).unwrap(), "mGBA.exe");
    }
}
