//! Jeux Steam sans DRM : un fichier `steam_appid.txt` à côté du jeu suffit
//! pour qu'il démarre sans passer par Steam. L'identifiant sert aussi à umu
//! pour appliquer les correctifs propres au jeu sous Linux.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::net;

const APPID_FILE: &str = "steam_appid.txt";
const STEAM_API: &[&str] = &["steam_api.dll", "steam_api64.dll", "libsteam_api.so"];
/// Profondeur explorée pour trouver les fichiers Steam du jeu.
const MAX_DEPTH: usize = 4;

fn find_files(dir: &Path, names: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if depth < MAX_DEPTH && name != crate::install::extract::INTERNAL_DIR {
                find_files(&path, names, depth + 1, out);
            }
        } else if names.iter().any(|n| e.file_name().to_string_lossy().eq_ignore_ascii_case(n)) {
            out.push(path);
        }
    }
}

/// Identifiant Steam déjà écrit dans le jeu.
pub fn read_app_id(game_dir: &Path) -> Option<u32> {
    let mut found = Vec::new();
    find_files(game_dir, &[APPID_FILE], 0, &mut found);
    found.iter().find_map(|p| std::fs::read_to_string(p).ok()?.trim().parse().ok())
}

/// Dossiers qui contiennent l'API Steam mais pas encore `steam_appid.txt`.
fn missing_app_id(game_dir: &Path) -> Vec<PathBuf> {
    let mut apis = Vec::new();
    find_files(game_dir, STEAM_API, 0, &mut apis);
    let mut dirs: Vec<PathBuf> = apis.into_iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    dirs.dedup();
    dirs.retain(|d| !d.join(APPID_FILE).exists());
    dirs
}

#[derive(Deserialize)]
struct Search {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    id: u32,
    name: String,
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Recherche l'identifiant d'un jeu dans le magasin Steam (nom exact).
async fn search(name: &str) -> Option<u32> {
    let url = format!(
        "https://store.steampowered.com/api/storesearch/?l=english&cc=US&term={}",
        url::form_urlencoded::byte_serialize(name.as_bytes()).collect::<String>()
    );
    let found: Search = serde_json::from_str(&net::get_text(&net::client(), &url).await.ok()?).ok()?;
    let want = squash(name);
    found.items.into_iter().find(|i| squash(&i.name) == want).map(|i| i.id)
}

/// Identifiant Steam du jeu : celui du jeu s'il en a un, sinon cherché sur
/// Steam et écrit à côté de l'API Steam (au mieux, sans erreur bloquante).
pub async fn ensure_app_id(game_dir: &Path, name: &str) -> Option<u32> {
    let dir = game_dir.to_path_buf();
    let (known, missing) = tokio::task::spawn_blocking(move || (read_app_id(&dir), missing_app_id(&dir))).await.ok()?;
    if missing.is_empty() {
        return known;
    }
    let id = match known {
        Some(id) => id,
        None => search(name).await?,
    };
    for d in missing {
        match std::fs::write(d.join(APPID_FILE), id.to_string()) {
            Ok(()) => tracing::info!("{APPID_FILE} ({id}) écrit dans {}", d.display()),
            Err(e) => tracing::warn!("{APPID_FILE} non écrit dans {}: {e}", d.display()),
        }
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_completes_app_ids() {
        let dir = std::env::temp_dir().join(format!("farfa-steam-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/steam_api64.dll"), b"").unwrap();
        assert_eq!(read_app_id(&dir), None);
        assert_eq!(missing_app_id(&dir), [dir.join("bin")]);
        std::fs::write(dir.join("steam_appid.txt"), "367520\n").unwrap();
        // Déjà connu ailleurs dans le jeu : recopié sans réseau.
        let id = tokio::runtime::Runtime::new().unwrap().block_on(ensure_app_id(&dir, "Hollow Knight"));
        assert_eq!(id, Some(367520));
        assert_eq!(std::fs::read_to_string(dir.join("bin/steam_appid.txt")).unwrap(), "367520");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
