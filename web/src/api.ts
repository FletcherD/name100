// The two calls the game makes to the lookup server. See name100.md for the API.

// Unless told otherwise, the server is on port 8080 of whatever host served
// the page, so the game works from another machine on the network as well as
// on localhost.
const sameHost =
  typeof location === "undefined" ? "http://127.0.0.1:8080" : `http://${location.hostname}:8080`;

export const API_BASE: string = (
  (import.meta.env.VITE_API_BASE as string | undefined) ?? sameHost
).replace(/\/+$/, "");

/** A lookup that takes longer than this counts as failed. */
export const LOOKUP_TIMEOUT_MS = 5000;

export interface Match {
  qid: string;
  name: string;
  description: string;
  birth_year: number | null;
  death_year: number | null;
  sitelinks: number;
  wikipedia_url: string | null;
  categories: string[];
  citizenships: string[];
  genders: string[];
}

export type Reason =
  | "not_a_name"
  | "not_woman"
  | "fictional"
  | "not_in_category"
  | "ambiguous"
  | "not_found";

export interface LookupResponse {
  input: string;
  verdict: "accepted" | "rejected";
  reason: Reason | null;
  reason_detail: string | null;
  corrected_from: string | null;
  matched_name: string | null;
  matched_as: "name" | "alias" | null;
  match: Match | null;
}

export interface Category {
  /** `women`, `men`, or `<kind>:<QID>`. */
  id: string;
  label: string;
  kind: string;
  people: number;
  women: number;
}

export type LookupOutcome =
  | { kind: "answer"; response: LookupResponse }
  /** The server refused the text outright (HTTP 400). */
  | { kind: "not_a_name" }
  /** Network error, timeout, rate limit or server error. */
  | { kind: "failed" };

/** The `category` parameter for a game; null for the original women game. */
export function categoryParam(ids: readonly string[]): string | null {
  if (ids.length === 0 || (ids.length === 1 && ids[0] === "women")) return null;
  return ids.join(",");
}

export async function lookup(name: string, categoryIds: readonly string[]): Promise<LookupOutcome> {
  const params = new URLSearchParams({ q: name });
  const category = categoryParam(categoryIds);
  if (category) params.set("category", category);
  try {
    const response = await fetch(`${API_BASE}/v1/lookup?${params}`, {
      signal: AbortSignal.timeout(LOOKUP_TIMEOUT_MS),
    });
    if (response.status === 400) return { kind: "not_a_name" };
    if (!response.ok) return { kind: "failed" };
    return { kind: "answer", response: (await response.json()) as LookupResponse };
  } catch {
    return { kind: "failed" };
  }
}

async function categories(params: URLSearchParams, signal?: AbortSignal): Promise<Category[]> {
  const response = await fetch(`${API_BASE}/v1/categories?${params}`, {
    signal: signal ?? AbortSignal.timeout(LOOKUP_TIMEOUT_MS),
  });
  if (!response.ok) throw new Error(`categories: HTTP ${response.status}`);
  const body = (await response.json()) as { categories: Category[] };
  return body.categories;
}

/** Playable categories matching the text, best first. Empty text lists the biggest. */
export function searchCategories(text: string, signal?: AbortSignal): Promise<Category[]> {
  return categories(new URLSearchParams({ q: text, limit: "10" }), signal);
}

/** The categories with these ids, in the order asked. Unknown ids are left out. */
export function categoriesById(ids: readonly string[]): Promise<Category[]> {
  return categories(new URLSearchParams({ ids: ids.join(",") }));
}
