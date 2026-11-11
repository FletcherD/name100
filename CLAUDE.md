# Name 100 Women — lookup server

`name100.md` is the spec for the lookup server and the source of truth for its behaviour. `name100-frontend.md` is the spec for the web game, built in `web/` with its open questions settled at their recommended defaults. This file tracks what is actually built.

## Status

| Milestone | State |
| --- | --- |
| 1. Builder on the fixture | done |
| 2. Builder on the full dump | not started; needs the dump in `data/` |
| 3. Matching, `lookup --local`, golden tests | done against the fixture; latency on a full index still to measure |
| 4. Server, HTTP API, `lookup` over HTTP | done against the fixture; real players' guesses and the miss review still to do |
| 5. Web game (`web/`) | first version built and played on localhost against the partial index; not deployed |

## What exists

- `crates/core` (`hundred-core`)
  - `normalize.rs`: the one name-normalization function, shared by builder and lookup.
  - `artifact.rs`: on-disk format (file names, magics, `HotRecord`, `ColdRecord`, `Manifest`).
  - `attribute.rs`: the per-person attribute list (gender, occupation, awards, ...).
  - `gender.rs`: accepted gender values and the "gender is ..." rejection text.
  - `category.rs`: broad occupation categories and the rule for a person's main one.
  - `catalogue.rs`: playable categories: terms, search, membership, file format.
  - `index.rs`: `Index::open`, exact name lookup, hot and cold record access.
  - `matcher.rs`: `Matcher::lookup`, the pipeline (input guard, exact, fuzzy, resolution rule) and `MatchConfig`.
  - `verdict.rs`: the `Response` JSON shape and the one-line CLI summary.
- `crates/build` (`hundred-build`)
  - `entity.rs`: one dump line → `Person` or a drop reason; classification rules.
  - `lib.rs`: streaming build, artifact writer, manifest.
  - `tests/fixture.rs`: builds the fixture and reads the artifact back.
- `crates/server` (`hundred-server`): the axum app. `/v1/lookup`, `/healthz`, `/metrics`, miss events, CORS, and the per-address rate limit (`limit.rs`).
  - `tests/http.rs`: the API in-process. `tests/miss_log.rs`: miss events (own binary: it sets a global `tracing` subscriber).
- `crates/cli` (`hundred-cli`, binary `hundred`): `build-index`, `serve`, `lookup` (over HTTP, or `--local`; `--category`), `categories`.
  - `src/config.rs`: `hundred.toml` plus `HUNDRED_*` overrides.
  - `tests/server.rs`: runs `hundred serve` as a real process on an ephemeral port.
  - `tests/golden.rs`: the spec's golden table, through the matcher and through the binary.
  - `benches/lookup.rs`: criterion benchmark; set `HUNDRED_BENCH_INDEX` to bench a real index.
- `web/`: the game. TypeScript and Vite, no UI framework, no runtime dependencies.
  - `src/main.ts`: the page and the game loop (guessing, clock, saving, the page address).
  - `src/api.ts`: the two server calls. `src/store.ts`: game state and local storage.
  - `src/wording.ts`: "Name 100 women who are scientists" from category kinds. `src/messages.ts`: the rejection wording table.
  - `src/picker.ts`: category search. `src/dialog.ts`: the confirmation dialog.
  - `src/results.ts`, `src/stats.ts`, `src/charts.ts`: the result screen, its numbers, and its inline-SVG charts.
  - `src/logic.test.ts`: vitest tests of wording, messages and statistics (no browser).
- `fixtures/sample.ndjson`: about 375 real Wikidata entities; `fixtures/titles.txt` lists them.
- `scripts/fetch_fixture.py`: regenerates the fixture from live Wikidata (manual, needs network).
- `scripts/download_dump.sh`: resumable download of the full dump into `data/`.
- `scripts/rank_misses.py`: ranks miss events from the server's JSON log.
- `hundred.example.toml`: every config key with its default.

All Phase 1 code exists. What remains is running it on the full dump (milestone 2) and measuring there.

## Commands

```
cargo test                      # everything runs against the fixture, no network
cargo clippy --all-targets      # kept warning-free
cargo run --release -p hundred-cli -- build-index --dump fixtures/sample.ndjson --out index/fixture
cargo run --release -p hundred-cli -- lookup "Ruth Bader Ginsberg" --local --index index/fixture
cargo bench -p hundred-cli --bench lookup
cargo run --release -p hundred-cli -- serve --index index/fixture        # JSON logs on stdout
cargo run --release -p hundred-cli -- lookup "Madona"                   # asks 127.0.0.1:8080
cd web && npm run dev          # the game on http://localhost:5173, calling the server on 127.0.0.1:8080
cd web && npm test && npm run build   # vitest, then typecheck and a static build in web/dist
lbzip2 -dc data/wikidata-*-all.json.bz2 | cargo run --release -p hundred-cli -- build-index --dump - --out index/full --dump-date YYYY-MM-DD
```

## Conventions and gotchas

- Rust 1.87 is installed. The workspace uses `resolver = "3"` so dependency versions respect it; do not pin crates by hand to work around MSRV errors.
- `data/` (the dump), `index/` (built artifacts) and `target/` are git-ignored.
- Never put a QID or property ID in code or tests from memory. Check it against Wikidata first; the spec's own example QIDs are illustrative.
- Tests build the fixture with `category_min_people: 5`; the default of 100 would leave its 369 people with almost no categories.
- `lbzip2` on a truncated file may drop a few of the last complete lines (it aborts on the bad block before flushing), so two partial builds of the same bytes can differ by a handful of entities.
- The fixture is real data and drifts when refetched (sitelink counts, new statements). Tests assert on relationships and stable facts, not exact sitelink numbers.
- The Wikidata query service is rate limited to about one request a minute; `fetch_fixture.py` makes a single combined query for that reason.
- The full dump is not in QID order: a prefix of the file is a scattered sample of people, not the famous ones. A truncated download can still be built: `head -c <N>G dump.bz2 | lbzip2 -dc | head -n -1 | hundred build-index --dump - ...` (the last line is cut off, so drop it).
- Changing `Attribute::ALL` or the record layout changes the artifact. Existing indexes must be rebuilt, and the reader refuses a mismatched attribute list.
- `serve` prints `hundred: listening on http://ADDR` as its first stderr line; tests bind to port 0 and parse it. Keep it first.
- Server metrics use a per-app recorder (`metrics::with_local_recorder`), not a global one, so tests can run several apps.
- The game finds the server through `VITE_API_BASE`; unset, it is port 8080 on the host that served the page. The Vite port is fixed at 5173. To play from another machine on the network: `npm run dev:lan` in `web/`, and `HUNDRED_CORS_ORIGINS=http://<this-host-ip>:5173 hundred serve --bind 0.0.0.0:8080 ...`.
- The game saves on `pagehide`, so a browser test that writes local storage and then navigates has its write overwritten; seed storage before the page loads instead.
- The rate limit lets 60 lookups through at once and then 10 a second. A script replaying names faster than that gets 429s, which the game reports as "Couldn't check that".
- The partial index's catalogue is thin ("actors" does not find *actor*) and most country labels are missing, so the category search and the countries chart look odd there. That is the partial dump, not the game.
- Do not edit the fixture to make a test pass. When real data disagrees with the spec, that is a spec decision (this is how the Miley Cyrus case was settled).
- The record store is hand-written rather than `bincode`: the current `bincode` release on crates.io was believed to be a non-working stub when this was chosen; not verified.

## Decisions made beyond the original spec

- Miley Cyrus is rejected: Wikidata's only current gender value for her is genderfluid. Rejections of humans carry a `reason_detail` such as "gender is genderfluid".
- "cisgender woman" (`Q15145779`) is accepted alongside female and trans woman.
- Gender classification uses best-ranked statements only; deprecated ones never count.
- Fictional is decided by a list of character classes, checked before human.
- The mononym sitelink threshold applies even when a single-token name has only one record.
- `fuzzy_transpositions` is on by default: two swapped neighbouring letters are one edit.
- Typo correction is one edit only, and only towards people with at least `fuzzy_min_sitelinks` articles (default 10). That limit is baked into the index as `names_fuzzy.fst`; changing it means rebuilding. Two edits over all names took 2–3 ms on a 13% index and accepted junk ("some woman" → alias "Swole Woman").
- Intersex woman and intersex trans woman are accepted gender values; the gender table was extended from values seen in a partial full-dump build.
- Fuzzy search uses the `levenshtein_automata` crate, not `fst`'s `levenshtein` feature: the latter took 1–18 ms per two-edit query, over the 1 ms p99 budget.
- Eleven attributes are stored per person so the index can serve other category games later.
- The server allows cross-origin calls from `cors_origins` (and localhost pages by default) and rate limits lookups per address; the game will be hosted on GitHub Pages with the server elsewhere.
- Matches carry `categories`, `occupations`, `citizenships` and `genders` as English labels. The builder spools labels of possible attribute values to `labels.tmp` during the scan, keeps the referenced ones in `labels.bin`, and maps occupations to broad categories (`occupation_categories.bin`) by walking `P279` links. The main category is a heuristic in `crates/core/src/category.rs`; changing the category list means rebuilding.
- The fixture now also holds about 2,000 referenced items (occupations and their ancestor classes, countries, sports, genres), fetched by `scripts/fetch_fixture.py`.
- Category games are built server-side: `catalogue.bin` holds every playable category (an attribute value with at least `category_min_people` people, widened through the class tree), `/v1/categories` searches it, and `lookup?category=a,b` plays it, resolving shared names among people in the category. The original women game (no `category`) is unchanged. Catalogue code: `crates/core/src/catalogue.rs` (search, membership) and `crates/build/src/catalogue.rs` (construction).
- A partial dump cannot validate labels, occupation categories or the catalogue: the classes and countries people point at are scattered through the file, so most are missing from a 13% slice.
- `/v1/categories?ids=a,b` returns exactly those categories. The game needs it to name a category it was linked to by id (`?category=occupation:Q...`) and to refresh saved counts.
- Web game choices beyond its spec: best times are kept per game (per category combination); the occupation pie shows at most seven named slices before "Other"; changing category or starting a new game asks for confirmation only while a game with names is in progress; a link to a different game than the saved one asks before replacing it.
- Responses carry `matched_name` and `matched_as` (`name` or `alias`), so the game can show which of a person's names a guess hit. Cold records store each person's aliases as written for this.

## Measured so far

- Fixture index, in-process: exact lookups about 1 µs, one-edit typo correction about 20 µs.
- Fixture index over loopback HTTP (keep-alive Python client): p50 about 95 µs, p99 about 245 µs. Target: p99 under 5 ms.
- First 14 GB of the dump (13%): 17.4 million entities, 886,725 people kept, 1.26 million names, 116 MB artifact, 10.5 minutes, 566 MB peak memory, server load 25 ms. Projects to roughly 6.5 million people, 850 MB and 75-80 minutes for the full dump.
- On that index, exact lookups stayed at 3-5 µs. The old two-edit search over all names took 2-3 ms, which is why typo correction was cut back. The replacement (one edit over 147,613 well-known names, a 1.7 MB index) takes 130-210 µs p50 and 170-275 µs p99 there. The full index will have several times as many names, so recheck against the 1 ms p99 target.
