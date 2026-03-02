# COMrade

A modern serial & HID monitor for hardware hackers, firmware engineers, and embedded developers.

Built in Rust with a native macOS GUI (Tauri v2), a terminal UI (ratatui), and a raw CLI mode.

## Features

**Serial monitoring**
- Connect to any serial port (USB CDC, FTDI, CH340, CP210x, etc.)
- Configurable baud rate, data bits, parity, stop bits, flow control
- Auto-asserts DTR for Arduino/CDC devices
- Streaming terminal with timestamps, color-coded lines (received/sent/system)

**HID device support** *(in progress)*
- Monitor HID input reports (hex + ASCII dump)
- Send output and feature reports
- View raw and parsed HID report descriptors
- Unified device list showing Serial, HID, or Both per USB device

**Three interfaces**
- **GUI** (`make dev`) — Tauri v2 native macOS app with dark terminal theme, port selector, status bar, input history
- **TUI** (`comrade <port>`) — Full terminal UI with scrollback, status bar, keyboard input
- **Raw** (`comrade --raw <port>`) — Pipe-friendly raw stdout output

## Prerequisites

- Rust toolchain (stable)
- [Tauri CLI](https://v2.tauri.app/start/prerequisites/) for the GUI: `cargo install tauri-cli --version "^2.0"`
- Node.js + npm (for the frontend build)

## Quick Start

```bash
# List available devices
cargo run --bin comrade -- --list

# Connect with TUI
cargo run --bin comrade -- /dev/cu.usbserial-1420

# Connect with raw output
cargo run --bin comrade -- --raw -b 9600 /dev/cu.usbserial-1420

# Launch the GUI app
make dev
```

## Build

```bash
# Debug build (all crates)
cargo build --workspace

# Release build
cargo build --release

# Build macOS .app bundle
make build

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings
```

## Project Structure

```
crates/
  comrade-protocol/   Shared types: Command, Event, SerialConfig, DeviceInfo
  comrade-core/       Engine (async serial I/O), device enumeration
  comrade-cli/        CLI binary with TUI and raw modes
  comrade-app/        Tauri v2 GUI app
    ui/               TypeScript frontend (Vite + vanilla TS)
```

## CLI Usage

```
comrade [OPTIONS] [PORT]

Arguments:
  [PORT]    Serial port path (e.g. /dev/cu.usbserial-1420, COM3)

Options:
  -b, --baud <BAUD>       Baud rate [default: 115200]
  -l, --list              List available serial ports
      --raw               Raw output mode (no TUI)
      --data-bits <N>     Data bits: 5-8 [default: 8]
      --parity <MODE>     none, odd, even [default: none]
      --stop-bits <N>     1 or 2 [default: 1]
      --flow <MODE>       none, hardware, software [default: none]
  -v, --verbose           Enable debug logging
```

## License

MIT
