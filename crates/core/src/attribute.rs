//! Per-person attributes: the Wikidata facts a category game filters on
//! ("name 100 actors", "name 100 Nobel laureates").
//!
//! Each attribute is one Wikidata property whose values are items; a person
//! holds a list of numeric QIDs per attribute. They are stored for every
//! person, accepted or not, because a different game may accept them.
//!
//! The order of [`Attribute::ALL`] is the on-disk order. Adding, removing or
//! reordering attributes changes the artifact: the manifest records the list
//! and the reader refuses an index built with a different one.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Attribute {
    /// Sex or gender. Only the current (best-ranked) values are kept.
    Gender,
    Occupation,
    Citizenship,
    Award,
    Position,
    Sport,
    /// Events taken part in, such as an Olympic Games.
    Event,
    Genre,
    Instrument,
    FieldOfWork,
    NobleTitle,
}

impl Attribute {
    pub const COUNT: usize = 11;

    pub const ALL: [Attribute; Attribute::COUNT] = [
        Attribute::Gender,
        Attribute::Occupation,
        Attribute::Citizenship,
        Attribute::Award,
        Attribute::Position,
        Attribute::Sport,
        Attribute::Event,
        Attribute::Genre,
        Attribute::Instrument,
        Attribute::FieldOfWork,
        Attribute::NobleTitle,
    ];

    /// The Wikidata property the values come from.
    pub fn property(self) -> &'static str {
        match self {
            Attribute::Gender => "P21",
            Attribute::Occupation => "P106",
            Attribute::Citizenship => "P27",
            Attribute::Award => "P166",
            Attribute::Position => "P39",
            Attribute::Sport => "P641",
            Attribute::Event => "P1344",
            Attribute::Genre => "P136",
            Attribute::Instrument => "P1303",
            Attribute::FieldOfWork => "P101",
            Attribute::NobleTitle => "P97",
        }
    }

    /// Wikidata's English name for the property.
    pub fn name(self) -> &'static str {
        match self {
            Attribute::Gender => "sex or gender",
            Attribute::Occupation => "occupation",
            Attribute::Citizenship => "country of citizenship",
            Attribute::Award => "award received",
            Attribute::Position => "position held",
            Attribute::Sport => "sport",
            Attribute::Event => "participant in",
            Attribute::Genre => "genre",
            Attribute::Instrument => "instrument",
            Attribute::FieldOfWork => "field of work",
            Attribute::NobleTitle => "noble title",
        }
    }

    /// The property IDs of every attribute, in on-disk order.
    pub fn properties() -> Vec<String> {
        Attribute::ALL.iter().map(|a| a.property().to_owned()).collect()
    }
}

/// Values kept per attribute. Far above anything in Wikidata; the busiest
/// lists are a few hundred tournaments for a tennis player.
pub const MAX_VALUES: usize = u16::MAX as usize;

/// One person's attribute values: a list of numeric QIDs per [`Attribute`].
///
/// Kept as one allocation, because the builder holds millions of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attributes {
    /// Every list, concatenated in [`Attribute::ALL`] order.
    values: Vec<u32>,
    /// Where each attribute's list ends in `values`.
    ends: [u32; Attribute::COUNT],
}

impl Attributes {
    /// Build from one list per attribute. Lists longer than [`MAX_VALUES`] are cut.
    pub fn from_fn(mut list: impl FnMut(Attribute) -> Vec<u32>) -> Attributes {
        let mut attributes = Attributes::default();
        for attribute in Attribute::ALL {
            let mut values = list(attribute);
            values.truncate(MAX_VALUES);
            attributes.values.extend_from_slice(&values);
            attributes.ends[attribute as usize] = attributes.values.len() as u32;
        }
        attributes.values.shrink_to_fit();
        attributes
    }

    pub fn get(&self, attribute: Attribute) -> &[u32] {
        let index = attribute as usize;
        let start = if index == 0 { 0 } else { self.ends[index - 1] };
        &self.values[start as usize..self.ends[index] as usize]
    }

    /// Whether the person holds any of `qids` for `attribute`. A category is
    /// usually several values: "actor" also means film actor, stage actor, ...
    pub fn has_any(&self, attribute: Attribute, qids: &[u32]) -> bool {
        self.get(attribute).iter().any(|value| qids.contains(value))
    }

    pub(crate) fn encoded_len(&self) -> usize {
        1 + Attribute::ALL.iter().map(|&a| count_len(self.get(a).len())).sum::<usize>()
            + 4 * self.values.len()
    }

    /// A count of lists, then per list its length and that many `u32`s. The
    /// length is one byte, or `0xFF` followed by a `u16` for 255 values or more.
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(Attribute::COUNT as u8);
        for attribute in Attribute::ALL {
            let list = self.get(attribute);
            if list.len() < LONG_LIST as usize {
                out.push(list.len() as u8);
            } else {
                out.push(LONG_LIST);
                out.extend_from_slice(&(list.len() as u16).to_le_bytes());
            }
            for qid in list {
                out.extend_from_slice(&qid.to_le_bytes());
            }
        }
    }

    /// Decode from the front of `bytes`; returns the attributes and the bytes used.
    pub(crate) fn decode(bytes: &[u8]) -> Option<(Attributes, usize)> {
        if *bytes.first()? as usize != Attribute::COUNT {
            return None;
        }
        let mut pos = 1;
        let mut attributes = Attributes::default();
        for end in &mut attributes.ends {
            let mut count = *bytes.get(pos)? as usize;
            pos += 1;
            if count == LONG_LIST as usize {
                count = u16::from_le_bytes(bytes.get(pos..pos + 2)?.try_into().ok()?) as usize;
                pos += 2;
            }
            let raw = bytes.get(pos..pos + 4 * count)?;
            attributes
                .values
                .extend(raw.chunks_exact(4).map(|id| u32::from_le_bytes(id.try_into().unwrap())));
            pos += 4 * count;
            *end = attributes.values.len() as u32;
        }
        Some((attributes, pos))
    }
}

/// Length byte that means "a `u16` length follows".
const LONG_LIST: u8 = 0xFF;

fn count_len(values: usize) -> usize {
    if values < LONG_LIST as usize { 1 } else { 3 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_attribute_in_discriminant_order() {
        for (position, attribute) in Attribute::ALL.iter().enumerate() {
            assert_eq!(*attribute as usize, position);
        }
        let mut properties = Attribute::properties();
        properties.sort();
        properties.dedup();
        assert_eq!(properties.len(), Attribute::COUNT);
    }

    fn sample(awards: usize) -> Attributes {
        Attributes::from_fn(|attribute| match attribute {
            Attribute::Occupation => vec![33999, 10800557],
            Attribute::Award => (0..awards as u32).collect(),
            Attribute::NobleTitle => vec![1],
            _ => Vec::new(),
        })
    }

    #[test]
    fn round_trip() {
        // 254 and 255 values sit either side of the long-list encoding.
        for awards in [0, 3, 254, 255, 256, 1000] {
            let attributes = sample(awards);
            assert_eq!(attributes.get(Attribute::Occupation), [33999, 10800557]);
            assert_eq!(attributes.get(Attribute::Award).len(), awards);
            assert_eq!(attributes.get(Attribute::NobleTitle), [1]);
            assert!(attributes.get(Attribute::Gender).is_empty());

            let mut bytes = Vec::new();
            attributes.encode(&mut bytes);
            assert_eq!(bytes.len(), attributes.encoded_len());
            bytes.extend_from_slice(b"rest");
            let (decoded, used) = Attributes::decode(&bytes).unwrap();
            assert_eq!(decoded, attributes);
            assert_eq!(&bytes[used..], b"rest");
        }
    }

    #[test]
    fn membership_and_truncation() {
        let attributes = sample(10);
        assert!(attributes.has_any(Attribute::Award, &[5000, 7]));
        assert!(!attributes.has_any(Attribute::Award, &[5000]));
        assert!(!attributes.has_any(Attribute::Sport, &[7]));
        let huge = Attributes::from_fn(|_| vec![7; MAX_VALUES + 10]);
        assert_eq!(huge.get(Attribute::Event).len(), MAX_VALUES);
    }

    #[test]
    fn an_index_with_a_different_attribute_count_does_not_decode() {
        let mut bytes = Vec::new();
        Attributes::default().encode(&mut bytes);
        bytes[0] += 1;
        assert!(Attributes::decode(&bytes).is_none());
    }
}
