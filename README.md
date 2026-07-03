# COMrade

A modern serial, HID, and BLE device monitor for hardware hackers, firmware engineers, and embedded developers.

Built in Rust with a native macOS GUI (Tauri v2), a terminal UI (ratatui), and a raw CLI mode.

> *The serial monitor that Arduino IDE should have been.*

## Features

### Multi-Device Tabbed Interface
- Open multiple device connections simultaneously in tabs
- CDC, HID, and BLE tab badges for quick identification
- Cmd+T to open new tab, Cmd+W to close, click to switch
- Device selector appears inline when adding tabs (existing tabs stay accessible)
- Duplicate detection — connecting to an already-open device switches to its tab

### Serial Monitoring (USB CDC)
- Connect to any serial port (USB CDC, FTDI, CH340, CP210x, etc.)
- Configurable baud rate (9600 to 2M), data bits, parity, stop bits, flow control
- DTR/RTS toggle controls with break signal support
- Timestamped, color-coded output (received / sent / system / MCP)
- Auto-reconnect with exponential backoff (2s → 30s)
- 10,000-line scrollback with smart auto-scroll

### HID Device Support
- Monitor HID input reports with hex + ASCII dump
- Send output and feature reports
- View raw and parsed HID report descriptors with usage table lookups
- Non-exclusive device access on macOS (doesn't steal device from system)

### BLE NUS (Nordic UART Service)
- Discover BLE NUS devices via btleplug scan
- Native CoreBluetooth fallback for paired macOS devices invisible to btleplug
- Full NUS session: connect, discover services, subscribe to TX notifications, write to RX
- BLE devices merged with HID entries when same device exposes both services

### Unified Device Discovery
- Single device list merges serial, HID, and BLE interfaces
- Transport badges (USB, BT) and interface badges (CDC, HID, NUS)
- Composite devices offer per-interface connect buttons
- Filters macOS system devices (debug console, Bluetooth, Apple HID)
- Split-pane device selector: sidebar with controls, full-height scrollable device list

### MCP Server (Claude Integration)
- Built-in MCP server on port 9712 for AI-assisted debugging
- Tools: `list_devices`, `connect_device`, `get_status`, `get_logs`, `search_logs`, `send_serial`, `send_hid_report`, `clear_logs`
- MCP-sent commands appear in terminal log with `[MCP]` prefix
- Multi-tab aware — all tools accept `tab_id` for routing

### Logging & Export
- Auto-log to file with timestamped entries
- Export to text or CSV via File > Export (Cmd+S)
- Copy full log to clipboard (Cmd+Shift+C)
- Search with regex/substring, match navigation, dimmed non-matches

### Three Interfaces

| Interface | Command | Best for |
|-----------|---------|----------|
| **GUI** | `make dev` | Daily use, multi-device, HID/BLE, visual monitoring |
| **TUI** | `comrade <port>` | SSH sessions, terminal workflows |
| **Raw** | `comrade --raw <port>` | Shell pipelines, logging to file |

### UART Bridge Firmware
- RP2040 PIO-based USB CDC ↔ UART bridge
- Auto-detects TX/RX pin orientation on any GPIO pair
- Pico W variant adds BLE NUS wireless serial bridge
- Configurable via `$CB:` commands (pins, baud, LED, save to flash)

## Prerequisites

- **Rust** toolchain (stable, 2021 edition)
- **Tauri CLI** for the GUI: `cargo install tauri-cli --version "^2.0"`
- **Node.js** + npm (for the frontend build)

## Quick Start

```bash
# List connected devices (serial + HID)
cargo run --bin comrade -- --list

# Connect with the terminal UI
cargo run --bin comrade -- /dev/cu.usbserial-1420

# Connect at a specific baud rate
cargo run --bin comrade -- -b 9600 /dev/cu.usbserial-1420

# Raw output (pipe-friendly, Ctrl+C to disconnect)
cargo run --bin comrade -- --raw /dev/cu.usbserial-1420

# Launch the GUI app (live reload)
make dev
```

## Build

```bash
cargo build --workspace              # Debug build, all crates
cargo build --release                # Release build
make build                           # macOS .app bundle (arm64 + x86_64)
cargo test --workspace               # Run all tests
cargo clippy --workspace -- -D warnings  # Lint
```

### Frontend (from `app/crates/comrade-app/ui/`)

```bash
npm install                          # Install dependencies
npm run build                        # TypeScript + Vite production build
```

### Firmware (requires Pico SDK)

```bash
export PICO_SDK_PATH=~/pico-sdk
make firmware                        # Standard RP2040 build
make firmware-w                      # Pico W build (BLE NUS)
```

### Makefile Targets

| Target | Description |
|--------|-------------|
| `make dev` | `cargo tauri dev` with Vite live reload |
| `make build` | `cargo tauri build` (.app bundle) |
| `make list` | List available devices via CLI |
| `make firmware` | Build RP2040 UART bridge firmware |
| `make firmware-w` | Build Pico W firmware with BLE NUS |
| `make clean` | `cargo clean` + remove frontend dist |

## MCP Setup (Claude Integration)

COMrade includes a built-in MCP server that lets Claude Code interact with connected devices.

Two ways to wire it up. **Pick one — don't do both**; they conflict.

### Option A: Auto-configured via `.mcp.json` (recommended for this repo)

A project-scoped MCP server is already committed to `.mcp.json`. It spawns the
built binary directly, so you need to build it once first:

```bash
cd app && cargo build --bin comrade
```

Then open Claude Code in the repo root and approve the `comrade` MCP server when
prompted. The server auto-starts as a headless daemon on port 9712 (or bridges
to a running COMrade GUI if there is one).

### Option B: HTTP transport (any directory, needs COMrade running)

```bash
claude mcp add --transport http comrade http://127.0.0.1:9712/mcp
```

Launch COMrade (GUI or `cargo run --bin comrade -- --mcp`) before starting Claude
Code so port 9712 is up.

## CLI Reference

```
comrade [OPTIONS] [PORT]

Arguments:
  [PORT]                   Serial port path (e.g. /dev/cu.usbserial-1420, COM3)

Options:
  -b, --baud <BAUD>        Baud rate [default: 115200]
  -l, --list               List available devices and exit
      --raw                Raw output mode (no TUI, bytes to stdout)
      --data-bits <N>      Data bits: 5, 6, 7, 8 [default: 8]
      --parity <MODE>      none, odd, even [default: none]
      --stop-bits <N>      1 or 2 [default: 1]
      --flow <MODE>        none, hardware, software [default: none]
  -v, --verbose            Enable debug logging (RUST_LOG=debug)
  -h, --help               Print help
  -V, --version            Print version
```

### Examples

```bash
# List all connected devices with VID/PID info
comrade --list

# Arduino at 9600 baud
comrade -b 9600 /dev/cu.usbmodem14201

# Full serial config: 7 data bits, even parity, 2 stop bits
comrade --data-bits 7 --parity even --stop-bits 2 /dev/cu.usbserial-1420

# Log raw output to file
comrade --raw /dev/cu.usbserial-1420 > session.log 2>&1

# Pipe through grep for filtering
comrade --raw /dev/cu.usbserial-1420 2>/dev/null | grep "ERROR"
```

## GUI Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Cmd+T` | New tab (open device selector) |
| `Cmd+W` | Close current tab |
| `Cmd+S` | Export log |
| `Cmd+F` | Search logs |
| `Cmd+K` | Clear terminal |
| `Cmd+Shift+C` | Copy full log to clipboard |
| `Cmd+C` | Copy selection |
| `Enter` | Send input |
| `Up/Down` | Navigate input history |
| `Escape` | Scroll to bottom |

## Architecture

```
app/
  crates/
    comrade-protocol/     Pure data types (Command, Event, SerialConfig, DeviceInfo)
    comrade-core/         Async engine (serial I/O, device enumeration)
    comrade-cli/          CLI binary (TUI + raw modes)
    comrade-app/          Tauri v2 GUI application
      ui/                 TypeScript + Vite frontend
firmware/
  src/                    RP2040 UART bridge + BLE NUS (Pico W)
scripts/                  Build helpers
```

Four crates in a layered architecture:

```
comrade-cli (TUI/raw)     comrade-app (Tauri GUI)
         \                    /
          comrade-core (Engine)
                |
          comrade-protocol (types)
```

## License

MIT
