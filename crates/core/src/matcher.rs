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
    /// Shortest normalized input, in characters, that may be off by one edit.
    pub fuzzy_one_edit_min_len: usize,
    /// Shortest normalized input, in characters, that may be off by two edits.
    pub fuzzy_two_edits_min_len: usize,
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
            fuzzy_one_edit_min_len: 5,
            fuzzy_two_edits_min_len: 10,
            fuzzy_transpositions: true,
            max_input_chars: 100,
            max_candidates: 5,
        }
    }
}

impl MatchConfig {
    /// Edits allowed for a normalized input of `chars` characters.
    pub fn fuzzy_distance(&self, chars: usize) -> u8 {
        if chars >= self.fuzzy_two_edits_min_len {
            2
        } else if chars >= self.fuzzy_one_edit_min_len {
            1
        } else {
            0
        }
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
    /// Precomputed automaton tables for one and two edits. Building these is
    /// slow; building a per-query DFA from them takes microseconds.
    one_edit: LevenshteinAutomatonBuilder,
    two_edits: LevenshteinAutomatonBuilder,
}

impl Matcher {
    pub fn new(index: Index, config: MatchConfig) -> Matcher {
        let one_edit = LevenshteinAutomatonBuilder::new(1, config.fuzzy_transpositions);
        let two_edits = LevenshteinAutomatonBuilder::new(2, config.fuzzy_transpositions);
        Matcher { index, config, one_edit, two_edits }
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

    /// Every record under the indexed names closest to `normalized`, within
    /// the edit distance its length allows.
    fn fuzzy(&self, normalized: &str) -> Stage {
        let builder = match self.config.fuzzy_distance(normalized.chars().count()) {
            0 => return Stage::Miss,
            1 => &self.one_edit,
            _ => &self.two_edits,
        };
        let dfa = builder.build_dfa(normalized);
        let mut stream = self.index.names().search_with_state(&dfa).into_stream();
        let mut best = u8::MAX;
        let mut records: Vec<u32> = Vec::new();
        while let Some((_, value, state)) = stream.next() {
            let Distance::Exact(distance) = dfa.distance(state) else {
                continue;
            };
            if distance < best {
                best = distance;
                records.clear();
            }
            if distance == best {
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
    fn fuzzy_distance_follows_input_length() {
        let config = MatchConfig::default();
        let distances: Vec<u8> = [0, 4, 5, 9, 10, 40].map(|n| config.fuzzy_distance(n)).to_vec();
        assert_eq!(distances, [0, 0, 1, 1, 2, 2]);
    }

    #[test]
    fn config_reads_partial_input_and_rejects_unknown_keys() {
        let config: MatchConfig = serde_json::from_str(r#"{"dominance_ratio": 5}"#).unwrap();
        assert_eq!(config.dominance_ratio, 5.0);
        assert_eq!(config.mononym_min_sitelinks, 40);
        assert!(serde_json::from_str::<MatchConfig>(r#"{"dominence_ratio": 5}"#).is_err());
    }
}
