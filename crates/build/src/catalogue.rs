//! Second half of the build: once the people are known, work out which
//! attribute values need labels, which categories are playable, and write
//! `labels.bin` and `catalogue.bin`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::Result;
use hundred_core::artifact::{
    Counts, Kind, CATALOGUE_FILE, CATALOGUE_MAGIC, LABELS_FILE, LABELS_MAGIC,
};
use hundred_core::catalogue::{CatalogueData, Entry, Term, MAN_GENDERS};
use hundred_core::category::MAX_DEPTH;
use hundred_core::Attribute;
use rayon::prelude::*;

use crate::entity::{Person, Reference};

/// Possible attribute values, spooled here during the scan, one line each:
/// `qid <TAB> label <TAB> aliases <TAB> instance-of QIDs`. There are tens of
/// millions, so they are kept on disk and filtered once the people are known.
pub const SPOOL: &str = "labels.tmp";
/// Separates aliases within their field.
const ALIAS_SEPARATOR: char = '\u{1f}';
/// Other names kept per category, for searching.
const MAX_ALIASES: usize = 20;

pub fn spool_line(reference: &Reference) -> String {
    let clean = |text: &str| text.replace(['\t', '\n', '\r', ALIAS_SEPARATOR], " ");
    let aliases: Vec<String> = reference.aliases.iter().map(|alias| clean(alias)).collect();
    let instance_of: Vec<String> = reference.instance_of.iter().map(u32::to_string).collect();
    format!(
        "{}\t{}\t{}\t{}",
        reference.qid,
        clean(&reference.label),
        aliases.join(&ALIAS_SEPARATOR.to_string()),
        instance_of.join(",")
    )
}

struct SpoolLine<'a> {
    qid: u32,
    label: &'a str,
    aliases: &'a str,
    instance_of: &'a str,
}

fn read_spool(path: &Path, mut each: impl FnMut(SpoolLine<'_>)) -> Result<()> {
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        let mut fields = line.splitn(4, '\t');
        let (Some(qid), Some(label)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Ok(qid) = qid.parse() else {
            continue;
        };
        each(SpoolLine {
            qid,
            label,
            aliases: fields.next().unwrap_or(""),
            instance_of: fields.next().unwrap_or(""),
        });
    }
    Ok(())
}

/// How a value's wider categories are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Widening {
    /// The value stands alone: a country, a gender.
    None,
    /// Up the "subclass of" tree: film actor → actor → artist.
    Classes,
    /// What it is an instance of, then up the tree: 2016 Summer Olympics →
    /// Summer Olympic Games; Nobel Prize in Physics → Nobel Prize.
    InstanceThenClasses,
}

fn widening(attribute: Attribute) -> Widening {
    match attribute {
        Attribute::Gender | Attribute::Citizenship => Widening::None,
        Attribute::Occupation
        | Attribute::Sport
        | Attribute::Genre
        | Attribute::Instrument
        | Attribute::FieldOfWork
        | Attribute::NobleTitle => Widening::Classes,
        Attribute::Award | Attribute::Position | Attribute::Event => Widening::InstanceThenClasses,
    }
}

/// Everything `value` is a kind of, itself excluded, sorted. `links` is
/// sorted (class, parent) pairs.
fn ancestors(
    value: u32,
    widening: Widening,
    links: &[(u32, u32)],
    instance_of: &HashMap<u32, Vec<u32>>,
) -> Vec<u32> {
    if widening == Widening::None {
        return Vec::new();
    }
    let parents = |class: u32| {
        let start = links.partition_point(|&(child, _)| child < class);
        links[start..].iter().take_while(move |&&(child, _)| child == class).map(|&(_, p)| p)
    };
    let mut seen: HashSet<u32> = HashSet::from([value]);
    let mut level: Vec<u32> = vec![value];
    if widening == Widening::InstanceThenClasses {
        let classes = instance_of.get(&value).into_iter().flatten().copied();
        level.extend(classes.filter(|&class| seen.insert(class)));
    }
    for _ in 0..MAX_DEPTH {
        level = level.iter().flat_map(|&qid| parents(qid)).filter(|&qid| seen.insert(qid)).collect();
        if level.is_empty() {
            break;
        }
    }
    seen.remove(&value);
    let mut out: Vec<u32> = seen.into_iter().collect();
    out.sort_unstable();
    out
}

#[derive(Default, Clone, Copy)]
struct Tally {
    people: u32,
    women: u32,
    /// People who hold exactly this value, not just something narrower.
    direct: u32,
}

type Tallies = HashMap<(Attribute, u32), Tally>;

fn merge(mut into: Tallies, from: Tallies) -> Tallies {
    for (key, tally) in from {
        let entry = into.entry(key).or_default();
        entry.people += tally.people;
        entry.women += tally.women;
        entry.direct += tally.direct;
    }
    into
}

/// Write `labels.bin` and `catalogue.bin`, and remove the spool.
///
/// `links` is sorted, deduplicated (class, parent class) pairs.
pub fn write(
    people: &[Person],
    links: &[(u32, u32)],
    min_people: u32,
    out: &Path,
    counts: &mut Counts,
) -> Result<()> {
    let spool = out.join(SPOOL);

    // Every value some person holds, per attribute.
    let mut held: HashMap<Attribute, HashSet<u32>> = HashMap::new();
    for person in people {
        for attribute in Attribute::ALL {
            held.entry(attribute).or_default().extend(person.attributes.get(attribute));
        }
    }
    let referenced: HashSet<u32> = held.values().flatten().copied().collect();

    // First pass over the spool: what awards, positions and events are instances of.
    let needs_instances: HashSet<u32> = held
        .iter()
        .filter(|(attribute, _)| widening(**attribute) == Widening::InstanceThenClasses)
        .flat_map(|(_, values)| values)
        .copied()
        .collect();
    let mut instance_of: HashMap<u32, Vec<u32>> = HashMap::new();
    read_spool(&spool, |line| {
        if needs_instances.contains(&line.qid) && !line.instance_of.is_empty() {
            let classes = line.instance_of.split(',').filter_map(|qid| qid.parse().ok());
            instance_of.insert(line.qid, classes.collect());
        }
    })?;

    // The wider categories of every held value.
    let pairs: Vec<(Attribute, u32)> = held
        .iter()
        .flat_map(|(attribute, values)| values.iter().map(move |&value| (*attribute, value)))
        .collect();
    let wider: HashMap<(Attribute, u32), Vec<u32>> = pairs
        .par_iter()
        .map(|&(attribute, value)| {
            ((attribute, value), ancestors(value, widening(attribute), links, &instance_of))
        })
        .collect();

    // Head counts: each real person counts once per category they fall in.
    let tallies: Tallies = people
        .par_iter()
        .filter(|person| person.kind != Kind::Fictional)
        .fold(Tallies::new, |mut tallies, person| {
            let woman = person.kind == Kind::Woman;
            let mut categories: Vec<u32> = Vec::new();
            for attribute in Attribute::ALL {
                categories.clear();
                for &value in person.attributes.get(attribute) {
                    categories.push(value);
                    categories.extend(&wider[&(attribute, value)]);
                    tallies.entry((attribute, value)).or_default().direct += 1;
                }
                categories.sort_unstable();
                categories.dedup();
                for &category in &categories {
                    let tally = tallies.entry((attribute, category)).or_default();
                    tally.people += 1;
                    tally.women += u32::from(woman);
                }
            }
            tallies
        })
        .reduce(Tallies::new, merge);

    // How many people have the attribute at all: a class that takes in most
    // of them ("person", "worker") is not a category worth playing.
    let mut holders: HashMap<Attribute, u32> = HashMap::new();
    for person in people.iter().filter(|person| person.kind != Kind::Fictional) {
        for attribute in Attribute::ALL {
            if !person.attributes.get(attribute).is_empty() {
                *holders.entry(attribute).or_default() += 1;
            }
        }
    }
    // The class tree also runs up into abstractions nobody would play ("agent",
    // "material object"). A wider category is kept only if people hold it
    // outright ("musician"), or it is the family that held values are
    // instances of ("Nobel Prize" for its physics and chemistry prizes).
    let families: HashSet<u32> = instance_of.values().flatten().copied().collect();
    let min_direct = (min_people / 20).max(1);
    let playable: HashMap<(Attribute, u32), Tally> = tallies
        .into_iter()
        .filter(|((attribute, qid), tally)| {
            let widened = widening(*attribute) != Widening::None;
            let too_broad = widened
                && tally.people as u64 * 2 > holders.get(attribute).copied().unwrap_or(0) as u64;
            let family = widening(*attribute) == Widening::InstanceThenClasses
                && families.contains(qid);
            let real = !widened || tally.direct >= min_direct || family;
            tally.people >= min_people && !too_broad && real
        })
        .collect();
    let playable_qids: HashSet<u32> = playable.keys().map(|&(_, qid)| qid).collect();

    // Second pass: labels of held values, and names of the playable categories.
    let mut labels: BTreeMap<u32, String> = BTreeMap::new();
    let mut names: HashMap<u32, Vec<String>> = HashMap::new();
    read_spool(&spool, |line| {
        if referenced.contains(&line.qid) {
            labels.insert(line.qid, line.label.to_owned());
        }
        if playable_qids.contains(&line.qid) {
            let mut all = vec![line.label.to_owned()];
            for alias in line.aliases.split(ALIAS_SEPARATOR).filter(|alias| !alias.is_empty()) {
                if all.len() <= MAX_ALIASES && !all.iter().any(|name| name == alias) {
                    all.push(alias.to_owned());
                }
            }
            names.insert(line.qid, all);
        }
    })?;
    fs::remove_file(&spool)?;
    write_labels(&labels, out)?;
    counts.labels = labels.len() as u64;

    // The catalogue. A category nobody could name (no label) is left out.
    let real = || people.iter().filter(|person| person.kind != Kind::Fictional);
    let women = real().filter(|person| person.kind == Kind::Woman).count() as u32;
    let men = real()
        .filter(|p| p.attributes.get(Attribute::Gender).iter().any(|g| MAN_GENDERS.contains(g)))
        .count() as u32;
    let mut entries = vec![
        Entry { term: Term::Women, people: women, women, names: vec!["women".to_owned()] },
        Entry { term: Term::Men, people: men, women: 0, names: vec!["men".to_owned()] },
    ];
    for (&(attribute, qid), tally) in &playable {
        if let Some(names) = names.get(&qid) {
            entries.push(Entry {
                term: Term::Value { attribute, qid },
                people: tally.people,
                women: tally.women,
                names: names.clone(),
            });
        }
    }
    entries.sort_by(|a, b| b.people.cmp(&a.people).then_with(|| a.term.id().cmp(&b.term.id())));

    // Ancestor lists, trimmed to the ones that are categories.
    let in_catalogue: HashSet<Term> = entries.iter().map(|entry| entry.term).collect();
    let mut ancestor_lists: Vec<(Attribute, u32, Vec<u32>)> = wider
        .into_iter()
        .filter_map(|((attribute, value), list)| {
            let kept: Vec<u32> = list
                .into_iter()
                .filter(|&qid| in_catalogue.contains(&Term::Value { attribute, qid }))
                .collect();
            (!kept.is_empty()).then_some((attribute, value, kept))
        })
        .collect();
    ancestor_lists.sort_unstable_by_key(|(attribute, value, _)| (*attribute as u8, *value));

    counts.catalogue_entries = entries.len() as u64;
    let data = CatalogueData { entries, ancestors: ancestor_lists };
    let mut bytes = CATALOGUE_MAGIC.to_vec();
    bytes.extend(data.encode());
    fs::write(out.join(CATALOGUE_FILE), bytes)?;
    Ok(())
}

fn write_labels(labels: &BTreeMap<u32, String>, out: &Path) -> Result<()> {
    let mut file = crate::create(out, LABELS_FILE)?;
    file.write_all(LABELS_MAGIC)?;
    file.write_all(&(labels.len() as u32).to_le_bytes())?;
    for qid in labels.keys() {
        file.write_all(&qid.to_le_bytes())?;
    }
    let mut offset = 0u32;
    for label in labels.values() {
        file.write_all(&offset.to_le_bytes())?;
        offset += label.len() as u32;
    }
    file.write_all(&offset.to_le_bytes())?;
    for label in labels.values() {
        file.write_all(label.as_bytes())?;
    }
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widening_follows_classes_and_optionally_one_instance_step() {
        let (film_actor, actor, performer, artist) = (1, 2, 3, 4);
        let (physics_prize, nobel_prize, science_award, award) = (10, 11, 12, 13);
        let mut links = vec![
            (film_actor, actor),
            (actor, performer),
            (performer, artist),
            (nobel_prize, science_award),
            (science_award, award),
            // A loop must not hang the walk.
            (artist, performer),
        ];
        links.sort_unstable();
        let instance_of = HashMap::from([(physics_prize, vec![nobel_prize])]);

        let up = |value, widening| ancestors(value, widening, &links, &instance_of);
        assert_eq!(up(film_actor, Widening::Classes), [actor, performer, artist]);
        assert_eq!(up(film_actor, Widening::None), [] as [u32; 0]);
        assert_eq!(up(artist, Widening::Classes), [performer]);
        // A prize is an instance of its family, which has classes above it.
        assert_eq!(
            up(physics_prize, Widening::InstanceThenClasses),
            [nobel_prize, science_award, award]
        );
        // Without the instance step it has no ancestors at all.
        assert_eq!(up(physics_prize, Widening::Classes), [] as [u32; 0]);
    }

    #[test]
    fn spool_lines_survive_awkward_text() {
        let reference = Reference {
            qid: 7,
            label: "tab\there".into(),
            aliases: vec!["one".into(), "two\nlines".into()],
            parents: vec![1],
            instance_of: vec![5, 6],
        };
        let dir = std::env::temp_dir().join(format!("hundred-spool-{}", std::process::id()));
        std::fs::write(&dir, spool_line(&reference) + "\n").unwrap();
        let mut seen = Vec::new();
        read_spool(&dir, |line| {
            seen.push((
                line.qid,
                line.label.to_owned(),
                line.aliases.split(ALIAS_SEPARATOR).map(str::to_owned).collect::<Vec<_>>(),
                line.instance_of.to_owned(),
            ));
        })
        .unwrap();
        std::fs::remove_file(&dir).unwrap();
        assert_eq!(
            seen,
            [(7, "tab here".to_owned(), vec!["one".to_owned(), "two lines".to_owned()], "5,6".to_owned())]
        );
    }
}
