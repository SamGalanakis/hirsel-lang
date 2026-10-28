//! Independent declaration observations made by C, replayed without a Value.
use crate::operation::{NativeType, OperationArgument, OperationDeclarations};
use crate::sealed_payload_source::ReceiptSource;
use crate::{FnAbi, LayoutTable, TargetSpec, layout::LayoutError};
use hirsel_core_contract::OperationId;
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

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
/// Source expectations are independently read and checked against their witness.
/// Neither the candidate's tables nor its header can manufacture this input.
#[derive(Clone, Debug)]
struct SealedRead {
    definition: DefPath,
    instance: ContentHash,
    witness: crate::sealed_payload_source::SealSourceWitness,
}
hirsel_codec::snapshot_struct!(SealedRead {
    definition,
    instance,
    witness
});

#[derive(Clone, Debug)]
struct ReceiptRead {
    definition: DefPath,
    instance: ContentHash,
    output: Option<ReceiptSource>,
}
hirsel_codec::snapshot_struct!(ReceiptRead {
    definition,
    instance,
    output
});

#[derive(Clone, Debug)]
struct EnvironmentRead {
    definition: DefPath,
    instance: ContentHash,
    output: Option<crate::sealed_payload_source::SourceEnvironments>,
}
hirsel_codec::snapshot_struct!(EnvironmentRead {
    definition,
    instance,
    output
});
fn environment_read(
    declarations: &dyn OperationDeclarations,
    definition: &DefPath,
    instance: ContentHash,
) -> Option<EnvironmentRead> {
    let output = declarations.seal_environments(definition, instance);
    if output
        .iter()
        .flat_map(|sources| sources.constructions.iter().chain(sources.entry.iter()))
        .any(|environment| {
            environment.original.definition != *definition || environment.check_structure().is_err()
        })
    {
        return None;
    }
    Some(EnvironmentRead {
        definition: definition.clone(),
        instance,
        output,
    })
}

fn receipt_read(
    declarations: &dyn OperationDeclarations,
    definition: &DefPath,
    instance: ContentHash,
) -> Option<ReceiptRead> {
    let output = declarations.receipt_source(definition, instance);
    if let Some(source) = &output {
        source.check_structure().ok()?;
        if source.original.definition != *definition || source.instance != instance {
            return None;
        }
    }
    Some(ReceiptRead {
        definition: definition.clone(),
        instance,
        output,
    })
}

#[derive(Clone, Debug)]
struct DescriptorRead {
    source: NativeType,
    owner: Option<(DefPath, ContentHash)>,
    output: ContentHash,
}
hirsel_codec::snapshot_struct!(DescriptorRead {
    source,
    owner,
    output
});

fn source_read(
    declarations: &dyn OperationDeclarations,
    definition: &DefPath,
    instance: ContentHash,
) -> Option<SealedRead> {
    let witness = declarations.seal_source_witness(definition, instance)?;
    let events = declarations.seal_events(definition, instance)?;
    let payloads = declarations.seal_payloads(definition, instance)?;
    let environments = declarations.seal_environments(definition, instance);
    for environment in environments
        .iter()
        .flat_map(|sources| sources.constructions.iter().chain(sources.entry.iter()))
    {
        environment.check_structure().ok()?;
    }
    if witness.definition != *definition
        || witness.instance != instance
        || witness
            != crate::sealed_payload_source::SealSourceWitness::from_expectations(
                witness.source,
                definition.clone(),
                witness.implementation_revision,
                instance,
                &events,
                &payloads,
            )
            .with_environments(environments.as_ref())
    {
        return None;
    }
    Some(SealedRead {
        definition: definition.clone(),
        instance,
        witness,
    })
}

fn expectation_identity<T: hirsel_codec::Snapshot>(value: &T) -> ContentHash {
    ContentHash::of_bytes(hirsel_codec::to_text(value).as_bytes())
}

// Follow objects::table_ids_mut's complete descriptor walk. Hash physical
// references by LayoutKey so a portable Binding's local numbering is irrelevant.
fn descriptor_identity(
    descriptor: &crate::sealed::SealDescriptor,
    table: &LayoutTable,
) -> Option<ContentHash> {
    descriptor.check(table).ok()?;
    let keys = crate::LayoutKeys::new(table);
    let mut descriptor = descriptor.clone();
    descriptor
        .access
        .sort_by_key(|access| keys.key(access.layout));
    let mut references = Vec::new();
    let mut erase = |layout: &mut crate::LayoutId| {
        references.push(keys.key(*layout));
        *layout = crate::LayoutId(0);
    };
    erase(&mut descriptor.root_layout);
    for node in &mut descriptor.projection.nodes {
        for layout in node.layouts_mut() {
            erase(layout);
        }
    }
    for access in &mut descriptor.access {
        erase(&mut access.layout);
        crate::layout_keys::scan_references_mut(&mut access.scan, &mut erase);
    }
    Some(expectation_identity(&(descriptor, references)))
}
impl DescriptorRead {
    fn matches(
        &self,
        declarations: &dyn OperationDeclarations,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> bool {
        let output = match &self.owner {
            Some((definition, instance)) => declarations.seal_descriptor_for_source(
                definition,
                *instance,
                &self.source,
                table,
                target,
            ),
            None => declarations.seal_descriptor(&self.source, table, target),
        };
        output
            .ok()
            .and_then(|descriptor| descriptor_identity(&descriptor, table))
            == Some(self.output)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AdapterRead {
    adapter: crate::ir::Symbol,
    primitive: crate::ir::Symbol,
    abi: ContentHash,
}
hirsel_codec::snapshot_struct!(AdapterRead {
    adapter,
    primitive,
    abi
});

fn adapter_inventory(
    declarations: &dyn OperationDeclarations,
    target: &TargetSpec,
) -> Option<Vec<AdapterRead>> {
    let mut rows = declarations
        .pretransfer_adapters()
        .iter()
        .map(|reference| {
            if reference.adapter.kind != crate::ir::SymbolKind::Function
                || reference.primitive.kind != crate::ir::SymbolKind::Function
                || reference.adapter.instance.is_none()
                || reference.primitive.instance.is_none()
            {
                return None;
            }
            let (abi, table) = declarations.pretransfer_abi(reference, target)?;
            Some(AdapterRead {
                adapter: reference.adapter.clone(),
                primitive: reference.primitive.clone(),
                abi: crate::LayoutKeys::new(&table).fn_abi(&abi),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    rows.sort_by(|a, b| {
        (&a.adapter.definition, a.adapter.instance)
            .cmp(&(&b.adapter.definition, b.adapter.instance))
    });
    if rows.windows(2).any(|w| w[0].adapter == w[1].adapter) {
        return None;
    }
    Some(rows)
}

#[derive(Clone, Debug)]
pub(super) struct Reads {
    invocation_required: Option<bool>,
    structural_required: Option<bool>,
    invocations: Vec<InvocationRead>,
    bodies: Vec<BodyRead>,
    structural: Vec<StructuralRead>,
    pretransfer: Option<Vec<AdapterRead>>,
    unavailable_events: Vec<(DefPath, ContentHash)>,
    unavailable_payloads: Vec<(DefPath, ContentHash)>,
    sealed: Vec<SealedRead>,
    descriptors: Vec<DescriptorRead>,
    receipts: Vec<ReceiptRead>,
    environments: Vec<EnvironmentRead>,
}
hirsel_codec::snapshot_struct!(Reads {
    invocation_required,
    structural_required,
    invocations,
    bodies,
    structural,
    pretransfer,
    unavailable_events,
    unavailable_payloads,
    sealed,
    descriptors,
    receipts,
    environments
});

fn invocation_identity(rows: Option<&[crate::invocations::ReferenceInvocation]>) -> ContentHash {
    let rows = rows.map(|rows| {
        rows.iter()
            .map(|row| {
                (
                    (row.body, row.block),
                    (
                        (row.statement, row.restart),
                        (row.primitive.clone(), row.callee.clone()),
                    ),
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
    pub(super) fn matches(
        &self,
        declarations: &dyn OperationDeclarations,
        target: &TargetSpec,
    ) -> bool {
        self.environments.iter().all(|read| {
            environment_read(declarations, &read.definition, read.instance)
                .is_some_and(|current| current.output == read.output)
        }) && self.receipts.iter().all(|read| {
            receipt_read(declarations, &read.definition, read.instance)
                .is_some_and(|current| current.output == read.output)
        }) && self.sealed.iter().all(|read| {
            source_read(declarations, &read.definition, read.instance)
                .is_some_and(|current| current.witness == read.witness)
        }) && self
            .unavailable_events
            .iter()
            .all(|(definition, instance)| declarations.seal_events(definition, *instance).is_none())
            && self
                .unavailable_payloads
                .iter()
                .all(|(definition, instance)| {
                    declarations.seal_payloads(definition, *instance).is_none()
                })
            && self.pretransfer.as_ref().is_none_or(|expected| {
                adapter_inventory(declarations, target).as_ref() == Some(expected)
            })
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
        !self.structural.is_empty() || !self.descriptors.is_empty()
    }
    pub(super) fn matches_structural(
        &self,
        declarations: &dyn OperationDeclarations,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> bool {
        self.descriptors
            .iter()
            .all(|read| read.matches(declarations, table, target))
            && self.structural.iter().all(|read| {
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
    available_events: BTreeMap<(DefPath, ContentHash), ContentHash>,
    available_payloads: BTreeMap<(DefPath, ContentHash), ContentHash>,
    descriptors: BTreeMap<String, DescriptorRead>,
    unavailable_events: BTreeSet<(DefPath, ContentHash)>,
    unavailable_payloads: BTreeSet<(DefPath, ContentHash)>,
    pretransfer: bool,
    receipts: BTreeMap<(DefPath, ContentHash), Option<ReceiptSource>>,
    environments:
        BTreeMap<(DefPath, ContentHash), Option<crate::sealed_payload_source::SourceEnvironments>>,
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
    pub(super) fn finish(
        &self,
        table: &LayoutTable,
        original: &LayoutTable,
        target: &TargetSpec,
    ) -> Option<Reads> {
        let reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
        let receipts = reads
            .receipts
            .iter()
            .map(|((definition, instance), output)| {
                let read = receipt_read(self.declarations, definition, *instance)?;
                (read.output == *output).then_some(read)
            })
            .collect::<Option<Vec<_>>>()?;
        let mut sealed = Vec::new();
        let available = reads
            .available_events
            .keys()
            .chain(reads.available_payloads.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for (definition, instance) in available {
            let read = source_read(self.declarations, &definition, instance)?;
            let events = self.declarations.seal_events(&definition, instance)?;
            let payloads = self.declarations.seal_payloads(&definition, instance)?;
            if reads
                .available_events
                .get(&(definition.clone(), instance))
                .is_some_and(|hash| *hash != expectation_identity(&events))
                || reads
                    .available_payloads
                    .get(&(definition, instance))
                    .is_some_and(|hash| *hash != expectation_identity(&payloads))
            {
                return None;
            }
            sealed.push(read);
        }
        let descriptors = reads.descriptors.values().cloned().collect::<Vec<_>>();
        if !descriptors
            .iter()
            .all(|read| read.matches(self.declarations, table, target))
        {
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
        let pretransfer = if reads.pretransfer {
            let inventory = adapter_inventory(self.declarations, target)?;
            let keys = crate::LayoutKeys::new(original);
            for reference in self.declarations.pretransfer_adapters() {
                let current = original.fn_abis.get(reference.abi.0 as usize)?;
                let expected = inventory
                    .iter()
                    .find(|row| row.adapter == reference.adapter)?;
                if keys.fn_abi(current) != expected.abi {
                    return None;
                }
            }
            Some(inventory)
        } else {
            None
        };
        let environments = reads
            .environments
            .iter()
            .map(|((definition, instance), output)| {
                let read = environment_read(self.declarations, definition, *instance)?;
                (read.output == *output).then_some(read)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Reads {
            environments,
            receipts,
            sealed,
            descriptors,
            unavailable_events: reads.unavailable_events.iter().cloned().collect(),
            unavailable_payloads: reads.unavailable_payloads.iter().cloned().collect(),
            pretransfer,
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
impl Observed<'_> {
    fn observe_descriptor(
        &self,
        source: &NativeType,
        owner: Option<(DefPath, ContentHash)>,
        descriptor: &crate::sealed::SealDescriptor,
        table: &LayoutTable,
    ) -> Result<(), LayoutError> {
        let output = descriptor_identity(descriptor, table).ok_or_else(|| {
            LayoutError::Invalid("invalid independently reconstructed seal descriptor".into())
        })?;
        let key = hirsel_codec::to_text(&(source.clone(), owner.clone()));
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .descriptors
            .insert(
                key,
                DescriptorRead {
                    source: source.clone(),
                    owner,
                    output,
                },
            );
        Ok(())
    }
}
impl OperationDeclarations for Observed<'_> {
    fn receipt_source(&self, definition: &DefPath, instance: ContentHash) -> Option<ReceiptSource> {
        let output = self.declarations.receipt_source(definition, instance);
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .receipts
            .insert((definition.clone(), instance), output.clone());
        output
    }

    fn seal_environments(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<crate::sealed_payload_source::SourceEnvironments> {
        let output = self.declarations.seal_environments(definition, instance);
        self.reads
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .environments
            .insert((definition.clone(), instance), output.clone());
        output
    }

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
        let output = self.declarations.seal_events(definition, instance);
        let mut reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
        if output.is_none() {
            // Observe unavailable, never independently expected-empty. A later
            // Some (even empty) invalidates this actual C output on replay.
            reads
                .unavailable_events
                .insert((definition.clone(), instance));
        } else {
            reads.available_events.insert(
                (definition.clone(), instance),
                expectation_identity(output.as_ref().unwrap()),
            );
        }
        output
    }
    fn seal_payloads(
        &self,
        definition: &DefPath,
        instance: ContentHash,
    ) -> Option<Vec<crate::sealed_payload_source::SourcePayload>> {
        let output = self.declarations.seal_payloads(definition, instance);
        let mut reads = self.reads.lock().unwrap_or_else(|e| e.into_inner());
        if output.is_none() {
            // Observe unavailable, never independently expected-empty. A later
            // Some (even empty) invalidates this actual C output on replay.
            reads
                .unavailable_payloads
                .insert((definition.clone(), instance));
        } else {
            reads.available_payloads.insert(
                (definition.clone(), instance),
                expectation_identity(output.as_ref().unwrap()),
            );
        }
        output
    }
    fn seal_descriptor(
        &self,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<crate::sealed::SealDescriptor, LayoutError> {
        let descriptor = self.declarations.seal_descriptor(source, table, target)?;
        self.observe_descriptor(source, None, &descriptor, table)?;
        Ok(descriptor)
    }
    fn seal_descriptor_for_source(
        &self,
        definition: &DefPath,
        instance: ContentHash,
        source: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<crate::sealed::SealDescriptor, LayoutError> {
        let descriptor = self
            .declarations
            .seal_descriptor_for_source(definition, instance, source, table, target)?;
        self.observe_descriptor(
            source,
            Some((definition.clone(), instance)),
            &descriptor,
            table,
        )?;
        Ok(descriptor)
    }
    fn pretransfer_adapters(&self) -> &[crate::pretransfer::AdapterReference] {
        self.reads
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pretransfer = true;
        self.declarations.pretransfer_adapters()
    }
    fn pretransfer_abi(
        &self,
        reference: &crate::pretransfer::AdapterReference,
        target: &TargetSpec,
    ) -> Option<(FnAbi, LayoutTable)> {
        self.declarations.pretransfer_abi(reference, target)
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

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use crate::invocations::ReferenceBody;
    use crate::sealed_payload_source::*;

    struct Declarations(Option<ReceiptSource>);
    impl OperationDeclarations for Declarations {
        fn receipt_source(&self, _: &DefPath, _: ContentHash) -> Option<ReceiptSource> {
            self.0.clone()
        }
        fn derive(
            &self,
            _: &OperationId,
            _: &[OperationArgument],
            _: &TargetSpec,
        ) -> Result<(FnAbi, LayoutTable), LayoutError> {
            Err(LayoutError::Invalid("no operations".into()))
        }
    }
    fn source() -> ReceiptSource {
        let profile = ClosedStorageProfile {
            schema: ClosedStorageProfile::SCHEMA,
            root: ClosedStorageType {
                ty: NativeType::Tuple(vec![]),
                source_type: ContentHash::of_bytes(b"original Unit"),
            },
            functions: vec![],
        };
        ReceiptSource {
            original: ReferenceBody {
                definition: DefPath::core("receipt-test"),
                instance: ContentHash::of_bytes(b"original base"),
                body: 0,
            },
            instance: ContentHash::of_bytes(b"aware request"),
            source: ContentHash::of_bytes(b"V source"),
            implementation_revision: ContentHash::of_bytes(b"source revision"),
            contract: ReceiptContract::new(
                vec![],
                Some(ReceiptResult {
                    ty: ReceiptType {
                        source_type: profile.root.source_type,
                        profile: profile.content().unwrap(),
                    },
                }),
                vec![],
                vec![],
            )
            .unwrap(),
            profiles: vec![ReceiptProfile {
                port: ReceiptPort::Result,
                profile,
            }],
        }
    }
    #[test]
    fn receipt_reads_replay_every_original_observation_and_complete_port_profile() {
        let source = source();
        source.check_structure().unwrap();
        let declarations = Declarations(Some(source.clone()));
        let observed = Observed::new(&declarations);
        assert_eq!(
            observed.receipt_source(&source.original.definition, source.instance),
            Some(source.clone())
        );
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let table = LayoutTable::new(&target);
        let reads = observed.finish(&table, &table, &target).unwrap();
        let bytes = hirsel_codec::to_text(&reads);
        let reads: Reads = hirsel_codec::from_text(&bytes).unwrap();
        assert!(reads.matches(&declarations, &target));
        assert!(!reads.matches(&Declarations(None), &target));
        for mutation in 0..8 {
            let mut changed = source.clone();
            let other = ContentHash::of_bytes(b"changed source observation");
            match mutation {
                0 => changed.source = other,
                1 => changed.implementation_revision = other,
                2 => changed.original.instance = other,
                3 => changed.original.body += 1,
                4 => changed.original.definition = DefPath::core("other"),
                5 => changed.instance = other,
                6 => changed.profiles.push(ReceiptProfile {
                    port: ReceiptPort::Capture(0),
                    profile: changed.profiles[0].profile.clone(),
                }),
                7 => {
                    changed.profiles[0].profile.root.source_type = other;
                    changed.contract.result.as_mut().unwrap().ty = ReceiptType {
                        source_type: other,
                        profile: changed.profiles[0].profile.content().unwrap(),
                    };
                    changed.contract =
                        ReceiptContract::new(vec![], changed.contract.result, vec![], vec![])
                            .unwrap();
                }
                _ => unreachable!(),
            }
            changed.check_structure().unwrap();
            assert!(
                !reads.matches(&Declarations(Some(changed)), &target),
                "mutation {mutation}"
            );
        }
        let absent = Declarations(None);
        let observed = Observed::new(&absent);
        assert!(
            observed
                .receipt_source(&source.original.definition, source.instance)
                .is_none()
        );
        let reads = observed.finish(&table, &table, &target).unwrap();
        assert!(reads.matches(&absent, &target));
        assert!(!reads.matches(&declarations, &target));
    }
}
