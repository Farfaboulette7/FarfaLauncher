//! Sauvegardes sur le serveur (docs/romm-api.md §7) : une archive zip par jeu
//! et par utilisateur, rangée par RomM sous l'« émulateur » `farfa-project`.

use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::client::{parse_json, RommClient};
use crate::error::{log_err, AppError, AppResult};

/// Nom de l'archive côté serveur (écrasée à chaque envoi).
pub const SAVE_FILE_NAME: &str = "farfa-save.zip";
/// Dossier « émulateur » de RomM : sépare nos archives des sauvegardes d'émulateurs.
pub const EMULATOR: &str = "farfa-project";
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSave {
    pub id: i64,
    pub file_name: String,
    #[serde(default)]
    pub emulator: Option<String>,
    pub updated_at: String,
}

/// Un token d'avant la synchronisation n'a pas les droits `assets.*` : RomM
/// répond 403. Ce n'est pas une session invalide, juste un droit à ajouter.
fn scope_error(e: AppError) -> AppError {
    match e {
        AppError::Forbidden => AppError::ScopeMissing,
        other => other,
    }
}

/// Notre archive pour ce jeu, si elle existe.
pub async fn find(client: &RommClient, rom_id: i64) -> AppResult<Option<ServerSave>> {
    let saves: Vec<ServerSave> = client.get_json(&format!("api/saves?rom_id={rom_id}")).await.map_err(scope_error)?;
    Ok(saves
        .into_iter()
        .filter(|s| s.file_name == SAVE_FILE_NAME && s.emulator.as_deref() == Some(EMULATOR))
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at)))
}

pub async fn download(client: &RommClient, save: &ServerSave) -> AppResult<Vec<u8>> {
    let resp = client.send(client.get_stream(&format!("api/saves/{}/content", save.id))).await.map_err(scope_error)?;
    let bytes = resp.bytes().await.map_err(|e| log_err(AppError::Download, "téléchargement de la sauvegarde", e))?;
    Ok(bytes.to_vec())
}

/// Envoie l'archive (remplace la précédente) et renvoie la sauvegarde créée.
pub async fn upload(client: &RommClient, rom_id: i64, zip: Vec<u8>) -> AppResult<ServerSave> {
    let part = Part::bytes(zip)
        .file_name(SAVE_FILE_NAME)
        .mime_str("application/zip")
        .map_err(|e| log_err(AppError::Internal, "envoi de la sauvegarde", e))?;
    let form = Form::new().part("saveFile", part);
    let path = format!("api/saves?rom_id={rom_id}&emulator={EMULATOR}&overwrite=true");
    let req = client.post_multipart(&path, form).timeout(UPLOAD_TIMEOUT);
    let resp = client.send(req).await.map_err(scope_error)?;
    parse_json(resp).await
}
