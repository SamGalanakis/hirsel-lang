//! `io.signals` grant validation (CAP-1).
use crate::invalid;
use hirsel_run_schema::{ProviderContract, RunRefusal, SignalsGrant};

/// A launcher's standalone `io.signals` grant: the JSON of a RunConfig's `io.signals`
/// field, admitted exactly as that field is.
pub fn validate_signals_grant(bytes: &[u8]) -> Result<SignalsGrant, RunRefusal> {
    let mut grant: SignalsGrant =
        serde_json::from_slice(bytes).map_err(|error| invalid("io.signals", &error.to_string()))?;
    if !matches!(
        ProviderContract::parse(&grant.provider),
        Some(ProviderContract::SignalsHost | ProviderContract::SignalsScript)
    ) {
        return Err(RunRefusal::Unsupported {
            field: "io.signals.provider".into(),
        });
    }
    validate(&mut grant)?;
    Ok(grant)
}

pub(super) fn validate(grant: &mut SignalsGrant) -> Result<(), RunRefusal> {
    let script = ProviderContract::parse(&grant.provider) == Some(ProviderContract::SignalsScript);
    if grant.script.is_some() != script {
        return Err(invalid(
            "io.signals.script",
            "script is required only for signals.script@1",
        ));
    }
    if grant.kinds.is_empty() {
        return Err(invalid("io.signals.kinds", "expected at least one kind"));
    }
    grant.kinds.sort_by_key(|kind| kind.name());
    if grant.kinds.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(invalid("io.signals.kinds", "duplicate kind"));
    }
    Ok(())
}
