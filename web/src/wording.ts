// How a game is put into words: "Name 100 women who are scientists".
// The wording depends on each category's kind.

import type { Category } from "./api";

/** A piece of the sentence: plain text, or the clickable name of category `term`. */
export type Part = string | { term: number; text: string };

const IRREGULAR: [RegExp, string][] = [
  [/(wo)?man$/i, "$1men"],
  [/person$/i, "people"],
  [/child$/i, "children"],
  [/wife$/i, "wives"],
];

export function pluralWord(word: string): string {
  if (/human$/i.test(word)) return `${word}s`;
  for (const [pattern, replacement] of IRREGULAR) {
    if (pattern.test(word)) return word.replace(pattern, replacement);
  }
  if (/(ss|x|z|ch|sh)$/i.test(word)) return `${word}es`;
  if (/s$/i.test(word)) return word;
  if (/[^aeiou]y$/i.test(word)) return `${word.slice(0, -1)}ies`;
  return `${word}s`;
}

/** "member of parliament" → "members of parliament"; "film actor" → "film actors". */
export function pluralPhrase(label: string): string {
  const cut = label.search(/ (of|in|for|at|to|on) /);
  const head = cut === -1 ? label : label.slice(0, cut);
  const tail = cut === -1 ? "" : label.slice(cut);
  const space = head.lastIndexOf(" ");
  return head.slice(0, space + 1) + pluralWord(head.slice(space + 1)) + tail;
}

function article(label: string): string {
  return /^(the|a|an) /i.test(label) ? "" : "the ";
}

export function countryArticle(label: string): string {
  if (/^the /i.test(label)) return "";
  const named =
    /^(United |Kingdom of |Republic of |People's Republic|Federal Republic|Federated States)/.test(label) ||
    /^(Netherlands|Philippines|Bahamas|Gambia|Maldives|Comoros|Seychelles)$/.test(label) ||
    /(Republic|Union|Empire|Kingdom|Islands|States|Emirates|Federation|Confederation|Commonwealth)$/.test(label);
  return named ? "the " : "";
}

interface Phrase {
  /** Words before the category's name. */
  lead: string;
  /** The category's name as shown (and clicked). */
  name: string;
}

const isSex = (c: Category) => c.id === "women" || c.id === "men";

/** The category as the noun of the sentence ("actors"), when it can be one. */
function asNoun(c: Category): Phrase | null {
  if (isSex(c)) return { lead: "", name: c.id };
  switch (c.kind) {
    case "occupation":
    case "title":
    case "position":
      return { lead: "", name: pluralPhrase(c.label) };
    case "award":
      return { lead: `winners of ${article(c.label)}`, name: c.label };
    default:
      return null;
  }
}

/** The category as a clause after the noun ("from France", "who play guitar"). */
function asClause(c: Category): Phrase {
  if (isSex(c)) return { lead: "who are ", name: c.id };
  switch (c.kind) {
    case "occupation":
    case "title":
    case "position":
      return { lead: "who are ", name: pluralPhrase(c.label) };
    case "award":
      return { lead: `who won ${article(c.label)}`, name: c.label };
    case "citizenship":
      return { lead: `from ${countryArticle(c.label)}`, name: c.label };
    case "sport":
      return { lead: "in ", name: c.label };
    case "instrument":
      return { lead: "who play ", name: c.label };
    case "event":
      return { lead: `who took part in ${article(c.label)}`, name: c.label };
    case "genre":
      return { lead: "known for ", name: c.label };
    case "field":
      return { lead: "who work in ", name: c.label };
    case "gender":
      return { lead: "recorded as ", name: c.label };
    default:
      return { lead: "in ", name: c.label };
  }
}

/** What is being named, without "Name 100": "women who are scientists". */
export function gameParts(categories: readonly Category[]): Part[] {
  const indexed = categories.map((category, term) => ({ category, term }));
  // The noun is women or men when present, else the first category that can be one.
  const noun =
    indexed.find(({ category }) => isSex(category)) ??
    indexed.find(({ category }) => asNoun(category) !== null);
  const parts: Part[] = [];
  const nounPhrase = noun && asNoun(noun.category);
  if (noun && nounPhrase) {
    if (nounPhrase.lead) parts.push(nounPhrase.lead);
    parts.push({ term: noun.term, text: nounPhrase.name });
  } else {
    parts.push("people");
  }
  let clauses = 0;
  for (const { category, term } of indexed) {
    if (term === noun?.term) continue;
    const clause = asClause(category);
    let lead = clause.lead;
    if (clauses > 0 && lead.startsWith("who ")) {
      lead = `and ${lead.slice(4)}`;
    } else if (lead === "who are " && noun && !isSex(noun.category)) {
      lead = "who are also ";
    }
    parts.push(` ${lead}`, { term, text: clause.name });
    clauses += 1;
  }
  return parts;
}

export function gameText(categories: readonly Category[]): string {
  return gameParts(categories)
    .map((part) => (typeof part === "string" ? part : part.text))
    .join("");
}

/** The word for the people being named: "women", "men" or "people". */
export function peopleWord(categories: readonly Category[]): string {
  return categories.find(isSex)?.id ?? "people";
}

const KIND_NAMES: Record<string, string> = {
  citizenship: "country",
  field: "field of work",
  title: "noble title",
};

export function kindName(kind: string): string {
  return KIND_NAMES[kind] ?? kind;
}

const number = new Intl.NumberFormat("en-US");

export function formatCount(n: number): string {
  return number.format(n);
}

/** Below this many people, 100 may be out of reach. */
export const SMALL_CATEGORY = 300;

/**
 * How many people the game can draw on, as far as the page can know. Two
 * categories overlap by an unknown amount, except `women` plus one other.
 */
export function sizeNote(categories: readonly Category[]): { text: string; small: boolean } | null {
  const [a, b] = categories;
  if (!a) return null;
  const size = (c: Category) =>
    isSex(c) ? `${formatCount(c.people)} ${c.id}` : `${formatCount(c.people)} people`;
  if (!b) {
    if (a.people === 0) return null;
    return { text: size(a), small: a.people < SMALL_CATEGORY };
  }
  const women = categories.find((c) => c.id === "women");
  if (women) {
    const other = a === women ? b : a;
    return { text: `${formatCount(other.women)} women`, small: other.women < SMALL_CATEGORY };
  }
  return {
    text: categories.map((c) => `${c.label}: ${formatCount(c.people)}`).join(" · "),
    small: Math.min(a.people, b.people) < SMALL_CATEGORY,
  };
}
