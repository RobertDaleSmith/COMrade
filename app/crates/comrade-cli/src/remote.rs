//! Remote commands — talk to a running COMrade instance (GUI or headless CLI)
//! via the HTTP MCP API on port 9712.
//!
//! These commands never open the serial port directly. They proxy through
//! whichever COMrade instance currently owns it.

use anyhow::{bail, Result};

const MCP_URL: &str = "http://127.0.0.1:9712/mcp";

/// Ensure a headless MCP server is running, starting one if needed.
async fn ensure_mcp_running() {
    let client = reqwest::Client::new();
    if client
        .post(MCP_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
        .is_ok()
    {
        return; // Already running.
    }

    // Start headless MCP server as a detached background process.
    eprintln!("Starting COMrade headless server...");
    let exe = std::env::current_exe().unwrap_or_else(|_| "comrade".into());
    let _ = std::process::Command::new(exe)
        .arg("--mcp")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    // Wait for it to be ready.
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if client
            .post(MCP_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body("{}")
            .send()
            .await
            .is_ok()
        {
            return;
        }
    }
    eprintln!("Warning: headless server may not have started");
}

async fn mcp_call(tool: &str, args: serde_json::Value) -> Result<String> {
    ensure_mcp_running().await;

    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    });

    let resp = client
        .post(MCP_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("Failed to connect to COMrade MCP server"))?;

    let json: serde_json::Value = resp.json().await?;

    if let Some(err) = json.get("error") {
        bail!("{}", err.get("message").and_then(|m| m.as_str()).unwrap_or("MCP error"));
    }

    let text = json
        .pointer("/result/content/0/text")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
}

pub async fn send(text: &str) -> Result<()> {
    let result = mcp_call("send_serial", serde_json::json!({ "text": text })).await?;
    println!("{result}");
    Ok(())
}

pub async fn logs(count: usize) -> Result<()> {
    let result = mcp_call("get_logs", serde_json::json!({ "count": count })).await?;
    println!("{result}");
    Ok(())
}

pub async fn status() -> Result<()> {
    let result = mcp_call("get_status", serde_json::json!({})).await?;
    println!("{result}");
    Ok(())
}

pub async fn connect(port: &str, baud: u32) -> Result<()> {
    // Try GUI's connect_device first (has UI integration).
    let result = match mcp_call(
        "connect_device",
        serde_json::json!({
            "connection_type": "serial",
            "path": port,
            "baud": baud,
        }),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => {
            // Fall back to headless connect.
            mcp_call("connect", serde_json::json!({ "port": port, "baud": baud })).await?
        }
    };
    println!("{result}");
    Ok(())
}

pub async fn disconnect() -> Result<()> {
    let result = match mcp_call("disconnect_device", serde_json::json!({})).await {
        Ok(r) => r,
        Err(_) => mcp_call("disconnect", serde_json::json!({})).await?,
    };
    println!("{result}");
    Ok(())
}
