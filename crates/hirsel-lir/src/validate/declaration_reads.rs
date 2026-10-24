//! Independent declaration observations made by C, replayed without a Value.
use crate::operation::{NativeType, OperationArgument, OperationDeclarations};
use crate::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_core_contract::OperationId;
use hirsel_syntax::{ContentHash, DefPath};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Clone, Debug)]
pub(super) struct InvocationRead {
    definition: DefPath,
    output: ContentHash,
}
hirsel_codec::snapshot_struct!(InvocationRead { definition, output });
#[derive(Clone, Debug)]
pub(super) struct BodyRead {
    definition: DefPath,
    instance: ContentHash,
    body: Option<u32>,
}
hirsel_codec::snapshot_struct!(BodyRead {
    definition,
    instance,
    body
});
#[derive(Clone, Debug)]
pub(super) struct StructuralRead {
    source: NativeType,
    output: ContentHash,
}
hirsel_codec::snapshot_struct!(StructuralRead { source, output });
#[derive(Clone, Debug)]
pub(super) struct Reads {
    invocation_required: Option<bool>,
    structural_required: Option<bool>,
    invocations: Vec<InvocationRead>,
    bodies: Vec<BodyRead>,
    structural: Vec<StructuralRead>,
}
hirsel_codec::snapshot_struct!(Reads {
    invocation_required,
    structural_required,
    invocations,
    bodies,
    structural
});

fn invocation_identity(rows: Option<&[crate::invocations::ReferenceInvocation]>) -> ContentHash {
    let rows = rows.map(|rows| {
        rows.iter()
            .map(|row| {
                (
                    row.body,
                    row.block,
                    row.statement,
                    row.restart,
                    row.primitive.clone(),
                    row.callee.clone(),
                )
            })
            .collect::<Vec<_>>()
    });
    let bytes = hirsel_codec::to_text(&rows);
    ContentHash::of_parts("hirsel.native.invocation-input.v1", [bytes.as_bytes()])
}
fn structural_identity(plan: &crate::structural_access::AccessPlan) -> ContentHash {
    let bytes = hirsel_codec::to_text(plan);
    ContentHash::of_parts("hirsel.native.structural-input.v1", [bytes.as_bytes()])
}
impl Reads {
    pub(super) fn matches(&self, declarations: &dyn OperationDeclarations) -> bool {
        // A target inventory also needs canonical ABI identities, not table-local
        // ABI indices. Until that input exists, those outputs are not reusable.
        declarations.pretransfer_adapters().is_empty()
            && self
                .invocation_required
                .is_none_or(|v| v == declarations.requires_invocation_reference())
            && self
                .structural_required
                .is_none_or(|v| v == declarations.requires_structural_reference())
            && self.invocations.iter().all(|read| {
                invocation_identity(declarations.invocations(&read.definition)) == read.output
            })
            && self.bodies.iter().all(|read| {
                declarations.invocation_body(&read.definition, read.instance) == read.body
            })
    }
    pub(super) fn has_structural(&self) -> bool {
        !self.structural.is_empty()
    }
    pub(super) fn matches_structural(
        &self,
        declarations: &dyn OperationDeclarations,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> bool {
        self.structural.iter().all(|read| {
            declarations
                .structural_access(&read.source, table, target)
                .is_ok_and(|plan| structural_identity(&plan) == read.output)
        })
    }
}

#[derive(Default)]
struct Observations {
    invocation_required: Option<bool>,
    structural_required: Option<bool>,
    invocations: BTreeMap<DefPath, ContentHash>,
    bodies: BTreeMap<(DefPath, ContentHash), Option<u32>>,
    structural: BTreeMap<String, NativeType>,
    unsupported: bool,
}
pub(super) struct Observed<'a> {
    declarations: &'a (dyn OperationDeclarations + Sync),
    reads: Mutex<Observations>,
}
impl<'a> Observed<'a> {
    pub(super) fn new(declarations: &'a (dyn OperationDeclarations + Sync)) -> Self {
        Self {
            declarations,
            reads: Mutex::new(Observations::default()),
        }
    }
    pub(super) fn finish(&self, table: &LayoutTable, target: &TargetSpec) -> Option<Reads> {
        let reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
        if reads.unsupported {
            return None;
        }
        let structural = reads
            .structural
            .values()
            .map(|source| {
                let plan = self
                    .declarations
                    .structural_access(source, table, target)
                    .ok()?;
                Some(StructuralRead {
                    source: source.clone(),
                    output: structural_identity(&plan),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Reads {
            invocation_required: reads.invocation_required,
            structural_required: reads.structural_required,
            invocations: reads
                .invocations
                .iter()
                .map(|(definition, output)| InvocationRead {
                    definition: definition.clone(),
                    output: *output,
                })
                .collect(),
            bodies: reads
                .bodies
                .iter()
                .map(|((definition, instance), body)| BodyRead {
                    definition: definition.clone(),
                    instance: *instance,
                    body: *body,
                })
                .collect(),
            structural,
        })
    }
}
impl OperationDeclarations for Observed<'_> {
    fn seal_source_witness(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<crate::sealed_payload_source::SealSourceWitness> {
        self.declarations.seal_source_witness(definition, instance)
    }
    fn seal_events(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<crate::sealed_payload_source::SourceEvent>> {
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unsupported = true;
        self.declarations.seal_events(definition, instance)
    }
    fn seal_payloads(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<crate::sealed_payload_source::SourcePayload>> {
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unsupported = true;
        self.declarations.seal_payloads(definition, instance)
    }
    fn pretransfer_adapters(&self) -> &[crate::pretransfer::AdapterReference] {
        let result = self.declarations.pretransfer_adapters();
        if !result.is_empty() {
            self.reads
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unsupported = true;
        }
        result
    }
    fn requires_structural_reference(&self) -> bool {
        let output = self.declarations.requires_structural_reference();
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .structural_required = Some(output);
        output
    }
    fn structural_access(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<crate::structural_access::AccessPlan, LayoutError> {
        let plan = self.declarations.structural_access(source, table, target)?;
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .structural
            .insert(hirsel_codec::to_text(source), source.clone());
        Ok(plan)
    }
    fn requires_invocation_reference(&self) -> bool {
        let output = self.declarations.requires_invocation_reference();
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invocation_required = Some(output);
        output
    }
    fn invocations(
        &self,
        definition: &DefPath,
    ) -> Option<&[crate::invocations::ReferenceInvocation]> {
        let output = self.declarations.invocations(definition);
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .invocations
            .insert(definition.clone(), invocation_identity(output));
        output
    }
    fn invocation_body(&self, definition: &DefPath, instance: ContentHash) -> Option<u32> {
        let output = self.declarations.invocation_body(definition, instance);
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bodies
            .insert((definition.clone(), instance), output);
        output
    }
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(FnAbi, LayoutTable), LayoutError> {
        self.declarations.derive(operation, arguments, target)
    }
}
