// The result screen's charts, drawn as inline SVG at the width they are given.
// Marks carry colour; text stays in the page's ink.

import { h, s } from "./dom";
import { OTHER, UNCATEGORIZED, type Bar } from "./stats";

const FONT = 12;
/** Bars are never thicker than this, however much room there is. */
const MAX_BAR = 24;

/** A rectangle rounded only at its data end (top for columns, right for bars). */
function roundedBar(x: number, y: number, w: number, hgt: number, end: "top" | "right"): string {
  const r = Math.min(4, w / 2, hgt / 2);
  if (end === "top") {
    return `M${x},${y + hgt}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + hgt}Z`;
  }
  return `M${x},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + hgt - r}Q${x + w},${y + hgt} ${x + w - r},${y + hgt}H${x}Z`;
}

const tip = (bar: Bar) => `${bar.label}: ${bar.count}`;

/** A histogram: one column per bar, in the order given. */
export function columnChart(bars: readonly Bar[], width: number): SVGElement {
  const plot = 132;
  const top = 18;
  const height = top + plot + 22;
  const band = width / Math.max(bars.length, 1);
  const thick = Math.max(4, Math.min(MAX_BAR, band - 4));
  const max = Math.max(1, ...bars.map((bar) => bar.count));
  // Value labels on every column only when there are few; otherwise the tallest.
  const labelAll = bars.length <= 8;
  // Axis labels thin out when the bands are narrower than the text.
  const longest = Math.max(...bars.map((bar) => bar.label.length), 1);
  const every = Math.max(1, Math.ceil((longest * FONT * 0.62 + 6) / band));
  const svg = s("svg", { width, height, viewBox: `0 0 ${width} ${height}`, "aria-hidden": "true" });
  bars.forEach((bar, position) => {
    const centre = band * (position + 0.5);
    const tall = bar.count === 0 ? 0 : Math.max(2, (bar.count / max) * plot);
    const group = s("g", { class: "mark", "data-tip": tip(bar) });
    if (tall > 0) {
      group.append(
        s("path", { class: "bar", d: roundedBar(centre - thick / 2, top + plot - tall, thick, tall, "top") }),
      );
    }
    if (bar.count > 0 && (labelAll || bar.count === max)) {
      group.append(
        s("text", { class: "value", x: centre, y: top + plot - tall - 5, "text-anchor": "middle" }, String(bar.count)),
      );
    }
    if (position % every === 0) {
      group.append(
        s("text", { class: "axis", x: centre, y: top + plot + 16, "text-anchor": "middle" }, bar.label),
      );
    }
    // The whole band answers to the pointer, not only the column.
    group.append(s("rect", { class: "hit", x: band * position, y: 0, width: band, height }));
    svg.append(group);
  });
  svg.append(s("line", { class: "baseline", x1: 0, x2: width, y1: top + plot + 0.5, y2: top + plot + 0.5 }));
  return svg;
}

/** Horizontal bars with their names on the left, in the order given. */
export function barChart(bars: readonly Bar[], width: number): SVGElement {
  const row = 26;
  const thick = 14;
  const labelWidth = Math.min(168, Math.round(width * 0.42));
  const valueWidth = 30;
  const room = Math.max(10, width - labelWidth - valueWidth);
  const max = Math.max(1, ...bars.map((bar) => bar.count));
  const fits = Math.max(4, Math.floor((labelWidth - 10) / (FONT * 0.58)));
  const height = bars.length * row;
  const svg = s("svg", { width, height, viewBox: `0 0 ${width} ${height}`, "aria-hidden": "true" });
  bars.forEach((bar, position) => {
    const middle = row * position + row / 2;
    const long = Math.max(2, (bar.count / max) * room);
    const name = bar.label.length > fits ? `${bar.label.slice(0, fits - 1).trimEnd()}…` : bar.label;
    svg.append(
      s(
        "g",
        { class: "mark", "data-tip": tip(bar) },
        s("text", { class: "name", x: labelWidth - 10, y: middle + 4, "text-anchor": "end" }, name),
        s("path", { class: "bar", d: roundedBar(labelWidth, middle - thick / 2, long, thick, "right") }),
        s("text", { class: "value", x: labelWidth + long + 6, y: middle + 4 }, String(bar.count)),
        s("rect", { class: "hit", x: 0, y: row * position, width, height: row }),
      ),
    );
  });
  svg.append(s("line", { class: "baseline", x1: labelWidth - 0.5, x2: labelWidth - 0.5, y1: 0, y2: height }));
  return svg;
}

/** The CSS colour of a slice: a fixed slot by position, grey for the catch-alls. */
export function sliceColor(bar: Bar, position: number): string {
  if (bar.label === OTHER) return "var(--series-other)";
  if (bar.label === UNCATEGORIZED) return "var(--series-none)";
  return `var(--series-${position + 1})`;
}

/** A pie of the parts of a whole, clockwise from twelve o'clock in the order given. */
export function pieChart(bars: readonly Bar[], size: number): SVGElement {
  const radius = size / 2;
  const total = bars.reduce((sum, bar) => sum + bar.count, 0);
  const svg = s("svg", { width: size, height: size, viewBox: `0 0 ${size} ${size}`, "aria-hidden": "true" });
  const point = (share: number) => {
    const angle = share * 2 * Math.PI - Math.PI / 2;
    return `${radius + radius * Math.cos(angle)},${radius + radius * Math.sin(angle)}`;
  };
  let before = 0;
  bars.forEach((bar, position) => {
    if (bar.count === 0) return;
    const from = before / total;
    before += bar.count;
    const to = before / total;
    const share = `${tip(bar)} (${Math.round((bar.count / total) * 100)}%)`;
    const attrs = { class: "slice", fill: sliceColor(bar, position), "data-tip": share };
    svg.append(
      bar.count === total
        ? s("circle", { ...attrs, cx: radius, cy: radius, r: radius })
        : s("path", {
            ...attrs,
            d: `M${radius},${radius}L${point(from)}A${radius},${radius} 0 ${to - from > 0.5 ? 1 : 0} 1 ${point(to)}Z`,
          }),
    );
  });
  return svg;
}

/** The numbers behind a chart, for screen readers. */
export function numbersList(bars: readonly Bar[]): HTMLElement {
  return h("ul", { class: "sr-only" }, ...bars.map((bar) => h("li", {}, tip(bar))));
}

/** One floating label that follows the pointer over any element with `data-tip`. */
export function installTooltip(): void {
  const label = h("div", { class: "tip", hidden: true, "aria-hidden": "true" });
  document.body.append(label);
  const hide = () => {
    label.hidden = true;
  };
  document.addEventListener("pointermove", (event) => {
    const target = (event.target as Element | null)?.closest?.("[data-tip]");
    if (!target) return hide();
    label.textContent = target.getAttribute("data-tip");
    label.hidden = false;
    const { offsetWidth: w, offsetHeight: tall } = label;
    const x = Math.min(Math.max(8, event.clientX - w / 2), window.innerWidth - w - 8);
    const above = event.clientY - tall - 12;
    label.style.left = `${x}px`;
    label.style.top = `${above < 8 ? event.clientY + 16 : above}px`;
  });
  document.addEventListener("pointerleave", hide);
  document.addEventListener("scroll", hide, { passive: true });
}
