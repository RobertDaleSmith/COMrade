mod mcp;
mod remote;
mod tui;

use std::io::{self, Write};

use anyhow::Result;
use clap::Parser;
use comrade_core::{enumerate_devices, DaemonClient};
use comrade_protocol::{DataBits, DeviceKind, Event, FlowControl, Parity, SerialConfig, StopBits};
use tokio::sync::broadcast;
use tracing::debug;

#[derive(Parser)]
#[command(
    name = "comrade",
    about = "COMrade - A modern serial monitor for hardware hackers",
    version
)]
struct Cli {
    /// Serial port device path (e.g. /dev/cu.usbserial-1420, COM3)
    #[arg(value_name = "PORT")]
    port: Option<String>,

    /// Baud rate
    #[arg(short, long, default_value = "115200")]
    baud: u32,

    /// Data bits (5, 6, 7, 8)
    #[arg(long, default_value = "8")]
    data_bits: u8,

    /// Parity (none, odd, even)
    #[arg(long, default_value = "none")]
    parity: String,

    /// Stop bits (1, 2)
    #[arg(long, default_value = "1")]
    stop_bits: u8,

    /// Flow control (none, hardware, software)
    #[arg(long, default_value = "none")]
    flow: String,

    /// List available serial ports and exit
    #[arg(short, long)]
    list: bool,

    /// Raw output mode (no TUI, print directly to stdout)
    #[arg(long)]
    raw: bool,

    /// Start as a stdio MCP server for Claude Code integration
    #[arg(long)]
    mcp: bool,

    /// Run as a background daemon for the given port (internal use)
    #[arg(long, hide = true)]
    daemon: bool,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<SubCmd>,
}

#[derive(clap::Subcommand)]
enum SubCmd {
    /// Send text to the connected device (via running COMrade instance)
    Send {
        /// Text to send (newline appended automatically)
        text: String,
    },
    /// Show recent log entries from the connected device
    Logs {
        /// Number of entries to show
        #[arg(short, long, default_value = "50")]
        count: usize,
    },
    /// Show connection status
    Status,
    /// Connect to a serial port (via running COMrade instance)
    Connect {
        /// Serial port path
        port: String,
        /// Baud rate
        #[arg(short, long, default_value = "115200")]
        baud: u32,
    },
    /// Disconnect the active connection
    Disconnect,
}

impl Cli {
    fn serial_config(&self) -> Result<SerialConfig> {
        let data_bits = match self.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            8 => DataBits::Eight,
            n => anyhow::bail!("invalid data bits: {n} (must be 5-8)"),
        };

        let parity = match self.parity.to_lowercase().as_str() {
            "none" | "n" => Parity::None,
            "odd" | "o" => Parity::Odd,
            "even" | "e" => Parity::Even,
            s => anyhow::bail!("invalid parity: {s} (must be none, odd, or even)"),
        };

        let stop_bits = match self.stop_bits {
            1 => StopBits::One,
            2 => StopBits::Two,
            n => anyhow::bail!("invalid stop bits: {n} (must be 1 or 2)"),
        };

        let flow_control = match self.flow.to_lowercase().as_str() {
            "none" | "n" => FlowControl::None,
            "hardware" | "hw" | "h" => FlowControl::Hardware,
            "software" | "sw" | "s" => FlowControl::Software,
            s => anyhow::bail!("invalid flow control: {s} (must be none, hardware, or software)"),
        };

        Ok(SerialConfig {
            baud_rate: self.baud,
            data_bits,
            parity,
            stop_bits,
            flow_control,
        })
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Set up tracing.
    let filter = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_writer(io::stderr)
        .init();

    if cli.list {
        return list_ports();
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // Subcommands — these proxy through the MCP HTTP API.
    if let Some(cmd) = cli.command {
        return rt.block_on(async {
            match cmd {
                SubCmd::Send { text } => remote::send(&text).await,
                SubCmd::Logs { count } => remote::logs(count).await,
                SubCmd::Status => remote::status().await,
                SubCmd::Connect { port, baud } => remote::connect(&port, baud).await,
                SubCmd::Disconnect => remote::disconnect().await,
            }
        });
    }

    if cli.mcp {
        return rt.block_on(mcp::run_mcp());
    }

    if cli.daemon {
        let port = cli.port.clone().unwrap_or_default();
        if port.is_empty() {
            eprintln!("error: --daemon requires a port");
            std::process::exit(1);
        }
        let config = cli.serial_config()?;
        return rt.block_on(comrade_core::run_daemon(port, config));
    }

    let port = match &cli.port {
        Some(p) => p.clone(),
        None => {
            eprintln!("error: no port specified. Use --list to see available ports.");
            std::process::exit(1);
        }
    };

    let config = cli.serial_config()?;

    if cli.raw {
        rt.block_on(run_raw(port, config))
    } else {
        tui::run_tui(&rt, port, config)
    }
}

fn list_ports() -> Result<()> {
    let devices = enumerate_devices()?;

    if devices.is_empty() {
        println!("No devices found.");
        return Ok(());
    }

    println!(
        "{:<30} {:<12} {:<10} {:<10} DESCRIPTION",
        "PORT", "TYPE", "VID", "PID"
    );
    println!("{}", "-".repeat(82));

    for dev in &devices {
        let vid = dev.vid.map(|v| format!("0x{v:04X}")).unwrap_or_default();
        let pid = dev.pid.map(|p| format!("0x{p:04X}")).unwrap_or_default();
        let desc = dev
            .product
            .as_deref()
            .or(dev.manufacturer.as_deref())
            .unwrap_or("");
        let kind = match dev.kind {
            DeviceKind::Serial => "Serial",
            DeviceKind::Hid => "HID",
            DeviceKind::Both => "Serial+HID",
            DeviceKind::Ble => "BLE",
        };
        println!("{:<30} {:<12} {:<10} {:<10} {}", dev.path, kind, vid, pid, desc);
    }

    println!("\n{} device(s) found.", devices.len());
    Ok(())
}

async fn run_raw(port: String, config: SerialConfig) -> Result<()> {
    let client = DaemonClient::connect_or_spawn(&port, &config).await?;
    let mut event_rx = client.subscribe();

    // Wait for connected event or error.
    let mut stdout = io::stdout().lock();
    let mut connected = false;

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                match event {
                    Event::Connected { port, config, .. } => {
                        connected = true;
                        eprintln!(
                            "Connected to {} at {} baud ({}{}{})",
                            port,
                            config.baud_rate,
                            data_bits_label(&config.data_bits),
                            parity_label(&config.parity),
                            stop_bits_label(&config.stop_bits),
                        );
                    }
                    Event::Data { bytes, .. } => {
                        stdout.write_all(&bytes)?;
                        stdout.flush()?;
                    }
                    Event::Disconnected { reason, .. } => {
                        if connected {
                            eprintln!("\nDisconnected: {reason}");
                        }
                        break;
                    }
                    Event::Error { message, .. } => {
                        eprintln!("Error: {message}");
                        if !connected {
                            break;
                        }
                    }
                    Event::Shutdown => {
                        debug!("engine shut down");
                        break;
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("\nInterrupted.");
                break;
            }
        }
    }

    Ok(())
}

fn data_bits_label(db: &DataBits) -> &'static str {
    match db {
        DataBits::Five => "5",
        DataBits::Six => "6",
        DataBits::Seven => "7",
        DataBits::Eight => "8",
    }
}

fn parity_label(p: &Parity) -> &'static str {
    match p {
        Parity::None => "N",
        Parity::Odd => "O",
        Parity::Even => "E",
    }
}

fn stop_bits_label(sb: &StopBits) -> &'static str {
    match sb {
        StopBits::One => "1",
        StopBits::Two => "2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cli = Cli::parse_from(["comrade", "--list"]);
        assert!(cli.list);
        assert_eq!(cli.baud, 115200);
    }

    #[test]
    fn test_custom_baud() {
        let cli = Cli::parse_from(["comrade", "-b", "9600", "--list"]);
        assert_eq!(cli.baud, 9600);
    }

    #[test]
    fn test_serial_config_parsing() {
        let cli = Cli::parse_from(["comrade", "-b", "9600", "--data-bits", "7", "--parity", "even", "--stop-bits", "2", "--flow", "hardware", "--list"]);
        let config = cli.serial_config().unwrap();
        assert_eq!(config.baud_rate, 9600);
        assert_eq!(config.data_bits, DataBits::Seven);
        assert_eq!(config.parity, Parity::Even);
        assert_eq!(config.stop_bits, StopBits::Two);
        assert_eq!(config.flow_control, FlowControl::Hardware);
    }
}
