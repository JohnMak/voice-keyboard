//! File-based logging for the updater/launcher
//!
//! Logs all launcher and update actions to logs.txt with timestamps.

use anyhow::Result;
use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Log file writer
pub struct UpdateLogger {
    file: Mutex<File>,
    path: PathBuf,
}

impl UpdateLogger {
    /// Create a new logger, opening the log file for append
    pub fn new(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        Ok(Self {
            file: Mutex::new(file),
            path,
        })
    }

    /// Log a message with timestamp
    pub fn log(&self, message: &str) {
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{}] {}\n", timestamp, message);

        // Also print to stderr
        eprint!("{}", line);

        // Write to file
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    /// Get the path to the log file
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

/// Simple standalone logging function when UpdateLogger isn't available
pub fn log_to_file(path: &std::path::Path, message: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let line = format!("[{}] {}\n", timestamp, message);

    // Print to stderr
    eprint!("{}", line);

    // Append to file
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}
