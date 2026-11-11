//! Read side of an index artifact.

use std::fs;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::attribute::Attribute;
use crate::artifact::{
    ColdRecord, HotRecord, Manifest, COLD_FILE, COLD_MAGIC, HOT_FILE, HOT_MAGIC, HOT_RECORD_LEN,
    MAGIC_LEN, MANIFEST_FILE, NAMES_FILE, POSTINGS_FILE, POSTINGS_MAGIC, SCHEMA_VERSION,
    SHARED_NAME_BIT,
};

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {message}")]
    Corrupt { path: PathBuf, message: String },
    #[error("index stores attributes {found:?} but this build expects {expected:?}; rebuild the index")]
    Attributes { found: Vec<String>, expected: Vec<String> },
    #[error("index schema version {found} is not supported (expected {expected}); rebuild the index")]
    SchemaVersion { found: u32, expected: u32 },
}

/// The records a name maps to, most popular first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hits<'a> {
    One(u32),
    Many(&'a [u32]),
}

impl Hits<'_> {
    pub fn as_slice(&self) -> &[u32] {
        match self {
            Hits::One(index) => std::slice::from_ref(index),
            Hits::Many(indices) => indices,
        }
    }
}

/// A loaded index. Names, postings and hot records are held in memory;
/// cold records are memory mapped and paged in on demand.
pub struct Index {
    names: fst::Map<Vec<u8>>,
    postings: Vec<u32>,
    hot: Vec<HotRecord>,
    cold: Mmap,
    cold_path: PathBuf,
    manifest: Manifest,
}

impl Index {
    pub fn open(dir: &Path) -> Result<Index, IndexError> {
        let manifest_path = dir.join(MANIFEST_FILE);
        let manifest: Manifest = serde_json::from_slice(&read(&manifest_path)?)
            .map_err(|e| corrupt(&manifest_path, e.to_string()))?;
        if manifest.schema_version != SCHEMA_VERSION {
            return Err(IndexError::SchemaVersion {
                found: manifest.schema_version,
                expected: SCHEMA_VERSION,
            });
        }

        if manifest.attributes != Attribute::properties() {
            return Err(IndexError::Attributes {
                found: manifest.attributes,
                expected: Attribute::properties(),
            });
        }

        let names_path = dir.join(NAMES_FILE);
        let names = fst::Map::new(read(&names_path)?)
            .map_err(|e| corrupt(&names_path, e.to_string()))?;

        let postings_path = dir.join(POSTINGS_FILE);
        let postings_bytes = read(&postings_path)?;
        let postings_body = body(&postings_path, &postings_bytes, POSTINGS_MAGIC, 4)?;
        let postings = postings_body
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect();

        let hot_path = dir.join(HOT_FILE);
        let hot_bytes = read(&hot_path)?;
        let hot = body(&hot_path, &hot_bytes, HOT_MAGIC, HOT_RECORD_LEN)?
            .chunks_exact(HOT_RECORD_LEN)
            .map(|chunk| {
                HotRecord::from_bytes(chunk)
                    .ok_or_else(|| corrupt(&hot_path, "record with unknown kind".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let cold_path = dir.join(COLD_FILE);
        let cold_file = fs::File::open(&cold_path)
            .map_err(|source| IndexError::Io { path: cold_path.clone(), source })?;
        // Safety: artifacts are immutable once written; nothing truncates or
        // rewrites the file while the index is open.
        let cold = unsafe { Mmap::map(&cold_file) }
            .map_err(|source| IndexError::Io { path: cold_path.clone(), source })?;
        let cold_count = cold
            .get(..MAGIC_LEN + 8)
            .filter(|head| &head[..MAGIC_LEN] == COLD_MAGIC)
            .map(|head| u64::from_le_bytes(head[MAGIC_LEN..].try_into().unwrap()))
            .ok_or_else(|| corrupt(&cold_path, "bad header".into()))?;
        if cold_count != hot.len() as u64 {
            return Err(corrupt(
                &cold_path,
                format!("{cold_count} cold records but {} hot records", hot.len()),
            ));
        }
        if (cold.len() as u64) < (MAGIC_LEN as u64 + 8 + (cold_count + 1) * 8) {
            return Err(corrupt(&cold_path, "truncated offset table".into()));
        }

        Ok(Index { names, postings, hot, cold, cold_path, manifest })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Number of records.
    pub fn len(&self) -> usize {
        self.hot.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hot.is_empty()
    }

    /// The name FST, for automaton searches.
    pub fn names(&self) -> &fst::Map<Vec<u8>> {
        &self.names
    }

    /// Records whose label or alias normalizes to exactly `normalized`.
    pub fn exact(&self, normalized: &str) -> Option<Hits<'_>> {
        self.names.get(normalized).map(|value| self.hits(value))
    }

    /// Decode a value stored in the name FST.
    pub fn hits(&self, value: u64) -> Hits<'_> {
        if value & SHARED_NAME_BIT == 0 {
            return Hits::One(value as u32);
        }
        let start = (value & !SHARED_NAME_BIT) as usize;
        let len = self.postings.get(start).copied().unwrap_or(0) as usize;
        Hits::Many(self.postings.get(start + 1..start + 1 + len).unwrap_or(&[]))
    }

    pub fn hot(&self, index: u32) -> Option<HotRecord> {
        self.hot.get(index as usize).copied()
    }

    pub fn cold(&self, index: u32) -> Result<ColdRecord, IndexError> {
        let bad = |message: &str| corrupt(&self.cold_path, format!("record {index}: {message}"));
        if index as usize >= self.hot.len() {
            return Err(bad("out of range"));
        }
        let table = MAGIC_LEN + 8 + index as usize * 8;
        let offset = |at: usize| {
            u64::from_le_bytes(self.cold[at..at + 8].try_into().unwrap()) as usize
        };
        let (start, end) = (offset(table), offset(table + 8));
        let bytes = self.cold.get(start..end).ok_or_else(|| bad("offsets out of bounds"))?;
        ColdRecord::decode(bytes).ok_or_else(|| bad("undecodable"))
    }
}

fn read(path: &Path) -> Result<Vec<u8>, IndexError> {
    fs::read(path).map_err(|source| IndexError::Io { path: path.to_owned(), source })
}

fn corrupt(path: &Path, message: String) -> IndexError {
    IndexError::Corrupt { path: path.to_owned(), message }
}

/// Check a file's magic and return what follows, which must be whole `unit`s.
fn body<'a>(
    path: &Path,
    bytes: &'a [u8],
    magic: &[u8; MAGIC_LEN],
    unit: usize,
) -> Result<&'a [u8], IndexError> {
    let rest = bytes
        .strip_prefix(magic.as_slice())
        .ok_or_else(|| corrupt(path, "bad magic".into()))?;
    if rest.len() % unit != 0 {
        return Err(corrupt(path, "truncated".into()));
    }
    Ok(rest)
}
