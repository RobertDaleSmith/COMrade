# COMrade - Serial Monitor Research & Landscape Analysis

> A modern, cross-platform serial monitor for hardware hackers, firmware engineers, and embedded developers.

---

## Problem Statement

Every hardware/firmware developer needs serial monitoring (TTL, UART, CDC) for debugging. The most common workflow today is opening Arduino IDE -- a full IDE -- just to use its serial monitor. That monitor is broken in fundamental ways, and the alternatives are fragmented, dated, or platform-limited.

The community has been asking for something better for **over 14 years** (Arduino Forum threads dating to 2011). Nobody has delivered a unified solution.

---

## Competitive Landscape

### GUI Desktop Applications

| Tool | Platforms | Cost | Key Strengths | Key Weaknesses |
|------|-----------|------|---------------|----------------|
| **CoolTerm** | Win/Mac/Linux | Free | Simple, cross-platform, reliable | No filtering, no ANSI, no plotting, dated UI |
| **Serial (Decisive Tactics)** | macOS only | $40 | Polished native Mac UI, VT100 emulation | Expensive, no plotting, no hex analysis, Mac-only |
| **Serial Studio** | Win/Mac/Linux | Free (open-core) | Excellent visualization (plots, gauges, maps, FFT). 6.6k GitHub stars | Dashboard tool, not a general terminal. Requires JSON frame definitions |
| **SerialTool (Duolabs)** | Win/Mac/Linux | Free/Paid | VT-100, multi-port, Modbus, Python scripting, triggers | Some features behind paywall, cluttered UI |
| **IO Ninja** | Win/Mac/Linux | ~$5/mo | Regex highlighting, protocol decoding, hex/text switching | Subscription model, steep learning curve, overkill for basic use |
| **RealTerm** | Windows only | Free | Gold standard hex/binary/decimal/float display, automation | Windows-only, UI stuck in 2005, no longer actively developed |
| **NinjaTerm** | Win/Linux (Mac planned) | Free (GPLv3) | Filtering, smart scroll, ANSI support, crash-safe logging | Electron-heavy, no macOS yet, 50kB/s throughput ceiling, ~104 stars |
| **serial-monitor-rust** | Win/Mac/Linux | Free (GPLv3) | Rust+egui, plotting + monitoring, low CPU, auto-reconnect | Young project, no ANSI, no filtering. ~233 stars |
| **Better Serial Plotter** | Win/Mac/Linux | Free | Drop-in Arduino plotter replacement, multiple plots | Plotting only, not a general terminal |
| **ScriptCommunicator** | Win/Linux | Free | JS scripting, supports serial/UDP/TCP/SPI/I2C/CAN | Dated UI, no macOS, complex setup |
| **electerm** | Win/Mac/Linux | Free | Terminal/SSH/SFTP/Serial/RDP/VNC, AI assistant | Serial is an afterthought, not optimized for embedded workflows |

### CLI Tools

| Tool | GitHub Stars | Key Strengths | Key Weaknesses |
|------|-------------|---------------|----------------|
| **tio** | ~2,800 | Auto-reconnect, Lua scripting, hex mode, RS-485, non-standard baud rates | CLI only, no plotting, no multi-port in single session |
| **picocom** | -- | Minimal (~40KB), POSIX-compatible | Very minimal -- no scripting, no hex mode, no auto-reconnect |
| **minicom** | -- | Full-featured, file transfer protocols | Dated ncurses UI, confusing setup, modem-era heritage |
| **screen / cu** | built-in | Zero install | Near-zero features, hard to exit |

### IDE-Integrated

| Tool | Status | Verdict |
|------|--------|---------|
| **Arduino IDE Serial Monitor** | Active | Universally inadequate. Broken copy-paste, scroll bugs, no filtering, no hex, no auto-reconnect |
| **PlatformIO Monitor** | Active | Config-driven but ANSI colors broken, no GUI send box |
| **VS Code Serial Monitor (Microsoft)** | **Archived July 2025** | Microsoft walked away. Performance issues, hex bugs, remote port locking |
| **JetBrains CLion Serial Port Monitor** | Active (first-party as of 2024) | Requires CLion subscription, basic feature set |

### Browser-Based

| Tool | Notes |
|------|-------|
| **Web Serial API tools** | Zero install via Chrome. Limited to Chrome/Edge, no Firefox/Safari. Sandbox limitations |

---

## Community Pain Points ("The Big Five")

### 1. Log Flooding With No Filtering
Fast-printing firmware overwhelms every popular monitor. No real-time grep/regex filtering in Arduino IDE, CoolTerm, PlatformIO, or most tools. Developers resort to commenting out print statements or piping `screen` output through grep -- hacks that break workflows.

### 2. No Auto-Reconnect
Every upload cycle disconnects the serial monitor. Reconnecting is manual in most tools. First requested on Arduino Forum in **2011**. Still not standard 14+ years later. `tio` is the standout exception with multiple reconnection strategies.

### 3. Timestamp Inaccuracy
Multiple Arduino Forum threads spanning 2011-2024 requesting accurate timestamps. Arduino IDE 2.x added timestamps but they are [wildly inaccurate](https://github.com/arduino/arduino-ide/issues/391). Developers want microsecond-precision timestamps for performance profiling.

### 4. No Simultaneous Hex + Text View
Most tools offer hex OR text, not synchronized side-by-side. RealTerm is the gold standard but is Windows-only with a 2005-era UI. VS Code Serial Monitor's hex view has [confirmed bugs](https://github.com/microsoft/vscode-serial-monitor/issues/134).

### 5. Poor macOS Support
The best serial tools (RealTerm, Tera Term) are Windows-only. macOS users are stuck with CoolTerm (limited), Serial ($40), or CLI tools. The macOS serial tool ecosystem is the weakest of any platform.

### Additional Frequently Cited Frustrations

- **No multi-port monitoring** -- debugging two MCUs communicating requires two unsynchronized terminal windows
- **No session persistence** -- close the app, lose your history
- **Baud rate guessing** -- no tool makes it easy to cycle through rates while watching output
- **No ANSI color support** -- escape sequences not rendered by Arduino IDE, CoolTerm, or PlatformIO
- **Arduino IDE copy-paste is broken** -- only copies visible rows, not full buffer
- **No pattern-triggered actions** -- no alerts on error strings, no auto-logging on pattern match

---

## Feature Gap Analysis

| Feature | Best Current Implementation | Gap |
|---------|---------------------------|-----|
| Real-time regex filtering | NinjaTerm, IO Ninja | Missing from all mainstream free tools |
| Simultaneous hex+text view | RealTerm (Windows only) | No cross-platform solution |
| Multi-port with time sync | IO Ninja (paid) | No free solution |
| Auto-reconnect on upload | tio | Missing from all GUI tools except serial-monitor-rust |
| Baud rate auto-detection | **No good solution exists** | Universal gap |
| Live data plotting | Serial Studio, serial-monitor-rust | Missing from most general-purpose terminals |
| ANSI color rendering | tio, NinjaTerm | Missing from Arduino IDE, CoolTerm, PlatformIO |
| Scripting engine | tio (Lua), SerialTool (Python) | Missing from most GUI tools |
| Pattern-triggered actions | SerialTool (auto-response) | Missing from nearly everything |
| Crash-safe logging | NinjaTerm | Missing from most tools |
| Regex-based highlighting | IO Ninja | Missing from all free tools |
| Smart scroll (pause on up, resume on down) | NinjaTerm | Missing from Arduino IDE, CoolTerm |
| Session recording with replay | Partial in some tools | No tool does this well |
| Built-in session diffing | **Nothing** | Universal gap |

---

## The Opportunity

**No cross-platform, modern, open-source tool combines terminal + hex view + plotting + filtering + scripting in a single, lightweight application with a polished UI.**

Every existing tool covers a subset. The community is vocal. Microsoft just abandoned their VS Code solution. The competition is fragmented and aging.

### Target Users
- Arduino / ESP32 / STM32 / RP2040 hobbyists (largest volume)
- Professional embedded firmware engineers
- Hardware reverse engineers
- IoT developers
- Robotics developers
- University embedded systems courses

### Positioning
**"The serial monitor that Arduino IDE should have been -- cross-platform, beautiful, and powerful enough for professionals."**

---

## Why Not "SerialBox"?

The name is taken multiple times:
1. **Serial Box / Realm** -- NPR-featured audio entertainment company (serialbox.com)
2. **SerialBox** -- well-known **macOS software piracy tool** for cracking serial numbers. Disqualifying for a macOS-targeted developer tool -- search results dominated by piracy links
3. **serialbox (GridTools)** -- data serialization library on GitHub

### Chosen Name: COMrade

- Memorable wordplay: COM port + comrade
- Fun, approachable tone that resonates with the hacker community
- Implies companionship -- the tool that's always by your side during debugging
- Unique and searchable

---

## Technology Direction

### Recommended Stack: Tauri v2
- **Rust backend** for serial I/O performance and safety
- **Web frontend** (TypeScript + modern framework) for UI flexibility
- **Native webview** -- not bundled Chromium (5-10MB binary vs ~150MB for Electron)
- `tauri-plugin-serialplugin` crate exists for serial port access
- Aligns with ecosystem direction (Betaflight Configurator evaluating Tauri migration)

### Why Not Alternatives?
- **Electron**: Too heavy for a utility app (150MB+ binary, high RAM)
- **Pure native (egui/iced)**: Harder to build polished, complex UIs; smaller ecosystem for UI components
- **Qt/GTK**: C++ complexity, licensing considerations, harder to attract contributors
- **Swift/SwiftUI**: macOS-only, defeats cross-platform goal

---

## References & Sources

- [CoolTerm](https://freeware.the-meiers.org/CoolTermHelp/)
- [Serial by Decisive Tactics](https://www.decisivetactics.com/products/serial/)
- [Serial Studio (GitHub)](https://github.com/Serial-Studio/Serial-Studio)
- [tio (GitHub)](https://github.com/tio/tio)
- [NinjaTerm](https://ninjaterm.mbedded.ninja/)
- [serial-monitor-rust (GitHub)](https://github.com/hacknus/serial-monitor-rust)
- [SerialTool](https://serialtool.com/_en/index.php)
- [IO Ninja](https://ioninja.com/)
- [RealTerm](https://sourceforge.net/projects/realterm/)
- [Tera Term](https://teratermproject.github.io/index-en.html)
- [VS Code Serial Monitor (GitHub, archived)](https://github.com/microsoft/vscode-serial-monitor)
- [JetBrains CLion Serial Port Monitor](https://blog.jetbrains.com/clion/2024/04/serial-port-monitor-for-embedded-developers/)
- [Arduino Forum: Auto-reconnect (2011)](https://forum.arduino.cc/t/auto-reconnection-of-serial-monitor/60921)
- [Arduino Forum: Timestamps](https://forum.arduino.cc/t/timestamp-option-for-serial-monitor/52587)
- [Arduino Forum: Copy-paste bugs](https://forum.arduino.cc/t/copy-paste-data-from-the-serial-monitor/1041585)
- [Arduino IDE scroll bug (GitHub #1250)](https://github.com/arduino/arduino-ide/issues/1250)
- [Arduino IDE timestamp bug (GitHub #391)](https://github.com/arduino/arduino-ide/issues/391)
- [Tauri Serial Plugin](https://crates.io/crates/tauri-plugin-serialplugin)
- [Web Serial API (Chrome)](https://developer.chrome.com/docs/capabilities/serial)
