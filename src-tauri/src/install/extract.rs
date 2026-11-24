//! Extraction des archives RECOMP et fusion « sans casse » dans le dossier du jeu.
//!
//! Règles (cahier des charges, mises à jour) :
//! 1. ne jamais toucher aux dossiers/fichiers `save` / `saves` (à toute
//!    profondeur, casse ignorée), ni à `mods_downloads/` ;
//! 2. n'écraser que les fichiers présents dans la nouvelle archive, ne jamais
//!    supprimer un fichier absent de l'archive ;
//! 3. sauvegarder les dossiers `save`/`saves` avant une mise à jour.
//!
//! Toutes ces fonctions sont bloquantes : à appeler dans `spawn_blocking`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use tokio_util::sync::CancellationToken;

use crate::error::{log_err, AppError, AppResult};
use crate::install::download::disk_error;
use crate::platform::ArchiveKind;

/// Dossier interne du launcher dans chaque dossier de jeu.
pub const INTERNAL_DIR: &str = ".farfalauncher";
pub const STATE_FILE: &str = "farfalauncher.json";
pub const MODS_DIR: &str = "mods_downloads";

/// Chemin (relatif au dossier du jeu) que le launcher ne doit jamais écraser.
pub fn is_protected(rel: &Path) -> bool {
    let mut comps = rel.components().filter_map(|c| match c {
        Component::Normal(s) => Some(s.to_string_lossy().to_lowercase()),
        _ => None,
    });
    let Some(first) = comps.next() else { return false };
    if first == MODS_DIR || first == INTERNAL_DIR || first == STATE_FILE {
        return true;
    }
    std::iter::once(first).chain(comps).any(|c| c == "save" || c == "saves")
}

/// Taille décompressée totale (pour vérifier l'espace disque avant extraction).
pub fn uncompressed_size(archive: &Path, kind: ArchiveKind) -> AppResult<u64> {
    match kind {
        ArchiveKind::Zip => {
            let mut zip = open_zip(archive)?;
            let mut total = 0u64;
            for i in 0..zip.len() {
                if let Ok(entry) = zip.by_index_raw(i) {
                    total += entry.size();
                }
            }
            Ok(total)
        }
        // Inconnue sans tout décompresser : estimation prudente.
        ArchiveKind::TarGz => Ok(fs::metadata(archive).map(|m| m.len() * 3).unwrap_or(0)),
        ArchiveKind::AppImage => Ok(fs::metadata(archive).map(|m| m.len()).unwrap_or(0)),
    }
}

/// Décompresse `archive` dans `staging` (vidé au préalable).
/// `progress(fait, total)` compte les octets : décompressés pour un zip, lus dans
/// l'archive pour un tar.gz.
pub fn extract(
    archive: &Path,
    kind: ArchiveKind,
    staging: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(u64, u64),
) -> AppResult<()> {
    if staging.exists() {
        fs::remove_dir_all(staging).map_err(|e| log_err(AppError::Disk, "nettoyage du dossier temporaire", e))?;
    }
    fs::create_dir_all(staging).map_err(|e| disk_error("création du dossier temporaire", e))?;
    match kind {
        ArchiveKind::Zip => extract_zip(archive, staging, cancel, &mut progress)?,
        ArchiveKind::TarGz => extract_tar_gz(archive, staging, cancel, &mut progress)?,
        ArchiveKind::AppImage => {
            let name = archive.file_name().ok_or(AppError::Extract)?;
            fs::copy(archive, staging.join(name)).map_err(|e| disk_error("copie de l'AppImage", e))?;
            progress(1, 1);
        }
    }
    if cfg!(target_os = "macos") {
        unpack_disk_images(staging)?;
    }
    Ok(())
}

/// Images disque macOS (`.dmg`) livrées dans l'archive : montées en lecture seule, leurs
/// applications copiées à côté (liens et attributs gardés), puis démontées et supprimées.
fn unpack_disk_images(dir: &Path) -> AppResult<()> {
    let dmgs: Vec<PathBuf> = walk_files(dir, 2).into_iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dmg"))).collect();
    for dmg in dmgs {
        let dest = dmg.parent().unwrap_or(dir).to_path_buf();
        let mount = dir.join(format!(".montage-{}", std::process::id()));
        let _ = fs::create_dir_all(&mount);
        let mut attach = std::process::Command::new("hdiutil");
        attach.args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount).arg(&dmg);
        // Licence à accepter dans certaines images : réponse « Y ».
        attach.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null());
        let mut child = attach.spawn().map_err(|e| log_err(AppError::Extract, "hdiutil attach", e))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = io::Write::write_all(&mut stdin, b"Y\n");
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if !ok {
            let _ = fs::remove_dir(&mount);
            tracing::error!("image disque illisible : {}", dmg.display());
            return Err(AppError::Extract);
        }
        let mut copied = 0;
        for entry in fs::read_dir(&mount).map_err(|e| disk_error("lecture de l'image disque", e))?.filter_map(Result::ok) {
            let name = entry.file_name();
            if !name.to_string_lossy().to_lowercase().ends_with(".app") {
                continue;
            }
            let status = std::process::Command::new("ditto").arg(entry.path()).arg(dest.join(&name)).status();
            if status.map(|s| s.success()).unwrap_or(false) {
                copied += 1;
            }
        }
        let _ = std::process::Command::new("hdiutil").args(["detach", "-force"]).arg(&mount).stdout(std::process::Stdio::null()).status();
        let _ = fs::remove_dir(&mount);
        if copied == 0 {
            tracing::error!("aucune application dans {}", dmg.display());
            return Err(AppError::Extract);
        }
        tracing::info!("{} : {copied} application(s) copiée(s)", dmg.display());
        fs::remove_file(&dmg).map_err(|e| disk_error("suppression de l'image disque", e))?;
    }
    Ok(())
}

/// Fichiers jusqu'à `depth` niveaux sous `dir` (sans descendre dans les .app).
fn walk_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_file() => out.push(p),
            Ok(t) if t.is_dir() && depth > 0 && !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("app")) => out.extend(walk_files(&p, depth - 1)),
            _ => {}
        }
    }
    out
}

/// Ouvre un zip qui commence au début du fichier (cas de toutes nos archives).
/// La détection par défaut de la bibliothèque relit tout le fichier pour un
/// zip64 : des dizaines de Go pour une archive de 12 Go. Repli sur elle si
/// l'archive a des données en tête (auto-extractible…).
fn open_zip(archive: &Path) -> AppResult<zip::ZipArchive<io::BufReader<fs::File>>> {
    use zip::read::{ArchiveOffset, Config};
    let open = || -> AppResult<io::BufReader<fs::File>> {
        let f = fs::File::open(archive).map_err(|e| log_err(AppError::Disk, "ouverture de l'archive", e))?;
        Ok(io::BufReader::new(f))
    };
    match zip::ZipArchive::with_config(Config { archive_offset: ArchiveOffset::Known(0) }, open()?) {
        Ok(zip) => Ok(zip),
        Err(e) => {
            tracing::info!("zip à décalage inconnu ({e}) : détection complète");
            zip::ZipArchive::new(open()?).map_err(|e| log_err(AppError::Extract, "lecture du zip", e))
        }
    }
}

fn skip_entry(rel: &Path) -> bool {
    rel.components().any(|c| matches!(c, Component::Normal(s) if s == "__MACOSX" || s == ".DS_Store"))
}

fn extract_zip(archive: &Path, staging: &Path, cancel: &CancellationToken, progress: &mut impl FnMut(u64, u64)) -> AppResult<()> {
    let mut zip = open_zip(archive)?;
    let total: u64 = (0..zip.len()).filter_map(|i| zip.by_index_raw(i).ok().map(|e| e.size())).sum::<u64>().max(1);
    let mut done = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    // Vrai dès qu'un lien a été posé dans `staging` (vidé avant) : sans lien, rien à vérifier.
    let mut links_made = false;
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = zip.by_index(i).map_err(|e| log_err(AppError::Extract, "entrée du zip", e))?;
        // `enclosed_name` refuse les chemins absolus et les `..` (zip slip).
        let Some(rel) = entry.enclosed_name() else {
            tracing::warn!("entrée de zip ignorée (chemin dangereux): {}", entry.name().map(|n| n.to_string()).unwrap_or_default());
            continue;
        };
        if skip_entry(&rel) {
            continue;
        }
        let out = staging.join(&rel);
        let mode = entry.unix_mode();
        let is_symlink = mode.map(|m| m & 0o170000 == 0o120000).unwrap_or(false);
        // Faille SL5 : jamais d'écriture à travers un lien déjà posé (le lien lui-même
        // peut être remplacé, d'où `parent` seulement pour un lien).
        let checked = if is_symlink { rel.parent().unwrap_or(Path::new("")) } else { rel.as_path() };
        if links_made && goes_through_link(staging, checked) {
            tracing::error!("archive refusée : « {} » passe par un lien symbolique", rel.display());
            return Err(AppError::Extract);
        }
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| disk_error("création d'un dossier", e))?;
        } else if is_symlink {
            // Liens symboliques des bundles macOS (frameworks) et des `.so` versionnées.
            let mut target = String::new();
            io::Read::read_to_string(&mut entry, &mut target).map_err(|e| log_err(AppError::Extract, "lien du zip", e))?;
            // Faille SL5 : une cible qui sort de l'archive est ignorée (voir `link_target_inside`).
            if !link_target_inside(&rel, &target) {
                tracing::warn!("lien ignoré (cible hors de l'archive) : {} -> {target}", rel.display());
                continue;
            }
            make_symlink(&target, &out)?;
            links_made = true;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| disk_error("création d'un dossier", e))?;
            }
            let mut w = fs::File::create(&out).map_err(|e| disk_error("écriture d'un fichier", e))?;
            // Copie par blocs : la progression avance aussi pendant un gros fichier.
            loop {
                let n = io::Read::read(&mut entry, &mut buf).map_err(|e| match e.kind() {
                    io::ErrorKind::InvalidData => log_err(AppError::Extract, "décompression", e),
                    _ => log_err(AppError::Extract, "lecture du zip", e),
                })?;
                if n == 0 {
                    break;
                }
                io::Write::write_all(&mut w, &buf[..n]).map_err(|e| disk_error("écriture d'un fichier", e))?;
                done += n as u64;
                progress(done.min(total), total);
                if cancel.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
            }
            #[cfg(unix)]
            if let Some(m) = mode {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(m & 0o7777));
            }
            #[cfg(not(unix))]
            let _ = mode;
        }
        progress(done.min(total), total);
    }
    progress(total, total);
    Ok(())
}

fn extract_tar_gz(archive: &Path, staging: &Path, cancel: &CancellationToken, progress: &mut impl FnMut(u64, u64)) -> AppResult<()> {
    let f = fs::File::open(archive).map_err(|e| log_err(AppError::Disk, "ouverture de l'archive", e))?;
    let total = f.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let read = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let counting = CountingReader { inner: f, read: read.clone() };
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(io::BufReader::new(counting)));
    tar.set_preserve_permissions(true);
    let mut links_made = false;
    for entry in tar.entries().map_err(|e| log_err(AppError::Extract, "lecture du tar.gz", e))? {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let mut entry = entry.map_err(|e| log_err(AppError::Extract, "entrée du tar.gz", e))?;
        let rel = entry.path().map(|p| p.into_owned()).unwrap_or_default();
        if skip_entry(&rel) {
            continue;
        }
        // Chemin tel que `unpack_in` le pose : sans `/` de tête ni `.`.
        let rel: PathBuf = rel.components().filter(|c| matches!(c, Component::Normal(_))).collect();
        // Faille SL5 : `unpack_in` refuse déjà d'écrire hors du dossier à travers un lien
        // (chemins canoniques) et contrôle les liens physiques, mais pas la cible d'un lien
        // symbolique, qui partirait ensuite dans le dossier du jeu.
        let is_symlink = entry.header().entry_type().is_symlink();
        if is_symlink {
            let target = entry.link_name().ok().flatten().map(|t| t.to_string_lossy().into_owned()).unwrap_or_default();
            if !link_target_inside(&rel, &target) {
                tracing::warn!("lien ignoré (cible hors de l'archive) : {} -> {target}", rel.display());
                continue;
            }
        }
        // Lien physique vers un lien symbolique : sous Linux et Mac, il copie le lien
        // symbolique tel quel, ailleurs dans l'archive, où sa cible relative (`../../x`)
        // mènerait dehors. Tout lien physique qui passe par un lien symbolique est ignoré,
        // comme celui dont la source a un `..` ou est absolue (calcul sûr impossible).
        if entry.header().entry_type().is_hard_link() {
            let source: PathBuf = entry.link_name().ok().flatten().map(|p| p.into_owned()).unwrap_or_default();
            let plain = source.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
            if !plain || (links_made && goes_through_link(staging, &source)) {
                tracing::warn!("lien physique ignoré (passe par un lien symbolique) : {} -> {}", rel.display(), source.display());
                continue;
            }
        }
        // Même règle que pour un zip : jamais d'écriture à travers un lien, même interne.
        let checked = if is_symlink { rel.parent().unwrap_or(Path::new("")) } else { rel.as_path() };
        if links_made && goes_through_link(staging, checked) {
            tracing::error!("archive refusée : « {} » passe par un lien symbolique", rel.display());
            return Err(AppError::Extract);
        }
        links_made |= is_symlink;
        // `unpack_in` refuse les chemins qui sortent du dossier cible.
        entry.unpack_in(staging).map_err(|e| disk_error("extraction du tar.gz", e))?;
        progress(read.get().min(total), total);
    }
    progress(total, total);
    Ok(())
}

/// Compte les octets lus dans l'archive compressée (progression d'un tar.gz).
struct CountingReader {
    inner: fs::File,
    read: std::rc::Rc<std::cell::Cell<u64>>,
}

impl io::Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// Faille SL5 : vrai si la cible `target` d'un lien placé en `link_rel` (chemin relatif à la
/// racine d'extraction) reste sous cette racine. Contrôle sur le texte seul, sans toucher au
/// disque, identique sous Windows, Linux et Mac.
///
/// Refusés :
/// - une cible vide ou absolue (`/…`, `\…`, `C:…`, tout `:`) ;
/// - un `..` qui remonte au-dessus de la racine ;
/// - un `..` après un nom (`a/..`) : si `a` est lui-même un lien, `a/..` ne désigne pas le
///   dossier du lien, et le calcul sur le texte serait faux.
///
/// Acceptés : `Versions/Current/Foo`, `libfoo.so.1`, `../lib/libfoo.so` (s'il reste dedans).
pub(crate) fn link_target_inside(link_rel: &Path, target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.starts_with('\\') || target.contains(':') {
        return false;
    }
    // Profondeur du dossier qui contient le lien, comptée depuis la racine.
    let mut depth: usize = 0;
    for c in link_rel.parent().unwrap_or(Path::new("")).components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
    }
    // On suit la cible morceau par morceau (`/` et `\`, pour la même règle partout).
    let mut seen_name = false;
    for part in target.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if seen_name || depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            _ => {
                seen_name = true;
                depth += 1;
            }
        }
    }
    true
}

/// Faille SL5 : vrai si l'un des morceaux de `rel` (lui compris), déjà présent sous `root`, est
/// un lien symbolique. Écrire là passerait par le lien.
fn goes_through_link(root: &Path, rel: &Path) -> bool {
    let mut path = root.to_path_buf();
    for c in rel.components() {
        if let Component::Normal(name) = c {
            path.push(name);
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn make_symlink(target: &str, link: &Path) -> AppResult<()> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).map_err(|e| disk_error("création d'un dossier", e))?;
    }
    let _ = fs::remove_file(link);
    std::os::unix::fs::symlink(target, link).map_err(|e| disk_error("création d'un lien", e))
}

#[cfg(not(unix))]
fn make_symlink(target: &str, link: &Path) -> AppResult<()> {
    // Sans intérêt sous Windows (liens des bundles macOS) : ignoré. La cible a quand même été
    // contrôlée avant (même avertissement dans le journal sous Windows que sous Linux).
    tracing::debug!("lien symbolique ignoré: {} -> {target}", link.display());
    Ok(())
}

/// Si l'archive ne contient qu'un dossier racine (hors bundle `.app`), on
/// travaille à l'intérieur de celui-ci (aplatissement).
pub fn content_root(staging: &Path) -> AppResult<PathBuf> {
    let entries: Vec<_> = fs::read_dir(staging)
        .map_err(|e| log_err(AppError::Disk, "lecture du dossier temporaire", e))?
        .filter_map(Result::ok)
        .collect();
    if let [only] = entries.as_slice() {
        let name = only.file_name().to_string_lossy().to_lowercase();
        let is_dir = only.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir && !name.ends_with(".app") {
            tracing::info!("archive à dossier racine unique « {name} » : aplatie");
            return Ok(only.path());
        }
    }
    Ok(staging.to_path_buf())
}

/// Fusionne `src` dans `dest` : écrase les fichiers de l'archive, ne supprime
/// rien, ne touche à aucun chemin protégé. Renvoie le nombre de fichiers posés.
pub fn merge_into(src: &Path, dest: &Path) -> AppResult<u64> {
    fs::create_dir_all(dest).map_err(|e| disk_error("création du dossier du jeu", e))?;
    let mut count = 0;
    merge_dir(src, dest, Path::new(""), &mut count, true)?;
    Ok(count)
}

/// Comme [`merge_into`], sans chemin protégé : archive d'émulateur posée telle quelle
/// (une sauvegarde de départ comprise).
pub fn merge_all(src: &Path, dest: &Path) -> AppResult<u64> {
    fs::create_dir_all(dest).map_err(|e| disk_error("création du dossier", e))?;
    let mut count = 0;
    merge_dir(src, dest, Path::new(""), &mut count, false)?;
    Ok(count)
}

fn merge_dir(src_root: &Path, dest_root: &Path, rel: &Path, count: &mut u64, protect: bool) -> AppResult<()> {
    let dir = src_root.join(rel);
    for entry in fs::read_dir(&dir).map_err(|e| log_err(AppError::Disk, "lecture de l'archive extraite", e))? {
        let entry = entry.map_err(|e| log_err(AppError::Disk, "lecture de l'archive extraite", e))?;
        let child_rel = rel.join(entry.file_name());
        if protect && is_protected(&child_rel) {
            tracing::info!("chemin protégé conservé tel quel: {}", child_rel.display());
            continue;
        }
        let ft = entry.file_type().map_err(|e| log_err(AppError::Disk, "type de fichier", e))?;
        let target = dest_root.join(&child_rel);
        if ft.is_dir() {
            // Faille SL5 : un lien déjà dans le jeu (ancienne version) n'est jamais suivi, même
            // interne (`d -> .` ferait remonter les liens suivants, `d -> saves` écrirait dans
            // les sauvegardes). Comme un fichier au même endroit, il est remplacé par un vrai
            // dossier : seul le lien est retiré, jamais ce vers quoi il mène.
            // Les `lien:` (links.rs) sont posés HORS du jeu et mènent vers un vrai dossier
            // du jeu : ils ne passent pas ici.
            if target.is_symlink() {
                tracing::warn!("ancien lien remplacé par un dossier : {}", child_rel.display());
                // `remove_file` retire un lien sous Unix ; sous Windows, un lien vers un
                // dossier (ou une jonction) se retire avec `remove_dir`, sans son contenu.
                fs::remove_file(&target)
                    .or_else(|_| fs::remove_dir(&target))
                    .map_err(|e| disk_error("remplacement d'un lien", e))?;
            }
            if target.is_file() {
                fs::remove_file(&target).map_err(|e| disk_error("remplacement d'un fichier", e))?;
            }
            fs::create_dir_all(&target).map_err(|e| disk_error("création d'un dossier", e))?;
            merge_dir(src_root, dest_root, &child_rel, count, protect)?;
        } else {
            if target.is_dir() {
                // Un dossier remplacé par un fichier dans la nouvelle version : on ne
                // supprime pas un dossier existant (il peut contenir des données).
                tracing::warn!("conflit dossier/fichier, conservé: {}", child_rel.display());
                continue;
            }
            // Faille SL5 : seconde vérification de la cible, par rapport à `src_root` cette
            // fois. Après l'aplatissement (`content_root`), un lien `../x` resté dans l'archive
            // sortirait du dossier du jeu.
            if ft.is_symlink() {
                let link_target = fs::read_link(entry.path()).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default();
                if !link_target_inside(&child_rel, &link_target) {
                    tracing::warn!("lien ignoré (cible hors du jeu) : {} -> {link_target}", child_rel.display());
                    continue;
                }
            }
            if ft.is_symlink() || target.is_symlink() {
                let _ = fs::remove_file(&target);
            }
            // Même volume (dossier interne du jeu) : simple renommage.
            if fs::rename(entry.path(), &target).is_err() {
                fs::copy(entry.path(), &target).map_err(|e| disk_error("copie d'un fichier du jeu", e))?;
            }
            *count += 1;
        }
    }
    Ok(())
}

/// Copie les dossiers `save`/`saves` du jeu dans `backup_dir` (avant une mise à jour).
/// Renvoie le nombre de dossiers sauvegardés.
pub fn backup_saves(game_dir: &Path, backup_dir: &Path) -> AppResult<u32> {
    let mut found = Vec::new();
    find_save_dirs(game_dir, Path::new(""), &mut found);
    for rel in &found {
        copy_dir(&game_dir.join(rel), &backup_dir.join(rel))?;
    }
    Ok(found.len() as u32)
}

fn find_save_dirs(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(root.join(rel)) else { return };
    for entry in rd.filter_map(Result::ok) {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let child = rel.join(entry.file_name());
        if rel.as_os_str().is_empty() && (name == INTERNAL_DIR || name == MODS_DIR) {
            continue;
        }
        if name == "save" || name == "saves" {
            out.push(child);
        } else {
            find_save_dirs(root, &child, out);
        }
    }
}

pub fn copy_dir(src: &Path, dest: &Path) -> AppResult<()> {
    fs::create_dir_all(dest).map_err(|e| disk_error("création d'un dossier", e))?;
    for entry in fs::read_dir(src).map_err(|e| log_err(AppError::Disk, "lecture d'un dossier", e))?.filter_map(Result::ok) {
        let target = dest.join(entry.file_name());
        let ft = entry.file_type().map_err(|e| log_err(AppError::Disk, "type de fichier", e))?;
        if ft.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ft.is_file() {
            fs::copy(entry.path(), &target).map_err(|e| disk_error("copie d'un fichier", e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("farfa-test-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn protected_paths() {
        assert!(is_protected(Path::new("save")));
        assert!(is_protected(Path::new("Saves/slot1.dat")));
        assert!(is_protected(Path::new("data/SAVE/x")));
        assert!(is_protected(Path::new("mods_downloads/mod.zip")));
        assert!(is_protected(Path::new(".farfalauncher/staging")));
        assert!(is_protected(Path::new("farfalauncher.json")));
        assert!(!is_protected(Path::new("data/mods_downloads/x")));
        assert!(!is_protected(Path::new("savestate.bin")));
        assert!(!is_protected(Path::new("game.exe")));
    }

    #[test]
    fn merge_overwrites_but_never_deletes_nor_touches_saves() {
        let src = tmp("src");
        let dest = tmp("dest");
        fs::write(dest.join("game.exe"), "old").unwrap();
        fs::write(dest.join("user_config.ini"), "mine").unwrap();
        fs::create_dir_all(dest.join("saves")).unwrap();
        fs::write(dest.join("saves/slot1"), "progress").unwrap();

        fs::write(src.join("game.exe"), "new").unwrap();
        fs::create_dir_all(src.join("saves")).unwrap();
        fs::write(src.join("saves/slot1"), "EMPTY").unwrap();
        fs::create_dir_all(src.join("data")).unwrap();
        fs::write(src.join("data/a.bin"), "a").unwrap();

        let n = merge_into(&src, &dest).unwrap();
        assert_eq!(n, 2);
        assert_eq!(fs::read_to_string(dest.join("game.exe")).unwrap(), "new");
        assert_eq!(fs::read_to_string(dest.join("user_config.ini")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(dest.join("saves/slot1")).unwrap(), "progress");
        assert_eq!(fs::read_to_string(dest.join("data/a.bin")).unwrap(), "a");
    }

    #[test]
    fn flattens_single_root_but_not_app_bundles() {
        let s = tmp("flat");
        fs::create_dir_all(s.join("Game-1.0/bin")).unwrap();
        assert_eq!(content_root(&s).unwrap(), s.join("Game-1.0"));
        let s2 = tmp("app");
        fs::create_dir_all(s2.join("Game.app/Contents")).unwrap();
        assert_eq!(content_root(&s2).unwrap(), s2);
        let s3 = tmp("multi");
        fs::create_dir_all(s3.join("a")).unwrap();
        fs::write(s3.join("b.txt"), "").unwrap();
        assert_eq!(content_root(&s3).unwrap(), s3);
    }

    #[test]
    fn zip_roundtrip_skips_macosx_and_slips() {
        let dir = tmp("zip");
        let zpath = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&zpath).unwrap());
            let o: zip::write::SimpleFileOptions = Default::default();
            w.start_file("Game/game.exe", o).unwrap();
            w.write_all(b"exe").unwrap();
            w.start_file("__MACOSX/._junk", o).unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("../evil.txt", o).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let staging = dir.join("staging");
        extract(&zpath, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        assert!(staging.join("Game/game.exe").exists());
        assert!(!staging.join("__MACOSX").exists());
        assert!(!dir.join("evil.txt").exists());
        assert_eq!(content_root(&staging).unwrap(), staging.join("Game"));
        assert_eq!(uncompressed_size(&zpath, ArchiveKind::Zip).unwrap(), 5);
    }

    // Faille SL5 : contrôle de la cible d'un lien (fonction pure, toutes plateformes).
    #[test]
    fn link_targets_must_stay_inside() {
        let ok = |link: &str, target: &str| link_target_inside(Path::new(link), target);
        // Liens normaux : bundles macOS, `.so` versionnées, dossier voisin.
        assert!(ok("Foo.framework/Versions/Current", "A"));
        assert!(ok("Foo.framework/Foo", "Versions/Current/Foo"));
        assert!(ok("lib/libx.so", "libx.so.1"));
        assert!(ok("bin/libx.so", "../lib/libx.so.1"));
        assert!(ok("a/b/c", "./../../d"));
        // Absolus.
        assert!(!ok("x", "/home/deck/.config/autostart"));
        assert!(!ok("x", "\\Windows\\System32"));
        assert!(!ok("x", "C:\\Users"));
        assert!(!ok("x", "C:Users"));
        assert!(!ok("x", ""));
        // Remontée hors de la racine.
        assert!(!ok("x", ".."));
        assert!(!ok("bin/x", "../.."));
        assert!(!ok("a/b/x", "../../../.config"));
        assert!(!ok("bin/x", "..\\..\\y"));
        // `..` après un nom : refusé même s'il resterait dedans sur le papier.
        assert!(!ok("a/b/x", "c/../d"));
        assert!(!ok("x", "lnk/.."));
    }

    /// Zip en mémoire : `(nom, Some(cible))` pour un lien, `(nom, None)` pour un fichier.
    fn zip_with(dir: &Path, entries: &[(&str, Option<&str>)]) -> PathBuf {
        let zpath = dir.join("t.zip");
        let mut w = zip::ZipWriter::new(fs::File::create(&zpath).unwrap());
        let o: zip::write::SimpleFileOptions = Default::default();
        for (name, link) in entries {
            match link {
                Some(target) => w.add_symlink(*name, *target, o).unwrap(),
                None => {
                    w.start_file(*name, o).unwrap();
                    w.write_all(b"data").unwrap();
                }
            }
        }
        w.finish().unwrap();
        zpath
    }

    #[test]
    fn zip_refuses_links_leading_outside() {
        let dir = tmp("sl5");
        let outside = dir.join("dehors");
        fs::create_dir_all(&outside).unwrap();
        let outside_s = outside.to_string_lossy().into_owned();
        let zpath = zip_with(
            &dir,
            &[
                ("Game/abs", Some(outside_s.as_str())),
                ("Game/haut", Some("../../..")),
                ("Game/lib/libx.so.1", None),
                ("Game/lib/libx.so", Some("libx.so.1")),
                // Lien vers le dehors, puis un fichier « à travers » lui.
                ("Game/x", Some(outside_s.as_str())),
                ("Game/x/piege.desktop", None),
            ],
        );
        let staging = dir.join("staging");
        extract(&zpath, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        // Rien n'est sorti : le lien refusé n'existe pas, le fichier est resté dedans.
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        assert!(fs::symlink_metadata(staging.join("Game/abs")).is_err());
        assert!(fs::symlink_metadata(staging.join("Game/haut")).is_err());
        assert!(staging.join("Game/x").is_dir() && !staging.join("Game/x").is_symlink());
        assert!(staging.join("Game/x/piege.desktop").is_file());
        // Le lien interne est gardé (sous Unix ; ignoré sous Windows, comme avant).
        #[cfg(unix)]
        assert_eq!(fs::read_link(staging.join("Game/lib/libx.so")).unwrap(), Path::new("libx.so.1"));
        // Et la fusion dans le jeu ne pose rien dehors non plus.
        let game = dir.join("jeu");
        merge_into(&content_root(&staging).unwrap(), &game).unwrap();
        assert!(game.join("x/piege.desktop").is_file());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn zip_refuses_writing_through_an_internal_link() {
        let dir = tmp("sl5-interne");
        let zpath = zip_with(&dir, &[("vrai/a.txt", None), ("lien", Some("vrai")), ("lien/b.txt", None)]);
        let staging = dir.join("staging");
        let result = extract(&zpath, ArchiveKind::Zip, &staging, &CancellationToken::new(), |_, _| {});
        if cfg!(unix) {
            // Le lien existe : écrire « à travers » lui arrête l'extraction.
            assert!(matches!(result, Err(AppError::Extract)));
            assert!(!staging.join("vrai/b.txt").exists());
        } else {
            // Windows : pas de lien posé, `lien` est un vrai dossier.
            result.unwrap();
            assert!(staging.join("lien/b.txt").is_file());
        }
    }

    #[test]
    fn tar_refuses_links_leading_outside() {
        let dir = tmp("sl5-tar");
        let outside = dir.join("dehors");
        fs::create_dir_all(&outside).unwrap();
        let tpath = dir.join("t.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(fs::File::create(&tpath).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "Game/x", &outside).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "Game/haut", "../../..").unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_size(4);
            h.set_mode(0o644);
            b.append_data(&mut h, "Game/x/piege.desktop", &b"data"[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let staging = dir.join("staging");
        extract(&tpath, ArchiveKind::TarGz, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        assert!(fs::symlink_metadata(staging.join("Game/haut")).is_err());
        assert!(staging.join("Game/x/piege.desktop").is_file());
    }

    // Liens posés à la main (pas par une archive) : Unix seulement.
    #[cfg(unix)]
    #[test]
    fn merge_never_follows_or_copies_links_leading_outside() {
        use std::os::unix::fs::symlink;
        let dir = tmp("sl5-fusion");
        let outside = dir.join("dehors");
        fs::create_dir_all(&outside).unwrap();
        // Source aplatie : un lien `../x` resté dans l'archive sortirait du jeu.
        let src = dir.join("src");
        fs::create_dir_all(src.join("config")).unwrap();
        fs::write(src.join("config/piege.desktop"), "x").unwrap();
        symlink("../voisin", src.join("voisin")).unwrap();
        // Jeu : un vieux lien `config -> dehors` laissé par une ancienne version.
        let game = dir.join("jeu");
        fs::create_dir_all(&game).unwrap();
        symlink(&outside, game.join("config")).unwrap();
        merge_into(&src, &game).unwrap();
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        assert!(fs::symlink_metadata(game.join("voisin")).is_err());
        // Le vieux lien est devenu un vrai dossier, avec le fichier de l'archive dedans.
        assert!(!game.join("config").is_symlink());
        assert!(game.join("config/piege.desktop").is_file());
    }

    // Relecture SL5 : un lien interne déjà dans le jeu (`d -> .`) n'est pas suivi non plus,
    // sinon `d/e/x -> ../..` (accepté pour l'archive) mènerait au-dessus du jeu.
    #[cfg(unix)]
    #[test]
    fn merge_never_follows_an_internal_link_of_the_game() {
        use std::os::unix::fs::symlink;
        let dir = tmp("sl5-interne-jeu");
        let game = dir.join("jeu");
        fs::create_dir_all(game.join("saves")).unwrap();
        fs::write(game.join("saves/slot1"), "progression").unwrap();
        symlink(".", game.join("d")).unwrap();
        symlink("saves", game.join("s")).unwrap();
        let src = dir.join("src");
        fs::create_dir_all(src.join("d/e")).unwrap();
        symlink("../..", src.join("d/e/x")).unwrap();
        fs::create_dir_all(src.join("s")).unwrap();
        fs::write(src.join("s/slot1"), "VIDE").unwrap();
        merge_into(&src, &game).unwrap();
        // `d` et `s` sont de vrais dossiers : rien n'est passé par les anciens liens.
        assert!(!game.join("d").is_symlink() && game.join("d").is_dir());
        assert!(fs::symlink_metadata(game.join("e")).is_err());
        assert_eq!(fs::read_link(game.join("d/e/x")).unwrap(), Path::new("../.."));
        assert!(!game.join("s").is_symlink());
        assert_eq!(fs::read_to_string(game.join("saves/slot1")).unwrap(), "progression");
        // `d/e/x -> ../..` mène bien à la racine du jeu, pas au-dessus.
        assert_eq!(fs::canonicalize(game.join("d/e/x")).unwrap(), fs::canonicalize(&game).unwrap());
    }

    // Relecture SL5 : lien physique vers un lien symbolique interne `a/b/s -> ../../x`.
    // Copié à la racine (`h`), il mènerait au-dessus du dossier : il doit être ignoré.
    #[cfg(unix)]
    #[test]
    fn tar_ignores_hard_link_to_a_symlink() {
        let dir = tmp("sl5-tar-physique");
        let tpath = dir.join("t.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(fs::File::create(&tpath).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(4);
            h.set_mode(0o644);
            b.append_data(&mut h, "x", &b"data"[..]).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, "a/b/s", "../../x").unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "h", "a/b/s").unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "h2", "a/../a/b/s").unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let staging = dir.join("staging");
        extract(&tpath, ArchiveKind::TarGz, &staging, &CancellationToken::new(), |_, _| {}).unwrap();
        // Le lien interne est gardé, les deux liens physiques sont ignorés.
        assert_eq!(fs::read_link(staging.join("a/b/s")).unwrap(), Path::new("../../x"));
        assert!(fs::symlink_metadata(staging.join("h")).is_err());
        assert!(fs::symlink_metadata(staging.join("h2")).is_err());
    }

    #[test]
    fn backup_finds_nested_saves() {
        let g = tmp("bk");
        fs::create_dir_all(g.join("user/Saves")).unwrap();
        fs::write(g.join("user/Saves/a"), "1").unwrap();
        fs::create_dir_all(g.join("save")).unwrap();
        fs::write(g.join("save/b"), "2").unwrap();
        let b = g.join(".farfalauncher/backups/x");
        assert_eq!(backup_saves(&g, &b).unwrap(), 2);
        assert_eq!(fs::read_to_string(b.join("user/Saves/a")).unwrap(), "1");
        assert_eq!(fs::read_to_string(b.join("save/b")).unwrap(), "2");
    }
}
