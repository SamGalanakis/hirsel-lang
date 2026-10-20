//! Registry-owned native operation facts, joined with the exported dispatch table.
use crate::registry::BuiltinRegistry;
use hirsel_interp::library::OperationDisposition;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{Allocation, DropAllocation, ParamId, PassingMode, SignatureKind, SignatureRef};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OperationKey {
    pub definition: DefPath,
    pub declaration_digest: ContentHash,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationParameter {
    pub ordinal: ParamId,
    pub passing: PassingMode,
    pub cleanup: DropAllocation,
}
/// Independent dimensions; true includes unknown effects. No purity is inferred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationEffects {
    pub reads: bool,
    pub writes: bool,
    pub owns: bool,
    pub may_fail: bool,
    pub allocates: bool,
    pub observable: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationRecord {
    pub key: OperationKey,
    /// Retains the owning graph, schema, contracts, authority and generics.
    pub declaration: SignatureRef,
    pub parameters: Vec<OperationParameter>,
    pub result_cleanup: DropAllocation,
    pub invocation_allocation: Allocation,
    pub effects: OperationEffects,
    /// None is an implementation gap; Unavailable is an explicit engine outcome.
    pub reference: Option<OperationDisposition>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryMismatch {
    pub missing_implementations: BTreeSet<DefPath>,
    pub undeclared_implementations: BTreeSet<DefPath>,
}
pub fn check_operation_coverage(
    declared: impl IntoIterator<Item = DefPath>,
    implemented: impl IntoIterator<Item = DefPath>,
) -> Result<(), InventoryMismatch> {
    let declared: BTreeSet<_> = declared.into_iter().collect();
    let implemented: BTreeSet<_> = implemented.into_iter().collect();
    let mismatch = InventoryMismatch {
        missing_implementations: declared.difference(&implemented).cloned().collect(),
        undeclared_implementations: implemented.difference(&declared).cloned().collect(),
    };
    if mismatch.missing_implementations.is_empty() && mismatch.undeclared_implementations.is_empty()
    {
        Ok(())
    } else {
        Err(mismatch)
    }
}
impl BuiltinRegistry {
    /// Headers remain the sole declaration authority. Implementation availability
    /// is annotation from the same tables that perform interpreter dispatch.
    /// Lowered intrinsics disappear into TIR and have no executable operation row.
    pub fn operation_inventory(&self) -> BTreeMap<OperationKey, OperationRecord> {
        let dispatch = hirsel_interp::library::operation_dispatch();
        let types = self.types();
        self.signatures()
            .filter_map(|signature| {
                let SignatureKind::Function(function) = &signature.kind else {
                    return None;
                };
                if hirsel_types::core_roles::LoweredIntrinsic::for_definition(
                    signature.definition.path(),
                )
                .is_some()
                {
                    return None;
                }
                let key = OperationKey {
                    definition: signature.definition.path().clone(),
                    declaration_digest: signature.fingerprint,
                };
                let parameters: Vec<_> = function
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(ordinal, p)| OperationParameter {
                        ordinal: ParamId(ordinal as u32),
                        passing: p.passing,
                        cleanup: types
                            .get(p.ty)
                            .expect("declared parameter type")
                            .drop_allocation
                            .clone(),
                    })
                    .collect();
                let result_cleanup = types
                    .get(function.result)
                    .expect("declared result type")
                    .drop_allocation
                    .clone();
                let allocates = function.allocation == Allocation::MayAllocate
                    || parameters.iter().any(|p| {
                        p.passing == PassingMode::Take && p.cleanup != DropAllocation::NoAlloc
                    })
                    || result_cleanup != DropAllocation::NoAlloc;
                let record = OperationRecord {
                    key: key.clone(),
                    declaration: signature.clone(),
                    parameters,
                    result_cleanup,
                    invocation_allocation: function.allocation,
                    effects: OperationEffects {
                        reads: true,
                        writes: true,
                        owns: true,
                        may_fail: true,
                        allocates,
                        observable: true,
                    },
                    reference: dispatch.get(signature.definition.path()).copied(),
                };
                Some((key, record))
            })
            .collect()
    }
}
