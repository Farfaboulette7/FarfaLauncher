//! Favoris : la collection « favoris » de RomM (le cœur de son interface web),
//! partagée entre tous les appareils de l'utilisateur (docs/romm-api.md §5).

use serde::Deserialize;

use super::auth;
use super::client::{parse_json, RommClient};
use crate::error::{AppError, AppResult};

/// Nom donné par l'interface web de RomM quand elle crée la collection.
const COLLECTION_NAME: &str = "Favorites";

#[derive(Debug, Deserialize)]
struct RawCollection {
    id: i64,
    #[serde(default)]
    user_id: i64,
    #[serde(default)]
    is_favorite: bool,
    #[serde(default)]
    rom_ids: Vec<i64>,
}

/// Un token d'avant les favoris n'a pas les droits `collections.*` : RomM
/// répond 403. Ce n'est pas une session invalide, juste un droit à ajouter.
fn scope_error(e: AppError) -> AppError {
    match e {
        AppError::Forbidden => AppError::FavoritesScope,
        other => other,
    }
}

/// Collection favoris de l'utilisateur (une autre personne peut avoir rendu
/// publique la sienne : on filtre sur le propriétaire, comme RomM).
async fn find(client: &RommClient) -> AppResult<Option<RawCollection>> {
    let me = auth::me(client).await?;
    let collections: Vec<RawCollection> = client.get_json("api/collections").await.map_err(scope_error)?;
    Ok(collections.into_iter().find(|c| c.is_favorite && c.user_id == me.id))
}

pub async fn list(client: &RommClient) -> AppResult<Vec<i64>> {
    Ok(find(client).await?.map(|c| sorted(c.rom_ids)).unwrap_or_default())
}

/// Ajoute ou retire un jeu des favoris ; renvoie la nouvelle liste.
pub async fn set(client: &RommClient, rom_id: i64, favorite: bool) -> AppResult<Vec<i64>> {
    let collection = match find(client).await? {
        Some(c) => c,
        None if !favorite => return Ok(Vec::new()),
        None => create(client).await?,
    };
    let path = format!("api/collections/{}/roms", collection.id);
    let body = serde_json::json!({ "rom_ids": [rom_id] });
    let req = if favorite { client.post(&path, &body) } else { client.delete(&path, &body) };
    let resp = client.send(req).await.map_err(scope_error)?;
    let updated: RawCollection = parse_json(resp).await?;
    tracing::info!("favori {} : jeu {rom_id}", if favorite { "ajouté" } else { "retiré" });
    Ok(sorted(updated.rom_ids))
}

async fn create(client: &RommClient) -> AppResult<RawCollection> {
    let req = client.post_form("api/collections?is_favorite=true&is_public=false", &[("name", COLLECTION_NAME)]);
    let resp = client.send(req).await.map_err(scope_error)?;
    tracing::info!("collection favoris créée sur le serveur");
    parse_json(resp).await
}

fn sorted(mut ids: Vec<i64>) -> Vec<i64> {
    ids.sort_unstable();
    ids
}
