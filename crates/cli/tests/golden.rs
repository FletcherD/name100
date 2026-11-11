//! The spec's golden cases, run against an index built from the fixture:
//! first through the matcher, then through the `hundred` binary.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use hundred_build::{build_index, open_dump, BuildOptions};
use hundred_core::{
    Attribute, Index, MatchConfig, MatchedAs, MatchedVia, Matcher, Reason, Response, Term, Verdict,
};

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
        build_index(open_dump(dump.to_str().unwrap()).unwrap(), &index_path, &BuildOptions { category_min_people: 5, ..Default::default() })
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
fn the_response_says_which_name_matched() {
    let matched = |name: &str| {
        let response = lookup(name);
        (response.matched_name.unwrap(), response.matched_as.unwrap())
    };
    // The main name, however it was typed.
    assert_eq!(matched("lady gaga"), ("Lady Gaga".to_owned(), MatchedAs::Name));
    assert_eq!(matched("BEYONCE"), ("Beyoncé".to_owned(), MatchedAs::Name));
    // An alias, as Wikidata writes it.
    assert_eq!(matched("stefani germanotta"), ("Stefani Germanotta".to_owned(), MatchedAs::Alias));
    assert_eq!(matched("rbg"), ("RBG".to_owned(), MatchedAs::Alias));
    assert_eq!(lookup("RBG").matched.unwrap().name, "Ruth Bader Ginsburg");
    // A corrected guess reports the name it was corrected to.
    assert_eq!(matched("Ruth Bader Ginsberg"), ("Ruth Bader Ginsburg".to_owned(), MatchedAs::Name));
    assert_eq!(matched("Stefani Germanota"), ("Stefani Germanotta".to_owned(), MatchedAs::Alias));
    // Rejections of a specific person report it too.
    assert_eq!(matched("Elon Musk"), ("Elon Musk".to_owned(), MatchedAs::Name));
    assert_eq!(matched("Hermione"), ("Hermione".to_owned(), MatchedAs::Alias));

    // Nothing matched one person: no matched name.
    for name in ["Jennifer", "Oh fuck off", "1234"] {
        let response = lookup(name);
        assert_eq!((response.matched_name, response.matched_as), (None, None), "{name}");
    }

    assert_eq!(
        lookup("Stefani Germanotta").summary_line().rsplit_once(" [").unwrap().1,
        "exact, alias \"Stefani Germanotta\"]"
    );
}

#[test]
fn a_match_carries_what_the_end_of_game_breakdowns_need() {
    let person = |name: &str| lookup(name).matched.unwrap_or_else(|| panic!("{name} did not resolve"));
    let main_category = |name: &str| person(name).categories.first().cloned().unwrap_or_default();

    // The main category follows the description, not the count of occupations.
    assert_eq!(main_category("Taylor Swift"), "Musician");
    assert_eq!(main_category("Meryl Streep"), "Actor");
    assert_eq!(main_category("Serena Williams"), "Athlete");
    assert_eq!(main_category("Marie Curie"), "Scientist or scholar");
    assert_eq!(main_category("Frida Kahlo"), "Artist or designer");
    assert_eq!(main_category("Hillary Clinton"), "Politician");
    assert_eq!(main_category("Kamala Harris"), "Politician");
    assert_eq!(main_category("Ruth Bader Ginsburg"), "Lawyer or judge");
    assert_eq!(main_category("Queen Victoria"), "Royalty or nobility");
    assert_eq!(main_category("Elizabeth II"), "Royalty or nobility");
    assert_eq!(main_category("Hedy Lamarr"), "Actor");
    // Rejected people are categorized too.
    assert_eq!(main_category("Elon Musk"), "Businessperson");

    let swift = person("Taylor Swift");
    assert!(swift.categories.len() > 1, "secondary categories are listed");
    assert!(swift.occupations.iter().any(|o| o == "singer-songwriter"), "{:?}", swift.occupations);
    assert_eq!(swift.citizenships, ["United States"]);
    assert_eq!(swift.genders, ["female"]);

    let curie = person("Marie Curie");
    assert!(curie.citizenships.iter().any(|c| c == "France"), "{:?}", curie.citizenships);
    assert_eq!(person("Miley Cyrus").genders, ["genderfluid"]);
    assert_eq!(person("Elliot Page").genders, ["non-binary", "trans man"]);

    // Candidates for an ambiguous name are full matches as well.
    assert!(lookup("Jennifer").candidates.iter().all(|c| !c.genders.is_empty()));
}

#[test]
fn most_people_in_the_fixture_get_a_category() {
    let matcher = &fixture().matcher;
    let index = matcher.index();
    let counts = &index.manifest().counts;
    assert!(counts.labels > 500 && counts.subclass_links > 1000, "{counts:?}");
    assert!(counts.occupations_categorized * 10 >= counts.occupations * 6, "{counts:?}");

    let mut humans = 0;
    let mut categorized = 0;
    for record in 0..index.len() as u32 {
        if index.hot(record).unwrap().kind == hundred_core::Kind::Fictional {
            continue;
        }
        let name = index.cold(record).unwrap().label;
        let response = matcher.lookup(&name).unwrap();
        let person = response.matched.or_else(|| response.candidates.into_iter().next());
        if let Some(person) = person {
            humans += 1;
            categorized += usize::from(!person.categories.is_empty());
        }
    }
    assert!(humans > 300);
    assert!(categorized * 100 >= humans * 95, "only {categorized} of {humans} people have a category");
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
        assert_eq!(response.matched.is_some(), response.matched_name.is_some(), "{name}");
        if let Some(matched) = &response.matched_name {
            assert_eq!(hundred_core::normalize(matched), name, "matched name must be the one typed");
        }
        seen += 1;
    }
    assert!(seen > 1000);
}

// ---- Category games ---------------------------------------------------

const ACTOR: Term = Term::Value { attribute: Attribute::Occupation, qid: 33999 };
const MUSICIAN: Term = Term::Value { attribute: Attribute::Occupation, qid: 639669 };
const SCIENTIST: Term = Term::Value { attribute: Attribute::Occupation, qid: 901 };
const TENNIS_PLAYER: Term = Term::Value { attribute: Attribute::Occupation, qid: 10833314 };
const FRANCE: Term = Term::Value { attribute: Attribute::Citizenship, qid: 142 };

fn play(name: &str, category: &[Term]) -> Response {
    fixture().matcher.lookup_in(name, category).unwrap()
}

#[test]
fn the_catalogue_finds_categories_by_what_a_player_types() {
    let catalogue = fixture().matcher.index().catalogue();
    let first = |query: &str| catalogue.search(query, 5).first().map(|entry| entry.term);
    assert_eq!(first("actors"), Some(ACTOR));
    assert_eq!(first("Musicians"), Some(MUSICIAN));
    assert_eq!(first("scientist"), Some(SCIENTIST));
    assert_eq!(first("tennis players"), Some(TENNIS_PLAYER));
    assert_eq!(first("france"), Some(FRANCE));
    assert_eq!(first("women"), Some(Term::Women));
    assert_eq!(first("zzzz"), None);

    // Head counts are of real people, and say how many are women.
    let actors = catalogue.get(&ACTOR).unwrap();
    assert_eq!(actors.label(), "actor");
    assert!(actors.people > 100 && actors.women > 50 && actors.women < actors.people);
    let women = catalogue.get(&Term::Women).unwrap();
    assert_eq!(women.people as u64, fixture().matcher.index().manifest().counts.women);

    // Abstract classes far up the tree are not offered.
    for junk in ["material object", "agent", "person", "entity"] {
        assert!(
            catalogue.search(junk, 50).iter().all(|entry| entry.label() != junk),
            "{junk} is in the catalogue"
        );
    }
    // Every entry is big enough and has a name.
    let floor = fixture().matcher.index().manifest().category_min_people;
    assert!(catalogue.entries().iter().all(|e| e.people >= floor && !e.label().is_empty()));
    assert!(catalogue.len() > 100);
}

#[test]
fn a_category_game_accepts_members_and_explains_the_rest() {
    let streep = play("Meryl Streep", &[ACTOR]);
    assert!(streep.is_accepted());
    assert_eq!(streep.category, ["occupation:Q33999"]);

    // A man is accepted when the category is not about gender.
    assert!(play("Tom Hanks", &[ACTOR]).is_accepted());
    assert!(play("Prince", &[MUSICIAN]).is_accepted());

    let merkel = play("Angela Merkel", &[ACTOR]);
    assert_eq!(merkel.reason, Some(Reason::NotInCategory));
    assert_eq!(merkel.reason_detail.as_deref(), Some("not in category: actor"));
    assert_eq!(merkel.matched.as_ref().unwrap().name, "Angela Merkel");
    assert_eq!(merkel.summary_line(), "✗ Angela Merkel (Q567) — not in category: actor");

    // Subclasses count: nobody in Wikidata is just a "scientist".
    assert!(play("Marie Curie", &[SCIENTIST]).is_accepted());
    assert!(play("Serena Williams", &[TENNIS_PLAYER]).is_accepted());
    assert!(play("Marie Curie", &[FRANCE]).is_accepted());

    assert_eq!(play("Hermione Granger", &[ACTOR]).reason, Some(Reason::Fictional));
    assert_eq!(play("Oh fuck off", &[ACTOR]).reason, Some(Reason::NotFound));
    assert_eq!(play("1234", &[ACTOR]).reason, Some(Reason::NotAName));
    // Typo correction works the same.
    let corrected = play("Serena Wiliams", &[TENNIS_PLAYER]);
    assert!(corrected.is_accepted());
    assert_eq!(corrected.matched_via, Some(MatchedVia::Fuzzy));
}

#[test]
fn two_categories_must_both_hold() {
    let women_scientists = [Term::Women, SCIENTIST];
    let curie = play("Marie Curie", &women_scientists);
    assert!(curie.is_accepted());
    assert_eq!(curie.category, ["women", "occupation:Q901"]);

    let einstein = play("Albert Einstein", &women_scientists);
    assert_eq!(einstein.reason, Some(Reason::NotInCategory));
    assert_eq!(einstein.reason_detail.as_deref(), Some("gender is male"));

    let swift = play("Taylor Swift", &women_scientists);
    assert_eq!(swift.reason, Some(Reason::NotInCategory));
    assert_eq!(swift.reason_detail.as_deref(), Some("not in category: scientist"));

    assert!(play("Albert Einstein", &[Term::Men, SCIENTIST]).is_accepted());
    // The explicit women category alone agrees with the original game on these.
    assert!(play("Marie Curie", &[Term::Women]).is_accepted());
    assert_eq!(play("Miley Cyrus", &[Term::Women]).reason_detail.as_deref(), Some("gender is genderfluid"));
}

#[test]
fn a_shared_name_resolves_among_the_people_in_the_category() {
    // "Prince" is borne by the musician (a man) and several obscure people.
    // In the women game the name resolves to him and is rejected...
    assert_eq!(lookup("Prince").reason, Some(Reason::NotWoman));
    // ...and among musicians it is him.
    assert_eq!(qid(&play("Prince", &[MUSICIAN])), "Q7542");

    // "Jennifer" is ambiguous overall. Among tennis players nobody is called
    // just Jennifer, so the answer names the most famous Jennifer and says
    // she is not one.
    let response = play("Jennifer", &[TENNIS_PLAYER]);
    assert_eq!(response.reason, Some(Reason::NotInCategory));
    assert!(response.candidates.is_empty());

    // Among actors several Jennifers remain and none dominates: still
    // ambiguous, and only actors are listed.
    let response = play("Jennifer", &[ACTOR]);
    assert_eq!(response.reason, Some(Reason::Ambiguous));
    assert!(!response.candidates.is_empty());
    assert!(response.candidates.len() < lookup("Jennifer").candidates.len() + 1);
    assert!(response.candidates.iter().all(|c| c.categories.iter().any(|cat| cat == "Actor")));
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
fn cli_plays_a_category_and_searches_the_catalogue() {
    let (code, stdout, _) = local_lookup("Tom Hanks", &["--category", "occupation:Q33999"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("✓ Tom Hanks"), "{stdout}");

    let (code, stdout, _) = local_lookup("Albert Einstein", &["--category", "women,occupation:Q901"]);
    assert_eq!(code, 1);
    assert!(stdout.contains("gender is male"), "{stdout}");

    let (code, _, stderr) = local_lookup("Tom Hanks", &["--category", "occupation:Q1"]);
    assert_eq!(code, 2);
    assert!(stderr.contains("unknown category: occupation:Q1"), "{stderr}");

    let index = fixture().index_path.to_str().unwrap();
    let (code, stdout, _) = hundred(&["categories", "actors", "--local", "--index", index]);
    assert_eq!(code, 0);
    let first = stdout.lines().next().unwrap();
    assert!(first.starts_with("actor ") && first.contains("occupation:Q33999"), "{first}");

    let (code, stdout, _) = hundred(&["categories", "zzzz", "--local", "--index", index]);
    assert_eq!((code, stdout.as_str()), (1, ""));
}

#[test]
fn cli_exits_2_on_error() {
    let (code, stdout, stderr) =
        hundred(&["lookup", "Cher", "--local", "--index", "/nonexistent/index"]);
    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.starts_with("error: "), "{stderr}");
}
