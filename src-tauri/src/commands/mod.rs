pub mod install;
pub mod launch;
pub mod mods;
pub mod music;
pub mod saves;
pub mod library;
pub mod session;
pub mod sounds;
pub mod stats;
pub mod community;
pub mod steam;
pub mod system;
pub mod update;
pub mod report;
pub mod presence;
pub mod requests;
pub mod fiches;
pub mod service;
pub mod window;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[tauri::command]
pub fn open_logs(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    crate::external::open_path(&app, &state.log_dir.to_string_lossy(), "ouverture du dossier de logs")
}

/// Erreurs de l'interface, recopiées dans le fichier de logs.
#[tauri::command]
pub fn log_frontend(level: String, message: String) {
    let message: String = message.chars().take(8000).collect();
    match level.as_str() {
        "error" => tracing::error!(target: "interface", "{message}"),
        "warn" => tracing::warn!(target: "interface", "{message}"),
        _ => tracing::info!(target: "interface", "{message}"),
    }
}

#[tauri::command]
pub fn open_url(app: tauri::AppHandle, state: State<'_, AppState>, url: String) -> AppResult<()> {
    // N'ouvre que des pages du serveur RomM configuré (ex. page d'approbation)…
    let server = state.server_url()?;
    // … ou le dépôt GitHub d'une recomp ou d'un émulateur connus.
    let known_repo = state.catalog().knows_repo(&url) || state.manifest().runtimes.iter().any(|r| r.repo == url);
    if !url_allowed(&url, &server, known_repo) {
        tracing::warn!("adresse refusée à l'ouverture : {url}");
        return Err(AppError::Internal);
    }
    crate::external::open_url(&app, &url)
}

/// Adresse que `open_url` accepte de passer au système (faille SL12) :
/// - une page du serveur RomM configuré (`<serveur>/…`, en http seulement sur le réseau
///   local, voir `normalize_server_url`) ;
/// - le dépôt d'une recomp ou d'un émulateur du catalogue, seulement en `https://` : un
///   `repo:` en `file://`, `ms-settings:`… n'est jamais ouvert.
/// Jamais d'espace ni de caractère de contrôle (l'adresse est donnée telle quelle au système).
fn url_allowed(url: &str, server: &str, known_repo: bool) -> bool {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let server_page = !server.is_empty() && url.starts_with(&format!("{server}/"));
    let https_repo = known_repo && url.starts_with("https://") && url.len() > "https://".len();
    server_page || https_repo
}

#[cfg(test)]
mod tests {
    use super::url_allowed;

    #[test]
    fn open_url_only_server_pages_and_https_repos() {
        let server = "https://romm.exemple.fr";
        assert!(url_allowed("https://romm.exemple.fr/rom/12", server, false));
        assert!(url_allowed("https://github.com/a/b", server, true));
        // Dépôt connu mais pas en https : refusé.
        for bad in ["http://github.com/a/b", "file:///C:/Windows/System32/calc.exe", "ms-settings:privacy", "https://", "javascript:alert(1)"] {
            assert!(!url_allowed(bad, server, true), "{bad}");
        }
        // Adresse inconnue, même en https.
        assert!(!url_allowed("https://ailleurs.fr/x", server, false));
        // Domaine qui commence comme le serveur : refusé (il faut le « / »).
        assert!(!url_allowed("https://romm.exemple.fr.pirate.com/x", server, false));
        assert!(!url_allowed("https://github.com/a/b c", server, true));
        // Serveur local en http : ses pages restent ouvertes (page d'approbation).
        assert!(url_allowed("http://192.168.1.10:8080/pair", "http://192.168.1.10:8080", false));
    }
}
