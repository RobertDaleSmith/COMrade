# Changelog

All notable changes to COMrade are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-05-21

Initial public release. A modern serial, HID, and BLE device monitor for
hardware hackers, firmware engineers, and embedded developers — built in Rust
with a native macOS GUI (Tauri v2), a terminal UI (ratatui), and a raw CLI mode.

### Added

#### GUI (Tauri v2)
- Tabbed multi-device interface — multiple connections open at once, with
  CDC/HID/BLE badges, Cmd+T / Cmd+W, and duplicate-device detection.
- Unified device discovery merging serial, HID, and BLE interfaces into a single
  list with transport (USB/BT) and interface (CDC/HID/NUS) badges, plus
  per-interface connect buttons for composite devices.
- Timestamped, color-coded output (received / sent / system / MCP) with a
  10,000-line scrollback and smart auto-scroll.
- Live chart view for CSV serial data.
- Native menus, regex/substring search with match navigation, and per-tab
  status bars showing connection state and RX byte counts.
- Log export to text or CSV (Cmd+S), copy-to-clipboard (Cmd+Shift+C), and
  auto-logging to file.

#### Serial (USB CDC)
- Connect to any serial port (FTDI, CH340, CP210x, etc.) with configurable baud
  (9600–2M), data bits, parity, stop bits, and flow control.
- DTR/RTS toggle controls and break-signal support.
- Auto-reconnect with exponential backoff (2s → 30s).

#### HID
- Monitor HID input reports with hex + ASCII dump.
- Send output and feature reports.
- View raw and parsed HID report descriptors with usage-table lookups.
- Non-exclusive device access on macOS (does not steal the device from the
  system), with an nusb fallback for composite CDC+HID devices.

#### BLE NUS (Nordic UART Service)
- Discover BLE NUS devices via btleplug scan.
- Native CoreBluetooth path for paired macOS devices invisible to btleplug.
- Full NUS session: connect, discover services, subscribe to TX notifications,
  write to RX.

#### CLI
- Terminal UI (ratatui) and raw stdout mode (`--raw`).
- Device listing (`--list`) covering serial and HID.
- Subcommands that proxy through a running COMrade instance, with a shared
  Engine daemon for multi-client serial port access and auto-started headless
  MCP server.

#### MCP Server (Claude integration)
- Built-in MCP server on port 9712 with tools: `list_devices`, `connect_device`,
  `get_status`, `get_logs`, `search_logs`, `send_serial`, `send_hid_report`,
  `clear_logs`.
- Multi-tab aware — all tools accept `tab_id`; MCP-sent commands appear in the
  log with an `[MCP]` prefix.

#### Firmware (RP2040)
- PIO-based USB CDC ↔ UART bridge with automatic TX/RX pin-pair detection.
- `$CB:` command protocol with flash-persistent configuration.
- Pico W variant adding a BLE NUS wireless bridge.

#### Build & release
- Cargo workspace (protocol / core / cli / app) with CI running tests and
  clippy, plus dual-arch (aarch64 + x86_64) macOS builds.
- VERSION-driven release workflow producing universal `.dmg` artifacts.
- Homebrew formula (CLI) and cask (app).

### Known limitations
- macOS only (Apple Silicon and Intel).
- Release builds are not yet code-signed or notarized; on first launch,
  right-click → Open to bypass Gatekeeper.

[0.1.0]: https://github.com/RobertDaleSmith/COMrade/releases/tag/v0.1.0
