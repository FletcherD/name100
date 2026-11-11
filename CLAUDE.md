# Name 100 Women — lookup server

`name100.md` is the spec and the source of truth for behaviour. This file tracks what is actually built.

## Status

| Milestone | State |
| --- | --- |
| 1. Builder on the fixture | done |
| 2. Builder on the full dump | not started; needs the dump in `data/` |
| 3. Matching, `lookup --local`, golden tests | done against the fixture; latency on a full index still to measure |
| 4. Server, HTTP API, `lookup` over HTTP | done against the fixture; real players' guesses and the miss review still to do |

## What exists

- `crates/core` (`hundred-core`)
  - `normalize.rs`: the one name-normalization function, shared by builder and lookup.
  - `artifact.rs`: on-disk format (file names, magics, `HotRecord`, `ColdRecord`, `Manifest`).
  - `attribute.rs`: the per-person attribute list (gender, occupation, awards, ...).
  - `gender.rs`: accepted gender values and the "gender is ..." rejection text.
  - `index.rs`: `Index::open`, exact name lookup, hot and cold record access.
  - `matcher.rs`: `Matcher::lookup`, the pipeline (input guard, exact, fuzzy, resolution rule) and `MatchConfig`.
  - `verdict.rs`: the `Response` JSON shape and the one-line CLI summary.
- `crates/build` (`hundred-build`)
  - `entity.rs`: one dump line → `Person` or a drop reason; classification rules.
  - `lib.rs`: streaming build, artifact writer, manifest.
  - `tests/fixture.rs`: builds the fixture and reads the artifact back.
- `crates/server` (`hundred-server`): the axum app. `/v1/lookup`, `/healthz`, `/metrics`, miss events.
  - `tests/http.rs`: the API in-process. `tests/miss_log.rs`: miss events (own binary: it sets a global `tracing` subscriber).
- `crates/cli` (`hundred-cli`, binary `hundred`): `build-index`, `serve`, `lookup` (over HTTP, or `--local`).
  - `src/config.rs`: `hundred.toml` plus `HUNDRED_*` overrides.
  - `tests/server.rs`: runs `hundred serve` as a real process on an ephemeral port.
  - `tests/golden.rs`: the spec's golden table, through the matcher and through the binary.
  - `benches/lookup.rs`: criterion benchmark; set `HUNDRED_BENCH_INDEX` to bench a real index.
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
lbzip2 -dc data/wikidata-*-all.json.bz2 | cargo run --release -p hundred-cli -- build-index --dump - --out index/full --dump-date YYYY-MM-DD
```

## Conventions and gotchas

- Rust 1.87 is installed. The workspace uses `resolver = "3"` so dependency versions respect it; do not pin crates by hand to work around MSRV errors.
- `data/` (the dump), `index/` (built artifacts) and `target/` are git-ignored.
- Never put a QID or property ID in code or tests from memory. Check it against Wikidata first; the spec's own example QIDs are illustrative.
- The fixture is real data and drifts when refetched (sitelink counts, new statements). Tests assert on relationships and stable facts, not exact sitelink numbers.
- The Wikidata query service is rate limited to about one request a minute; `fetch_fixture.py` makes a single combined query for that reason.
- The full dump is not in QID order: a prefix of the file is a scattered sample of people, not the famous ones. A truncated download can still be built: `head -c <N>G dump.bz2 | lbzip2 -dc | head -n -1 | hundred build-index --dump - ...` (the last line is cut off, so drop it).
- Changing `Attribute::ALL` or the record layout changes the artifact. Existing indexes must be rebuilt, and the reader refuses a mismatched attribute list.
- `serve` prints `hundred: listening on http://ADDR` as its first stderr line; tests bind to port 0 and parse it. Keep it first.
- Server metrics use a per-app recorder (`metrics::with_local_recorder`), not a global one, so tests can run several apps.
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

## Measured so far

- Fixture index, in-process: exact lookups about 1 µs, one-edit typo correction about 20 µs.
- Fixture index over loopback HTTP (keep-alive Python client): p50 about 95 µs, p99 about 245 µs. Target: p99 under 5 ms.
- First 14 GB of the dump (13%): 17.4 million entities, 886,725 people kept, 1.26 million names, 116 MB artifact, 10.5 minutes, 566 MB peak memory, server load 25 ms. Projects to roughly 6.5 million people, 850 MB and 75-80 minutes for the full dump.
- On that index, exact lookups stayed at 3-5 µs. The old two-edit search over all names took 2-3 ms, which is why typo correction was cut back. The replacement has not been measured on a large index yet.
