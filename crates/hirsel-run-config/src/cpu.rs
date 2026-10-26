//! Finite scoped CPU authority, attached to Tasks rather than a new root.
use crate::invalid;
use hirsel_run_schema::{CpuGrant, ProviderContract, RunRefusal};

pub(super) fn validate(grant: &CpuGrant, tasks: bool, field: &str) -> Result<(), RunRefusal> {
    let at = |member: &str| format!("{field}.{member}");
    if ProviderContract::parse(&grant.provider) != Some(ProviderContract::CpuScoped) {
        return Err(RunRefusal::Unsupported {
            field: at("provider"),
        });
    }
    if !tasks {
        return Err(invalid(field, "cpu.scoped@1 requires Tasks"));
    }
    if !(1..=16).contains(&grant.max_workers) {
        return Err(invalid(&at("max_workers"), "expected 1..=16"));
    }
    if grant.max_domains != 1 {
        return Err(invalid(
            &at("max_domains"),
            "cpu.scoped@1 requires one domain",
        ));
    }
    for (member, bytes) in [
        ("max_physical_bytes", grant.max_physical_bytes),
        ("max_batch_bytes", grant.max_batch_bytes),
    ] {
        if bytes == 0 {
            return Err(invalid(&at(member), "expected a positive byte limit"));
        }
    }
    Ok(())
}
