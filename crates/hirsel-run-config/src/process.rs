//! Run grant normalization and validation.
use crate::invalid;
use hirsel_run_schema::*;

/// A launcher's standalone `io.process` grant: the JSON of a RunConfig's `io.process`
/// field, admitted exactly as that field is. Scripted runs never widen the rules.
pub fn validate_process_grant(bytes: &[u8]) -> Result<ProcessGrant, RunRefusal> {
    let mut grant: ProcessGrant =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.process", &error.to_string()))?;
    if !matches!(
        ProviderContract::parse(&grant.provider),
        Some(
            ProviderContract::ProcessSpawn
                | ProviderContract::ProcessScript
                | ProviderContract::ProcessScriptTimed
        )
    ) {
        return Err(RunRefusal::Unsupported {
            field: "io.process.provider".into(),
        });
    }
    validate(&mut grant)?;
    Ok(grant)
}

pub(super) fn validate(grant: &mut ProcessGrant) -> Result<(), RunRefusal> {
    let timed =
        ProviderContract::parse(&grant.provider) == Some(ProviderContract::ProcessScriptTimed);
    let script =
        timed || ProviderContract::parse(&grant.provider) == Some(ProviderContract::ProcessScript);
    if timed != grant.timed.is_some() || timed && grant.children.is_some() {
        return Err(invalid(
            "io.process",
            "invalid timed process representation",
        ));
    }
    if let Some(timed) = &grant.timed {
        if timed.clock != "run.monotonic" {
            return Err(invalid("io.process.clock", "expected run.monotonic"));
        }
        for child in &timed.children {
            child
                .validate()
                .map_err(|e| invalid("io.process.children", e))?;
        }
    }
    if grant.runs.is_some() != script {
        return Err(invalid(
            "io.process.runs",
            "runs is required only for process.script@1",
        ));
    }
    if !script && grant.children.is_some() {
        return Err(invalid(
            "io.process.children",
            "children is only for process.script@1",
        ));
    }
    grant.rules.sort_by(|a, b| a.program.cmp(&b.program));
    for rule in &mut grant.rules {
        if !canonical_program(&rule.program) {
            return Err(invalid(
                "io.process.rules.program",
                "expected a canonical absolute path",
            ));
        }
        rule.env.sort();
        if rule
            .env
            .iter()
            .any(|name| name.is_empty() || name.contains(['=', '\0']))
        {
            return Err(invalid("io.process.rules.env", "invalid environment name"));
        }
        if rule.env.windows(2).any(|p| p[0] == p[1]) {
            return Err(invalid(
                "io.process.rules.env",
                "duplicate environment name",
            ));
        }
    }
    if grant.rules.windows(2).any(|p| p[0].program == p[1].program) {
        return Err(invalid("io.process.rules", "duplicate program"));
    }
    for run in grant.runs.iter().flatten() {
        if !canonical_program(&run.program) {
            return Err(invalid(
                "io.process.runs.program",
                "expected a canonical absolute path",
            ));
        }
        if !scriptable(&run.outcome) {
            return Err(invalid(
                "io.process.runs.outcome",
                "a script cannot be canceled",
            ));
        }
    }
    for child in grant.children.iter().flatten() {
        if !canonical_program(&child.program) {
            return Err(invalid(
                "io.process.children.program",
                "expected a canonical absolute path",
            ));
        }
    }
    Ok(())
}
