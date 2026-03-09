use serde::{Deserialize, Serialize};

use crate::SerialConfig;

/// Commands sent from a frontend to the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Connect to a serial port.
    Connect {
        port: String,
        config: SerialConfig,
    },
    /// Disconnect from the current port.
    Disconnect,
    /// Send raw bytes to the serial port.
    Send {
        data: Vec<u8>,
    },
    /// Request a list of available serial ports.
    ListPorts,
    /// Shut down the engine.
    Shutdown,
}
