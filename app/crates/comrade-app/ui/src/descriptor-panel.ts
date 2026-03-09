/** Descriptor info from the Rust backend — pre-formatted text. */
export interface HidDescriptorInfo {
  raw_hex: string;
  annotated: string;
}

/** Manages the HID descriptor side panel. */
export class DescriptorPanel {
  private panel: HTMLElement;
  private parsedEl: HTMLElement;
  private rawEl: HTMLElement;
  private tabs: NodeListOf<HTMLElement>;

  constructor() {
    this.panel = document.getElementById("descriptor-panel")!;
    this.parsedEl = document.getElementById("descriptor-parsed")!;
    this.rawEl = document.getElementById("descriptor-raw")!;
    this.tabs = this.panel.querySelectorAll(".panel-tab");

    // Tab switching.
    for (const tab of this.tabs) {
      tab.addEventListener("click", () => {
        for (const t of this.tabs) t.classList.remove("active");
        tab.classList.add("active");
        const which = tab.dataset.tab;
        this.parsedEl.classList.toggle("hidden", which !== "parsed");
        this.rawEl.classList.toggle("hidden", which !== "raw");
      });
    }

    // Close button.
    document
      .getElementById("descriptor-close-btn")!
      .addEventListener("click", () => {
        this.hide();
      });
  }

  show(info: HidDescriptorInfo): void {
    this.parsedEl.textContent = info.annotated;
    this.rawEl.textContent = info.raw_hex;
    this.panel.classList.remove("hidden");
  }

  hide(): void {
    this.panel.classList.add("hidden");
  }

  toggle(info: HidDescriptorInfo): void {
    if (this.panel.classList.contains("hidden")) {
      this.show(info);
    } else {
      this.hide();
    }
  }
}
