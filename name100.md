# Name 100 Women — Lookup Server Spec

Nov 7, 2026 · @Fletcher D

## Overview

Build a single-instance lookup server that decides whether a typed name refers to a real, notable woman, and returns her canonical identity. Phase 1 has two deliverables, built in this order:

1. **Index builder**, `hundred build-index`: Wikidata dump in, index files out, with a report of counts and size.
2. **Lookup server and CLI**, `hundred serve` and `hundred lookup "<name>"`: the CLI queries the running server and prints the verdict.

The design rests on three rules:

- **Data decides, models only resolve.** Acceptance always comes from verified Wikidata facts. A language model may propose a candidate, but never accepts one.
- **Identity is the Wikidata QID.** Every accepted guess maps to a QID, so aliases and spellings of the same person dedupe naturally.
- **The hot path is in memory.** Lookups hit a prebuilt index, not a database or a network call. Phase 1 has no fallback at all.

**Phase 1 (this build):** index builder from Wikidata labels and aliases, exact and fuzzy typo matching, lookup server, CLI client, tests, and miss logging.

**Phase 2 (only if the miss data justifies it):** Wikidata search fallback, LLM resolver, learned-alias overlay, negative cache, SQLite. **Out of scope entirely for now:** frontend, game sessions, timers, scoring, leaderboards, multi-instance deployment.

## Tech stack and layout

One Rust binary, `hundred`, with subcommands. Use these crates unless there is a strong reason not to:

| Concern | Crate |
| --- | --- |
| Async runtime, HTTP server | `tokio`, `axum` |
| CLI | `clap` (derive) |
| Name index | `fst`, read into memory |
| Fuzzy search | `levenshtein_automata` DFAs run over the `fst` (not `fst`'s own `levenshtein` feature, which measured 1–18 ms per two-edit query against roughly 0.1 ms) |
| Record store | flat binary files in a small hand-written format (see `crates/core/src/artifact.rs`) |
| Normalization | `unicode-normalization`, `caseless` |
| Dump parsing | `serde_json` streaming per line, `bzip2` or `flate2` |
| Logging, metrics | `tracing`, `metrics` + Prometheus exporter |
| HTTP client (CLI only) | `reqwest` |

Suggested layout:

```
crates/
  core/       normalize, index types, matcher, verdict types
  build/      Wikidata dump → index artifact
  server/     axum app
  cli/        clap entrypoint: build-index, serve, lookup
fixtures/     small NDJSON Wikidata sample + golden cases
scripts/      fixture fetcher, dump downloader
```

## Index build (`hundred build-index`)

The builder streams a Wikidata entity dump and writes one immutable index artifact. It runs offline, weekly in production.

**Input.** `--dump <path>` accepts the full dump (`latest-all.json.bz2` or `.gz` from dumps.wikimedia.org), or an NDJSON fixture in the same entity format. Parse line by line; never load the dump into memory. Parallelize JSON parsing across cores; decompression is the usual bottleneck, so allow piping from `lbzip2 -dc`.

**Which entities to keep.** Index every human, not only women, so rejections can say why:

| Wikidata condition | Stored kind |
| --- | --- |
| `P31` includes `Q5` (human), `P21` includes `Q6581072` (female), `Q1052281` (trans woman) or `Q15145779` (cisgender woman) | `woman` (accepted) |
| `P31` includes `Q5`, any other or missing `P21` | `not_woman` |
| `P31` includes a listed fictional class | `fictional` |

Rules that the real data forced:

- **Statement ranks.** Classification uses only the current statements: the preferred ones if any exist, otherwise the normal ones. Deprecated statements never count (Elliot Page carries deprecated male and female values).
- **Fictional is checked first**, and needs a class list. Wikidata does not tag every character as fictional human (`Q15632617`): Hermione Granger is only a literary, film and theatrical character. The listed classes are fictional human, character, and literary, film, television, animated, comics, video game, theatrical, anime and musical theatre character (`FICTIONAL_CLASSES` in `crates/build/src/entity.rs`).
- **Entities that are neither** (the goddess Venus, "Madonna" the artistic theme) are dropped.

Keep only entities with at least `min_sitelinks` sitelinks (Wikipedia articles in any language; count only Wikipedia sitelinks, not Commons, Wikiquote, Wikisource, Wikispecies or Abstract Wikipedia; default 1). This is the notability bar: it excludes the millions of mass-imported researchers with no articles. Everything else is dropped.

**Fields per record:** QID, display label (the `en` label, else `mul`), short description, sitelink count, birth year (`P569`), death year (`P570`), English Wikipedia title, kind, and the attribute lists below. Aliases are not stored on the record; they live in the name index.

**Attributes.** Every kept person, accepted or not, carries a list of Wikidata item QIDs for each of these properties, so the same index can later serve other category games (see "Generic categories"):

| Attribute | Property | Values kept |
| --- | --- | --- |
| Sex or gender | `P21` | current (best-ranked) only |
| Occupation | `P106` | every non-deprecated value |
| Country of citizenship | `P27` | every non-deprecated value |
| Award received | `P166` | every non-deprecated value |
| Position held | `P39` | every non-deprecated value |
| Sport | `P641` | every non-deprecated value |
| Participant in | `P1344` | every non-deprecated value |
| Genre | `P136` | every non-deprecated value |
| Instrument | `P1303` | every non-deprecated value |
| Field of work | `P101` | every non-deprecated value |
| Noble title | `P97` | every non-deprecated value |

The list is defined once, in `crates/core/src/attribute.rs`. The manifest records it, and the server refuses an index built with a different list.

**Which names to index.** Labels and aliases in `en` and `mul` only, to start. `mul` is required: Wikidata increasingly stores a person's name once as a multilingual label instead of per language, so `en` alone would miss people. Make the language list configurable (`--langs`). Adding more languages, or every Latin-script label, is a later decision made after measuring the first full build.

**Normalization** (shared with lookup; lives in `core`):

1. Unicode NFKD, then drop combining marks ("Beyoncé" → "beyonce").
2. Full case fold.
3. Map punctuation and hyphens to spaces; drop apostrophes and periods ("O'Keeffe" → "okeeffe", "J.K." → "jk").
4. Collapse whitespace and trim.

**Artifact.** One directory, versioned by build date:

- `names.fst`: normalized name → value. FST keys must be sorted and unique. Most names belong to one record, so the value encodes that record's index directly (top bit clear); shared names set the top bit and point into the posting list instead.
- `postings.bin`: for each shared name, a list of record indices, ascending (which is most popular first; see below).
- `records_hot.bin`: fixed-size 8-byte records holding only what matching needs: QID, sitelink count, and the stored kind.
- `records_cold.bin`: fields read once per returned match: label, description, birth and death years, Wikipedia title, and the attribute lists.
- `manifest.json`: build date, dump date, counts (including people per gender value), sizes, attribute list, schema version.

**Record order.** Sort records by sitelink count, descending, before writing. A record's index is then its popularity rank, so the dominance check compares indices, and the most-guessed people sit together in the first few MB of `records_hot.bin`.

**Loading.** At startup the server reads `names.fst`, `postings.bin`, and `records_hot.bin` fully into memory, so the first lookups after a restart don't wait on disk. `records_cold.bin` is mmapped and paged in on demand. Rough size estimate for 2–4 million people: 200–450 MB total, mostly the cold file; milestone 2 measures the real figure. Target startup under 5 seconds.

**Fixture.** `fixtures/sample.ndjson` holds about 375 real entities covering every golden test case, plus confusable neighbors (other Jennifers, men sharing a surname). It is generated from live Wikidata by `scripts/fetch_fixture.py` from the titles in `fixtures/titles.txt`, trimmed to the fields the builder reads, and committed. All tests run against it; nothing in CI downloads the dump or runs the script.

## Lookup pipeline

Each lookup normalizes the input, then runs these stages in order and stops at the first one that yields a verdict:

1. **Input guard.** Reject empty input, input over 100 characters, or input with no letters, as `not_a_name`.
2. **Exact match** against all indexed labels and aliases. Get the posting list and apply the resolution rule below.
3. **Fuzzy match**, only if there was no exact hit, using a Levenshtein automaton over the FST. Allowed edit distance depends on input length: 0 under 5 characters, 1 for 5–9, 2 for 10 or more. Take the candidates with the lowest distance, then apply the resolution rule. Set `corrected_from` in the response. Swapping two adjacent letters counts as one edit (`fuzzy_transpositions`, on by default), since swaps are the most common typing error.
4. **No match** → `not_found`.

**Miss logging.** Every `not_found` and `ambiguous` verdict emits a structured `tracing` event with the normalized input. No database: a script over the logs ranks misses by frequency. That ranking is the evidence for deciding whether Phase 2 is needed.

**Resolution rule.** When a name maps to several records, pick the top one by sitelinks only if it is clearly dominant:

- Top sitelinks ≥ `dominance_ratio` × runner-up (default 3).
- For single-token input ("Madonna", "Cher", "Jennifer"), the top record also needs ≥ `mononym_min_sitelinks` (default 40).
- Otherwise return `ambiguous`, with up to 5 candidates so a future UI can ask "which one?"

The mononym threshold also applies when a single-token name has only one record: an obscure person whose alias happens to be "Jennifer" is not accepted on that alone, and the verdict is `ambiguous` with that one candidate.

If the chosen record's kind is `not_woman` or `fictional`, return that rejection. A `not_woman` rejection explains itself in `reason_detail`, from the person's stored gender values: "gender is male", "gender is genderfluid", "gender is non-binary, trans man", or "no gender recorded". The dominance check runs over all humans, so a name shared with a far more famous man resolves to him and is rejected, rather than silently accepting an obscure woman with the same name.

**Latency targets**: p50 under 50 µs and p99 under 1 ms in-process; p99 under 5 ms over loopback HTTP.

## HTTP API and CLI

**Endpoints:**

- `GET /v1/lookup?q=<name>` → verdict JSON (below). Always HTTP 200 for a verdict, including rejections; 400 with `{"error": ...}` for a missing `q`.
- `GET /healthz` → 200 with the loaded index's `manifest.json`.
- `GET /metrics` → Prometheus text.

**Response:**

```json
{
  "input": "Ruth Bader Ginsberg",
  "normalized": "ruth bader ginsberg",
  "verdict": "accepted",
  "reason": null,
  "reason_detail": null,
  "matched_via": "fuzzy",
  "corrected_from": "ruth bader ginsberg",
  "match": {
    "qid": "Q11116",
    "name": "Ruth Bader Ginsburg",
    "description": "American lawyer and jurist",
    "birth_year": 1933,
    "death_year": 2020,
    "sitelinks": 120,
    "wikipedia_url": "https://en.wikipedia.org/wiki/Ruth_Bader_Ginsburg"
  },
  "candidates": [],
  "latency_us": 41
}
```

Field values (the QID and sitelink count above are illustrative):

- `verdict`: `accepted` | `rejected`
- `reason`: null when accepted; otherwise `not_a_name`, `not_woman`, `fictional`, `ambiguous`, `not_found`
- `reason_detail`: a short human-readable explanation, set for `not_woman` (for example `gender is genderfluid`); otherwise null
- `matched_via`: `exact` | `fuzzy` | null when nothing matched
- `candidates`: up to 5 `match` objects, filled only for `ambiguous`

**CLI** (`clap` subcommands of one binary):

```
hundred build-index --dump <path> --out <dir> [--langs en,mul] [--min-sitelinks 1]
hundred serve --index <dir> [--config hundred.toml] [--bind 127.0.0.1:8080]
hundred lookup "<name>" [--server http://127.0.0.1:8080] [--json]
hundred lookup "<name>" --local --index <dir>      # in-process, no server
```

`lookup` prints one human-readable line by default, for example `✓ Ruth Bader Ginsburg (Q11116) — American lawyer and jurist, 1933–2020 [fuzzy, corrected]` or `✗ Elon Musk (Q317521) — not a woman: gender is male`. `--json` prints the raw response. Exit code: 0 accepted, 1 rejected, 2 error.

## Config, testing, and milestones

**Config.** `hundred.toml` (read from the working directory, or `--config <file>`), every key overridable by a `HUNDRED_<KEY>` environment variable. Flags beat the environment, which beats the file. Keys: `index`, `bind`, `langs`, `min_sitelinks`, `dominance_ratio`, `mononym_min_sitelinks`, `fuzzy_one_edit_min_len`, `fuzzy_two_edits_min_len`, `fuzzy_transpositions`, `max_input_chars`, `max_candidates`. Unknown keys are an error. `hundred.example.toml` lists them with their defaults.

**Observability.** `serve` writes JSON logs to stdout, including the miss events above (target `hundred::miss`, fields `reason` and `normalized`); `scripts/rank_misses.py` ranks them. `/metrics` exports `hundred_lookups_total` by `verdict`, `reason` and `matched_via`, and the histograms `hundred_lookup_seconds` and `hundred_lookup_stage_seconds` (by `stage`: guard, exact, fuzzy, resolve).

**Golden tests** (run against the fixture):

| Input | Expected |
| --- | --- |
| Billie Eilish, Kamala Harris, Hillary Clinton | accepted, exact |
| Miley Cyrus | rejected, `not_woman`, detail "gender is genderfluid" (Wikidata's only current gender value for her) |
| Caitlyn Jenner | accepted, exact |
| Beyonce (no accent) | accepted, exact after normalization |
| Madonna, Cher | accepted (dominant mononyms) |
| Ruth Bader Ginsberg | accepted, fuzzy, `corrected_from` set |
| Lady Gaga and Stefani Germanotta | accepted, same QID (via Wikidata aliases) |
| Elon Musk | rejected, `not_woman`, detail "gender is male" |
| Hermione Granger | rejected, `fictional` |
| Jennifer | rejected, `ambiguous`, candidates listed |
| Oh fuck off | rejected, `not_found`, miss event logged |
| 1234 | rejected, `not_a_name` |

Also required: property tests that normalization is idempotent, and a criterion benchmark of the in-process lookup.

**Milestones** (each ends with passing tests):

1. **Builder on the fixture.** `core` normalization, entity parser and classifier, artifact writer, `manifest.json` with counts. *Done.*
2. **Builder on the full dump** (`en` + `mul`, `min_sitelinks` 1). Report entity counts by kind, name count, artifact size, build time, and peak memory. Decide from these whether to add languages or change the sitelink bar. This completes deliverable 1.
3. **Matching.** Exact match, fuzzy match, the resolution rule, and `lookup --local`; all golden tests pass. *Done against the fixture; the miss-event half of the "Oh fuck off" case lands with milestone 4's logging.*
4. **Server.** `serve` with HTTP API, health, metrics, miss logging; `lookup` against the server. This completes deliverable 2. *Built and tested against the fixture.* Run real players' guesses through it and review the miss ranking to decide on Phase 2.

**Open questions:**

- [x] Should nonbinary people with no female `P21` value count? **Decided: no.** They are rejected as `not_woman`, and the response says why in `reason_detail` ("gender is genderfluid"). This includes Miley Cyrus.
- [ ] Whether to index more languages than en + mul, after measuring the first full build.
- [ ] Minimum sitelinks for acceptance. Default is 1; raising it makes the game harder and the index smaller.

## Generic categories (future)

The game may later run with other categories: name 100 trans people, actors, Nobel laureates, tennis players. Phase 1 does not build this, but the index is shaped for it:

- Name matching and the dominance rule are the same for every category. They run over all humans, so the most famous bearer of a name is always the one chosen.
- Only the last step differs. Instead of "is the chosen person's kind `woman`", the server tests the chosen person's stored attributes against the category.
- A category is a set of values for one attribute, not a single value: "actor" has to list actor, film actor, television actor, stage actor and so on. Those sets are per-game configuration, still to be designed.
- Birth and death years and the sitelink count are stored too, so "born before 1900" or a hard mode with a fame ceiling need nothing new.

Not stored, deliberately: sexual orientation, ethnic group, religion and medical conditions (sparse and sensitive; a wrong rejection is worse than for an occupation), and place of birth (stored in Wikidata as a town, so "born in Texas" would need a place hierarchy). Adding an attribute is a few lines in `crates/core/src/attribute.rs` plus an index rebuild.

## Phase 2 (only if needed)

Build this only if Phase 1's miss log shows many real, notable women being rejected, for example nicknames, maiden or married names, or people added to Wikidata since the last build. If most misses are typos beyond the fuzzy threshold or garbage, tune Phase 1 instead.

If it is needed, the sketch is:

- **Fallback.** On `not_found`, query Wikidata's `wbsearchentities`, then verify candidates with `wbgetentities` using the same rules as the index build. Optionally, a small LLM proposes a canonical name to search for, but it never decides acceptance; the player's input is treated as untrusted data.
- **Learned aliases.** Verified fallback results are written to SQLite (input → QID, with source and timestamp) and loaded into an in-memory overlay checked after the exact match. The weekly index build folds them in.
- **Negative cache.** An in-process `moka` cache of rejected inputs, with TTLs by reason (long for garbage, short for `not_found`), checked before the fallback.
- **Protection.** Singleflight for identical concurrent misses, timeouts, per-IP rate limits, a daily LLM budget, a Wikidata-compliant User-Agent, and a config kill switch.
