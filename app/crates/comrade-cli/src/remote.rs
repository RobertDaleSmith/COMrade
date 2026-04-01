//! Remote commands — talk to a running COMrade instance (GUI or headless)
//! via the daemon's Unix socket, or directly via Engine for simple operations.

use anyhow::{bail, Result};
use comrade_core::{DaemonClient, enumerate_devices};
use comrade_protocol::SerialConfig;

pub async fn send(text: &str) -> Result<()> {
    let client = find_active_client().await?;
    client
        .send_command(comrade_protocol::Command::Send {
            data: format!("{text}\n").into_bytes(),
        })
        .await?;
    println!("Sent.");
    Ok(())
}

pub async fn logs(count: usize) -> Result<()> {
    // Try MCP HTTP first (GUI or headless MCP running).
    if let Ok(text) = mcp_call("get_logs", serde_json::json!({ "count": count })).await {
        println!("{text}");
        return Ok(());
    }
    bail!("No COMrade instance running. Open the GUI or connect to a port first.");
}

pub async fn status() -> Result<()> {
    if let Ok(text) = mcp_call("get_status", serde_json::json!({})).await {
        println!("{text}");
        return Ok(());
    }
    // No MCP — check for daemon sockets.
    let devices = enumerate_devices().unwrap_or_default();
    let mut found = false;
    for dev in &devices {
        if let Some(ref path) = dev.serial_path {
            if comrade_core::daemon_is_running(path) {
                println!("Daemon active: {path}");
                found = true;
            }
        }
    }
    if !found {
        println!("No active connections.");
    }
    Ok(())
}

pub async fn connect(port: &str, baud: u32) -> Result<()> {
    let config = SerialConfig {
        baud_rate: baud,
        ..SerialConfig::default()
    };
    let client = DaemonClient::connect_or_spawn(port, &config).await?;
    // Send Connect in case daemon already exists but port is disconnected.
    client
        .send_command(comrade_protocol::Command::Connect {
            port: port.to_string(),
            config,
        })
        .await?;
    println!("Connected to {port} at {baud} baud");
    Ok(())
}

pub async fn disconnect() -> Result<()> {
    if let Ok(text) = mcp_call("disconnect_device", serde_json::json!({})).await {
        println!("{text}");
        return Ok(());
    }
    bail!("No COMrade instance running.");
}

/// Find an active DaemonClient by checking known daemon sockets.
async fn find_active_client() -> Result<DaemonClient> {
    let devices = enumerate_devices().unwrap_or_default();
    for dev in &devices {
        if let Some(ref path) = dev.serial_path {
            if comrade_core::daemon_is_running(path) {
                let config = SerialConfig::default();
                if let Ok(client) = DaemonClient::connect_or_spawn(path, &config).await {
                    return Ok(client);
                }
            }
        }
    }
    bail!("No active daemon. Connect to a port first: comrade connect <port>");
}

/// Try calling the MCP HTTP API (GUI or headless MCP server).
async fn mcp_call(tool: &str, args: serde_json::Value) -> Result<String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    });

    let resp = client
        .post("http://127.0.0.1:9712/mcp")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await?;

    let json: serde_json::Value = resp.json().await?;

    if let Some(err) = json.get("error") {
        bail!(
            "{}",
            err.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("MCP error")
        );
    }

    let text = json
        .pointer("/result/content/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
}
