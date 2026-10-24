//! CLI transport of the public MIT run-schema worlds. Provider effects and launch
//! admission remain authoritative; this is deliberately not SimulationConfig serde.
use std::path::Path;

pub(crate) const PROJECTIONS: &[&str] = &[
    "stdin",
    "stderr",
    "stdout",
    "tasks",
    "net",
    "process",
    "random",
    "dir",
    "signals",
    "telemetry",
];

fn bytes(value: &serde_json::Value, field: &str) -> Result<Vec<u8>, String> {
    if let Some(text) = value.as_str() {
        return Ok(text.as_bytes().to_vec());
    }
    value
        .as_array()
        .ok_or_else(|| format!("{field}: expected text or an array of bytes"))?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| format!("{field}: expected bytes (0..=255)"))
        })
        .collect()
}

pub(crate) fn read(path: &Path) -> Result<hirsel_engine::SimulationConfig, String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    decode(value)
}

pub(crate) fn decode(value: serde_json::Value) -> Result<hirsel_engine::SimulationConfig, String> {
    let object = value.as_object().ok_or("expected a JSON object")?;
    let mut config = hirsel_engine::SimulationConfig::default();
    for (key, value) in object {
        match key.as_str() {
            "args" | "grants" | "directories" | "projections" => {
                let values = value
                    .as_array()
                    .ok_or_else(|| format!("{key}: expected an array"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("{key}: expected strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                match key.as_str() {
                    "args" => config.args = values,
                    "grants" => config.grants = values.into_iter().collect(),
                    "projections" => {
                        for projection in &values {
                            if !PROJECTIONS.contains(&projection.as_str()) {
                                let recipe = if matches!(
                                    projection.as_str(),
                                    "monotonic_clock" | "wall_clock" | "clock"
                                ) {
                                    "; clocks use grants: [\"Io\", \"MonotonicClock\", \"WallClock\"] with monotonic/wall values"
                                } else {
                                    ""
                                };
                                return Err(format!("unknown Io projection {projection}{recipe}"));
                            }
                        }
                        config.projections = values.into_iter().collect();
                    }
                    _ => config.directories = values.into_iter().collect(),
                }
            }
            "race" => config.debug.races = value.as_bool().ok_or("race: expected a boolean")?,
            "sim_points" => {
                let points = value
                    .as_str()
                    .and_then(hirsel_driver::testing::SimulationDebug::parse_points)
                    .ok_or(
                        "sim_points: expected root-release, atomic-store, or both comma-separated",
                    )?;
                config.debug.root_releases = points.root_releases;
                config.debug.atomic_stores = points.atomic_stores;
            }
            "scheduler" => {
                config.scheduler = Some(
                    value
                        .as_str()
                        .and_then(hirsel_driver::testing::Scheduler::parse)
                        .ok_or("scheduler: expected name@1:seed")?,
                )
            }
            "stdin" => {
                config.stdin = bytes(value, "stdin")?;
            }
            "files" | "env" => {
                let entries = value
                    .as_object()
                    .ok_or_else(|| format!("{key}: expected an object"))?;
                for (name, value) in entries {
                    if key == "files" {
                        config
                            .files
                            .insert(name.clone(), bytes(value, &format!("files.{name}"))?);
                    } else if value.is_null() {
                        config.env.insert(name.clone(), None);
                    } else {
                        config.env.insert(
                            name.clone(),
                            Some(value.as_str().ok_or("env: expected string values")?.into()),
                        );
                    }
                }
            }
            "signals" => {
                config.signals = Some(
                    hirsel_run_config::validate_signals_grant(value.to_string().as_bytes())
                        .map_err(|e| format!("signals: {e:?}"))?,
                );
            }
            "mounts" => {
                config.mounts =
                    serde_json::from_value(value.clone()).map_err(|e| format!("mounts: {e}"))?;
            }
            "file_reads" => {
                // FileReadScript's shared derived enum tolerates extra AfterWrite fields.
                // Close that JSON shape here without changing the typed provider contract.
                let scripts = value.as_object().ok_or("file_reads: expected an object")?;
                for script in scripts.values() {
                    if let Some(after) = script.get("AfterWrite") {
                        let after = after
                            .as_object()
                            .ok_or("file_reads.AfterWrite: expected an object")?;
                        for field in after.keys() {
                            if field != "path" {
                                return Err(format!(
                                    "file_reads.AfterWrite: unknown field {field}"
                                ));
                            }
                        }
                    }
                }
                config.file_reads = serde_json::from_value(value.clone())
                    .map_err(|e| format!("file_reads: {e}"))?;
            }
            "file_faults" => {
                config.file_faults = serde_json::from_value(value.clone())
                    .map_err(|e| format!("file_faults: {e}"))?;
            }

            // The RunConfig `io.net` object; the simulation serves net.script@2.
            "net" => {
                config.net =
                    Some(serde_json::from_value(value.clone()).map_err(|e| format!("net: {e}"))?)
            }
            // The RunConfig `io.process` object; the simulation serves process.script@1.
            "process" => {
                config.process = Some(
                    serde_json::from_value(value.clone()).map_err(|e| format!("process: {e}"))?,
                )
            }
            "wall" => config.wall = value.as_i64().ok_or("wall: expected a signed integer")?,
            "monotonic" => {
                config.monotonic = value
                    .as_u64()
                    .ok_or("monotonic: expected a nonnegative integer")?
            }
            _ => return Err(format!("unknown field {key}")),
        }
    }
    hirsel_run_schema::validate_file_faults(&config.file_faults)?;
    Ok(config)
}
