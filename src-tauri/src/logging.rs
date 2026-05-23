use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;

static LOG_FILE_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init_file_logging(app_data_dir: &Path) -> Result<(), std::io::Error> {
    let log_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_file_path = log_dir.join("app.log");
    let _ = LOG_FILE_PATH.set(log_file_path.clone());

    let file_appender = tracing_appender::rolling::never(&log_dir, "app.log");
    let (non_blocking, guard): (_, WorkerGuard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_level(true)
        .with_max_level(tracing::Level::DEBUG)
        .try_init()
        .map_err(std::io::Error::other)?;

    Box::leak(Box::new(guard));
    tracing::info!(
        log_file = %log_file_path.display(),
        "file logging initialized"
    );

    Ok(())
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let message = if let Some(location) = panic_info.location() {
            format!(
                "panic at {}:{}:{}: {}",
                location.file(),
                location.line(),
                location.column(),
                panic_info
            )
        } else {
            format!("panic: {panic_info}")
        };

        if let Some(log_file_path) = LOG_FILE_PATH.get() {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file_path)
            {
                let _ = writeln!(file, "{}", message);
            }
        }

        eprintln!("{}", message);
    }));
}
