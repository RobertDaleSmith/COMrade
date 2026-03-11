# Core Serial Improvements — Implementation Plan

## Status Legend
- [ ] Not started
- [~] In progress
- [x] Complete

---

## 1. Fail-safe Logging
**Goal**: Auto-log every line to disk as it arrives, preventing data loss on crash.

### Files to modify
- `app/crates/comrade-app/src/commands.rs` — start/stop log file on connect/disconnect
- `app/crates/comrade-app/src/lib.rs` — register new command if needed
- `app/crates/comrade-app/ui/src/main.ts` — UI toggle for auto-logging
- `app/crates/comrade-app/ui/src/style.css` — styling for log toggle

### Implementation
- [x] Add `AutoLogger` struct that wraps `BufWriter<File>` with immediate flush
- [x] On start, open timestamped log file (e.g. `comrade_2026-03-11_13-45-00.log`)
- [x] Each `LogEntry` written to file automatically via `LogBuffer::push()` hook
- [x] On stop, close file handle and report entry count
- [x] Add tauri commands: `start_auto_log`, `stop_auto_log`, `auto_log_status`
- [x] Add "Log" toggle button in toolbar + directory picker via Tauri dialog
- [x] Button shows "Log ●" (red) when active

---

## 2. Log Export
**Goal**: Save current terminal buffer to file on demand.

### Files to modify
- `app/crates/comrade-app/src/commands.rs` — new `export_log` command
- `app/crates/comrade-app/src/log_buffer.rs` — format buffer for export
- `app/crates/comrade-app/ui/src/main.ts` — export button handler

### Implementation
- [x] Add tauri command `export_log(path: String, format: String)` that reads `LogBuffer` and writes to file
- [x] Support formats: plain text (timestamp + text), CSV (timestamp, direction, text)
- [x] Use Tauri save dialog for file path selection (`tauri-plugin-dialog`)
- [x] Add "Export" button in toolbar
- [x] Keyboard shortcut: Cmd+S

---

## 3. Timestamp Formats
**Goal**: Configurable timestamp display in terminal and exports.

### Files to modify
- `app/crates/comrade-app/src/line_assembler.rs` — timestamp generation
- `app/crates/comrade-app/ui/src/main.ts` — settings UI
- `app/crates/comrade-app/ui/src/terminal.ts` — timestamp rendering

### Implementation
- [x] Three formats: Time (HH:MM:SS.mmm), Elapsed (+N.NNNs since connect), ISO (date+time)
- [x] Formatting done in frontend TerminalUI — backend always sends consistent format
- [x] Click RX counter in status bar to cycle format
- [x] Connect time reset on new connection

---

## 4. Serial Config Enhancements
**Goal**: Custom baud rates, DTR/RTS control, break signal.

### Files to modify
- `app/crates/comrade-core/src/engine.rs` — DTR/RTS/break control commands
- `app/crates/comrade-protocol/src/lib.rs` — new Command variants
- `app/crates/comrade-app/src/commands.rs` — new tauri commands
- `app/crates/comrade-app/ui/src/main.ts` — custom baud input, DTR/RTS toggles

### Implementation
- [x] Custom baud rate input (dropdown + "Custom..." option with number input)
- [x] Added `Command::SetDtr`, `Command::SetRts`, `Command::SendBreak` to protocol
- [x] Handled in engine_loop with proper error reporting
- [x] Added tauri commands: `set_dtr`, `set_rts`, `send_break`
- [x] DTR/RTS toggle buttons + BRK button in toolbar (serial mode only)

---

## 5. Connection Robustness
**Goal**: Graceful cleanup, user warnings, port validation.

### Files to modify
- `app/crates/comrade-app/src/lib.rs` — shutdown hook
- `app/crates/comrade-app/src/commands.rs` — port validation
- `app/crates/comrade-app/ui/src/main.ts` — unsaved warning, beforeunload

### Implementation
- [x] Add Tauri `on_window_event(Destroyed)` handler — shut down active connection on quit
- [x] `shutdown_connection()` made public, called from both disconnect command and window close
- [ ] Validate port exists before connect attempt (check path is still in enumerated list)

---

## 6. UX Polish
**Goal**: Context menus, shortcuts, activity indicators.

### Files to modify
- `app/crates/comrade-app/ui/src/main.ts` — context menu, shortcuts, activity LED
- `app/crates/comrade-app/ui/src/terminal.ts` — context menu on terminal area
- `app/crates/comrade-app/ui/src/style.css` — activity indicator styling

### Implementation
- [x] Right-click context menu (already existed: Copy, Clear Above, Clear Below)
- [x] Keyboard shortcut Cmd+S for export (see #2)
- [x] TX/RX activity LED in status bar — green pulse on RX, orange pulse on TX
- [x] Total bytes shown in status bar (already existed)

---

## Implementation Order
1. **Log Export** (#2) — most immediately useful, builds on existing LogBuffer
2. **Fail-safe Logging** (#1) — extends export with auto-write
3. **Connection Robustness** (#5) — quick wins, important for reliability
4. **Serial Config** (#4) — custom baud + DTR/RTS
5. **UX Polish** (#6) — context menu, activity LED
6. **Timestamp Formats** (#3) — nice-to-have refinement
