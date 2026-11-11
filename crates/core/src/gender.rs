//! Wikidata "sex or gender" (`P21`) values: which ones are accepted, and how
//! to name the others when explaining a rejection.
//!
//! Records store the numeric QIDs of a person's current `P21` values; the
//! names live here so they can be corrected without rebuilding the index.

/// `P21` values that make a human a `woman` record.
pub const WOMAN_GENDERS: &[u32] = &[
    6581072,  // female
    1052281,  // trans woman
    15145779, // cisgender woman
];

/// English names of the `P21` values seen on people, as labelled in Wikidata.
const LABELS: &[(u32, &str)] = &[
    (6581097, "male"),
    (6581072, "female"),
    (1052281, "trans woman"),
    (2449503, "trans man"),
    (15145778, "cisgender man"),
    (15145779, "cisgender woman"),
    (48270, "non-binary"),
    (18116794, "genderfluid"),
    (12964198, "genderqueer"),
    (505371, "agender"),
    (859614, "bigender"),
    (7130936, "pangender"),
    (1289754, "neutrois"),
    (93954933, "demiboy"),
    (93955709, "demigirl"),
    (189125, "transgender"),
    (27679766, "transmasculine"),
    (27679684, "transfeminine"),
    (1097630, "intersex"),
    (48279, "third gender"),
    (301702, "two-spirit"),
    (660882, "hijra"),
    (746411, "kathoey"),
    (3277905, "māhū"),
    (350374, "fakaleitī"),
    (3177577, "muxe"),
    (207959, "androgyny"),
    (179294, "eunuch"),
    (52261234, "neutral sex"),
];

/// The English name of a gender value, if it is one of the known ones.
pub fn gender_label(qid: u32) -> Option<&'static str> {
    LABELS.iter().find(|(id, _)| *id == qid).map(|(_, label)| *label)
}

/// Why a person with these `P21` values is not accepted, for showing to a
/// player: "gender is genderfluid", "gender is non-binary, trans man", or
/// "no gender recorded" when Wikidata has no current value. A value missing
/// from the table is shown as its QID.
pub fn describe_genders(genders: &[u32]) -> String {
    if genders.is_empty() {
        return "no gender recorded".to_owned();
    }
    let names: Vec<String> = genders
        .iter()
        .map(|&qid| gender_label(qid).map(str::to_owned).unwrap_or_else(|| format!("Q{qid}")))
        .collect();
    format!("gender is {}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions() {
        assert_eq!(describe_genders(&[18116794]), "gender is genderfluid");
        assert_eq!(describe_genders(&[6581097]), "gender is male");
        assert_eq!(describe_genders(&[48270, 2449503]), "gender is non-binary, trans man");
        assert_eq!(describe_genders(&[]), "no gender recorded");
        assert_eq!(describe_genders(&[999999999]), "gender is Q999999999");
    }

    #[test]
    fn every_accepted_gender_has_a_label() {
        assert!(WOMAN_GENDERS.iter().all(|&qid| gender_label(qid).is_some()));
    }
}
