//! The answer to a lookup: the JSON the server returns and the line the CLI prints.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Empty, too long, or without a single letter.
    NotAName,
    /// A real human who is not recorded as a woman; see `reason_detail`.
    NotWoman,
    Fictional,
    /// Several people share the name and none clearly dominates.
    Ambiguous,
    NotFound,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NotAName => "not_a_name",
            Reason::NotWoman => "not_woman",
            Reason::Fictional => "fictional",
            Reason::Ambiguous => "ambiguous",
            Reason::NotFound => "not_found",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedVia {
    Exact,
    Fuzzy,
}

impl MatchedVia {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchedVia::Exact => "exact",
            MatchedVia::Fuzzy => "fuzzy",
        }
    }
}

/// A person, as shown to the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub qid: String,
    pub name: String,
    pub description: String,
    pub birth_year: Option<i32>,
    pub death_year: Option<i32>,
    pub sitelinks: u32,
    /// Null when the person has no English Wikipedia article.
    pub wikipedia_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub input: String,
    pub normalized: String,
    pub verdict: Verdict,
    /// Null when accepted.
    pub reason: Option<Reason>,
    /// Why, in words, for `not_woman`: "gender is genderfluid".
    pub reason_detail: Option<String>,
    /// Null when nothing matched.
    pub matched_via: Option<MatchedVia>,
    /// The normalized input, when a fuzzy match corrected it.
    pub corrected_from: Option<String>,
    /// The person the name resolved to. Set for accepted names and for
    /// `not_woman` and `fictional` rejections.
    #[serde(rename = "match")]
    pub matched: Option<Match>,
    /// Up to five people, most famous first; filled only for `ambiguous`.
    pub candidates: Vec<Match>,
    pub latency_us: u64,
}

impl Response {
    pub fn is_accepted(&self) -> bool {
        self.verdict == Verdict::Accepted
    }

    /// One line for a terminal:
    /// `✓ Ruth Bader Ginsburg (Q11116) — American lawyer and jurist, 1933–2020 [fuzzy, corrected]`
    /// or `✗ Elon Musk (Q317521) — not a woman: gender is male`.
    pub fn summary_line(&self) -> String {
        let person = |m: &Match| format!("{} ({})", m.name, m.qid);
        let typed = || format!("\"{}\"", self.input.trim());
        match (self.reason, &self.matched) {
            (None, Some(m)) => {
                let mut facts = Vec::new();
                if !m.description.is_empty() {
                    facts.push(m.description.clone());
                }
                // Many Wikidata descriptions already end in "(1933–2020)".
                let years_in_description = [m.birth_year, m.death_year]
                    .iter()
                    .flatten()
                    .any(|year| m.description.contains(&year.abs().to_string()));
                if !years_in_description {
                    facts.extend(lifespan(m.birth_year, m.death_year));
                }
                let mut line = format!("✓ {}", person(m));
                if !facts.is_empty() {
                    line.push_str(&format!(" — {}", facts.join(", ")));
                }
                match (self.matched_via, &self.corrected_from) {
                    (Some(via), Some(_)) => line.push_str(&format!(" [{}, corrected]", via.as_str())),
                    (Some(via), None) => line.push_str(&format!(" [{}]", via.as_str())),
                    (None, _) => {}
                }
                line
            }
            (Some(Reason::NotWoman), Some(m)) => match &self.reason_detail {
                Some(detail) => format!("✗ {} — not a woman: {detail}", person(m)),
                None => format!("✗ {} — not a woman", person(m)),
            },
            (Some(Reason::Fictional), Some(m)) => format!("✗ {} — fictional", person(m)),
            (Some(Reason::Ambiguous), _) => {
                let names: Vec<String> = self.candidates.iter().map(person).collect();
                format!("✗ {} — ambiguous: {}", typed(), names.join(", "))
            }
            (Some(Reason::NotAName), _) => format!("✗ {} — not a name", typed()),
            (Some(Reason::NotFound), _) => format!("✗ {} — not found", typed()),
            (Some(reason), None) => format!("✗ {} — {}", typed(), reason.as_str()),
            (None, None) => format!("✓ {}", typed()),
        }
    }
}

/// "1933–2020", "born 1958", "died 1431", or nothing.
fn lifespan(birth: Option<i32>, death: Option<i32>) -> Option<String> {
    let year = |y: i32| if y < 0 { format!("{} BC", -y) } else { y.to_string() };
    match (birth, death) {
        (Some(b), Some(d)) => Some(format!("{}–{}", year(b), year(d))),
        (Some(b), None) => Some(format!("born {}", year(b))),
        (None, Some(d)) => Some(format!("died {}", year(d))),
        (None, None) => None,
    }
}

/// The English Wikipedia URL for an article title.
pub fn wikipedia_url(title: &str) -> Option<String> {
    if title.is_empty() {
        return None;
    }
    let mut url = String::from("https://en.wikipedia.org/wiki/");
    for byte in title.bytes() {
        match byte {
            b' ' => url.push('_'),
            // Unreserved characters, plus the punctuation Wikipedia leaves bare in titles.
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => url.push(byte as char),
            b'-' | b'_' | b'.' | b'~' | b'(' | b')' | b',' | b'\'' | b':' | b'!' | b'*' | b'/' => {
                url.push(byte as char)
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rbg() -> Match {
        Match {
            qid: "Q11116".into(),
            name: "Ruth Bader Ginsburg".into(),
            description: "American lawyer and jurist".into(),
            birth_year: Some(1933),
            death_year: Some(2020),
            sitelinks: 120,
            wikipedia_url: wikipedia_url("Ruth Bader Ginsburg"),
        }
    }

    fn response(reason: Option<Reason>, matched: Option<Match>) -> Response {
        Response {
            input: "Ruth Bader Ginsberg".into(),
            normalized: "ruth bader ginsberg".into(),
            verdict: if reason.is_none() { Verdict::Accepted } else { Verdict::Rejected },
            reason,
            reason_detail: None,
            matched_via: matched.as_ref().map(|_| MatchedVia::Fuzzy),
            corrected_from: matched.as_ref().map(|_| "ruth bader ginsberg".into()),
            matched,
            candidates: Vec::new(),
            latency_us: 41,
        }
    }

    #[test]
    fn json_has_the_spec_shape() {
        let json = serde_json::to_value(response(None, Some(rbg()))).unwrap();
        let expected = serde_json::json!({
            "input": "Ruth Bader Ginsberg",
            "normalized": "ruth bader ginsberg",
            "verdict": "accepted",
            "reason": null,
            "reason_detail": null,
            "matched_via": "fuzzy",
            "corrected_from": "ruth bader ginsberg",
            "match": {
                "qid": "Q11116",
                "name": "Ruth Bader Ginsburg",
                "description": "American lawyer and jurist",
                "birth_year": 1933,
                "death_year": 2020,
                "sitelinks": 120,
                "wikipedia_url": "https://en.wikipedia.org/wiki/Ruth_Bader_Ginsburg"
            },
            "candidates": [],
            "latency_us": 41
        });
        assert_eq!(json, expected);
        let back: Response = serde_json::from_value(json).unwrap();
        assert_eq!(back, response(None, Some(rbg())));
    }

    #[test]
    fn rejection_reasons_serialize_in_snake_case() {
        let json = serde_json::to_value(response(Some(Reason::NotAName), None)).unwrap();
        assert_eq!(json["verdict"], "rejected");
        assert_eq!(json["reason"], "not_a_name");
        assert_eq!(json["match"], serde_json::Value::Null);
    }

    #[test]
    fn summary_lines() {
        assert_eq!(
            response(None, Some(rbg())).summary_line(),
            "✓ Ruth Bader Ginsburg (Q11116) — American lawyer and jurist, 1933–2020 [fuzzy, corrected]"
        );
        let mut rejected = response(Some(Reason::NotWoman), Some(rbg()));
        rejected.reason_detail = Some("gender is male".into());
        assert_eq!(rejected.summary_line(), "✗ Ruth Bader Ginsburg (Q11116) — not a woman: gender is male");
        assert_eq!(
            response(Some(Reason::Fictional), Some(rbg())).summary_line(),
            "✗ Ruth Bader Ginsburg (Q11116) — fictional"
        );
        assert_eq!(
            response(Some(Reason::NotFound), None).summary_line(),
            "✗ \"Ruth Bader Ginsberg\" — not found"
        );
        let mut dated = rbg();
        dated.description = "American lawyer and jurist (1933–2020)".into();
        assert_eq!(
            response(None, Some(dated)).summary_line(),
            "✓ Ruth Bader Ginsburg (Q11116) — American lawyer and jurist (1933–2020) [fuzzy, corrected]"
        );
        let mut ambiguous = response(Some(Reason::Ambiguous), None);
        ambiguous.candidates = vec![rbg(), rbg()];
        assert_eq!(
            ambiguous.summary_line(),
            "✗ \"Ruth Bader Ginsberg\" — ambiguous: Ruth Bader Ginsburg (Q11116), Ruth Bader Ginsburg (Q11116)"
        );
    }

    #[test]
    fn lifespans() {
        assert_eq!(lifespan(Some(-69), Some(-30)).as_deref(), Some("69 BC–30 BC"));
        assert_eq!(lifespan(Some(1958), None).as_deref(), Some("born 1958"));
        assert_eq!(lifespan(None, Some(1431)).as_deref(), Some("died 1431"));
        assert_eq!(lifespan(None, None), None);
    }

    #[test]
    fn wikipedia_urls() {
        assert_eq!(wikipedia_url(""), None);
        assert_eq!(
            wikipedia_url("Beyoncé").as_deref(),
            Some("https://en.wikipedia.org/wiki/Beyonc%C3%A9")
        );
        assert_eq!(
            wikipedia_url("Sinéad O'Connor").as_deref(),
            Some("https://en.wikipedia.org/wiki/Sin%C3%A9ad_O'Connor")
        );
        assert_eq!(
            wikipedia_url("Pink (singer)").as_deref(),
            Some("https://en.wikipedia.org/wiki/Pink_(singer)")
        );
        assert_eq!(
            wikipedia_url("Who? What").as_deref(),
            Some("https://en.wikipedia.org/wiki/Who%3F_What")
        );
    }
}
