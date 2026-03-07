mod commands;
mod connection_status;
mod hid_descriptor;
mod hid_session;
mod line_assembler;
mod log_buffer;
mod mcp;

use std::sync::Arc;

use commands::{AppState, SharedState};
use connection_status::StatusTracker;
use log_buffer::LogBuffer;
use tokio::sync::Mutex;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let log_buffer = Arc::new(LogBuffer::new());
    let status_tracker = Arc::new(StatusTracker::new());

    let mcp_log_buffer = log_buffer.clone();
    let mcp_status_tracker = status_tracker.clone();

    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(AppState::new())) as SharedState)
        .manage(log_buffer)
        .manage(status_tracker)
        .setup(|_app| {
            tauri::async_runtime::spawn(mcp::start_mcp_server(
                mcp_log_buffer,
                mcp_status_tracker,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::connect,
            commands::connect_hid,
            commands::send_data,
            commands::send_hid_report,
            commands::get_hid_descriptor,
            commands::disconnect,
        ])
        .run(tauri::generate_context!())
        .expect("error while running COMrade");
}
