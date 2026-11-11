//! `hundred.toml`, with every key overridable by a `HUNDRED_*` environment
//! variable (`HUNDRED_BIND`, `HUNDRED_DOMINANCE_RATIO`, ...).
//!
//! Precedence, highest first: command-line flag, environment, file, default.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use hundred_core::MatchConfig;
use serde::{Deserialize, Deserializer, Serialize};

/// Read from the working directory when `--config` is not given.
pub const DEFAULT_FILE: &str = "hundred.toml";

/// Every key, for mapping `HUNDRED_<KEY>` variables. Other `HUNDRED_*`
/// variables (such as `HUNDRED_BENCH_INDEX`) are ignored.
const KEYS: &[&str] = &[
    "index",
    "bind",
    "langs",
    "min_sitelinks",
    "dominance_ratio",
    "mononym_min_sitelinks",
    "fuzzy_min_sitelinks",
    "fuzzy_min_len",
    "fuzzy_transpositions",
    "max_input_chars",
    "max_candidates",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Index directory served by `serve` and read by `lookup --local`.
    pub index: Option<PathBuf>,
    /// Address `serve` listens on.
    pub bind: String,
    /// Languages `build-index` takes labels and aliases from.
    #[serde(deserialize_with = "list_or_commas")]
    pub langs: Vec<String>,
    /// Fewest Wikipedia articles a person needs for `build-index` to keep them.
    pub min_sitelinks: u32,
    /// Sitelinks a person needs before `build-index` lets a misspelling be
    /// corrected to them. Changing it means rebuilding the index.
    pub fuzzy_min_sitelinks: u32,
    pub dominance_ratio: f64,
    pub mononym_min_sitelinks: u32,
    pub fuzzy_min_len: usize,
    pub fuzzy_transpositions: bool,
    pub max_input_chars: usize,
    pub max_candidates: usize,
}

impl Default for Config {
    fn default() -> Self {
        let matching = MatchConfig::default();
        Config {
            index: None,
            bind: "127.0.0.1:8080".to_owned(),
            langs: vec!["en".to_owned(), "mul".to_owned()],
            min_sitelinks: 1,
            fuzzy_min_sitelinks: 10,
            dominance_ratio: matching.dominance_ratio,
            mononym_min_sitelinks: matching.mononym_min_sitelinks,
            fuzzy_min_len: matching.fuzzy_min_len,
            fuzzy_transpositions: matching.fuzzy_transpositions,
            max_input_chars: matching.max_input_chars,
            max_candidates: matching.max_candidates,
        }
    }
}

impl Config {
    /// Load `file` if given (it must exist), else `hundred.toml` if present,
    /// then apply `HUNDRED_*` variables.
    pub fn load(file: Option<&Path>) -> Result<Config> {
        let mut figment = Figment::from(Serialized::defaults(Config::default()));
        match file {
            Some(path) => {
                if !path.is_file() {
                    bail!("config file {} not found", path.display());
                }
                figment = figment.merge(Toml::file_exact(path));
            }
            None if Path::new(DEFAULT_FILE).is_file() => {
                figment = figment.merge(Toml::file_exact(DEFAULT_FILE));
            }
            None => {}
        }
        let config: Config = figment
            .merge(Env::prefixed("HUNDRED_").only(KEYS))
            .extract()
            .context("invalid configuration")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.langs.is_empty() {
            bail!("invalid configuration: langs must list at least one language");
        }
        if self.dominance_ratio.is_nan() || self.dominance_ratio < 1.0 {
            bail!("invalid configuration: dominance_ratio must be at least 1");
        }
        if self.fuzzy_min_len < 2 {
            bail!("invalid configuration: fuzzy_min_len must be at least 2");
        }
        Ok(())
    }

    pub fn matching(&self) -> MatchConfig {
        MatchConfig {
            dominance_ratio: self.dominance_ratio,
            mononym_min_sitelinks: self.mononym_min_sitelinks,
            fuzzy_min_len: self.fuzzy_min_len,
            fuzzy_transpositions: self.fuzzy_transpositions,
            max_input_chars: self.max_input_chars,
            max_candidates: self.max_candidates,
        }
    }
}

/// Accept `["en", "mul"]` (TOML) or `en,mul` (convenient in an environment variable).
fn list_or_commas<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ListOrString {
        List(Vec<String>),
        String(String),
    }
    Ok(match ListOrString::deserialize(deserializer)? {
        ListOrString::List(list) => list,
        ListOrString::String(text) => text
            .split(',')
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::Jail;

    #[test]
    fn defaults_match_the_spec() {
        Jail::expect_with(|_| {
            let config = Config::load(None).unwrap();
            assert_eq!(config, Config::default());
            assert_eq!(config.bind, "127.0.0.1:8080");
            assert_eq!(config.langs, ["en", "mul"]);
            assert_eq!(config.matching(), MatchConfig::default());
            assert_eq!(config.matching().dominance_ratio, 3.0);
            assert_eq!(config.matching().mononym_min_sitelinks, 40);
            Ok(())
        });
    }

    #[test]
    fn file_then_environment_override_defaults() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "hundred.toml",
                r#"
                    index = "index/full"
                    bind = "0.0.0.0:9000"
                    dominance_ratio = 4.5
                    langs = ["en", "mul", "fr"]
                "#,
            )?;
            let config = Config::load(None).unwrap();
            assert_eq!(config.index.as_deref(), Some(Path::new("index/full")));
            assert_eq!(config.bind, "0.0.0.0:9000");
            assert_eq!(config.dominance_ratio, 4.5);
            assert_eq!(config.langs, ["en", "mul", "fr"]);
            assert_eq!(config.mononym_min_sitelinks, 40, "untouched keys keep their default");

            jail.set_env("HUNDRED_BIND", "127.0.0.1:7000");
            jail.set_env("HUNDRED_MONONYM_MIN_SITELINKS", "25");
            jail.set_env("HUNDRED_FUZZY_TRANSPOSITIONS", "false");
            jail.set_env("HUNDRED_LANGS", "en,de");
            jail.set_env("HUNDRED_BENCH_INDEX", "ignored: not a config key");
            let config = Config::load(None).unwrap();
            assert_eq!(config.bind, "127.0.0.1:7000");
            assert_eq!(config.mononym_min_sitelinks, 25);
            assert!(!config.fuzzy_transpositions);
            assert_eq!(config.langs, ["en", "de"]);
            assert_eq!(config.dominance_ratio, 4.5, "the file still applies");
            Ok(())
        });
    }

    #[test]
    fn an_explicit_file_must_exist_and_replaces_the_default_one() {
        Jail::expect_with(|jail| {
            jail.create_file("hundred.toml", "bind = \"0.0.0.0:1\"")?;
            jail.create_file("other.toml", "min_sitelinks = 5")?;
            let config = Config::load(Some(Path::new("other.toml"))).unwrap();
            assert_eq!(config.min_sitelinks, 5);
            assert_eq!(config.bind, "127.0.0.1:8080");
            let error = Config::load(Some(Path::new("missing.toml"))).unwrap_err();
            assert!(error.to_string().contains("missing.toml not found"));
            Ok(())
        });
    }

    #[test]
    fn typos_and_nonsense_are_rejected() {
        Jail::expect_with(|jail| {
            jail.create_file("hundred.toml", "dominence_ratio = 4")?;
            assert!(format!("{:#}", Config::load(None).unwrap_err()).contains("dominence_ratio"));

            for bad in [
                "dominance_ratio = 0.5",
                "langs = []",
                "fuzzy_min_len = 0",
                "fuzzy_two_edits_min_len = 10",
                "mononym_min_sitelinks = \"forty\"",
            ] {
                jail.create_file("hundred.toml", bad)?;
                assert!(Config::load(None).is_err(), "{bad} should be rejected");
            }
            Ok(())
        });
    }
}
