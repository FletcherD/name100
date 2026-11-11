//! Build the committed fixture and read the artifact back.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::artifact::{COLD_FILE, HOT_FILE, NAMES_FILE, POSTINGS_FILE};
use hundred_core::{describe_genders, normalize, Attribute, Index, Kind, Manifest};

fn fixture() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sample.ndjson")
        .to_str()
        .unwrap()
        .to_owned()
}

struct Built {
    _dir: tempfile::TempDir,
    path: PathBuf,
    manifest: Manifest,
    index: Index,
}

/// One build shared by every test in this file.
fn built() -> &'static Built {
    static BUILT: OnceLock<Built> = OnceLock::new();
    BUILT.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        let options = BuildOptions { dump_date: Some("2026-11-10".into()), ..Default::default() };
        let manifest = build_index(open_dump(&fixture()).unwrap(), &path, &options).unwrap();
        let index = Index::open(&path).unwrap();
        Built { _dir: dir, path, manifest, index }
    })
}

/// What a rejection of this person would tell the player.
fn rejection_detail(name: &str) -> String {
    let index = &built().index;
    let record = index.exact(&normalize(name)).unwrap().as_slice()[0];
    describe_genders(index.cold(record).unwrap().attributes.get(Attribute::Gender))
}

/// The single record a name resolves to, as (QID, kind).
fn only(name: &str) -> (String, Kind) {
    let index = &built().index;
    let hits = index.exact(&normalize(name)).unwrap_or_else(|| panic!("{name:?} is not indexed"));
    let [record] = hits.as_slice() else {
        panic!("{name:?} maps to {} records", hits.as_slice().len());
    };
    let hot = index.hot(*record).unwrap();
    (hot.qid_string(), hot.kind)
}

#[test]
fn manifest_counts_are_consistent() {
    let Built { manifest, index, path, .. } = built();
    let c = &manifest.counts;
    assert_eq!(c.parse_errors, 0);
    assert!(c.records >= 300, "fixture should hold about 300 people, has {}", c.records);
    assert_eq!(c.records, index.len() as u64);
    assert_eq!(c.records, c.women + c.not_women + c.fictional);
    assert_eq!(
        c.entities_read,
        c.records + c.dropped_not_person + c.dropped_below_min_sitelinks + c.dropped_no_name
    );
    assert!(c.women > 200 && c.not_women > 50 && c.fictional > 20, "{c:?}");
    assert_eq!(c.names, index.names().len() as u64);
    assert!(c.shared_names > 0 && c.name_record_pairs > c.names);

    assert_eq!(manifest.langs, ["en", "mul"]);
    assert_eq!(manifest.min_sitelinks, 1);
    assert_eq!(manifest.dump_date.as_deref(), Some("2026-11-10"));
    assert_eq!(index.manifest(), manifest);

    let size = |name: &str| std::fs::metadata(path.join(name)).unwrap().len();
    assert_eq!(manifest.sizes.names_fst, size(NAMES_FILE));
    assert_eq!(manifest.sizes.postings_bin, size(POSTINGS_FILE));
    assert_eq!(manifest.sizes.records_hot_bin, size(HOT_FILE));
    assert_eq!(manifest.sizes.records_cold_bin, size(COLD_FILE));
}

#[test]
fn records_are_sorted_by_sitelinks_descending() {
    let index = &built().index;
    let sitelinks: Vec<u16> =
        (0..index.len() as u32).map(|i| index.hot(i).unwrap().sitelinks).collect();
    assert!(sitelinks.windows(2).all(|pair| pair[0] >= pair[1]));
    assert!(*sitelinks.last().unwrap() >= 1, "min_sitelinks is 1");
}

#[test]
fn every_name_resolves_to_ascending_valid_records() {
    use fst::Streamer;
    let index = &built().index;
    let mut stream = index.names().stream();
    let mut pairs = 0u64;
    while let Some((name, value)) = stream.next() {
        let name = std::str::from_utf8(name).unwrap();
        assert_eq!(normalize(name), name, "index keys are normalized");
        let hits = index.hits(value);
        let records = hits.as_slice();
        assert!(!records.is_empty(), "{name:?} has no records");
        assert!(records.windows(2).all(|pair| pair[0] < pair[1]), "{name:?} postings not ascending");
        assert!(records.iter().all(|&r| (r as usize) < index.len()));
        pairs += records.len() as u64;
    }
    assert_eq!(pairs, built().manifest.counts.name_record_pairs);
}

#[test]
fn every_cold_record_decodes() {
    let index = &built().index;
    for i in 0..index.len() as u32 {
        let cold = index.cold(i).unwrap();
        assert!(!cold.label.is_empty(), "record {i} has no label");
    }
    assert!(index.cold(index.len() as u32).is_err());
}

#[test]
fn golden_women_are_indexed_as_women() {
    for name in [
        "Billie Eilish",
        "Kamala Harris",
        "Hillary Clinton",
        "Caitlyn Jenner",
        "Beyonce",
        "Cher",
        "Ruth Bader Ginsburg",
    ] {
        assert_eq!(only(name).1, Kind::Woman, "{name}");
    }
}

#[test]
fn display_fields_round_trip() {
    let index = &built().index;
    let record = index.exact("ruth bader ginsburg").unwrap().as_slice()[0];
    assert_eq!(index.hot(record).unwrap().qid_string(), "Q11116");
    let cold = index.cold(record).unwrap();
    assert_eq!(cold.label, "Ruth Bader Ginsburg");
    assert_eq!(cold.birth_year, Some(1933));
    assert_eq!(cold.death_year, Some(2020));
    assert_eq!(cold.enwiki_title, "Ruth Bader Ginsburg");
    assert!(!cold.description.is_empty());

    let cleopatra = index.exact("cleopatra").unwrap().as_slice()[0];
    assert_eq!(index.cold(cleopatra).unwrap().birth_year, Some(-69));
}

/// Lady Gaga has no English label at all in Wikidata, only a `mul` one, and
/// her birth name is a `mul` alias: the case `mul` indexing exists for.
#[test]
fn mul_labels_and_aliases_are_indexed() {
    let (gaga, kind) = only("Lady Gaga");
    assert_eq!(kind, Kind::Woman);
    assert_eq!(only("Stefani Germanotta").0, gaga);
    let index = &built().index;
    let record = index.exact("lady gaga").unwrap().as_slice()[0];
    assert_eq!(index.cold(record).unwrap().label, "Lady Gaga");
}

#[test]
fn rejections_are_classified() {
    assert_eq!(only("Elon Musk").1, Kind::NotWoman);
    assert_eq!(only("Hermione Granger").1, Kind::Fictional);
    // Non-binary, with deprecated male and female statements that must not count.
    assert_eq!(only("Elliot Page").1, Kind::NotWoman);
    assert_eq!(only("Sam Smith").1, Kind::NotWoman);
}

/// Wikidata's only current gender statement for Miley Cyrus is "genderfluid",
/// so she is stored as `not_woman`, and the record keeps that value so the
/// rejection can explain itself.
#[test]
fn miley_cyrus_is_not_woman_and_the_record_says_why() {
    assert_eq!(only("Miley Cyrus"), ("Q4235".to_owned(), Kind::NotWoman));
    assert_eq!(rejection_detail("Miley Cyrus"), "gender is genderfluid");
}

#[test]
fn rejected_humans_carry_their_gender_values() {
    assert_eq!(rejection_detail("Elon Musk"), "gender is male");
    assert_eq!(rejection_detail("Sam Smith"), "gender is non-binary");
    // Deprecated male and female statements are not listed.
    assert_eq!(rejection_detail("Elliot Page"), "gender is non-binary, trans man");
    // A preferred statement outranks a normal one.
    assert_eq!(rejection_detail("Demi Lovato"), "gender is non-binary");

    let counts = &built().manifest.counts;
    assert_eq!(counts.gender_values["Q18116794"], 1);
    assert!(counts.gender_values["Q6581072"] > 200);
    assert!(counts.gender_values["none"] >= 1);
    let humans = counts.women + counts.not_women;
    assert!(counts.gender_values.values().sum::<u64>() >= humans);
}

#[test]
fn non_people_are_dropped() {
    let index = &built().index;
    // "Madonna" is also an artistic theme (Q926743); only the singer is a person.
    assert_eq!(only("Madonna"), ("Q1744".to_owned(), Kind::Woman));
    // The goddess Venus is neither human nor a listed fictional class.
    assert!(index.exact("venus").is_none());
}

#[test]
fn shared_names_list_every_bearer_most_popular_first() {
    let index = &built().index;
    let hits = index.exact("jennifer").expect("several people are called just Jennifer");
    let records = hits.as_slice();
    assert!(records.len() >= 4, "only {} Jennifers", records.len());
    let sitelinks: Vec<u16> = records.iter().map(|&r| index.hot(r).unwrap().sitelinks).collect();
    assert!(sitelinks.windows(2).all(|pair| pair[0] >= pair[1]));
    // No Jennifer dominates: milestone 3's resolution rule must call this ambiguous.
    assert!(sitelinks[0] < 3 * sitelinks[1], "{sitelinks:?}");
}

#[test]
fn builds_are_deterministic() {
    let first = built();
    let dir = tempfile::tempdir().unwrap();
    let options = BuildOptions::default();
    build_index(open_dump(&fixture()).unwrap(), dir.path(), &options).unwrap();
    for name in [NAMES_FILE, POSTINGS_FILE, HOT_FILE, COLD_FILE] {
        assert_eq!(
            std::fs::read(first.path.join(name)).unwrap(),
            std::fs::read(dir.path().join(name)).unwrap(),
            "{name} differs between builds"
        );
    }
}

#[test]
fn dump_array_syntax_and_gzip_are_accepted() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let lines: Vec<String> =
        std::fs::read_to_string(fixture()).unwrap().lines().take(40).map(str::to_owned).collect();

    // The real dump wraps the entities in a JSON array, one per line.
    let dump_path = dir.path().join("wikidata-20261105-all.json.gz");
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&dump_path).unwrap(),
        flate2::Compression::fast(),
    );
    writeln!(gz, "[").unwrap();
    writeln!(gz, "{}", lines.join(",\n")).unwrap();
    writeln!(gz, "]").unwrap();
    gz.finish().unwrap();

    let plain_path = dir.path().join("plain.ndjson");
    std::fs::write(&plain_path, lines.join("\n")).unwrap();

    let options = BuildOptions::default();
    let from_dump = build_index(
        open_dump(dump_path.to_str().unwrap()).unwrap(),
        &dir.path().join("a"),
        &options,
    )
    .unwrap();
    let from_plain = build_index(
        open_dump(plain_path.to_str().unwrap()).unwrap(),
        &dir.path().join("b"),
        &options,
    )
    .unwrap();
    assert_eq!(from_dump.counts, from_plain.counts);
    assert_eq!(from_dump.counts.entities_read, 40);
    assert_eq!(from_dump.counts.parse_errors, 0);
}

#[test]
fn min_sitelinks_and_langs_change_what_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let strict = BuildOptions { min_sitelinks: 50, ..Default::default() };
    let manifest =
        build_index(open_dump(&fixture()).unwrap(), &dir.path().join("strict"), &strict).unwrap();
    assert!(manifest.counts.records < built().manifest.counts.records);
    assert!(manifest.counts.dropped_below_min_sitelinks > 0);

    // With English only, people who have just a `mul` label lose their name.
    let en_only = BuildOptions { langs: vec!["en".into()], ..Default::default() };
    let manifest =
        build_index(open_dump(&fixture()).unwrap(), &dir.path().join("en"), &en_only).unwrap();
    assert!(manifest.counts.dropped_no_name > 0);
    let index = Index::open(&dir.path().join("en")).unwrap();
    assert!(index.exact("lady gaga").is_none());
}

/// The attributes a category game ("name 100 actors", "name 100 trans
/// people") filters on are stored for every person, accepted or not.
#[test]
fn people_carry_category_attributes() {
    use Attribute::*;
    const ACTOR: u32 = 33999;
    const UNITED_STATES: u32 = 30;
    const FRANCE: u32 = 142;
    const TRANS_WOMAN: u32 = 1052281;
    const TRANS_MAN: u32 = 2449503;
    const NOBEL_PHYSICS: u32 = 38104;
    const NOBEL_CHEMISTRY: u32 = 44585;
    const TENNIS: u32 = 847;
    const US_PRESIDENT: u32 = 11696;

    let index = &built().index;
    let of = |name: &str| {
        let record = index.exact(&normalize(name)).unwrap().as_slice()[0];
        index.cold(record).unwrap().attributes
    };

    let streep = of("Meryl Streep");
    assert!(streep.has_any(Occupation, &[ACTOR]));
    assert_eq!(streep.get(Citizenship), [UNITED_STATES]);
    assert!(!streep.get(Award).is_empty());

    let curie = of("Marie Curie");
    assert!(!curie.has_any(Occupation, &[ACTOR]));
    assert!(curie.has_any(Citizenship, &[FRANCE]) && curie.get(Citizenship).len() == 2);
    assert!(curie.has_any(Award, &[NOBEL_PHYSICS]) && curie.has_any(Award, &[NOBEL_CHEMISTRY]));
    assert!(!curie.get(FieldOfWork).is_empty());

    assert!(of("Laverne Cox").has_any(Gender, &[TRANS_WOMAN]));
    assert!(of("Elliot Page").has_any(Gender, &[TRANS_MAN]));
    assert!(of("Serena Williams").has_any(Sport, &[TENNIS]));
    assert!(!of("Serena Williams").get(Event).is_empty());
    assert!(!of("Taylor Swift").get(Genre).is_empty());
    assert!(!of("Taylor Swift").get(Instrument).is_empty());
    assert!(!of("Elizabeth II").get(NobleTitle).is_empty());
    // Rejected people keep their attributes too: a different game may accept them.
    assert!(of("Tom Hanks").has_any(Occupation, &[ACTOR]));
    assert!(of("Barack Obama").has_any(Position, &[US_PRESIDENT]));

    let count = |attribute, qid| {
        (0..index.len() as u32)
            .filter(|&i| index.cold(i).unwrap().attributes.has_any(attribute, &[qid]))
            .count()
    };
    assert!(count(Occupation, ACTOR) > 100);
    assert!(count(Sport, TENNIS) >= 5);
    assert_eq!(built().manifest.attributes, Attribute::properties());
}

#[test]
fn an_index_built_with_other_attributes_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(&built().path).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    let manifest_path = dir.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["attributes"].as_array_mut().unwrap().pop();
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    let error = Index::open(dir.path()).err().expect("mismatched attributes must not load");
    assert!(error.to_string().contains("rebuild the index"), "{error}");
}
