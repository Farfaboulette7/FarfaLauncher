//! Bruitages de l'interface. Ils ne sont pas dans le dépôt (droits non
//! redistribuables) : ils sont servis par le serveur des assets (conteneur
//! `farfa-assets`), seulement aux utilisateurs connectés, et le launcher les
//! garde en cache. Sans eux, il reste muet.

use std::collections::HashMap;

use futures_util::future::join_all;
use serde::Serialize;
use tauri::State;

use crate::assets;
use crate::cache;
use crate::error::AppResult;
use crate::romm::client::normalize_server_url;
use crate::state::AppState;

/// Dossier des bruitages sur le serveur des assets.
pub const SOUNDS_DIR: &str = "sfx";

pub const SOUND_NAMES: &[&str] = &[
    "OpeningMenu",
    "Click",
    "deplacementFocus",
    "deplacementOption",
    "DownloadEnd",
    "LancementGame",
    "retourSelection",
    "Error",
];

/// Chemins locaux des sons disponibles (nom → fichier en cache).
#[tauri::command]
pub async fn sound_paths(state: State<'_, AppState>) -> AppResult<HashMap<String, String>> {
    let Some(base) = assets::base_url(&state.settings()) else {
        tracing::info!("aucun serveur d'assets configuré : launcher muet");
        return Ok(HashMap::new());
    };
    let client = state.client()?.with_base(&base);
    let dir = state.cache_dir.join("sounds");
    let downloads = SOUND_NAMES.iter().map(|name| {
        let client = client.clone();
        let dir = dir.clone();
        async move {
            let path = format!("{SOUNDS_DIR}/{name}.wav");
            let key = format!("{}/{path}", client.base());
            (*name, cache::cached_download_checked(&client, &dir, &key, &path, "wav", b"RIFF").await)
        }
    });
    let mut found = HashMap::new();
    for (name, res) in join_all(downloads).await {
        match res {
            Ok(p) => {
                found.insert(name.to_string(), p.to_string_lossy().into_owned());
            }
            Err(e) => tracing::debug!("son {name} indisponible: {}", e.code()),
        }
    }
    tracing::info!("{} son(s) sur {} disponibles", found.len(), SOUND_NAMES.len());
    Ok(found)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetsInfo {
    /// Adresse effective ; `None` si elle ne peut pas être déduite.
    pub url: Option<String>,
    /// `true` si saisie dans les paramètres, `false` si déduite du serveur RomM.
    pub custom: bool,
}

fn assets_info_of(state: &AppState) -> AssetsInfo {
    let settings = state.settings();
    AssetsInfo { url: assets::base_url(&settings), custom: settings.assets_url.is_some() }
}

#[tauri::command]
pub fn assets_info(state: State<'_, AppState>) -> AssetsInfo {
    assets_info_of(&state)
}

/// Enregistre l'adresse du serveur des assets ; vide ou `None` = adresse déduite.
/// Mêmes règles que pour RomM (https obligatoire hors réseau local) : le token y est envoyé.
#[tauri::command]
pub fn set_assets_url(state: State<'_, AppState>, url: Option<String>) -> AppResult<AssetsInfo> {
    let url = match url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => Some(normalize_server_url(u)?),
        None => None,
    };
    state.update_settings(|s| s.assets_url = url)?;
    Ok(assets_info_of(&state))
}
