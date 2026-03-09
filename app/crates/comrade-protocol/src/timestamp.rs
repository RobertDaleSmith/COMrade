use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A timestamp combining wall-clock time and monotonic microseconds since session start.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Timestamp {
    /// Wall-clock time (UTC).
    pub wall: DateTime<Utc>,
    /// Microseconds elapsed since the engine started.
    pub mono_us: u64,
}

impl Timestamp {
    pub fn new(wall: DateTime<Utc>, mono_us: u64) -> Self {
        Self { wall, mono_us }
    }
}
