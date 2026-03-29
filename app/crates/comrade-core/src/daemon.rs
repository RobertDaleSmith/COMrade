//! Daemon server — owns a serial Engine, multiplexes to clients over Unix socket.
//!
//! One daemon per port. Clients connect, receive events, send commands.
//! When all clients disconnect, the daemon waits a grace period then shuts down.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use comrade_protocol::{Command, DaemonRequest, DaemonResponse, Event, SerialConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc, Notify};
use tracing::{debug, error, info, warn};

use crate::Engine;

/// Compute the socket path for a given serial port.
pub fn socket_path_for(port: &str) -> PathBuf {
    let slug = port.replace(['/', '.', ' '], "_");
    PathBuf::from(format!("/tmp/comrade-{slug}.sock"))
}

/// Check if a daemon is already running for the given port.
pub fn daemon_is_running(port: &str) -> bool {
    let sock = socket_path_for(port);
    sock.exists() && std::os::unix::net::UnixStream::connect(&sock).is_ok()
}

/// Run the daemon server for a given port. This blocks until all clients
/// disconnect and the grace period expires.
pub async fn run_daemon(port: String, config: SerialConfig) -> anyhow::Result<()> {
    let sock_path = socket_path_for(&port);

    // Clean up stale socket.
    if sock_path.exists() {
        if std::os::unix::net::UnixStream::connect(&sock_path).is_ok() {
            anyhow::bail!("Daemon already running for {port}");
        }
        let _ = std::fs::remove_file(&sock_path);
    }

    let engine = Engine::spawn();
    engine.send(Command::Connect { port: port.clone(), config }).await?;

    let listener = UnixListener::bind(&sock_path)?;
    info!("Daemon listening on {}", sock_path.display());

    let client_count = Arc::new(AtomicUsize::new(0));
    let event_tx = Arc::new(broadcast::Sender::new(512));
    let shutdown_notify = Arc::new(Notify::new());

    // Broadcast engine events to all clients.
    let mut engine_rx = engine.subscribe();
    let evt_tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            match engine_rx.recv().await {
                Ok(event) => {
                    let _ = evt_tx.send(event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    // Grace period watcher — shuts down when no clients for 2 seconds.
    let count_clone = client_count.clone();
    let notify_clone = shutdown_notify.clone();
    let engine_cmd = engine.cmd_sender();
    let sock_cleanup = sock_path.clone();
    let mut grace_handle = tokio::spawn(async move {
        // Wait for at least one client before starting the watcher.
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if count_clone.load(Ordering::Relaxed) > 0 {
                break;
            }
            // If no client connects within 10s, shut down.
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            let start = START.get_or_init(std::time::Instant::now);
            if start.elapsed() > std::time::Duration::from_secs(10) {
                info!("No clients connected within 10s, shutting down daemon");
                let _ = engine_cmd.send(Command::Shutdown).await;
                let _ = std::fs::remove_file(&sock_cleanup);
                return;
            }
        }

        // Watch for all clients disconnecting.
        loop {
            tokio::select! {
                _ = notify_clone.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }

            if count_clone.load(Ordering::Relaxed) > 0 {
                continue;
            }

            debug!("No clients, starting 2s grace period");
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            if count_clone.load(Ordering::Relaxed) == 0 {
                info!("Grace period expired, shutting down daemon");
                let _ = engine_cmd.send(Command::Shutdown).await;
                let _ = std::fs::remove_file(&sock_cleanup);
                return;
            }
            debug!("Client reconnected during grace period, continuing");
        }
    });

    // Accept loop.
    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, _)) => {
                        client_count.fetch_add(1, Ordering::Relaxed);
                        let n = client_count.load(Ordering::Relaxed);
                        debug!("Client connected ({n} total)");

                        let cmd_tx = engine.cmd_sender();
                        let evt_rx = event_tx.subscribe();
                        let count = client_count.clone();
                        let notify = shutdown_notify.clone();

                        tokio::spawn(async move {
                            handle_client(stream, cmd_tx, evt_rx).await;
                            let n = count.fetch_sub(1, Ordering::Relaxed) - 1;
                            debug!("Client disconnected ({n} remaining)");
                            notify.notify_one();
                        });
                    }
                    Err(e) => {
                        error!("Accept error: {e}");
                        break;
                    }
                }
            }
            _ = &mut grace_handle => {
                // Grace period task completed — daemon is shutting down.
                break;
            }
        }
    }

    info!("Daemon stopped for {port}");
    Ok(())
}

async fn handle_client(
    stream: UnixStream,
    cmd_tx: mpsc::Sender<Command>,
    mut evt_rx: broadcast::Receiver<Event>,
) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Spawn event writer task.
    let (done_tx, mut done_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                event = evt_rx.recv() => {
                    match event {
                        Ok(event) => {
                            let resp = DaemonResponse::Event { event };
                            let mut line = serde_json::to_string(&resp).unwrap_or_default();
                            line.push('\n');
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = done_rx.recv() => break,
            }
        }
    });

    // Read requests from client.
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let req: DaemonRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                warn!("Bad request: {e}");
                continue;
            }
        };

        // Ping is a keepalive — no engine command needed.
        if matches!(&req, DaemonRequest::Ping) {
            continue;
        }

        let cmd = match req {
            DaemonRequest::Connect { port, config } => Command::Connect { port, config },
            DaemonRequest::Send { data } => Command::Send { data },
            DaemonRequest::Disconnect => Command::Disconnect,
            DaemonRequest::SetDtr { active } => Command::SetDtr { active },
            DaemonRequest::SetRts { active } => Command::SetRts { active },
            DaemonRequest::SendBreak => Command::SendBreak,
            DaemonRequest::Ping => unreachable!(),
        };

        if cmd_tx.send(cmd).await.is_err() {
            break;
        }
    }

    // Signal the writer task to stop.
    drop(done_tx);
}
