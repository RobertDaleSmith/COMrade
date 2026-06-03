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

// ---- Reconnect context type ----

type ReconnectCtx =
  | { type: "serial"; port: string; baud: number }
  | { type: "hid"; hidPath: string; deviceName: string; vid: number; pid: number }
  | { type: "ble_nus"; bleId: string; deviceName: string };

// ---- Per-tab state ----

interface Tab {
  id: string;
  label: string;
  terminal: TerminalUI;
  descriptorPanel: DescriptorPanel | null;
  connected: boolean;
  wasConnected: boolean;
  isHidMode: boolean;
  userDisconnected: boolean;
  reconnectCtx: ReconnectCtx | null;
  reconnectTimer: ReturnType<typeof setTimeout> | null;
  reconnectDelay: number;
  reconnectAttempts: number;
  lineChannel: Channel<SerialLine> | null;
  reportChannel: Channel<HidReport> | null;
  history: string[];
  historyIdx: number;
  savedInput: string;
  dtrState: boolean;
  rtsState: boolean;
  draftInput: string;
  isRemote: boolean;
  tabBtn: HTMLElement;
}

const tabs = new Map<string, Tab>();
let activeTabId: string | null = null;

function activeTab(): Tab | null {
  return activeTabId ? tabs.get(activeTabId) ?? null : null;
}

// ---- DOM elements ----

const portSelectEl = document.getElementById("port-select")!;
const terminalEl = document.getElementById("terminal")!;
const portListEl = document.getElementById("port-list")!;
const baudSelect = document.getElementById("baud-select") as HTMLSelectElement;
const refreshBtn = document.getElementById("refresh-btn")!;
const inputEl = document.getElementById("input") as HTMLInputElement;
const copyBtn = document.getElementById("copy-btn")!;
const clearBtn = document.getElementById("clear-btn")!;
const pauseBtn = document.getElementById("pause-btn")!;
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
const tabBar = document.getElementById("tab-bar")!;
const newTabBtn = document.getElementById("new-tab-btn")!;

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

// Timers.
let deviceListTimer: ReturnType<typeof setInterval> | null = null;
let searchDebounceTimer: ReturnType<typeof setTimeout> | null = null;
let lastDeviceListJson = "";

// ---- Tab management ----

function generateTabId(): string {
  return crypto.randomUUID();
}

type TabKind = "serial" | "hid" | "ble";

const TAB_ICONS: Record<TabKind, string> = {
  serial: "CDC",
  hid: "HID",
  ble: "BLE",
};

function createTab(label: string, tooltip?: string, kind: TabKind = "serial"): Tab {
  const id = generateTabId();
  const terminal = new TerminalUI();
  terminal.setTimestampsVisible(showTimestamps);
  terminal.setPauseChangeHandler((paused, heldCount) => {
    // Only the active tab's terminal drives the toolbar button.
    if (activeTab()?.terminal === terminal) {
      renderPauseBtn(paused, heldCount);
    }
  });

  // Create tab button in the tab bar.
  const tabBtn = document.createElement("div");
  tabBtn.className = "tab-btn";
  tabBtn.dataset.tabId = id;
  if (tooltip) tabBtn.title = tooltip;

  const tabIcon = document.createElement("span");
  tabIcon.className = "tab-icon";
  tabIcon.textContent = TAB_ICONS[kind];

  const tabLabel = document.createElement("span");
  tabLabel.className = "tab-label";
  tabLabel.textContent = label;

  const closeBtn = document.createElement("span");
  closeBtn.className = "tab-close";
  closeBtn.textContent = "\u00d7";
  closeBtn.title = "Close tab";
  closeBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    closeTab(id);
  });

  tabBtn.appendChild(tabIcon);
  tabBtn.appendChild(tabLabel);
  tabBtn.appendChild(closeBtn);
  tabBtn.addEventListener("click", () => switchTab(id));

  // Insert before the "+" button.
  tabBar.insertBefore(tabBtn, newTabBtn);

  const tab: Tab = {
    id,
    label,
    terminal,
    descriptorPanel: null,
    connected: false,
    wasConnected: false,
    isHidMode: false,
    userDisconnected: false,
    reconnectCtx: null,
    reconnectTimer: null,
    reconnectDelay: 2000,
    reconnectAttempts: 0,
    lineChannel: null,
    reportChannel: null,
    history: [],
    historyIdx: -1,
    savedInput: "",
    dtrState: true,
    rtsState: false,
    draftInput: "",
    isRemote: false,
    tabBtn,
  };

  tabs.set(id, tab);
  return tab;
}

function switchTab(tabId: string): void {
  const tab = tabs.get(tabId);
  if (!tab) return;

  // Hide inline device selector if showing.
  hideInlineDeviceSelector();

  // Save current tab's input text.
  const current = activeTab();
  if (current) {
    current.draftInput = inputEl.value;
    current.terminal.hide();
    current.tabBtn.classList.remove("active");
  }

  activeTabId = tabId;
  tab.terminal.show();
  tab.tabBtn.classList.add("active");

  // Restore this tab's input text.
  inputEl.value = tab.draftInput;

  // Update toolbar to reflect this tab's state.
  updateToolbar(tab);
  inputEl.focus();
}

function updateToolbar(tab: Tab): void {
  if (tab.isHidMode) {
    hidInputControls.classList.remove("hidden");
    descriptorBtn.classList.remove("hidden");
    serialControls.classList.add("hidden");
    inputEl.placeholder = "Enter hex bytes: 0A 1B 2C";
  } else {
    hidInputControls.classList.add("hidden");
    descriptorBtn.classList.add("hidden");
    // Only show serial controls for serial connections.
    if (tab.reconnectCtx?.type === "serial") {
      serialControls.classList.remove("hidden");
    } else {
      serialControls.classList.add("hidden");
    }
    inputEl.placeholder = "Type command, Enter to send";
  }

  dtrBtn.classList.toggle("active", tab.dtrState);
  rtsBtn.classList.toggle("active", tab.rtsState);
  renderPauseBtn(tab.terminal.isPaused(), tab.terminal.heldLineCount());
}

function renderPauseBtn(paused: boolean, heldCount: number): void {
  pauseBtn.classList.toggle("active", paused);
  pauseBtn.title = paused ? "Resume (Space)" : "Pause (Space)";
  const countEl = pauseBtn.querySelector(".held-count") as HTMLElement | null;
  if (countEl) {
    countEl.textContent = paused && heldCount > 0 ? String(heldCount) : "";
  }
}

function closeTab(tabId: string): void {
  const tab = tabs.get(tabId);
  if (!tab) return;

  // Stop reconnect.
  if (tab.reconnectTimer !== null) {
    clearTimeout(tab.reconnectTimer);
  }

  // Disconnect in backend.
  invoke("disconnect", { tabId }).catch(() => {});

  // Remove DOM elements.
  tab.terminal.destroy();
  tab.tabBtn.remove();
  tabs.delete(tabId);

  if (activeTabId === tabId) {
    activeTabId = null;
    // Switch to another tab or show device selector.
    const remaining = Array.from(tabs.keys());
    if (remaining.length > 0) {
      switchTab(remaining[remaining.length - 1]);
    } else {
      showPortSelect();
    }
  }

  // Hide tab bar if only one tab remains.
  tabBar.classList.remove("hidden");
}

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
    const devices = await invoke<DeviceInfo[]>("list_devices");

    // Check for a remote headless CLI session.
    const remoteStatus = await invoke<any>("check_remote_mcp").catch(() => null);

    const json = JSON.stringify(devices) + JSON.stringify(remoteStatus);
    if (json === lastDeviceListJson) return;
    lastDeviceListJson = json;

    portListEl.innerHTML = "";

    // Show remote session banner if headless CLI is connected.
    if (remoteStatus && remoteStatus.connected && remoteStatus.port) {
      const div = document.createElement("div");
      div.className = "port-item remote-item";

      const topRow = document.createElement("div");
      topRow.className = "port-item-top";

      const name = document.createElement("div");
      name.className = "port-name";
      name.textContent = remoteStatus.port;

      const badges = document.createElement("div");
      badges.className = "device-badges";
      const remoteBadge = document.createElement("span");
      remoteBadge.className = "device-badge badge-ble";
      remoteBadge.textContent = "REMOTE";
      const cdcBadge = document.createElement("span");
      cdcBadge.className = "device-badge badge-serial";
      cdcBadge.textContent = "CDC";
      badges.appendChild(remoteBadge);
      badges.appendChild(cdcBadge);

      topRow.appendChild(name);
      topRow.appendChild(badges);

      const path = document.createElement("div");
      path.className = "port-path";
      path.textContent = `Headless CLI session @ ${remoteStatus.baud || 115200} baud`;

      div.appendChild(topRow);
      div.appendChild(path);
      div.addEventListener("click", () => connectRemote(remoteStatus.port));
      portListEl.appendChild(div);
    }

    if (devices.length === 0 && !remoteStatus?.connected) {
      portListEl.innerHTML += '<div class="no-ports"><div class="no-ports-icon">\u{1F50C}</div><div>No devices found</div><div class="no-ports-hint">Connect a USB or BLE device to get started</div></div>';
      return;
    }
    for (const dev of devices) {
      const div = document.createElement("div");
      div.className = "port-item";

      const topRow = document.createElement("div");
      topRow.className = "port-item-top";

      const devName = dev.product || dev.manufacturer || "Unknown device";
      const name = document.createElement("div");
      name.className = "port-name";
      name.textContent = devName;

      const badges = document.createElement("div");
      badges.className = "device-badges";
      const bus = dev.bus_type;
      const svcs = dev.ble_services || [];

      const addBadge = (text: string, cls: string) => {
        const b = document.createElement("span");
        b.className = `device-badge ${cls}`;
        b.textContent = text;
        badges.appendChild(b);
      };

      // Transport badge first.
      if (bus === "Bluetooth" || dev.kind === "Ble") {
        addBadge("BT", "badge-ble");
      } else if (bus === "USB") {
        addBadge("USB", "badge-usb");
      } else if (bus) {
        addBadge(bus, "badge-usb");
      }

      // Interface badges.
      if (dev.kind === "Both") {
        addBadge("CDC", "badge-serial");
        addBadge("HID", "badge-hid");
      } else if (dev.kind === "Ble") {
        if (svcs.includes("nus")) addBadge("NUS", "badge-serial");
        if (svcs.includes("hid")) addBadge("HID", "badge-hid");
      } else if (dev.kind === "Hid") {
        addBadge("HID", "badge-hid");
      } else if (dev.kind === "Serial") {
        addBadge("CDC", "badge-serial");
      }

      topRow.appendChild(name);
      topRow.appendChild(badges);

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
        serialBtn.textContent = "Connect CDC";
        serialBtn.addEventListener("click", (e) => {
          e.stopPropagation();
          connectToPort(dev.serial_path!, devName);
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
        const svcs = dev.ble_services || [];
        const hasNus = svcs.includes("nus");
        const hasHid = svcs.includes("hid") && dev.hid_path;
        if (hasNus && hasHid) {
          const btns = document.createElement("div");
          btns.className = "port-actions";
          const nusBtn = document.createElement("button");
          nusBtn.className = "port-action-btn";
          nusBtn.textContent = "Connect NUS";
          nusBtn.addEventListener("click", (e) => {
            e.stopPropagation();
            connectBleNus(dev.ble_id!, dev.product || "BLE Device");
          });
          const hidBtn = document.createElement("button");
          hidBtn.className = "port-action-btn";
          hidBtn.textContent = "Connect HID";
          hidBtn.addEventListener("click", (e) => {
            e.stopPropagation();
            connectHid(dev.hid_path!, dev.product || dev.manufacturer || "BLE HID Device", dev.vid ?? 0, dev.pid ?? 0);
          });
          btns.appendChild(nusBtn);
          btns.appendChild(hidBtn);
          div.appendChild(btns);
        } else if (hasNus) {
          div.addEventListener("click", () => connectBleNus(dev.ble_id!, dev.product || "BLE Device"));
        } else if (hasHid) {
          div.addEventListener("click", () =>
            connectHid(dev.hid_path!, dev.product || dev.manufacturer || "BLE HID Device", dev.vid ?? 0, dev.pid ?? 0)
          );
        }
      } else if (dev.kind === "Serial") {
        div.addEventListener("click", () => connectToPort(dev.serial_path || dev.path, devName));
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

// ---- Switch to terminal view ----

function showTerminalView(tab: Tab): void {
  stopDeviceListPolling();
  lastDeviceListJson = "";

  // If port-select is inline (inside terminal-body), move it back.
  hideInlineDeviceSelector();

  portSelectEl.classList.add("hidden");
  terminalEl.classList.remove("hidden");

  tabBar.classList.remove("hidden");

  switchTab(tab.id);
}

/** Find an existing tab actively connected to this device. */
function findExistingTab(type: string, id: string, vid?: number, pid?: number): Tab | undefined {
  for (const tab of tabs.values()) {
    if (!tab.connected) continue;
    const ctx = tab.reconnectCtx;
    if (!ctx) continue;
    if (type === "serial" && ctx.type === "serial" && ctx.port === id) return tab;
    if (type === "hid" && ctx.type === "hid" && (ctx.hidPath === id || (vid != null && pid != null && ctx.vid === vid && ctx.pid === pid))) return tab;
    if (type === "ble_nus" && ctx.type === "ble_nus" && ctx.bleId === id) return tab;
  }
  return undefined;
}

// ---- Serial connection ----

async function connectToPort(port: string, deviceName?: string): Promise<void> {
  const existing = findExistingTab("serial", port);
  if (existing) { showTerminalView(existing); return; }

  const baud = getSelectedBaud();
  const label = deviceName || port.split("/").pop() || port;
  const tab = createTab(label, port, "serial");

  tab.isHidMode = false;
  showTerminalView(tab);
  tab.terminal.setConnecting(port);

  tab.reconnectCtx = { type: "serial", port, baud };

  tab.lineChannel = new Channel<SerialLine>();
  tab.lineChannel.onmessage = (line: SerialLine) => {
    tab.terminal.appendLine(line);

    // Mark connected on first data or explicit Connected event.
    if (!tab.connected && (line.kind === "received" || (line.kind === "system" && line.text.startsWith("Connected to")))) {
      markConnected(tab);
      tab.terminal.setConnected(port, baud);
    }

    if (line.kind === "system") {
      if (line.text.startsWith("Disconnected:")) {
        tab.connected = false;
        scheduleReconnect(tab);
      } else if (line.text.startsWith("Error:")) {
        if (!tab.connected && tab.wasConnected) {
          scheduleReconnect(tab);
        }
      }
    }
  };

  try {
    await invoke("connect", { tabId: tab.id, port, baud, onLine: tab.lineChannel });
    // With the daemon, the connection may already be established.
    // Mark connected immediately — the line callback will confirm with data.
    markConnected(tab);
    tab.terminal.setConnected(port, baud);
  } catch (e) {
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (tab.wasConnected) {
      scheduleReconnect(tab);
    }
  }

  inputEl.focus();
}

// ---- HID connection ----

async function connectHid(hidPath: string, deviceName: string, vid: number, pid: number): Promise<void> {
  const existing = findExistingTab("hid", hidPath, vid, pid);
  if (existing) { showTerminalView(existing); return; }

  const tab = createTab(deviceName, hidPath, "hid");

  tab.isHidMode = true;
  showTerminalView(tab);
  tab.terminal.setConnecting(deviceName);

  tab.descriptorPanel = new DescriptorPanel();
  tab.reconnectCtx = { type: "hid", hidPath, deviceName, vid, pid };

  tab.reportChannel = new Channel<HidReport>();
  tab.reportChannel.onmessage = (report: HidReport) => {
    if (report.kind === "error") {
      tab.connected = false;
      tab.terminal.appendHidReport(report);
      if (tab.wasConnected) {
        scheduleReconnect(tab);
      }
      return;
    }

    if (!tab.connected) {
      markConnected(tab);
      tab.terminal.setHidConnected(deviceName);
    }

    tab.terminal.appendHidReport(report);
  };

  try {
    await invoke("connect_hid", { tabId: tab.id, hidPath, vid, pid, onReport: tab.reportChannel });
    markConnected(tab);
    tab.terminal.setHidConnected(deviceName);
  } catch (e) {
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (tab.wasConnected) {
      scheduleReconnect(tab);
    }
  }

  inputEl.focus();
}

// ---- BLE NUS connection ----

async function connectBleNus(bleId: string, deviceName: string): Promise<void> {
  const existing = findExistingTab("ble_nus", bleId);
  if (existing) { showTerminalView(existing); return; }

  const tab = createTab(deviceName, bleId, "ble");

  tab.isHidMode = false;
  showTerminalView(tab);
  tab.terminal.setConnecting(deviceName);

  tab.reconnectCtx = { type: "ble_nus", bleId, deviceName };

  tab.lineChannel = new Channel<SerialLine>();
  tab.lineChannel.onmessage = (line: SerialLine) => {
    tab.terminal.appendLine(line);

    if (line.kind === "system") {
      if (line.text.includes("disconnected")) {
        tab.connected = false;
        scheduleReconnect(tab);
      }
    } else if (!tab.connected) {
      markConnected(tab);
      tab.terminal.setBleConnected(deviceName, "NUS");
    }
  };

  try {
    await invoke("connect_ble_nus", { tabId: tab.id, bleId, onLine: tab.lineChannel });
    markConnected(tab);
    tab.terminal.setBleConnected(deviceName, "NUS");
  } catch (e) {
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
    if (tab.wasConnected) {
      scheduleReconnect(tab);
    }
  }

  inputEl.focus();
}

// ---- Remote connection (headless CLI) ----

async function connectRemote(port: string): Promise<void> {
  const existing = findExistingTab("serial", port);
  if (existing) { showTerminalView(existing); return; }

  const label = port.split("/").pop() || port;
  const tab = createTab(label, port, "serial");
  tab.isRemote = true;

  showTerminalView(tab);
  tab.terminal.setConnecting(`${port} (remote)`);

  tab.lineChannel = new Channel<SerialLine>();
  tab.lineChannel.onmessage = (line: SerialLine) => {
    tab.terminal.appendLine(line);
    if (!tab.connected && line.kind !== "system") {
      markConnected(tab);
      tab.terminal.setConnected(port, 115200);
    }
  };

  try {
    await invoke("connect_remote", { tabId: tab.id, onLine: tab.lineChannel });
    markConnected(tab);
    tab.terminal.setConnected(port, 115200);
  } catch (e) {
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Failed to connect to remote: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  }

  inputEl.focus();
}

function markConnected(tab: Tab): void {
  tab.connected = true;
  tab.wasConnected = true;
  tab.reconnectDelay = 2000;
  tab.reconnectAttempts = 0;
}

// ---- Reconnect ----

const MAX_RECONNECT_ATTEMPTS = 50;

function scheduleReconnect(tab: Tab): void {
  if (tab.userDisconnected || !tab.reconnectCtx) return;
  stopReconnect(tab);

  tab.reconnectAttempts++;
  if (tab.reconnectAttempts > MAX_RECONNECT_ATTEMPTS) {
    tab.terminal.setDisconnected("max reconnect attempts reached");
    return;
  }

  const delay = tab.reconnectDelay;
  // Only show "RECONNECTING" on first attempt to avoid DOM spam.
  if (tab.reconnectAttempts === 1) {
    tab.terminal.setReconnecting();
  }

  tab.reconnectTimer = setTimeout(async () => {
    tab.reconnectTimer = null;
    if (tab.userDisconnected || !tab.reconnectCtx) return;

    // Increase delay for next attempt (exponential backoff, max 30s).
    tab.reconnectDelay = Math.min(tab.reconnectDelay * 2, 30000);

    if (tab.reconnectCtx.type === "serial") {
      await reconnectSerial(tab, tab.reconnectCtx.port, tab.reconnectCtx.baud);
    } else if (tab.reconnectCtx.type === "hid") {
      const ctx = tab.reconnectCtx;
      try {
        const devices = await invoke<DeviceInfo[]>("list_devices");
        const match = devices.find(
          (d) => d.vid === ctx.vid && d.pid === ctx.pid && d.hid_path
        );
        if (match && match.hid_path) {
          ctx.hidPath = match.hid_path;
          await reconnectHid(tab, match.hid_path, ctx.deviceName, ctx.vid, ctx.pid);
        } else {
          scheduleReconnect(tab);
        }
      } catch {
        scheduleReconnect(tab);
      }
    } else if (tab.reconnectCtx.type === "ble_nus") {
      await reconnectBleNus(tab, tab.reconnectCtx.bleId, tab.reconnectCtx.deviceName);
    }
  }, delay);
}

function stopReconnect(tab: Tab): void {
  if (tab.reconnectTimer !== null) {
    clearTimeout(tab.reconnectTimer);
    tab.reconnectTimer = null;
  }
}

// Reconnect helpers — reuse existing tab instead of creating new one.

async function reconnectSerial(tab: Tab, port: string, baud: number): Promise<void> {
  tab.lineChannel = new Channel<SerialLine>();
  tab.lineChannel.onmessage = (line: SerialLine) => {
    tab.terminal.appendLine(line);
    if (line.kind === "system") {
      if (line.text.startsWith("Connected to")) {
        markConnected(tab);
        tab.terminal.setConnected(port, baud);
      } else if (line.text.startsWith("Disconnected:")) {
        tab.connected = false;
        scheduleReconnect(tab);
      } else if (line.text.startsWith("Error:") && !tab.connected && tab.wasConnected) {
        scheduleReconnect(tab);
      }
    }
  };
  try {
    await invoke("connect", { tabId: tab.id, port, baud, onLine: tab.lineChannel });
  } catch {
    scheduleReconnect(tab);
  }
}

async function reconnectHid(tab: Tab, hidPath: string, deviceName: string, vid: number, pid: number): Promise<void> {
  tab.reportChannel = new Channel<HidReport>();
  tab.reportChannel.onmessage = (report: HidReport) => {
    if (report.kind === "error") {
      tab.connected = false;
      tab.terminal.appendHidReport(report);
      if (tab.wasConnected) scheduleReconnect(tab);
      return;
    }
    if (!tab.connected) {
      markConnected(tab);
      tab.terminal.setHidConnected(deviceName);
    }
    tab.terminal.appendHidReport(report);
  };
  try {
    await invoke("connect_hid", { tabId: tab.id, hidPath, vid, pid, onReport: tab.reportChannel });
    markConnected(tab);
    tab.terminal.setHidConnected(deviceName);
  } catch {
    scheduleReconnect(tab);
  }
}

async function reconnectBleNus(tab: Tab, bleId: string, deviceName: string): Promise<void> {
  tab.lineChannel = new Channel<SerialLine>();
  tab.lineChannel.onmessage = (line: SerialLine) => {
    tab.terminal.appendLine(line);
    if (line.kind === "system" && line.text.includes("disconnected")) {
      tab.connected = false;
      scheduleReconnect(tab);
    } else if (!tab.connected) {
      markConnected(tab);
      tab.terminal.setBleConnected(deviceName, "NUS");
    }
  };
  try {
    await invoke("connect_ble_nus", { tabId: tab.id, bleId, onLine: tab.lineChannel });
    markConnected(tab);
    tab.terminal.setBleConnected(deviceName, "NUS");
  } catch {
    scheduleReconnect(tab);
  }
}

// ---- Disconnect ----

async function disconnect(): Promise<void> {
  const tab = activeTab();
  if (!tab) return;

  tab.userDisconnected = true;
  stopReconnect(tab);
  closeTab(tab.id);
}

function showPortSelect(): void {
  terminalEl.classList.add("hidden");
  tabBar.classList.add("hidden");
  portSelectEl.classList.remove("hidden");
  serialControls.classList.add("hidden");
  activeTabId = null;
  refreshDevices();
  startDeviceListPolling();
}

// ---- New tab button ----

const terminalBody = document.getElementById("terminal-body")!;
const portSelectParent = portSelectEl.parentElement!;

let newTabPlaceholder: HTMLElement | null = null;

/** Show device selector inline within terminal-body as a "New" tab. */
function showInlineDeviceSelector(): void {
  // If already showing, just focus it.
  if (newTabPlaceholder) return;

  // Hide active tab output.
  const current = activeTab();
  if (current) {
    current.terminal.hide();
    current.tabBtn.classList.remove("active");
  }
  activeTabId = null;

  // Create a placeholder "New" tab button.
  newTabPlaceholder = document.createElement("div");
  newTabPlaceholder.className = "tab-btn active";
  const placeholderLabel = document.createElement("span");
  placeholderLabel.className = "tab-label";
  placeholderLabel.textContent = "New";
  const placeholderClose = document.createElement("span");
  placeholderClose.className = "tab-close";
  placeholderClose.textContent = "\u00d7";
  placeholderClose.addEventListener("click", (e) => {
    e.stopPropagation();
    hideInlineDeviceSelector();
    // Switch to last real tab or show full-screen selector.
    const remaining = Array.from(tabs.keys());
    if (remaining.length > 0) {
      switchTab(remaining[remaining.length - 1]);
    } else {
      showPortSelect();
    }
  });
  newTabPlaceholder.appendChild(placeholderLabel);
  newTabPlaceholder.appendChild(placeholderClose);
  tabBar.insertBefore(newTabPlaceholder, newTabBtn);

  // Move port-select into terminal-body and hide input bar.
  portSelectEl.classList.add("inline");
  terminalBody.appendChild(portSelectEl);
  portSelectEl.classList.remove("hidden");
  document.getElementById("input-bar")!.classList.add("hidden");

  lastDeviceListJson = "";
  refreshDevices();
  startDeviceListPolling();
}

/** Move port-select back and remove placeholder tab. */
function hideInlineDeviceSelector(): void {
  portSelectEl.classList.remove("inline");
  portSelectEl.classList.add("hidden");
  portSelectParent.appendChild(portSelectEl);
  stopDeviceListPolling();
  document.getElementById("input-bar")!.classList.remove("hidden");

  if (newTabPlaceholder) {
    newTabPlaceholder.remove();
    newTabPlaceholder = null;
  }
}

newTabBtn.addEventListener("click", () => {
  showInlineDeviceSelector();
});

// ---- Input handling ----

async function sendInput(): Promise<void> {
  const tab = activeTab();
  if (!tab || !tab.connected) return;
  const text = inputEl.value;
  if (!text) return;

  if (tab.history.length === 0 || tab.history[tab.history.length - 1] !== text) {
    tab.history.push(text);
  }
  tab.historyIdx = -1;
  inputEl.value = "";

  if (tab.isHidMode) {
    await sendHidInput(tab, text);
  } else {
    await sendSerialInput(tab, text);
  }
}

async function sendSerialInput(tab: Tab, text: string): Promise<void> {
  const ts = makeTimestamp();
  tab.terminal.appendLine({
    timestamp: ts,
    text,
    kind: "sent",
    rx_bytes_total: 0,
  });

  try {
    if (tab.isRemote) {
      await invoke("send_remote", { text });
    } else {
      await invoke("send_data", { tabId: tab.id, text });
    }
  } catch (e) {
    tab.terminal.appendLine({
      timestamp: ts,
      text: `Send error: ${e}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  }
}

async function sendHidInput(tab: Tab, hexStr: string): Promise<void> {
  const bytes = parseHexBytes(hexStr);
  if (bytes === null) {
    tab.terminal.appendLine({
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
  tab.terminal.appendLine({
    timestamp: ts,
    text: `> ${reportType} [${reportId.toString(16).padStart(2, "0").toUpperCase()}] ${hex}`,
    kind: "sent",
    rx_bytes_total: 0,
  });

  try {
    await invoke("send_hid_report", { tabId: tab.id, data, reportType });
  } catch (e) {
    tab.terminal.appendLine({
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
  const tab = activeTab();
  if (!tab?.descriptorPanel) return;
  try {
    const info: HidDescriptorInfo = await invoke("get_hid_descriptor", { tabId: tab.id });
    tab.descriptorPanel.toggle(info);
  } catch (e) {
    console.error("Failed to get descriptor:", e);
  }
}

// ---- Search ----

function openSearch(): void {
  const tab = activeTab();
  if (!tab) return;
  searchBar.classList.remove("hidden");
  tab.terminal.startSearch();
  searchInput.focus();
  searchInput.select();
}

function closeSearch(): void {
  searchBar.classList.add("hidden");
  searchInput.value = "";
  activeTab()?.terminal.endSearch();
  inputEl.focus();
}

searchInput.addEventListener("input", () => {
  if (searchDebounceTimer !== null) clearTimeout(searchDebounceTimer);
  searchDebounceTimer = setTimeout(() => {
    activeTab()?.terminal.updateSearch(searchInput.value);
  }, 100);
});

searchInput.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.key === "Enter" && e.shiftKey) {
    e.preventDefault();
    activeTab()?.terminal.prevMatch();
  } else if (e.key === "Enter") {
    e.preventDefault();
    activeTab()?.terminal.nextMatch();
  } else if (e.key === "Escape") {
    e.preventDefault();
    closeSearch();
  }
});

searchCloseBtn.addEventListener("click", closeSearch);
searchPrevBtn.addEventListener("click", () => activeTab()?.terminal.prevMatch());
searchNextBtn.addEventListener("click", () => activeTab()?.terminal.nextMatch());
searchBtn.addEventListener("click", () => {
  if (searchBar.classList.contains("hidden")) {
    openSearch();
  } else {
    closeSearch();
  }
});

// ---- Keyboard shortcuts ----

inputEl.addEventListener("keydown", (e: KeyboardEvent) => {
  const tab = activeTab();
  if (e.key === "Enter") {
    e.preventDefault();
    sendInput();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (!tab || tab.history.length === 0) return;
    if (tab.historyIdx === -1) {
      tab.savedInput = inputEl.value;
      tab.historyIdx = tab.history.length - 1;
    } else if (tab.historyIdx > 0) {
      tab.historyIdx--;
    }
    inputEl.value = tab.history[tab.historyIdx];
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    if (!tab || tab.historyIdx === -1) return;
    if (tab.historyIdx < tab.history.length - 1) {
      tab.historyIdx++;
      inputEl.value = tab.history[tab.historyIdx];
    } else {
      tab.historyIdx = -1;
      inputEl.value = tab.savedInput;
    }
  } else if (e.key === "Escape") {
    e.preventDefault();
    tab?.terminal.scrollToBottom();
  }
});

document.addEventListener("keydown", (e: KeyboardEvent) => {
  const tab = activeTab();
  if ((e.metaKey || e.ctrlKey) && e.key === "w") {
    e.preventDefault();
    // Close the "New" placeholder tab if it's open.
    if (newTabPlaceholder) {
      hideInlineDeviceSelector();
      const remaining = Array.from(tabs.keys());
      if (remaining.length > 0) {
        switchTab(remaining[remaining.length - 1]);
      } else {
        showPortSelect();
      }
      return;
    }
    if (tab) {
      tab.userDisconnected = true;
      stopReconnect(tab);
      closeTab(tab.id);
    }
    // Close app if no tabs remain.
    if (tabs.size === 0) {
      import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
        getCurrentWindow().close();
      });
    }
    return;
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "k") {
    e.preventDefault();
    tab?.terminal.clear();
  }
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "C") {
    e.preventDefault();
    copyLog();
  }
  if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "B") {
    e.preventDefault();
    invoke<string[]>("debug_ble").then((lines) => {
      for (const line of lines) {
        tab?.terminal.appendLine({
          timestamp: makeTimestamp(),
          text: line,
          kind: "system",
          rx_bytes_total: 0,
        });
      }
      if (lines.length === 0) {
        tab?.terminal.appendLine({
          timestamp: makeTimestamp(),
          text: "No BLE peripherals found by btleplug",
          kind: "system",
          rx_bytes_total: 0,
        });
      }
    }).catch((e) => console.error("debug_ble:", e));
  }
  if ((e.metaKey || e.ctrlKey) && e.key === "f") {
    e.preventDefault();
    if (tab) openSearch();
  }
  if (e.key === " " && !e.metaKey && !e.ctrlKey && !e.altKey) {
    const target = e.target as HTMLElement | null;
    const tag = target?.tagName;
    // Don't steal Space from text inputs.
    if (tag !== "INPUT" && tag !== "TEXTAREA" && !target?.isContentEditable) {
      e.preventDefault();
      tab?.terminal.togglePause();
    }
  }
});

// ---- Tooltip helper ----

function showTooltip(anchor: HTMLElement, text: string): void {
  const tip = document.createElement("div");
  tip.className = "tooltip-fade";
  tip.textContent = text;
  anchor.style.position = "relative";
  anchor.appendChild(tip);
  setTimeout(() => tip.classList.add("visible"), 10);
  setTimeout(() => {
    tip.classList.remove("visible");
    setTimeout(() => tip.remove(), 300);
  }, 1200);
}

// ---- Copy log ----

async function copyLog(): Promise<void> {
  const tab = activeTab();
  if (!tab) return;
  const ok = await tab.terminal.copyLog();
  if (ok) {
    showTooltip(copyBtn, "Copied!");
  }
}

// ---- Log export ----

async function exportLog(): Promise<void> {
  const tab = activeTab();
  if (!tab) return;

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
    const count = await invoke<number>("export_log", { tabId: tab.id, path, format });
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: `Exported ${count} entries to ${path}`,
      kind: "system",
      rx_bytes_total: 0,
    });
  } catch (e) {
    tab.terminal.appendLine({
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
  const tab = activeTab();
  if (autoLogActive) {
    try {
      const result = await invoke<[string, number] | null>("stop_auto_log");
      autoLogActive = false;
      autologBtn.classList.remove("active");
      autologBtn.classList.remove("active");
      if (result) {
        tab?.terminal.appendLine({
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
    const dir = await open({
      title: "Choose log directory",
      directory: true,
    });
    if (!dir) return;

    try {
      const path = await invoke<string>("start_auto_log", { directory: dir });
      autoLogActive = true;
      autologBtn.classList.add("active");
      autologBtn.classList.add("active");
      tab?.terminal.appendLine({
        timestamp: makeTimestamp(),
        text: `Auto-logging to ${path}`,
        kind: "system",
        rx_bytes_total: 0,
      });
    } catch (e) {
      tab?.terminal.appendLine({
        timestamp: makeTimestamp(),
        text: `Auto-log failed: ${e}`,
        kind: "system",
        rx_bytes_total: 0,
      });
    }
  }
}

autologBtn.addEventListener("click", toggleAutoLog);

invoke<string | null>("auto_log_status").then((path) => {
  if (path) {
    autoLogActive = true;
    autologBtn.classList.add("active");
    autologBtn.textContent = "Log \u25cf";
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

const exportBtn = document.getElementById("export-btn");

dtrBtn.addEventListener("click", async () => {
  const tab = activeTab();
  if (!tab) return;
  tab.dtrState = !tab.dtrState;
  dtrBtn.classList.toggle("active", tab.dtrState);
  try {
    await invoke("set_dtr", { tabId: tab.id, active: tab.dtrState });
  } catch (e) {
    console.error("DTR:", e);
  }
});

rtsBtn.addEventListener("click", async () => {
  const tab = activeTab();
  if (!tab) return;
  tab.rtsState = !tab.rtsState;
  rtsBtn.classList.toggle("active", tab.rtsState);
  try {
    await invoke("set_rts", { tabId: tab.id, active: tab.rtsState });
  } catch (e) {
    console.error("RTS:", e);
  }
});

breakBtn.addEventListener("click", async () => {
  const tab = activeTab();
  if (!tab) return;
  try {
    await invoke("send_break", { tabId: tab.id });
    tab.terminal.appendLine({
      timestamp: makeTimestamp(),
      text: "Break signal sent",
      kind: "system",
      rx_bytes_total: 0,
    });
  } catch (e) {
    console.error("Break:", e);
  }
});


refreshBtn.addEventListener("click", refreshDevices);
exportBtn?.addEventListener("click", exportLog);
copyBtn.addEventListener("click", copyLog);
clearBtn.addEventListener("click", () => activeTab()?.terminal.clear());
pauseBtn.addEventListener("click", () => activeTab()?.terminal.togglePause());
disconnectBtn.addEventListener("click", disconnect);
descriptorBtn.addEventListener("click", toggleDescriptor);

const chartBtn = document.getElementById("chart-btn")!;
chartBtn.addEventListener("click", () => {
  const tab = activeTab();
  if (!tab) return;
  const showing = tab.terminal.toggleChart();
  chartBtn.classList.toggle("active", showing);
});

// ---- Menu events ----

let showTimestamps = true;

(window as any).__toggleTimestamps = (visible: boolean) => {
  showTimestamps = visible;
  for (const tab of tabs.values()) {
    tab.terminal.setTimestampsVisible(visible);
  }
};

(window as any).__newTab = () => {
  if (tabs.size > 0) {
    showInlineDeviceSelector();
  } else {
    showPortSelect();
  }
};

(window as any).__mcpCloseTab = (tabId: string) => {
  closeTab(tabId);
};

(window as any).__mcpConnect = (type: string, path: string, baud: number, name: string) => {
  if (type === "serial") {
    connectToPort(path, name);
  } else if (type === "hid") {
    connectHid(path, name, 0, 0);
  } else if (type === "ble_nus") {
    connectBleNus(path, name);
  }
};

(window as any).__exportLog = () => {
  exportLog();
};

// ---- Init ----

refreshDevices();
startDeviceListPolling();
