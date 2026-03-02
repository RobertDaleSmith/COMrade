# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build
cargo build --workspace              # Debug build, all crates
cargo build --release                # Release build

# Test
cargo test --workspace               # All tests
cargo test -p comrade-protocol       # Single crate
cargo test -p comrade-core test_make_timestamp  # Single test

# Lint
cargo clippy --workspace -- -D warnings

# Tauri GUI
make dev                             # cargo tauri dev (live reload)
make build                           # cargo tauri build (.app bundle)

# Frontend (run from crates/comrade-app/ui/)
npm install                          # Install frontend deps
npm run build                        # tsc + vite build

# CLI
cargo run --bin comrade -- --list    # List serial ports
cargo run --bin comrade -- /dev/cu.usbserial-1420  # TUI mode
cargo run --bin comrade -- --raw /dev/cu.usbserial-1420  # Raw stdout mode
```

## Architecture

Four crates in a layered architecture — protocol types at the bottom, frontends at the top:

```
comrade-cli (TUI/raw)     comrade-app (Tauri GUI)
         \                    /
          comrade-core (Engine)
                |
          comrade-protocol (types)
```

### comrade-protocol
Pure data types with serde. No async, no I/O. Defines `Command` (Connect, Disconnect, Send, ListPorts, Shutdown), `Event` (Data, Connected, Disconnected, Error, PortList, Shutdown), `SerialConfig`, `PortInfo`, `DeviceInfo`, `DeviceKind`.

### comrade-core
The `Engine` is the central async state machine. It spawns a tokio task (`engine_loop`) that owns the serial port and communicates via channels:
- **Commands in**: `mpsc::Sender<Command>` — frontends send connect/disconnect/send
- **Events out**: `broadcast::Sender<Event>` — multiple subscribers get Data, Connected, etc.
- `engine_loop` uses `tokio::select!` between serial reads and command receives

`enumerate_ports()` returns serial-only `PortInfo`. `enumerate_devices()` merges serial + HID devices by (vid, pid, serial_number) into `DeviceInfo` with `DeviceKind::Serial | Hid | Both`.

### comrade-cli
Two modes: **TUI** (ratatui + crossterm) and **raw** (`--raw`, writes bytes to stdout). The TUI bridges async Engine events into a sync `std::sync::mpsc` channel alongside terminal input and tick events (`AppEvent` enum). Commands go TUI → `tokio::UnboundedSender` → Engine.

### comrade-app
Tauri v2 GUI. `AppState` holds an `ActiveConnection` enum (None | Serial | Hid). Tauri commands (`#[tauri::command]`) handle both serial (via Engine) and HID (via `HidSession`). Streaming data flows through Tauri `Channel<T>` (not Tauri events). The `LineAssembler` buffers serial bytes into complete lines; HID reports are sent as atomic `HidReport` structs.

Frontend: vanilla TypeScript + Vite at `crates/comrade-app/ui/`. Port selector → terminal view with scrolling output, status bar, input bar. `TerminalUI` class manages display.

## Key Patterns

- **Engine is always spawned, never constructed directly**: `Engine::spawn()` creates channels and the background task. Use `engine.subscribe()` to get additional event receivers.
- **Blocking serial/HID calls use `spawn_blocking`**: `enumerate_ports()`, `enumerate_devices()`, and all `hidapi` calls are blocking and must not run on the tokio executor.
- **LineAssembler exists in both CLI and GUI**: each frontend has its own copy (`comrade-cli/src/tui/line_assembler.rs` and `comrade-app/src/line_assembler.rs`). They buffer partial data, split on `\n`, strip `\r`, lossy UTF-8.
- **macOS port filtering**: `list_devices` in comrade-app filters out `/dev/tty.*`, `/dev/cu.debug-console`, and `/dev/cu.Bluetooth-Incoming-Port`. Only `cu.*` paths are valid for outgoing connections on macOS.
- **HID uses `hidapi` with `macos-shared-device` feature**: allows monitoring HID reports without stealing the device from the system.
