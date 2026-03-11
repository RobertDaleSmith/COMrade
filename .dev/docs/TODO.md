# COMrade TODO

## Future Features

### RTT (Real-Time Transfer)
- SEGGER RTT support for ARM Cortex-M debug output
- Non-intrusive, no UART pins needed — uses debug probe (J-Link, CMSIS-DAP)
- Would complement existing UART/BLE transports as a third data source

### HID Usage Report Parsing
- Parse HID report descriptors into structured field-level views
- Map usage pages/IDs to human-readable names (buttons, axes, LEDs, etc.)
- Show decoded field values alongside raw hex in the HID monitor
- Reference: USB HID Usage Tables spec

## Core Serial Improvements

Priority items learned from analyzing SerialLogger and general usage:

### Fail-safe Logging
- Write each line to log file immediately (not buffered in memory)
- Prevents data loss on crash/power failure
- Append mode for existing log files

### Log Export
- Save terminal output to .log/.txt/.csv
- Configurable timestamp format in export
- Include/exclude sent commands option

### Timestamp Formats
- ISO 8601 with timezone
- Configurable delimiter (space, comma, semicolon) for CSV-friendly output
- Relative timestamps (time since connect)

### Serial Config
- Custom/arbitrary baud rates (beyond standard list)
- DTR/RTS line control toggle
- Break signal sending

### Connection Robustness
- Graceful shutdown hook — close port even on app crash/force quit
- Warn on unsaved buffer before disconnect/quit
- Port validation before connect attempt

### UX Polish
- Right-click context menu (copy, select all, clear)
- Keyboard shortcut for log export
- Visual indicator for TX/RX activity (LED-style blink)
