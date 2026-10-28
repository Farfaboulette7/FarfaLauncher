//! Lecture des plateformes `portage-*` et de leurs jeux (docs/romm-api.md §2-4).

use serde::{Deserialize, Serialize};

use super::client::{parse_json, RommClient};
use crate::error::{log_err, AppError, AppResult};
use crate::platform::{self, ArchiveKind, Target};

/// Plateformes gérées : dossiers dont le nom commence par ce préfixe
/// (`portage-n64`, `portage-ngc`…).
pub const PLATFORM_PREFIX: &str = "portage";
const VERSION_FILE: &str = "VERSION.txt";
const PAGE_SIZE: usize = 100;
/// RomM peut mettre longtemps à décrire un jeu aux nombreux fichiers.
const DETAIL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ---------- Schémas RomM (seuls les champs utiles, tolérants) ----------

#[derive(Debug, Deserialize)]
struct RawPlatform {
    id: i64,
    slug: String,
    fs_slug: String,
    name: String,
    #[serde(default)]
    custom_name: Option<String>,
    #[serde(default)]
    rom_count: u32,
}

#[derive(Debug, Deserialize)]
struct RawPage {
    items: Vec<RawRom>,
}

#[derive(Debug, Deserialize)]
pub struct RawRom {
    id: i64,
    platform_id: i64,
    #[serde(default)]
    platform_fs_slug: String,
    name: Option<String>,
    fs_name: String,
    full_path: String,
    summary: Option<String>,
    #[serde(default)]
    path_cover_large: Option<String>,
    #[serde(default)]
    merged_screenshots: Vec<String>,
    #[serde(default)]
    path_video: Option<String>,
    #[serde(default)]
    has_manual: bool,
    #[serde(default)]
    path_manual: Option<String>,
    #[serde(default)]
    metadatum: Option<RawMetadatum>,
    #[serde(default)]
    ss_metadata: Option<RawMedia>,
    #[serde(default)]
    gamelist_metadata: Option<RawMedia>,
    #[serde(default)]
    created_at: String,
    updated_at: String,
    #[serde(default)]
    files: Vec<RawFile>,
}

/// Médias stockés par RomM (ScreenScraper ou gamelist.xml), chemins relatifs
/// à `/assets/romm/resources/`.
#[derive(Debug, Default, Deserialize)]
struct RawMedia {
    #[serde(default)]
    box3d_path: Option<String>,
    #[serde(default)]
    box2d_path: Option<String>,
    #[serde(default)]
    box2d_back_path: Option<String>,
    #[serde(default)]
    box2d_side_path: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawMetadatum {
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    first_release_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawFile {
    id: i64,
    file_name: String,
    full_path: String,
    file_size_bytes: u64,
    #[serde(default)]
    sha1_hash: Option<String>,
    #[serde(default)]
    md5_hash: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

// ---------- Modèle FarfaLauncher ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFile {
    pub id: i64,
    pub file_name: String,
    /// Chemin relatif au dossier du jeu : `RECOMP/x.zip`, `ROM/y.z64`, `VERSION.txt`…
    pub rel_path: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub md5: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformArchive {
    pub target: Target,
    pub kind: ArchiveKind,
    /// Préférence quand plusieurs archives couvrent le même système.
    pub score: u8,
    pub file: RemoteFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Manual {
    /// Livret scrapé / uploadé, servi en statique (sans authentification).
    Resource { url: String },
    /// PDF rangé dans le dossier `manual/` du jeu.
    File { file: RemoteFile },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Platform {
    pub id: i64,
    pub slug: String,
    pub fs_slug: String,
    pub name: String,
    pub rom_count: u32,
    /// Icônes servies par RomM, à essayer dans l'ordre (comme son interface web).
    pub icon_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoxFaces {
    pub front: String,
    /// Tranche (côté gauche de la boîte).
    pub side: String,
    pub back: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: i64,
    #[serde(default)]
    pub platform_id: i64,
    pub name: String,
    pub folder_name: String,
    pub summary: Option<String>,
    pub cover_url: Option<String>,
    /// Image de boîte 3D détourée (média `box3d` de RomM), si le serveur l'a.
    #[serde(default)]
    pub box3d_url: Option<String>,
    /// Faces de la boîte (médias `box2d`, `box2d_side`, `box2d_back`) pour
    /// construire la boîte 3D interactive, comme l'interface web de RomM.
    #[serde(default)]
    pub box_faces: Option<BoxFaces>,
    pub screenshot_urls: Vec<String>,
    pub video_url: Option<String>,
    pub genres: Vec<String>,
    pub developers: Vec<String>,
    pub release_year: Option<i32>,
    pub manuals: Vec<Manual>,
    /// Une archive RECOMP par plateforme présente sur le serveur.
    pub archives: Vec<PlatformArchive>,
    /// Archive retenue pour ce système (`None` = non disponible).
    pub my_archive: Option<PlatformArchive>,
    pub rom_files: Vec<RemoteFile>,
    pub mods: Vec<RemoteFile>,
    /// Jeu de la plateforme « Ordinateur » (installateurs, jeu prêt à lancer…).
    #[serde(default)]
    pub pc: Option<crate::pc::PcGame>,
    pub version_file: Option<RemoteFile>,
    /// Contenu de VERSION.txt côté serveur.
    pub server_version: Option<String>,
    /// Contenu complet (normalisé) de VERSION.txt, comparé pour les mises à jour.
    #[serde(default)]
    pub server_version_raw: Option<String>,
    /// `false` tant que les fichiers du jeu n'ont pas été lus sur le serveur
    /// (les champs ci-dessus peuvent alors venir du cache précédent).
    #[serde(default)]
    pub details_loaded: bool,
    /// Date d'ajout sur le serveur (RFC 3339), pour l'onglet « Récent ».
    #[serde(default)]
    pub created_at: String,
    pub updated_at: String,
}

/// Plateformes `portage-*` du serveur, triées par nom.
pub async fn list_platforms(client: &RommClient) -> AppResult<Vec<Platform>> {
    let raws: Vec<RawPlatform> = client.get_json("api/platforms").await?;
    let total = raws.len();
    let base = client.base();
    let mut platforms: Vec<Platform> = raws
        .into_iter()
        .filter(|p| is_portage(&p.fs_slug) || crate::pc::is_pc_folder(&p.fs_slug))
        .map(|p| {
            let pc = crate::pc::is_pc_folder(&p.fs_slug);
            let icon_urls = [&p.fs_slug, &p.slug]
                .iter()
                .flat_map(|s| ["svg", "ico"].map(|ext| format!("{base}/assets/platforms/{}.{ext}", s.to_lowercase())))
                .collect();
            Platform {
                id: p.id,
                name: if pc { "Ordinateur".to_string() } else { p.custom_name.filter(|n| !n.trim().is_empty()).unwrap_or(p.name) },
                slug: p.slug,
                fs_slug: p.fs_slug,
                rom_count: p.rom_count,
                icon_urls,
            }
        })
        .collect();
    if platforms.is_empty() {
        tracing::error!("aucune plateforme {PLATFORM_PREFIX}-* parmi {total} plateformes");
        return Err(AppError::PlatformMissing);
    }
    platforms.sort_by_key(|p| p.name.to_lowercase());
    Ok(platforms)
}

/// Jeu de la plateforme « Ordinateur » : d'après RomM, sinon d'après son
/// chemin (`roms/ordinateur/<jeu>`).
fn is_pc_rom(r: &RawRom) -> bool {
    if !r.platform_fs_slug.is_empty() {
        return crate::pc::is_pc_folder(&r.platform_fs_slug);
    }
    let path = r.full_path.replace('\\', "/");
    // `roms/ordinateur/<jeu>` ou `ordinateur/roms/<jeu>` selon la structure du serveur.
    path.trim_end_matches('/').rsplit('/').skip(1).take(2).any(crate::pc::is_pc_folder)
}

fn is_portage(fs_slug: &str) -> bool {
    fs_slug.to_ascii_lowercase().starts_with(PLATFORM_PREFIX)
}

/// Liste légère des jeux (sans leurs fichiers) : rapide même sur un serveur lent.
/// Les fichiers sont chargés ensuite jeu par jeu (`fetch_game_details`), pour
/// qu'un jeu lent à décrire par RomM ne bloque pas toute la bibliothèque.
pub async fn list_games(client: &RommClient, platforms: &[Platform], me: Target) -> AppResult<Vec<Game>> {
    let mut raws: Vec<RawRom> = Vec::new();
    for p in platforms {
        list_platform_games(client, p.id, &mut raws).await?;
    }
    let base = client.base().to_string();
    // Même jeu en version Windows et Linux : seule la version native de ce système est montrée.
    let pc_folders: Vec<(i64, String)> = raws.iter().filter(|r| is_pc_rom(r)).map(|r| (r.id, r.fs_name.clone())).collect();
    let hidden = crate::pc::hidden_duplicates(&pc_folders, me);
    let mut games: Vec<Game> = raws.into_iter().filter(|r| !hidden.contains(&r.id)).map(|r| to_game(r, &base, me, false)).collect();
    games.sort_by_key(|g| g.name.to_lowercase());
    let with_box = games.iter().filter(|g| g.box_faces.is_some() || g.box3d_url.is_some()).count();
    tracing::info!("{} jeux sur {} plateformes, {with_box} avec une boîte 3D", games.len(), platforms.len());
    Ok(games)
}

async fn list_platform_games(client: &RommClient, platform_id: i64, raws: &mut Vec<RawRom>) -> AppResult<()> {
    let first = raws.len();
    loop {
        let path = format!(
            "api/roms?platform_ids={platform_id}&with_char_index=false&with_filter_values=false\
             &with_rom_id_index=false&with_total=false&limit={PAGE_SIZE}&offset={}",
            raws.len() - first
        );
        let started = std::time::Instant::now();
        let page: RawPage = client.get_json(&path).await?;
        let n = page.items.len();
        tracing::info!("plateforme {platform_id}: {n} jeux reçus en {:?}", started.elapsed());
        raws.extend(page.items);
        if n < PAGE_SIZE {
            return Ok(());
        }
    }
}

impl Game {
    /// Reprend les informations de fichiers d'une version précédente (cache)
    /// en attendant le rechargement des détails.
    pub fn keep_details_from(&mut self, old: &Game) {
        self.manuals = old.manuals.clone();
        self.archives = old.archives.clone();
        self.my_archive = old.my_archive.clone();
        self.rom_files = old.rom_files.clone();
        self.mods = old.mods.clone();
        self.pc = old.pc.clone();
        self.version_file = old.version_file.clone();
        self.server_version = old.server_version.clone();
        self.server_version_raw = old.server_version_raw.clone();
        self.details_loaded = false;
    }
}

/// Détail complet d'un jeu : fichiers, archives par plateforme, mods, VERSION.txt.
pub async fn fetch_game_details(client: &RommClient, rom_id: i64, me: Target) -> AppResult<Game> {
    let started = std::time::Instant::now();
    let resp = client.send(client.get(&format!("api/roms/{rom_id}")).timeout(DETAIL_TIMEOUT)).await?;
    let raw: RawRom = parse_json(resp).await?;
    let mut game = to_game(raw, client.base(), me, true);
    tracing::info!("détails de « {} » ({} fichiers) reçus en {:?}", game.name, game.rom_files.len(), started.elapsed());
    if let Some(f) = game.version_file.clone() {
        match read_text_file(client, &f).await {
            Ok(text) => {
                game.server_version = Some(display_version(&text));
                game.server_version_raw = Some(text);
            }
            Err(e) => tracing::warn!("VERSION.txt illisible pour {}: {}", game.name, e.code()),
        }
    }
    Ok(game)
}

/// Télécharge VERSION.txt et renvoie son contenu normalisé (BOM, fins de
/// ligne et espaces de fin retirés) : c'est ce texte entier qui est comparé
/// pour détecter une mise à jour.
pub async fn read_text_file(client: &RommClient, file: &RemoteFile) -> AppResult<String> {
    let resp = client.send(client.get(&file_content_path(file))).await?;
    let bytes = resp.bytes().await.map_err(|e| log_err(AppError::ServerUnreachable, "lecture VERSION.txt", e))?;
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(16 * 1024)]);
    Ok(normalize_version_text(&text))
}

pub fn normalize_version_text(text: &str) -> String {
    text.trim_start_matches('\u{feff}').lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Version lisible : la ligne « Version : … » de la section RECOMP
/// (« v0.4.2 (publiée le …) » → « v0.4.2 »), sinon la première ligne.
pub fn display_version(text: &str) -> String {
    let mut in_recomp = false;
    let mut first_version = None;
    for line in text.lines() {
        let t = line.trim();
        if !line.starts_with([' ', '\t']) && !t.is_empty() {
            in_recomp = t.eq_ignore_ascii_case("RECOMP");
        }
        if let Some((key, value)) = t.split_once(':') {
            if key.trim().eq_ignore_ascii_case("version") {
                let v = value.trim();
                let v = v.split(" (").next().unwrap_or(v).trim().to_string();
                if in_recomp {
                    return v;
                }
                first_version.get_or_insert(v);
            }
        }
    }
    first_version.unwrap_or_else(|| text.lines().next().unwrap_or("").trim().to_string())
}

/// Chemin de téléchargement d'un fichier précis (le `{id}` est celui du fichier).
pub fn file_content_path(file: &RemoteFile) -> String {
    format!("api/roms/{}/files/content/{}", file.id, urlencode(&file.file_name))
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

/// Ressource statique de RomM (`/assets/romm/resources/…`), avec la date de
/// mise à jour du jeu pour renouveler le cache local quand elle change.
fn resource_url(base: &str, path: &str, updated_at: &str) -> String {
    format!("{base}/assets/romm/resources/{}?v={}", path.trim_start_matches('/'), urlencode(updated_at))
}

fn absolute(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        path.to_string()
    } else {
        format!("{base}/{}", path.trim_start_matches('/'))
    }
}

/// `roms/farfa-portage/Jeu/RECOMP/x.zip` → `RECOMP/x.zip` (préfixe = dossier du jeu).
fn relative_path(rom_full_path: &str, rom_fs_name: &str, file_full_path: &str) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let rom = norm(rom_full_path);
    let file = norm(file_full_path);
    let prefix = format!("{}/", rom.trim_end_matches('/'));
    if let Some(rest) = file.strip_prefix(&prefix) {
        return rest.to_string();
    }
    // Repli : ce qui suit le segment portant le nom du dossier du jeu.
    let marker = format!("/{rom_fs_name}/");
    match file.find(&marker) {
        Some(i) => file[i + marker.len()..].to_string(),
        None => file.rsplit('/').next().unwrap_or(&file).to_string(),
    }
}

fn top_folder(rel: &str) -> Option<String> {
    let (first, rest) = rel.split_once('/')?;
    (!rest.is_empty()).then(|| first.to_ascii_lowercase())
}

fn to_game(r: RawRom, base: &str, me: Target, details_loaded: bool) -> Game {
    let pc_rom = is_pc_rom(&r);
    let files: Vec<(RemoteFile, Option<String>)> = r
        .files
        .into_iter()
        .map(|f| {
            let rel = relative_path(&r.full_path, &r.fs_name, &f.full_path);
            let category = f.category;
            (
                RemoteFile {
                    id: f.id,
                    file_name: f.file_name,
                    rel_path: rel,
                    size: f.file_size_bytes,
                    sha1: f.sha1_hash.filter(|h| !h.is_empty()),
                    md5: f.md5_hash.filter(|h| !h.is_empty()),
                },
                category,
            )
        })
        .collect();

    let pc = if pc_rom {
        let all: Vec<RemoteFile> = files.iter().map(|(f, _)| f.clone()).collect();
        crate::pc::classify(&r.fs_name, &all, me)
    } else {
        None
    };
    let mut archives = Vec::new();
    let mut rom_files = Vec::new();
    let mut mods = Vec::new();
    let mut manuals = Vec::new();
    let mut version_file = None;
    for (f, category) in files {
        match top_folder(&f.rel_path).as_deref() {
            // Jeu PC : ses fichiers sont décrits par `pc` (livrets gardés).
            Some("recomp" | "rom" | "mods") | None if pc.is_some() => {}
            Some("recomp") => {
                // Seules les archives posées directement dans RECOMP/ comptent.
                if f.rel_path.matches('/').count() != 1 {
                    continue;
                }
                let matches = platform::archive_matches(&f.file_name);
                if matches.is_empty() {
                    tracing::debug!("archive RECOMP ignorée (nom non reconnu): {}", f.rel_path);
                }
                for m in matches {
                    // Une seule archive par système : la mieux notée.
                    match archives.iter_mut().find(|a: &&mut PlatformArchive| a.target == m.target) {
                        Some(a) if a.score >= m.score => {}
                        Some(a) => *a = PlatformArchive { target: m.target, kind: m.kind, score: m.score, file: f.clone() },
                        None => archives.push(PlatformArchive { target: m.target, kind: m.kind, score: m.score, file: f.clone() }),
                    }
                }
            }
            Some("rom") => rom_files.push(f),
            Some("mods") => mods.push(f),
            Some("manual") | Some("manuals") if f.file_name.to_ascii_lowercase().ends_with(".pdf") => {
                manuals.push(Manual::File { file: f })
            }
            None if f.file_name.eq_ignore_ascii_case(VERSION_FILE) => version_file = Some(f),
            _ if category.as_deref() == Some("manual") && f.file_name.to_ascii_lowercase().ends_with(".pdf") => {
                manuals.push(Manual::File { file: f })
            }
            _ => {}
        }
    }
    if r.has_manual {
        if let Some(p) = r.path_manual.as_deref().filter(|p| !p.is_empty()) {
            manuals.insert(0, Manual::Resource { url: resource_url(base, p, &r.updated_at) });
        }
    }
    archives.sort_by_key(|a| crate::platform::ALL_TARGETS.iter().position(|t| *t == a.target));
    let my_archive = me
        .candidates()
        .iter()
        .find_map(|t| archives.iter().find(|a| a.target == *t))
        .cloned();

    let media: Vec<&RawMedia> = [&r.ss_metadata, &r.gamelist_metadata].into_iter().flatten().collect();
    let media_url = |pick: fn(&RawMedia) -> &Option<String>| {
        media
            .iter()
            .find_map(|m| pick(m).as_deref().filter(|p| !p.is_empty()))
            .map(|p| resource_url(base, p, &r.updated_at))
    };
    let box3d_url = media_url(|m| &m.box3d_path).filter(|_| !pc_rom);
    let cover_url = r.path_cover_large.clone().filter(|p| !p.is_empty()).map(|p| absolute(base, &p));
    // Sans tranche, pas de boîte 3D : la jaquette s'affiche à plat.
    // Jeux PC : rarement une vraie boîte, la jaquette de RomM s'affiche à plat.
    let box_faces = media_url(|m| &m.box2d_side_path).filter(|_| !pc_rom).and_then(|side| {
        let front = media_url(|m| &m.box2d_path).or_else(|| cover_url.clone())?;
        Some(BoxFaces { front, side, back: media_url(|m| &m.box2d_back_path) })
    });

    let meta = r.metadatum.unwrap_or_default();
    let mut developers = meta.developers;
    if developers.is_empty() {
        developers = meta.publishers;
    }

    Game {
        id: r.id,
        platform_id: r.platform_id,
        name: r.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.fs_name.clone()),
        folder_name: r.fs_name,
        summary: r.summary.filter(|s| !s.trim().is_empty()),
        cover_url,
        box3d_url,
        box_faces,
        screenshot_urls: r.merged_screenshots.iter().map(|p| absolute(base, p)).collect(),
        video_url: r.path_video.filter(|p| !p.is_empty()).map(|p| absolute(base, &p)),
        genres: meta.genres,
        developers,
        release_year: meta.first_release_date.and_then(release_year),
        manuals,
        archives,
        my_archive,
        rom_files,
        mods,
        pc,
        version_file,
        server_version: None,
        server_version_raw: None,
        details_loaded,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// `first_release_date` est un timestamp Unix (secondes ou millisecondes selon la source).
fn release_year(ts: i64) -> Option<i32> {
    if ts == 0 {
        return None; // 0 = date inconnue
    }
    let secs = if ts.abs() > 100_000_000_000 { ts / 1000 } else { ts };
    let days = secs.div_euclid(86_400);
    // Algorithme civil_from_days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    i32::try_from(year).ok().filter(|y| (1950..=2200).contains(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        let rom = "roms/farfa-portage/Super Mario 64";
        assert_eq!(
            relative_path(rom, "Super Mario 64", "roms/farfa-portage/Super Mario 64/RECOMP/sm64coopdx_Windows.zip"),
            "RECOMP/sm64coopdx_Windows.zip"
        );
        assert_eq!(relative_path(rom, "Super Mario 64", "roms/farfa-portage/Super Mario 64/VERSION.txt"), "VERSION.txt");
        // Préfixe différent : repli sur le nom du dossier.
        assert_eq!(
            relative_path("farfa-portage/roms/Super Mario 64", "Super Mario 64", "library/farfa-portage/roms/Super Mario 64/ROM/a.z64"),
            "ROM/a.z64"
        );
    }

    #[test]
    fn portage_platforms() {
        assert!(is_portage("portage-ngc"));
        assert!(is_portage("Portage-N64"));
        assert!(!is_portage("farfa-portage"));
        assert!(!is_portage("ngc"));
    }

    #[test]
    fn top_folders() {
        assert_eq!(top_folder("RECOMP/x.zip").as_deref(), Some("recomp"));
        assert_eq!(top_folder("VERSION.txt"), None);
    }

    #[test]
    fn years() {
        assert_eq!(release_year(835_833_600), Some(1996)); // 1996-06-27
        assert_eq!(release_year(835_833_600_000), Some(1996));
        assert_eq!(release_year(0), None);
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}Super Smash Bros. Melee\r\n=======================\r\nMis à jour le 23/10/2026 à 21:29\r\n\r\nRECOMP\r\n  Version : v0.4.2 (publiée le 21/10/2026)\r\n  Source  : https://example.org/x\r\n";

    #[test]
    fn display_reads_recomp_version() {
        let n = normalize_version_text(SAMPLE);
        assert!(n.starts_with("Super Smash Bros. Melee\n"));
        assert!(!n.contains('\r'));
        assert_eq!(display_version(&n), "v0.4.2");
    }

    #[test]
    fn display_falls_back() {
        assert_eq!(display_version("1.2.3"), "1.2.3");
        assert_eq!(display_version("ROM\n  Version : r1\nRECOMP\n  Version : v9"), "v9");
        assert_eq!(display_version("Jeu\nVersion: 2.0"), "2.0");
    }
}
