//! Validated execution placement, excluded from semantic identity (DET-3).
use crate::{RunRefusal, invalid};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Cores {
    Fixed(u64),
    Range(CoreRange),
    Auto(AutoCores),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreRange {
    pub min: u64,
    pub max: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub enum AutoCores {
    #[serde(rename = "auto")]
    Auto,
}
impl<'de> Deserialize<'de> for AutoCores {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = String::deserialize(de)?;
        match value.as_str() {
            "auto" => Ok(Self::Auto),
            _ => Err(serde::de::Error::unknown_variant(&value, &["auto"])),
        }
    }
}
impl Default for Cores {
    fn default() -> Self {
        Self::Fixed(1)
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pin {
    #[default]
    None,
    Compact,
    Spread,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Commit {
    #[default]
    Shared,
    Dedicated,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reorder {
    #[default]
    Auto,
    Off,
}

/// Requests only: host grants and resolved widths are launcher-owned, never record inputs.
/// Ordered admission knobs stay optional so the executor can choose its policy defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    #[serde(default)]
    pub cores: Cores,
    pub cpus: Option<String>,
    #[serde(default)]
    pub pin: Pin,
    pub commit: Option<Commit>,
    pub run_ahead: Option<u8>,
    pub reorder: Option<Reorder>,
    pub speculation_bytes: Option<u64>,
    pub metrics: Option<String>,
    pub control: Option<String>,
}
impl Placement {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("placement serialization")
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, RunRefusal> {
        crate::integer_tokens(bytes)?;
        let placement: Self =
            serde_json::from_slice(bytes).map_err(|e| invalid("placement", &e.to_string()))?;
        placement.validate()?;
        Ok(placement)
    }

    /// Validate syntax without consulting topology or opening any named path. Host
    /// resolution and sidecar admission are separate, and replay never performs them.
    pub fn validate(&self) -> Result<(), RunRefusal> {
        match &self.cores {
            Cores::Fixed(0) => return Err(invalid("placement.cores", "expected at least 1")),
            Cores::Range(range) if range.min == 0 || range.min > range.max => {
                return Err(invalid("placement.cores", "expected 1 <= min <= max"));
            }
            _ => {}
        }
        if let Some(cpus) = &self.cpus {
            let number = |text: &str| text.parse::<u32>().ok().filter(|n| n.to_string() == text);
            for part in cpus.split(',') {
                let valid = if let Some((lo, hi)) = part.split_once('-') {
                    matches!((number(lo), number(hi)), (Some(lo), Some(hi)) if lo <= hi)
                } else {
                    number(part).is_some()
                };
                if !valid {
                    return Err(invalid("placement.cpus", "expected a nonempty cpulist"));
                }
            }
        }
        if self.run_ahead.is_some_and(|n| n > 64) {
            return Err(invalid("placement.run_ahead", "expected 0..64"));
        }
        for (field, path) in [
            ("placement.metrics", &self.metrics),
            ("placement.control", &self.control),
        ] {
            if path
                .as_ref()
                .is_some_and(|p| p.is_empty() || p.contains('\0'))
            {
                return Err(invalid(field, "expected a nonempty path without NUL"));
            }
        }
        Ok(())
    }
}
