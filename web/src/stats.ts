// Everything on the result screen, computed from the accepted entries.

import type { Entry } from "./store";

export interface Bar {
  label: string;
  count: number;
}

const by = <T>(items: readonly T[], key: (item: T) => string): Map<string, number> => {
  const counts = new Map<string, number>();
  for (const item of items) counts.set(key(item), (counts.get(key(item)) ?? 0) + 1);
  return counts;
};

const descending = (counts: Map<string, number>): Bar[] =>
  [...counts]
    .map(([label, count]) => ({ label, count }))
    .sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));

export const OTHER = "Other";
export const UNCATEGORIZED = "Uncategorized";
/** Categories with fewer people than this are merged into "Other". */
const SLICE_MIN = 3;
/** Most named slices; the chart has eight colours and keeps one in hand. */
const SLICE_MAX = 7;

/** Main occupation category of each person, biggest first, then Other and Uncategorized. */
export function occupations(entries: readonly Entry[]): Bar[] {
  const counts = by(entries, (entry) => entry.categories[0] ?? UNCATEGORIZED);
  const uncategorized = counts.get(UNCATEGORIZED) ?? 0;
  counts.delete(UNCATEGORIZED);
  const ranked = descending(counts);
  const kept = ranked.filter((bar, position) => bar.count >= SLICE_MIN && position < SLICE_MAX);
  const other = ranked.slice(kept.length).reduce((sum, bar) => sum + bar.count, 0);
  if (other > 0) kept.push({ label: OTHER, count: other });
  if (uncategorized > 0) kept.push({ label: UNCATEGORIZED, count: uncategorized });
  return kept;
}

const FAME_BANDS: [string, number][] = [
  ["Under 10", 10],
  ["10–24", 25],
  ["25–49", 50],
  ["50–99", 100],
  ["100+", Infinity],
];

/** How many Wikipedia language editions have an article, in bands. */
export function fame(entries: readonly Entry[]): Bar[] {
  return FAME_BANDS.map(([label, below], band) => ({
    label,
    count: entries.filter(
      (entry) => entry.sitelinks < below && entry.sitelinks >= (FAME_BANDS[band - 1]?.[1] ?? 0),
    ).length,
  }));
}

export function median(values: readonly number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = sorted.length >> 1;
  return sorted.length % 2 ? sorted[middle]! : Math.round((sorted[middle - 1]! + sorted[middle]!) / 2);
}

const ordinal = (n: number): string => {
  const tens = n % 100;
  const suffix = tens >= 11 && tens <= 13 ? "th" : (["th", "st", "nd", "rd"][n % 10] ?? "th");
  return `${n}${suffix}`;
};

/** A year as the lookup server counts it: negative years are BC. */
export function formatYear(year: number): string {
  return year < 0 ? `${-year} BC` : String(year);
}

/**
 * A key for a birth year's period that sorts in time order: the decade from
 * 1900, the century's first year before that, and minus the century for BC.
 */
function period(year: number): number {
  if (year >= 1900) return Math.floor(year / 10) * 10;
  if (year >= 1) return Math.floor((year - 1) / 100) * 100 + 1;
  return -Math.ceil(Math.max(-year, 1) / 100) * 100;
}

function periodLabel(key: number): string {
  if (key >= 1900) return `${key}s`;
  return key > 0 ? `${ordinal((key - 1) / 100 + 1)} c.` : `${ordinal(-key / 100)} c. BC`;
}

export interface Births {
  /** Decades from 1900 without gaps; earlier centuries only where someone was born. */
  bars: Bar[];
  unknown: number;
}

export function births(entries: readonly Entry[]): Births {
  const years = entries.flatMap((entry) => (entry.birth === null ? [] : [entry.birth]));
  const counts = new Map<number, number>();
  for (const year of years) counts.set(period(year), (counts.get(period(year)) ?? 0) + 1);
  const decades = [...counts.keys()].filter((start) => start >= 1900);
  if (decades.length > 0) {
    for (let decade = Math.min(...decades); decade <= Math.max(...decades); decade += 10) {
      if (!counts.has(decade)) counts.set(decade, 0);
    }
  }
  const bars = [...counts]
    .sort((a, b) => a[0] - b[0])
    .map(([start, count]) => ({ label: periodLabel(start), count }));
  return { bars, unknown: entries.length - years.length };
}

/** People with a death recorded, and without. Wikidata does not record every death. */
export function deaths(entries: readonly Entry[]): { recorded: number; none: number } {
  const recorded = entries.filter((entry) => entry.death !== null).length;
  return { recorded, none: entries.length - recorded };
}

/** The ten most common countries. Someone with two countries counts under both. */
export function countries(entries: readonly Entry[]): Bar[] {
  const counts = new Map<string, number>();
  for (const entry of entries) {
    for (const country of new Set(entry.citizenships)) {
      counts.set(country, (counts.get(country) ?? 0) + 1);
    }
  }
  return descending(counts).slice(0, 10);
}

export interface Superlatives {
  mostFamous: Entry | null;
  mostObscure: Entry | null;
  earliestBorn: Entry | null;
  latestBorn: Entry | null;
  /** The longest wait between two accepted names, and the name that ended it. */
  longestGap: { ms: number; before: Entry; after: Entry } | null;
  /** The least time ten names in a row took, and the first and last of them. */
  fastestTen: { ms: number; first: Entry; last: Entry } | null;
}

const best = <T>(items: readonly T[], better: (a: T, b: T) => boolean): T | null =>
  items.reduce<T | null>((top, item) => (top === null || better(item, top) ? item : top), null);

export function superlatives(entries: readonly Entry[]): Superlatives {
  const born = entries.filter((entry) => entry.birth !== null);
  let longestGap: Superlatives["longestGap"] = null;
  let fastestTen: Superlatives["fastestTen"] = null;
  entries.forEach((entry, position) => {
    const previous = entries[position - 1];
    if (previous && (longestGap === null || entry.at - previous.at > longestGap.ms)) {
      longestGap = { ms: entry.at - previous.at, before: previous, after: entry };
    }
    // Ten names, timed from the name before the first of them (or the start).
    const first = entries[position - 9];
    if (first) {
      const ms = entry.at - (entries[position - 10]?.at ?? 0);
      if (fastestTen === null || ms < fastestTen.ms) fastestTen = { ms, first, last: entry };
    }
  });
  return {
    mostFamous: best(entries, (a, b) => a.sitelinks > b.sitelinks),
    mostObscure: best(entries, (a, b) => a.sitelinks < b.sitelinks),
    earliestBorn: best(born, (a, b) => a.birth! < b.birth!),
    latestBorn: best(born, (a, b) => a.birth! > b.birth!),
    longestGap,
    fastestTen,
  };
}

/** "8 seconds", "1 min 12 s". */
export function formatDuration(ms: number): string {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} second${seconds === 1 ? "" : "s"}`;
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}
