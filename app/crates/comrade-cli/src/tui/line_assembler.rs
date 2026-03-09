use chrono::{DateTime, Local};

/// A completed line of text with metadata.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// Local timestamp when the line was completed.
    pub timestamp: DateTime<Local>,
    /// The text content (no trailing newline).
    pub text: String,
    /// What kind of line this is.
    pub kind: LineKind,
}

/// Classification of a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Data received from the device.
    Received,
    /// Data sent to the device.
    Sent,
    /// System message (connect, disconnect, error).
    System,
}

/// Assembles raw byte chunks into complete lines.
///
/// Buffers partial data until a newline is seen. Handles `\r\n` and bare `\n`.
/// Uses lossy UTF-8 conversion.
pub struct LineAssembler {
    buf: Vec<u8>,
}

impl LineAssembler {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed raw bytes, returning any completed lines.
    pub fn feed(&mut self, data: &[u8], kind: LineKind) -> Vec<LogLine> {
        let mut lines = Vec::new();
        let now = Local::now();

        for &byte in data {
            if byte == b'\n' {
                let text = self.take_line();
                lines.push(LogLine {
                    timestamp: now,
                    text,
                    kind,
                });
            } else if byte != b'\r' {
                self.buf.push(byte);
            }
            // \r is silently dropped
        }

        lines
    }

    /// Flush any remaining partial line (e.g. on disconnect).
    pub fn flush(&mut self, kind: LineKind) -> Option<LogLine> {
        if self.buf.is_empty() {
            return None;
        }
        Some(LogLine {
            timestamp: Local::now(),
            text: self.take_line(),
            kind,
        })
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
        let lines = asm.feed(b"hello\n", LineKind::Received);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
        assert_eq!(lines[0].kind, LineKind::Received);
    }

    #[test]
    fn crlf_handling() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"hello\r\n", LineKind::Received);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
    }

    #[test]
    fn multiple_lines() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"one\ntwo\nthree\n", LineKind::Received);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "one");
        assert_eq!(lines[1].text, "two");
        assert_eq!(lines[2].text, "three");
    }

    #[test]
    fn partial_lines() {
        let mut asm = LineAssembler::new();

        let lines = asm.feed(b"hel", LineKind::Received);
        assert_eq!(lines.len(), 0);

        let lines = asm.feed(b"lo\n", LineKind::Received);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "hello");
    }

    #[test]
    fn flush_partial() {
        let mut asm = LineAssembler::new();
        asm.feed(b"partial", LineKind::Received);

        let line = asm.flush(LineKind::Received);
        assert!(line.is_some());
        assert_eq!(line.unwrap().text, "partial");
    }

    #[test]
    fn flush_empty() {
        let mut asm = LineAssembler::new();
        assert!(asm.flush(LineKind::Received).is_none());
    }

    #[test]
    fn empty_lines() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"\n\n", LineKind::Received);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "");
        assert_eq!(lines[1].text, "");
    }

    #[test]
    fn lossy_utf8() {
        let mut asm = LineAssembler::new();
        // Invalid UTF-8 byte sequence.
        let lines = asm.feed(b"\xff\xfe\n", LineKind::Received);
        assert_eq!(lines.len(), 1);
        // Should contain replacement characters.
        assert!(lines[0].text.contains('\u{FFFD}'));
    }

    #[test]
    fn mixed_kinds() {
        let mut asm = LineAssembler::new();
        let lines = asm.feed(b"sent\n", LineKind::Sent);
        assert_eq!(lines[0].kind, LineKind::Sent);
    }
}
