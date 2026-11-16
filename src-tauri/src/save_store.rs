//! Où vit la sauvegarde d'un jeu : sur RomM, ou sur FarfaRomManager pour une recomp, dont la
//! fiche RomM peut disparaître. La première fois, celle d'une recomp est reprise de RomM.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use reqwest::header::HeaderValue;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::{log_err, AppError, AppResult};
use crate::install::state::InstalledGame;
use crate::manager::{self, Catalog};
use crate::romm::client::{check_status, error_chain, parse_json, RommClient};
use crate::romm::saves::{self as romm, ServerSave};
use crate::settings::Settings;

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// En-tête qui donne au gestionnaire le nom de l'appareil (historique des sauvegardes).
const DEVICE_HEADER: &str = "X-Farfa-Device";
/// Jeux déjà contrôlés pour la reprise depuis RomM.
const IMPORTED_FILE: &str = "recomp-saves-imported.json";

/// Sauvegarde présente sur le serveur.
pub struct RemoteSave {
    pub updated_at: String,
    /// Absente : sauvegarde de FarfaRomManager.
    romm: Option<ServerSave>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerSave {
    updated_at: String,
}

/// Version précédente d'une sauvegarde, gardée par FarfaRomManager.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveVersion {
    /// Identifiant à rendre pour la restaurer.
    pub version: String,
    /// Date de son envoi (RFC 3339).
    pub saved_at: String,
    /// Date à laquelle une autre l'a remplacée.
    pub replaced_at: String,
    pub size: u64,
    /// Appareil qui l'a envoyée (inconnu pour les plus anciennes).
    #[serde(default)]
    pub device: Option<String>,
}

/// Nom de cet appareil, tel qu'il apparaît dans l'historique des sauvegardes.
pub fn device_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Ordinateur".to_string())
}

/// Nom de l'appareil pour l'en-tête HTTP : ASCII visible seulement, 64 caractères au plus.
fn device_header() -> Option<HeaderValue> {
    let name: String = device_name().chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(64).collect();
    HeaderValue::from_str(name.trim()).ok().filter(|v| !v.is_empty())
}

pub enum SaveStore {
    Romm { client: RommClient, rom_id: i64 },
    Recomp(RecompSave),
}

pub struct RecompSave {
    romm: RommClient,
    manager: RommClient,
    /// Ancienne fiche `portage-*` du jeu, d'où sa sauvegarde est reprise une fois. Sans elle,
    /// la sauvegarde RomM de la ROM est celle d'un émulateur : elle ne va pas à la recomp.
    rom_id: Option<i64>,
    path: String,
}

/// Recomp et jeu sous lesquels ce jeu installé garde ses sauvegardes. Lancé par un émulateur,
/// il reste sur RomM, même si sa console a aussi une recomp.
fn recomp_key(catalog: &Catalog, st: &InstalledGame) -> Option<(String, String)> {
    if st.shared.as_ref().is_some_and(|s| s.is_emulator()) {
        return None;
    }
    key_for_rom(catalog, st.rom_id)
}

fn key_for_rom(catalog: &Catalog, rom_id: i64) -> Option<(String, String)> {
    let recomp = catalog.recomp_for_rom(rom_id)?;
    let game = recomp.games.iter().find(|g| g.rom_id == Some(rom_id))?.game.clone();
    Some((recomp.id.clone(), game.unwrap_or_else(|| recomp.id.clone())))
}

impl SaveStore {
    pub fn of(catalog: &Catalog, settings: &Settings, client: RommClient, st: &InstalledGame) -> Self {
        match (recomp_key(catalog, st), manager::base_url(settings)) {
            (Some((recomp, game)), Some(base)) => SaveStore::Recomp(RecompSave::new(client, &base, catalog.old_save_id(st.rom_id), &recomp, &game)),
            _ => SaveStore::Romm { client, rom_id: st.rom_id },
        }
    }

    pub async fn find(&self) -> AppResult<Option<RemoteSave>> {
        match self {
            SaveStore::Romm { client, rom_id } => Ok(romm::find(client, *rom_id).await?.map(|s| RemoteSave { updated_at: s.updated_at.clone(), romm: Some(s) })),
            SaveStore::Recomp(save) => match save.find().await? {
                Some(found) => Ok(Some(found)),
                None => save.import_from_romm().await,
            },
        }
    }

    pub async fn download(&self, remote: &RemoteSave) -> AppResult<Vec<u8>> {
        match (self, &remote.romm) {
            (SaveStore::Romm { client, .. }, Some(save)) => romm::download(client, save).await,
            (SaveStore::Recomp(save), _) => save.download().await,
            _ => Err(AppError::Internal),
        }
    }

    /// Remplace la sauvegarde du serveur ; renvoie sa nouvelle date.
    pub async fn upload(&self, zip: Vec<u8>) -> AppResult<String> {
        match self {
            SaveStore::Romm { client, rom_id } => Ok(romm::upload(client, *rom_id, zip).await?.updated_at),
            SaveStore::Recomp(save) => save.upload(zip).await,
        }
    }

    /// Versions précédentes ; `None` : pas d'historique (sauvegarde gardée par RomM, qui la remplace).
    pub async fn history(&self) -> AppResult<Option<Vec<SaveVersion>>> {
        match self {
            SaveStore::Romm { .. } => Ok(None),
            SaveStore::Recomp(save) => save.history().await.map(Some),
        }
    }

    /// Remet une version précédente en place sur le serveur (l'actuelle passe dans l'historique).
    pub async fn restore(&self, version: &str) -> AppResult<String> {
        match self {
            SaveStore::Romm { .. } => Err(AppError::NotAvailable),
            SaveStore::Recomp(save) => save.restore(version).await,
        }
    }
}

impl RecompSave {
    pub fn new(romm: RommClient, manager_base: &str, rom_id: Option<i64>, recomp: &str, game: &str) -> Self {
        let manager = romm.with_base(manager_base);
        Self { romm, manager, rom_id, path: format!("api/saves/{recomp}/{game}") }
    }

    pub async fn find(&self) -> AppResult<Option<RemoteSave>> {
        let resp = self.manager.get(&self.path).send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let save: ManagerSave = parse_json(check_status(resp).await?).await?;
        Ok(Some(RemoteSave { updated_at: save.updated_at, romm: None }))
    }

    pub async fn download(&self) -> AppResult<Vec<u8>> {
        let resp = self.manager.send(self.manager.get_stream(&format!("{}/content", self.path))).await?;
        let bytes = resp.bytes().await.map_err(|e| log_err(AppError::Download, "téléchargement de la sauvegarde", e))?;
        Ok(bytes.to_vec())
    }

    pub async fn upload(&self, zip: Vec<u8>) -> AppResult<String> {
        let mut req = self.manager.put_bytes(&self.path, zip).timeout(UPLOAD_TIMEOUT);
        if let Some(device) = device_header() {
            req = req.header(DEVICE_HEADER, device);
        }
        let save: ManagerSave = parse_json(self.manager.send(req).await?).await?;
        Ok(save.updated_at)
    }

    pub async fn history(&self) -> AppResult<Vec<SaveVersion>> {
        self.manager.get_json(&format!("{}/history", self.path)).await
    }

    pub async fn restore(&self, version: &str) -> AppResult<String> {
        // Identifiant venu du serveur : chiffres et « T » seulement (pas de chemin).
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == 'T') {
            return Err(AppError::NotAvailable);
        }
        let mut req = self.manager.post(&format!("{}/history/{version}/restore", self.path), &serde_json::json!({}));
        if let Some(device) = device_header() {
            req = req.header(DEVICE_HEADER, device);
        }
        let save: ManagerSave = parse_json(self.manager.send(req).await?).await?;
        Ok(save.updated_at)
    }

    /// Copie sur FarfaRomManager la sauvegarde gardée sur la fiche RomM, qui reste en place.
    pub async fn import_from_romm(&self) -> AppResult<Option<RemoteSave>> {
        let Some(rom_id) = self.rom_id else { return Ok(None) };
        let old = match romm::find(&self.romm, rom_id).await {
            Ok(old) => old,
            // Token sans droit sur les sauvegardes RomM : il n'en a jamais envoyé.
            Err(AppError::ScopeMissing) => None,
            Err(e) => return Err(e),
        };
        let Some(old) = old else { return Ok(None) };
        let zip = romm::download(&self.romm, &old).await?;
        let updated_at = self.upload(zip).await?;
        tracing::info!("sauvegarde du jeu {} reprise de RomM ({})", rom_id, self.path);
        Ok(Some(RemoteSave { updated_at, romm: None }))
    }
}

/// Reprend de RomM les sauvegardes de toutes les recomps, installées ou non : elles sont ainsi à
/// l'abri quand les fiches `portage-*` disparaissent. Un jeu réussi n'est plus recontrôlé.
pub async fn import_all(config_dir: &Path, catalog: &Catalog, settings: &Settings, client: &RommClient) {
    let Some(base) = manager::base_url(settings) else { return };
    let path = config_dir.join(IMPORTED_FILE);
    let mut done: BTreeSet<String> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let before = done.len();
    let rom_ids: Vec<i64> = catalog.recomps.iter().flat_map(|r| r.games.iter().filter_map(|g| g.rom_id)).collect();
    for rom_id in rom_ids {
        let Some((recomp, game)) = key_for_rom(catalog, rom_id) else { continue };
        let key = format!("{recomp}/{game}");
        if done.contains(&key) {
            continue;
        }
        let save = RecompSave::new(client.clone(), &base, catalog.old_save_id(rom_id), &recomp, &game);
        let checked = match save.find().await {
            Ok(Some(_)) => Ok(()),
            Ok(None) => save.import_from_romm().await.map(|_| ()),
            Err(e) => Err(e),
        };
        match checked {
            Ok(()) => {
                done.insert(key);
            }
            Err(e) => tracing::warn!("sauvegarde de {key} non reprise de RomM ({}), nouvel essai au prochain démarrage", e.code()),
        }
    }
    if done.len() != before {
        match serde_json::to_string(&done) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&path, text) {
                    tracing::warn!("reprise des sauvegardes non mémorisée: {e}");
                }
            }
            Err(e) => tracing::warn!("reprise des sauvegardes non mémorisée: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        serde_json::from_str(
            r#"{"recomps":[{"id":"pokemon","games":[{"romId":7,"game":"red"},{"romId":null,"game":"blue"}]},{"id":"melee","games":[{"romId":9}]}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn reads_the_manager_history() {
        let v: Vec<SaveVersion> = serde_json::from_str(
            r#"[{"version":"20261116T020000123456","savedAt":"2026-11-16T01:00:00+00:00","replacedAt":"2026-11-16T02:00:00.123456+00:00","size":12,"device":null},
                {"version":"20261115T020000123456","savedAt":"2026-11-15T01:00:00+00:00","replacedAt":"2026-11-15T02:00:00+00:00","size":8}]"#,
        )
        .unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].device, None);
        assert_eq!(v[0].size, 12);
    }

    #[test]
    fn device_header_is_plain_ascii() {
        if let Some(h) = device_header() {
            assert!(h.to_str().is_ok());
            assert!(h.len() <= 64);
        }
    }

    #[test]
    fn recomps_have_a_key_of_their_own() {
        let c = catalog();
        assert_eq!(key_for_rom(&c, 7), Some(("pokemon".into(), "red".into())));
        // Recomp à un seul jeu : son identifiant sert de jeu.
        assert_eq!(key_for_rom(&c, 9), Some(("melee".into(), "melee".into())));
        // Une ROM de console reste sur RomM.
        assert_eq!(key_for_rom(&c, 8), None);
    }
}
