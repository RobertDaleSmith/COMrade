use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("serial port error: {0}")]
    Serial(#[from] serialport::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("port not found: {0}")]
    PortNotFound(String),

    #[error("not connected")]
    NotConnected,

    #[error("HID error: {0}")]
    Hid(String),

    #[error("engine shut down")]
    Shutdown,
}
