//! The catalogue of playable categories ("name 100 actors").
//!
//! A category is one value of one attribute: occupation *actor*, citizenship
//! *France*, sport *tennis*. A person belongs to it if they hold that value
//! or a more specific one: a *film actor* is an *actor*. "More specific" comes
//! from Wikidata's class tree, flattened when the index is built into a list
//! of ancestors per value.
//!
//! The builder puts in the catalogue every category with enough people to
//! play, with its English name, other names it goes by, and head counts.
//! `women` and `men` are built in.

use std::collections::HashMap;

use crate::artifact::{ColdRecord, HotRecord, Kind};
use crate::attribute::Attribute;
use crate::gender::WOMAN_GENDERS;
use crate::normalize::normalize;

/// `P21` values that put a human in the built-in `men` category.
pub const MAN_GENDERS: &[u32] = &[
    6581097,   // male
    2449503,   // trans man
    15145778,  // cisgender man
    121307094, // intersex man
    138806924, // intersex trans man
];

/// Most terms one game may combine ("women" and "scientist").
pub const MAX_TERMS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Term {
    Women,
    Men,
    /// People holding `qid`, or something more specific, for `attribute`.
    Value { attribute: Attribute, qid: u32 },
}

impl Term {
    /// The stable id used in URLs: `women`, `occupation:Q33999`.
    pub fn id(&self) -> String {
        match self {
            Term::Women => "women".to_owned(),
            Term::Men => "men".to_owned(),
            Term::Value { attribute, qid } => format!("{}:Q{qid}", attribute.slug()),
        }
    }

    pub fn parse(id: &str) -> Option<Term> {
        match id {
            "women" => Some(Term::Women),
            "men" => Some(Term::Men),
            _ => {
                let (slug, qid) = id.split_once(":Q")?;
                let attribute = Attribute::ALL.into_iter().find(|a| a.slug() == slug)?;
                Some(Term::Value { attribute, qid: qid.parse().ok()? })
            }
        }
    }

    /// What kind of thing the category is, for display: "occupation", "sport".
    pub fn kind(&self) -> &'static str {
        match self {
            Term::Women | Term::Men => "gender",
            Term::Value { attribute, .. } => attribute.slug(),
        }
    }
}

/// One playable category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub term: Term,
    /// Real people in the category.
    pub people: u32,
    /// How many of them are women.
    pub women: u32,
    /// The English name first, then other names it is known by.
    pub names: Vec<String>,
}

impl Entry {
    pub fn label(&self) -> &str {
        self.names.first().map_or("", String::as_str)
    }
}

/// What the builder hands over to be written: the entries, and for each
/// attribute the ancestors of each value (itself excluded), sorted by value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogueData {
    pub entries: Vec<Entry>,
    pub ancestors: Vec<(Attribute, u32, Vec<u32>)>,
}

const TAG_WOMEN: u8 = 254;
const TAG_MEN: u8 = 255;

impl CatalogueData {
    /// Layout: `u32` entry count, then per entry a tag byte (attribute index,
    /// or 254 women, 255 men), `u32` QID, `u32` people, `u32` women, a `u8`
    /// name count and the names (`u16` length, UTF-8). Then a `u32` count of
    /// ancestor lists, each an attribute byte, `u32` value, `u16` count and
    /// that many `u32`s.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            let (tag, qid) = match entry.term {
                Term::Women => (TAG_WOMEN, 0),
                Term::Men => (TAG_MEN, 0),
                Term::Value { attribute, qid } => (attribute as u8, qid),
            };
            out.push(tag);
            out.extend_from_slice(&qid.to_le_bytes());
            out.extend_from_slice(&entry.people.to_le_bytes());
            out.extend_from_slice(&entry.women.to_le_bytes());
            let names = &entry.names[..entry.names.len().min(u8::MAX as usize)];
            out.push(names.len() as u8);
            for name in names {
                let bytes = &name.as_bytes()[..name.len().min(u16::MAX as usize)];
                out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(&(self.ancestors.len() as u32).to_le_bytes());
        for (attribute, value, ancestors) in &self.ancestors {
            let ancestors = &ancestors[..ancestors.len().min(u16::MAX as usize)];
            out.push(*attribute as u8);
            out.extend_from_slice(&value.to_le_bytes());
            out.extend_from_slice(&(ancestors.len() as u16).to_le_bytes());
            for ancestor in ancestors {
                out.extend_from_slice(&ancestor.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<CatalogueData> {
        let mut pos = 0;
        let mut take = |n: usize| -> Option<&[u8]> {
            let slice = bytes.get(pos..pos + n)?;
            pos += n;
            Some(slice)
        };
        let u32_of = |raw: &[u8]| u32::from_le_bytes(raw.try_into().unwrap());
        let u16_of = |raw: &[u8]| u16::from_le_bytes(raw.try_into().unwrap());

        let entry_count = u32_of(take(4)?);
        let mut entries = Vec::new();
        for _ in 0..entry_count {
            let tag = take(1)?[0];
            let qid = u32_of(take(4)?);
            let term = match tag {
                TAG_WOMEN => Term::Women,
                TAG_MEN => Term::Men,
                index => Term::Value { attribute: *Attribute::ALL.get(index as usize)?, qid },
            };
            let people = u32_of(take(4)?);
            let women = u32_of(take(4)?);
            let name_count = take(1)?[0];
            let mut names = Vec::with_capacity(name_count as usize);
            for _ in 0..name_count {
                let len = u16_of(take(2)?) as usize;
                names.push(std::str::from_utf8(take(len)?).ok()?.to_owned());
            }
            entries.push(Entry { term, people, women, names });
        }
        let list_count = u32_of(take(4)?);
        let mut ancestors = Vec::new();
        for _ in 0..list_count {
            let attribute = *Attribute::ALL.get(take(1)?[0] as usize)?;
            let value = u32_of(take(4)?);
            let count = u16_of(take(2)?) as usize;
            let list = take(4 * count)?.chunks_exact(4).map(u32_of).collect();
            ancestors.push((attribute, value, list));
        }
        (pos == bytes.len()).then_some(CatalogueData { entries, ancestors })
    }
}

/// The catalogue, loaded for searching and for testing membership.
pub struct Catalogue {
    entries: Vec<Entry>,
    /// Each entry's names, normalized for searching.
    search_names: Vec<Vec<String>>,
    by_term: HashMap<Term, usize>,
    ancestors: HashMap<(Attribute, u32), Vec<u32>>,
}

impl Catalogue {
    pub fn new(data: CatalogueData) -> Catalogue {
        let search_names = data
            .entries
            .iter()
            .map(|entry| entry.names.iter().map(|name| normalize(name)).collect())
            .collect();
        let by_term = data.entries.iter().enumerate().map(|(i, entry)| (entry.term, i)).collect();
        let ancestors = data
            .ancestors
            .into_iter()
            .map(|(attribute, value, list)| ((attribute, value), list))
            .collect();
        Catalogue { entries: data.entries, search_names, by_term, ancestors }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry for a term, if it is a playable category.
    pub fn get(&self, term: &Term) -> Option<&Entry> {
        self.by_term.get(term).map(|&index| &self.entries[index])
    }

    /// Categories matching what a player typed, best first: an exact name,
    /// then names starting with it, then names with a word starting with it,
    /// then names containing it; the bigger category first within each, and
    /// a category's main name ahead of its other names. A
    /// plural finds its singular ("actors" finds "actor"). An empty query
    /// lists the biggest categories.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&Entry> {
        let query = normalize(query);
        let variants = singulars(&query);
        let mut scored: Vec<(u8, &Entry)> = self
            .entries
            .iter()
            .zip(&self.search_names)
            .filter_map(|(entry, names)| {
                // The category's own name counts for more than its other names.
                let best = names
                    .iter()
                    .enumerate()
                    .flat_map(|(position, name)| {
                        variants.iter().filter_map(move |variant| {
                            Some(score(name, variant)? * 2 + u8::from(position > 0))
                        })
                    })
                    .min()?;
                Some((best, entry))
            })
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.people.cmp(&a.1.people)));
        scored.into_iter().take(limit).map(|(_, entry)| entry).collect()
    }

    /// Whether a person is in the category `term`.
    pub fn contains(&self, term: &Term, hot: HotRecord, cold: &ColdRecord) -> bool {
        if hot.kind == Kind::Fictional {
            return false;
        }
        let genders = cold.attributes.get(Attribute::Gender);
        match *term {
            Term::Women => genders.iter().any(|g| WOMAN_GENDERS.contains(g)),
            Term::Men => genders.iter().any(|g| MAN_GENDERS.contains(g)),
            Term::Value { attribute, qid } => cold.attributes.get(attribute).iter().any(|&held| {
                held == qid
                    || self
                        .ancestors
                        .get(&(attribute, held))
                        .is_some_and(|list| list.binary_search(&qid).is_ok())
            }),
        }
    }
}

/// How well `name` answers `query`; lower is better, `None` is no match.
fn score(name: &str, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.split(' ').any(|word| word.starts_with(query)) {
        Some(2)
    } else if name.contains(query) {
        Some(3)
    } else {
        None
    }
}

/// `query` and its plausible singular forms: "actresses" → "actress",
/// "tennis players" → "tennis player", "celebrities" → "celebrity".
fn singulars(query: &str) -> Vec<String> {
    let mut out = vec![query.to_owned()];
    if let Some(stem) = query.strip_suffix("ies") {
        out.push(format!("{stem}y"));
    }
    if let Some(stem) = query.strip_suffix("es") {
        out.push(stem.to_owned());
    }
    if let Some(stem) = query.strip_suffix('s') {
        out.push(stem.to_owned());
    }
    out.retain(|variant| !variant.is_empty() || query.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::Attributes;

    const ACTOR: u32 = 33999;
    const FILM_ACTOR: u32 = 10800557;
    const ARTIST: u32 = 483501;
    const FRANCE: u32 = 142;

    fn occupation(qid: u32) -> Term {
        Term::Value { attribute: Attribute::Occupation, qid }
    }

    fn entry(term: Term, people: u32, names: &[&str]) -> Entry {
        Entry { term, people, women: people / 3, names: names.iter().map(|n| n.to_string()).collect() }
    }

    fn data() -> CatalogueData {
        CatalogueData {
            entries: vec![
                entry(Term::Women, 900, &["women"]),
                entry(occupation(ACTOR), 500, &["actor", "actress"]),
                entry(occupation(ARTIST), 800, &["artist"]),
                entry(occupation(1), 300, &["activist"]),
                entry(occupation(2), 200, &["voice actor"]),
                entry(occupation(3), 100, &["tennis player"]),
                entry(occupation(4), 700, &["agent", "actor"]),
                entry(Term::Value { attribute: Attribute::Citizenship, qid: FRANCE }, 400, &["France"]),
            ],
            ancestors: vec![(Attribute::Occupation, FILM_ACTOR, vec![ACTOR, ARTIST])],
        }
    }

    fn labels(found: Vec<&Entry>) -> Vec<&str> {
        found.into_iter().map(Entry::label).collect()
    }

    #[test]
    fn round_trip() {
        let data = data();
        assert_eq!(CatalogueData::decode(&data.encode()), Some(data.clone()));
        let mut truncated = data.encode();
        truncated.pop();
        assert_eq!(CatalogueData::decode(&truncated), None);
    }

    #[test]
    fn term_ids_round_trip() {
        for term in [Term::Women, Term::Men, occupation(ACTOR)] {
            assert_eq!(Term::parse(&term.id()), Some(term));
        }
        assert_eq!(occupation(ACTOR).id(), "occupation:Q33999");
        assert_eq!(Term::parse("occupation:33999"), None);
        assert_eq!(Term::parse("hobby:Q1"), None);
        assert_eq!(Term::parse(""), None);
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_word_then_substring() {
        let catalogue = Catalogue::new(data());
        // "act": prefix matches by size, then a word prefix, then nothing else.
        assert_eq!(labels(catalogue.search("act", 10)), ["actor", "activist", "agent", "voice actor"]);
        // The category called "actor" beats a bigger one merely also known as "actor".
        assert_eq!(labels(catalogue.search("actor", 10)), ["actor", "agent", "voice actor"]);
        // The plural finds the singular, and an alias counts as a name.
        assert_eq!(labels(catalogue.search("Actors", 10))[0], "actor");
        assert_eq!(labels(catalogue.search("actresses", 10)), ["actor"]);
        assert_eq!(labels(catalogue.search("tennis players", 10)), ["tennis player"]);
        assert_eq!(labels(catalogue.search("france", 10)), ["France"]);
        // A substring still finds it, last.
        assert_eq!(labels(catalogue.search("ennis", 10)), ["tennis player"]);
        assert!(catalogue.search("plumber", 10).is_empty());
        // No query: the biggest categories.
        assert_eq!(labels(catalogue.search("", 3)), ["women", "artist", "agent"]);
    }

    #[test]
    fn membership_follows_the_class_tree() {
        let catalogue = Catalogue::new(data());
        let person = |kind: Kind, occupations: Vec<u32>, genders: Vec<u32>| {
            let attributes = Attributes::from_fn(|attribute| match attribute {
                Attribute::Occupation => occupations.clone(),
                Attribute::Gender => genders.clone(),
                _ => Vec::new(),
            });
            (HotRecord { qid: 1, sitelinks: 50, kind }, ColdRecord { attributes, ..Default::default() })
        };
        let (hot, cold) = person(Kind::Woman, vec![FILM_ACTOR], vec![6581072]);
        assert!(catalogue.contains(&occupation(FILM_ACTOR), hot, &cold));
        assert!(catalogue.contains(&occupation(ACTOR), hot, &cold), "a film actor is an actor");
        assert!(catalogue.contains(&occupation(ARTIST), hot, &cold));
        assert!(!catalogue.contains(&occupation(1), hot, &cold));
        assert!(catalogue.contains(&Term::Women, hot, &cold));
        assert!(!catalogue.contains(&Term::Men, hot, &cold));

        // An actor is not thereby a film actor.
        let (hot, cold) = person(Kind::NotWoman, vec![ACTOR], vec![6581097]);
        assert!(!catalogue.contains(&occupation(FILM_ACTOR), hot, &cold));
        assert!(catalogue.contains(&Term::Men, hot, &cold));

        // Fictional characters are in no category.
        let (hot, cold) = person(Kind::Fictional, vec![ACTOR], vec![6581072]);
        assert!(!catalogue.contains(&occupation(ACTOR), hot, &cold));
        assert!(!catalogue.contains(&Term::Women, hot, &cold));
    }
}
