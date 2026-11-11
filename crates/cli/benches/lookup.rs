//! In-process lookup latency.
//!
//! Runs against `HUNDRED_BENCH_INDEX` if set (point it at a full-dump index
//! for real numbers); otherwise against an index built from the fixture,
//! which only shows the fixed per-lookup cost.

use std::path::{Path, PathBuf};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Index, MatchConfig, Matcher};

fn matcher() -> (Matcher, Option<tempfile::TempDir>) {
    let (path, dir): (PathBuf, _) = match std::env::var_os("HUNDRED_BENCH_INDEX") {
        Some(path) => (path.into(), None),
        None => {
            let dump = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.ndjson");
            let dir = tempfile::tempdir().unwrap();
            build_index(open_dump(dump.to_str().unwrap()).unwrap(), dir.path(), &BuildOptions::default())
                .unwrap();
            (dir.path().to_owned(), Some(dir))
        }
    };
    (Matcher::new(Index::open(&path).unwrap(), MatchConfig::default()), dir)
}

fn lookups(c: &mut Criterion) {
    let (matcher, _dir) = matcher();
    let cases = [
        ("exact", "Kamala Harris"),
        ("exact_normalized", "BEYONCÉ"),
        ("exact_mononym", "Madonna"),
        ("ambiguous", "Jennifer"),
        ("rejected_man", "Elon Musk"),
        ("fuzzy_short", "Madona"),
        ("fuzzy", "Ruth Bader Ginsberg"),
        ("fuzzy_long", "Alexandria Ocasio Cortes"),
        ("not_found_short", "zzzz"),
        ("not_found_fuzzy", "Oh fuck off"),
        ("not_a_name", "1234"),
    ];
    let mut group = c.benchmark_group("lookup");
    for (name, input) in cases {
        group.bench_function(name, |b| b.iter(|| matcher.lookup(black_box(input)).unwrap()));
    }
    group.finish();
}

criterion_group!(benches, lookups);
criterion_main!(benches);
