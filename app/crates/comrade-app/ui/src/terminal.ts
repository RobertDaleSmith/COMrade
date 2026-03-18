/** A line received from the Rust backend. */
export interface SerialLine {
  timestamp: string;
  text: string;
  kind: "received" | "sent" | "system" | "mcp";
  rx_bytes_total: number;
}

/** A HID report received from the Rust backend. */
export interface HidReport {
  timestamp: string;
  data: number[];
  hex: string;
  ascii: string;
  report_id: number | null;
  report_count: number;
  rx_bytes_total: number;
  kind: "input" | "error";
}

/** Manages the terminal output display. */
export class TerminalUI {
  private output: HTMLElement;
  private statusPort: HTMLElement;
  private statusConfig: HTMLElement;
  private statusState: HTMLElement;
  private statusRx: HTMLElement;
  private autoScroll = true;
  private maxLines = 10000;
  private timestampsVisible = true;
  private hidReportCount = 0;
  private pendingFragment: DocumentFragment = document.createDocumentFragment();
  private flushScheduled = false;
  private lastRxLine: SerialLine | null = null;
  private lastHidReport: HidReport | null = null;
  private contextMenu: HTMLElement;
  private contextTarget: HTMLElement | null = null;
  private activityLed: HTMLElement;
  private activityTimer: ReturnType<typeof setTimeout> | null = null;

  // Timestamp formatting.
  private tsFormat: "time" | "elapsed" | "iso" = "time";
  private connectTime: number = Date.now();

  // Search state.
  private searchQuery = "";
  private searchMatches: HTMLElement[] = [];
  private currentMatchIndex = -1;
  private searchActive = false;

  constructor() {
    // Create a dedicated output container for this tab (hidden until activated).
    this.output = document.createElement("div");
    this.output.className = "output hidden";
    document.getElementById("terminal-body")!.appendChild(this.output);

    this.statusPort = document.getElementById("status-port")!;
    this.statusConfig = document.getElementById("status-config")!;
    this.statusState = document.getElementById("status-state")!;
    this.statusRx = document.getElementById("status-rx")!;
    this.activityLed = document.getElementById("activity-led")!;

    // Track scroll position for auto-scroll.
    this.output.addEventListener("scroll", () => {
      const el = this.output;
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
      this.autoScroll = atBottom;
    });

    // Block the default browser context menu globally (no Reload / Inspect).
    document.addEventListener("contextmenu", (e) => {
      e.preventDefault();
    });

    // Build context menu.
    this.contextMenu = document.createElement("div");
    this.contextMenu.className = "ctx-menu hidden";
    this.contextMenu.innerHTML =
      '<div class="ctx-item" data-action="copy">Copy</div>' +
      '<div class="ctx-item" data-action="clear-above">Clear Above</div>' +
      '<div class="ctx-item" data-action="clear-below">Clear Below</div>';
    document.body.appendChild(this.contextMenu);

    this.contextMenu.addEventListener("click", (e) => {
      const item = (e.target as HTMLElement).closest(".ctx-item") as HTMLElement | null;
      if (!item) return;
      const action = item.dataset.action;
      if (action === "copy") {
        const sel = window.getSelection();
        if (sel && sel.toString()) {
          navigator.clipboard.writeText(sel.toString());
        }
      } else if (this.contextTarget) {
        if (action === "clear-above") {
          while (this.contextTarget.previousElementSibling) {
            this.contextTarget.previousElementSibling.remove();
          }
        } else if (action === "clear-below") {
          while (this.contextTarget.nextElementSibling) {
            this.contextTarget.nextElementSibling.remove();
          }
        }
      }
      this.hideContextMenu();
    });

    this.output.addEventListener("contextmenu", (e) => {
      const line = (e.target as HTMLElement).closest(".line") as HTMLElement | null;
      if (!line) return;
      this.contextTarget = line;
      this.contextMenu.style.left = `${e.clientX}px`;
      this.contextMenu.style.top = `${e.clientY}px`;
      this.contextMenu.classList.remove("hidden");

      // Clamp to viewport.
      const rect = this.contextMenu.getBoundingClientRect();
      if (rect.right > window.innerWidth) {
        this.contextMenu.style.left = `${window.innerWidth - rect.width - 4}px`;
      }
      if (rect.bottom > window.innerHeight) {
        this.contextMenu.style.top = `${window.innerHeight - rect.height - 4}px`;
      }
    });

    document.addEventListener("click", () => this.hideContextMenu());
  }

  private hideContextMenu(): void {
    this.contextMenu.classList.add("hidden");
    this.contextTarget = null;
  }

  /** Append a line to the terminal output (batched). */
  appendLine(line: SerialLine): void {
    const div = document.createElement("div");
    div.className = `line ${line.kind}`;

    const ts = document.createElement("span");
    ts.className = "ts";
    ts.textContent = this.formatTimestamp(line.timestamp);

    const text = document.createElement("span");
    text.className = "text";
    text.textContent = line.text;

    div.appendChild(ts);
    div.appendChild(text);
    this.pendingFragment.appendChild(div);

    this.lastRxLine = line;
    this.scheduleFlush();
  }

  /** Append a HID report to the terminal output (batched). */
  appendHidReport(report: HidReport): void {
    this.hidReportCount = report.report_count;
    const div = document.createElement("div");
    div.className = `line hid-report ${report.kind}`;

    const ts = document.createElement("span");
    ts.className = "ts";
    ts.textContent = this.formatTimestamp(report.timestamp);

    const text = document.createElement("span");
    text.className = "text";

    if (report.kind === "error") {
      text.textContent = report.hex;
    } else {
      const idStr =
        report.report_id !== null
          ? `[${report.report_id.toString(16).padStart(2, "0").toUpperCase()}] `
          : "";
      text.textContent = `${idStr}${report.hex}  |${report.ascii}|`;
    }

    div.appendChild(ts);
    div.appendChild(text);
    this.pendingFragment.appendChild(div);

    this.lastHidReport = report;
    this.scheduleFlush();
  }

  /** Schedule a batched DOM flush on the next animation frame. */
  private scheduleFlush(): void {
    if (this.flushScheduled) return;
    this.flushScheduled = true;
    requestAnimationFrame(() => this.flushPending());
  }

  /** Flush all pending lines to the DOM in one batch. */
  private flushPending(): void {
    this.flushScheduled = false;

    // Append all pending elements at once.
    this.output.appendChild(this.pendingFragment);
    this.pendingFragment = document.createDocumentFragment();

    // Apply search to newly added lines.
    if (this.searchActive) {
      const lines = this.output.querySelectorAll(".line:not([data-searched])");
      for (const div of Array.from(lines)) {
        (div as HTMLElement).setAttribute("data-searched", "1");
        this.applySearchToLine(div as HTMLElement);
      }
    }

    // Update status bar from latest data.
    if (this.lastHidReport) {
      const r = this.lastHidReport;
      this.statusRx.textContent = `RX: ${this.formatBytes(r.rx_bytes_total)} (${r.report_count} reports)`;
      this.pulseActivity("rx");
      this.lastHidReport = null;
    }
    if (this.lastRxLine) {
      const l = this.lastRxLine;
      this.statusRx.textContent = `RX: ${this.formatBytes(l.rx_bytes_total)}`;
      if (l.kind === "received") this.pulseActivity("rx");
      else if (l.kind === "sent" || l.kind === "mcp") this.pulseActivity("tx");
      this.lastRxLine = null;
    }

    this.trimAndScroll();
  }

  /** Clear all output lines. */
  clear(): void {
    this.output.innerHTML = "";
    this.searchMatches = [];
    this.currentMatchIndex = -1;
    this.updateSearchCount();
  }

  /** Set connection info in the status bar for serial. */
  setConnected(port: string, baud: number): void {
    this.statusPort.textContent = port;
    this.statusConfig.textContent = `${baud} 8N1`;
    this.statusState.textContent = "CONNECTED";
    this.statusState.className = "connected";
    this.resetConnectTime();
  }

  /** Set connection info in the status bar for BLE. */
  setBleConnected(deviceName: string, profile: string): void {
    this.statusPort.textContent = deviceName;
    this.statusConfig.textContent = `BLE ${profile}`;
    this.statusState.textContent = "CONNECTED";
    this.statusState.className = "connected";
  }

  /** Set connection info in the status bar for HID. */
  setHidConnected(deviceName: string): void {
    this.statusPort.textContent = deviceName;
    this.statusConfig.textContent = "HID";
    this.statusState.textContent = "CONNECTED";
    this.statusState.className = "connected";
  }

  /** Set connecting state. */
  setConnecting(port: string): void {
    this.statusPort.textContent = port;
    this.statusConfig.textContent = "";
    this.statusState.textContent = "CONNECTING";
    this.statusState.className = "connecting";
  }

  /** Set disconnected state. */
  setDisconnected(reason: string): void {
    this.statusState.textContent = `DISCONNECTED: ${reason}`;
    this.statusState.className = "disconnected";
  }

  /** Set reconnecting state. */
  setReconnecting(): void {
    this.statusState.textContent = "RECONNECTING...";
    this.statusState.className = "connecting";
  }

  /** Copy all log lines to clipboard. Returns true on success. */
  async copyLog(): Promise<boolean> {
    const lines: string[] = [];
    for (const div of this.output.children) {
      const ts = div.querySelector(".ts")?.textContent ?? "";
      const text = div.querySelector(".text")?.textContent ?? "";
      lines.push(`${ts}  ${text}`);
    }
    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      return true;
    } catch {
      return false;
    }
  }

  /** Activate search mode. Caller is responsible for showing the search bar. */
  startSearch(): void {
    this.searchActive = true;
  }

  /** Deactivate search mode and clear all highlights/dimming. */
  endSearch(): void {
    this.searchActive = false;
    this.searchQuery = "";
    this.searchMatches = [];
    this.currentMatchIndex = -1;

    for (const div of Array.from(this.output.querySelectorAll(".line"))) {
      this.restoreLine(div as HTMLElement);
    }
  }

  /** Update the search query and re-highlight all lines. */
  updateSearch(query: string): void {
    this.searchQuery = query;
    this.searchMatches = [];
    this.currentMatchIndex = -1;

    const lines = this.output.querySelectorAll(".line");
    if (!query) {
      for (const div of Array.from(lines)) {
        this.restoreLine(div as HTMLElement);
      }
      this.updateSearchCount();
      return;
    }

    for (const div of Array.from(lines)) {
      this.applySearchToLine(div as HTMLElement);
    }

    if (this.searchMatches.length > 0) {
      this.currentMatchIndex = 0;
      this.highlightCurrentMatch();
    }
    this.updateSearchCount();
  }

  /** Navigate to the next match. */
  nextMatch(): void {
    if (this.searchMatches.length === 0) return;
    this.clearCurrentMark();
    this.currentMatchIndex = (this.currentMatchIndex + 1) % this.searchMatches.length;
    this.highlightCurrentMatch();
    this.updateSearchCount();
  }

  /** Navigate to the previous match. */
  prevMatch(): void {
    if (this.searchMatches.length === 0) return;
    this.clearCurrentMark();
    this.currentMatchIndex = (this.currentMatchIndex - 1 + this.searchMatches.length) % this.searchMatches.length;
    this.highlightCurrentMatch();
    this.updateSearchCount();
  }

  /** Apply search highlighting/dimming to a single line div. */
  private applySearchToLine(div: HTMLElement): void {
    const textEl = div.querySelector(".text") as HTMLElement | null;
    if (!textEl) return;

    // Get plain text — use stored original if available, else current textContent.
    const plain = textEl.dataset.plainText ?? textEl.textContent ?? "";
    textEl.dataset.plainText = plain;

    if (!this.searchQuery) {
      textEl.textContent = plain;
      div.classList.remove("search-dim");
      return;
    }

    const highlighted = this.highlightText(plain, this.searchQuery);
    if (highlighted !== null) {
      textEl.innerHTML = highlighted;
      div.classList.remove("search-dim");
      this.searchMatches.push(div);
      this.updateSearchCount();
    } else {
      textEl.textContent = plain;
      div.classList.add("search-dim");
    }
  }

  /** Restore a line to plain text and remove dimming. */
  private restoreLine(div: HTMLElement): void {
    div.classList.remove("search-dim");
    const textEl = div.querySelector(".text") as HTMLElement | null;
    if (!textEl) return;
    const plain = textEl.dataset.plainText ?? textEl.textContent ?? "";
    textEl.textContent = plain;
    delete textEl.dataset.plainText;
  }

  /** HTML-escape text and wrap case-insensitive matches in <mark>. Returns null if no match. */
  private highlightText(text: string, query: string): string | null {
    const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const re = new RegExp(escaped, "gi");
    let match: RegExpExecArray | null;
    const parts: string[] = [];
    let last = 0;
    let found = false;

    while ((match = re.exec(text)) !== null) {
      found = true;
      parts.push(this.escapeHtml(text.slice(last, match.index)));
      parts.push(`<mark>${this.escapeHtml(match[0])}</mark>`);
      last = match.index + match[0].length;
    }

    if (!found) return null;
    parts.push(this.escapeHtml(text.slice(last)));
    return parts.join("");
  }

  /** Escape HTML special characters. */
  private escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
  }

  /** Add .current class to the first <mark> in the current match line and scroll to it. */
  private highlightCurrentMatch(): void {
    if (this.currentMatchIndex < 0 || this.currentMatchIndex >= this.searchMatches.length) return;
    const div = this.searchMatches[this.currentMatchIndex];
    const mark = div.querySelector("mark");
    if (mark) {
      mark.classList.add("current");
      mark.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }

  /** Remove .current class from the previous match's mark. */
  private clearCurrentMark(): void {
    if (this.currentMatchIndex < 0 || this.currentMatchIndex >= this.searchMatches.length) return;
    const div = this.searchMatches[this.currentMatchIndex];
    const mark = div.querySelector("mark.current");
    if (mark) mark.classList.remove("current");
  }

  /** Update the search count display element. */
  private updateSearchCount(): void {
    const el = document.getElementById("search-count");
    if (!el) return;
    if (!this.searchActive || !this.searchQuery) {
      el.textContent = "";
      return;
    }
    if (this.searchMatches.length === 0) {
      el.textContent = "No matches";
    } else {
      el.textContent = `${this.currentMatchIndex + 1} of ${this.searchMatches.length}`;
    }
  }

  /** Scroll to the bottom of the output. */
  scrollToBottom(): void {
    this.autoScroll = true;
    this.output.scrollTop = this.output.scrollHeight;
  }

  private trimAndScroll(): void {
    while (this.output.children.length > this.maxLines) {
      const removed = this.output.firstChild! as HTMLElement;
      if (this.searchActive) {
        const idx = this.searchMatches.indexOf(removed);
        if (idx !== -1) {
          this.searchMatches.splice(idx, 1);
          if (this.currentMatchIndex >= idx) {
            this.currentMatchIndex = Math.max(0, this.currentMatchIndex - 1);
          }
          this.updateSearchCount();
        }
      }
      this.output.removeChild(removed);
    }
    if (this.autoScroll) {
      this.output.scrollTop = this.output.scrollHeight;
    }
  }

  /** Format a timestamp string for display based on current format. */
  private formatTimestamp(ts: string): string {
    if (this.tsFormat === "time") return ts;
    if (this.tsFormat === "elapsed") {
      // Parse HH:MM:SS.mmm and compute elapsed from connect time.
      const elapsed = ((Date.now() - this.connectTime) / 1000).toFixed(3);
      return `+${elapsed}s`;
    }
    // ISO: prepend today's date.
    const today = new Date().toISOString().slice(0, 10);
    return `${today}T${ts}`;
  }

  /** Cycle through timestamp formats: time → elapsed → ISO → time. */
  cycleTimestampFormat(): string {
    if (this.tsFormat === "time") this.tsFormat = "elapsed";
    else if (this.tsFormat === "elapsed") this.tsFormat = "iso";
    else this.tsFormat = "time";
    return this.tsFormat;
  }

  /** Reset the connect time for elapsed timestamps. */
  resetConnectTime(): void {
    this.connectTime = Date.now();
  }

  /** Pulse the activity LED for RX or TX. */
  pulseActivity(direction: "rx" | "tx"): void {
    this.activityLed.classList.remove("rx", "tx");
    this.activityLed.classList.add(direction);
    if (this.activityTimer) clearTimeout(this.activityTimer);
    this.activityTimer = setTimeout(() => {
      this.activityLed.classList.remove("rx", "tx");
      this.activityTimer = null;
    }, 150);
  }

  /** Set whether timestamps are visible. */
  setTimestampsVisible(visible: boolean): void {
    this.timestampsVisible = visible;
    this.output.classList.toggle("hide-timestamps", !visible);
  }

  /** Show this tab's output container. */
  show(): void {
    this.output.classList.remove("hidden");
  }

  /** Hide this tab's output container. */
  hide(): void {
    this.output.classList.add("hidden");
  }

  /** Remove this tab's output container from the DOM. */
  destroy(): void {
    this.output.remove();
    this.contextMenu.remove();
  }

  private formatBytes(n: number): string {
    if (n < 1024) return `${n}`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K`;
    return `${(n / (1024 * 1024)).toFixed(1)}M`;
  }
}
