# COMrade

A modern serial & HID device monitor for hardware hackers, firmware engineers, and embedded developers.

Built in Rust with a native macOS GUI (Tauri v2), a terminal UI (ratatui), and a raw CLI mode.

> *The serial monitor that Arduino IDE should have been.*

## Features

### Serial Monitoring
- Connect to any serial port (USB CDC, FTDI, CH340, CP210x, etc.)
- Configurable baud rate (9600 to 2M), data bits, parity, stop bits, flow control
- Auto-asserts DTR for Arduino/CDC devices
- Timestamped, color-coded output (received / sent / system)
- Auto-reconnect on device disconnect
- 10,000-line scrollback with smart auto-scroll

### HID Device Support
- Monitor HID input reports with hex + ASCII dump
- Send output and feature reports
- View raw and parsed HID report descriptors with usage table lookups
- Non-exclusive device access on macOS (doesn't steal device from system)

### Unified Device Discovery
- Single device list merges serial and HID interfaces by USB VID/PID
- Devices classified as Serial, HID, or Both
- Composite devices offer per-interface connect buttons
- Filters macOS system devices (debug console, Bluetooth, Apple HID)

### Three Interfaces

| Interface | Command | Best for |
|-----------|---------|----------|
| **GUI** | `make dev` | Daily use, HID inspection, visual monitoring |
| **TUI** | `comrade <port>` | SSH sessions, terminal workflows |
| **Raw** | `comrade --raw <port>` | Shell pipelines, logging to file |

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

### Frontend (from `crates/comrade-app/ui/`)

```bash
npm install                          # Install dependencies
npm run build                        # TypeScript + Vite production build
```

### Makefile Targets

| Target | Description |
|--------|-------------|
| `make dev` | `cargo tauri dev` with Vite live reload |
| `make build` | `cargo tauri build` (.app bundle) |
| `make list` | List available devices via CLI |
| `make clean` | `cargo clean` + remove frontend dist |

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
| `Enter` | Send input |
| `Up/Down` | Navigate input history |
| `Cmd+K` | Clear terminal |
| `Cmd+Shift+C` | Copy full log to clipboard |
| `Escape` | Scroll to bottom |

## Architecture

Four crates in a layered architecture:

```
comrade-cli (TUI/raw)     comrade-app (Tauri GUI)
         \                    /
          comrade-core (Engine)
                |
          comrade-protocol (types)
```

See [docs/architecture.md](docs/architecture.md) for detailed design documentation.

## Project Structure

```
crates/
  comrade-protocol/     Pure data types (Command, Event, SerialConfig, DeviceInfo)
  comrade-core/         Async engine (serial I/O, device enumeration)
  comrade-cli/          CLI binary (TUI + raw modes)
  comrade-app/          Tauri v2 GUI application
    ui/                 TypeScript + Vite frontend
docs/
  architecture.md       System design and internals
  architecture-gui.md   GUI-specific architecture
  research-landscape.md Competitive analysis and market research
  comrade_design_language.md  Visual identity and branding guide
```

## Supported Baud Rates

9600, 19200, 38400, 57600, **115200** (default), 230400, 460800, 921600, 1000000, 2000000

## CI/CD

GitHub Actions runs on every push to `main` and on pull requests:
- **Test**: `cargo test --workspace` + `cargo clippy` on macOS
- **Build**: macOS .app bundles for arm64 and x86_64 (main branch only, draft releases via `tauri-action`)

## License

MIT
