use serde::{Deserialize, Serialize};

/// Serial port configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SerialConfig {
    /// Baud rate (e.g. 9600, 115200).
    pub baud_rate: u32,
    /// Number of data bits.
    pub data_bits: DataBits,
    /// Parity checking mode.
    pub parity: Parity,
    /// Number of stop bits.
    pub stop_bits: StopBits,
    /// Flow control mode.
    pub flow_control: FlowControl,
}

impl Default for SerialConfig {
    fn default() -> Self {
        Self {
            baud_rate: 115200,
            data_bits: DataBits::Eight,
            parity: Parity::None,
            stop_bits: StopBits::One,
            flow_control: FlowControl::None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DataBits {
    Five,
    Six,
    Seven,
    Eight,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Parity {
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum StopBits {
    One,
    Two,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FlowControl {
    None,
    Hardware,
    Software,
}

/// Strategy for reconnecting after a port disconnects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReconnectStrategy {
    /// Do not reconnect.
    Disabled,
    /// Reopen the same device path with exponential backoff.
    #[default]
    Direct,
    /// Watch for a matching USB VID:PID to reappear.
    ByUsbId { vid: u16, pid: u16 },
    /// Connect to the most recently appeared serial device.
    Latest,
}
