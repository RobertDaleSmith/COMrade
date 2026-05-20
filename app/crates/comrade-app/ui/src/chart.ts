import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";

const MAX_POINTS = 200;
const COLORS = [
  "#D72626", "#FF6A00", "#5B9BD5", "#3fb950", "#E066FF",
  "#FFD700", "#00CED1", "#FF69B4", "#7B68EE", "#F1E6CF",
];

export class SerialChart {
  private container: HTMLElement;
  private plot: uPlot | null = null;
  private data: number[][] = [[]]; // data[0] = x (index), data[1..] = series
  private seriesCount = 0;
  private pointIndex = 0;
  private visible = false;
  private labels: string[] = [];

  constructor() {
    this.container = document.createElement("div");
    this.container.className = "serial-chart hidden";
  }

  /** Mount into a parent element. */
  mount(parent: HTMLElement): void {
    parent.appendChild(this.container);
    // Auto-resize when container changes size.
    new ResizeObserver(() => this.resize()).observe(this.container);
  }

  /** Show the chart, hide terminal output. */
  show(): void {
    this.visible = true;
    this.container.classList.remove("hidden");
    // Delay resize to let layout settle.
    requestAnimationFrame(() => this.resize());
  }

  /** Hide the chart, show terminal output. */
  hide(): void {
    this.visible = false;
    this.container.classList.add("hidden");
  }

  /** Toggle visibility. Returns new state. */
  toggle(): boolean {
    if (this.visible) {
      this.hide();
    } else {
      this.show();
    }
    return this.visible;
  }

  /** Feed a line of text. If it's CSV numbers, add to chart. */
  feedLine(text: string): void {
    const values = parseCSV(text);
    if (!values || values.length === 0) return;

    // Initialize series on first data.
    if (this.seriesCount === 0 || values.length !== this.seriesCount) {
      this.initSeries(values.length, text);
    }

    this.pointIndex++;
    this.data[0].push(this.pointIndex);
    for (let i = 0; i < this.seriesCount; i++) {
      this.data[i + 1].push(values[i] ?? 0);
    }

    // Trim to max points.
    if (this.data[0].length > MAX_POINTS) {
      for (let i = 0; i < this.data.length; i++) {
        this.data[i] = this.data[i].slice(-MAX_POINTS);
      }
    }

    if (this.visible && this.plot) {
      this.plot.setData(this.data as any);
    }
  }

  /** Clear all data. */
  clear(): void {
    this.data = [[]];
    this.seriesCount = 0;
    this.pointIndex = 0;
    this.labels = [];
    if (this.plot) {
      this.plot.destroy();
      this.plot = null;
    }
  }

  /** Resize to fit container. */
  resize(): void {
    if (!this.plot || !this.visible) return;
    const rect = this.container.getBoundingClientRect();
    if (rect.width > 100 && rect.height > 100) {
      this.plot.setSize({ width: rect.width, height: Math.max(rect.height - 30, 100) });
    }
  }

  destroy(): void {
    if (this.plot) {
      this.plot.destroy();
      this.plot = null;
    }
    this.container.remove();
  }

  private initSeries(count: number, firstLine: string): void {
    this.seriesCount = count;
    this.data = [[]];
    this.pointIndex = 0;

    // Try to detect labels from the line (e.g. "lx=128,ly=64" or just numbers).
    this.labels = detectLabels(firstLine, count);

    for (let i = 0; i < count; i++) {
      this.data.push([]);
    }

    if (this.plot) {
      this.plot.destroy();
    }

    const series: uPlot.Series[] = [{}]; // x-axis
    for (let i = 0; i < count; i++) {
      series.push({
        label: this.labels[i] || `ch${i}`,
        stroke: COLORS[i % COLORS.length],
        width: 2,
      });
    }

    const rect = this.container.getBoundingClientRect();
    const opts: uPlot.Options = {
      width: Math.max(rect.width, 100),
      height: Math.max(rect.height - 30, 100), // leave room for legend
      series,
      scales: {
        x: { time: false },
      },
      axes: [
        { show: false },
        {
          stroke: "#6B6157",
          grid: { stroke: "#2F353A", width: 1 },
          ticks: { stroke: "#2F353A", width: 1 },
          font: "11px 'JetBrains Mono', monospace",
        },
      ],
      legend: {
        show: true,
      },
      cursor: {
        show: true,
      },
    };

    this.plot = new uPlot(opts, this.data as any, this.container);
  }
}

/** Parse a line as comma-separated numbers. Returns null if not CSV-numeric. */
function parseCSV(text: string): number[] | null {
  const trimmed = text.trim();
  if (!trimmed) return null;

  // Strip common prefixes like timestamps, brackets, labels.
  // Try to find the numeric CSV portion.
  let csv = trimmed;

  // If the line has key=value pairs, extract values.
  const kvMatch = trimmed.match(/[\w]+=(-?[\d.]+)/g);
  if (kvMatch && kvMatch.length >= 2) {
    return kvMatch.map(kv => {
      const val = kv.split("=")[1];
      return parseFloat(val);
    }).filter(n => !isNaN(n));
  }

  // Try splitting by common delimiters.
  for (const delim of [",", "\t", " "]) {
    const parts = csv.split(delim).map(s => s.trim()).filter(s => s.length > 0);
    const nums = parts.map(s => parseFloat(s));
    if (nums.length >= 2 && nums.every(n => !isNaN(n))) {
      return nums;
    }
  }

  return null;
}

/** Try to detect column labels from key=value patterns. */
function detectLabels(text: string, count: number): string[] {
  const kvMatch = text.match(/([\w]+)=(-?[\d.]+)/g);
  if (kvMatch && kvMatch.length === count) {
    return kvMatch.map(kv => kv.split("=")[0]);
  }
  return Array.from({ length: count }, (_, i) => `ch${i}`);
}
