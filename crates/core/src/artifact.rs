//! On-disk layout of an index artifact, and the types stored in it.
//!
//! An artifact is one directory:
//!
//! | file               | contents                                                        |
//! | ------------------ | --------------------------------------------------------------- |
//! | `names.fst`        | normalized name → value (see [`SHARED_NAME_BIT`])                |
//! | `postings.bin`     | magic, then `u32` words: `[len, index, index, ...]` per shared name |
//! | `records_hot.bin`  | magic, then one 8-byte [`HotRecord`] per record                  |
//! | `records_cold.bin` | magic, `u64` count, `count + 1` `u64` offsets, then the records  |
//! | `manifest.json`    | [`Manifest`]                                                     |
//!
//! All integers are little endian. Records are sorted by sitelink count,
//! descending, so a record's index is its popularity rank and every posting
//! list is ascending.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::attribute::Attributes;

pub const SCHEMA_VERSION: u32 = 1;

pub const NAMES_FILE: &str = "names.fst";
pub const POSTINGS_FILE: &str = "postings.bin";
pub const HOT_FILE: &str = "records_hot.bin";
pub const COLD_FILE: &str = "records_cold.bin";
pub const MANIFEST_FILE: &str = "manifest.json";

pub const MAGIC_LEN: usize = 8;
pub const POSTINGS_MAGIC: &[u8; MAGIC_LEN] = b"HNDRPST1";
pub const HOT_MAGIC: &[u8; MAGIC_LEN] = b"HNDRHOT1";
pub const COLD_MAGIC: &[u8; MAGIC_LEN] = b"HNDRCLD1";

/// FST values with this bit clear are a record index. With it set, the low
/// 63 bits are a word offset into `postings.bin` (after the magic), where a
/// length word is followed by that many record indices.
pub const SHARED_NAME_BIT: u64 = 1 << 63;

pub const HOT_RECORD_LEN: usize = 8;

/// Marks a missing birth or death year in a cold record.
pub const NO_YEAR: i32 = i32::MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Kind {
    Woman = 0,
    NotWoman = 1,
    Fictional = 2,
}

impl Kind {
    pub fn from_u8(value: u8) -> Option<Kind> {
        match value {
            0 => Some(Kind::Woman),
            1 => Some(Kind::NotWoman),
            2 => Some(Kind::Fictional),
            _ => None,
        }
    }
}

/// What matching needs, and nothing else: 8 bytes per record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotRecord {
    /// Numeric part of the QID (`Q11116` → `11116`).
    pub qid: u32,
    /// Wikipedia sitelinks, saturated at `u16::MAX`.
    pub sitelinks: u16,
    pub kind: Kind,
}

impl HotRecord {
    pub fn to_bytes(self) -> [u8; HOT_RECORD_LEN] {
        let mut out = [0u8; HOT_RECORD_LEN];
        out[0..4].copy_from_slice(&self.qid.to_le_bytes());
        out[4..6].copy_from_slice(&self.sitelinks.to_le_bytes());
        out[6] = self.kind as u8;
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<HotRecord> {
        if bytes.len() != HOT_RECORD_LEN {
            return None;
        }
        Some(HotRecord {
            qid: u32::from_le_bytes(bytes[0..4].try_into().ok()?),
            sitelinks: u16::from_le_bytes(bytes[4..6].try_into().ok()?),
            kind: Kind::from_u8(bytes[6])?,
        })
    }

    pub fn qid_string(self) -> String {
        format!("Q{}", self.qid)
    }
}

/// Display-only fields, read once per returned match.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColdRecord {
    pub label: String,
    pub description: String,
    pub birth_year: Option<i32>,
    pub death_year: Option<i32>,
    /// English Wikipedia title; empty when there is no English article.
    pub enwiki_title: String,
    /// Gender, occupations, awards and the other category attributes.
    pub attributes: Attributes,
}

impl ColdRecord {
    fn texts(&self) -> [&str; 3] {
        [&self.label, &self.description, &self.enwiki_title].map(|text| clip(text))
    }

    pub fn encoded_len(&self) -> usize {
        8 + self.attributes.encoded_len()
            + self.texts().iter().map(|text| 2 + text.len()).sum::<usize>()
    }

    /// Layout: birth and death year (`i32`), the attribute lists, then three
    /// strings (a `u16` length and UTF-8 bytes).
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.birth_year.unwrap_or(NO_YEAR).to_le_bytes());
        out.extend_from_slice(&self.death_year.unwrap_or(NO_YEAR).to_le_bytes());
        self.attributes.encode(out);
        for text in self.texts() {
            out.extend_from_slice(&(text.len() as u16).to_le_bytes());
            out.extend_from_slice(text.as_bytes());
        }
    }

    pub fn decode(bytes: &[u8]) -> Option<ColdRecord> {
        let year = |raw: &[u8]| -> Option<Option<i32>> {
            let value = i32::from_le_bytes(raw.try_into().ok()?);
            Some((value != NO_YEAR).then_some(value))
        };
        let birth_year = year(bytes.get(0..4)?)?;
        let death_year = year(bytes.get(4..8)?)?;
        let (attributes, used) = Attributes::decode(bytes.get(8..)?)?;
        let mut pos = 8 + used;
        let mut text = || -> Option<String> {
            let len = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
            let value = std::str::from_utf8(bytes.get(pos + 2..pos + 2 + len)?).ok()?;
            pos += 2 + len;
            Some(value.to_owned())
        };
        Some(ColdRecord {
            birth_year,
            death_year,
            label: text()?,
            description: text()?,
            enwiki_title: text()?,
            attributes,
        })
    }
}

/// Longest prefix of `s` that fits a `u16` length, cut on a char boundary.
fn clip(s: &str) -> &str {
    let mut end = s.len().min(u16::MAX as usize);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Counts {
    /// Entity lines read from the dump.
    pub entities_read: u64,
    /// Lines that were not valid entity JSON.
    pub parse_errors: u64,
    /// Entities that are neither human nor a listed fictional class.
    pub dropped_not_person: u64,
    /// People below `min_sitelinks`.
    pub dropped_below_min_sitelinks: u64,
    /// People with no usable name in the indexed languages.
    pub dropped_no_name: u64,
    pub records: u64,
    pub women: u64,
    pub not_women: u64,
    pub fictional: u64,
    /// Distinct normalized names (FST keys).
    pub names: u64,
    /// Names that map to more than one record.
    pub shared_names: u64,
    /// Total (name, record) pairs.
    pub name_record_pairs: u64,
    /// Kept humans per "sex or gender" value, keyed by QID, with `none` for
    /// humans that have no current value. Someone with two values counts
    /// under both. Fictional records are not counted.
    pub gender_values: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sizes {
    pub names_fst: u64,
    pub postings_bin: u64,
    pub records_hot_bin: u64,
    pub records_cold_bin: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// UTC date of the build, `YYYY-MM-DD`.
    pub build_date: String,
    /// Date of the Wikidata dump, `YYYY-MM-DD`, when known.
    pub dump_date: Option<String>,
    pub langs: Vec<String>,
    /// Wikidata properties stored as per-person attributes, in on-disk order.
    pub attributes: Vec<String>,
    pub min_sitelinks: u32,
    pub counts: Counts,
    pub sizes: Sizes,
    pub build_seconds: f64,
    /// Peak resident memory of the builder, where the OS reports it.
    pub peak_rss_mb: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::Attribute;

    #[test]
    fn hot_record_round_trip() {
        let record = HotRecord { qid: 11116, sitelinks: 73, kind: Kind::Woman };
        assert_eq!(HotRecord::from_bytes(&record.to_bytes()), Some(record));
        assert_eq!(record.qid_string(), "Q11116");
    }

    #[test]
    fn cold_record_round_trip() {
        let attributes = Attributes::from_fn(|attribute| match attribute {
            Attribute::Gender => vec![6581072],
            Attribute::NobleTitle => vec![37110],
            _ => Vec::new(),
        });
        let record = ColdRecord {
            label: "Cleopatra".into(),
            description: "Queen of Egypt".into(),
            birth_year: Some(-69),
            death_year: None,
            enwiki_title: String::new(),
            attributes,
        };
        let mut bytes = Vec::new();
        record.encode(&mut bytes);
        assert_eq!(bytes.len(), record.encoded_len());
        assert_eq!(ColdRecord::decode(&bytes), Some(record));
    }

    #[test]
    fn cold_record_rejects_truncated_input() {
        let mut bytes = Vec::new();
        ColdRecord { label: "Cher".into(), ..Default::default() }.encode(&mut bytes);
        assert_eq!(ColdRecord::decode(&bytes[..bytes.len() - 1]), None);
    }
}
