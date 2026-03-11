mod auto_log;
mod ble_session;
mod commands;
mod connection_status;
mod hid_descriptor;
mod hid_session;
mod line_assembler;
mod log_buffer;
mod mcp;
#[cfg(target_os = "macos")]
mod native_ble_nus;

use std::sync::Arc;

use auto_log::AutoLogger;
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
    let auto_logger = Arc::new(AutoLogger::new());
    log_buffer.set_auto_logger(auto_logger.clone());

    let mcp_log_buffer = log_buffer.clone();
    let mcp_status_tracker = status_tracker.clone();

    let shared_state = Arc::new(Mutex::new(AppState::new())) as SharedState;
    let mcp_shared_state = shared_state.clone();
    let shutdown_state = shared_state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared_state)
        .manage(log_buffer)
        .manage(status_tracker)
        .manage(auto_logger)
        .on_window_event({
            let state = shutdown_state;
            move |_window, event| {
                if let tauri::WindowEvent::Destroyed = event {
                    let state = state.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut app = state.lock().await;
                        commands::shutdown_connection(&mut app.connection).await;
                    });
                }
            }
        })
        .setup(|_app| {
            tauri::async_runtime::spawn(mcp::start_mcp_server(
                mcp_log_buffer,
                mcp_status_tracker,
                mcp_shared_state,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_devices,
            commands::scan_ble,
            commands::connect,
            commands::connect_hid,
            commands::connect_ble_nus,
            commands::send_data,
            commands::send_hid_report,
            commands::get_hid_descriptor,
            commands::set_dtr,
            commands::set_rts,
            commands::send_break,
            commands::disconnect,
            commands::export_log,
            commands::debug_ble,
            commands::start_auto_log,
            commands::stop_auto_log,
            commands::auto_log_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running COMrade");
}
