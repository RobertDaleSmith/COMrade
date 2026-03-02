/** A line received from the Rust backend. */
export interface SerialLine {
  timestamp: string;
  text: string;
  kind: "received" | "sent" | "system";
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
  private hidReportCount = 0;
  private contextMenu: HTMLElement;
  private contextTarget: HTMLElement | null = null;

  constructor() {
    this.output = document.getElementById("output")!;
    this.statusPort = document.getElementById("status-port")!;
    this.statusConfig = document.getElementById("status-config")!;
    this.statusState = document.getElementById("status-state")!;
    this.statusRx = document.getElementById("status-rx")!;

    // Track scroll position for auto-scroll.
    this.output.addEventListener("scroll", () => {
      const el = this.output;
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
      this.autoScroll = atBottom;
    });

    // Build context menu.
    this.contextMenu = document.createElement("div");
    this.contextMenu.className = "ctx-menu hidden";
    this.contextMenu.innerHTML =
      '<div class="ctx-item" data-action="clear-above">Clear Above</div>' +
      '<div class="ctx-item" data-action="clear-below">Clear Below</div>';
    document.body.appendChild(this.contextMenu);

    this.contextMenu.addEventListener("click", (e) => {
      const item = (e.target as HTMLElement).closest(".ctx-item") as HTMLElement | null;
      if (!item || !this.contextTarget) return;
      const action = item.dataset.action;
      if (action === "clear-above") {
        while (this.contextTarget.previousElementSibling) {
          this.contextTarget.previousElementSibling.remove();
        }
      } else if (action === "clear-below") {
        while (this.contextTarget.nextElementSibling) {
          this.contextTarget.nextElementSibling.remove();
        }
      }
      this.hideContextMenu();
    });

    this.output.addEventListener("contextmenu", (e) => {
      const line = (e.target as HTMLElement).closest(".line") as HTMLElement | null;
      if (!line) return;
      e.preventDefault();
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
    document.addEventListener("contextmenu", (e) => {
      if (!(e.target as HTMLElement).closest("#output")) {
        this.hideContextMenu();
      }
    });
  }

  private hideContextMenu(): void {
    this.contextMenu.classList.add("hidden");
    this.contextTarget = null;
  }

  /** Append a line to the terminal output. */
  appendLine(line: SerialLine): void {
    const div = document.createElement("div");
    div.className = `line ${line.kind}`;

    const ts = document.createElement("span");
    ts.className = "ts";
    ts.textContent = line.timestamp;

    const text = document.createElement("span");
    text.className = "text";
    text.textContent = line.text;

    div.appendChild(ts);
    div.appendChild(text);
    this.output.appendChild(div);

    // Update RX counter.
    this.statusRx.textContent = `RX: ${this.formatBytes(line.rx_bytes_total)}`;

    this.trimAndScroll();
  }

  /** Append a HID report to the terminal output. */
  appendHidReport(report: HidReport): void {
    this.hidReportCount = report.report_count;
    const div = document.createElement("div");
    div.className = `line hid-report ${report.kind}`;

    const ts = document.createElement("span");
    ts.className = "ts";
    ts.textContent = report.timestamp;

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
    this.output.appendChild(div);

    // Update RX counter with report count.
    this.statusRx.textContent = `RX: ${this.formatBytes(report.rx_bytes_total)} (${report.report_count} reports)`;

    this.trimAndScroll();
  }

  /** Clear all output lines. */
  clear(): void {
    this.output.innerHTML = "";
  }

  /** Set connection info in the status bar for serial. */
  setConnected(port: string, baud: number): void {
    this.statusPort.textContent = port;
    this.statusConfig.textContent = `${baud} 8N1`;
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

  /** Scroll to the bottom of the output. */
  scrollToBottom(): void {
    this.autoScroll = true;
    this.output.scrollTop = this.output.scrollHeight;
  }

  private trimAndScroll(): void {
    while (this.output.children.length > this.maxLines) {
      this.output.removeChild(this.output.firstChild!);
    }
    if (this.autoScroll) {
      this.output.scrollTop = this.output.scrollHeight;
    }
  }

  private formatBytes(n: number): string {
    if (n < 1024) return `${n}`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)}K`;
    return `${(n / (1024 * 1024)).toFixed(1)}M`;
  }
}
