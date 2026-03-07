use std::collections::VecDeque;
use std::sync::Mutex;

use crate::hid_session::HidReport;
use crate::line_assembler::SerialLine;

const MAX_ENTRIES: usize = 10_000;

/// A unified log entry wrapping either a serial line or a HID report.
#[derive(Debug, Clone)]
pub enum LogEntry {
    Serial(SerialLine),
    Hid(HidReport),
}

#[allow(dead_code)]
impl LogEntry {
    pub fn timestamp(&self) -> &str {
        match self {
            LogEntry::Serial(l) => &l.timestamp,
            LogEntry::Hid(r) => &r.timestamp,
        }
    }

    pub fn kind(&self) -> &str {
        match self {
            LogEntry::Serial(l) => l.kind,
            LogEntry::Hid(r) => r.kind,
        }
    }

    /// Human-readable text representation for MCP output.
    pub fn text_content(&self) -> String {
        match self {
            LogEntry::Serial(l) => l.text.clone(),
            LogEntry::Hid(r) => {
                if r.data.is_empty() {
                    r.hex.clone()
                } else {
                    format!("{} | {}", r.hex, r.ascii)
                }
            }
        }
    }

    /// Formatted line for MCP tool output.
    pub fn format_line(&self) -> String {
        let kind_label = match self {
            LogEntry::Serial(l) => l.kind.to_string(),
            LogEntry::Hid(r) => format!("hid:{}", r.kind),
        };
        format!("[{}] [{}] {}", self.timestamp(), kind_label, self.text_content())
    }
}

/// Thread-safe ring buffer of log entries.
///
/// Uses `std::sync::Mutex` because all operations are fast in-memory
/// with no `.await` while the lock is held.
pub struct LogBuffer {
    inner: Mutex<VecDeque<LogEntry>>,
}

#[allow(dead_code)]
impl LogBuffer {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)),
        }
    }

    pub fn push(&self, entry: LogEntry) {
        let mut buf = self.inner.lock().unwrap();
        if buf.len() >= MAX_ENTRIES {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Return the last `n` entries.
    pub fn tail(&self, n: usize) -> Vec<LogEntry> {
        let buf = self.inner.lock().unwrap();
        let skip = buf.len().saturating_sub(n);
        buf.iter().skip(skip).cloned().collect()
    }

    /// Return entries with timestamps strictly greater than `since`.
    pub fn since(&self, since: &str) -> Vec<LogEntry> {
        let buf = self.inner.lock().unwrap();
        buf.iter()
            .filter(|e| e.timestamp() > since)
            .cloned()
            .collect()
    }

    /// Search entries by pattern (regex, falls back to substring).
    pub fn search(&self, pattern: &str, max: usize) -> Vec<LogEntry> {
        let buf = self.inner.lock().unwrap();
        let re = regex::Regex::new(pattern).ok();
        buf.iter()
            .filter(|e| {
                let text = e.text_content();
                match &re {
                    Some(re) => re.is_match(&text),
                    None => text.contains(pattern),
                }
            })
            .rev()
            .take(max)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}
