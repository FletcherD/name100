# Name 100 Women — lookup server

`name100.md` is the spec and the source of truth for behaviour. This file tracks what is actually built.

## Status

| Milestone | State |
| --- | --- |
| 1. Builder on the fixture | done |
| 2. Builder on the full dump | not started; needs the dump in `data/` |
| 3. Matching, `lookup --local`, golden tests | done against the fixture; latency on a full index still to measure |
| 4. Server, HTTP API, `lookup` over HTTP | not started |

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
- `crates/cli` (`hundred-cli`, binary `hundred`): `build-index` and `lookup --local`.
  - `tests/golden.rs`: the spec's golden table, through the matcher and through the binary.
  - `benches/lookup.rs`: criterion benchmark; set `HUNDRED_BENCH_INDEX` to bench a real index.
- `fixtures/sample.ndjson`: about 375 real Wikidata entities; `fixtures/titles.txt` lists them.
- `scripts/fetch_fixture.py`: regenerates the fixture from live Wikidata (manual, needs network).
- `scripts/download_dump.sh`: resumable download of the full dump into `data/`.

Not built yet: `crates/server`, the `serve` subcommand, `lookup` against a server, config file, metrics, miss logging.

## Commands

```
cargo test                      # everything runs against the fixture, no network
cargo clippy --all-targets      # kept warning-free
cargo run --release -p hundred-cli -- build-index --dump fixtures/sample.ndjson --out index/fixture
cargo run --release -p hundred-cli -- lookup "Ruth Bader Ginsberg" --local --index index/fixture
cargo bench -p hundred-cli --bench lookup
lbzip2 -dc data/wikidata-*-all.json.bz2 | cargo run --release -p hundred-cli -- build-index --dump - --out index/full --dump-date YYYY-MM-DD
```

## Conventions and gotchas

- Rust 1.87 is installed. The workspace uses `resolver = "3"` so dependency versions respect it; do not pin crates by hand to work around MSRV errors.
- `data/` (the dump), `index/` (built artifacts) and `target/` are git-ignored.
- Never put a QID or property ID in code or tests from memory. Check it against Wikidata first; the spec's own example QIDs are illustrative.
- The fixture is real data and drifts when refetched (sitelink counts, new statements). Tests assert on relationships and stable facts, not exact sitelink numbers.
- The Wikidata query service is rate limited to about one request a minute; `fetch_fixture.py` makes a single combined query for that reason.
- Changing `Attribute::ALL` or the record layout changes the artifact. Existing indexes must be rebuilt, and the reader refuses a mismatched attribute list.
- Do not edit the fixture to make a test pass. When real data disagrees with the spec, that is a spec decision (this is how the Miley Cyrus case was settled).
- The record store is hand-written rather than `bincode`: the current `bincode` release on crates.io was believed to be a non-working stub when this was chosen; not verified.

## Decisions made beyond the original spec

- Miley Cyrus is rejected: Wikidata's only current gender value for her is genderfluid. Rejections of humans carry a `reason_detail` such as "gender is genderfluid".
- "cisgender woman" (`Q15145779`) is accepted alongside female and trans woman.
- Gender classification uses best-ranked statements only; deprecated ones never count.
- Fictional is decided by a list of character classes, checked before human.
- The mononym sitelink threshold applies even when a single-token name has only one record.
- Fuzzy search uses the `levenshtein_automata` crate, not `fst`'s `levenshtein` feature: the latter took 1–18 ms per two-edit query, over the 1 ms p99 budget.

## Measured so far (fixture index, in-process)

Exact lookups take about 1 µs, one-edit fuzzy about 20 µs, two-edit fuzzy 60–120 µs. Nearly all the fuzzy cost is building the per-query automaton, so it should not grow much on the full index, but that is unmeasured.
- Eleven attributes are stored per person so the index can serve other category games later.
