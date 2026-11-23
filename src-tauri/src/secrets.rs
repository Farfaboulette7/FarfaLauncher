//! Stockage du Client API Token dans le trousseau sécurisé de l'OS
//! (Gestionnaire d'identification Windows, Trousseau macOS, Secret Service
//! sous Linux).
//!
//! Linux sans trousseau (pas de Secret Service : bureau minimal, mode Bureau du Steam Deck,
//! trousseau verrouillé…) : le token est rangé dans `<config>/token.json`, lisible par
//! l'utilisateur seul (droits 0600), comme le fait déjà le programme du Steam Deck
//! (`deck.rs`). Sans ce repli, la connexion échouait sans solution (L10).
//! Sous Windows et Mac, le trousseau existe toujours : pas de repli.

use std::path::Path;

use keyring::Entry;

use crate::error::{log_err, AppError, AppResult};

const SERVICE: &str = "FarfaLauncher";

fn entry(server_url: &str) -> AppResult<Entry> {
    Entry::new(SERVICE, server_url).map_err(|e| log_err(AppError::Keychain, "ouverture du trousseau", e))
}

fn keyring_store(server_url: &str, token: &str) -> AppResult<()> {
    entry(server_url)?
        .set_password(token)
        .map_err(|e| log_err(AppError::Keychain, "écriture du token dans le trousseau", e))
}

fn keyring_load(server_url: &str) -> AppResult<Option<String>> {
    match entry(server_url)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(log_err(AppError::Keychain, "lecture du token dans le trousseau", e)),
    }
}

fn keyring_delete(server_url: &str) -> AppResult<()> {
    match entry(server_url)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(log_err(AppError::Keychain, "suppression du token du trousseau", e)),
    }
}

/// Le repli en fichier ne sert que sous Linux.
const FILE_FALLBACK: bool = cfg!(target_os = "linux");

pub fn store_token(config_dir: &Path, server_url: &str, token: &str) -> AppResult<()> {
    match keyring_store(server_url, token) {
        Ok(()) => {
            // Un ancien token rangé en fichier n'a plus lieu d'être.
            let _ = file::remove(config_dir, server_url);
            Ok(())
        }
        Err(e) if FILE_FALLBACK => {
            tracing::warn!("trousseau indisponible : token rangé dans token.json (droits 0600)");
            file::store(config_dir, server_url, token).map_err(|_| e)
        }
        Err(e) => Err(e),
    }
}

pub fn load_token(config_dir: &Path, server_url: &str) -> AppResult<Option<String>> {
    match keyring_load(server_url) {
        Ok(Some(token)) => Ok(Some(token)),
        Ok(None) if FILE_FALLBACK => Ok(file::load(config_dir, server_url)),
        // Trousseau illisible : le token du fichier s'il y en a un, sinon pas connecté (le
        // joueur se reconnecte et le token ira dans le fichier).
        Err(_) if FILE_FALLBACK => Ok(file::load(config_dir, server_url)),
        other => other,
    }
}

pub fn delete_token(config_dir: &Path, server_url: &str) -> AppResult<()> {
    if FILE_FALLBACK {
        file::remove(config_dir, server_url).map_err(|e| log_err(AppError::Internal, "suppression de token.json", e))?;
        // Sans trousseau, il n'y a rien à y effacer.
        if let Err(e) = keyring_delete(server_url) {
            tracing::warn!("trousseau indisponible à la déconnexion ({})", e.code());
        }
        return Ok(());
    }
    keyring_delete(server_url)
}

/// Repli : `token.json` = { adresse du serveur : token }, lisible par l'utilisateur seul.
mod file {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("token.json")
    }

    fn read_all(config_dir: &Path) -> BTreeMap<String, String> {
        std::fs::read_to_string(path(config_dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write_all(config_dir: &Path, tokens: &BTreeMap<String, String>) -> std::io::Result<()> {
        let path = path(config_dir);
        if tokens.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        std::fs::create_dir_all(config_dir)?;
        let tmp = path.with_extension("json.tmp");
        {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(serde_json::to_string_pretty(tokens).unwrap_or_default().as_bytes())?;
        }
        std::fs::rename(&tmp, &path)
    }

    pub fn store(config_dir: &Path, server_url: &str, token: &str) -> std::io::Result<()> {
        let mut tokens = read_all(config_dir);
        tokens.insert(server_url.to_string(), token.to_string());
        write_all(config_dir, &tokens)
    }

    pub fn load(config_dir: &Path, server_url: &str) -> Option<String> {
        read_all(config_dir).remove(server_url)
    }

    pub fn remove(config_dir: &Path, server_url: &str) -> std::io::Result<()> {
        let mut tokens = read_all(config_dir);
        if tokens.remove(server_url).is_none() {
            return Ok(());
        }
        write_all(config_dir, &tokens)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // L10 : le token rangé en fichier se relit, ne sert qu'à son serveur, et s'efface.
        #[test]
        fn token_file_round_trip() {
            let dir = std::env::temp_dir().join(format!("farfa-jeton-{}", uuid::Uuid::new_v4()));
            store(&dir, "https://romm.example.org", "rmm_essai").unwrap();
            assert_eq!(load(&dir, "https://romm.example.org").as_deref(), Some("rmm_essai"));
            assert_eq!(load(&dir, "https://autre.example.org"), None);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "token.json lisible par d'autres");
            }
            remove(&dir, "https://romm.example.org").unwrap();
            assert!(!path(&dir).exists());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
