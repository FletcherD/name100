//! Broad occupation categories ("Musician", "Politician") for the game's
//! end-of-round breakdowns.
//!
//! Wikidata has thousands of occupations. Each one a person holds is mapped,
//! when the index is built, to the nearest of these categories by walking up
//! Wikidata's "subclass of" tree from the occupation to one of the anchors:
//! singer-songwriter → singer → musician. The map is stored in the artifact,
//! and the manifest records this list so a mismatched index is refused.

pub struct Category {
    pub label: &'static str,
    /// Numeric QIDs of the occupation classes that define the category.
    pub anchors: &'static [u32],
}

/// In priority order: when an occupation is equally close to two categories,
/// the earlier one wins. A category's position is its on-disk id.
pub const CATEGORIES: &[Category] = &[
    // musician, music artist
    Category { label: "Musician", anchors: &[639669, 1294626] },
    Category { label: "Actor", anchors: &[33999] },
    // athlete, sportsperson, competitive player
    Category { label: "Athlete", anchors: &[2066131, 50995749, 18536342] },
    Category { label: "Politician", anchors: &[82955] },
    Category { label: "Writer", anchors: &[36180] },
    Category { label: "Journalist", anchors: &[1930187] },
    // scientist (humanities scholars, social scientists and academics are subclasses)
    Category { label: "Scientist or scholar", anchors: &[901] },
    // visual artist, designer
    Category { label: "Artist or designer", anchors: &[3391743, 5322166] },
    // filmmaker, director, producer
    Category { label: "Filmmaker", anchors: &[1414443, 3455803, 47541952] },
    Category { label: "Model", anchors: &[4610556] },
    // presenter, television presenter
    Category { label: "Presenter", anchors: &[13590141, 947873] },
    Category { label: "Comedian", anchors: &[245068] },
    Category { label: "Dancer", anchors: &[5716684] },
    Category { label: "Businessperson", anchors: &[43845] },
    // cleric, religious person, religious role
    Category { label: "Religious figure", anchors: &[2259532, 3111753, 4504549] },
    Category { label: "Activist", anchors: &[15253558] },
    // monarch, aristocrat, sovereign
    Category { label: "Royalty or nobility", anchors: &[116, 2478141, 2304859] },
    Category { label: "Military", anchors: &[47064] },
    // jurist
    Category { label: "Lawyer or judge", anchors: &[185351] },
    // health professional
    Category { label: "Health professional", anchors: &[11974939] },
    // educator, educational personnel
    Category { label: "Educator", anchors: &[974144, 93918935] },
    Category { label: "Engineer", anchors: &[81096] },
    // explorer, astronaut
    Category { label: "Explorer or astronaut", anchors: &[11900058, 11631] },
];

/// The id of "Royalty or nobility", given to people who hold a noble title
/// but no occupation that maps to a category.
pub const ROYALTY: u8 = 16;

/// The id of "Politician".
pub const POLITICIAN: u8 = 3;

/// Words in a description that point at a category even when they are not
/// the label of one of the person's occupations: Wikidata lists "actor"
/// where the description says "actress". They only count for a category
/// the person's occupations already put them in.
const KEYWORDS: &[(&str, u8)] = &[
    ("singer", 0),
    ("musician", 0),
    ("rapper", 0),
    ("actress", 1),
    ("actor", 1),
    ("footballer", 2),
    ("player", 2),
    ("politician", 3),
    ("author", 4),
    ("writer", 4),
    ("novelist", 4),
    ("poet", 4),
    ("journalist", 5),
    ("comedienne", 11),
    ("businesswoman", 13),
    ("businessman", 13),
    ("entrepreneur", 13),
    ("queen", ROYALTY),
    ("king", ROYALTY),
    ("empress", ROYALTY),
    ("emperor", ROYALTY),
    ("princess", ROYALTY),
    ("prince", ROYALTY),
];

/// How far up the subclass tree an occupation is followed.
pub const MAX_DEPTH: usize = 8;

pub fn category_label(id: u8) -> Option<&'static str> {
    CATEGORIES.get(id as usize).map(|category| category.label)
}

/// The category an anchor QID belongs to.
pub fn anchor_category(qid: u32) -> Option<u8> {
    CATEGORIES.iter().position(|category| category.anchors.contains(&qid)).map(|id| id as u8)
}

/// Every category label, in id order: recorded in the manifest.
pub fn category_labels() -> Vec<String> {
    CATEGORIES.iter().map(|category| category.label.to_owned()).collect()
}

/// A person's categories, the main one first.
///
/// `occupations` is each occupation's English label (empty if unknown) and
/// category (`None` if unmapped), in Wikidata's order. Wikidata does not say
/// which occupation is someone's main one, so the first category is chosen by:
///
/// 1. the occupation named earliest in the person's description ("American
///    politician and diplomat" makes Hillary Clinton a Politician, though
///    more of her listed occupations are legal ones);
/// 2. else Royalty, for anyone with a royal occupation (Queen Victoria also
///    painted and kept a diary);
/// 3. else Politician, for a politician who has held an office (Kamala
///    Harris has more legal occupations listed than political ones);
/// 4. else whichever category most of their occupations fall under.
///
/// The rest follow by how many occupations they cover.
pub fn rank_categories(
    description: &str,
    occupations: &[(&str, Option<u8>)],
    held_office: bool,
) -> Vec<u8> {
    let mut tally: Vec<(u8, usize)> = Vec::new();
    for category in occupations.iter().filter_map(|(_, category)| *category) {
        match tally.iter_mut().find(|(id, _)| *id == category) {
            Some((_, count)) => *count += 1,
            None => tally.push((category, 1)),
        }
    }
    // Stable sort: equal counts keep first-seen order.
    tally.sort_by(|a, b| b.1.cmp(&a.1));
    let mut ranked: Vec<u8> = tally.into_iter().map(|(id, _)| id).collect();

    let main = named_in_description(description, occupations, &ranked)
        .or_else(|| ranked.contains(&ROYALTY).then_some(ROYALTY))
        .or_else(|| (held_office && ranked.contains(&POLITICIAN)).then_some(POLITICIAN));
    if let Some(main) = main {
        ranked.retain(|&id| id != main);
        ranked.insert(0, main);
    }
    ranked
}

/// The category of the occupation whose label appears earliest in
/// `description` as whole words; the longer label wins a shared position
/// ("film director" over "director").
fn named_in_description(
    description: &str,
    occupations: &[(&str, Option<u8>)],
    categories: &[u8],
) -> Option<u8> {
    let description = description.to_lowercase();
    let labels = occupations
        .iter()
        .filter_map(|&(label, category)| Some((label.to_lowercase(), category?)))
        .filter(|(label, _)| !label.is_empty());
    let keywords = KEYWORDS
        .iter()
        .filter(|(_, category)| categories.contains(category))
        .map(|&(word, category)| (word.to_owned(), category));

    // (position, longest word first, category)
    let mut best: Option<(usize, usize, u8)> = None;
    for (word, category) in labels.chain(keywords) {
        let Some(at) = find_words(&description, &word) else {
            continue;
        };
        let better = match best {
            None => true,
            Some((best_at, best_len, _)) => at < best_at || (at == best_at && word.len() > best_len),
        };
        if better {
            best = Some((at, word.len(), category));
        }
    }
    best.map(|(_, _, category)| category)
}

/// Where `needle` first occurs in `haystack` without a letter or digit on either side.
fn find_words(haystack: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let before = haystack[..start].chars().next_back();
        let after = haystack[end..].chars().next();
        if !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric) {
            return Some(start);
        }
        from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_unique_and_royalty_points_at_royalty() {
        let mut anchors: Vec<u32> = CATEGORIES.iter().flat_map(|c| c.anchors.iter().copied()).collect();
        let total = anchors.len();
        anchors.sort_unstable();
        anchors.dedup();
        assert_eq!(anchors.len(), total, "an anchor is listed twice");
        assert_eq!(category_label(ROYALTY), Some("Royalty or nobility"));
        assert_eq!(category_label(POLITICIAN), Some("Politician"));
        assert_eq!(anchor_category(639669).and_then(category_label), Some("Musician"));
        assert_eq!(anchor_category(5), None);
    }

    const MUSICIAN: u8 = 0;
    const ACTOR: u8 = 1;
    const WRITER: u8 = 4;
    const ARTIST: u8 = 7;
    const FILMMAKER: u8 = 8;
    const LAWYER: u8 = 18;

    #[test]
    fn without_a_hint_the_most_common_category_leads() {
        let occupations = [
            ("actor", Some(ACTOR)),
            ("singer", Some(MUSICIAN)),
            ("socialite", None),
            ("guitarist", Some(MUSICIAN)),
            ("writer", Some(WRITER)),
        ];
        assert_eq!(rank_categories("American celebrity", &occupations, false), [MUSICIAN, ACTOR, WRITER]);
        assert!(rank_categories("someone", &[("socialite", None)], false).is_empty());
        assert!(rank_categories("someone", &[], false).is_empty());
    }

    #[test]
    fn the_description_names_the_main_occupation() {
        let occupations = [
            ("lawyer", Some(LAWYER)),
            ("jurist", Some(LAWYER)),
            ("politician", Some(POLITICIAN)),
            ("writer", Some(WRITER)),
        ];
        assert_eq!(
            rank_categories("American politician and diplomat (born 1947)", &occupations, false),
            [POLITICIAN, LAWYER, WRITER]
        );
        // Earliest mention wins, not the first occupation listed.
        assert_eq!(
            rank_categories("Writer, lawyer and Politician", &occupations, true),
            [WRITER, LAWYER, POLITICIAN]
        );
    }

    #[test]
    fn labels_match_whole_words_and_longer_labels_win() {
        let occupations = [("director", Some(ACTOR)), ("film director", Some(FILMMAKER))];
        assert_eq!(rank_categories("American film director", &occupations, false)[0], FILMMAKER);
        // "art" is not in "artist", and "painter" is not in "painters".
        let occupations = [("art", Some(WRITER)), ("painter", Some(ARTIST)), ("poet", Some(MUSICIAN))];
        assert_eq!(rank_categories("artist among painters; poet", &occupations, false)[0], MUSICIAN);
    }

    #[test]
    fn an_office_holder_who_is_a_politician_is_one_first() {
        let occupations =
            [("lawyer", Some(LAWYER)), ("prosecutor", Some(LAWYER)), ("politician", Some(POLITICIAN))];
        let description = "Vice President of the United States from 2021 to 2025";
        assert_eq!(rank_categories(description, &occupations, true), [POLITICIAN, LAWYER]);
        assert_eq!(rank_categories(description, &occupations, false), [LAWYER, POLITICIAN]);
        // Holding an office does not make a non-politician one.
        assert_eq!(rank_categories(description, &occupations[..2], true), [LAWYER]);
    }

    #[test]
    fn descriptions_may_use_another_word_for_the_occupation() {
        let occupations = [("inventor", Some(WRITER)), ("actor", Some(ACTOR))];
        assert_eq!(
            rank_categories("Austrian-American actress and inventor", &occupations, false)[0],
            ACTOR
        );
        // "businessman" points at Businessperson, which "investor" put him in.
        let occupations = [("politician", Some(POLITICIAN)), ("investor", Some(13))];
        assert_eq!(rank_categories("American businessman", &occupations, true)[0], 13);
        // A keyword never adds a category the occupations do not support.
        assert_eq!(rank_categories("King of Pop", &[("singer", Some(MUSICIAN))], false), [MUSICIAN]);
    }

    #[test]
    fn keyword_ids_match_their_categories() {
        let label = |word: &str| {
            KEYWORDS.iter().find(|(w, _)| *w == word).and_then(|&(_, id)| category_label(id))
        };
        assert_eq!(label("singer"), Some("Musician"));
        assert_eq!(label("actress"), Some("Actor"));
        assert_eq!(label("player"), Some("Athlete"));
        assert_eq!(label("politician"), Some("Politician"));
        assert_eq!(label("author"), Some("Writer"));
        assert_eq!(label("journalist"), Some("Journalist"));
        assert_eq!(label("comedienne"), Some("Comedian"));
        assert_eq!(label("businesswoman"), Some("Businessperson"));
        assert_eq!(label("queen"), Some("Royalty or nobility"));
    }

    #[test]
    fn royalty_leads_unless_the_description_says_otherwise() {
        let occupations =
            [("painter", Some(ARTIST)), ("diarist", Some(WRITER)), ("writer", Some(WRITER)), ("monarch", Some(ROYALTY))];
        assert_eq!(
            rank_categories("Queen of the United Kingdom from 1837 to 1901", &occupations, true),
            [ROYALTY, WRITER, ARTIST]
        );
        assert_eq!(rank_categories("English painter", &occupations, true)[0], ARTIST);
    }
}
