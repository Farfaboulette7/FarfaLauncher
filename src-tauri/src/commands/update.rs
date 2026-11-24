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

/// Le fichier à installer doit venir de la release `tag` du dépôt officiel (faille SL7) :
/// `latest.json` ne peut pas envoyer le launcher télécharger ailleurs ni dans une autre release.
/// (La signature, elle, est déjà vérifiée par le module de mise à jour avec la clé publique.)
pub fn test_download_allowed(tag: &str, download_url: &str) -> bool {
    download_url.starts_with(&format!("https://github.com/Farfaboulette7/FarfaLauncher/releases/download/{tag}/"))
}

/// `true` si la version `remote` n'est pas plus ancienne que `current` (règles semver :
/// 1.2.2-beta.1 < 1.2.2 < 1.2.3-beta.1 ; la partie `+…` est ignorée). Une version illisible
/// est refusée. Sert à ne pas installer une vieille version de test, qui ramènerait une
/// faille déjà corrigée (faille SL7). Même numéro : accepté, c'est le cas normal (une version
/// de test garde le numéro du launcher dont elle part).
pub fn version_not_older(current: &str, remote: &str) -> bool {
    use std::cmp::Ordering;

    // « 1.2.3-beta.4+abc » → ([1, 2, 3], Some(["beta", "4"])).
    fn parse(v: &str) -> Option<([u64; 3], Option<Vec<&str>>)> {
        let v = v.trim().split('+').next()?;
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p.split('.').collect::<Vec<_>>())),
            None => (v, None),
        };
        let nums = core.split('.').map(|n| n.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
        let core: [u64; 3] = nums.try_into().ok()?;
        if pre.as_ref().is_some_and(|p| p.iter().any(|s| s.is_empty())) {
            return None;
        }
        Some((core, pre))
    }
    // Ordre de deux suffixes de pré-version, morceau par morceau : nombres comparés comme
    // nombres, un nombre passe avant un mot, et à égalité le plus court passe avant.
    fn cmp_pre(a: &[&str], b: &[&str]) -> Ordering {
        for (x, y) in a.iter().zip(b) {
            let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        a.len().cmp(&b.len())
    }

    let (Some((c, cp)), Some((r, rp))) = (parse(current), parse(remote)) else {
        return false;
    };
    match r.cmp(&c) {
        Ordering::Greater => true,
        Ordering::Less => false,
        // Même X.Y.Z : une version sans suffixe (stable) passe après ses pré-versions.
        Ordering::Equal => match (cp, rp) {
            (None, None) | (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(cp), Some(rp)) => cmp_pre(&rp, &cp) != Ordering::Less,
        },
    }
}

/// Les deux contrôles de la version de test trouvée (faille SL7), avec l'erreur montrée au
/// joueur : version plus ancienne → message clair (`TestVersionTooOld`) ; fichier venu
/// d'ailleurs → `NotAvailable` (cas d'attaque, le détail est dans le journal).
pub fn check_test_version(tag: &str, current: &str, remote: &str, download_url: &str) -> AppResult<()> {
    if !version_not_older(current, remote) {
        tracing::warn!("version de test {tag} refusée : {remote} est plus ancienne que {current}");
        return Err(AppError::TestVersionTooOld);
    }
    if !test_download_allowed(tag, download_url) {
        tracing::warn!("version de test {tag} refusée : fichier hors de la release {tag} du dépôt officiel");
        return Err(AppError::NotAvailable);
    }
    Ok(())
}

/// Installe la version de test d'un signalement, proposée dans la discussion avec le support.
/// Elle porte le numéro de version du launcher dont elle part : on l'installe donc même sans
/// numéro plus grand, mais jamais une version plus ancienne que celle en service, ni un
/// fichier venu d'ailleurs que la release `tag` du dépôt officiel (faille SL7). La prochaine
/// version publiée la remplace par le canal habituel.
#[tauri::command]
pub async fn install_test_version(app: AppHandle, tag: String) -> AppResult<()> {
    let url = test_endpoint(&tag).ok_or(AppError::NotAvailable)?;
    let url = url::Url::parse(&url).map_err(|e| log_err(AppError::Internal, "adresse de la version de test", e))?;
    // Le comparateur accepte tout ici : la comparaison des versions est faite juste après,
    // par version_not_older, pour pouvoir dire dans le journal pourquoi on refuse.
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
    check_test_version(&tag, &update.current_version, &update.version, update.download_url.as_str())?;
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

    // Garantit : une version de test n'est jamais plus ancienne que celle en service (SL7),
    // mais le même numéro (cas normal) passe.
    #[test]
    fn test_version_never_older_than_current() {
        // Même numéro : la version de test garde celui du launcher dont elle part.
        assert!(version_not_older("1.2.2", "1.2.2"));
        assert!(version_not_older("1.2.3-beta.1", "1.2.3-beta.1"));
        // Plus récente.
        assert!(version_not_older("1.2.2", "1.2.3"));
        assert!(version_not_older("1.2.2", "1.2.3-beta.1"));
        assert!(version_not_older("1.2.3-beta.1", "1.2.3"));
        assert!(version_not_older("1.2.3-beta.2", "1.2.3-beta.10"));
        assert!(version_not_older("1.9.0", "1.10.0"));
        // Plus ancienne : refusée.
        assert!(!version_not_older("1.2.2", "1.2.1"));
        assert!(!version_not_older("1.2.3", "1.2.3-beta.4"));
        assert!(!version_not_older("1.2.3-beta.10", "1.2.3-beta.2"));
        assert!(!version_not_older("2.0.0", "1.99.99"));
        // La partie « +… » ne compte pas.
        assert!(version_not_older("1.2.2", "1.2.2+test"));
        // Illisible : refusée.
        assert!(!version_not_older("1.2.2", "abc"));
        assert!(!version_not_older("1.2.2", "1.2"));
        assert!(!version_not_older("1.2.2", "1.2.3-"));
        assert!(!version_not_older("1.2.2", ""));
    }

    // Garantit : le fichier d'une version de test vient de la release de son tag, dans le
    // dépôt officiel, et nulle part ailleurs (SL7).
    #[test]
    fn test_download_only_from_its_own_release() {
        let tag = "test-20261117-130619-1-10625-1";
        let base = format!("https://github.com/Farfaboulette7/FarfaLauncher/releases/download/{tag}/");
        assert!(test_download_allowed(tag, &format!("{base}FaRFa.project_1.2.2_x64-setup.exe")));
        assert!(!test_download_allowed(tag, "https://github.com/Farfaboulette7/FarfaLauncher/releases/download/test-autre-1/x.exe"));
        assert!(!test_download_allowed(tag, "https://github.com/Farfaboulette7/FarfaLauncher/releases/download/v1.0.0/x.exe"));
        assert!(!test_download_allowed(tag, "https://github.com/quelquun/FarfaLauncher/releases/download/test-20261117-130619-1-10625-1/x.exe"));
        assert!(!test_download_allowed(tag, "https://exemple.invalid/x.exe"));
        assert!(!test_download_allowed(tag, &format!("http://github.com/Farfaboulette7/FarfaLauncher/releases/download/{tag}/x.exe")));
        // Un tag préfixe d'un autre ne suffit pas (« test-1 » ne couvre pas « test-10 »).
        assert!(!test_download_allowed("test-1", "https://github.com/Farfaboulette7/FarfaLauncher/releases/download/test-10/x.exe"));
    }

    // Garantit : une version de test plus ancienne est refusée avec un message clair pour le
    // joueur (code « test_version_too_old », lu par ReportsInbox.tsx), un fichier venu
    // d'ailleurs avec « not_available », et le cas normal passe.
    #[test]
    fn test_version_refusals_have_their_own_error() {
        let tag = "test-20261124-1-2";
        let url = format!("https://github.com/Farfaboulette7/FarfaLauncher/releases/download/{tag}/x.exe");
        assert!(check_test_version(tag, "1.2.2", "1.2.2", &url).is_ok());
        let trop_vieille = check_test_version(tag, "1.2.3-beta.2", "1.2.3-beta.1", &url).unwrap_err();
        assert_eq!(trop_vieille.code(), "test_version_too_old");
        assert!(trop_vieille.to_string().contains("plus ancienne que votre launcher"));
        let ailleurs = check_test_version(tag, "1.2.2", "1.2.2", "https://exemple.invalid/x.exe").unwrap_err();
        assert_eq!(ailleurs.code(), "not_available");
        let ui = include_str!("../../../src/components/ReportsInbox.tsx");
        assert!(ui.contains("\"test_version_too_old\""), "ReportsInbox.tsx doit montrer ce message au joueur");
    }

    // Garantit : la clé de signature n'est jamais donnée à la CI des branches, et les
    // workflows qui la reçoivent passent par l'approbation du propriétaire (SG3, SL6).
    #[test]
    fn signing_key_only_behind_owner_approval() {
        let ci = include_str!("../../../.github/workflows/ci.yml");
        assert!(!ci.contains("TAURI_SIGNING"), "ci.yml ne doit jamais recevoir la clé de signature");
        for (nom, wf) in [
            ("test.yml", include_str!("../../../.github/workflows/test.yml")),
            ("release.yml", include_str!("../../../.github/workflows/release.yml")),
        ] {
            assert_eq!(wf.matches("secrets.TAURI_SIGNING_PRIVATE_KEY }}").count(), 1, "{nom} : la clé dans un seul job");
            assert_eq!(wf.matches("environment: signature").count(), 1, "{nom} : environnement « signature » manquant");
            // L'environnement est déclaré dans le job qui reçoit la clé (« build »).
            // Lignes du job : de « build: » jusqu'au job suivant (ligne à deux espaces de retrait).
            let wf = wf.replace("\r\n", "\n");
            let build: String = wf
                .lines()
                .skip_while(|l| *l != "  build:")
                .skip(1)
                .take_while(|l| !(l.starts_with("  ") && l.chars().nth(2).is_some_and(|c| c.is_ascii_alphabetic())))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(build.contains("environment: signature") && build.contains("TAURI_SIGNING_PRIVATE_KEY"), "{nom} : job build");
        }
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
