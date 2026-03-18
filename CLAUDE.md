# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Layout

```
app/          Rust workspace — Tauri GUI, CLI, core engine, protocol types
firmware/     RP2040 firmware — UART bridge with auto-detect TX/RX
scripts/      Build helpers (sync-version.sh)
```

## Build & Test Commands

```bash
# All cargo commands run from app/
cd app

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

# Frontend (run from app/crates/comrade-app/ui/)
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
Tauri v2 GUI with tabbed multi-device connections. `AppState` holds `HashMap<String, TabState>` where each tab owns an `ActiveConnection` enum (None | Serial | Hid | BleNus | NativeBleNus), a `LogBuffer`, and a `StatusTracker`. All Tauri commands take a `tab_id` parameter.

Streaming data flows through Tauri `Channel<T>` (not Tauri events). The `LineAssembler` buffers serial bytes into complete lines; HID reports are sent as atomic `HidReport` structs. BLE NUS uses btleplug for scanning devices and native CoreBluetooth (via `objc2-core-bluetooth`) for paired macOS devices.

Frontend: vanilla TypeScript + Vite at `app/crates/comrade-app/ui/`. Each `TerminalUI` instance creates its own `.output` div. DOM appends are batched via `requestAnimationFrame` for high-throughput resilience.

Built-in MCP server on port 9712 (rmcp + axum) provides tools for AI-assisted device interaction.

### firmware/uart-bridge
RP2040 PIO-based USB CDC ↔ UART bridge. Auto-detects TX/RX pin orientation. Pico W variant adds BLE NUS wireless bridge. Build with Pico SDK, flash via UF2 drag-and-drop.

## Key Patterns

- **Engine is always spawned, never constructed directly**: `Engine::spawn()` creates channels and the background task. Use `engine.subscribe()` to get additional event receivers.
- **Blocking serial/HID calls use `spawn_blocking`**: `enumerate_ports()`, `enumerate_devices()`, and all `hidapi` calls are blocking and must not run on the tokio executor.
- **LineAssembler exists in both CLI and GUI**: each frontend has its own copy (`comrade-cli/src/tui/line_assembler.rs` and `comrade-app/src/line_assembler.rs`). They buffer partial data, split on `\n`, strip `\r`, lossy UTF-8.
- **macOS port filtering**: `list_devices` in comrade-app filters out `/dev/tty.*`, `/dev/cu.debug-console`, and `/dev/cu.Bluetooth-Incoming-Port`. Only `cu.*` paths are valid for outgoing connections on macOS.
- **HID uses `hidapi` with `macos-shared-device` feature**: allows monitoring HID reports without stealing the device from the system.
- **Tabbed state**: every Tauri command that operates on a connection requires `tab_id`. Frontend generates UUIDs per tab.
- **BLE dual-path**: btleplug for advertising devices, native CoreBluetooth for paired devices. CoreBluetooth objects are `!Send`/`!Sync` and must stay on their creating thread.
- **DOM safety**: if an HTML element is removed, its `getElementById` must use `?` not `!` — the `!` assertion crashes the entire script.
