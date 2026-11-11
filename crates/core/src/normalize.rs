//! Name normalization, shared by the index builder and the lookup path.
//!
//! Both sides must agree byte for byte, so this is the only place a name is
//! ever turned into an index key.

use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// Normalize a name into its index key.
///
/// 1. Unicode NFKD, then drop combining marks ("Beyoncé" → "beyonce").
/// 2. Full case fold.
/// 3. Drop apostrophes and periods ("O'Keeffe" → "okeeffe", "J.K." → "jk");
///    map every other non-alphanumeric character to a space.
/// 4. Collapse whitespace and trim.
///
/// The result is a fixed point: `normalize(normalize(x)) == normalize(x)`.
pub fn normalize(input: &str) -> String {
    let mut current = pass(input);
    // Case folding can produce characters that decompose further, so one pass
    // is not always a fixed point. Re-run until it is; real names settle on
    // the first check.
    for _ in 0..8 {
        let next = pass(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn pass(input: &str) -> String {
    let stripped: String = input.nfkd().filter(|c| !is_combining_mark(*c)).collect();
    let folded = caseless::default_case_fold_str(&stripped);

    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    for c in folded.chars() {
        if is_dropped(c) {
            continue;
        }
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    out
}

/// Characters removed outright, joining what is on either side of them.
fn is_dropped(c: char) -> bool {
    matches!(
        c,
        '.' | '\''
            | '\u{2018}' // ‘
            | '\u{2019}' // ’
            | '\u{201B}' // ‛
            | '\u{02BC}' // ʼ modifier apostrophe
            | '\u{02BB}' // ʻ okina
            | '\u{02B9}' // ʹ modifier prime
            | '\u{2032}' // ′ prime
            | '\u{FF07}' // ＇ fullwidth apostrophe
            | '`'
            | '\u{00B4}' // ´
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn spec_examples() {
        assert_eq!(normalize("Beyoncé"), "beyonce");
        assert_eq!(normalize("O'Keeffe"), "okeeffe");
        assert_eq!(normalize("J.K."), "jk");
        assert_eq!(normalize("J. K. Rowling"), "j k rowling");
    }

    #[test]
    fn case_and_whitespace() {
        assert_eq!(normalize("  RUTH   Bader\tGinsburg "), "ruth bader ginsburg");
        assert_eq!(normalize("Straße"), "strasse");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize(" .' - "), "");
    }

    #[test]
    fn punctuation_and_hyphens_become_spaces() {
        assert_eq!(normalize("Alexandria Ocasio-Cortez"), "alexandria ocasio cortez");
        assert_eq!(normalize("Diana, Princess of Wales"), "diana princess of wales");
        assert_eq!(normalize("Jay-Z"), "jay z");
        assert_eq!(normalize("Sinéad O’Connor"), "sinead oconnor");
    }

    #[test]
    fn diacritics_and_compatibility_forms() {
        assert_eq!(normalize("Björk"), "bjork");
        assert_eq!(normalize("Nadia Comăneci"), "nadia comaneci");
        assert_eq!(normalize("Ｌａｄｙ　Ｇａｇａ"), "lady gaga");
        assert_eq!(normalize("ﬁona"), "fiona");
    }

    #[test]
    fn digits_are_kept() {
        assert_eq!(normalize("1234"), "1234");
        assert_eq!(normalize("Elizabeth II"), "elizabeth ii");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4000))]

        #[test]
        fn idempotent(s in "\\PC*") {
            let once = normalize(&s);
            prop_assert_eq!(normalize(&once), once);
        }

        #[test]
        fn idempotent_on_namelike_input(s in "[ A-Za-zÀ-ɏ'’.,\\-]{0,40}") {
            let once = normalize(&s);
            prop_assert_eq!(normalize(&once), once);
        }

        #[test]
        fn output_is_clean(s in "\\PC*") {
            let n = normalize(&s);
            prop_assert!(!n.starts_with(' ') && !n.ends_with(' '));
            prop_assert!(!n.contains("  "));
            prop_assert!(n.chars().all(|c| c == ' ' || c.is_alphanumeric()));
        }
    }
}
