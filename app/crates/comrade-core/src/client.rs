//! Daemon client — connects to a running daemon over Unix socket.
//!
//! Provides the same interface as Engine (send commands, receive events)
//! so frontends can swap in with minimal changes.

use std::path::Path;
use std::time::Duration;

use comrade_protocol::{Command, DaemonRequest, DaemonResponse, Event, SerialConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

use crate::daemon::socket_path_for;
use crate::error::CoreError;

const EVENT_CAPACITY: usize = 4096;

/// Client handle to a daemon. Send commands, receive events.
pub struct DaemonClient {
    req_tx: mpsc::Sender<DaemonRequest>,
    event_tx: broadcast::Sender<Event>,
}

impl DaemonClient {
    /// Connect to an existing daemon, or spawn one if none exists.
    pub async fn connect_or_spawn(port: &str, config: &SerialConfig) -> Result<Self, CoreError> {
        let sock = socket_path_for(port);

        // Try connecting to existing daemon.
        if sock.exists() {
            if let Ok(client) = Self::connect_to(&sock).await {
                debug!("Connected to existing daemon for {port}");
                return Ok(client);
            }
            // Stale socket — remove it.
            let _ = std::fs::remove_file(&sock);
        }

        // Spawn a new daemon process.
        Self::spawn_daemon(port, config)?;

        // Wait for daemon to start (retry with backoff up to 3s).
        for i in 0..15 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(client) = Self::connect_to(&sock).await {
                debug!("Connected to new daemon for {port} (attempt {i})");
                return Ok(client);
            }
        }

        Err(CoreError::Other("Daemon failed to start".to_string()))
    }

    /// Connect to a daemon at the given socket path.
    async fn connect_to(sock: &Path) -> Result<Self, CoreError> {
        let stream = UnixStream::connect(sock)
            .await
            .map_err(|e| CoreError::Other(format!("Socket connect: {e}")))?;

        let (reader, writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        let (req_tx, mut req_rx) = mpsc::channel::<DaemonRequest>(256);
        let (event_tx, _) = broadcast::channel::<Event>(EVENT_CAPACITY);

        // Writer task: forwards requests to the socket.
        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(req) = req_rx.recv().await {
                let mut line = match serde_json::to_string(&req) {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                line.push('\n');
                if writer.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Reader task: reads events from the socket and broadcasts them.
        let evt_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let resp: DaemonResponse = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                match resp {
                    DaemonResponse::Event { event } => {
                        let _ = evt_tx.send(event);
                    }
                    DaemonResponse::Pong => {}
                    DaemonResponse::Error { message } => {
                        warn!("Daemon error: {message}");
                    }
                }
            }
        });

        Ok(Self { req_tx, event_tx })
    }

    /// Spawn a daemon as a background process.
    fn spawn_daemon(port: &str, config: &SerialConfig) -> Result<(), CoreError> {
        // Find the comrade binary.
        let exe = which_comrade()?;

        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--daemon")
            .arg(port)
            .arg("-b")
            .arg(config.baud_rate.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        // Detach on Unix so daemon outlives parent.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }

        cmd.spawn()
            .map_err(|e| CoreError::Other(format!("Failed to spawn daemon: {e}")))?;

        Ok(())
    }

    /// Subscribe to the event stream.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.event_tx.subscribe()
    }

    /// Receive the next event.
    pub async fn recv(&mut self) -> Result<Event, CoreError> {
        let mut rx = self.event_tx.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => return Ok(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return Err(CoreError::Shutdown),
            }
        }
    }

    /// Send a command (translated to DaemonRequest).
    pub async fn send_command(&self, cmd: Command) -> Result<(), CoreError> {
        let req = match cmd {
            Command::Connect { port, config } => DaemonRequest::Connect { port, config },
            Command::Send { data } => DaemonRequest::Send { data },
            Command::Disconnect => DaemonRequest::Disconnect,
            Command::SetDtr { active } => DaemonRequest::SetDtr { active },
            Command::SetRts { active } => DaemonRequest::SetRts { active },
            Command::SendBreak => DaemonRequest::SendBreak,
            Command::Shutdown => DaemonRequest::Disconnect,
            Command::ListPorts => return Ok(()), // Not supported through daemon.
        };
        self.req_tx
            .send(req)
            .await
            .map_err(|_| CoreError::Shutdown)
    }

    /// Get a command sender that translates Command -> DaemonRequest.
    pub fn cmd_sender(&self) -> DaemonCmdSender {
        DaemonCmdSender {
            req_tx: self.req_tx.clone(),
        }
    }
}

/// Adapter: sends `Command` as `DaemonRequest` over the daemon socket.
/// Drop-in replacement for `mpsc::Sender<Command>` in existing code.
#[derive(Clone)]
pub struct DaemonCmdSender {
    req_tx: mpsc::Sender<DaemonRequest>,
}

impl DaemonCmdSender {
    pub async fn send(&self, cmd: Command) -> Result<(), CoreError> {
        let req = match cmd {
            Command::Connect { port, config } => DaemonRequest::Connect { port, config },
            Command::Send { data } => DaemonRequest::Send { data },
            Command::Disconnect => DaemonRequest::Disconnect,
            Command::SetDtr { active } => DaemonRequest::SetDtr { active },
            Command::SetRts { active } => DaemonRequest::SetRts { active },
            Command::SendBreak => DaemonRequest::SendBreak,
            Command::Shutdown => DaemonRequest::Disconnect,
            Command::ListPorts => return Ok(()),
        };
        self.req_tx
            .send(req)
            .await
            .map_err(|_| CoreError::Shutdown)
    }
}

/// Find the `comrade` binary in PATH or next to current exe.
fn which_comrade() -> Result<std::path::PathBuf, CoreError> {
    // Check PATH.
    if let Ok(output) = std::process::Command::new("which")
        .arg("comrade")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(path.into());
            }
        }
    }

    // Check next to current executable.
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("comrade");
        if sibling.exists() {
            return Ok(sibling);
        }
    }

    Err(CoreError::Other(
        "Cannot find 'comrade' binary. Install with: cargo install --path crates/comrade-cli"
            .to_string(),
    ))
}
