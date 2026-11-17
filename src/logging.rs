//! Einfacher Datei-Logger: %APPDATA%\DeuClip\deuclip.log

use log::{LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_LOG_BYTES: u64 = 1_000_000;

struct FileLogger {
    file: Mutex<File>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Eigene Meldungen ab Info, Fremdbibliotheken nur Warnungen und Fehler.
        if metadata.target().starts_with("deuclip") {
            metadata.level() <= log::Level::Info
        } else {
            metadata.level() <= log::Level::Warn
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} [{}] {}: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut f) = self.file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

pub fn log_path() -> PathBuf {
    crate::config::app_dir().join("deuclip.log")
}

/// Startet den Datei-Logger. Gibt den Pfad zurueck, falls es geklappt hat.
pub fn init() -> Option<PathBuf> {
    let dir = crate::config::app_dir();
    fs::create_dir_all(&dir).ok()?;
    let path = log_path();
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = fs::rename(&path, dir.join("deuclip.old.log"));
        }
    }
    let file = OpenOptions::new().create(true).append(true).open(&path).ok()?;
    let logger = Box::new(FileLogger {
        file: Mutex::new(file),
    });
    log::set_boxed_logger(logger).ok()?;
    log::set_max_level(LevelFilter::Info);

    std::panic::set_hook(Box::new(|info| {
        log::error!("PANIK: {info}");
        log::logger().flush();
    }));
    log::info!("DeuClip {} gestartet", env!("CARGO_PKG_VERSION"));
    Some(path)
}
