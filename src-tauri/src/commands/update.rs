//! Mise à jour du launcher, par canal : stable (la dernière release GitHub) ou bêta
//! (« Recevoir les versions de test » dans les Paramètres).
//!
//! Le canal bêta lit `latest.json` sur la release technique `canal-beta`, que `release.yml`
//! remplace à chaque publication, bêta ou stable : un testeur reçoit donc aussi les versions
//! stables. Une pré-release n'est jamais la « dernière release » de GitHub : les launchers
//! du canal stable ne la voient pas.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::error::{log_err, AppError, AppResult};
use crate::state::AppState;
use crate::error::LockOrRecover;

const STABLE: &str = "https://github.com/Farfaboulette7/FarfaLauncher/releases/latest/download/latest.json";
const BETA: &str = "https://github.com/Farfaboulette7/FarfaLauncher/releases/download/canal-beta/latest.json";
/// Évènement de progression du téléchargement de la mise à jour.
const PROGRESS_EVENT: &str = "update-progress";

/// Adresses lues, dans l'ordre : la suivante sert si la précédente ne répond pas (canal bêta
/// pas encore publié…).
pub fn endpoints(beta: bool) -> Vec<&'static str> {
    if beta {
        vec![BETA, STABLE]
    } else {
        vec![STABLE]
    }
}

/// Mise à jour trouvée par `check_update`, gardée pour `install_update`.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
    /// Version de test (pré-release).
    pub beta: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "event")]
enum UpdateProgress {
    Progress { done: u64, total: Option<u64> },
    Finished,
}

/// Cherche une nouvelle version sur le canal choisi.
#[tauri::command]
pub async fn check_update(app: AppHandle, state: State<'_, AppState>, pending: State<'_, PendingUpdate>) -> AppResult<Option<UpdateInfo>> {
    let beta = state.settings().beta_channel;
    let urls = endpoints(beta)
        .into_iter()
        .map(|u| url::Url::parse(u).map_err(|e| log_err(AppError::Internal, "adresse de mise à jour", e)))
        .collect::<AppResult<Vec<_>>>()?;
    let updater = app
        .updater_builder()
        .endpoints(urls)
        .and_then(|b| b.build())
        .map_err(|e| log_err(AppError::Internal, "module de mise à jour", e))?;
    let found = updater.check().await.map_err(|e| log_err(AppError::ServerUnreachable, "vérification des mises à jour", e))?;
    let info = found.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        body: u.body.clone(),
        beta: u.version.contains('-'),
    });
    if let Some(i) = &info {
        tracing::info!("mise à jour disponible : {} → {} (canal {})", i.current_version, i.version, if beta { "bêta" } else { "stable" });
    }
    *pending.0.lock_or_recover() = found;
    Ok(info)
}

/// Télécharge et installe la mise à jour trouvée ; l'interface relance ensuite le launcher.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> AppResult<()> {
    let update = app.state::<PendingUpdate>().0.lock_or_recover().clone().ok_or(AppError::NotAvailable)?;
    let mut done = 0u64;
    let emitter = app.clone();
    update
        .download_and_install(
            |chunk, total| {
                done += chunk as u64;
                let _ = emitter.emit(PROGRESS_EVENT, UpdateProgress::Progress { done, total });
            },
            || {
                let _ = app.emit(PROGRESS_EVENT, UpdateProgress::Finished);
            },
        )
        .await
        .map_err(|e| log_err(AppError::Download, "installation de la mise à jour", e))
}

/// Adresse du `latest.json` d'une version de test de correction (release `test-<rapport>-<n>`,
/// construite par `test.yml`). Seuls les noms produits par le support sont acceptés.
pub fn test_endpoint(tag: &str) -> Option<String> {
    let ok = tag.starts_with("test-") && tag.len() <= 80 && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| format!("https://github.com/Farfaboulette7/FarfaLauncher/releases/download/{tag}/latest.json"))
}

/// Installe la version de test d'un signalement, proposée dans la discussion avec le support.
/// Elle porte le numéro de version du launcher dont elle part : on l'installe donc même sans
/// numéro plus grand. La prochaine version publiée la remplace par le canal habituel.
#[tauri::command]
pub async fn install_test_version(app: AppHandle, tag: String) -> AppResult<()> {
    let url = test_endpoint(&tag).ok_or(AppError::NotAvailable)?;
    let url = url::Url::parse(&url).map_err(|e| log_err(AppError::Internal, "adresse de la version de test", e))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .and_then(|b| b.version_comparator(|_, _| true).build())
        .map_err(|e| log_err(AppError::Internal, "module de mise à jour", e))?;
    let update = updater
        .check()
        .await
        .map_err(|e| log_err(AppError::ServerUnreachable, "version de test", e))?
        .ok_or(AppError::NotAvailable)?;
    tracing::info!("version de test {tag} : {} → {}", update.current_version, update.version);
    *app.state::<PendingUpdate>().0.lock_or_recover() = Some(update);
    install_update(app).await
}

/// Canal choisi : `true` pour recevoir les versions de test.
#[tauri::command]
pub fn beta_channel(state: State<'_, AppState>) -> bool {
    state.settings().beta_channel
}

#[tauri::command]
pub fn set_beta_channel(state: State<'_, AppState>, beta: bool) -> AppResult<()> {
    state.update_settings(|s| s.beta_channel = beta)?;
    tracing::info!("canal des mises à jour : {}", if beta { "bêta" } else { "stable" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_falls_back_to_stable() {
        assert_eq!(endpoints(false), [STABLE]);
        assert_eq!(endpoints(true), [BETA, STABLE]);
        // L'adresse stable est celle des launchers déjà installés (tauri.conf.json).
        let conf = include_str!("../../tauri.conf.json");
        assert!(conf.contains(STABLE));
    }

    #[test]
    fn test_versions_only_from_support_tags() {
        assert_eq!(
            test_endpoint("test-20261117-130619-1-10625-1").as_deref(),
            Some("https://github.com/Farfaboulette7/FarfaLauncher/releases/download/test-20261117-130619-1-10625-1/latest.json")
        );
        assert_eq!(test_endpoint("v1.1.0"), None);
        assert_eq!(test_endpoint("test-../../x"), None);
        assert_eq!(test_endpoint("test-a/b"), None);
    }

    // Les adresses des deux canaux sont des URL valides (sinon check_update échoue toujours).
    #[test]
    fn channel_endpoints_are_valid_urls() {
        for beta in [false, true] {
            for u in endpoints(beta) {
                let url = url::Url::parse(u).unwrap();
                assert_eq!(url.scheme(), "https");
                assert!(u.ends_with("/latest.json"));
            }
        }
    }

    // Le canal bêta lit la release « canal-beta » que release.yml remplit à chaque publication.
    #[test]
    fn beta_channel_matches_the_release_workflow() {
        let release = include_str!("../../../.github/workflows/release.yml");
        assert!(BETA.contains("/releases/download/canal-beta/"));
        assert!(release.contains("tag: 'canal-beta'"), "release.yml ne remplit plus la release canal-beta");
    }

    // Les tags acceptés pour une version de test sont ceux que test.yml compile (`test-*`).
    #[test]
    fn test_tags_match_the_test_workflow() {
        let workflow = include_str!("../../../.github/workflows/test.yml");
        assert!(workflow.contains(r#"tags: ["test-*"]"#));
        assert!(test_endpoint("test-x").is_some());
        assert_eq!(test_endpoint("Test-x"), None);
        assert_eq!(test_endpoint("test-a b"), None);
    }

    // Un tag de plus de 80 caractères est refusé (garde-fou sur l'adresse construite).
    #[test]
    fn test_tag_length_is_bounded() {
        let ok = format!("test-{}", "1".repeat(75));
        assert_eq!(ok.len(), 80);
        assert!(test_endpoint(&ok).is_some());
        assert_eq!(test_endpoint(&format!("{ok}1")), None);
    }
}
