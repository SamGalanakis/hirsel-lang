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

/// Launcher-owned provenance, distinct from a request accepted by RunConfig.
/// This encoding is reset with ordinary tape v9 / durable journal v3.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedPlacement {
    pub schema: u32,
    pub request: Placement,
    pub admission: CapacityAdmission,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapacityAdmission {
    Physical {
        requested: Cores,
        resolved: u64,
        launched: u64,
        permitted: u64,
        effective: u64,
        worker_phase: crate::managed_cores::WorkerPhase,
    },
    NotPhysical {
        requested: Cores,
        worker_phase: crate::managed_cores::WorkerPhase,
        reason: CapacityExclusion,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityExclusion {
    ExecutorNotObserved,
    ScriptedWorld,
}
impl From<Placement> for RecordedPlacement {
    fn from(request: Placement) -> Self {
        Self {
            schema: 1,
            admission: CapacityAdmission::NotPhysical {
                requested: request.cores.clone(),
                worker_phase: crate::managed_cores::WorkerPhase::Starting,
                reason: CapacityExclusion::ExecutorNotObserved,
            },
            request,
        }
    }
}
impl RecordedPlacement {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("recorded placement serialization")
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self, RunRefusal> {
        crate::integer_tokens(bytes)?;
        let value: Self = serde_json::from_slice(bytes)
            .map_err(|e| invalid("recorded_placement", &e.to_string()))?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), RunRefusal> {
        use crate::managed_cores::WorkerPhase;
        self.request.validate()?;
        if self.schema != 1 {
            return Err(invalid("recorded_placement.schema", "expected 1"));
        }
        let requested = match &self.admission {
            CapacityAdmission::Physical {
                requested,
                resolved,
                launched,
                permitted,
                effective,
                worker_phase,
            } => {
                let bound = match requested {
                    Cores::Fixed(n) => *n,
                    Cores::Range(range) => range.max,
                    Cores::Auto(_) => u64::MAX,
                };
                if *resolved == 0
                    || *resolved > bound
                    || *launched == 0
                    || launched > resolved
                    || *effective != (*launched).min(*permitted)
                    || *worker_phase != WorkerPhase::Running
                {
                    return Err(invalid(
                        "recorded_placement.admission",
                        "inconsistent physical capacity",
                    ));
                }
                requested
            }
            CapacityAdmission::NotPhysical { requested, .. } => requested,
        };
        if requested != &self.request.cores {
            return Err(invalid(
                "recorded_placement.admission.requested",
                "differs from placement request",
            ));
        }
        Ok(())
    }
    pub fn proves(&self, width: u64) -> bool {
        self.validate().is_ok()
            && matches!(&self.admission,
            CapacityAdmission::Physical { requested: Cores::Fixed(n), resolved, effective, .. }
                if *n == width && *resolved == width && *effective == width)
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::*;
    use crate::managed_cores::WorkerPhase;
    fn physical() -> RecordedPlacement {
        RecordedPlacement {
            schema: 1,
            request: Placement {
                cores: Cores::Fixed(4),
                ..Default::default()
            },
            admission: CapacityAdmission::Physical {
                requested: Cores::Fixed(4),
                resolved: 4,
                launched: 4,
                permitted: 4,
                effective: 4,
                worker_phase: WorkerPhase::Running,
            },
        }
    }
    #[test]
    fn physical_capacity_is_derived_from_launched_destinations_and_permits() {
        let value = physical();
        assert!(value.proves(4));
        assert_eq!(
            RecordedPlacement::from_json(&value.to_json()).unwrap(),
            value
        );
        for (field, bad) in [
            ("effective", serde_json::json!(3)),
            ("effective", serde_json::json!(5)),
            ("requested", serde_json::json!(2)),
            ("resolved", serde_json::json!(5)),
            ("launched", serde_json::json!(3)),
            ("permitted", serde_json::json!(3)),
            ("worker_phase", serde_json::json!("starting")),
        ] {
            let mut forged = serde_json::to_value(&value).unwrap();
            forged["admission"][field] = bad;
            assert!(
                RecordedPlacement::from_json(&serde_json::to_vec(&forged).unwrap()).is_err(),
                "{field}"
            );
        }
        let mut capped = value;
        if let CapacityAdmission::Physical {
            permitted,
            effective,
            ..
        } = &mut capped.admission
        {
            *permitted = 2;
            *effective = 2;
        }
        capped.validate().unwrap();
        assert!(!capped.proves(4));
        if let CapacityAdmission::Physical {
            permitted,
            effective,
            ..
        } = &mut capped.admission
        {
            *permitted = 0;
            *effective = 0;
        }
        capped.validate().unwrap();
        assert!(!capped.proves(4));
    }
    #[test]
    fn requests_cannot_supply_launcher_evidence_and_exclusions_cannot_supply_effective() {
        assert!(Placement::from_json(&physical().to_json()).is_err());
        let value = RecordedPlacement::from(Placement::default());
        let mut json = serde_json::to_value(&value).unwrap();
        assert!(json["admission"].get("effective").is_none());
        assert!(!value.proves(1));
        json["admission"]["effective"] = serde_json::json!(1);
        assert!(RecordedPlacement::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
