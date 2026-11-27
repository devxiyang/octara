//! Optional request parameters. Defaults omit overrides rather than guessing model support.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Generation {
    pub temperature: Option<f64>,
    pub max_completion_tokens: Option<u32>,
}

impl Generation {
    pub fn validate(self) -> Result<Self, String> {
        if self.temperature.is_some_and(|n| !n.is_finite() || !(0.0..=2.0).contains(&n)) {
            return Err("Temperature must be between 0 and 2, or blank for the server default.".into());
        }
        if self.max_completion_tokens.is_some_and(|n| !(1..=131_072).contains(&n)) {
            return Err("Maximum output tokens must be a whole number from 1 to 131072, or blank.".into());
        }
        Ok(self)
    }

    pub fn parse(temperature: &str, tokens: &str) -> Result<Self, String> {
        let temperature = if temperature.trim().is_empty() {
            None
        } else {
            Some(temperature.trim().parse().map_err(|_| "Temperature must be a number.")?)
        };
        let max_completion_tokens = if tokens.trim().is_empty() {
            None
        } else {
            Some(tokens.trim().parse().map_err(|_| "Maximum output tokens must be a whole number.")?)
        };
        Self { temperature, max_completion_tokens }.validate()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub model_id: Option<String>,
    pub generation: Generation,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_settings_validate_without_silently_fixing_invalid_values() {
        assert_eq!(Generation::parse(" ", ""), Ok(Generation::default()));
        for (temperature, tokens) in [("NaN", ""), ("inf", ""), ("2.1", ""), ("-0.1", ""), ("", "0"), ("", "1.5"), ("", "131073")] {
            assert!(Generation::parse(temperature, tokens).is_err());
        }
    }
}
