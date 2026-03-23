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
use tauri::menu::{CheckMenuItem, MenuBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::Manager;
use tokio::sync::Mutex;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let auto_logger = Arc::new(AutoLogger::new());

    let shared_state = Arc::new(Mutex::new(AppState::new(auto_logger.clone()))) as SharedState;
    let mcp_shared_state = shared_state.clone();
    let shutdown_state = shared_state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(shared_state)
        .manage(auto_logger)
        .on_window_event({
            let state = shutdown_state;
            move |_window, event| {
                if let tauri::WindowEvent::Destroyed = event {
                    let state = state.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut app = state.lock().await;
                        for (_, tab) in app.tabs.drain() {
                            let mut conn = tab.connection;
                            commands::shutdown_connection(&mut conn).await;
                        }
                    });
                }
            }
        })
        .setup(|app| {
            // Store app handle for MCP to use.
            {
                let state = app.state::<SharedState>();
                let mut app_state = tauri::async_runtime::block_on(state.lock());
                app_state.app_handle = Some(app.handle().clone());
            }

            // Build native menu bar with View menu.
            let show_timestamps = CheckMenuItem::with_id(
                app,
                "show_timestamps",
                "Show Timestamps",
                true,
                true,
                None::<&str>,
            )?;

            let app_menu = SubmenuBuilder::new(app, "COMrade")
                .item(&tauri::menu::MenuItem::with_id(
                    app,
                    "about",
                    "About COMrade",
                    true,
                    None::<&str>,
                )?)
                .separator()
                .item(&PredefinedMenuItem::hide(app, None)?)
                .item(&PredefinedMenuItem::hide_others(app, None)?)
                .item(&PredefinedMenuItem::show_all(app, None)?)
                .separator()
                .item(&PredefinedMenuItem::quit(app, None)?)
                .build()?;

            let file_menu = SubmenuBuilder::new(app, "File")
                .item(&tauri::menu::MenuItem::with_id(
                    app,
                    "new_tab",
                    "New Tab",
                    true,
                    Some("CmdOrCtrl+T"),
                )?)
                .separator()
                .item(&tauri::menu::MenuItem::with_id(
                    app,
                    "export_log",
                    "Export",
                    true,
                    Some("CmdOrCtrl+S"),
                )?)
                .build()?;

            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .item(&PredefinedMenuItem::cut(app, None)?)
                .item(&PredefinedMenuItem::copy(app, None)?)
                .item(&PredefinedMenuItem::paste(app, None)?)
                .item(&PredefinedMenuItem::select_all(app, None)?)
                .build()?;

            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&show_timestamps)
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&app_menu)
                .item(&file_menu)
                .item(&edit_menu)
                .item(&view_menu)
                .build()?;

            app.set_menu(menu)?;

            app.on_menu_event(move |app, event| {
                if event.id() == "about" {
                    if let Some(w) = app.get_webview_window("about") {
                        let _ = w.set_focus();
                        return;
                    }
                    let _ = tauri::WebviewWindowBuilder::new(
                        app,
                        "about",
                        tauri::WebviewUrl::App("about.html".into()),
                    )
                    .title("About COMrade")
                    .inner_size(320.0, 280.0)
                    .resizable(false)
                    .minimizable(false)
                    .maximizable(false)
                    .build();
                    return;
                }
                if let Some(window) = app.get_webview_window("main") {
                    if event.id() == "show_timestamps" {
                        let checked = show_timestamps.is_checked().unwrap_or(true);
                        let _ = window.eval(format!(
                            "window.__toggleTimestamps && window.__toggleTimestamps({})",
                            checked
                        ));
                    } else if event.id() == "new_tab" {
                        let _ = window.eval(
                            "window.__newTab && window.__newTab()",
                        );
                    } else if event.id() == "export_log" {
                        let _ = window.eval(
                            "window.__exportLog && window.__exportLog()",
                        );
                    }
                }
            });

            tauri::async_runtime::spawn(mcp::start_mcp_server(mcp_shared_state));
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
            commands::check_remote_mcp,
            commands::connect_remote,
            commands::send_remote,
            commands::debug_ble,
            commands::start_auto_log,
            commands::stop_auto_log,
            commands::auto_log_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running COMrade");
}
