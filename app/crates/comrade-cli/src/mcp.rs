//! Stdio MCP bridge — proxies JSON-RPC between stdin/stdout and the
//! GUI app's HTTP MCP server on port 9712.
//!
//! Claude launches `comrade --mcp` as a subprocess (stdio transport).
//! This process forwards every request to the running COMrade GUI app
//! and relays responses back, so Claude can interact with the app's
//! live tabs, connections, and log buffers.

use std::io::{self, BufRead, Write};

use anyhow::{bail, Result};

const MCP_URL: &str = "http://127.0.0.1:9712/mcp";

pub async fn run_mcp() -> Result<()> {
    let client = reqwest::Client::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Check that the GUI MCP server is reachable.
    match client.post(MCP_URL)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await
    {
        Ok(_) => {}
        Err(_) => {
            eprintln!("COMrade GUI is not running (MCP server not reachable on port 9712).");
            eprintln!("Start COMrade first, then retry.");
            bail!("MCP server not reachable");
        }
    }

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        // Forward the JSON-RPC request to the HTTP MCP server.
        let response = client
            .post(MCP_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(line)
            .send()
            .await;

        match response {
            Ok(resp) => {
                let body = resp.text().await.unwrap_or_default();
                if !body.is_empty() {
                    writeln!(stdout, "{body}")?;
                    stdout.flush()?;
                }
            }
            Err(e) => {
                // Return a JSON-RPC error response.
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32603,
                        "message": format!("MCP proxy error: {e}")
                    }
                });
                writeln!(stdout, "{err}")?;
                stdout.flush()?;
            }
        }
    }

    Ok(())
}
