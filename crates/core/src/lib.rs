//! Shared pieces of the Name 100 Women lookup server: normalization, the index
//! artifact format, and the index reader.

pub mod artifact;
pub mod attribute;
pub mod catalogue;
pub mod category;
pub mod gender;
pub mod index;
pub mod matcher;
pub mod normalize;
pub mod verdict;

pub use attribute::{Attribute, Attributes};
pub use catalogue::{Catalogue, CatalogueData, Entry, Term};
pub use artifact::{ColdRecord, Counts, HotRecord, Kind, Manifest, Sizes};
pub use gender::{describe_genders, gender_label, WOMAN_GENDERS};
pub use index::{Hits, Index, IndexError};
pub use matcher::{MatchConfig, Matcher, StageTimings};
pub use normalize::normalize;
pub use verdict::{Match, MatchedAs, MatchedVia, Reason, Response, Verdict};
