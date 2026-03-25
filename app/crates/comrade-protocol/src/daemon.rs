use serde::{Deserialize, Serialize};

use crate::SerialConfig;

/// Request from client to daemon (JSON line over Unix socket).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Connect { port: String, config: SerialConfig },
    Send { data: Vec<u8> },
    Disconnect,
    SetDtr { active: bool },
    SetRts { active: bool },
    SendBreak,
    Ping,
}

/// Response from daemon to client (JSON line over Unix socket).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    Event { event: crate::Event },
    Pong,
    Error { message: String },
}
