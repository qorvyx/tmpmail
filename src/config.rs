//! Optional config file plus environment overrides, for interactive use and CI.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::error::{Error, Result};
use crate::provider;

/// Seconds between inbox checks when nothing sets them.
pub const DEFAULT_POLL_INTERVAL: u64 = 5;
/// Seconds before a request gives up when nothing sets it.
pub const DEFAULT_HTTP_TIMEOUT: u64 = 20;
/// The longest request timeout accepted (a day): far more would overflow
/// the clock arithmetic of a request's deadline.
const MAX_HTTP_TIMEOUT: u64 = 86_400;

/// tmpmail's settings: the config file plus environment overrides.
///
/// Known keys are checked strictly (type and value); unknown ones are kept
/// aside and reported as warnings, so a newer key (or a typo) never stops an
/// older tmpmail from starting.
///
/// Fields can be added in any release, so the struct is `#[non_exhaustive]`:
/// read one with [`Config::load`] (or [`Config::parse`]), or build one with
/// `Config::default()` and then set fields. Fields set by hand skip the
/// checks `load` makes (provider ids, `http_timeout` from 1 to 86400).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Config {
    /// Provider ids tried in order by `new`.
    pub providers: Option<Vec<String>>,
    pub browser: Option<String>,
    pub clipboard_cmd: Option<String>,
    /// Pager for long messages (default: $PAGER, else less).
    pub pager: Option<String>,
    /// Seconds between inbox checks in `wait` and `watch`.
    pub poll_interval: Option<u64>,
    /// Default `wait` timeout in seconds.
    pub timeout: Option<u64>,
    /// Use providers' push streams in `wait` and `watch` (default true).
    pub push: Option<bool>,
    /// Seconds before a single request to a provider gives up (default 20;
    /// `TMPMAIL_HTTP_TIMEOUT` wins over it).
    pub http_timeout: Option<u64>,
    /// Top-level keys this version doesn't know (names only), ignored.
    #[serde(flatten)]
    unknown: BTreeMap<String, IgnoredAny>,
    /// The file this was read from, for the warnings.
    #[serde(skip)]
    source: Option<PathBuf>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("TMPMAIL_CONFIG") {
            return Some(PathBuf::from(p));
        }
        dirs::config_dir().map(|d| d.join("tmpmail").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let mut cfg = match Self::path() {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(s) => {
                    let mut cfg = Self::parse(&s)
                        .with_context(|| format!("invalid config {}", path.display()))?;
                    cfg.source = Some(path);
                    cfg
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
                Err(e) => {
                    return Err(anyhow::Error::from(e)
                        .context(format!("cannot read {}", path.display()))
                        .into());
                }
            },
            None => Config::default(),
        };
        if let Ok(p) = std::env::var("TMPMAIL_PROVIDER") {
            cfg.providers = Some(p.split(',').map(|s| s.trim().to_string()).collect());
        }
        if let Some(t) = env_u64("TMPMAIL_TIMEOUT")? {
            cfg.timeout = Some(t);
        }
        if let Some(t) = env_u64("TMPMAIL_POLL_INTERVAL")? {
            cfg.poll_interval = Some(t);
        }
        if let Ok(v) = std::env::var("TMPMAIL_PUSH") {
            cfg.push = Some(match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    return Err(Error::Usage(format!(
                        "TMPMAIL_PUSH must be 1 or 0, got {v:?}"
                    )));
                }
            });
        }
        if let Some(ids) = &cfg.providers {
            check_providers(ids)?;
        }
        if let Some(t) = cfg.http_timeout {
            check_http_timeout(t, "http_timeout in the config")?;
        }
        // Checked like the config's key, and wins over it.
        if let Some(t) = env_u64("TMPMAIL_HTTP_TIMEOUT")? {
            check_http_timeout(t, "TMPMAIL_HTTP_TIMEOUT")?;
            cfg.http_timeout = Some(t);
        }

        Ok(cfg)
    }

    /// One warning per top-level key this version doesn't know (and
    /// ignores), in order, for the front ends to show.
    pub fn warnings(&self) -> Vec<String> {
        self.warnings_for(self.unknown.keys())
    }

    /// The warnings for unknown keys that `old` didn't have, so a reload
    /// (the desktop app's settings) doesn't repeat what was already shown.
    pub(crate) fn new_warnings(&self, old: &Config) -> Vec<String> {
        self.warnings_for(
            self.unknown
                .keys()
                .filter(|k| !old.unknown.contains_key(*k)),
        )
    }

    fn warnings_for<'a>(&self, keys: impl Iterator<Item = &'a String>) -> Vec<String> {
        let file = self
            .source
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "the config".into());
        keys.map(|k| format!("tmpmail: unknown key {k:?} in {file}, ignored"))
            .collect()
    }

    /// Reads a config file's text, without the environment overrides.
    pub fn parse(s: &str) -> std::result::Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn provider_order(&self) -> Vec<String> {
        match &self.providers {
            Some(p) if !p.is_empty() => p.clone(),
            _ => provider::DEFAULT_ORDER
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Seconds between checks: the command-line value, else the config, at least 2.
    pub fn poll_interval(&self, cli: Option<u64>) -> u64 {
        cli.or(self.poll_interval)
            .unwrap_or(DEFAULT_POLL_INTERVAL)
            .max(2)
    }

    pub fn push(&self) -> bool {
        self.push.unwrap_or(true)
    }

    pub fn timeout(&self) -> u64 {
        self.timeout.unwrap_or(180)
    }
}

/// The provider order: known ids, and at least one (`providers = []`
/// would quietly mean the default order).
fn check_providers(ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Err(Error::Usage(
            "providers in the config is empty; list at least one provider, or remove the key for the default order".into(),
        ));
    }
    for id in ids {
        provider::get(id)?;
    }
    Ok(())
}

fn check_http_timeout(secs: u64, what: &str) -> Result<()> {
    if (1..=MAX_HTTP_TIMEOUT).contains(&secs) {
        Ok(())
    } else {
        Err(Error::Usage(format!(
            "{what} must be from 1 to {MAX_HTTP_TIMEOUT} seconds, got {secs}"
        )))
    }
}

fn env_u64(key: &str) -> Result<Option<u64>> {
    match std::env::var(key) {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|_| Error::Usage(format!("{key} must be a number of seconds, got {v:?}"))),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_load_with_a_warning() {
        let cfg =
            Config::parse("poll_interval = 7\nhttp_timeuot = 3\n[gui]\ntray = true\n").unwrap();
        assert_eq!(cfg.poll_interval(None), 7);
        assert_eq!(
            cfg.warnings(),
            [
                "tmpmail: unknown key \"gui\" in the config, ignored",
                "tmpmail: unknown key \"http_timeuot\" in the config, ignored"
            ]
        );
        assert!(
            Config::parse("push = true\n")
                .unwrap()
                .warnings()
                .is_empty()
        );
    }

    #[test]
    fn known_keys_stay_strict() {
        for bad in [
            "poll_interval = \"5\"",
            "poll_interval = -1",
            "push = 1",
            "providers = \"mailtm\"",
            "providers = [1]",
            "http_timeout = 2.5",
            "browser = false",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad}");
        }
        let err = Config::parse("http_timeout = \"x\"\nfoo = 1\n").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
    }

    #[test]
    fn an_empty_provider_list_is_refused() {
        assert!(check_providers(&[]).is_err());
        assert!(check_providers(&["mailtm".into()]).is_ok());
        assert!(check_providers(&["nope".into()]).is_err());
    }

    #[test]
    fn http_timeouts_are_bounded() {
        assert!(check_http_timeout(20, "x").is_ok());
        assert!(check_http_timeout(0, "x").is_err());
        assert!(check_http_timeout(u64::MAX, "x").is_err());
    }

    #[test]
    fn http_timeout_round_trips() {
        let cfg = Config::parse("http_timeout = 45\n").unwrap();
        assert_eq!(cfg.http_timeout, Some(45));
        assert!(cfg.warnings().is_empty());
        let text = format!("http_timeout = {}\n", cfg.http_timeout.unwrap());
        assert_eq!(Config::parse(&text).unwrap().http_timeout, Some(45));
    }

    #[test]
    fn parses_every_key() {
        let cfg = Config::parse(
            "providers = [\"mailtm\"]\npoll_interval = 7\npush = false\nhttp_timeout = 30\n",
        )
        .unwrap();
        assert_eq!(cfg.provider_order(), ["mailtm"]);
        assert_eq!(cfg.poll_interval(None), 7);
        assert!(!cfg.push());
        assert_eq!(cfg.http_timeout, Some(30));
        let empty = Config::parse("").unwrap();
        assert_eq!(empty.http_timeout, None);
        assert_eq!(empty.poll_interval(None), 5);
    }
}
