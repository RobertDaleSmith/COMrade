import { invoke, Channel } from "@tauri-apps/api/core";
import { save, open } from "@tauri-apps/plugin-dialog";
import { TerminalUI, type SerialLine, type HidReport } from "./terminal";
import { DescriptorPanel, type HidDescriptorInfo } from "./descriptor-panel";

/** Device info returned by list_devices command. */
interface DeviceInfo {
  path: string;
  serial_path: string | null;
  hid_path: string | null;
  vid: number | null;
  pid: number | null;
  serial_number: string | null;
  manufacturer: string | null;
  product: string | null;
  kind: "Serial" | "Hid" | "Both" | "Ble";
  hid_usage: {
    usage_page: number;
    usage: number;
    usage_name: string | null;
  } | null;
  ble_id: string | null;
  ble_services: string[] | null;
  bus_type: string | null;
}

// DOM elements.
const portSelectEl = document.getElementById("port-select")!;
const terminalEl = document.getElementById("terminal")!;
const portListEl = document.getElementById("port-list")!;
const baudSelect = document.getElementById("baud-select") as HTMLSelectElement;
const baudLabel = document.getElementById("baud-label")!;
const refreshBtn = document.getElementById("refresh-btn")!;
const inputEl = document.getElementById("input") as HTMLInputElement;
const copyBtn = document.getElementById("copy-btn")!;
const clearBtn = document.getElementById("clear-btn")!;
const disconnectBtn = document.getElementById("disconnect-btn")!;
const descriptorBtn = document.getElementById("descriptor-btn")!;
const searchBtn = document.getElementById("search-btn")!;
const searchBar = document.getElementById("search-bar")!;
const searchInput = document.getElementById("search-input") as HTMLInputElement;
const searchCloseBtn = document.getElementById("search-close")!;
const searchPrevBtn = document.getElementById("search-prev")!;
const searchNextBtn = document.getElementById("search-next")!;
const hidInputControls = document.getElementById("hid-input-controls")!;
const hidReportType = document.getElementById("hid-report-type") as HTMLSelectElement;
const hidReportId = document.getElementById("hid-report-id") as HTMLInputElement;

const baudCustom = document.getElementById("baud-custom") as HTMLInputElement;
const serialControls = document.getElementById("serial-controls")!;
const dtrBtn = document.getElementById("dtr-btn")!;
const rtsBtn = document.getElementById("rts-btn")!;
const breakBtn = document.getElementById("break-btn")!;

// Custom baud rate handling.
baudSelect.addEventListener("change", () => {
  if (baudSelect.value === "custom") {
    baudCustom.classList.remove("hidden");
    baudCustom.focus();
  } else {
    baudCustom.classList.add("hidden");
  }
});

function getSelectedBaud(): number {
  if (baudSelect.value === "custom") {
    const v = parseInt(baudCustom.value, 10);
    return v > 0 ? v : 115200;
  }
  return parseInt(baudSelect.value, 10);
}

// State.
let terminal: TerminalUI | null = null;
let descriptorPanel: DescriptorPanel | null = null;
let connected = false;
let wasConnected = false;
let isHidMode = false;
let userDisconnected = false;
const history: string[] = [];
let historyIdx = -1;
let savedInput = "";

// Channels.
let lineChannel: Channel<SerialLine> | null = null;
let reportChannel: Channel<HidReport> | null = null;

// Timers.
let deviceListTimer: ReturnType<typeof setInterval> | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;

// Last device list snapshot for diffing (avoid flicker on no-change refresh).
let lastDeviceListJson = "";

// Reconnect context — remembers what we were connected to.
let reconnectCtx: {
  type: "serial";
  port: string;
  baud: number;
} | {
  type: "hid";
  hidPath: string;
  deviceName: string;
  vid: number;
  pid: number;
} | {
  type: "ble_nus";
  bleId: string;
  deviceName: string;
} | null = null;

// ---- Device list auto-refresh ----

function startDeviceListPolling(): void {
  stopDeviceListPolling();
  deviceListTimer = setInterval(refreshDevices, 2000);
}

function stopDeviceListPolling(): void {
  if (deviceListTimer !== null) {
    clearInterval(deviceListTimer);
    deviceListTimer = null;
  }
}

// ---- Device selection ----

async function refreshDevices(): Promise<void> {
  try {
    const [usbDevices, bleDevices] = await Promise.all([
      invoke<DeviceInfo[]>("list_devices"),
      invoke<DeviceInfo[]>("scan_ble").catch(() => []),
    ]);
    const devices: DeviceInfo[] = [...usbDevices, ...bleDevices];
    // Skip DOM rebuild if nothing changed.
    const json = JSON.stringify(devices);
    if (json === lastDeviceListJson) return;
    lastDeviceListJson = json;

    portListEl.innerHTML = "";
    if (devices.length === 0) {
      portListEl.innerHTML = '<div class="no-ports">No devices found.</div>';
      return;
    }
    for (const dev of devices) {
      const div = document.createElement("div");
      div.className = "port-item";

      const topRow = document.createElement("div");
      topRow.className = "port-item-top";

      const name = document.createElement("div");
      name.className = "port-name";
      name.textContent = dev.product || dev.manufacturer || "Unknown device";

      const badge = document.createElement("span");
      const bus = dev.bus_type; // "USB", "Bluetooth", "I2C", "SPI", or null
      badge.className = `device-badge badge-${dev.kind.toLowerCase()}`;
      if (bus === "Bluetooth") badge.className += " badge-ble";
      if (dev.kind === "Both") {
        badge.textContent = "SERIAL+HID";
      } else if (dev.kind === "Ble") {
        const svcs = dev.ble_services || [];
        if (svcs.includes("nus") && svcs.includes("hid")) {
          badge.textContent = "BLE NUS+HID";
        } else if (svcs.includes("nus")) {
          badge.textContent = "BLE NUS";
        } else if (svcs.includes("hid")) {
          badge.textContent = "BLE HID";
        } else {
          badge.textContent = "BLE";
        }
      } else if (dev.kind === "Hid" && bus === "Bluetooth") {
        badge.textContent = "BLE HID";
      } else if (dev.kind === "Hid") {
        badge.textContent = bus ? `${bus} HID` : "HID";
      } else if (dev.kind === "Serial") {
        badge.textContent = bus ? `${bus} SERIAL` : "SERIAL";
      }

      topRow.appendChild(name);
      topRow.appendChild(badge);

      const path = document.createElement("div");
      path.className = "port-path";
      path.textContent = dev.path;

      const desc = document.createElement("div");
      desc.className = "port-desc";
      const parts: string[] = [];
      if (dev.product && dev.manufacturer) parts.push(dev.manufacturer);
      if (dev.vid !== null && dev.pid !== null) {
        parts.push(
          `VID:0x${dev.vid.toString(16).padStart(4, "0")} PID:0x${dev.pid.toString(16).padStart(4, "0")}`
        );
      }
      desc.textContent = parts.join(" | ");

      div.appendChild(topRow);
      div.appendChild(path);
      if (desc.textContent) div.appendChild(desc);

      // Connect actions depend on device kind.
      if (dev.kind === "Both") {
        const btns = document.createElement("div");
        btns.className = "port-actions";

        const serialBtn = document.createElement("button");
        serialBtn.className = "port-action-btn";
        serialBtn.textContent = "Connect Serial";
        serialBtn.addEventListener("click", (e) => {
          e.stopPropagation();
          connectToPort(dev.serial_path!);
        });

        const hidBtn = document.createElement("button");
        hidBtn.className = "port-action-btn";
        hidBtn.textContent = "Connect HID";
        hidBtn.addEventListener("click", (e) => {
          e.stopPropagation();
          connectHid(dev.hid_path!, dev.product || dev.manufacturer || "HID Device", dev.vid ?? 0, dev.pid ?? 0);
        });

        btns.appendChild(serialBtn);
        btns.appendChild(hidBtn);
        div.appendChild(btns);
      } else if (dev.kind === "Ble") {
        const devName = dev.product || "BLE Device";
        const bleId = dev.ble_id!;
        div.addEventListener("click", () => connectBleNus(bleId, devName));
      } else if (dev.kind === "Serial") {
        div.addEventListener("click", () => connectToPort(dev.serial_path || dev.path));
      } else {
        div.addEventListener("click", () =>
          connectHid(dev.hid_path || dev.path, dev.product || dev.manufacturer || "HID Device", dev.vid ?? 0, dev.pid ?? 0)
        );
      }

      portListEl.appendChild(div);
    }
  } catch (e) {
    lastDeviceListJson = "";
    portListEl.innerHTML = `<div class="no-ports">Error: ${e}</div>`;
  }
}

// ---- Serial connection ----

async function connectToPort(port: string): Promise<void> {
  const baud = getSelectedBaud();
  stopDeviceListPolling();
  stopReconnect();
  userDisconnected = false;
  lastDeviceListJson = "";

  // Switch to terminal view.
  portSelectEl.classList.add("hidden");
  terminalEl.classList.remove("hidden");
  isHidMode = false;
  hidInputControls.classList.add("hidden");
  descriptorBtn.classList.add("hidden");
  serialControls.classList.remove("hidden");
  inputEl.placeholder = "Type command, Enter to send";

  // Only create a new terminal if this isn't a reconnect attempt.
  if (!terminal) {
    terminal = new TerminalUI();
  }
  terminal.setConnecting(port);
  connected = false;

  // Remember for reconnect.
  reconnectCtx = { type: "serial", port, baud };

  // Create channel for streaming lines.
  lineChannel = new Channel<SerialLine>();
  lineChannel.onmessage = (line: SerialLine) => {
    if (!terminal) return;

    terminal.appendLine(line);

    // Detect connection state from system messages.
    if (line.kind === "system") {
      if (line.text.startsWith("Connected to")) {
        connected = true;
        wasConnected = true;
        terminal.setConnected(port, baud);
      } else if (line.text.startsWith("Disconnected:")) {
        connected = false;
        scheduleReconnect();
      } else if (line.text.startsWith("Error:")) {
        if (!connected && wasConnected) {
          scheduleReconnect();
        }
      }
    }
  };

  try {
    await invoke("connect", { port, baud, onLine: lineChannel });
  } catch (e) {
    terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (wasConnected) {
      scheduleReconnect();
    }
  }

  inputEl.focus();
}

// ---- HID connection ----

async function connectHid(hidPath: string, deviceName: string, vid: number, pid: number): Promise<void> {
  stopDeviceListPolling();
  stopReconnect();
  userDisconnected = false;
  lastDeviceListJson = "";

  // Switch to terminal view.
  portSelectEl.classList.add("hidden");
  terminalEl.classList.remove("hidden");
  isHidMode = true;
  hidInputControls.classList.remove("hidden");
  descriptorBtn.classList.remove("hidden");
  serialControls.classList.add("hidden");
  inputEl.placeholder = "Enter hex bytes: 0A 1B 2C";

  if (!terminal) {
    terminal = new TerminalUI();
  }
  terminal.setConnecting(deviceName);
  connected = false;
  if (!descriptorPanel) {
    descriptorPanel = new DescriptorPanel();
  }

  // Remember for reconnect.
  reconnectCtx = { type: "hid", hidPath, deviceName, vid, pid };

  // Create channel for streaming reports.
  reportChannel = new Channel<HidReport>();
  reportChannel.onmessage = (report: HidReport) => {
    if (!terminal) return;

    if (report.kind === "error") {
      connected = false;
      terminal.appendHidReport(report);
      if (wasConnected) {
        scheduleReconnect();
      }
      return;
    }

    if (!connected) {
      connected = true;
      wasConnected = true;
      terminal.setHidConnected(deviceName);
    }

    terminal.appendHidReport(report);
  };

  try {
    await invoke("connect_hid", { hidPath, vid, pid, onReport: reportChannel });
    connected = true;
    wasConnected = true;
    terminal.setHidConnected(deviceName);
  } catch (e) {
    terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (wasConnected) {
      scheduleReconnect();
    }
  }

  inputEl.focus();
}

// ---- BLE NUS connection ----

async function connectBleNus(bleId: string, deviceName: string): Promise<void> {
  stopDeviceListPolling();
  stopReconnect();
  userDisconnected = false;
  lastDeviceListJson = "";

  portSelectEl.classList.add("hidden");
  terminalEl.classList.remove("hidden");
  isHidMode = false;
  hidInputControls.classList.add("hidden");
  descriptorBtn.classList.add("hidden");
  serialControls.classList.add("hidden");
  inputEl.placeholder = "Type command, Enter to send";

  if (!terminal) {
    terminal = new TerminalUI();
  }
  terminal.setConnecting(deviceName);
  connected = false;

  reconnectCtx = { type: "ble_nus", bleId, deviceName };

  lineChannel = new Channel<SerialLine>();
  lineChannel.onmessage = (line: SerialLine) => {
    if (!terminal) return;

    terminal.appendLine(line);

    if (line.kind === "system") {
      if (line.text.includes("disconnected")) {
        connected = false;
        scheduleReconnect();
      }
    } else if (!connected) {
      connected = true;
      wasConnected = true;
      terminal.setBleConnected(deviceName, "NUS");
    }
  };

  try {
    await invoke("connect_ble_nus", { bleId, onLine: lineChannel });
    connected = true;
    wasConnected = true;
    terminal.setBleConnected(deviceName, "NUS");
  } catch (e) {
    terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (wasConnected) {
      scheduleReconnect();
    }
  }

  inputEl.focus();
}

// ---- Reconnect ----

function scheduleReconnect(): void {
  if (userDisconnected || !reconnectCtx) return;
  stopReconnect();

  terminal?.setReconnecting();

  reconnectTimer = setTimeout(async () => {
    reconnectTimer = null;
    if (userDisconnected || !reconnectCtx) return;

    if (reconnectCtx.type === "serial") {
      await connectToPort(reconnectCtx.port);
    } else if (reconnectCtx.type === "hid") {
      // Re-enumerate to get fresh HID path (BLE devices get new DevSrvsID on reconnect).
      const ctx = reconnectCtx;
      try {
        const devices = await invoke<DeviceInfo[]>("list_devices");
        const match = devices.find(
          (d) => d.vid === ctx.vid && d.pid === ctx.pid && d.hid_path
        );
        if (match && match.hid_path) {
          ctx.hidPath = match.hid_path;
          await connectHid(match.hid_path, ctx.deviceName, ctx.vid, ctx.pid);
        } else {
          // Device not found yet, try again.
          scheduleReconnect();
        }
      } catch {
        scheduleReconnect();
      }
    } else if (reconnectCtx.type === "ble_nus") {
      await connectBleNus(reconnectCtx.bleId, reconnectCtx.deviceName);
    }
  }, 2000);
}

function stopReconnect(): void {
  if (reconnectTimer !== null) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
}

// ---- Disconnect ----

async function disconnect(): Promise<void> {
  userDisconnected = true;
  stopReconnect();
  reconnectCtx = null;
  try {
    await invoke("disconnect");
  } catch (_) {
    // ignore
  }
  showPortSelect();
}

function showPortSelect(): void {
  terminalEl.classList.add("hidden");
  portSelectEl.classList.remove("hidden");
  connected = false;
  wasConnected = false;
  isHidMode = false;
  serialControls.classList.add("hidden");
  dtrState = true;
  rtsState = false;
  dtrBtn.classList.add("active");
  rtsBtn.classList.remove("active");
  terminal = null;
  descriptorPanel = null;
  lineChannel = null;
  reportChannel = null;
  refreshDevices();
  startDeviceListPolling();
}

// ---- Input handling ----

async function sendInput(): Promise<void> {
  const text = inputEl.value;
  if (!text || !connected) return;

  // Push to history.
  if (history.length === 0 || history[history.length - 1] !== text) {
    history.push(text);
  }
  historyIdx = -1;
  inputEl.value = "";

  if (isHidMode) {
    await sendHidInput(text);
  } else {
    await sendSerialInput(text);
  }
}

async function sendSerialInput(text: string): Promise<void> {
  const ts = makeTimestamp();
  terminal?.appendLine({
    timestamp: ts,
    text,
    kind: "sent",
    rx_bytes_total: 0,
  });

  try {
    await invoke("send_data", { text });
  } catch (e) {
    terminal?.appendLine({
      timestamp: ts,
      text: `Send error: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  }
}

async function sendHidInput(hexStr: string): Promise<void> {
  const bytes = parseHexBytes(hexStr);
  if (bytes === null) {
    terminal?.appendLine({
      timestamp: makeTimestamp(),
      text: "Invalid hex input. Use space-separated hex bytes: 0A 1B 2C",
      kind: "system",
      rx_bytes_total: 0,
    });
    return;
  }

  const reportIdStr = hidReportId.value.trim();
  const reportId = reportIdStr ? parseInt(reportIdStr, 16) : 0;
  const data = [reportId, ...bytes];
  const reportType = hidReportType.value;

  const ts = makeTimestamp();
  const hex = data.map((b) => b.toString(16).padStart(2, "0").toUpperCase()).join(" ");
  terminal?.appendLine({
    timestamp: ts,
    text: `> ${reportType} [${reportId.toString(16).padStart(2, "0").toUpperCase()}] ${hex}`,
    kind: "sent",
    rx_bytes_total: 0,
  });

  try {
    await invoke("send_hid_report", { data, reportType });
  } catch (e) {
    terminal?.appendLine({
      timestamp: ts,
      text: `Send error: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  }
}

function parseHexBytes(str: string): number[] | null {
  const trimmed = str.trim();
  if (!trimmed) return [];
  const parts = trimmed.split(/\s+/);
  const bytes: number[] = [];
  for (const part of parts) {
    if (!/^[0-9a-fA-F]{1,2}$/.test(part)) return null;
    bytes.push(parseInt(part, 16));
  }
  return bytes;
}

function makeTimestamp(): string {
  const now = new Date();
  return (
    now.toLocaleTimeString("en-GB", { hour12: false }) +
    "." +
    now.getMilliseconds().toString().padStart(3, "0")
  );
}

// ---- Descriptor panel ----

async function toggleDescriptor(): Promise<void> {
  if (!descriptorPanel) return;
  try {
    const info: HidDescriptorInfo = await invoke("get_hid_descriptor");
    descriptorPanel.toggle(info);
  } catch (e) {
    console.error("Failed to get descriptor:", e);
  }
}

// ---- Search ----

function openSearch(): void {
  if (!terminal) return;
  searchBar.classList.remove("hidden");
  terminal.startSearch();
  searchInput.focus();
  searchInput.select();
}

function closeSearch(): void {
  searchBar.classList.add("hidden");
  searchInput.value = "";
  terminal?.endSearch();
  inputEl.focus();
}

searchInput.addEventListener("input", () => {
  if (searchDebounceTimer !== null) clearTimeout(searchDebounceTimer);
  searchDebounceTimer = setTimeout(() => {
    terminal?.updateSearch(searchInput.value);
  }, 100);
});

searchInput.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.key === "Enter" && e.shiftKey) {
    e.preventDefault();
    terminal?.prevMatch();
  } else if (e.key === "Enter") {
    e.preventDefault();
    terminal?.nextMatch();
  } else if (e.key === "Escape") {
    e.preventDefault();
    closeSearch();
  }
});

searchCloseBtn.addEventListener("click", closeSearch);
searchPrevBtn.addEventListener("click", () => terminal?.prevMatch());
searchNextBtn.addEventListener("click", () => terminal?.nextMatch());
searchBtn.addEventListener("click", () => {
  if (searchBar.classList.contains("hidden")) {
    openSearch();
  } else {
    closeSearch();
  }
});

// ---- Keyboard shortcuts ----

inputEl.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.key === "Enter") {
    e.preventDefault();
    sendInput();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (history.length === 0) return;
    if (historyIdx === -1) {
      savedInput = inputEl.value;
      historyIdx = history.length - 1;
    } else if (historyIdx > 0) {
      historyIdx--;
    }
    inputEl.value = history[historyIdx];
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    if (historyIdx === -1) return;
    if (historyIdx < history.length - 1) {
      historyIdx++;
      inputEl.value = history[historyIdx];
    } else {
      historyIdx = -1;
      inputEl.value = savedInput;
    }
  } else if (e.key === "Escape") {
    e.preventDefault();
    terminal?.scrollToBottom();
  }
});

document.addEventListener("keydown", (e: KeyboardEvent) => {
  if ((e.metaKey || e.ctrlKey) && e.key === "k") {
    e.preventDefault();
    terminal?.clear();
  }
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "C") {
    e.preventDefault();
    copyLog();
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "s") {
    e.preventDefault();
    if (terminal) exportLog();
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "f") {
    e.preventDefault();
    if (terminal) openSearch();
  }
});

// ---- Copy log ----

async function copyLog(): Promise<void> {
  if (!terminal) return;
  const ok = await terminal.copyLog();
  if (ok) {
    copyBtn.textContent = "Copied!";
    setTimeout(() => (copyBtn.textContent = "Copy"), 1500);
  }
}

// ---- Log export ----

async function exportLog(): Promise<void> {
  const path = await save({
    title: "Export Log",
    defaultPath: `comrade_${new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19)}.log`,
    filters: [
      { name: "Log files", extensions: ["log", "txt"] },
      { name: "CSV", extensions: ["csv"] },
    ],
  });
  if (!path) return;

  const format = path.endsWith(".csv") ? "csv" : "text";
  try {
    const count = await invoke<number>("export_log", { path, format });
    terminal?.appendLine({
      timestamp: makeTimestamp(),
      text: `Exported ${count} entries to ${path}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  } catch (e) {
    terminal?.appendLine({
      timestamp: makeTimestamp(),
      text: `Export failed: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  }
}

// ---- Auto-logging ----

let autoLogActive = false;
const autologBtn = document.getElementById("autolog-btn")!;

async function toggleAutoLog(): Promise<void> {
  if (autoLogActive) {
    // Stop auto-logging.
    try {
      const result = await invoke<[string, number] | null>("stop_auto_log");
      autoLogActive = false;
      autologBtn.classList.remove("active");
      autologBtn.textContent = "Log";
      if (result) {
        terminal?.appendLine({
          timestamp: makeTimestamp(),
          text: `Auto-log stopped: ${result[1]} entries saved to ${result[0]}`,
          kind: "system",
          rx_bytes_total: 0,
        });
      }
    } catch (e) {
      console.error("Stop auto-log:", e);
    }
  } else {
    // Pick a directory and start.
    const dir = await open({
      title: "Choose log directory",
      directory: true,
    });
    if (!dir) return;

    try {
      const path = await invoke<string>("start_auto_log", { directory: dir });
      autoLogActive = true;
      autologBtn.classList.add("active");
      autologBtn.textContent = "Log ●";
      terminal?.appendLine({
        timestamp: makeTimestamp(),
        text: `Auto-logging to ${path}`,
        kind: "system",
        rx_bytes_total: 0,
      });
    } catch (e) {
      terminal?.appendLine({
        timestamp: makeTimestamp(),
        text: `Auto-log failed: ${e}`,
        kind: "system",
        rx_bytes_total: 0,
      });
    }
  }
}

autologBtn.addEventListener("click", toggleAutoLog);

// Check if auto-log is already active (e.g. reconnect scenario).
invoke<string | null>("auto_log_status").then((path) => {
  if (path) {
    autoLogActive = true;
    autologBtn.classList.add("active");
    autologBtn.textContent = "Log ●";
  }
});

// ---- MCP copy ----

const mcpCopyBtn = document.getElementById("mcp-copy-btn")!;
const mcpCmd = document.getElementById("mcp-cmd")!;

mcpCopyBtn.addEventListener("click", async () => {
  await navigator.clipboard.writeText(mcpCmd.textContent!);
  mcpCopyBtn.textContent = "Copied!";
  setTimeout(() => (mcpCopyBtn.textContent = "Copy"), 1500);
});

// ---- Event listeners ----

const exportBtn = document.getElementById("export-btn")!;

// Serial control buttons.
let dtrState = true; // DTR is on by default (set on connect)
let rtsState = false;

dtrBtn.addEventListener("click", async () => {
  dtrState = !dtrState;
  dtrBtn.classList.toggle("active", dtrState);
  try {
    await invoke("set_dtr", { active: dtrState });
  } catch (e) {
    console.error("DTR:", e);
  }
});

rtsBtn.addEventListener("click", async () => {
  rtsState = !rtsState;
  rtsBtn.classList.toggle("active", rtsState);
  try {
    await invoke("set_rts", { active: rtsState });
  } catch (e) {
    console.error("RTS:", e);
  }
});

breakBtn.addEventListener("click", async () => {
  try {
    await invoke("send_break");
    terminal?.appendLine({
      timestamp: makeTimestamp(),
      text: "Break signal sent",
      kind: "system",
      rx_bytes_total: 0,
    });
  } catch (e) {
    console.error("Break:", e);
  }
});

// Timestamp format cycle — click the RX counter to cycle.
document.getElementById("status-rx")!.addEventListener("click", () => {
  if (!terminal) return;
  const fmt = terminal.cycleTimestampFormat();
  const label: Record<string, string> = { time: "Time", elapsed: "Elapsed", iso: "ISO" };
  terminal.appendLine({
    timestamp: makeTimestamp(),
    text: `Timestamp format: ${label[fmt] ?? fmt}`,
    kind: "system",
    rx_bytes_total: 0,
  });
});

refreshBtn.addEventListener("click", refreshDevices);
exportBtn.addEventListener("click", exportLog);
copyBtn.addEventListener("click", copyLog);
clearBtn.addEventListener("click", () => terminal?.clear());
disconnectBtn.addEventListener("click", disconnect);
descriptorBtn.addEventListener("click", toggleDescriptor);

// ---- Init ----

refreshDevices();
startDeviceListPolling();
