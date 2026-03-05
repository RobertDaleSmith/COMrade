# Architecture

COMrade is a four-crate Rust workspace with a layered architecture. Protocol types at the bottom, frontends at the top.

```
comrade-cli (TUI/raw)     comrade-app (Tauri GUI)
         \                    /
          comrade-core (Engine)
                |
          comrade-protocol (types)
```

Each layer depends only on the layers below it. The two frontends (`comrade-cli` and `comrade-app`) are independent of each other.

---

## comrade-protocol

Pure data types with serde serialization. No async, no I/O, no side effects.

### Types

**Command** — messages sent from frontends to the engine:

| Variant | Fields | Purpose |
|---------|--------|---------|
| `Connect` | `port: String`, `config: SerialConfig` | Open a serial connection |
| `Disconnect` | — | Close the active connection |
| `Send` | `data: Vec<u8>` | Write raw bytes to the port |
| `ListPorts` | — | Request available port list |
| `Shutdown` | — | Shut down the engine |

**Event** — messages emitted by the engine to frontends:

| Variant | Fields | Purpose |
|---------|--------|---------|
| `Data` | `ts: Timestamp`, `bytes: Vec<u8>` | Raw bytes received from port |
| `Connected` | `ts`, `port`, `config` | Port successfully opened |
| `Disconnected` | `ts`, `port`, `reason` | Port closed (intentional or error) |
| `Reconnecting` | `ts`, `port`, `attempt` | Attempting reconnection |
| `Error` | `ts`, `message` | Non-fatal error |
| `PortList` | `ts`, `ports: Vec<PortInfo>` | Available port enumeration |
| `Shutdown` | — | Engine shutting down |

**SerialConfig** — connection parameters:

| Field | Type | Default |
|-------|------|---------|
| `baud_rate` | `u32` | 115200 |
| `data_bits` | `DataBits` | Eight |
| `parity` | `Parity` | None |
| `stop_bits` | `StopBits` | One |
| `flow_control` | `FlowControl` | None |

**DeviceInfo** — unified device representation:

| Field | Type | Description |
|-------|------|-------------|
| `path` | `String` | Display path |
| `serial_path` | `Option<String>` | `/dev/cu.*` on macOS |
| `hid_path` | `Option<String>` | HID device path |
| `vid` / `pid` | `Option<u16>` | USB vendor/product ID |
| `serial_number` | `Option<String>` | USB serial number |
| `manufacturer` | `Option<String>` | USB manufacturer string |
| `product` | `Option<String>` | USB product string |
| `kind` | `DeviceKind` | `Serial`, `Hid`, or `Both` |
| `hid_usage` | `Option<HidUsageInfo>` | HID usage page + usage ID |

**ReconnectStrategy** — reconnection behavior:

| Variant | Behavior |
|---------|----------|
| `Disabled` | No reconnection |
| `Direct` (default) | Reopen same path with exponential backoff |
| `ByUsbId { vid, pid }` | Wait for matching USB VID:PID to reappear |
| `Latest` | Connect to most recently appeared device |

**Timestamp** — dual-source timing:

| Field | Type | Purpose |
|-------|------|---------|
| `wall` | `DateTime<Utc>` | Wall-clock UTC time |
| `mono_us` | `u64` | Microseconds since engine epoch |

The monotonic counter starts when the engine spawns (`Instant::now()`). This gives accurate relative timing independent of clock adjustments.

---

## comrade-core

The async engine and device enumeration layer. Depends on `comrade-protocol` and `tokio`.

### Engine

The `Engine` is the central async state machine. It is always created via `Engine::spawn()`, which:

1. Creates an `mpsc::Sender<Command>` channel (capacity: 256)
2. Creates a `broadcast::Sender<Event>` channel (capacity: 4096)
3. Records an epoch (`Instant::now()`) for monotonic timestamps
4. Spawns the `engine_loop` as a background tokio task
5. Returns the `Engine` handle

```rust
let engine = Engine::spawn();
engine.send(Command::Connect { port, config }).await?;
let event = engine.recv().await?;
```

**Key methods:**

| Method | Returns | Purpose |
|--------|---------|---------|
| `spawn()` | `Engine` | Create engine + spawn background task |
| `subscribe()` | `broadcast::Receiver<Event>` | Get additional event receiver |
| `send(cmd)` | `Result<()>` | Send command to engine |
| `recv()` | `Result<Event>` | Receive next event |
| `cmd_sender()` | `mpsc::Sender<Command>` | Clone command sender |
| `epoch()` | `Instant` | Session start time for timestamps |

**Engine loop internals:**

The `engine_loop` function uses `tokio::select!` to multiplex:
- **Serial reads**: 4096-byte buffer, emits `Event::Data` on each read
- **Command receives**: processes `Connect`, `Disconnect`, `Send`, `ListPorts`, `Shutdown`

When no port is open, it blocks on command receive only. When a port is open, it selects between serial read and command receive.

**DTR assertion**: On connect, the engine calls `write_data_terminal_ready(true)` on the serial stream. This is required for USB CDC devices (Arduino, etc.) to enable communication.

**Broadcast channels**: The event channel uses `broadcast`, so multiple subscribers can independently receive all events. Lagged receivers log a warning and continue.

### Device Enumeration

Two functions, both blocking (must run via `spawn_blocking`):

**`enumerate_ports()`** — serial ports only:
- Calls `serialport::available_ports()`
- Returns `Vec<PortInfo>` with path, VID, PID, serial number, manufacturer, product

**`enumerate_devices()`** — unified serial + HID:
- Enumerates serial ports via `serialport`
- Enumerates HID devices via `hidapi`
- Merges by `(vid, pid, serial_number)` tuple
- Composite devices (same VID/PID/serial on both serial and HID) become `DeviceKind::Both`
- On macOS: skips `/dev/tty.*` paths (only `cu.*` are valid for connections)
- Sorts: Both first, then Serial, then HID; alphabetical by product within each kind

---

## comrade-cli

Command-line interface binary with two modes.

### TUI Mode (default)

Full terminal UI using ratatui + crossterm.

**Architecture**: bridges async engine events into a sync `std::sync::mpsc` channel alongside terminal input and tick events:

```rust
enum AppEvent {
    Terminal(crossterm::event::Event),  // Keyboard, mouse, resize
    Engine(Event),                       // From engine
    Tick,                               // Cursor blink timer
}
```

The TUI app state tracks:
- `lines: Vec<LogLine>` — scrollback buffer
- `scroll_offset` — 0 means auto-scroll to latest
- `status: ConnStatus` — Connecting, Connected, Disconnected
- `rx_bytes` — total bytes received counter
- `input: InputState` — current user input with history
- `assembler: LineAssembler` — buffers partial data into complete lines

**LineAssembler** buffers incoming bytes and splits on `\n`:
- Handles `\r\n` and bare `\n`
- Strips `\r` silently
- Uses lossy UTF-8 conversion
- Flushes partial lines on timeout

### Raw Mode (`--raw`)

Minimal mode for scripting and pipelines:
- Writes received bytes directly to stdout (no buffering, no formatting)
- Connection info logged to stderr
- `Ctrl+C` triggers clean disconnect via `tokio::signal::ctrl_c()`
- Exit on disconnect or error

### Argument Parsing

Uses `clap` (derive) with these flags:

| Flag | Short | Default | Notes |
|------|-------|---------|-------|
| `PORT` | positional | — | Required unless `--list` |
| `--baud` | `-b` | 115200 | Any u32 value |
| `--list` | `-l` | — | List devices and exit |
| `--raw` | — | — | Raw stdout mode |
| `--data-bits` | — | 8 | 5, 6, 7, or 8 |
| `--parity` | — | none | none/n, odd/o, even/e |
| `--stop-bits` | — | 1 | 1 or 2 |
| `--flow` | — | none | none/n, hardware/hw/h, software/sw/s |
| `--verbose` | `-v` | — | Sets tracing to debug level |

---

## Key Patterns

### Blocking calls use `spawn_blocking`

`enumerate_ports()`, `enumerate_devices()`, and all `hidapi` calls are blocking and must not run on the tokio executor. They are always wrapped in `tokio::task::spawn_blocking`.

### LineAssembler exists in both frontends

Both `comrade-cli` and `comrade-app` have their own `LineAssembler` that buffers raw bytes from the engine into displayable text lines. Each strips `\r`, splits on `\n`, and handles lossy UTF-8.

### macOS port filtering

The GUI's `list_devices` command applies additional filters:
- Blocks `/dev/cu.debug-console` and `/dev/cu.Bluetooth-Incoming-Port`
- Blocks Apple internal HID devices (VID `0x05AC`)
- Blocks virtual endpoints (VID `0x0000`, PID `0x0000`)

### Engine is always spawned, never constructed

`Engine::spawn()` creates channels and the background task. There is no public constructor. Use `engine.subscribe()` to create additional event receivers.

---

## Dependencies

### Core

| Crate | Version | Purpose |
|-------|---------|---------|
| tokio | 1.x | Async runtime (full features) |
| tokio-serial | 5.4 | Async serial port I/O |
| serialport | 4.x | Port enumeration |
| serde / serde_json | 1.x | Serialization |
| chrono | 0.4 | Wall-clock timestamps |
| thiserror | 2.x | Error type derivation |
| anyhow | 1.x | Error handling |
| tracing | 0.1 | Structured logging |

### HID

| Crate | Version | Purpose |
|-------|---------|---------|
| hidapi | 2.6 | HID device access (`macos-shared-device` feature) |
| nusb | 0.2 | USB device access |
| hidreport | 0.6 | HID descriptor parsing |
| hut | 0.5 | HID usage table lookups |

### CLI

| Crate | Version | Purpose |
|-------|---------|---------|
| clap | 4.x | Argument parsing (derive) |
| ratatui | 0.29 | Terminal UI framework |
| crossterm | 0.28 | Terminal control |

### GUI

| Crate | Version | Purpose |
|-------|---------|---------|
| tauri | 2.x | Native desktop framework |
| @tauri-apps/api | 2.x | Frontend IPC |
| vite | 6.x | Frontend build tool |
| typescript | 5.x | Frontend language |

### Utilities

| Crate | Version | Purpose |
|-------|---------|---------|
| vte | 0.13 | ANSI escape sequence parsing |
| regex | 1.x | Pattern matching |
| bytes | 1.x | Byte buffers |
| arc-swap | 1.x | Lock-free swapping |
| dirs | 6.x | Platform directory paths |
| toml | 0.8 | TOML configuration |
