// The game's state and its copy in local storage. Nothing here touches the page.

import type { Category, LookupResponse } from "./api";

export const TARGET = 100;

/** One accepted name, with everything the result screen needs. */
export interface Entry {
  qid: string;
  name: string;
  description: string;
  /** The alias the guess matched, when it was not the main name. */
  alias: string | null;
  corrected: boolean;
  /** Play time, in ms, when the name was accepted. */
  at: number;
  url: string | null;
  sitelinks: number;
  birth: number | null;
  death: number | null;
  categories: string[];
  citizenships: string[];
}

export interface Game {
  categories: Category[];
  /** In the order named. */
  entries: Entry[];
  /** Play time accumulated so far, in ms. Closing the tab pauses it. */
  elapsedMs: number;
  /** The clock has started (first guess sent) and the game is not over. */
  running: boolean;
  finished: boolean;
  /** Guesses that did not count, duplicates included. */
  rejected: number;
}

export const WOMEN: Category = { id: "women", label: "women", kind: "gender", people: 0, women: 0 };

export function newGame(categories: Category[]): Game {
  return { categories, entries: [], elapsedMs: 0, running: false, finished: false, rejected: 0 };
}

export function entryFrom(response: LookupResponse, at: number): Entry | null {
  const match = response.match;
  if (!match) return null;
  return {
    qid: match.qid,
    name: match.name,
    description: match.description,
    alias: response.matched_as === "alias" ? response.matched_name : null,
    corrected: response.corrected_from !== null,
    at,
    url: match.wikipedia_url,
    sitelinks: match.sitelinks,
    birth: match.birth_year,
    death: match.death_year,
    categories: match.categories ?? [],
    citizenships: match.citizenships ?? [],
  };
}

/** Identifies a game for best times and links: `women`, `occupation:Q1,women`. */
export function gameKey(categories: readonly Category[]): string {
  return categories
    .map((c) => c.id)
    .sort()
    .join(",");
}

const GAME_KEY = "name100.game.v1";
const BEST_KEY = "name100.best.v1";

// Local storage can be missing or full (private windows); the game works without it.
function read<T>(key: string): T | null {
  try {
    const text = localStorage.getItem(key);
    return text === null ? null : (JSON.parse(text) as T);
  } catch {
    return null;
  }
}

function write(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Nothing to do: the game goes on unsaved.
  }
}

export function loadGame(): Game | null {
  const game = read<Game>(GAME_KEY);
  const sound =
    game !== null &&
    Array.isArray(game.categories) &&
    game.categories.length > 0 &&
    Array.isArray(game.entries) &&
    typeof game.elapsedMs === "number";
  return sound ? game : null;
}

export function saveGame(game: Game): void {
  write(GAME_KEY, game);
}

/** The best finished time for 100 in this game, in ms. */
export function bestTime(categories: readonly Category[]): number | null {
  return read<Record<string, number>>(BEST_KEY)?.[gameKey(categories)] ?? null;
}

/** Record a finished time for 100; true when it is a new best. */
export function recordTime(categories: readonly Category[], ms: number): boolean {
  const all = read<Record<string, number>>(BEST_KEY) ?? {};
  const key = gameKey(categories);
  const previous = all[key];
  if (previous !== undefined && previous <= ms) return false;
  all[key] = ms;
  write(BEST_KEY, all);
  return true;
}

/** "0:07", "12:34", "1:02:03". */
export function formatTime(ms: number): string {
  const total = Math.floor(ms / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
    : `${minutes}:${seconds}`;
}
