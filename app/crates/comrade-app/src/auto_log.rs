use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Local;

use crate::log_buffer::LogEntry;

/// Auto-logger that writes each log entry to disk immediately.
///
/// Uses `BufWriter` with explicit flush after each entry for fail-safe
/// persistence without excessive syscalls.
pub struct AutoLogger {
    inner: Mutex<Option<LogWriter>>,
}

struct LogWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    count: usize,
}

impl AutoLogger {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Start auto-logging to a new file in the given directory.
    /// Returns the path of the created log file.
    pub fn start(&self, directory: &str) -> Result<String, String> {
        let dir = PathBuf::from(directory);
        if !dir.is_dir() {
            return Err(format!("Not a directory: {directory}"));
        }

        let ts = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("comrade_{ts}.log");
        let path = dir.join(&filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Failed to create log file: {e}"))?;

        let mut inner = self.inner.lock().unwrap();
        *inner = Some(LogWriter {
            writer: BufWriter::new(file),
            path: path.clone(),
            count: 0,
        });

        Ok(path.to_string_lossy().to_string())
    }

    /// Stop auto-logging and return the path + entry count.
    pub fn stop(&self) -> Option<(String, usize)> {
        let mut inner = self.inner.lock().unwrap();
        inner.take().map(|w| {
            let path = w.path.to_string_lossy().to_string();
            (path, w.count)
        })
    }

    /// Write a log entry to disk immediately. No-op if not active.
    pub fn write(&self, entry: &LogEntry) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ref mut w) = *inner {
            let line = entry.format_line();
            let _ = writeln!(w.writer, "{line}");
            let _ = w.writer.flush();
            w.count += 1;
        }
    }

    /// Whether auto-logging is currently active.
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    /// Path of the current log file, if active.
    pub fn current_path(&self) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .as_ref()
            .map(|w| w.path.to_string_lossy().to_string())
    }
}
