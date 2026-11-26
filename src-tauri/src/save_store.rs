//! Où vit la sauvegarde d'un jeu : sur RomM, ou sur FarfaRomManager pour une recomp, dont la
//! fiche RomM peut disparaître. La première fois, celle d'une recomp est reprise de RomM.
//!
//! Historique : FarfaRomManager garde les 10 versions précédentes de chaque sauvegarde. Pour un
//! jeu gardé par RomM (émulateur, jeu PC), RomM reste la référence ; le launcher envoie en plus
//! une copie de chaque sauvegarde au gestionnaire (`api/saves/jeux.romm/<id RomM>`), qui en tient
//! l'historique comme pour une recomp (docs/architecture.md §5).

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use reqwest::header::HeaderValue;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::{log_err, AppError, AppResult};
use crate::install::state::InstalledGame;
use crate::manager::{self, Catalog};
use crate::romm::client::{error_chain, parse_json, RommClient};
use crate::romm::saves::{self as romm, ServerSave};
use crate::settings::Settings;

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// En-tête qui donne au gestionnaire le nom de l'appareil (historique des sauvegardes).
const DEVICE_HEADER: &str = "X-Farfa-Device";
/// Jeux déjà contrôlés pour la reprise depuis RomM.
const IMPORTED_FILE: &str = "recomp-saves-imported.json";
/// « Recomp » du gestionnaire sous laquelle il garde la copie des sauvegardes RomM (le point la
/// rend impossible pour une vraie recomp).
const ROMM_GAMES: &str = "jeux.romm";

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
    /// SHA-1 de l'archive.
    #[serde(default)]
    sha1: Option<String>,
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
    /// Sauvegarde gardée par RomM. `history` : sa copie sur le gestionnaire, qui en tient
    /// l'historique (absente si aucun gestionnaire n'est configuré).
    Romm { client: RommClient, rom_id: i64, history: Option<RecompSave> },
    Recomp(RecompSave),
}

/// Sauvegarde gardée par FarfaRomManager : celle d'une recomp, ou la copie d'une sauvegarde RomM.
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
    // Chaque recomp a ses propres sauvegardes : celle de cette installation, pas la proposée.
    if st.shared.is_none() {
        if let Some(id) = crate::install::state::recomp_id_of(&st.archive_name) {
            if let Some(r) = catalog.recomps_for_rom(st.rom_id).into_iter().find(|r| r.id == id) {
                let game = r.games.iter().find(|g| g.rom_id == Some(st.rom_id)).and_then(|g| g.game.clone());
                return Some((r.id.clone(), game.unwrap_or_else(|| r.id.clone())));
            }
        }
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
            (None, base) => {
                let history = base.map(|b| RecompSave::romm_copy(client.clone(), &b, st.rom_id));
                SaveStore::Romm { client, rom_id: st.rom_id, history }
            }
            (Some(_), None) => SaveStore::Romm { client, rom_id: st.rom_id, history: None },
        }
    }

    pub async fn find(&self) -> AppResult<Option<RemoteSave>> {
        match self {
            SaveStore::Romm { client, rom_id, .. } => Ok(romm::find(client, *rom_id).await?.map(|s| RemoteSave { updated_at: s.updated_at.clone(), romm: Some(s) })),
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
            SaveStore::Romm { client, rom_id, history } => {
                let copy = history.as_ref().map(|_| zip.clone());
                let updated_at = romm::upload(client, *rom_id, zip).await?.updated_at;
                // Copie pour l'historique : son échec n'empêche jamais l'envoi, RomM a déjà la
                // sauvegarde (elle sera recopiée à la prochaine lecture de l'historique).
                if let (Some(history), Some(copy)) = (history, copy) {
                    match history.send_copy(copy, true).await {
                        Ok(true) => {}
                        Ok(false) => tracing::info!("gestionnaire sans historique des sauvegardes RomM (pas encore à jour)"),
                        Err(e) => tracing::warn!("sauvegarde du jeu {rom_id} non copiée pour l'historique ({})", e.code()),
                    }
                }
                Ok(updated_at)
            }
            SaveStore::Recomp(save) => save.upload(zip).await,
        }
    }

    /// Versions précédentes ; `None` : pas d'historique (aucun gestionnaire, ou gestionnaire d'avant
    /// l'historique des sauvegardes RomM).
    pub async fn history(&self) -> AppResult<Option<Vec<SaveVersion>>> {
        match self {
            SaveStore::Romm { history: None, .. } => Ok(None),
            SaveStore::Romm { client, rom_id, history: Some(history) } => {
                match copy_romm_current(client, *rom_id, history).await {
                    Ok(true) => {}
                    Ok(false) => return Ok(None),
                    // Simple lecture : la liste reste utile. La restauration, elle, refait ce
                    // contrôle et s'arrête s'il échoue.
                    Err(e) => tracing::warn!("sauvegarde RomM du jeu {rom_id} non comparée à sa copie ({})", e.code()),
                }
                history.history().await.map(Some)
            }
            SaveStore::Recomp(save) => save.history().await.map(Some),
        }
    }

    /// Remet une version précédente en place sur le serveur (l'actuelle passe dans l'historique).
    pub async fn restore(&self, version: &str) -> AppResult<String> {
        match self {
            SaveStore::Romm { history: None, .. } => Err(AppError::NotAvailable),
            SaveStore::Romm { client, rom_id, history: Some(history) } => {
                // D'abord, la sauvegarde actuelle de RomM doit être dans la copie du gestionnaire :
                // sinon elle serait écrasée sans passer dans l'historique. Au moindre doute
                // (erreur, gestionnaire pas à jour), on s'arrête sans rien changer.
                if !copy_romm_current(client, *rom_id, history).await? {
                    return Err(AppError::NotAvailable);
                }
                // Sur le gestionnaire : l'actuelle passe dans l'historique, la version choisie la remplace.
                history.restore(version).await?;
                // Puis sur RomM, qui reste la référence. En cas d'échec ici, RomM garde l'actuelle,
                // qui est aussi dans l'historique : rien n'est perdu, on peut recommencer.
                let zip = history.download().await?;
                Ok(romm::upload(client, *rom_id, zip).await?.updated_at)
            }
            SaveStore::Recomp(save) => save.restore(version).await,
        }
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha1::digest(bytes))
}

/// Copie la sauvegarde actuelle de RomM sur le gestionnaire si sa copie diffère (sauvegarde
/// envoyée par un ancien launcher, ou copie qui avait échoué) : l'ancienne copie passe dans
/// l'historique. Rend `false` si le gestionnaire n'a pas l'historique des sauvegardes RomM.
async fn copy_romm_current(client: &RommClient, rom_id: i64, history: &RecompSave) -> AppResult<bool> {
    let Some(current) = romm::find(client, rom_id).await? else { return Ok(true) };
    let zip = romm::download(client, &current).await?;
    let copied = history.info().await?.and_then(|s| s.sha1);
    if !needs_copy(copied.as_deref(), &zip) {
        return Ok(true);
    }
    tracing::info!("sauvegarde RomM du jeu {rom_id} copiée sur le gestionnaire pour l'historique");
    // Sans nom d'appareil : elle ne vient pas forcément de celui-ci.
    history.send_copy(zip, false).await
}

/// La copie du gestionnaire (son SHA-1) n'est pas cette sauvegarde.
fn needs_copy(copied_sha1: Option<&str>, zip: &[u8]) -> bool {
    copied_sha1 != Some(sha1_hex(zip).as_str())
}

impl RecompSave {
    pub fn new(romm: RommClient, manager_base: &str, rom_id: Option<i64>, recomp: &str, game: &str) -> Self {
        let manager = romm.with_base(manager_base);
        Self { romm, manager, rom_id, path: format!("api/saves/{recomp}/{game}") }
    }

    /// Copie de la sauvegarde RomM d'un jeu, gardée par le gestionnaire pour l'historique.
    fn romm_copy(romm: RommClient, manager_base: &str, rom_id: i64) -> Self {
        Self::new(romm, manager_base, None, ROMM_GAMES, &rom_id.to_string())
    }

    pub async fn find(&self) -> AppResult<Option<RemoteSave>> {
        Ok(self.info().await?.map(|save| RemoteSave { updated_at: save.updated_at, romm: None }))
    }

    async fn info(&self) -> AppResult<Option<ManagerSave>> {
        let resp = self.manager.get(&self.path).send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(parse_json(self.manager.check(resp).await?).await?))
    }

    /// Envoie la copie d'une sauvegarde RomM (`device` : avec le nom de cet appareil). `false` :
    /// le gestionnaire refuse ce chemin (404), il n'a pas encore l'historique des sauvegardes RomM.
    async fn send_copy(&self, zip: Vec<u8>, device: bool) -> AppResult<bool> {
        let mut req = self.manager.put_bytes(&self.path, zip).timeout(UPLOAD_TIMEOUT);
        if let Some(name) = device_header().filter(|_| device) {
            req = req.header(DEVICE_HEADER, name);
        }
        let resp = req.send().await.map_err(|e| log_err(AppError::ServerUnreachable, "requête HTTP", error_chain(&e)))?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        self.manager.check(resp).await?;
        Ok(true)
    }

    pub async fn download(&self) -> AppResult<Vec<u8>> {
        // Délai d'attente sur la réponse et entre deux morceaux (L6).
        let resp = self.manager.send_stream(self.manager.get_stream(&format!("{}/content", self.path))).await?;
        crate::romm::client::read_body(resp, "téléchargement de la sauvegarde").await
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
    fn romm_saves_are_copied_only_when_different() {
        let zip = b"PK\x03\x04sauvegarde";
        assert!(!needs_copy(Some(&sha1_hex(zip)), zip));
        assert!(needs_copy(Some(&sha1_hex(b"PK\x03\x04autre")), zip));
        // Pas encore de copie, ou gestionnaire sans SHA-1 : on copie (il dédoublonne lui-même).
        assert!(needs_copy(None, zip));
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn romm_copies_have_their_own_path() {
        let client = RommClient::new("https://romm.exemple.fr", None).unwrap();
        let copy = RecompSave::romm_copy(client, "https://manager.exemple.fr", 42);
        assert_eq!(copy.path, "api/saves/jeux.romm/42");
        // Jamais de reprise depuis RomM pour une copie : c'est déjà la sauvegarde RomM.
        assert_eq!(copy.rom_id, None);
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
