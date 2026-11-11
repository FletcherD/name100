//! The spec's golden cases, run against an index built from the fixture:
//! first through the matcher, then through the `hundred` binary.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{Index, MatchConfig, MatchedVia, Matcher, Reason, Response, Verdict};

struct Fixture {
    _dir: tempfile::TempDir,
    index_path: PathBuf,
    matcher: Matcher,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dump = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample.ndjson");
        let dir = tempfile::tempdir().unwrap();
        let index_path = dir.path().join("index");
        build_index(open_dump(dump.to_str().unwrap()).unwrap(), &index_path, &BuildOptions::default())
            .unwrap();
        let matcher = Matcher::new(Index::open(&index_path).unwrap(), MatchConfig::default());
        Fixture { _dir: dir, index_path, matcher }
    })
}

fn lookup(name: &str) -> Response {
    fixture().matcher.lookup(name).unwrap()
}

fn matcher_with(config: MatchConfig) -> Matcher {
    Matcher::new(Index::open(&fixture().index_path).unwrap(), config)
}

#[track_caller]
fn assert_accepted(name: &str, via: MatchedVia) -> Response {
    let response = lookup(name);
    assert_eq!(response.verdict, Verdict::Accepted, "{name}: {}", response.summary_line());
    assert_eq!(response.reason, None, "{name}");
    assert_eq!(response.reason_detail, None, "{name}");
    assert_eq!(response.matched_via, Some(via), "{name}");
    assert!(response.matched.is_some() && response.candidates.is_empty(), "{name}");
    response
}

#[track_caller]
fn assert_rejected(name: &str, reason: Reason) -> Response {
    let response = lookup(name);
    assert_eq!(response.verdict, Verdict::Rejected, "{name}: {}", response.summary_line());
    assert_eq!(response.reason, Some(reason), "{name}: {}", response.summary_line());
    response
}

fn qid(response: &Response) -> &str {
    &response.matched.as_ref().unwrap().qid
}

// ---- The golden table -------------------------------------------------

#[test]
fn famous_women_are_accepted_exactly() {
    for name in ["Billie Eilish", "Kamala Harris", "Hillary Clinton", "Caitlyn Jenner"] {
        let response = assert_accepted(name, MatchedVia::Exact);
        assert_eq!(response.corrected_from, None, "{name}");
        assert_eq!(response.matched.as_ref().unwrap().name, name);
    }
}

#[test]
fn miley_cyrus_is_rejected_with_the_reason_spelled_out() {
    let response = assert_rejected("Miley Cyrus", Reason::NotWoman);
    assert_eq!(response.reason_detail.as_deref(), Some("gender is genderfluid"));
    assert_eq!(qid(&response), "Q4235");
    assert_eq!(response.summary_line(), "✗ Miley Cyrus (Q4235) — not a woman: gender is genderfluid");
}

#[test]
fn beyonce_without_the_accent_is_an_exact_match() {
    let response = assert_accepted("Beyonce", MatchedVia::Exact);
    assert_eq!(response.normalized, "beyonce");
    assert_eq!(response.matched.as_ref().unwrap().name, "Beyoncé");
    assert_eq!(qid(&response), qid(&lookup("BEYONCÉ")));
}

#[test]
fn dominant_mononyms_are_accepted() {
    assert_eq!(qid(&assert_accepted("Madonna", MatchedVia::Exact)), "Q1744");
    assert_eq!(qid(&assert_accepted("Cher", MatchedVia::Exact)), "Q12003");
}

#[test]
fn a_misspelling_is_corrected() {
    let response = assert_accepted("Ruth Bader Ginsberg", MatchedVia::Fuzzy);
    assert_eq!(response.normalized, "ruth bader ginsberg");
    assert_eq!(response.corrected_from.as_deref(), Some("ruth bader ginsberg"));
    let person = response.matched.as_ref().unwrap();
    assert_eq!(person.qid, "Q11116");
    assert_eq!(person.name, "Ruth Bader Ginsburg");
    assert_eq!((person.birth_year, person.death_year), (Some(1933), Some(2020)));
    assert_eq!(
        person.wikipedia_url.as_deref(),
        Some("https://en.wikipedia.org/wiki/Ruth_Bader_Ginsburg")
    );
    assert!(response.summary_line().starts_with("✓ Ruth Bader Ginsburg (Q11116) — "));
    assert!(response.summary_line().contains("1933–2020"));
    assert!(response.summary_line().ends_with(" [fuzzy, corrected]"));
}

#[test]
fn stage_name_and_birth_name_are_the_same_person() {
    let gaga = assert_accepted("Lady Gaga", MatchedVia::Exact);
    let germanotta = assert_accepted("Stefani Germanotta", MatchedVia::Exact);
    assert_eq!(qid(&gaga), qid(&germanotta));
    assert_eq!(germanotta.matched.as_ref().unwrap().name, "Lady Gaga");
}

#[test]
fn a_man_is_rejected_as_not_a_woman() {
    let response = assert_rejected("Elon Musk", Reason::NotWoman);
    assert_eq!(response.reason_detail.as_deref(), Some("gender is male"));
    assert_eq!(response.matched_via, Some(MatchedVia::Exact));
    assert_eq!(response.summary_line(), "✗ Elon Musk (Q317521) — not a woman: gender is male");
}

#[test]
fn a_character_is_rejected_as_fictional() {
    let response = assert_rejected("Hermione Granger", Reason::Fictional);
    assert_eq!(response.reason_detail, None);
    assert_eq!(response.summary_line(), "✗ Hermione Granger (Q174009) — fictional");
}

#[test]
fn a_shared_first_name_is_ambiguous_and_lists_candidates() {
    let response = assert_rejected("Jennifer", Reason::Ambiguous);
    assert_eq!(response.matched, None);
    assert_eq!(response.matched_via, Some(MatchedVia::Exact));
    assert_eq!(response.candidates.len(), 5, "capped at five");
    let sitelinks: Vec<u32> = response.candidates.iter().map(|c| c.sitelinks).collect();
    assert!(sitelinks.windows(2).all(|pair| pair[0] >= pair[1]), "most famous first");
    assert!(response.summary_line().starts_with("✗ \"Jennifer\" — ambiguous: "));
}

#[test]
fn garbage_is_not_found() {
    let response = assert_rejected("Oh fuck off", Reason::NotFound);
    assert_eq!(response.normalized, "oh fuck off");
    assert_eq!(response.matched_via, None);
    assert_eq!(response.corrected_from, None);
    assert_eq!(response.matched, None);
    assert_eq!(response.summary_line(), "✗ \"Oh fuck off\" — not found");
}

#[test]
fn input_without_letters_is_not_a_name() {
    for input in ["1234", "", "   ", "?!", "...", &"a".repeat(101)] {
        let response = assert_rejected(input, Reason::NotAName);
        assert_eq!(response.matched_via, None, "{input:?}");
    }
    // 100 characters is still looked up.
    assert_rejected(&"a".repeat(100), Reason::NotFound);
}

// ---- Rules behind the table -------------------------------------------

#[test]
fn other_rejections_explain_the_gender() {
    let detail = |name: &str| assert_rejected(name, Reason::NotWoman).reason_detail.unwrap();
    assert_eq!(detail("Sam Smith"), "gender is non-binary");
    assert_eq!(detail("Elliot Page"), "gender is non-binary, trans man");
}

#[test]
fn typo_correction_is_one_edit_and_needs_five_characters() {
    // Under 5 characters: no correction. "Cher" is 4.
    assert_rejected("Chir", Reason::NotFound);
    // One edit, at any length from 5 up.
    assert_eq!(qid(&assert_accepted("Madona", MatchedVia::Fuzzy)), "Q1744");
    assert_eq!(qid(&assert_accepted("Hilary Clinton", MatchedVia::Fuzzy)), qid(&lookup("Hillary Clinton")));
    // Never two, however long the name.
    assert_rejected("Maddona", Reason::NotFound);
    assert_rejected("Hilary Clintom", Reason::NotFound);
    assert_rejected("Alexandria Ocasio Kortes", Reason::NotFound);
}

#[test]
fn typos_are_only_corrected_towards_well_known_people() {
    let index = fixture().matcher.index();
    assert_eq!(index.manifest().fuzzy_min_sitelinks, 10);

    // Jennifer Lanvin has 2 articles: found when typed exactly, never by correction.
    let exact = lookup("Jennifer Lanvin");
    assert_eq!(exact.matched_via, Some(MatchedVia::Exact));
    assert!(exact.matched.as_ref().unwrap().sitelinks < 10);
    assert!(index.fuzzy_names().len() < index.names().len());
    assert!(!index.fuzzy_names().contains("jennifer lanvin"));
    assert_rejected("Jennifer Lanvim", Reason::NotFound);

    // Jennifer Warnes has about 30: her typo is corrected.
    assert!(index.fuzzy_names().contains("jennifer warnes"));
    let corrected = assert_accepted("Jennifer Warnez", MatchedVia::Fuzzy);
    assert!(corrected.matched.as_ref().unwrap().sitelinks >= 10);

    // Every correctable name is a real name with a well-known bearer.
    use fst::Streamer;
    let mut stream = index.fuzzy_names().stream();
    let mut seen = 0u64;
    while let Some(name) = stream.next() {
        let hits = index.exact(std::str::from_utf8(name).unwrap()).expect("also in the main index");
        let top = index.hot(hits.as_slice()[0]).unwrap();
        assert!(top.sitelinks >= 10);
        seen += 1;
    }
    assert_eq!(seen, index.manifest().counts.fuzzy_names);
}

#[test]
fn an_exact_hit_is_never_overridden_by_a_fuzzy_one() {
    // "Selena" is one edit from "Serena" names but matches exactly.
    let response = assert_accepted("Selena", MatchedVia::Exact);
    assert_eq!(response.corrected_from, None);
}

#[test]
fn a_typo_of_a_mans_name_is_still_a_rejection() {
    let response = assert_rejected("Elon Muskk", Reason::NotWoman);
    assert_eq!(response.matched_via, Some(MatchedVia::Fuzzy));
    assert_eq!(response.corrected_from.as_deref(), Some("elon muskk"));
    assert_eq!(qid(&response), "Q317521");
}

#[test]
fn a_name_shared_with_a_far_more_famous_man_resolves_to_him() {
    // Several people are called just "Prince"; the musician dominates.
    let response = assert_rejected("Prince", Reason::NotWoman);
    assert_eq!(qid(&response), "Q7542");
}

#[test]
fn dominance_ratio_and_mononym_threshold_are_configurable() {
    // With no dominance required, the most famous Jennifer would win, but she
    // still falls short of the mononym threshold.
    let lenient = matcher_with(MatchConfig { dominance_ratio: 1.0, ..Default::default() });
    assert_eq!(lenient.lookup("Jennifer").unwrap().reason, Some(Reason::Ambiguous));

    let no_threshold = matcher_with(MatchConfig {
        dominance_ratio: 1.0,
        mononym_min_sitelinks: 0,
        ..Default::default()
    });
    assert_ne!(no_threshold.lookup("Jennifer").unwrap().reason, Some(Reason::Ambiguous));

    // A mononym threshold above everyone makes even Madonna ambiguous, with
    // her as the only candidate.
    let strict = matcher_with(MatchConfig { mononym_min_sitelinks: 10_000, ..Default::default() });
    let madonna = strict.lookup("Madonna").unwrap();
    assert_eq!(madonna.reason, Some(Reason::Ambiguous));
    assert_eq!(madonna.candidates.len(), 1);
    // Full names are not subject to it.
    assert!(strict.lookup("Kamala Harris").unwrap().is_accepted());

    // A huge dominance ratio makes any shared name ambiguous.
    let picky = matcher_with(MatchConfig { dominance_ratio: 1000.0, ..Default::default() });
    assert_eq!(picky.lookup("Prince").unwrap().reason, Some(Reason::Ambiguous));
}

#[test]
fn swapped_letters_count_as_one_edit_unless_turned_off() {
    let response = assert_accepted("Madnona", MatchedVia::Fuzzy);
    assert_eq!(qid(&response), "Q1744");
    assert_eq!(qid(&assert_accepted("Billei Eilish", MatchedVia::Fuzzy)), qid(&lookup("Billie Eilish")));

    let strict = matcher_with(MatchConfig { fuzzy_transpositions: false, ..Default::default() });
    assert_eq!(strict.lookup("Madnona").unwrap().reason, Some(Reason::NotFound));
}

#[test]
fn every_indexed_name_gets_a_verdict_without_error() {
    use fst::Streamer;
    let matcher = &fixture().matcher;
    let mut stream = matcher.index().names().stream();
    let mut seen = 0;
    while let Some((name, _)) = stream.next() {
        let name = std::str::from_utf8(name).unwrap();
        let response = matcher.lookup(name).unwrap();
        if name.chars().count() > 100 {
            // Picasso's full baptismal name is indexed but too long to type.
            assert_eq!(response.reason, Some(Reason::NotAName));
            continue;
        }
        assert_eq!(response.normalized, name);
        assert_eq!(response.matched_via, Some(MatchedVia::Exact), "{name}");
        assert_eq!(response.reason == Some(Reason::Ambiguous), !response.candidates.is_empty());
        seen += 1;
    }
    assert!(seen > 1000);
}

// ---- The binary ---------------------------------------------------------

fn hundred(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_hundred")).args(args).output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn local_lookup(name: &str, extra: &[&str]) -> (i32, String, String) {
    let index = fixture().index_path.to_str().unwrap();
    let mut args = vec!["lookup", name, "--local", "--index", index];
    args.extend_from_slice(extra);
    hundred(&args)
}

#[test]
fn cli_prints_one_line_and_exits_0_when_accepted() {
    let (code, stdout, _) = local_lookup("Ruth Bader Ginsberg", &[]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("✓ Ruth Bader Ginsburg (Q11116) — "), "{stdout}");
    assert!(stdout.trim_end().ends_with("[fuzzy, corrected]"), "{stdout}");
    assert_eq!(stdout.lines().count(), 1);
}

#[test]
fn cli_exits_1_when_rejected() {
    let (code, stdout, _) = local_lookup("Elon Musk", &[]);
    assert_eq!(code, 1);
    assert_eq!(stdout.trim_end(), "✗ Elon Musk (Q317521) — not a woman: gender is male");
    assert_eq!(local_lookup("1234", &[]).0, 1);
    assert_eq!(local_lookup("Jennifer", &[]).0, 1);
}

#[test]
fn cli_json_is_the_full_response() {
    let (code, stdout, _) = local_lookup("Miley Cyrus", &["--json"]);
    assert_eq!(code, 1);
    let response: Response = serde_json::from_str(&stdout).unwrap();
    assert_eq!(response.reason, Some(Reason::NotWoman));
    assert_eq!(response.reason_detail.as_deref(), Some("gender is genderfluid"));
    assert_eq!(response.input, "Miley Cyrus");
}

#[test]
fn cli_exits_2_on_error() {
    let (code, stdout, stderr) =
        hundred(&["lookup", "Cher", "--local", "--index", "/nonexistent/index"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.starts_with("error: "), "{stderr}");
}
