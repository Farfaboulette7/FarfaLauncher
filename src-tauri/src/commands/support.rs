//! Section « Soutenir le projet » : la commande du launcher. La lecture du fichier de don est
//! dans `support.rs` (partagée avec le moteur du Steam Deck).

use std::sync::Mutex;

use tauri::State;

use crate::error::LockOrRecover as _;
use crate::state::AppState;
use crate::support::{fetch, SupportInfo, SupportLink};

/// Liens `url` de la dernière lecture réussie. `open_url` (commands/mod.rs) n'ouvre que des
/// adresses connues (faille SL12) : celles-ci y sont ajoutées, et seulement elles. Remplacée à
/// chaque lecture, vidée si le fichier disparaît ou est désactivé.
static SUPPORT_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Lit les infos de don. Ne renvoie **jamais** d'erreur : `None` = section cachée.
#[tauri::command]
pub async fn support_info(state: State<'_, AppState>) -> Result<Option<SupportInfo>, ()> {
    let info = fetch(&state).await;
    // Mémorise les liens à ouvrir (ou les oublie si la section est cachée).
    let urls = info
        .iter()
        .flat_map(|i| i.links.iter())
        .filter_map(|l| match l {
            SupportLink::Url { value, .. } => Some(value.clone()),
            SupportLink::Text { .. } => None,
        })
        .collect();
    *SUPPORT_URLS.lock_or_recover() = urls;
    Ok(info)
}

/// `true` si `url` est un lien `url` de la section, tel que lu à la dernière ouverture.
pub fn is_support_link(url: &str) -> bool {
    SUPPORT_URLS.lock_or_recover().iter().any(|u| u == url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liens_memorises_pour_open_url() {
        *SUPPORT_URLS.lock_or_recover() = vec!["https://www.paypal.me/Farfaboulette".into()];
        assert!(is_support_link("https://www.paypal.me/Farfaboulette"));
        assert!(!is_support_link("https://www.paypal.me/Autre"));
        SUPPORT_URLS.lock_or_recover().clear();
        assert!(!is_support_link("https://www.paypal.me/Farfaboulette"));
    }
}
