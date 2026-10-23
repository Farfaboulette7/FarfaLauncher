//! Logs détaillés dans un fichier tournant par jour (dossier de logs de l'OS),
//! jamais à l'écran. Aucun token ni secret n'y est écrit.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, EnvFilter};

pub fn init(log_dir: &Path) -> Option<WorkerGuard> {
    if let Err(e) = std::fs::create_dir_all(log_dir) {
        eprintln!("impossible de créer le dossier de logs: {e}");
        return None;
    }
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("farfalauncher")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,farfalauncher_lib=debug"));
    fmt().with_env_filter(filter).with_writer(writer).with_ansi(false).init();
    tracing::info!("FarfaLauncher {} démarré ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    Some(guard)
}
