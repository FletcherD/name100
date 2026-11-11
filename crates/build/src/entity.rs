//! One line of the Wikidata JSON dump → a person to index, or a reason to drop it.

use std::borrow::Cow;
use std::collections::HashMap;

use hundred_core::{normalize, Attribute, Attributes, Kind, WOMAN_GENDERS};
use serde::de::IgnoredAny;
use serde::Deserialize;

const HUMAN: &str = "Q5";

/// `P31` values stored as `fictional`. Wikidata does not put every character
/// under "fictional human": Hermione Granger, for one, is only a literary,
/// film and theatrical character. So the common character classes are listed.
pub const FICTIONAL_CLASSES: &[&str] = &[
    "Q15632617", // fictional human
    "Q95074",    // character
    "Q3658341",  // literary character
    "Q15773347", // film character
    "Q15773317", // television character
    "Q15711870", // animated character
    "Q1114461",  // comics character
    "Q1569167",  // video game character
    "Q3375722",  // theatrical character
    "Q80447738", // anime character
    "Q63975020", // musical theatre character
];

/// Sitelink keys that end in "wiki" but are not a Wikipedia language edition.
const NOT_WIKIPEDIA: &[&str] = &[
    "commonswiki",
    "specieswiki",
    "sourceswiki",
    "metawiki",
    "wikidatawiki",
    "mediawikiwiki",
    "incubatorwiki",
    "outreachwiki",
    "foundationwiki",
    "wikimaniawiki",
    "wikifunctionswiki",
    "abstractwiki",
];

#[derive(Deserialize)]
struct RawEntity<'a> {
    #[serde(borrow)]
    id: Cow<'a, str>,
    #[serde(borrow, default)]
    labels: HashMap<&'a str, Term<'a>>,
    #[serde(borrow, default)]
    aliases: HashMap<&'a str, Vec<Term<'a>>>,
    #[serde(borrow, default)]
    descriptions: HashMap<&'a str, Term<'a>>,
    #[serde(borrow, default)]
    claims: Claims<'a>,
    #[serde(borrow, default)]
    sitelinks: HashMap<&'a str, SiteLink<'a>>,
}

#[derive(Deserialize)]
struct Term<'a> {
    #[serde(borrow)]
    value: Cow<'a, str>,
}

#[derive(Deserialize)]
struct SiteLink<'a> {
    #[serde(borrow, default)]
    title: Cow<'a, str>,
}

/// Only the properties the builder reads; every other property in the
/// entity is skipped without being materialized.
#[derive(Deserialize, Default)]
struct Claims<'a> {
    #[serde(rename = "P31", borrow, default)]
    instance_of: Vec<Statement<'a>>,
    #[serde(rename = "P21", borrow, default)]
    gender: Vec<Statement<'a>>,
    #[serde(rename = "P569", borrow, default)]
    birth: Vec<Statement<'a>>,
    #[serde(rename = "P570", borrow, default)]
    death: Vec<Statement<'a>>,
    #[serde(rename = "P106", borrow, default)]
    occupation: Vec<Statement<'a>>,
    #[serde(rename = "P27", borrow, default)]
    citizenship: Vec<Statement<'a>>,
    #[serde(rename = "P166", borrow, default)]
    award: Vec<Statement<'a>>,
    #[serde(rename = "P39", borrow, default)]
    position: Vec<Statement<'a>>,
    #[serde(rename = "P641", borrow, default)]
    sport: Vec<Statement<'a>>,
    #[serde(rename = "P1344", borrow, default)]
    event: Vec<Statement<'a>>,
    #[serde(rename = "P136", borrow, default)]
    genre: Vec<Statement<'a>>,
    #[serde(rename = "P1303", borrow, default)]
    instrument: Vec<Statement<'a>>,
    #[serde(rename = "P101", borrow, default)]
    field_of_work: Vec<Statement<'a>>,
    #[serde(rename = "P97", borrow, default)]
    noble_title: Vec<Statement<'a>>,
    #[serde(rename = "P279", borrow, default)]
    subclass_of: Vec<Statement<'a>>,
}

impl<'a> Claims<'a> {
    fn statements(&self, attribute: Attribute) -> &[Statement<'a>] {
        match attribute {
            Attribute::Gender => &self.gender,
            Attribute::Occupation => &self.occupation,
            Attribute::Citizenship => &self.citizenship,
            Attribute::Award => &self.award,
            Attribute::Position => &self.position,
            Attribute::Sport => &self.sport,
            Attribute::Event => &self.event,
            Attribute::Genre => &self.genre,
            Attribute::Instrument => &self.instrument,
            Attribute::FieldOfWork => &self.field_of_work,
            Attribute::NobleTitle => &self.noble_title,
        }
    }

    /// Gender keeps only the current values, since it decides acceptance.
    /// Everything else keeps every non-deprecated value.
    fn attributes(&self) -> Attributes {
        Attributes::from_fn(|attribute| {
            let statements = self.statements(attribute);
            match attribute {
                Attribute::Gender => numeric_qids(item_ids(statements)),
                _ => numeric_qids(all_item_ids(statements)),
            }
        })
    }
}

#[derive(Deserialize)]
struct Statement<'a> {
    #[serde(borrow)]
    mainsnak: Snak<'a>,
    #[serde(default)]
    rank: Rank,
}

#[derive(Deserialize)]
struct Snak<'a> {
    /// Absent for "unknown value" and "no value" snaks.
    #[serde(borrow, default)]
    datavalue: Option<DataValue<'a>>,
}

#[derive(Deserialize)]
struct DataValue<'a> {
    #[serde(borrow)]
    value: SnakValue<'a>,
}

/// Item values carry `id`, time values carry `time`. Values of other shapes
/// (which these four properties should never have) parse as neither.
#[derive(Deserialize)]
#[serde(untagged)]
enum SnakValue<'a> {
    Object {
        #[serde(borrow, default)]
        id: Option<Cow<'a, str>>,
        #[serde(borrow, default)]
        time: Option<Cow<'a, str>>,
    },
    Other(IgnoredAny),
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Rank {
    Preferred,
    #[default]
    Normal,
    Deprecated,
}

/// The statements Wikidata treats as current: the preferred ones if there are
/// any, otherwise the normal ones. Deprecated statements never count, so a
/// superseded gender claim cannot classify anyone.
fn best_rank<'s, 'a>(statements: &'s [Statement<'a>]) -> impl Iterator<Item = &'s Statement<'a>> {
    let wanted = if statements.iter().any(|s| s.rank == Rank::Preferred) {
        Rank::Preferred
    } else {
        Rank::Normal
    };
    statements.iter().filter(move |s| s.rank == wanted)
}

fn item_ids<'s, 'a: 's>(statements: &'s [Statement<'a>]) -> impl Iterator<Item = &'s str> {
    best_rank(statements).filter_map(|s| match &s.mainsnak.datavalue.as_ref()?.value {
        SnakValue::Object { id: Some(id), .. } => Some(id.as_ref()),
        _ => None,
    })
}

/// Year of the first current statement that has one. Wikidata times look like
/// `+1933-03-15T00:00:00Z` or `-0069-01-13T00:00:00Z`.
fn year(statements: &[Statement<'_>]) -> Option<i32> {
    best_rank(statements).find_map(|s| match &s.mainsnak.datavalue.as_ref()?.value {
        SnakValue::Object { time: Some(time), .. } => parse_year(time),
        _ => None,
    })
}

fn parse_year(time: &str) -> Option<i32> {
    let (negative, rest) = match time.as_bytes().first()? {
        b'-' => (true, &time[1..]),
        b'+' => (false, &time[1..]),
        _ => (false, time),
    };
    let digits = rest.split('-').next()?;
    let year: i32 = digits.parse().ok()?;
    Some(if negative { -year } else { year })
}

/// Numeric QIDs, in statement order, without repeats.
fn numeric_qids<'s>(ids: impl Iterator<Item = &'s str>) -> Vec<u32> {
    let mut out = Vec::new();
    for qid in ids.filter_map(|id| id.strip_prefix('Q')?.parse().ok()) {
        if !out.contains(&qid) {
            out.push(qid);
        }
    }
    out
}

/// Every item value that is not deprecated. Used for attributes a person can
/// hold several of at once: a preferred "singer" must not hide a normal "actor".
fn all_item_ids<'s, 'a: 's>(statements: &'s [Statement<'a>]) -> impl Iterator<Item = &'s str> {
    statements.iter().filter(|s| s.rank != Rank::Deprecated).filter_map(|s| {
        match &s.mainsnak.datavalue.as_ref()?.value {
            SnakValue::Object { id: Some(id), .. } => Some(id.as_ref()),
            _ => None,
        }
    })
}

fn classify(claims: &Claims<'_>, genders: &[u32]) -> Option<Kind> {
    let mut human = false;
    for id in item_ids(&claims.instance_of) {
        if FICTIONAL_CLASSES.contains(&id) {
            return Some(Kind::Fictional);
        }
        human |= id == HUMAN;
    }
    if !human {
        return None;
    }
    let woman = genders.iter().any(|qid| WOMAN_GENDERS.contains(qid));
    Some(if woman { Kind::Woman } else { Kind::NotWoman })
}

fn wikipedia_sitelinks(sitelinks: &HashMap<&str, SiteLink<'_>>) -> u32 {
    sitelinks
        .keys()
        .filter(|site| site.ends_with("wiki") && !NOT_WIKIPEDIA.contains(site))
        .count() as u32
}

/// A person kept for the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub qid: u32,
    pub kind: Kind,
    pub sitelinks: u32,
    pub label: String,
    pub description: String,
    pub birth_year: Option<i32>,
    pub death_year: Option<i32>,
    pub enwiki_title: String,
    pub attributes: Attributes,
    /// Every indexed name other than `label`, as written in Wikidata.
    pub aliases: Vec<String>,
    /// Distinct normalized names, from labels and aliases in the indexed languages.
    pub names: Vec<String>,
}

/// Something a person's attributes may point at: an occupation, a country,
/// a sport. Kept for its English label and its place in the class tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub qid: u32,
    /// English label (the `mul` label when there is no English one).
    pub label: String,
    /// Other English names, for finding it as a category ("actress").
    pub aliases: Vec<String>,
    /// The classes this is a "subclass of" (`P279`), as numeric QIDs.
    pub parents: Vec<u32>,
    /// What this is an "instance of" (`P31`), as numeric QIDs.
    pub instance_of: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Kept(Box<Person>),
    /// Not an item, or neither human nor a listed fictional class. Carries a
    /// [`Reference`] when the entity could be an attribute value: it has a
    /// label and either a Wikipedia article or a parent class.
    NotPerson(Option<Box<Reference>>),
    BelowMinSitelinks,
    /// No label or alias in the indexed languages survives normalization.
    NoName,
    ParseError(String),
}

fn reference(qid: u32, raw: &RawEntity<'_>) -> Option<Box<Reference>> {
    let label = raw.labels.get("en").or_else(|| raw.labels.get("mul"))?;
    let parents = numeric_qids(all_item_ids(&raw.claims.subclass_of));
    if parents.is_empty() && wikipedia_sitelinks(&raw.sitelinks) == 0 {
        return None;
    }
    Some(Box::new(Reference {
        qid,
        label: label.value.to_string(),
        aliases: raw.aliases.get("en").into_iter().flatten().map(|a| a.value.to_string()).collect(),
        parents,
        instance_of: numeric_qids(all_item_ids(&raw.claims.instance_of)),
    }))
}

/// Parse one entity. `json` is a bare JSON object: the caller strips the
/// dump's array brackets and trailing commas.
pub fn extract(json: &str, langs: &[String], min_sitelinks: u32) -> Outcome {
    let raw: RawEntity = match serde_json::from_str(json) {
        Ok(raw) => raw,
        Err(err) => return Outcome::ParseError(err.to_string()),
    };
    let Some(qid) = raw.id.strip_prefix('Q').and_then(|n| n.parse::<u32>().ok()) else {
        return Outcome::NotPerson(None);
    };
    let attributes = raw.claims.attributes();
    let Some(kind) = classify(&raw.claims, attributes.get(Attribute::Gender)) else {
        return Outcome::NotPerson(reference(qid, &raw));
    };
    let sitelinks = wikipedia_sitelinks(&raw.sitelinks);
    if sitelinks < min_sitelinks {
        return Outcome::BelowMinSitelinks;
    }

    let mut names = Vec::new();
    let mut written: Vec<&str> = Vec::new();
    for lang in langs {
        let lang = lang.as_str();
        let label = raw.labels.get(lang).into_iter();
        for term in label.chain(raw.aliases.get(lang).into_iter().flatten()) {
            let normalized = normalize(&term.value);
            if normalized.is_empty() {
                continue;
            }
            names.push(normalized);
            if !written.contains(&term.value.as_ref()) {
                written.push(&term.value);
            }
        }
    }
    // The display name is the label in the first language that has one.
    let label = langs
        .iter()
        .find_map(|lang| raw.labels.get(lang.as_str()))
        .map(|term| term.value.to_string());
    let aliases: Vec<String> = written
        .into_iter()
        .filter(|name| Some(*name) != label.as_deref())
        .map(str::to_owned)
        .collect();
    names.sort_unstable();
    names.dedup();
    let Some(label) = label.filter(|_| !names.is_empty()) else {
        return Outcome::NoName;
    };

    Outcome::Kept(Box::new(Person {
        qid,
        kind,
        sitelinks,
        label,
        description: raw.descriptions.get("en").map(|d| d.value.to_string()).unwrap_or_default(),
        birth_year: year(&raw.claims.birth),
        death_year: year(&raw.claims.death),
        enwiki_title: raw.sitelinks.get("enwiki").map(|s| s.title.to_string()).unwrap_or_default(),
        attributes,
        aliases,
        names,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn langs() -> Vec<String> {
        vec!["en".into(), "mul".into()]
    }

    fn entity(p31: &str, p21: &[(&str, &str)], sites: &[&str]) -> String {
        let item = |id: &str, rank: &str| {
            format!(
                r#"{{"mainsnak":{{"snaktype":"value","datavalue":{{"value":{{"entity-type":"item","id":"{id}"}},"type":"wikibase-entityid"}}}},"rank":"{rank}"}}"#
            )
        };
        let genders: Vec<String> = p21.iter().map(|(id, rank)| item(id, rank)).collect();
        let links: Vec<String> = sites
            .iter()
            .map(|site| format!(r#""{site}":{{"site":"{site}","title":"Some Title"}}"#))
            .collect();
        format!(
            r#"{{"type":"item","id":"Q42","labels":{{"en":{{"language":"en","value":"Zoë O'Hara"}}}},"aliases":{{"en":[{{"language":"en","value":"Z. O'Hara"}},{{"language":"en","value":"ZOE OHARA"}}],"de":[{{"language":"de","value":"Nur Deutsch"}}]}},"descriptions":{{"en":{{"language":"en","value":"test person"}}}},"claims":{{"P31":[{}],"P21":[{}],"P106":[{{"mainsnak":{{"snaktype":"value","datavalue":{{"value":"a string","type":"string"}}}},"rank":"normal"}}]}},"sitelinks":{{{}}}}}"#,
            item(p31, "normal"),
            genders.join(","),
            links.join(",")
        )
    }

    fn kept(json: &str) -> Person {
        match extract(json, &langs(), 1) {
            Outcome::Kept(person) => *person,
            other => panic!("expected a kept person, got {other:?}"),
        }
    }

    #[test]
    fn woman_with_names_and_fields() {
        let person = kept(&entity("Q5", &[("Q6581072", "normal")], &["enwiki", "frwiki"]));
        assert_eq!(person.qid, 42);
        assert_eq!(person.kind, Kind::Woman);
        assert_eq!(person.sitelinks, 2);
        assert_eq!(person.label, "Zoë O'Hara");
        assert_eq!(person.description, "test person");
        assert_eq!(person.enwiki_title, "Some Title");
        // "Zoë O'Hara" and "ZOE OHARA" normalize to the same key; the German alias is not indexed.
        assert_eq!(person.names, vec!["z ohara", "zoe ohara"]);
        // The other names, as written, without the label itself.
        assert_eq!(person.aliases, vec!["Z. O'Hara", "ZOE OHARA"]);
    }

    #[test]
    fn gender_classification() {
        let kind = |p21: &[(&str, &str)]| kept(&entity("Q5", p21, &["enwiki"])).kind;
        assert_eq!(kind(&[("Q1052281", "normal")]), Kind::Woman);
        assert_eq!(kind(&[("Q15145779", "normal")]), Kind::Woman);
        assert_eq!(kind(&[("Q18116794", "normal")]), Kind::NotWoman);
        assert_eq!(kind(&[("Q6581097", "normal")]), Kind::NotWoman);
        assert_eq!(kind(&[("Q48270", "normal")]), Kind::NotWoman);
        assert_eq!(kind(&[]), Kind::NotWoman);
    }

    #[test]
    fn deprecated_and_outranked_gender_statements_are_ignored() {
        let kind = |p21: &[(&str, &str)]| kept(&entity("Q5", p21, &["enwiki"])).kind;
        assert_eq!(kind(&[("Q48270", "normal"), ("Q6581072", "deprecated")]), Kind::NotWoman);
        assert_eq!(kind(&[("Q6581097", "normal"), ("Q1052281", "preferred")]), Kind::Woman);
        assert_eq!(kind(&[("Q6581072", "normal"), ("Q6581097", "preferred")]), Kind::NotWoman);
    }

    #[test]
    fn current_gender_values_are_recorded() {
        let genders = |p21: &[(&str, &str)]| {
            kept(&entity("Q5", p21, &["enwiki"])).attributes.get(Attribute::Gender).to_vec()
        };
        assert_eq!(genders(&[("Q18116794", "normal")]), vec![18116794]);
        assert_eq!(
            genders(&[("Q48270", "normal"), ("Q2449503", "normal"), ("Q6581072", "deprecated")]),
            vec![48270, 2449503]
        );
        assert_eq!(genders(&[("Q48270", "normal"), ("Q48270", "normal")]), vec![48270]);
        assert!(genders(&[]).is_empty());
    }

    #[test]
    fn fictional_and_non_person() {
        let json = entity("Q3658341", &[("Q6581072", "normal")], &["enwiki"]);
        assert_eq!(kept(&json).kind, Kind::Fictional);
        let json = entity("Q277759", &[], &["enwiki"]);
        // A non-person with a label and an article is kept as a reference.
        let Outcome::NotPerson(Some(reference)) = extract(&json, &langs(), 1) else {
            panic!("expected a reference");
        };
        assert_eq!((reference.qid, reference.label.as_str()), (42, "Zoë O'Hara"));
        assert!(reference.parents.is_empty());
        assert_eq!(reference.aliases, ["Z. O'Hara", "ZOE OHARA"]);
        assert_eq!(reference.instance_of, [277759]);
        // Without an article or a parent class it is not.
        let json = entity("Q277759", &[], &[]);
        assert_eq!(extract(&json, &langs(), 1), Outcome::NotPerson(None));
    }

    #[test]
    fn classes_report_their_parents() {
        let json = r#"{"type":"item","id":"Q488205","labels":{"en":{"language":"en","value":"singer-songwriter"}},"claims":{"P279":[
            {"mainsnak":{"snaktype":"value","datavalue":{"value":{"id":"Q177220"},"type":"wikibase-entityid"}},"rank":"normal"},
            {"mainsnak":{"snaktype":"value","datavalue":{"value":{"id":"Q753110"},"type":"wikibase-entityid"}},"rank":"preferred"},
            {"mainsnak":{"snaktype":"value","datavalue":{"value":{"id":"Q1"},"type":"wikibase-entityid"}},"rank":"deprecated"}]},"sitelinks":{}}"#;
        let Outcome::NotPerson(Some(reference)) = extract(json, &langs(), 1) else {
            panic!("expected a reference");
        };
        assert_eq!(reference.label, "singer-songwriter");
        assert_eq!(reference.parents, [177220, 753110]);
    }

    #[test]
    fn only_wikipedia_sitelinks_count() {
        let sites = ["enwiki", "simplewiki", "zh_min_nanwiki", "commonswiki", "abstractwiki", "enwikiquote", "frwikisource"];
        let json = entity("Q5", &[], &sites);
        assert_eq!(kept(&json).sitelinks, 3);
        let json = entity("Q5", &[], &["commonswiki", "enwikiquote"]);
        assert_eq!(extract(&json, &langs(), 1), Outcome::BelowMinSitelinks);
        assert!(matches!(extract(&json, &langs(), 0), Outcome::Kept(_)));
    }

    #[test]
    fn no_name_in_indexed_languages() {
        let json = entity("Q5", &[], &["enwiki"]);
        assert_eq!(extract(&json, &["ja".to_string()], 1), Outcome::NoName);
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        assert!(matches!(extract("{\"id\":", &langs(), 1), Outcome::ParseError(_)));
    }

    #[test]
    fn years() {
        assert_eq!(parse_year("+1933-03-15T00:00:00Z"), Some(1933));
        assert_eq!(parse_year("-0069-01-13T00:00:00Z"), Some(-69));
        assert_eq!(parse_year("+1987-00-00T00:00:00Z"), Some(1987));
        assert_eq!(parse_year("garbage"), None);
    }
}
