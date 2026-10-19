//! Explicit expected stdout for compiled-only library evidence. Ordinary
//! corpus and regression programs always retain their interpreter reference.
use hirsel_driver::native::Observation;
use std::path::{Component, Path};

pub(super) fn expected_stdout(
    id: &str,
    source: &str,
    path: Option<&Path>,
) -> Result<Option<Observation>, String> {
    let mut oracle = None;
    let mut reason = None;
    let mut in_header = true;
    for line in source.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix("//").map(str::trim) else {
            in_header = false;
            continue;
        };
        let Some((key, value)) = comment.split_once(':') else {
            if matches!(
                comment.split_whitespace().next(),
                Some("oracle" | "oracle-reason")
            ) {
                return Err("malformed oracle directive (missing ':')".into());
            }
            continue;
        };
        let slot = match key.trim() {
            "oracle" => &mut oracle,
            "oracle-reason" => &mut reason,
            _ => continue,
        };
        if !in_header {
            return Err("oracle directive after source code".into());
        }
        if slot.replace(value.trim()).is_some() {
            return Err(format!("duplicate {} directive", key.trim()));
        }
    }
    let Some(oracle) = oracle else {
        return if reason.is_some() {
            Err("oracle-reason requires oracle: expected-stdout".into())
        } else {
            Ok(None)
        };
    };
    if oracle != "expected-stdout" {
        return Err("oracle must be expected-stdout".into());
    }
    if reason.is_none_or(str::is_empty) {
        return Err("oracle: expected-stdout requires a nonempty oracle-reason line".into());
    }
    let id = Path::new(id);
    if !id.starts_with("native-tests/library")
        || !id
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || id.extension().is_none_or(|extension| extension != "hir")
    {
        return Err("oracle: expected-stdout is only allowed in native-tests/library".into());
    }
    let path = path.ok_or("oracle: expected-stdout requires a source path and .stdout sidecar")?;
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !path.ends_with(id) {
        return Err("oracle source must be inside native-tests/library with its case ID".into());
    }
    let sidecar = path.with_extension("stdout");
    let output = std::fs::read(&sidecar).map_err(|e| format!("{}: {e}", sidecar.display()))?;
    Ok(Some(Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output,
        failures: Vec::new(),
        roots: Vec::new(),
    }))
}
