//! Ouverture de pages web et de dossiers dans les applications du système, et
//! réglages propres à Linux.
//!
//! Dans une AppImage, le lanceur exporte ses propres bibliothèques
//! (`LD_LIBRARY_PATH`, modules GTK/GIO/GStreamer…) : un navigateur lancé avec
//! cet environnement plante ou ne s'ouvre pas. On lance donc `xdg-open` avec
//! l'environnement d'origine, nettoyé de ce que l'AppImage a ajouté.

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::error::{log_err, AppError, AppResult};

/// À appeler avant la création de la fenêtre.
pub fn prepare_linux() {
    #[cfg(target_os = "linux")]
    {
        // WebKitGTK : avec le pilote propriétaire NVIDIA, le rendu « DMA-BUF »
        // laisse la fenêtre vide ou figée. Ailleurs on le garde : c'est le plus
        // fluide. Variable respectée si l'utilisateur l'a déjà définie.
        let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
        if nvidia && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}

/// Ouvre une adresse web dans le navigateur par défaut.
pub fn open_url(app: &AppHandle, url: &str) -> AppResult<()> {
    if appimage_open(url) {
        return Ok(());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| log_err(AppError::Internal, "ouverture du navigateur", e))
}

/// Ouvre un dossier ou un fichier dans le gestionnaire de fichiers.
pub fn open_path(app: &AppHandle, path: &str, context: &str) -> AppResult<()> {
    if appimage_open(path) {
        return Ok(());
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| log_err(AppError::Internal, context, e))
}

/// Programme lancé depuis l'AppImage (un jeu…) : on lui rend l'environnement
/// du système, sinon il charge les bibliothèques de l'AppImage et plante.
pub fn use_system_env(cmd: &mut std::process::Command) {
    #[cfg(target_os = "linux")]
    if let Some(appdir) = std::env::var_os("APPDIR") {
        clean_env(cmd, &appdir.to_string_lossy());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// Dans une AppImage : `xdg-open` (ou `gio open`) avec un environnement propre.
/// Renvoie `false` hors AppImage, ou si aucun des deux n'a pu être lancé.
#[cfg(target_os = "linux")]
fn appimage_open(target: &str) -> bool {
    use std::process::{Command, Stdio};
    let Some(appdir) = std::env::var_os("APPDIR").map(|d| d.to_string_lossy().into_owned()) else {
        return false;
    };
    for (program, args) in [("xdg-open", vec![target]), ("gio", vec!["open", target])] {
        let mut cmd = Command::new(program);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        clean_env(&mut cmd, &appdir);
        match cmd.spawn() {
            Ok(_) => return true,
            Err(e) => tracing::warn!("{program} introuvable ou en échec: {e}"),
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn appimage_open(_target: &str) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn clean_env(cmd: &mut std::process::Command, appdir: &str) {
    const ADDED_BY_APPIMAGE: &[&str] = &[
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "GTK_PATH",
        "GTK_EXE_PREFIX",
        "GTK_DATA_PREFIX",
        "GTK_IM_MODULE_FILE",
        "GDK_PIXBUF_MODULE_FILE",
        "GDK_PIXBUF_MODULEDIR",
        "GIO_MODULE_DIR",
        "GIO_EXTRA_MODULES",
        "GSETTINGS_SCHEMA_DIR",
        "GST_PLUGIN_PATH",
        "GST_PLUGIN_PATH_1_0",
        "GST_PLUGIN_SYSTEM_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GST_PLUGIN_SCANNER",
        "GST_PLUGIN_SCANNER_1_0",
        "PYTHONHOME",
        "PYTHONPATH",
        "PERLLIB",
        "QT_PLUGIN_PATH",
        "WEBKIT_DISABLE_DMABUF_RENDERER",
    ];
    for var in ADDED_BY_APPIMAGE {
        cmd.env_remove(var);
    }
    // Listes de chemins : on retire seulement les entrées de l'AppImage.
    for var in ["PATH", "XDG_DATA_DIRS", "XDG_CONFIG_DIRS"] {
        if let Some(value) = std::env::var_os(var) {
            let kept: Vec<_> = std::env::split_paths(&value).filter(|p| !p.starts_with(appdir)).collect();
            match std::env::join_paths(kept) {
                Ok(v) if !v.is_empty() => {
                    cmd.env(var, v);
                }
                _ => {
                    cmd.env_remove(var);
                }
            }
        }
    }
}
