//! API list prices, for the Cost screen's "what would these tokens cost on the API".
//! The table comes from Anthropic's published pricing page (contracts/pricing.json, with
//! its source and date); nothing here is a remembered price. The admin can keep their own
//! copy beside comptroller.json (Cost.PricingFile).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The table built into this terminal.
pub const BUILT_IN: &str = include_str!("../../contracts/pricing.json");
/// After this many days, the Cost screen suggests checking the prices.
pub const STALE_DAYS: i64 = 90;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pricing {
    pub source: String,
    /// `yyyy-mm-dd`.
    pub last_updated: String,
    pub currency: String,
    pub models: Vec<Model>,
}

/// Prices per million tokens.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub input: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
    pub output: f64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub name: String,
    pub family: String,
    pub version: String,
    #[serde(flatten)]
    pub rates: Rates,
    pub long_context: Option<LongContext>,
}

/// Higher rates for every token of a request whose prompt is over a size.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LongContext {
    /// Prompt size: input, cache writes and cache reads together.
    pub over_input_tokens: i64,
    #[serde(flatten)]
    pub rates: Rates,
}

/// One request's tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct Tokens {
    pub input: i64,
    pub output: i64,
    /// Cache writes not split by lifetime are priced at the 5-minute rate.
    pub cache_write_5m: i64,
    pub cache_write_1h: i64,
    pub cache_read: i64,
}

impl Tokens {
    pub fn cache_write(&self) -> i64 {
        self.cache_write_5m + self.cache_write_1h
    }

    pub fn prompt(&self) -> i64 {
        self.input + self.cache_write() + self.cache_read
    }

    pub fn total(&self) -> i64 {
        self.prompt() + self.output
    }

    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write_5m += other.cache_write_5m;
        self.cache_write_1h += other.cache_write_1h;
        self.cache_read += other.cache_read;
    }
}

/// The family and version in a model id: `claude-opus-5-5` and `claude-opus-5-5-20261001`
/// are ("opus", "5.5"); the older `claude-3-5-sonnet-20241022` is ("sonnet", "3.5").
pub fn family_version(model: &str) -> Option<(String, String)> {
    let lower = model.trim().to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('-').collect();
    if parts.first() != Some(&"claude") {
        return None;
    }
    let numeric = |s: &&str| !s.is_empty() && s.len() <= 2 && s.chars().all(|c| c.is_ascii_digit());
    let rest = &parts[1..];
    let family_at = rest.iter().position(|p| p.chars().all(|c| c.is_ascii_alphabetic()) && !p.is_empty())?;
    let family = rest[family_at].to_string();
    // Version after the family (new ids), else before it (old ids). A date (8 digits) isn't part of it.
    let after: Vec<&str> = rest[family_at + 1..].iter().copied().take_while(|s| numeric(&s)).collect();
    let before: Vec<&str> = rest[..family_at].iter().copied().filter(|s| numeric(&s)).collect();
    let version = if !after.is_empty() { after } else { before };
    if version.is_empty() {
        return None;
    }
    Some((family, version.join(".")))
}

impl Pricing {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("not a valid pricing table: {e}"))
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        let (family, version) = family_version(id)?;
        self.models.iter().find(|m| m.family.eq_ignore_ascii_case(&family) && m.version == version)
    }

    /// The list price of one request, in the table's currency. None: the model isn't in the table.
    pub fn cost(&self, model: &str, tokens: &Tokens) -> Option<f64> {
        let entry = self.model(model)?;
        let rates = match &entry.long_context {
            Some(long) if tokens.prompt() > long.over_input_tokens => &long.rates,
            _ => &entry.rates,
        };
        Some(
            (tokens.input as f64 * rates.input
                + tokens.output as f64 * rates.output
                + tokens.cache_write_5m as f64 * rates.cache_write_5m
                + tokens.cache_write_1h as f64 * rates.cache_write_1h
                + tokens.cache_read as f64 * rates.cache_read)
                / 1_000_000.0,
        )
    }

    /// Days since the prices were copied, given today's date as seconds since 1970.
    pub fn age_days(&self, now: i64) -> Option<i64> {
        let updated = crate::health::parse_utc(&format!("{}T00:00:00", self.last_updated))?;
        Some((now - updated) / 86_400)
    }
}

/// Where the table comes from: the admin's file, or the built-in one when there's none.
#[derive(Clone, Debug)]
pub enum Source {
    File(PathBuf),
    BuiltIn,
}

/// The admin's table if the file exists, else the built-in one. An unreadable file is an error,
/// so a typo never silently falls back to other prices.
pub fn load(path: &Path) -> Result<(Pricing, Source), String> {
    if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Pricing::parse(&text).map(|p| (p, Source::File(path.to_path_buf()))).map_err(|e| format!("{}: {e}", path.display()));
    }
    Ok((Pricing::parse(BUILT_IN).expect("the built-in table is valid"), Source::BuiltIn))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Pricing {
        Pricing::parse(BUILT_IN).unwrap()
    }

    #[test]
    fn reads_family_and_version_from_ids() {
        assert_eq!(family_version("claude-opus-5-5"), Some(("opus".into(), "5.5".into())));
        assert_eq!(family_version("claude-opus-5-5-20261001"), Some(("opus".into(), "5.5".into())));
        assert_eq!(family_version("claude-sonnet-5"), Some(("sonnet".into(), "5".into())));
        assert_eq!(family_version("claude-opus-4-20250514"), Some(("opus".into(), "4".into())));
        assert_eq!(family_version("claude-3-5-sonnet-20241022"), Some(("sonnet".into(), "3.5".into())));
        assert_eq!(family_version("claude-mock-1"), Some(("mock".into(), "1".into())));
        assert_eq!(family_version("gpt-4"), None);
    }

    #[test]
    fn the_built_in_table_is_complete_and_dated() {
        let t = table();
        assert!(t.source.starts_with("https://"));
        assert_eq!(t.currency, "USD");
        assert!(t.age_days(crate::health::parse_utc("2026-11-27T00:00:00").unwrap()).is_some());
        for m in &t.models {
            assert!(m.rates.input > 0.0 && m.rates.output > m.rates.input, "{}", m.name);
            assert!(t.model(&format!("claude-{}-{}", m.family, m.version.replace('.', "-"))).is_some(), "{} matches its own id", m.name);
        }
    }

    #[test]
    fn prices_a_request_from_its_tokens() {
        let t = table();
        let rates = &t.model("claude-opus-5-5").unwrap().rates;
        let tokens = Tokens { input: 1_000_000, output: 1_000_000, cache_write_5m: 1_000_000, cache_write_1h: 1_000_000, cache_read: 1_000_000 };
        let expected = rates.input + rates.output + rates.cache_write_5m + rates.cache_write_1h + rates.cache_read;
        assert!((t.cost("claude-opus-5-5-20261001", &tokens).unwrap() - expected).abs() < 1e-9);
        assert_eq!(t.cost("claude-mock-1", &tokens), None, "not in the table");
    }

    #[test]
    fn long_prompts_use_the_long_context_rates_for_every_token() {
        let t = table();
        let haiku = t.model("claude-haiku-5-5").unwrap();
        let long = haiku.long_context.as_ref().unwrap();
        let short = Tokens { input: long.over_input_tokens, output: 1000, ..Default::default() };
        let over = Tokens { input: 1000, cache_read: long.over_input_tokens, output: 1000, ..Default::default() };
        let short_cost = t.cost("claude-haiku-5-5", &short).unwrap();
        assert!((short_cost - (short.input as f64 * haiku.rates.input + 1000.0 * haiku.rates.output) / 1e6).abs() < 1e-12);
        let over_cost = t.cost("claude-haiku-5-5", &over).unwrap();
        let expected = (1000.0 * long.rates.input + over.cache_read as f64 * long.rates.cache_read + 1000.0 * long.rates.output) / 1e6;
        assert!((over_cost - expected).abs() < 1e-12);
    }
}
