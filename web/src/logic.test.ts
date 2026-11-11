import { describe, expect, it } from "vitest";
import { categoryParam, type Category, type LookupResponse, type Match } from "./api";
import { rejection } from "./messages";
import * as stats from "./stats";
import { formatTime, gameKey, type Entry } from "./store";
import { gameText, pluralPhrase, sizeNote } from "./wording";

const category = (id: string, label: string, people = 1000, women = 400): Category => ({
  id,
  label,
  kind: id.split(":")[0] === id ? "gender" : id.split(":")[0]!,
  people,
  women,
});
const women = category("women", "women", 5000, 5000);
// The QIDs here are placeholders; nothing looks them up.
const actor = category("occupation:Q1", "actor");
const france = category("citizenship:Q2", "France");

describe("wording", () => {
  it("makes plurals of the head noun", () => {
    expect(pluralPhrase("actor")).toBe("actors");
    expect(pluralPhrase("film actor")).toBe("film actors");
    expect(pluralPhrase("member of parliament")).toBe("members of parliament");
    expect(pluralPhrase("businesswoman")).toBe("businesswomen");
    expect(pluralPhrase("duchess")).toBe("duchesses");
    expect(pluralPhrase("spy")).toBe("spies");
    expect(pluralPhrase("jockey")).toBe("jockeys");
  });

  it("words a game by the kind of its categories", () => {
    expect(gameText([women])).toBe("women");
    expect(gameText([actor])).toBe("actors");
    expect(gameText([france])).toBe("people from France");
    expect(gameText([category("citizenship:Q3", "United States")])).toBe("people from the United States");
    expect(gameText([category("sport:Q4", "tennis")])).toBe("people in tennis");
    expect(gameText([category("instrument:Q5", "guitar")])).toBe("people who play guitar");
    expect(gameText([category("award:Q6", "Nobel Prize in Physics")])).toBe(
      "winners of the Nobel Prize in Physics",
    );
  });

  it("combines two categories around one noun", () => {
    expect(gameText([women, category("occupation:Q7", "scientist")])).toBe("women who are scientists");
    expect(gameText([actor, women])).toBe("women who are actors");
    expect(gameText([actor, france])).toBe("actors from France");
    expect(gameText([france, actor])).toBe("actors from France");
    expect(gameText([actor, category("occupation:Q8", "politician")])).toBe(
      "actors who are also politicians",
    );
    expect(gameText([category("instrument:Q5", "guitar"), category("field:Q9", "physics")])).toBe(
      "people who play guitar and work in physics",
    );
  });

  it("sizes a game only as far as it can know", () => {
    expect(sizeNote([women])).toEqual({ text: "5,000 women", small: false });
    expect(sizeNote([women, actor])).toEqual({ text: "400 women", small: false });
    expect(sizeNote([actor, france])?.text).toBe("actor: 1,000 · France: 1,000");
    expect(sizeNote([category("sport:Q4", "tennis", 120)])?.small).toBe(true);
  });

  it("leaves the category off for the original game", () => {
    expect(categoryParam(["women"])).toBeNull();
    expect(categoryParam(["women", "occupation:Q1"])).toBe("women,occupation:Q1");
    expect(gameKey([actor, women])).toBe(gameKey([women, actor]));
  });
});

const person: Match = {
  qid: "Q1",
  name: "Somebody",
  description: "American physicist",
  birth_year: 1950,
  death_year: null,
  sitelinks: 30,
  wikipedia_url: null,
  categories: ["Scientist or scholar"],
  citizenships: ["United States"],
  genders: ["male"],
};
const rejected = (fields: Partial<LookupResponse>): LookupResponse => ({
  input: " Some Body ",
  verdict: "rejected",
  reason: "not_found",
  reason_detail: null,
  corrected_from: null,
  matched_name: null,
  matched_as: null,
  match: null,
  ...fields,
});
const said = (response: LookupResponse) =>
  rejection(response)
    .map((part) => (typeof part === "string" ? part : part.em))
    .join("");

describe("rejection messages", () => {
  it("follows the wording table", () => {
    expect(said(rejected({}))).toBe("Don't know Some Body.");
    expect(said(rejected({ reason: "ambiguous" }))).toBe(
      "More than one person is called Some Body. Be more specific.",
    );
    expect(said(rejected({ reason: "not_a_name" }))).toBe("That doesn't look like a name.");
    expect(said(rejected({ reason: "fictional", match: person }))).toBe("Somebody is fictional.");
    expect(said(rejected({ reason: "not_in_category", match: person }))).toBe(
      "Somebody isn't one: American physicist.",
    );
  });

  it("states gender only as what Wikidata records", () => {
    expect(said(rejected({ reason: "not_woman", match: person }))).toBe(
      "Somebody doesn't count here: Wikidata records their gender as male.",
    );
    const two = { ...person, genders: ["non-binary", "trans man"] };
    expect(said(rejected({ reason: "not_in_category", reason_detail: "gender is non-binary", match: two }))).toBe(
      "Somebody doesn't count here: Wikidata records their gender as non-binary and trans man.",
    );
    expect(said(rejected({ reason: "not_woman", match: { ...person, genders: [] } }))).toBe(
      "Somebody doesn't count here: Wikidata doesn't record a gender for them.",
    );
  });
});

const entry = (fields: Partial<Entry>): Entry => ({
  qid: "Q0",
  name: "Name",
  description: "",
  alias: null,
  corrected: false,
  at: 0,
  url: null,
  sitelinks: 20,
  birth: null,
  death: null,
  categories: [],
  citizenships: [],
  ...fields,
});

describe("result statistics", () => {
  it("merges small occupation groups into Other", () => {
    const entries = [
      ...Array.from({ length: 4 }, () => entry({ categories: ["Musician", "Actor"] })),
      ...Array.from({ length: 3 }, () => entry({ categories: ["Actor"] })),
      entry({ categories: ["Writer"] }),
      entry({ categories: ["Model"] }),
      entry({}),
    ];
    expect(stats.occupations(entries)).toEqual([
      { label: "Musician", count: 4 },
      { label: "Actor", count: 3 },
      { label: "Other", count: 2 },
      { label: "Uncategorized", count: 1 },
    ]);
  });

  it("bands fame by article count", () => {
    const counts = [0, 9, 10, 24, 25, 49, 50, 99, 100, 300].map((sitelinks) => entry({ sitelinks }));
    expect(stats.fame(counts).map((bar) => bar.count)).toEqual([2, 2, 2, 2, 2]);
    expect(stats.median([5, 1, 9])).toBe(5);
  });

  it("groups births by decade from 1900 and by century before", () => {
    const years = [-69, 1412, 1800, 1801, 1899, 1900, 1909, 1930, null].map((birth) => entry({ birth }));
    expect(stats.births(years)).toEqual({
      bars: [
        { label: "1st c. BC", count: 1 },
        { label: "15th c.", count: 1 },
        { label: "18th c.", count: 1 },
        { label: "19th c.", count: 2 },
        { label: "1900s", count: 2 },
        { label: "1910s", count: 0 },
        { label: "1920s", count: 0 },
        { label: "1930s", count: 1 },
      ],
      unknown: 1,
    });
    expect(stats.formatYear(-69)).toBe("69 BC");
  });

  it("counts someone with two countries under both", () => {
    const entries = [entry({ citizenships: ["France", "Poland"] }), entry({ citizenships: ["France"] })];
    expect(stats.countries(entries)).toEqual([
      { label: "France", count: 2 },
      { label: "Poland", count: 1 },
    ]);
  });

  it("finds the longest gap and the fastest ten", () => {
    // One name a second, except a ten-second wait before the fourth.
    const times = [1, 2, 3, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23].map((s) => s * 1000);
    const entries = times.map((at, i) => entry({ qid: `Q${i}`, name: `N${i}`, at }));
    const found = stats.superlatives(entries);
    expect(found.longestGap).toMatchObject({ ms: 10_000, after: { name: "N3" } });
    expect(found.fastestTen).toMatchObject({ ms: 10_000, first: { name: "N4" }, last: { name: "N13" } });
    expect(stats.superlatives(entries.slice(0, 9)).fastestTen).toBeNull();
  });

  it("formats times", () => {
    expect(formatTime(7_900)).toBe("0:07");
    expect(formatTime(754_000)).toBe("12:34");
    expect(formatTime(3_723_000)).toBe("1:02:03");
  });
});
