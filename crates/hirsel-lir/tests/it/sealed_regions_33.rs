//! Descriptor controls; typed transition/CFG adversaries are separate.
use crate::support;
use hirsel_layout::layout::{LayoutBuilder, Owners};
use hirsel_lir::logical_storage::LogicalBytes;
use hirsel_lir::operation::{NativeType, OperationArgument, OperationDeclarations};
use hirsel_lir::{LayoutTable, TargetSpec};
use hirsel_types::*;

fn fixture() -> (
    hirsel_layout::operation::OperationRegistry,
    LayoutTable,
    hirsel_lir::sealed::SealDescriptor,
) {
    let types = support::session();
    let bytes = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::Bytes)));
    let list = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::List,
        arguments: vec![GenericArgument::Type(bytes)].into(),
    }));
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let owners = Owners::default();
    let mut builder = LayoutBuilder::new(&target, &owners);
    builder.layout(&types, list).unwrap();
    let table = builder.finish();
    let arguments =
        hirsel_layout::operation::collect_arguments(&types, &[GenericArgument::Type(list)])
            .unwrap();
    let [OperationArgument::Type(root)] = arguments.as_slice() else {
        unreachable!()
    };
    let registry = support::registry();
    let descriptor = registry.seal_descriptor(root, &table, &target).unwrap();
    (registry, table, descriptor)
}

use hirsel_core_contract::{OperationId, Passing};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::{LayoutId, ScalarKind};
use hirsel_lir::sealed::{SourceEvent, SourceEventKind};
use hirsel_lir::stage::Rc as TestStage;

struct Declarations {
    registry: hirsel_layout::operation::OperationRegistry,
    expected: Option<Vec<SourceEvent>>,
    payloads: Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>>,
}
impl OperationDeclarations for Declarations {
    fn derive(
        &self,
        operation: &OperationId,
        arguments: &[OperationArgument],
        target: &TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(operation, arguments, target)
    }
    fn seal_descriptor(
        &self,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_descriptor_for_source(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
        root: &NativeType,
        table: &LayoutTable,
        target: &TargetSpec,
    ) -> Result<hirsel_lir::sealed::SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(root, table, target)
    }
    fn seal_payloads(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
    ) -> Option<Vec<hirsel_lir::sealed_payload_source::SourcePayload>> {
        self.payloads.clone()
    }
    fn seal_events(
        &self,
        _: &hirsel_syntax::DefPath,
        _: hirsel_syntax::ContentHash,
    ) -> Option<Vec<SourceEvent>> {
        self.expected.clone()
    }
}

/// Original source correspondence is authored independently of the candidate's
/// instructions. Mutations never recompute it from the mutated LIR.
fn transition_fixture() -> (support::Lir<TestStage>, Declarations, ValueId, ValueId) {
    transition_fixture_with_root(false)
}

fn transition_fixture_with_root(
    heap: bool,
) -> (support::Lir<TestStage>, Declarations, ValueId, ValueId) {
    let types = support::session();
    let scalar = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U64)));
    let word = if heap {
        types.intern(Type::with_kind(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments: vec![GenericArgument::Type(scalar)].into(),
        }))
    } else {
        scalar
    };
    let region = types.intern(Type::with_kind(TypeKind::Builtin {
        kind: BuiltinType::Region,
        arguments: vec![GenericArgument::Type(word)].into(),
    }));
    let owners = Owners::default();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, &owners);
    let root_layout = builder.layout(&types, word).unwrap();
    let owner_layout = builder.layout(&types, region).unwrap();
    let extra = builder.finish();
    let mut t = support::Lir::<TestStage>::new(&[], None);
    let offset = t.table.layouts.len() as u32;
    for mut row in extra.layouts {
        row.for_each_reference_mut(&mut |id| id.0 += offset);
        t.table.layouts.push(row);
    }
    let root_layout = LayoutId(root_layout.0 + offset);
    let owner_layout = LayoutId(owner_layout.0 + offset);
    let abi = t.abi(
        &[
            (Passing::Take, support::l::ARENA),
            (Passing::Take, root_layout),
        ],
        Some(owner_layout),
    );
    t.builder.unit.header.abi = abi;
    t.builder.unit.header.origin = UnitOrigin::Library;
    let grant = t.plain(support::l::ARENA);
    let root = t.value_as(
        if heap {
            ValueRepr::Block
        } else {
            ValueRepr::Scalar(ScalarKind::U64)
        },
        root_layout,
        if heap { Holding::Owned } else { Holding::Plain },
    );
    t.param(grant);
    t.param(root);
    let owner = t.value_as(ValueRepr::Handle, owner_layout, Holding::Owned);
    let args = hirsel_layout::operation::collect_arguments(&types, &[GenericArgument::Type(word)])
        .unwrap();
    let [OperationArgument::Type(native)] = args.as_slice() else {
        unreachable!()
    };
    let native = native.clone();
    let descriptor = t
        .operations
        .seal_descriptor(&native, &t.table, &target)
        .unwrap();
    t.builder.unit.tables.seal_descriptors.push(descriptor);
    let failure = if heap {
        let cleanup = t.releasing(&[root]);
        t.failure(cleanup)
    } else {
        t.fails()
    };
    // An ordinary bit read of the token slot is a new SSA name of the
    // same grant. Consumption must follow that origin, including on CFG edges.
    let slot = t.slot(support::l::ARENA, SlotKind::Parameter(0), support::ROOT);
    let abort = if heap {
        vec![support::Undo::Release(root)]
    } else {
        vec![]
    };
    let address = t.at(support::ENTRY, slot, &[], &abort);
    let alias = t.read(support::ENTRY, address, grant, ReadMode::Bits, None);
    if heap {
        // Keep the ordinary cleanup independently reachable when a mutation
        // redirects only the seal's abort edge past it.
        let condition = t.boolean(support::ENTRY);
        let cleanup_edge = t.builder.unit.tables.failures[failure.index()].cleanup;
        let cleanup = t.builder.unit.tables.edges[cleanup_edge.index()].target;
        let check_failure = t.failure(cleanup);
        t.builder.push(
            support::ENTRY,
            Inst::Check {
                condition,
                failure: check_failure,
            },
        );
    }
    t.builder.push(
        support::ENTRY,
        Inst::SealArena {
            result: owner,
            grant: alias,
            root,
            shape: SealDescriptorId(0),
            failure,
            origin: SourceAnchorId(0),
        },
    );
    t.builder.ret(support::ENTRY, Some(owner));
    let declarations = Declarations {
        registry: t.operations.clone(),
        payloads: None,
        expected: Some(vec![SourceEvent {
            anchor: t.builder.unit.tables.source_anchors[0].clone(),
            kind: SourceEventKind::Seal,
            root_type: native,
        }]),
    };
    (t, declarations, alias, root)
}

fn checked(
    t: support::Lir<TestStage>,
    declarations: &Declarations,
) -> Vec<hirsel_lir::validate::Violation> {
    let (unit, table, manifest) = t.finish();
    checked_raw(unit, &table, &manifest, declarations)
}

fn checked_raw(
    unit: hirsel_lir::LirUnit<TestStage>,
    table: &LayoutTable,
    manifest: &hirsel_core_contract::Manifest,
    declarations: &Declarations,
) -> Vec<hirsel_lir::validate::Violation> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: declarations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::validate(unit, &inputs)
        .err()
        .map_or(Vec::new(), |rejected| rejected.violations)
}

#[test]
fn public_validation_accepts_a_declared_typed_seal_transition() {
    let (t, d, _, _) = transition_fixture();
    assert_eq!(checked(t, &d), []);
}

#[test]
fn source_correspondence_survives_candidate_event_deletion() {
    let (_, d, _, _) = transition_fixture();
    // A fully well-formed ordinary unit represents deletion of both producer
    // and sink, including every candidate descriptor. Its current-source
    // expectation still names the original seal operation independently.
    let mut missing = support::Lir::<TestStage>::new(&[], None);
    missing.builder.ret(support::ENTRY, None);
    let (unit, table, manifest) = missing.finish();
    let empty = Declarations {
        registry: d.registry.clone(),
        payloads: None,
        expected: Some(vec![]),
    };
    assert!(checked_raw(unit.clone(), &table, &manifest, &empty).is_empty());
    let errors = checked_raw(unit, &table, &manifest, &d);
    assert_eq!(errors.len(), 1, "{errors:?}");
    sealed_rejection(&errors, "source event correspondence");
    let (t, mut d, _, _) = transition_fixture();
    d.expected = None;
    assert!(!checked(t, &d).is_empty());
}

#[test]
fn public_validation_rejects_descriptor_charge_and_edge_mutations() {
    for mutation in 0..3 {
        let (mut t, d, _, _) = transition_fixture();
        let descriptor = &mut t.builder.unit.tables.seal_descriptors[0];
        match mutation {
            0 => descriptor.logical.facts[0].bytes = LogicalBytes::Bytes(9),
            1 => {
                descriptor.access.pop();
            }
            _ => {
                descriptor.root_type = NativeType::Primitive(hirsel_lir::operation::Primitive::I64)
            }
        }
        assert!(!checked(t, &d).is_empty(), "mutation {mutation}");
    }
}

#[test]
fn region_drop_cannot_be_laundered_into_an_ordinary_rc_release() {
    let (mut t, d, _, _) = transition_fixture();
    let root = t.builder.unit.tables.seal_descriptors[0].root_layout;
    let owner = t
        .table
        .layouts
        .iter_mut()
        .find(|row| row.kind == (hirsel_lir::layout_types::LayoutKind::SealedRegion { root }))
        .unwrap();
    owner.drop = vec![hirsel_lir::layout_types::DropStep::Release { offset: 0 }];
    assert!(!checked(t, &d).is_empty());
}

fn sealed_rejection(errors: &[hirsel_lir::validate::Violation], text: &str) {
    assert!(
        errors.iter().any(|e| matches!(&e.kind,
        hirsel_lir::validate::ViolationKind::Sealed(reason) if reason.contains(text))),
        "expected sealed rejection containing {text:?}, got {errors:?}"
    );
}

#[test]
fn ordinary_borrow_entry_cannot_forge_a_consumable_grant() {
    let (t, d, _, _) = transition_fixture();
    let (unit, mut table, manifest) = t.finish();
    assert_eq!(checked_raw(unit.clone(), &table, &manifest, &d), []);
    let parameter = &mut table.fn_abis[unit.header.abi.0 as usize].parameters[0];
    parameter.convention = Passing::Borrow;
    parameter.passing = hirsel_lir::abi_types::ParameterPassing::ReadOnlyAddress;
    sealed_rejection(&checked_raw(unit, &table, &manifest, &d), "borrow-only");
}

#[test]
fn duplicate_publication_through_a_plain_token_alias_is_rejected() {
    let (control, declarations, _, _) = transition_fixture();
    assert_eq!(checked(control, &declarations), []);
    let (mut t, mut d, grant, root) = transition_fixture();
    let owner = t
        .builder
        .unit
        .values
        .iter()
        .enumerate()
        .find(|(_, v)| v.holding == Holding::Owned)
        .map(|(i, _)| ValueId(i as u32))
        .unwrap();
    t.release(support::ENTRY, owner, &[]);
    let other = t.value_as(
        ValueRepr::Handle,
        t.builder.unit.types[t.builder.unit.values[owner.index()].ty.index()].layout,
        Holding::Owned,
    );
    let failure = t.fails();
    t.builder.push(
        support::ENTRY,
        Inst::SealArena {
            result: other,
            grant,
            root,
            shape: SealDescriptorId(0),
            failure,
            origin: SourceAnchorId(0),
        },
    );
    t.builder.ret(support::ENTRY, Some(other));
    let event = d.expected.as_ref().unwrap()[0].clone();
    d.expected.as_mut().unwrap().push(event);
    sealed_rejection(&checked(t, &d), "consumed");
}

#[test]
fn seal_failure_retains_the_owned_heap_root_for_compiler_cleanup() {
    let (t, d, _, root) = transition_fixture_with_root(true);
    let (unit, table, manifest) = t.finish();
    assert_eq!(checked_raw(unit.clone(), &table, &manifest, &d), []);
    let mut mutant = unit;
    // Redirect the seal abort edge past root cleanup. The old cleanup
    // remains well-formed; no dangling edge masks the ownership violation.
    let failure = mutant
        .instructions
        .iter()
        .find_map(|inst| match *inst {
            Inst::SealArena { failure, .. } => Some(failure),
            _ => None,
        })
        .unwrap();
    let bare_propagation = mutant
        .blocks
        .iter()
        .enumerate()
        .find_map(|(i, block)| {
            (block.kind == BlockKind::FailureCleanup
                && block.instructions.len == 0
                && matches!(mutant.terminators[i], Terminator::Propagate))
            .then_some(BlockId(i as u32))
        })
        .unwrap();
    let edge = mutant.tables.failures[failure.index()].cleanup;
    mutant.tables.edges[edge.index()].target = bare_propagation;
    let errors = checked_raw(mutant, &table, &manifest, &d);
    assert!(
        errors.iter().any(|e| matches!(e.kind,
        hirsel_lir::validate::ViolationKind::Leak(v) if v == root)),
        "{errors:?}"
    );
}

#[test]
fn branch_local_seal_consumption_survives_a_token_phi() {
    for mutant in [false, true] {
        let (mut t, mut d, grant, root) = transition_fixture();
        let owner = ValueId(2);
        let condition = t.boolean(support::ENTRY);
        let left = t.builder.block(BlockKind::Body, &[]);
        let right = t.builder.block(BlockKind::Body, &[]);
        let joined = t.plain(support::l::ARENA);
        let join = t.builder.block(BlockKind::Body, &[joined]);
        let yes = t.builder.edge(left, &[]);
        let no = t.builder.edge(right, &[]);
        t.builder
            .term(support::ENTRY, Terminator::Branch { condition, yes, no });
        t.builder.jump(left, join, &[grant]);
        t.builder.jump(right, join, &[grant]);
        if mutant {
            let another = t.value_as(
                ValueRepr::Handle,
                t.builder.unit.types[t.builder.unit.values[owner.index()].ty.index()].layout,
                Holding::Owned,
            );
            // First owner is dead on both arms before testing second publication.
            t.release(join, owner, &[]);
            let failure = t.fails();
            t.builder.push(
                join,
                Inst::SealArena {
                    result: another,
                    grant: joined,
                    root,
                    shape: SealDescriptorId(0),
                    failure,
                    origin: SourceAnchorId(0),
                },
            );
            t.builder.ret(join, Some(another));
            let event = d.expected.as_ref().unwrap()[0].clone();
            d.expected.as_mut().unwrap().push(event);
        } else {
            t.builder.ret(join, Some(owner));
        }
        let errors = checked(t, &d);
        if mutant {
            sealed_rejection(&errors, "consumed");
        } else {
            assert_eq!(errors, []);
        }
    }
}

#[derive(Clone, Debug, Default, Hash)]
struct LegacyTables {
    pub calls: Vec<CallSite>,
    pub constructs: Vec<ConstructSite>,
    pub closures: Vec<ClosureSite>,
    pub constants: Vec<ConstantData>,
    pub symbols: Vec<Symbol>,
    pub operations: Vec<OperationId>,
    pub loans: Vec<LoanInfo>,
    pub failures: Vec<FailureEdge>,
    pub edges: Vec<Edge>,
    pub masks: Vec<FieldMask>,
    pub regions: Vec<RegionInstance>,
    pub arenas: Vec<ExternalInstance>,
    pub shared: Vec<ExternalInstance>,
    pub task_domains: u32,
    pub source_anchors: Vec<SourceAnchor>,
    pub cleanup_anchors: Vec<CleanupAnchor>,
    /// Drop flags (LIR schema revision 8).
    pub init_flags: Vec<InitFlag>,
    /// SER-2 codec plans (LIR schema revision 29).
    pub codecs: Vec<hirsel_lir::codec::CodecPlan>,
    /// MEM-3 logical types and complete physical access, independently validated.
    pub structural: Vec<hirsel_lir::structural_access::AccessPlan>,
}
hirsel_codec::snapshot_struct!(LegacyTables {
    calls,
    constructs,
    closures,
    constants,
    symbols,
    operations,
    loans,
    failures,
    edges,
    masks,
    regions,
    arenas,
    shared,
    task_domains,
    source_anchors,
    cleanup_anchors,
    init_flags,
    codecs,
    structural
});

#[test]
fn absent_sealed_tables_preserve_ordinary_side_table_bytes_and_hash() {
    use std::hash::{Hash, Hasher};
    let old = LegacyTables {
        task_domains: 3,
        ..Default::default()
    };
    let value = SideTables {
        task_domains: 3,
        ..Default::default()
    };
    let old_text = hirsel_codec::to_text(&old).replace("LegacyTables", "SideTables");
    assert_eq!(hirsel_codec::to_text(&value), old_text);
    assert_eq!(
        hirsel_codec::from_text::<SideTables>(&old_text).unwrap(),
        value
    );
    let hash = |input: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
        let mut state = std::collections::hash_map::DefaultHasher::new();
        input(&mut state);
        state.finish()
    };
    assert_eq!(hash(&|s| old.hash(s)), hash(&|s| value.hash(s)));
    let (_, _, descriptor) = fixture();
    let demanded = SideTables {
        seal_descriptors: vec![descriptor],
        ..value.clone()
    };
    assert_ne!(hash(&|s| value.hash(s)), hash(&|s| demanded.hash(s)));
    let text = hirsel_codec::to_text(&demanded);
    assert_eq!(
        hirsel_codec::from_text::<SideTables>(&text).unwrap(),
        demanded
    );
}

#[test]
fn sealed_descriptor_objects_erase_and_rebind_complete_physical_projection() {
    let (t, _, _, _) = transition_fixture_with_root(true);
    let (unit, _, _) = t.finish();
    let descriptor = &unit.tables.seal_descriptors[0];
    let (erased, layouts, abis) = hirsel_lir::objects::erased(&unit);
    assert!(descriptor.layouts().all(|id| layouts.contains(&id)));
    assert!(layouts.len() > 1);
    let restored = hirsel_lir::objects::rebound(erased.clone(), &layouts, &abis).unwrap();
    assert_eq!(
        restored.tables.seal_descriptors,
        unit.tables.seal_descriptors
    );
    let moved: Vec<_> = layouts.iter().map(|id| LayoutId(id.0 + 100)).collect();
    let rebound = hirsel_lir::objects::rebound(erased, &moved, &abis).unwrap();
    let before: Vec<_> = descriptor.layouts().collect();
    let after: Vec<_> = rebound.tables.seal_descriptors[0].layouts().collect();
    assert_eq!(
        after,
        before
            .iter()
            .map(|id| LayoutId(id.0 + 100))
            .collect::<Vec<_>>()
    );
}

fn payload_fixture() -> (support::Lir<TestStage>, Declarations, ValueId) {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _, root) = transition_fixture();
    let root_type = t.builder.unit.tables.seal_descriptors[0].root_type.clone();
    let recipe = PayloadRecipe {
        source: t.builder.unit.tables.source_anchors[0].clone(),
        operation: PayloadOperation::Transfer { previous: 1 },
        inputs: vec![PayloadOperand {
            place: 1,
            ty: root_type.clone(),
            producer: PayloadProducer::Parameter(1),
            capture_slot: None,
        }],
    };
    declarations.payloads = Some(vec![SourcePayload {
        root_type,
        recipe: recipe.clone(),
    }]);
    t.builder.unit.tables.seal_payloads.push(PayloadTable {
        descriptor: SealDescriptorId(0),
        bindings: vec![PayloadBinding {
            recipe,
            values: vec![root],
        }],
    });
    (t, declarations, root)
}

#[test]
fn semantic_payload_33_valid_source_recipe_binds_the_actual_operand_and_roundtrips() {
    let (t, declarations, _) = payload_fixture();
    let table = &t.builder.unit.tables.seal_payloads[0];
    let bytes = hirsel_codec::to_text(table);
    assert_eq!(
        hirsel_codec::from_text::<hirsel_lir::sealed_payload::PayloadTable>(&bytes).unwrap(),
        *table
    );
    assert!(checked(t, &declarations).is_empty());
}

#[test]
fn semantic_payload_33_forged_recipe_is_permanently_refused_by_public_c() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, declarations, _) = payload_fixture();
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .operation = PayloadOperation::Allocate {
        destination: PayloadDestination::Heap,
    };
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_operand_substitution_cannot_launder_source_lineage() {
    let (mut t, declarations, _) = payload_fixture();
    let grant = t.builder.unit.parameters[0];
    t.builder.unit.tables.seal_payloads[0].bindings[0].values[0] = grant;
    sealed_rejection(
        &checked(t, &declarations),
        "operand producer lineage differs",
    );
}

#[test]
fn semantic_payload_33_missing_recipe_cannot_delete_its_expected_source_obligation() {
    let (mut t, declarations, _) = payload_fixture();
    t.builder.unit.tables.seal_payloads.clear();
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_unavailable_source_never_authorizes_candidate_recipes() {
    let (t, mut declarations, _) = payload_fixture();
    declarations.payloads = None;
    sealed_rejection(
        &checked(t, &declarations),
        "recipes differ from independent source",
    );
}

#[test]
fn semantic_payload_33_agreeing_false_recipe_claims_do_not_replace_ssa_proof() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _) = payload_fixture();
    // Even matching recipe/source declarations cannot assert that parameter 1
    // was produced by parameter 0. C follows the candidate value independently.
    declarations.payloads.as_mut().unwrap()[0].recipe.inputs[0].producer =
        PayloadProducer::Parameter(0);
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .inputs[0]
        .producer = PayloadProducer::Parameter(0);
    sealed_rejection(
        &checked(t, &declarations),
        "operand producer lineage differs",
    );
}

#[test]
fn semantic_payload_33_zero_sized_parameter_keeps_original_source_ordinal() {
    use hirsel_lir::sealed_payload::*;
    let (mut t, mut declarations, _) = payload_fixture();
    let old = t.builder.unit.header.abi.0 as usize;
    let owner = t.table.fn_abis[old].result.as_ref().unwrap().layout;
    let root = t.builder.unit.tables.seal_descriptors[0].root_layout;
    let unit = t.plain(support::l::UNIT);
    t.builder.unit.parameters.insert(1, unit);
    let abi = t.abi(
        &[
            (Passing::Take, support::l::ARENA),
            (Passing::Take, support::l::UNIT),
            (Passing::Take, root),
        ],
        Some(owner),
    );
    t.builder.unit.header.abi = abi;
    assert!(
        t.table.fn_abis[abi.0 as usize].parameters[1]
            .slots
            .is_empty()
    );
    declarations.payloads.as_mut().unwrap()[0].recipe.inputs[0].producer =
        PayloadProducer::Parameter(2);
    t.builder.unit.tables.seal_payloads[0].bindings[0]
        .recipe
        .inputs[0]
        .producer = PayloadProducer::Parameter(2);
    let errors = checked(t, &declarations);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn receipt_local_33_public_c_refuses_producer_before_complete_custody_proof() {
    use hirsel_lir::layout_types::*;
    let (t, declarations, root) = payload_fixture();
    let (mut unit, mut table, manifest) = t.finish();
    let baseline = checked_raw(unit.clone(), &table, &manifest, &declarations);
    assert!(baseline.is_empty(), "{baseline:#?}");
    let cleanup = CleanupAnchorId(unit.tables.cleanup_anchors.len() as u32);
    unit.tables.cleanup_anchors.push(CleanupAnchor {
        body: 0,
        cleanup: INSTRUCTION_DROP,
        action: 0,
        site: unit.tables.source_anchors[0].site,
    });
    let inputs = PoolRange::push(
        &mut unit.pools.receipt_inputs,
        [ReceiptInput {
            original: root,
            receipt: None,
        }],
    );
    let mut row = table.layouts[0].clone();
    row.kind = LayoutKind::PayloadReceipt {
        root: unit.tables.seal_descriptors[0].root_type.clone(),
    };
    row.size = SizeSpec::Fixed(8);
    row.align = 8;
    row.fields.clear();
    row.tag = None;
    row.scan.clear();
    row.drop = vec![DropStep::ReleaseReceipt { offset: 0 }];
    row.copy = CopyRecipe::NotCopyable;
    row.finalizer = None;
    row.abi = AbiInputs {
        scalars: vec![(0, ScalarKind::Pointer)],
        unaligned: false,
    };
    let layout = LayoutId(table.layouts.len() as u32);
    table.layouts.push(row);
    let ty = LirTypeId(unit.types.len() as u32);
    unit.types.push(LirType {
        repr: ValueRepr::PayloadReceipt,
        layout,
    });
    let result = ValueId(unit.values.len() as u32);
    unit.values.push(ValueInfo {
        ty,
        holding: Holding::Owned,
    });
    unit.instructions.insert(
        1,
        Inst::PayloadReceipt {
            result,
            value: root,
            shape: SealDescriptorId(0),
            binding: PayloadBindingId(0),
            inputs,
        },
    );
    // Failure edges are owned by their operation, even for mechanical metadata.
    let mut release_failure = unit.tables.failures[0].clone();
    let release_cleanup = unit.tables.edges[release_failure.cleanup.index()].clone();
    release_failure.cleanup = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(release_cleanup);
    let failure = FailureId(unit.tables.failures.len() as u32);
    unit.tables.failures.push(release_failure);
    unit.instructions.insert(
        2,
        Inst::Release {
            value: result,
            layout,
            live: MaskRef::All,
            anchor: cleanup,
            failure,
        },
    );
    for (index, block) in unit.blocks.iter_mut().enumerate() {
        if index == support::ENTRY.index() {
            block.instructions.len += 2;
        } else {
            block.instructions.start += 2;
        }
    }
    let errors = checked_raw(unit.clone(), &table, &manifest, &declarations);
    assert!(
        errors.iter().any(
            |e| matches!(&e.kind, hirsel_lir::validate::ViolationKind::Schema(text)
        if text == "private receipt storage requires independent source and custody validation")
        ),
        "{errors:#?}"
    );
    let mut wrong = unit.clone();
    if let Inst::PayloadReceipt { binding, .. } = &mut wrong.instructions[1] {
        *binding = PayloadBindingId(1);
    }
    assert!(wrong.check_identity(&table).is_err());
    let mut wrong = unit;
    if let Inst::PayloadReceipt { inputs, .. } = &mut wrong.instructions[1] {
        *inputs = PoolRange::new(0, 2);
    }
    assert!(wrong.check_identity(&table).is_err());
}
