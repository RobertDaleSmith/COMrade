# AGENTS.md

Guidelines for AI coding agents (Claude Code, Copilot, Cursor, etc.) contributing to this repository.

## Project Overview

COMrade is a multi-interface serial, HID, and BLE device monitor built in Rust. It has a Tauri v2 GUI, a ratatui TUI, and a raw CLI mode. The firmware directory contains RP2040 UART bridge firmware.

## Repository Structure

```
app/                      Rust workspace (all cargo commands run from here)
  crates/
    comrade-protocol/     Pure data types — no async, no I/O
    comrade-core/         Async engine — serial port I/O, device enumeration
    comrade-cli/          CLI binary — TUI (ratatui) + raw mode
    comrade-app/          Tauri v2 GUI — tabbed multi-device monitor
      src/                Rust backend (commands, MCP server, BLE, HID)
      ui/                 Frontend (vanilla TypeScript + Vite)
        src/              main.ts, terminal.ts, style.css
        public/           Static assets (about.html)
firmware/                 RP2040 firmware (C, CMake, Pico SDK)
  src/                    UART bridge + BLE NUS
```

## Build & Verify

Always run these from `app/`:

```bash
cargo clippy --workspace -- -D warnings   # Must pass with zero warnings
cargo test --workspace                      # Must pass all tests
```

Frontend (from `app/crates/comrade-app/ui/`):

```bash
npm run build                               # tsc + vite, must succeed
```

Run both backend and frontend checks before considering any change complete.

## Architecture Rules

### Crate Layering
- `comrade-protocol` is the bottom layer — pure types, serde only, no dependencies on other crates
- `comrade-core` depends only on `comrade-protocol` — the async engine with serial I/O
- `comrade-cli` and `comrade-app` are frontends — they depend on core and protocol
- Never add dependencies from lower layers to higher layers

### Backend (Rust)

- **Tabbed state model**: `AppState` holds `HashMap<String, TabState>`. Every Tauri command that operates on a connection takes a `tab_id: String` parameter.
- **Engine pattern**: `Engine::spawn()` returns an engine handle. Never construct directly. Use `engine.subscribe()` for additional event receivers.
- **Blocking I/O**: Serial port, HID, and device enumeration calls are blocking. Use `spawn_blocking` or dedicated threads — never run on the tokio executor.
- **ActiveConnection enum**: `None | Serial | Hid | BleNus | NativeBleNus`. Each variant holds its session handle. `shutdown_connection()` drains via `std::mem::replace`.
- **MCP server**: runs on port 9712 via `rmcp` + `axum`. Tools are defined with `#[tool]` attribute macros. All tools that operate on connections require `tab_id`.

### Frontend (TypeScript)

- **Vanilla TypeScript** — no React, no framework. DOM manipulation only.
- **TerminalUI** creates its own `.output` div per tab instance. Starts hidden (`class="output hidden"`). `show()`/`hide()`/`destroy()` manage lifecycle.
- **Batched rendering**: `appendLine` and `appendHidReport` buffer into a `DocumentFragment`, flushed once per `requestAnimationFrame`. Never append directly to the DOM in a hot path.
- **Tab state**: the `Tab` interface holds all per-connection state (terminal, channels, reconnect context, history). Module-level state is only for truly global things.
- **`window.__*` callbacks**: menu events and MCP triggers call through `window.__toggleTimestamps`, `window.__newTab`, `window.__exportLog`, `window.__mcpConnect` etc. registered in main.ts.

### BLE (macOS)

- btleplug 0.12 for scanning/advertising devices
- Native CoreBluetooth (via `objc2-core-bluetooth`) for paired devices invisible to btleplug — uses `retrieveConnectedPeripheralsWithServices`
- CoreBluetooth objects are `!Send`/`!Sync` — must stay on dedicated threads
- NUS UUIDs: Service `6E400001-...`, RX `6E400002-...`, TX `6E400003-...`

## Coding Standards

- **Clippy clean**: `-D warnings` with no exceptions. Fix warnings, don't suppress them.
- **No over-engineering**: solve the current problem, not hypothetical future ones.
- **Prefer editing over creating**: modify existing files rather than adding new ones.
- **CSS class `.hidden`**: each element that can be hidden needs its own `.foo.hidden { display: none }` rule. There is no global `.hidden` class.
- **Removed DOM elements**: if you remove an HTML element, grep for its `getElementById` in TypeScript — the `!` assertion will crash the entire script if the element is missing.
- **No emojis in UI** unless explicitly requested. Use text badges (CDC, HID, BLE).
- **Serial line kinds**: `"received"`, `"sent"`, `"system"`, `"mcp"` — each has distinct CSS styling.

## Common Pitfalls

1. **Forgetting `tab_id`**: every Tauri command and MCP tool that touches a connection needs it.
2. **DOM crashes**: `document.getElementById("foo")!` crashes if `#foo` was removed from HTML. Always use `?` for optional elements.
3. **BLE scan blocking**: `list_ble_devices()` can take seconds. The `list_devices` command wraps it in a 3-second timeout.
4. **Reconnect flooding**: without backoff, rapid disconnect/reconnect cycles flood the terminal. Current backoff: 2s → 4s → 8s → ... → 30s max, reset on successful connect.
5. **Multiple output divs visible**: `.output.hidden { display: none }` is required. Without it, flex layout shows all tab outputs side by side.
6. **CoreBluetooth thread safety**: `CBCentralManager` must be created and used on the same thread. Use `std::thread::spawn` with channels, not tokio tasks.

## Commit Guidelines

- Commit messages: imperative mood, first line under 72 chars
- Include `Co-Authored-By` trailer for AI-assisted commits
- Run clippy + tests + frontend build before committing
- One logical change per commit — don't bundle unrelated fixes

## Testing

```bash
cargo test --workspace               # All unit tests
cargo test -p comrade-protocol       # Single crate
cargo test -p comrade-core test_make_timestamp  # Single test
```

There are no integration tests yet — manual testing with real devices is currently required for connection flows.

## Firmware

The RP2040 firmware is a separate build system (CMake + Pico SDK). It's independent of the Rust workspace.

```bash
export PICO_SDK_PATH=~/pico-sdk
make firmware       # Standard RP2040
make firmware-w     # Pico W with BLE NUS
```

Flash by holding BOOTSEL, connecting USB, and copying the `.uf2` file to the `RPI-RP2` volume.
