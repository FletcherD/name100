//! The lookup pipeline: input guard, exact match, fuzzy match, resolution.

use std::time::{Duration, Instant};

use fst::{IntoStreamer, Streamer};
use levenshtein_automata::{Distance, LevenshteinAutomatonBuilder};
use serde::{Deserialize, Serialize};

use crate::artifact::Kind;
use crate::attribute::Attribute;
use crate::gender::describe_genders;
use crate::index::{Index, IndexError};
use crate::normalize::normalize;
use crate::verdict::{wikipedia_url, Match, MatchedVia, Reason, Response, Verdict};

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

/// How one stage of the pipeline ended.
enum Stage {
    /// Records the name maps to, most famous first, never empty.
    Hit(Vec<u32>),
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
        self.lookup_timed(input).map(|(response, _)| response)
    }

    /// [`Matcher::lookup`], also reporting where the time went.
    pub fn lookup_timed(&self, input: &str) -> Result<(Response, StageTimings), IndexError> {
        let started = Instant::now();
        let mut timings = StageTimings::default();
        let mut response = self.run(input, started, &mut timings)?;
        response.latency_us = started.elapsed().as_micros() as u64;
        Ok((response, timings))
    }

    fn run(
        &self,
        input: &str,
        started: Instant,
        timings: &mut StageTimings,
    ) -> Result<Response, IndexError> {
        let mut response = Response {
            input: input.to_owned(),
            normalized: String::new(),
            verdict: Verdict::Rejected,
            reason: Some(Reason::NotAName),
            reason_detail: None,
            matched_via: None,
            corrected_from: None,
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
        let (records, via) = match exact {
            Stage::Hit(records) => (records, MatchedVia::Exact),
            Stage::Miss => {
                let stage = Instant::now();
                let fuzzy = self.fuzzy(&response.normalized);
                timings.fuzzy = Some(stage.elapsed());
                match fuzzy {
                    Stage::Hit(records) => (records, MatchedVia::Fuzzy),
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
        let outcome = self.decide(&records, &mut response);
        timings.resolve = Some(stage.elapsed());
        outcome.map(|()| response)
    }

    /// Apply the resolution rule to `records` and fill in the verdict.
    fn decide(&self, records: &[u32], response: &mut Response) -> Result<(), IndexError> {
        let single_word = !response.normalized.contains(' ');
        let Some(chosen) = self.resolve(records, single_word) else {
            response.reason = Some(Reason::Ambiguous);
            for &record in records.iter().take(self.config.max_candidates) {
                response.candidates.push(self.person(record)?.0);
            }
            return Ok(());
        };

        let (person, genders) = self.person(chosen)?;
        match self.index.hot(chosen).map(|hot| hot.kind) {
            Some(Kind::Woman) => {
                response.verdict = Verdict::Accepted;
                response.reason = None;
            }
            Some(Kind::Fictional) => response.reason = Some(Reason::Fictional),
            Some(Kind::NotWoman) | None => {
                response.reason = Some(Reason::NotWoman);
                response.reason_detail = Some(describe_genders(&genders));
            }
        }
        response.matched = Some(person);
        Ok(())
    }

    fn exact(&self, normalized: &str) -> Stage {
        match self.index.exact(normalized) {
            Some(hits) => Stage::Hit(hits.as_slice().to_vec()),
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
        while let Some((name, state)) = stream.next() {
            if !matches!(dfa.distance(state), Distance::Exact(_)) {
                continue;
            }
            if let Some(value) = self.index.names().get(name) {
                records.extend_from_slice(self.index.hits(value).as_slice());
            }
        }
        if records.is_empty() {
            return Stage::Miss;
        }
        // Several near names can point at the same person, and each brings
        // its own ascending list: merge them back into popularity order.
        records.sort_unstable();
        records.dedup();
        Stage::Hit(records)
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

    /// A record as shown to the player, with its current gender values.
    fn person(&self, record: u32) -> Result<(Match, Vec<u32>), IndexError> {
        let cold = self.index.cold(record)?;
        let hot = self.index.hot(record);
        let person = Match {
            qid: hot.map(|h| h.qid_string()).unwrap_or_default(),
            name: cold.label,
            description: cold.description,
            birth_year: cold.birth_year,
            death_year: cold.death_year,
            sitelinks: hot.map_or(0, |h| h.sitelinks as u32),
            wikipedia_url: wikipedia_url(&cold.enwiki_title),
        };
        Ok((person, cold.attributes.get(Attribute::Gender).to_vec()))
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
