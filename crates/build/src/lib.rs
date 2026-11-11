//! Index builder: Wikidata entity dump in, index artifact out.

mod catalogue;
pub mod entity;

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use hundred_core::artifact::{
    ColdRecord, Counts, HotRecord, Kind, Manifest, Sizes, COLD_FILE, COLD_MAGIC, HOT_FILE,
    CATALOGUE_FILE, CATEGORIES_FILE, CATEGORIES_MAGIC, FUZZY_NAMES_FILE, HOT_MAGIC, LABELS_FILE, MAGIC_LEN, MANIFEST_FILE, NAMES_FILE, POSTINGS_FILE, POSTINGS_MAGIC,
    SCHEMA_VERSION, SHARED_NAME_BIT,
};
use hundred_core::category::{anchor_category, category_labels, CATEGORIES, MAX_DEPTH};
use hundred_core::Attribute;
use rayon::prelude::*;

use entity::{extract, Outcome, Person};

/// Lines parsed per parallel batch. Bounds how much of the dump is in memory.
const BATCH_LINES: usize = 2048;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Languages whose labels and aliases are indexed. The first one that has
    /// a label supplies the display name.
    pub langs: Vec<String>,
    /// Minimum Wikipedia sitelinks for an entity to be kept.
    pub min_sitelinks: u32,
    /// Sitelinks a person needs before a misspelling is corrected to them.
    /// Exact matches are not affected.
    pub fuzzy_min_sitelinks: u32,
    /// People a category needs before it is offered as a game.
    pub category_min_people: u32,
    /// Date of the dump, `YYYY-MM-DD`, recorded in the manifest.
    pub dump_date: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        BuildOptions {
            langs: vec!["en".into(), "mul".into()],
            min_sitelinks: 1,
            fuzzy_min_sitelinks: 10,
            category_min_people: 100,
            dump_date: None,
        }
    }
}

/// Open a dump for streaming. `-` is stdin (for `lbzip2 -dc dump.bz2 | hundred
/// build-index --dump -`); `.bz2` and `.gz` are decompressed on the fly;
/// anything else is read as plain NDJSON.
pub fn open_dump(path: &str) -> Result<Box<dyn BufRead + Send>> {
    const BUF: usize = 1 << 20;
    if path == "-" {
        return Ok(Box::new(BufReader::with_capacity(BUF, io::stdin())));
    }
    let file = File::open(path).with_context(|| format!("opening dump {path}"))?;
    let reader: Box<dyn Read + Send> = if path.ends_with(".bz2") {
        // Multi-stream: parallel compressors write one bzip2 stream per block.
        Box::new(bzip2::read::MultiBzDecoder::new(BufReader::with_capacity(BUF, file)))
    } else if path.ends_with(".gz") {
        Box::new(flate2::read::MultiGzDecoder::new(BufReader::with_capacity(BUF, file)))
    } else {
        Box::new(file)
    };
    Ok(Box::new(BufReader::with_capacity(BUF, reader)))
}

/// The dump date encoded in a Wikimedia dump filename
/// (`wikidata-20261105-all.json.bz2` → `2026-11-05`).
pub fn dump_date_from_path(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?;
    let digits = name.strip_prefix("wikidata-")?.get(..8)?;
    digits.bytes().all(|b| b.is_ascii_digit()).then(|| {
        format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..])
    })
}

/// Stream `dump`, write the artifact into `out`, and return its manifest.
pub fn build_index(dump: impl BufRead, out: &Path, options: &BuildOptions) -> Result<Manifest> {
    if options.langs.is_empty() {
        bail!("at least one language must be indexed");
    }
    let started = Instant::now();
    let mut counts = Counts::default();
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    let Scan { mut people, mut subclass_links } = scan_dump(dump, out, options, &mut counts)?;

    // Most sitelinks first, so a record's index is its popularity rank. QID
    // breaks ties to keep builds of the same dump byte-identical.
    people.sort_unstable_by(|a, b| b.sitelinks.cmp(&a.sitelinks).then(a.qid.cmp(&b.qid)));
    if people.len() > u32::MAX as usize {
        bail!("{} records do not fit a 32-bit record index", people.len());
    }

    write_records(&people, out, &mut counts)?;
    subclass_links.par_sort_unstable();
    subclass_links.dedup();
    catalogue::write(&people, &subclass_links, options.category_min_people, out, &mut counts)?;
    write_occupation_categories(&people, &subclass_links, out, &mut counts)?;
    // People are sorted by sitelinks, so the well-known ones are a prefix.
    let well_known = people.partition_point(|p| p.sitelinks >= options.fuzzy_min_sitelinks);
    write_names(&mut people, well_known as u32, out, &mut counts)?;

    let size = |name: &str| -> Result<u64> { Ok(fs::metadata(out.join(name))?.len()) };
    let mut sizes = Sizes {
        names_fst: size(NAMES_FILE)?,
        names_fuzzy_fst: size(FUZZY_NAMES_FILE)?,
        postings_bin: size(POSTINGS_FILE)?,
        records_hot_bin: size(HOT_FILE)?,
        records_cold_bin: size(COLD_FILE)?,
        labels_bin: size(LABELS_FILE)?,
        occupation_categories_bin: size(CATEGORIES_FILE)?,
        catalogue_bin: size(CATALOGUE_FILE)?,
        total: 0,
    };
    sizes.total = sizes.names_fst
        + sizes.names_fuzzy_fst
        + sizes.postings_bin
        + sizes.records_hot_bin
        + sizes.records_cold_bin
        + sizes.labels_bin
        + sizes.occupation_categories_bin
        + sizes.catalogue_bin;

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        build_date: time::OffsetDateTime::now_utc().date().to_string(),
        dump_date: options.dump_date.clone(),
        langs: options.langs.clone(),
        attributes: Attribute::properties(),
        occupation_categories: category_labels(),
        min_sitelinks: options.min_sitelinks,
        fuzzy_min_sitelinks: options.fuzzy_min_sitelinks,
        category_min_people: options.category_min_people,
        counts,
        sizes,
        build_seconds: (started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        peak_rss_mb: peak_rss_mb(),
    };
    let mut json = serde_json::to_vec_pretty(&manifest)?;
    json.push(b'\n');
    fs::write(out.join(MANIFEST_FILE), json).context("writing manifest.json")?;
    Ok(manifest)
}

struct Scan {
    people: Vec<Person>,
    /// (class, parent class) pairs from "subclass of" statements.
    subclass_links: Vec<(u32, u32)>,
}

fn scan_dump(
    mut dump: impl BufRead,
    out: &Path,
    options: &BuildOptions,
    counts: &mut Counts,
) -> Result<Scan> {
    let mut people = Vec::new();
    let mut subclass_links: Vec<(u32, u32)> = Vec::new();
    let mut spool = create(out, catalogue::SPOOL)?;
    let mut batch: Vec<String> = Vec::with_capacity(BATCH_LINES);
    let mut first_error: Option<String> = None;
    loop {
        batch.clear();
        while batch.len() < BATCH_LINES {
            let mut line = String::new();
            if dump.read_line(&mut line).context("reading dump")? == 0 {
                break;
            }
            batch.push(line);
        }
        if batch.is_empty() {
            break;
        }
        let outcomes: Vec<Option<Outcome>> = batch
            .par_iter()
            .map(|line| {
                entity_json(line).map(|json| extract(json, &options.langs, options.min_sitelinks))
            })
            .collect();
        for outcome in outcomes.into_iter().flatten() {
            counts.entities_read += 1;
            match outcome {
                Outcome::Kept(person) => people.push(*person),
                Outcome::NotPerson(reference) => {
                    counts.dropped_not_person += 1;
                    if let Some(reference) = reference {
                        writeln!(spool, "{}", catalogue::spool_line(&reference))?;
                        subclass_links
                            .extend(reference.parents.iter().map(|&parent| (reference.qid, parent)));
                    }
                }
                Outcome::BelowMinSitelinks => counts.dropped_below_min_sitelinks += 1,
                Outcome::NoName => counts.dropped_no_name += 1,
                Outcome::ParseError(message) => {
                    counts.parse_errors += 1;
                    first_error.get_or_insert(message);
                }
            }
        }
    }
    if let Some(message) = first_error {
        eprintln!("warning: {} unparseable line(s); first error: {message}", counts.parse_errors);
    }
    spool.flush()?;
    counts.subclass_links = subclass_links.len() as u64;
    Ok(Scan { people, subclass_links })
}

/// Map every occupation held by a kept person to its nearest category, by
/// walking up the subclass tree, and write the map.
fn write_occupation_categories(
    people: &[Person],
    subclass_links: &[(u32, u32)],
    out: &Path,
    counts: &mut Counts,
) -> Result<()> {
    let mut occupations: Vec<u32> = people
        .iter()
        .flat_map(|person| person.attributes.get(Attribute::Occupation))
        .copied()
        .collect();
    occupations.par_sort_unstable();
    occupations.dedup();

    let mut file = create(out, CATEGORIES_FILE)?;
    file.write_all(CATEGORIES_MAGIC)?;
    counts.occupations = occupations.len() as u64;
    for occupation in occupations {
        if let Some(category) = nearest_category(occupation, subclass_links) {
            file.write_all(&occupation.to_le_bytes())?;
            file.write_all(&(category as u32).to_le_bytes())?;
            counts.occupations_categorized += 1;
        }
    }
    file.flush()?;
    Ok(())
}

/// The category whose anchor is fewest "subclass of" steps above `class`;
/// between equally near ones, the earlier in [`CATEGORIES`]. `links` is
/// sorted (class, parent) pairs.
fn nearest_category(class: u32, links: &[(u32, u32)]) -> Option<u8> {
    debug_assert!(CATEGORIES.len() <= u8::MAX as usize);
    let parents = |class: u32| {
        let start = links.partition_point(|&(child, _)| child < class);
        links[start..].iter().take_while(move |&&(child, _)| child == class).map(|&(_, p)| p)
    };
    let mut seen: HashSet<u32> = HashSet::from([class]);
    let mut level = vec![class];
    for _ in 0..=MAX_DEPTH {
        if let Some(category) = level.iter().filter_map(|&qid| anchor_category(qid)).min() {
            return Some(category);
        }
        let next: Vec<u32> =
            level.iter().flat_map(|&qid| parents(qid)).filter(|&qid| seen.insert(qid)).collect();
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// The JSON object on a dump line, without the array syntax around it. The
/// dump is one huge array: `[`, one entity per line with a trailing comma, `]`.
fn entity_json(line: &str) -> Option<&str> {
    let line = line.trim();
    let line = line.strip_suffix(',').unwrap_or(line);
    (!line.is_empty() && line != "[" && line != "]").then_some(line)
}

fn write_records(people: &[Person], out: &Path, counts: &mut Counts) -> Result<()> {
    let mut hot = create(out, HOT_FILE)?;
    hot.write_all(HOT_MAGIC)?;
    for person in people {
        match person.kind {
            Kind::Woman => counts.women += 1,
            Kind::NotWoman => counts.not_women += 1,
            Kind::Fictional => counts.fictional += 1,
        }
        if person.kind != Kind::Fictional {
            let genders = person.attributes.get(Attribute::Gender);
            if genders.is_empty() {
                *counts.gender_values.entry("none".to_owned()).or_default() += 1;
            }
            for gender in genders {
                *counts.gender_values.entry(format!("Q{gender}")).or_default() += 1;
            }
        }
        let record = HotRecord {
            qid: person.qid,
            sitelinks: person.sitelinks.min(u16::MAX as u32) as u16,
            kind: person.kind,
        };
        hot.write_all(&record.to_bytes())?;
    }
    hot.flush()?;
    counts.records = people.len() as u64;

    let cold_records = people.iter().map(|person| ColdRecord {
        label: person.label.clone(),
        description: person.description.clone(),
        birth_year: person.birth_year,
        death_year: person.death_year,
        enwiki_title: person.enwiki_title.clone(),
        attributes: person.attributes.clone(),
        aliases: person.aliases.clone(),
    });
    let mut cold = create(out, COLD_FILE)?;
    cold.write_all(COLD_MAGIC)?;
    cold.write_all(&(people.len() as u64).to_le_bytes())?;
    // Offsets are absolute file positions; entry `count` is the end of the last record.
    let mut offset = (MAGIC_LEN + 8 + (people.len() + 1) * 8) as u64;
    for record in cold_records.clone() {
        cold.write_all(&offset.to_le_bytes())?;
        offset += record.encoded_len() as u64;
    }
    cold.write_all(&offset.to_le_bytes())?;
    let mut buffer = Vec::new();
    for record in cold_records {
        buffer.clear();
        record.encode(&mut buffer);
        cold.write_all(&buffer)?;
    }
    cold.flush()?;
    Ok(())
}

/// Write the name index. Records below `well_known` (a record index) are the
/// people typo correction may land on.
fn write_names(
    people: &mut [Person],
    well_known: u32,
    out: &Path,
    counts: &mut Counts,
) -> Result<()> {
    let mut pairs: Vec<(String, u32)> = Vec::new();
    for (index, person) in people.iter_mut().enumerate() {
        pairs.extend(std::mem::take(&mut person.names).into_iter().map(|n| (n, index as u32)));
    }
    // By name (byte order, as the FST requires), then by record index, which
    // leaves each name's records most popular first.
    pairs.par_sort_unstable();
    pairs.dedup();
    counts.name_record_pairs = pairs.len() as u64;

    let mut names = fst::MapBuilder::new(create(out, NAMES_FILE)?)?;
    let mut fuzzy_names = fst::SetBuilder::new(create(out, FUZZY_NAMES_FILE)?)?;
    let mut postings = create(out, POSTINGS_FILE)?;
    postings.write_all(POSTINGS_MAGIC)?;
    let mut next_word: u64 = 0;

    for group in pairs.chunk_by(|a, b| a.0 == b.0) {
        let name = &group[0].0;
        counts.names += 1;
        // The group is in popularity order, so its first record is its most famous.
        if group[0].1 < well_known {
            fuzzy_names.insert(name)?;
            counts.fuzzy_names += 1;
        }
        if let [(_, index)] = group {
            names.insert(name, *index as u64)?;
            continue;
        }
        counts.shared_names += 1;
        names.insert(name, SHARED_NAME_BIT | next_word)?;
        postings.write_all(&(group.len() as u32).to_le_bytes())?;
        for (_, index) in group {
            postings.write_all(&index.to_le_bytes())?;
        }
        next_word += 1 + group.len() as u64;
    }
    names.finish()?;
    fuzzy_names.finish()?;
    postings.flush()?;
    Ok(())
}

pub(crate) fn create(dir: &Path, name: &str) -> Result<BufWriter<File>> {
    let path = dir.join(name);
    let file = File::create(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(BufWriter::with_capacity(1 << 20, file))
}

/// Peak resident set size in MB, from `/proc` (Linux only).
fn peak_rss_mb() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_array_syntax_is_stripped() {
        assert_eq!(entity_json("[\n"), None);
        assert_eq!(entity_json("]\n"), None);
        assert_eq!(entity_json("\n"), None);
        assert_eq!(entity_json("{\"id\":\"Q1\"},\n"), Some("{\"id\":\"Q1\"}"));
        assert_eq!(entity_json("{\"id\":\"Q1\"}\n"), Some("{\"id\":\"Q1\"}"));
    }

    #[test]
    fn occupations_take_the_nearest_category_up_the_class_tree() {
        const MUSICIAN: u32 = 639669;
        const ACTOR: u32 = 33999;
        const WRITER: u32 = 36180;
        let (singer, songwriter, singer_songwriter, tv_actor, plumber, deep) = (1, 2, 3, 4, 5, 6);
        let mut links = vec![
            (singer, MUSICIAN),
            (songwriter, WRITER),
            (singer_songwriter, singer),
            (singer_songwriter, songwriter),
            (tv_actor, ACTOR),
            // Classes can loop; the walk must still end.
            (plumber, 7),
            (7, plumber),
        ];
        // A chain longer than MAX_DEPTH never reaches its anchor.
        links.extend((0..MAX_DEPTH as u32 + 1).map(|i| (deep + 100 + i, deep + 101 + i)));
        links.push((deep, deep + 100));
        links.push((deep + 101 + MAX_DEPTH as u32, MUSICIAN));
        links.sort_unstable();

        let label = |class| nearest_category(class, &links).map(|id| CATEGORIES[id as usize].label);
        assert_eq!(label(MUSICIAN), Some("Musician"));
        assert_eq!(label(singer), Some("Musician"));
        assert_eq!(label(tv_actor), Some("Actor"));
        // Two steps from both Musician and Writer: the earlier category wins.
        assert_eq!(label(singer_songwriter), Some("Musician"));
        assert_eq!(label(songwriter), Some("Writer"));
        assert_eq!(label(plumber), None);
        assert_eq!(label(deep), None);
        assert_eq!(label(999_999), None);
    }

    #[test]
    fn dump_date_is_read_from_the_filename() {
        assert_eq!(
            dump_date_from_path("data/wikidata-20261105-all.json.bz2").as_deref(),
            Some("2026-11-05")
        );
        assert_eq!(dump_date_from_path("fixtures/sample.ndjson"), None);
        assert_eq!(dump_date_from_path("-"), None);
    }
}
