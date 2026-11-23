//! Écriture et lecture prudentes des petits fichiers JSON du dossier de config
//! (`installed.json`, `settings.json`).
//!
//! - `write_atomic` : écrit dans un fichier temporaire au nom unique, puis le renomme. Deux
//!   écritures en même temps ne se marchent pas dessus, et un arrêt brutal laisse l'ancien
//!   fichier entier.
//! - `set_aside` : un fichier illisible (tronqué, abîmé) n'est jamais écrasé. Il est renommé
//!   en `<nom>.illisible-<AAAAMMJJ-HHMMSS>` à côté, pour pouvoir le réparer à la main.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Numéro des fichiers temporaires de ce processus (rend leur nom unique).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Écrit `text` dans `path` en passant par un fichier temporaire unique, puis renommage.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Ex. : `installed.json.1234-7.tmp` (numéro du processus, puis numéro d'écriture).
    let tmp = path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Au démarrage : efface les fichiers temporaires de `write_atomic` laissés par un arrêt
/// brutal (`installed.json.<…>.tmp`, `settings.json.<…>.tmp`) de plus d'une heure.
pub fn remove_stale_tmp(config_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(config_dir) else { return };
    for e in entries.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let ours = (name.starts_with("installed.json.") || name.starts_with("settings.json.")) && name.ends_with(".tmp");
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
        if ours && old && std::fs::remove_file(e.path()).is_ok() {
            tracing::info!("fichier temporaire abandonné effacé : {name}");
        }
    }
}

/// Met de côté un fichier illisible : il est renommé en `<nom>.illisible-<date>` et l'opération
/// est journalisée. Renvoie le nouveau chemin (`None` si le renommage a échoué).
pub fn set_aside(path: &Path, why: &str) -> Option<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let date = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        date.year(),
        u8::from(date.month()),
        date.day(),
        date.hour(),
        date.minute(),
        date.second()
    );
    let mut aside = path.with_file_name(format!("{name}.illisible-{stamp}"));
    // Deux fichiers mis de côté dans la même seconde : on ajoute un numéro.
    let mut i = 1;
    while aside.exists() {
        aside = path.with_file_name(format!("{name}.illisible-{stamp}-{i}"));
        i += 1;
    }
    match std::fs::rename(path, &aside) {
        Ok(()) => {
            tracing::error!("{} illisible ({why}) : mis de côté sous {}, on repart d'un fichier vide", path.display(), aside.display());
            Some(aside)
        }
        Err(e) => {
            tracing::error!("{} illisible ({why}) et impossible à mettre de côté : {e}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Un fichier illisible est renommé, pas effacé : son contenu reste lisible à côté.
    #[test]
    fn set_aside_keeps_the_content() {
        let dir = std::env::temp_dir().join(format!("farfa-de-cote-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("installed.json");
        std::fs::write(&path, "{ tronqué").unwrap();
        let aside = set_aside(&path, "test").unwrap();
        assert!(!path.exists());
        assert!(aside.file_name().unwrap().to_string_lossy().starts_with("installed.json.illisible-"));
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ tronqué");
        // Un deuxième dans la même seconde ne remplace pas le premier.
        std::fs::write(&path, "autre").unwrap();
        let second = set_aside(&path, "test").unwrap();
        assert_ne!(second, aside);
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ tronqué");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Seuls nos fichiers temporaires de plus d'une heure sont effacés au démarrage.
    #[test]
    fn stale_tmp_files_are_removed() {
        let dir = std::env::temp_dir().join(format!("farfa-tmp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("installed.json.12-3.tmp");
        let recent = dir.join("settings.json.12-4.tmp");
        let other = dir.join("autre.tmp");
        for p in [&old, &recent, &other] {
            std::fs::write(p, "x").unwrap();
        }
        let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        for p in [&old, &other] {
            std::fs::File::options().write(true).open(p).unwrap().set_modified(two_hours_ago).unwrap();
        }
        remove_stale_tmp(&dir);
        assert!(!old.exists());
        assert!(recent.exists(), "une écriture en cours ne doit pas être effacée");
        assert!(other.exists(), "un fichier qui n'est pas à nous reste");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // L'écriture atomique ne laisse aucun fichier temporaire derrière elle.
    #[test]
    fn write_atomic_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("farfa-atomique-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("installed.json");
        write_atomic(&path, "un").unwrap();
        write_atomic(&path, "deux").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deux");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
