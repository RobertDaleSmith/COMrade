use std::time::Instant;

use chrono::Utc;
use comrade_protocol::{Command, DataBits, Event, FlowControl, Parity, PortInfo, SerialConfig, StopBits, Timestamp};
use serialport::SerialPort;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::error::CoreError;

const CMD_CHANNEL_CAPACITY: usize = 256;
const EVENT_CHANNEL_CAPACITY: usize = 512;
const READ_BUF_SIZE: usize = 4096;

/// The serial engine. Owns the command/event channels and manages the connection.
pub struct Engine {
    cmd_tx: mpsc::Sender<Command>,
    event_rx: broadcast::Receiver<Event>,
    epoch: Instant,
}

impl Engine {
    /// Create a new engine and spawn its background task.
    /// Returns the engine handle for sending commands and receiving events.
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel(CMD_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let epoch = Instant::now();

        tokio::spawn(engine_loop(cmd_rx, event_tx, epoch));

        Self {
            cmd_tx,
            event_rx,
            epoch,
        }
    }

    /// Subscribe to the event stream. Each subscriber gets its own receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_rx.resubscribe()
    }

    /// Send a command to the engine.
    pub async fn send(&self, cmd: Command) -> Result<(), CoreError> {
        self.cmd_tx
            .send(cmd)
            .await
            .map_err(|_| CoreError::Shutdown)
    }

    /// Receive the next event.
    pub async fn recv(&mut self) -> Result<Event, CoreError> {
        loop {
            match self.event_rx.recv().await {
                Ok(event) => return Ok(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("event receiver lagged, skipped {n} events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CoreError::Shutdown);
                }
            }
        }
    }

    /// Get the engine epoch for timestamp calculations.
    pub fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Get a clone of the command sender for use outside the lock.
    pub fn cmd_sender(&self) -> mpsc::Sender<Command> {
        self.cmd_tx.clone()
    }
}

fn make_timestamp(epoch: Instant) -> Timestamp {
    Timestamp::new(Utc::now(), epoch.elapsed().as_micros() as u64)
}

fn to_serialport_config(config: &SerialConfig) -> (u32, serialport::DataBits, serialport::Parity, serialport::StopBits, serialport::FlowControl) {
    let data_bits = match config.data_bits {
        DataBits::Five => serialport::DataBits::Five,
        DataBits::Six => serialport::DataBits::Six,
        DataBits::Seven => serialport::DataBits::Seven,
        DataBits::Eight => serialport::DataBits::Eight,
    };
    let parity = match config.parity {
        Parity::None => serialport::Parity::None,
        Parity::Odd => serialport::Parity::Odd,
        Parity::Even => serialport::Parity::Even,
    };
    let stop_bits = match config.stop_bits {
        StopBits::One => serialport::StopBits::One,
        StopBits::Two => serialport::StopBits::Two,
    };
    let flow = match config.flow_control {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::Hardware => serialport::FlowControl::Hardware,
        FlowControl::Software => serialport::FlowControl::Software,
    };
    (config.baud_rate, data_bits, parity, stop_bits, flow)
}

/// List available serial ports (blocking, should be called from async context).
async fn list_ports_async() -> Result<Vec<PortInfo>, CoreError> {
    tokio::task::spawn_blocking(crate::port::enumerate_ports)
        .await
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?
}

async fn engine_loop(
    mut cmd_rx: mpsc::Receiver<Command>,
    event_tx: broadcast::Sender<Event>,
    epoch: Instant,
) {
    let mut port: Option<tokio_serial::SerialStream> = None;
    let mut read_buf = vec![0u8; READ_BUF_SIZE];

    info!("engine started");

    loop {
        // If we have no port open, just wait for commands.
        if port.is_none() {
            match cmd_rx.recv().await {
                Some(cmd) => {
                    handle_command(cmd, &mut port, &event_tx, epoch).await;
                }
                None => {
                    debug!("command channel closed, shutting down");
                    let _ = event_tx.send(Event::Shutdown);
                    return;
                }
            }
            continue;
        }

        // We have a port open -- select between reading data and receiving commands.
        let serial = port.as_mut().unwrap();
        tokio::select! {
            result = serial.read(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        info!("serial port EOF");
                        let _ = event_tx.send(Event::Disconnected {
                            ts: make_timestamp(epoch),
                            port: String::new(),
                            reason: "EOF".into(),
                        });
                        port = None;
                    }
                    Ok(n) => {
                        let data = read_buf[..n].to_vec();
                        let _ = event_tx.send(Event::Data {
                            ts: make_timestamp(epoch),
                            bytes: data,
                        });
                    }
                    Err(e) => {
                        error!("serial read error: {e}");
                        let _ = event_tx.send(Event::Disconnected {
                            ts: make_timestamp(epoch),
                            port: String::new(),
                            reason: e.to_string(),
                        });
                        port = None;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        handle_command(cmd, &mut port, &event_tx, epoch).await;
                    }
                    None => {
                        debug!("command channel closed, shutting down");
                        let _ = event_tx.send(Event::Shutdown);
                        return;
                    }
                }
            }
        }
    }
}

async fn handle_command(
    cmd: Command,
    port: &mut Option<tokio_serial::SerialStream>,
    event_tx: &broadcast::Sender<Event>,
    epoch: Instant,
) {
    match cmd {
        Command::Connect { port: path, config } => {
            // If already connected to this port, just re-send the Connected event
            // so new clients get the notification.
            if port.is_some() {
                info!("already connected to {path}, notifying new client");
                let _ = event_tx.send(Event::Connected {
                    ts: make_timestamp(epoch),
                    port: path,
                    config,
                });
                return;
            }

            info!("connecting to {path} at {} baud", config.baud_rate);

            let (baud, data_bits, parity, stop_bits, flow) = to_serialport_config(&config);

            let builder = tokio_serial::new(&path, baud)
                .data_bits(data_bits)
                .parity(parity)
                .stop_bits(stop_bits)
                .flow_control(flow);

            match tokio_serial::SerialStream::open(&builder) {
                Ok(mut stream) => {
                    // Assert DTR — required for USB CDC devices (Arduino, etc.)
                    // to enable serial communication.
                    if let Err(e) = stream.write_data_terminal_ready(true) {
                        warn!("failed to set DTR: {e}");
                    }
                    info!("connected to {path}");
                    *port = Some(stream);
                    let _ = event_tx.send(Event::Connected {
                        ts: make_timestamp(epoch),
                        port: path,
                        config,
                    });
                }
                Err(e) => {
                    let mut msg = format!("failed to open {path}: {e}");

                    // Check what process holds the port (macOS/Linux).
                    if e.to_string().contains("busy") || e.to_string().contains("Resource busy") {
                        if let Ok(output) = std::process::Command::new("lsof")
                            .arg(&path)
                            .output()
                        {
                            let lsof = String::from_utf8_lossy(&output.stdout);
                            for line in lsof.lines().skip(1) {
                                let parts: Vec<&str> = line.split_whitespace().collect();
                                if parts.len() >= 2 {
                                    msg = format!(
                                        "failed to open {path}: port held by {} (PID {})",
                                        parts[0], parts[1]
                                    );
                                    break;
                                }
                            }
                        }
                    }

                    error!("{msg}");
                    let _ = event_tx.send(Event::Error {
                        ts: make_timestamp(epoch),
                        message: msg,
                    });
                }
            }
        }
        Command::Disconnect => {
            if port.is_some() {
                info!("disconnecting");
                *port = None;
                let _ = event_tx.send(Event::Disconnected {
                    ts: make_timestamp(epoch),
                    port: String::new(),
                    reason: "user requested disconnect".into(),
                });
            }
        }
        Command::Send { data } => {
            if let Some(serial) = port.as_mut() {
                match serial.write_all(&data).await {
                    Ok(()) => {
                        if let Err(e) = serial.flush().await {
                            warn!("serial flush error: {e}");
                        }
                    }
                    Err(e) => {
                        error!("write error: {e}");
                        let _ = event_tx.send(Event::Error {
                            ts: make_timestamp(epoch),
                            message: format!("write error: {e}"),
                        });
                    }
                }
            } else {
                warn!("send command but no port open");
            }
        }
        Command::SetDtr { active } => {
            if let Some(serial) = port.as_mut() {
                if let Err(e) = serial.write_data_terminal_ready(active) {
                    warn!("failed to set DTR: {e}");
                    let _ = event_tx.send(Event::Error {
                        ts: make_timestamp(epoch),
                        message: format!("DTR error: {e}"),
                    });
                } else {
                    debug!("DTR set to {active}");
                }
            }
        }
        Command::SetRts { active } => {
            if let Some(serial) = port.as_mut() {
                if let Err(e) = serial.write_request_to_send(active) {
                    warn!("failed to set RTS: {e}");
                    let _ = event_tx.send(Event::Error {
                        ts: make_timestamp(epoch),
                        message: format!("RTS error: {e}"),
                    });
                } else {
                    debug!("RTS set to {active}");
                }
            }
        }
        Command::SendBreak => {
            if let Some(serial) = port.as_mut() {
                if let Err(e) = serial.set_break() {
                    warn!("failed to send break: {e}");
                }
                // Brief break pulse.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let _ = serial.clear_break();
            }
        }
        Command::ListPorts => {
            match list_ports_async().await {
                Ok(ports) => {
                    let _ = event_tx.send(Event::PortList {
                        ts: make_timestamp(epoch),
                        ports,
                    });
                }
                Err(e) => {
                    let _ = event_tx.send(Event::Error {
                        ts: make_timestamp(epoch),
                        message: format!("failed to enumerate ports: {e}"),
                    });
                }
            }
        }
        Command::Shutdown => {
            info!("shutdown requested");
            *port = None;
            let _ = event_tx.send(Event::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_timestamp() {
        let epoch = Instant::now();
        let ts = make_timestamp(epoch);
        assert!(ts.mono_us < 1_000_000); // should be less than 1 second
    }

    #[test]
    fn test_serial_config_defaults() {
        let config = SerialConfig::default();
        assert_eq!(config.baud_rate, 115200);
        let (baud, data, par, stop, flow) = to_serialport_config(&config);
        assert_eq!(baud, 115200);
        assert!(matches!(data, serialport::DataBits::Eight));
        assert!(matches!(par, serialport::Parity::None));
        assert!(matches!(stop, serialport::StopBits::One));
        assert!(matches!(flow, serialport::FlowControl::None));
    }
}
