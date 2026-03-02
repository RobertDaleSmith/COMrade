mod commands;
mod hid_descriptor;
mod hid_session;
mod line_assembler;

use std::sync::Arc;

use commands::{AppState, SharedState};
use tokio::sync::Mutex;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    tauri::Builder::default()
        .manage(Arc::new(Mutex::new(AppState::new())) as SharedState)
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
