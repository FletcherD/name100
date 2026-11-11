//! Shared pieces of the Name 100 Women lookup server: normalization, the index
//! artifact format, and the index reader.

pub mod artifact;
pub mod attribute;
pub mod gender;
pub mod index;
pub mod normalize;

pub use attribute::{Attribute, Attributes};
pub use artifact::{ColdRecord, Counts, HotRecord, Kind, Manifest, Sizes};
pub use gender::{describe_genders, gender_label, WOMAN_GENDERS};
pub use index::{Hits, Index, IndexError};
pub use normalize::normalize;
