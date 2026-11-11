//! The lookup pipeline: input guard, exact match, fuzzy match, resolution.

use std::time::{Duration, Instant};

use fst::{IntoStreamer, Streamer};
use levenshtein_automata::{Distance, LevenshteinAutomatonBuilder};
use serde::{Deserialize, Serialize};

use crate::artifact::Kind;
use crate::attribute::Attribute;
use crate::catalogue::Term;
use crate::category::{category_label, rank_categories, ROYALTY};
use crate::gender::{describe_genders, gender_label};
use crate::index::{Index, IndexError};
use crate::normalize::normalize;
use crate::artifact::ColdRecord;
use crate::verdict::{wikipedia_url, Match, MatchedAs, MatchedVia, Reason, Response, Verdict};

/// Tunables of the lookup pipeline. Every field has the spec's default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatchConfig {
    /// A name shared by several people resolves to the most famous one only
    /// if they have at least this many times the runner-up's sitelinks.
    pub dominance_ratio: f64,
    /// Sitelinks a person needs before a single-word name resolves to them.
    pub mononym_min_sitelinks: u32,
    /// Shortest normalized input, in characters, that gets typo correction.
    /// Correction is always a single edit, and only towards well-known
    /// people (the index's `fuzzy_min_sitelinks`, fixed when it is built).
    pub fuzzy_min_len: usize,
    /// Count swapping two adjacent letters as one edit instead of two. On by
    /// default: swaps are the most common typing error.
    pub fuzzy_transpositions: bool,
    /// Longest accepted input, in characters.
    pub max_input_chars: usize,
    /// Most candidates listed for an ambiguous name.
    pub max_candidates: usize,
}

impl Default for MatchConfig {
    fn default() -> Self {
        MatchConfig {
            dominance_ratio: 3.0,
            mononym_min_sitelinks: 40,
            fuzzy_min_len: 5,
            fuzzy_transpositions: true,
            max_input_chars: 100,
            max_candidates: 5,
        }
    }
}

impl MatchConfig {
    /// Whether a normalized input of `chars` characters gets typo correction.
    pub fn allows_fuzzy(&self, chars: usize) -> bool {
        chars >= self.fuzzy_min_len
    }
}

/// Time spent in each stage of one lookup. A stage that did not run is `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageTimings {
    /// Input guard and normalization.
    pub guard: Duration,
    pub exact: Option<Duration>,
    pub fuzzy: Option<Duration>,
    /// The resolution rule and reading the chosen records.
    pub resolve: Option<Duration>,
}

impl StageTimings {
    /// Each stage that ran, with its name.
    pub fn stages(&self) -> impl Iterator<Item = (&'static str, Duration)> {
        [
            ("guard", Some(self.guard)),
            ("exact", self.exact),
            ("fuzzy", self.fuzzy),
            ("resolve", self.resolve),
        ]
        .into_iter()
        .filter_map(|(name, time)| Some((name, time?)))
    }
}

/// Most bearers of one name checked against a category.
const MAX_BEARERS: usize = 2000;

/// Most labels listed per attribute in a response.
const MAX_LABELS: usize = 12;

/// How one stage of the pipeline ended.
enum Stage {
    /// The records the matched names map to, most famous first and never
    /// empty, and those names (index keys).
    Hit(Vec<u32>, Vec<String>),
    Miss,
}

pub struct Matcher {
    index: Index,
    config: MatchConfig,
    /// Precomputed automaton tables for one edit. Building these is slow;
    /// building a per-query DFA from them takes microseconds.
    one_edit: LevenshteinAutomatonBuilder,
}

impl Matcher {
    pub fn new(index: Index, config: MatchConfig) -> Matcher {
        let one_edit = LevenshteinAutomatonBuilder::new(1, config.fuzzy_transpositions);
        Matcher { index, config, one_edit }
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn config(&self) -> &MatchConfig {
        &self.config
    }

    /// Decide whether `input` names a real, notable woman.
    ///
    /// Errors only if the index files are corrupt.
    pub fn lookup(&self, input: &str) -> Result<Response, IndexError> {
        self.lookup_timed(input, &[]).map(|(response, _)| response)
    }

    /// Decide whether `input` names a real person in every one of the
    /// `category` terms: `[women, scientist]` is "women scientists".
    ///
    /// Unlike the women game, a category game resolves a shared name among
    /// the people in the category only: in a game of actors, a name borne by
    /// a famous athlete and a lesser-known actor means the actor.
    pub fn lookup_in(&self, input: &str, category: &[Term]) -> Result<Response, IndexError> {
        self.lookup_timed(input, category).map(|(response, _)| response)
    }

    /// A lookup that also reports where the time went. An empty `category`
    /// is the original women game.
    pub fn lookup_timed(
        &self,
        input: &str,
        category: &[Term],
    ) -> Result<(Response, StageTimings), IndexError> {
        let started = Instant::now();
        let mut timings = StageTimings::default();
        let mut response = self.run(input, category, started, &mut timings)?;
        response.latency_us = started.elapsed().as_micros() as u64;
        Ok((response, timings))
    }

    fn run(
        &self,
        input: &str,
        category: &[Term],
        started: Instant,
        timings: &mut StageTimings,
    ) -> Result<Response, IndexError> {
        let mut response = Response {
            input: input.to_owned(),
            normalized: String::new(),
            category: category.iter().map(Term::id).collect(),
            verdict: Verdict::Rejected,
            reason: Some(Reason::NotAName),
            reason_detail: None,
            matched_via: None,
            corrected_from: None,
            matched_name: None,
            matched_as: None,
            matched: None,
            candidates: Vec::new(),
            latency_us: 0,
        };

        // 1. Input guard. Over-long input is not even normalized.
        let is_candidate = input.chars().count() <= self.config.max_input_chars
            && input.chars().any(char::is_alphabetic);
        if is_candidate {
            response.normalized = normalize(input);
        }
        timings.guard = started.elapsed();
        if response.normalized.is_empty() {
            return Ok(response);
        }

        // 2. Exact match, then 3. fuzzy match only if nothing matched exactly.
        let stage = Instant::now();
        let exact = self.exact(&response.normalized);
        timings.exact = Some(stage.elapsed());
        let (records, keys, via) = match exact {
            Stage::Hit(records, keys) => (records, keys, MatchedVia::Exact),
            Stage::Miss => {
                let stage = Instant::now();
                let fuzzy = self.fuzzy(&response.normalized);
                timings.fuzzy = Some(stage.elapsed());
                match fuzzy {
                    Stage::Hit(records, keys) => (records, keys, MatchedVia::Fuzzy),
                    Stage::Miss => {
                        // 4. No match.
                        response.reason = Some(Reason::NotFound);
                        return Ok(response);
                    }
                }
            }
        };
        response.matched_via = Some(via);
        if via == MatchedVia::Fuzzy {
            response.corrected_from = Some(response.normalized.clone());
        }

        let stage = Instant::now();
        let outcome = if category.is_empty() {
            self.decide(&records, &keys, &mut response)
        } else {
            self.decide_in(category, &records, &keys, &mut response)
        };
        timings.resolve = Some(stage.elapsed());
        outcome.map(|()| response)
    }

    /// Apply the resolution rule to `records` and fill in the verdict.
    /// `keys` are the index names that produced them.
    fn decide(
        &self,
        records: &[u32],
        keys: &[String],
        response: &mut Response,
    ) -> Result<(), IndexError> {
        let single_word = !response.normalized.contains(' ');
        let Some(chosen) = self.resolve(records, single_word) else {
            response.reason = Some(Reason::Ambiguous);
            for &record in records.iter().take(self.config.max_candidates) {
                let cold = self.index.cold(record)?;
                response.candidates.push(self.person(record, &cold));
            }
            return Ok(());
        };

        let cold = self.index.cold(chosen)?;
        match self.index.hot(chosen).map(|hot| hot.kind) {
            Some(Kind::Woman) => {
                response.verdict = Verdict::Accepted;
                response.reason = None;
            }
            Some(Kind::Fictional) => response.reason = Some(Reason::Fictional),
            Some(Kind::NotWoman) | None => {
                response.reason = Some(Reason::NotWoman);
                response.reason_detail =
                    Some(describe_genders(cold.attributes.get(Attribute::Gender)));
            }
        }
        let (name, matched_as) = matched_name(&cold, keys);
        response.matched_name = Some(name);
        response.matched_as = Some(matched_as);
        response.matched = Some(self.person(chosen, &cold));
        Ok(())
    }

    /// The verdict for a category game: resolve among the bearers of the name
    /// who are in the category; if none is, say who the name belongs to and
    /// why they do not count.
    fn decide_in(
        &self,
        category: &[Term],
        records: &[u32],
        keys: &[String],
        response: &mut Response,
    ) -> Result<(), IndexError> {
        let catalogue = self.index.catalogue();
        let mut members: Vec<u32> = Vec::new();
        for &record in records.iter().take(MAX_BEARERS) {
            let (Some(hot), cold) = (self.index.hot(record), self.index.cold(record)?) else {
                continue;
            };
            if category.iter().all(|term| catalogue.contains(term, hot, &cold)) {
                members.push(record);
            }
        }

        let single_word = !response.normalized.contains(' ');
        let (chosen, accepted) = match members.first() {
            Some(_) => match self.resolve(&members, single_word) {
                Some(chosen) => (chosen, true),
                None => {
                    response.reason = Some(Reason::Ambiguous);
                    for &record in members.iter().take(self.config.max_candidates) {
                        let cold = self.index.cold(record)?;
                        response.candidates.push(self.person(record, &cold));
                    }
                    return Ok(());
                }
            },
            // Nobody by that name is in the category: report its most famous bearer.
            None => (records[0], false),
        };

        let cold = self.index.cold(chosen)?;
        let hot = self.index.hot(chosen);
        if accepted {
            response.verdict = Verdict::Accepted;
            response.reason = None;
        } else if hot.is_some_and(|hot| hot.kind == Kind::Fictional) {
            response.reason = Some(Reason::Fictional);
        } else {
            response.reason = Some(Reason::NotInCategory);
            let failed = category
                .iter()
                .find(|term| !hot.is_some_and(|hot| catalogue.contains(term, hot, &cold)));
            response.reason_detail = failed.map(|term| match term {
                Term::Women | Term::Men => describe_genders(cold.attributes.get(Attribute::Gender)),
                other => {
                    let label = catalogue.get(other).map_or_else(|| other.id(), |e| e.label().to_owned());
                    format!("not in category: {label}")
                }
            });
        }
        let (name, matched_as) = matched_name(&cold, keys);
        response.matched_name = Some(name);
        response.matched_as = Some(matched_as);
        response.matched = Some(self.person(chosen, &cold));
        Ok(())
    }

    fn exact(&self, normalized: &str) -> Stage {
        match self.index.exact(normalized) {
            Some(hits) => Stage::Hit(hits.as_slice().to_vec(), vec![normalized.to_owned()]),
            None => Stage::Miss,
        }
    }

    /// Every record under the well-known names one edit from `normalized`.
    ///
    /// Only names of well-known people are searched, but each hit brings
    /// everyone who shares that name, so the dominance rule still sees them.
    fn fuzzy(&self, normalized: &str) -> Stage {
        if !self.config.allows_fuzzy(normalized.chars().count()) {
            return Stage::Miss;
        }
        let dfa = self.one_edit.build_dfa(normalized);
        let mut stream = self.index.fuzzy_names().search_with_state(&dfa).into_stream();
        let mut records: Vec<u32> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        while let Some((name, state)) = stream.next() {
            if !matches!(dfa.distance(state), Distance::Exact(_)) {
                continue;
            }
            if let Some(value) = self.index.names().get(name) {
                records.extend_from_slice(self.index.hits(value).as_slice());
                keys.push(String::from_utf8_lossy(name).into_owned());
            }
        }
        if records.is_empty() {
            return Stage::Miss;
        }
        // Several near names can point at the same person, and each brings
        // its own ascending list: merge them back into popularity order.
        records.sort_unstable();
        records.dedup();
        Stage::Hit(records, keys)
    }

    /// The resolution rule: the most famous record, if it clearly dominates.
    /// `records` is in popularity order. `None` means ambiguous.
    fn resolve(&self, records: &[u32], single_word: bool) -> Option<u32> {
        let sitelinks = |record: u32| self.index.hot(record).map_or(0, |hot| hot.sitelinks as u32);
        let top = *records.first()?;
        if let Some(&runner_up) = records.get(1) {
            if (sitelinks(top) as f64) < self.config.dominance_ratio * sitelinks(runner_up) as f64 {
                return None;
            }
        }
        if single_word && sitelinks(top) < self.config.mononym_min_sitelinks {
            return None;
        }
        Some(top)
    }

    /// A record as shown to the player.
    fn person(&self, record: u32, cold: &ColdRecord) -> Match {
        let hot = self.index.hot(record);
        let attributes = &cold.attributes;
        let labels = |attribute: Attribute| -> Vec<String> {
            attributes
                .get(attribute)
                .iter()
                .filter_map(|&qid| self.index.label(qid))
                .take(MAX_LABELS)
                .map(str::to_owned)
                .collect()
        };

        let occupations: Vec<(&str, Option<u8>)> = attributes
            .get(Attribute::Occupation)
            .iter()
            .map(|&qid| {
                (self.index.label(qid).unwrap_or_default(), self.index.occupation_category(qid))
            })
            .collect();
        let held_office = !attributes.get(Attribute::Position).is_empty();
        let mut categories = rank_categories(&cold.description, &occupations, held_office);
        if categories.is_empty() && !attributes.get(Attribute::NobleTitle).is_empty() {
            categories.push(ROYALTY);
        }

        Match {
            qid: hot.map(|h| h.qid_string()).unwrap_or_default(),
            name: cold.label.clone(),
            description: cold.description.clone(),
            birth_year: cold.birth_year,
            death_year: cold.death_year,
            sitelinks: hot.map_or(0, |h| h.sitelinks as u32),
            wikipedia_url: wikipedia_url(&cold.enwiki_title),
            categories: categories
                .into_iter()
                .filter_map(category_label)
                .map(str::to_owned)
                .collect(),
            occupations: labels(Attribute::Occupation),
            citizenships: labels(Attribute::Citizenship),
            genders: attributes
                .get(Attribute::Gender)
                .iter()
                .filter_map(|&qid| gender_label(qid).or_else(|| self.index.label(qid)))
                .map(str::to_owned)
                .collect(),
        }
    }
}

/// Which of the person's names, as written in Wikidata, is one of the matched
/// index `keys`. The main name wins over an alias that normalizes the same.
fn matched_name(person: &ColdRecord, keys: &[String]) -> (String, MatchedAs) {
    let is_match = |name: &str| keys.contains(&normalize(name));
    if is_match(&person.label) {
        return (person.label.clone(), MatchedAs::Name);
    }
    match person.aliases.iter().find(|alias| is_match(alias)) {
        Some(alias) => (alias.clone(), MatchedAs::Alias),
        // Unreachable for an index built by this code: every key comes from
        // the label or an alias. Fall back to the main name.
        None => (person.label.clone(), MatchedAs::Name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_input_gets_no_typo_correction() {
        let config = MatchConfig::default();
        assert_eq!([0, 4, 5, 40].map(|n| config.allows_fuzzy(n)), [false, false, true, true]);
    }

    #[test]
    fn config_reads_partial_input_and_rejects_unknown_keys() {
        let config: MatchConfig = serde_json::from_str(r#"{"dominance_ratio": 5}"#).unwrap();
        assert_eq!(config.dominance_ratio, 5.0);
        assert_eq!(config.mononym_min_sitelinks, 40);
        assert!(serde_json::from_str::<MatchConfig>(r#"{"dominence_ratio": 5}"#).is_err());
    }
}
