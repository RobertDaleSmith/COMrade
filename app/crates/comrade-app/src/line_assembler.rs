use chrono::Local;
use serde::Serialize;

/// A completed line sent to the frontend via Tauri Channel.
#[derive(Debug, Clone, Serialize)]
pub struct SerialLine {
    /// Pre-formatted local timestamp, e.g. "12:34:56.789".
    pub timestamp: String,
    /// The text content (no trailing newline).
    pub text: String,
    /// Line kind: "received", "sent", or "system".
    pub kind: &'static str,
    /// Running total of received bytes (for status bar).
    pub rx_bytes_total: u64,
}

/// Assembles raw byte chunks into complete lines.
///
/// Buffers partial data until a newline is seen. Handles `\r\n` and bare `\n`.
/// Uses lossy UTF-8 conversion.
pub struct LineAssembler {
    buf: Vec<u8>,
    pub rx_bytes: u64,
}

impl LineAssembler {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            rx_bytes: 0,
        }
    }

    /// Feed raw bytes, returning any completed lines.
    pub fn feed(&mut self, data: &[u8], kind: &'static str) -> Vec<SerialLine> {
        if kind == "received" {
            self.rx_bytes += data.len() as u64;
        }

        let mut lines = Vec::new();
        let now = Local::now().format("%H:%M:%S%.3f").to_string();

        for &byte in data {
            if byte == b'\n' {
                let text = self.take_line();
                lines.push(SerialLine {
                    timestamp: now.clone(),
                    text,
                    kind,
                    rx_bytes_total: self.rx_bytes,
                });
            } else if byte != b'\r' {
                self.buf.push(byte);
            }
        }

        lines
    }

    /// Flush any remaining partial line (e.g. on disconnect).
    pub fn flush(&mut self, kind: &'static str) -> Option<SerialLine> {
        if self.buf.is_empty() {
            return None;
        }
        Some(SerialLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            text: self.take_line(),
            kind,
            rx_bytes_total: self.rx_bytes,
        })
    }

    /// Create a system message line.
    pub fn system_line(&self, text: &str) -> SerialLine {
        SerialLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            text: text.to_string(),
            kind: "system",
            rx_bytes_total: self.rx_bytes,
        }
    }

    fn take_line(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_line() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"hello\n", "received");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].kind, "received");
    }

    #[test]
    fn crlf_handling() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"hello\r\n", "received");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
    }

    #[test]
    fn partial_lines() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"hel", "received");
        assert_eq!(lines.len(), 0);
        let lines = asm.feed(b"lo\n", "received");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
    }

    #[test]
    fn rx_bytes_counting() {
        let mut asm = LineAssembler::new();
        asm.feed(b"hello\n", "received");
        assert_eq!(asm.rx_bytes, 6);
        asm.feed(b"world\n", "received");
        assert_eq!(asm.rx_bytes, 12);
    }

    #[test]
    fn sent_not_counted() {
        let mut asm = LineAssembler::new();
        asm.feed(b"hello\n", "sent");
        assert_eq!(asm.rx_bytes, 0);
    }

    #[test]
    fn flush_partial() {
        let mut asm = LineAssembler::new();
        asm.feed(b"partial", "received");
        let line = asm.flush("received");
        assert!(line.is_some());
        assert_eq!(line.unwrap().text, "partial");
    }

    #[test]
    fn system_line() {
        let asm = LineAssembler::new();
        let line = asm.system_line("Connected");
        assert_eq!(line.kind, "system");
        assert_eq!(line.text, "Connected");
    }
}
