//! Original S4 source linked to the unregistered compiler bodies for H only.
//! Public operation disposition and native code generation remain unavailable.
use crate::rc_support::{compile, dump, manifest, path, target};
use hirsel_layout::{layout::LayoutBuilder, operation::OperationRegistry};
use hirsel_lir::{ir::*, operation::*, sealed::*, stage::Value, validate::Inputs};
use hirsel_rc::{
    instance::InstanceId,
    library::{Cx, Request},
};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{GenericArgument, Type, TypeKind};
use std::collections::BTreeMap;

pub(super) struct Declarations {
    registry: OperationRegistry,
    selected: Vec<(DefPath, ContentHash, SourceEventKind)>,
    root: NativeType,
}
impl OperationDeclarations for Declarations {
    fn derive(
        &self,
        op: &hirsel_core_contract::OperationId,
        args: &[OperationArgument],
        target: &hirsel_lir::TargetSpec,
    ) -> Result<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable), hirsel_lir::layout::LayoutError> {
        self.registry.derive(op, args, target)
    }
    fn pretransfer_adapters(&self) -> &[hirsel_lir::pretransfer::AdapterReference] {
        self.registry.pretransfer_adapters()
    }
    fn pretransfer_abi(
        &self,
        r: &hirsel_lir::pretransfer::AdapterReference,
        t: &hirsel_lir::TargetSpec,
    ) -> Option<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable)> {
        self.registry.pretransfer_abi(r, t)
    }
    fn requires_structural_reference(&self) -> bool {
        self.registry.requires_structural_reference()
    }
    fn structural_access(
        &self,
        n: &NativeType,
        l: &hirsel_lir::LayoutTable,
        t: &hirsel_lir::TargetSpec,
    ) -> Result<hirsel_lir::structural_access::AccessPlan, hirsel_lir::layout::LayoutError> {
        self.registry.structural_access(n, l, t)
    }
    fn requires_invocation_reference(&self) -> bool {
        self.registry.requires_invocation_reference()
    }
    fn invocations(&self, d: &DefPath) -> Option<&[hirsel_lir::invocations::ReferenceInvocation]> {
        self.registry.invocations(d)
    }
    fn invocation_body(&self, d: &DefPath, i: ContentHash) -> Option<u32> {
        self.registry.invocation_body(d, i)
    }
    fn seal_descriptor(
        &self,
        n: &NativeType,
        l: &hirsel_lir::LayoutTable,
        t: &hirsel_lir::TargetSpec,
    ) -> Result<SealDescriptor, hirsel_lir::layout::LayoutError> {
        self.registry.seal_descriptor(n, l, t)
    }
    fn seal_descriptor_for_source(
        &self,
        d: &DefPath,
        i: ContentHash,
        n: &NativeType,
        l: &hirsel_lir::LayoutTable,
        t: &hirsel_lir::TargetSpec,
    ) -> Result<SealDescriptor, hirsel_lir::layout::LayoutError> {
        if !self
            .selected
            .iter()
            .any(|(def, instance, _)| def == d && *instance == i)
            || n != &self.root
        {
            return Err(hirsel_lir::layout::LayoutError::Invalid(
                "unselected seal/read body".into(),
            ));
        }
        self.registry.seal_descriptor(n, l, t)
    }
    fn seal_events(&self, d: &DefPath, i: ContentHash) -> Option<Vec<SourceEvent>> {
        let (_, _, kind) = self
            .selected
            .iter()
            .find(|(def, instance, _)| def == d && *instance == i)?;
        Some(vec![SourceEvent {
            anchor: SourceAnchor {
                body: 0,
                block: 0,
                statement: 0,
                site: SiteRange {
                    source: 0,
                    start: 0,
                    end: 0,
                },
            },
            kind: *kind,
            root_type: self.root.clone(),
        }])
    }
}

pub(super) fn original(
    source: &str,
    root_kind: TypeKind,
    root_native: NativeType,
) -> (
    hirsel_heapcheck::Program<hirsel_lir::stage::Rc>,
    Declarations,
    hirsel_core_contract::Manifest,
) {
    original_with_guard_observer(source, root_kind, root_native, |_, _, _| {})
}

pub(super) fn original_with_guard_observer(
    source: &str,
    root_kind: TypeKind,
    root_native: NativeType,
    mut observe: impl FnMut(
        &[hirsel_lir::LirUnit<hirsel_lir::stage::Value>],
        usize,
        &mut hirsel_lir::LirUnit<hirsel_lir::stage::Placed>,
    ),
) -> (
    hirsel_heapcheck::Program<hirsel_lir::stage::Rc>,
    Declarations,
    hirsel_core_contract::Manifest,
) {
    let program = compile(source);
    let target = target();
    let root = program.types.intern(Type::with_kind(root_kind));
    let result = program.types.intern(Type::with_kind(TypeKind::Primitive(
        hirsel_types::Primitive::U64,
    )));
    let owners =
        hirsel_rc::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut layouts = LayoutBuilder::new(&target, &owners);
    layouts.prepare_native_controls();
    let mut instances = hirsel_rc::instance::expand(&program, &mut layouts).unwrap();
    // The two selected bodies are linked explicitly by this component test;
    // the production BODIES table is unchanged. Selection precedes candidates.
    let mut selected = Vec::new();
    let mut links = BTreeMap::new();
    for (body, args, event) in [
        (
            &hirsel_rc::library::sealed_region::SEAL,
            vec![GenericArgument::Type(root)],
            SourceEventKind::Seal,
        ),
        (
            &hirsel_rc::library::sealed_region::WITH,
            vec![GenericArgument::Type(root), GenericArgument::Type(result)],
            SourceEventKind::BorrowRoot,
        ),
    ] {
        let definition = (body.path)();
        assert!(hirsel_rc::library::body_index(&definition).is_none());
        let request = Request::operation(&program, usize::MAX, &definition, args).unwrap();
        let id = hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
        selected.push((definition.clone(), instances.view(id).key.digest(), event));
        links.insert(definition, (id, body));
    }
    let mut units: Vec<LirUnit<Value>> = vec![];
    let mut statics = vec![];
    let mut next = 0;
    while next < instances.len() {
        let id = InstanceId(next as u32);
        next += 1;
        let definition = instances.view(id).key.definition().clone();
        let mut unit = if let Some((_, body)) = links.get(&definition) {
            (body.build)(&mut Cx::new(&program, &mut layouts, &mut instances, id)).unwrap()
        } else {
            let imported = hirsel_rc::import::import_instance(
                &program,
                &mut instances,
                id,
                None,
                &mut layouts,
            )
            .unwrap();
            for request in imported.required {
                hirsel_rc::library::instantiate(&mut layouts, &mut instances, request).unwrap();
            }
            imported.unit
        };
        for call in &mut unit.tables.calls {
            let Callee::Operation(op) = call.callee else {
                continue;
            };
            let definition = &unit.tables.operations[op.index()].definition;
            let Some((callee, _)) = links.get(definition) else {
                continue;
            };
            let symbol = SymbolId(unit.tables.symbols.len() as u32);
            unit.tables.symbols.push(Symbol {
                kind: SymbolKind::Function,
                definition: definition.clone(),
                instance: Some(instances.view(*callee).key.digest()),
            });
            call.callee = Callee::Direct(symbol);
            call.operation_arguments.clear();
            call.abi = instances.abis[callee.index()];
        }
        statics.extend(
            hirsel_rc::import::instance_statics(&program, &instances, &unit, &mut layouts).unwrap(),
        );
        units.push(unit);
    }
    layouts.header();
    let table = layouts.finish();
    let semantic_owners = instances
        .ids()
        .map(|id| {
            let path = instances.view(id).key.definition().clone();
            let owner = (!instances.roles[id.index()].is_library()).then(|| path.clone());
            (path, owner)
        })
        .collect();
    let mut lowered = hirsel_rc::import::Lowered {
        instances,
        units,
        table,
        statics,
        semantic_owners,
        plans: vec![],
        declined: vec![],
        unit_origins: vec![],
        unit_keys: vec![],
        inline_instances: Default::default(),
        discharged: vec![],
        native_discharge: Default::default(),
    };
    let specialized =
        hirsel_rc::closure_calls::specialize_scoped(&mut lowered, &program, &target, true);
    lowered.unit_origins = specialized.origins;
    lowered.unit_keys = specialized.keys;
    lowered.inline_instances = specialized.inline;
    let declarations = Declarations {
        registry: lowered.operation_registry(&program),
        selected,
        root: root_native,
    };
    hirsel_rc::import::prune_pretransfer(
        &mut lowered.units,
        &lowered.table,
        declarations.pretransfer_adapters(),
    );
    hirsel_rc::arena_credit::summarize(&mut lowered.units, &mut lowered.table);
    let manifest = manifest(&program);
    let inputs = Inputs {
        target: &target,
        layouts: &lowered.table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::ScopedQualification::new(&target, &manifest, &declarations, None)
        .qualify(&lowered.units, &lowered.table)
        .unwrap();
    let entry = lowered
        .units
        .iter()
        .position(|unit| unit.header.definition == path("main"))
        .unwrap();
    let demand = hirsel_rc::seal_demand::Demand::analyze(&lowered.units, &[entry]).unwrap();
    let mut counted = vec![];
    let mut guards = 0;
    for (unit_index, (unit, origin)) in lowered.units.iter().zip(&lowered.unit_origins).enumerate()
    {
        let rejected = hirsel_lir::validate::check_scoped_transport(unit, &inputs);
        assert!(
            rejected.is_empty(),
            "Value {} {rejected:?}\n{}",
            unit.header.definition,
            dump(unit)
        );
        let mut placed = hirsel_rc::placement::place(
            unit.clone(),
            &lowered.table,
            &hirsel_rc::placement::Options::for_instance(true, &lowered.instances, *origin),
        )
        .unwrap();
        demand.apply(unit_index, &mut placed).unwrap();
        observe(&lowered.units, entry, &mut placed);
        guards += placed
            .tables
            .regions
            .iter()
            .filter(|r| r.seal_demand)
            .count();
        let rc = hirsel_rc::insert::insert(
            placed,
            &lowered.table,
            &hirsel_rc::insert::Proofs::for_instance(&lowered.instances, *origin),
        )
        .unwrap();
        let rejected = hirsel_lir::validate::check_scoped_transport(&rc, &inputs);
        assert!(
            rejected.is_empty(),
            "Rc {} {rejected:?}\n{}",
            rc.header.definition,
            dump(&rc)
        );
        counted.push(rc);
    }
    assert_eq!(
        guards, 1,
        "only the source arena supplying the seal gets an extension"
    );
    let h = hirsel_heapcheck::Program {
        target,
        layouts: lowered.table,
        units: counted,
        statics: lowered.statics,
        invocation_bodies: declarations.registry.invocation_bodies.clone(),
        invocations: declarations.registry.invocations.clone(),
        pretransfer_adapters: declarations.registry.pretransfer_adapters.clone(),
    };
    (h, declarations, manifest)
}

fn execute_original(source: &str, root_kind: TypeKind, root_native: NativeType) {
    let (h, _, _) = original(source, root_kind, root_native);
    for full in [false, true] {
        let config = hirsel_heapcheck::Config {
            full_recount_every_step: full,
            context_handles: vec![(1, "test".into())],
            ..Default::default()
        };
        let run = hirsel_heapcheck::run(
            &h,
            &path("main"),
            &[hirsel_heapcheck::Arg::Handle(1)],
            &mut hirsel_heapcheck::NoOperations,
            &config,
        );
        let audit = run
            .result
            .unwrap_or_else(|v| panic!("original source H full={full}: {v:?}"));
        assert!(
            audit.observation.failures.is_empty(),
            "{:?}",
            audit.observation
        );
        assert!(!audit.observation.blocked);
        assert_eq!(run.counters.allocations, run.counters.frees);
    }
}

#[test]
fn original_s4_unit_source_executes_generated_seal_and_read_bodies_in_h() {
    execute_original(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        TypeKind::Tuple(vec![].into()),
        NativeType::Tuple(vec![]),
    );
}
#[test]
fn original_s4_scalar_source_executes_generated_seal_and_read_bodies_in_h() {
    execute_original(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
    );
}

#[test]
fn ordinary_arena_many_literal_has_the_declared_once_callback_entry() {
    let (program, lowered) = crate::rc_support::lower_valid(
        "pub fn main()->u64 {let value=arena.bounded(0u64,fn(a:Arena)->u64 {1u64}); match take value {Ok(n)=>n, Error(e)=>0u64}}",
    );
    let target = target();
    let manifest = manifest(&program);
    let declarations = lowered.operation_registry(&program);
    hirsel_lir::validate::ScopedQualification::new(&target, &manifest, &declarations, None)
        .qualify(&lowered.units, &lowered.table)
        .expect("the ordinary Many literal must reach the declared Once callback entry");
}
