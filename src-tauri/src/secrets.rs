//! Stockage du Client API Token dans le trousseau sécurisé de l'OS
//! (Gestionnaire d'identification Windows, Trousseau macOS, Secret Service
//! sous Linux). Le token n'est jamais écrit en clair sur le disque.

use keyring::Entry;

use crate::error::{log_err, AppError, AppResult};

const SERVICE: &str = "FarfaLauncher";

fn entry(server_url: &str) -> AppResult<Entry> {
    Entry::new(SERVICE, server_url).map_err(|e| log_err(AppError::Keychain, "ouverture du trousseau", e))
}

pub fn store_token(server_url: &str, token: &str) -> AppResult<()> {
    entry(server_url)?
        .set_password(token)
        .map_err(|e| log_err(AppError::Keychain, "écriture du token dans le trousseau", e))
}

pub fn load_token(server_url: &str) -> AppResult<Option<String>> {
    match entry(server_url)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(log_err(AppError::Keychain, "lecture du token dans le trousseau", e)),
    }
}

pub fn delete_token(server_url: &str) -> AppResult<()> {
    match entry(server_url)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(log_err(AppError::Keychain, "suppression du token du trousseau", e)),
    }
}
