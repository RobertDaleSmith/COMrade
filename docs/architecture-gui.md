# GUI Architecture

The COMrade GUI is a Tauri v2 application with a Rust backend and a vanilla TypeScript + Vite frontend.

## Backend (Rust)

### Entry Point

`crates/comrade-app/src/lib.rs` sets up the Tauri application:

```rust
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
```

### State Management

```rust
enum ActiveConnection {
    None,
    Serial { engine: Engine, assembler: LineAssembler },
    Hid { session: HidSession },
}

pub struct AppState {
    connection: ActiveConnection,
}

pub type SharedState = Arc<Mutex<AppState>>;
```

Only one connection is active at a time. Opening a new connection shuts down the previous one.

### Tauri Commands (IPC)

| Command | Signature | Description |
|---------|-----------|-------------|
| `list_devices` | `() -> Vec<DeviceInfo>` | Enumerate serial + HID devices with filtering |
| `connect` | `(port, baud, on_line: Channel<SerialLine>)` | Open serial connection, stream lines |
| `connect_hid` | `(hid_path, on_report: Channel<HidReport>)` | Open HID device, stream reports |
| `send_data` | `(text)` | Send text + newline to serial port |
| `send_hid_report` | `(data, report_type)` | Send output or feature report |
| `get_hid_descriptor` | `()` | Get parsed HID descriptor info |
| `disconnect` | `()` | Close active connection |

### Data Streaming

Data flows from backend to frontend via **Tauri `Channel<T>`** (not Tauri events):

**Serial flow:**
1. Engine emits `Event::Data { bytes }` on the broadcast channel
2. A spawned tokio task receives events from `engine.subscribe()`
3. `LineAssembler` buffers bytes, splits on `\n`, strips `\r`
4. Complete `SerialLine` structs sent via `Channel<SerialLine>`
5. Partial lines flushed after 100ms timeout

**HID flow:**
1. `HidSession` spawns a blocking read loop via `spawn_blocking`
2. Each HID report converted to `HidReport` struct (timestamp, hex dump, ASCII dump, report ID)
3. Reports sent via `Channel<HidReport>`

### SerialLine Format

```typescript
interface SerialLine {
  timestamp: string;     // "HH:MM:SS.mmm"
  text: string;          // Line content
  kind: "received" | "sent" | "system";
  rx_bytes_total: number;
}
```

### HidReport Format

```typescript
interface HidReport {
  timestamp: string;
  data: number[];        // Raw bytes
  hex: string;           // "0A 1B 2C ..."
  ascii: string;         // Printable ASCII representation
  report_id: number | null;
  report_count: number;
  rx_bytes_total: number;
  kind: "input" | "error";
}
```

### HID Session

`HidSession` wraps `hidapi` with non-exclusive access on macOS (`macos-shared-device` feature):

- `open(hid_path, callback)` — opens device, spawns blocking read loop
- `send_output_report(data)` — write output report
- `send_feature_report(data)` — write feature report
- `raw_descriptor()` — return raw HID descriptor bytes
- `stop()` — signal the read loop to exit

### HID Descriptor Parsing

`hid_descriptor::parse_hid_descriptor(raw_bytes)` uses the `hidreport` and `hut` crates to produce:
- Parsed field breakdown (usage pages, report IDs, field sizes)
- Human-readable usage names from HID usage tables
- Raw hex dump of the descriptor

---

## Frontend (TypeScript)

### File Structure

```
crates/comrade-app/ui/
  index.html              Single-page app shell
  package.json            Dependencies (@tauri-apps/api, vite, typescript)
  tsconfig.json           TypeScript config
  src/
    main.ts               App logic, event handlers, IPC calls
    terminal.ts           TerminalUI class (output display, context menu)
    descriptor-panel.ts   HID descriptor viewer (parsed + raw tabs)
    style.css             All styling (CSS custom properties)
```

### Views

The app has two views, toggled by CSS class:

1. **Port Selector** (`#port-select`) — shown when disconnected
   - Device list with name, path, VID:PID, device type badges
   - Baud rate selector (9600 to 2,000,000)
   - Refresh button (also auto-refreshes every 2 seconds)

2. **Terminal** (`#terminal`) — shown when connected
   - **Toolbar**: port name, config string, connection state, RX counter, action buttons
   - **Output**: scrollable log with timestamped, color-coded lines
   - **Descriptor Panel**: slide-out panel with parsed/raw HID descriptor tabs
   - **Input Bar**: text input with optional HID report type/ID controls

### TerminalUI Class

Manages the output display:

- `appendLine(line)` — add a serial line to output
- `appendHidReport(report)` — add a HID report to output
- `clear()` — clear all output
- `copyLog()` — copy full log to clipboard
- `scrollToBottom()` — force scroll to bottom
- `setConnected/setDisconnected/setConnecting/setReconnecting` — update status bar

**Auto-scroll**: tracks scroll position. If user is at bottom (within 30px), new lines auto-scroll. Scrolling up pauses auto-scroll. `Escape` key resumes.

**Line limit**: caps at 10,000 lines, removing oldest when exceeded.

**Context menu**: custom right-click menu on output lines with Copy, Clear Above, Clear Below. Default browser context menu is disabled globally.

### DescriptorPanel Class

HID descriptor viewer with two tabs:
- **Parsed**: formatted field breakdown with indentation
- **Raw**: hex dump of raw descriptor bytes

### Device List Polling

- Calls `list_devices` every 2 seconds while on the port selector
- Diffs JSON response to avoid unnecessary DOM rebuilds
- Polling stops when connected, resumes on disconnect

### Auto-Reconnect

Frontend-driven reconnection:
- Remembers connection context (serial port+baud or HID path+name)
- On disconnect or error, schedules reconnect after 2 seconds
- Shows "RECONNECTING..." status
- User-initiated disconnect (`userDisconnected` flag) suppresses reconnect

### Input History

- Up/Down arrows navigate command history
- History deduplicates consecutive identical entries
- Current input preserved while browsing history

### HID Input

When connected to a HID device:
- Input interpreted as space-separated hex bytes (e.g., `0A 1B 2C`)
- Report type selector (Output / Feature)
- Report ID input field (hex)

---

## Tauri Configuration

```json
{
  "productName": "COMrade",
  "version": "0.1.0",
  "identifier": "com.comrade.serial-monitor",
  "app": {
    "withGlobalTauri": true,
    "windows": [{
      "title": "COMrade",
      "width": 900, "height": 600,
      "minWidth": 600, "minHeight": 400,
      "resizable": true
    }]
  }
}
```

### Build Pipeline

- **Dev**: Vite dev server on port 1421 + Tauri window with live reload
- **Build**: `tsc && vite build` produces static assets in `ui/dist/`, then Tauri bundles into `.app`

---

## Design Language

The UI follows the COMrade design language (see `docs/comrade_design_language.md`):

### Color Palette (CSS Custom Properties)

| Variable | Value | Role |
|----------|-------|------|
| `--bg-deep` | `#14171A` | Main background |
| `--bg-panel` | `#24292E` | Toolbar, input bar, panels |
| `--bg-surface` | `#1B1F23` | Hover backgrounds, context menu |
| `--bg-elevated` | `#2F353A` | Button hover, scrollbar thumb |
| `--fg-primary` | `#F1E6CF` | Primary text (cream) |
| `--fg-secondary` | `#A89B8C` | Secondary text, system messages |
| `--fg-tertiary` | `#6B6157` | Timestamps, placeholders |
| `--accent-red` | `#D72626` | Primary accent (connected, active, title) |
| `--accent-red-dim` | `#8B1A1A` | Context menu hover |
| `--accent-orange` | `#FF6A00` | Sent text, connecting state |
| `--border` | `#2F353A` | All borders |

### Typography

| Variable | Value | Used for |
|----------|-------|----------|
| `--font-ui` | Inter, SF Pro Display, system sans-serif | Labels, buttons, headers |
| `--font-mono` | JetBrains Mono, SF Mono, Menlo | Body text, data, input |

### Industrial Design Accents

- Title: uppercase, letter-spacing 2px, red bottom border
- Port items: red left accent bar on hover (via `::before` pseudo-element)
- Toolbar: 2px red top border
- Buttons: subtle red glow on hover
- Disconnect button: red border + glow on hover
- Input caret: red
- Scrollbar: 6px narrow track
