//! P2 SSA import ("CFG obligations and mandatory loops"): one
//! instance view of validated TIR becomes one `LirUnit<Value>`.
//!
//! Mutable TIR locals become explicit slots; the immutable value a slot holds
//! after each write is a fresh version name, reads name the reaching version,
//! and merge blocks join versions through block parameters. Operands are read
//! in TIR source order and keep their declaration ordinals (`CallArgument.
//! parameter`, `ConstructField.field`); nothing rematches labels.
//!
//! Every potentially failing operation gets an explicit failure successor. A
//! failure successor is the TIR instruction's cleanup chain expanded into
//! `FailureCleanup` blocks, specialized by the abstract slot and loan state at
//! that point: a `drop?` of a slot that is definitely empty there is a no-op and
//! is not emitted, a `drop?` of a slot that is definitely full loses its `?`.
//! Normal-path cleanup (`RunCleanup`, `Return`) runs in `Cleanup` blocks, and a
//! drop that fails there continues the rest of its list, then the enclosing
//! failure cleanup, so the list always finishes (`RunCleanup` may fail and still
//! finishes). `Return` extracts its owned result before cleanup and a cleanup
//! failure disposes of that result before propagating. Owned values pending
//! when an operation fails (an earlier operand's copy, an extracted result) are
//! passed to the failure block and disposed through a scratch slot at a
//! synthetic cleanup anchor.
//!
//! Anchors: a TIR cleanup action anchors at `(body, cleanup, ordinal)`; the
//! synthetic drops anchor at reserved cleanup ordinals (`INSTRUCTION_DROP`,
//! `LOOP_RELEASE`, `SCRATCH_DISPOSAL`), so no anchor names a TIR action it does
//! not perform. Allocation-class failures (construction, copies, closures)
//! carry no language rule: allocation exhaustion is environmental.
//!
//! PS-13 check removal (`lower_discharging`): a licensed TIR `Check` imports
//! without its `Check` and failure edge (its condition is still read), and a
//! licensed SEM-1 integer operator as a wrapping `IntArith`. Projection paths
//! mark only their licensed bounds/tag guards unchecked. Plans name original
//! TIR `(body, block, statement)` sites and canonical places.
use crate::PassError;
use crate::instance::{Actuals, InstanceError, InstanceId, Instances, Program, substitute};
use crate::library::builder::Layouts as _;
use crate::normalize::{Declined, Facts, Home, LoopPlan, LoopSite, NormalizeError};
use hirsel_core_contract::{OperationId, Passing};
use hirsel_layout::layout::LayoutBuilder;
use hirsel_lir::abi_types::{CallableShape, FnAbiId};
use hirsel_lir::discharge::{Discharge, Discharged, Form};
use hirsel_lir::effect::Effects;
use hirsel_lir::ir::{
    AddressBase, AddressId, AddressInfo, ArgumentValue, BinaryOp, Block, BlockId, BlockKind,
    CallArgument, CallId, CallSite, Callee, CaptureValue, CleanupAnchor, CleanupAnchorId,
    ClosureCode, ClosureSite, CompareOp, ConstantData, ConstantId, ConstantValue, ConstructSite,
    ConstructorKind, DestinationClass, Edge, EdgeId, FailureEdge, FailureId, FieldValue, Holding,
    Inst, LirType, LirTypeId, LirUnit, LoanId, LoanInfo, OperationRef, OriginKind, PoolRange,
    ProjectionStep, ReadMode, ScopeId, ScopeInfo, SiteRange, SlotId, SlotInfo, SlotKind,
    SourceAnchor, SourceAnchorId, SwitchArm, Symbol, SymbolId, SymbolKind, Terminator, UnaryOp,
    UnitHeader, UnitOrigin, ValueId, ValueInfo, ValueRepr,
};
use hirsel_lir::layout_types::{
    CopyRecipe, LayoutId, LayoutKind, LayoutTable, SizeSpec, TargetSpec,
};
use hirsel_lir::persistent::{Element, PVec};
use hirsel_lir::stage::{StageKind, Value};
use hirsel_span::Span;
use hirsel_syntax::{ContentHash, DefId, DefPath};
use hirsel_tir as tir;
use hirsel_types::{GenericArgument, SignatureKind, TypeId, TypeInterner, UseKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub use hirsel_lir::ir::{INSTRUCTION_DROP, LOOP_RELEASE, SCRATCH_DISPOSAL, STATE_RELEASE};

mod build;
mod byte_views;
mod calls;
mod cleanup;
mod encoding_views;
mod flat_builder;
mod flat_field;
mod flat_views;
mod flow;
mod holes;
mod inline;
mod instructions;
mod loops;
mod materialize;
mod patterns;
mod places;
mod slice_views;

type R<T> = Result<T, PassError>;

fn unsupported<T>(what: impl Into<String>) -> R<T> {
    Err(PassError::Unsupported(what.into()))
}
fn malformed<T>(what: impl Into<String>) -> R<T> {
    Err(PassError::Malformed(what.into()))
}

/// A lowered program: its instances, their loop plans and declined
/// published sites, one Value unit per instance, and the layout table the
/// units name.
pub struct Lowered {
    pub instances: Instances,
    pub plans: Vec<Option<LoopPlan>>,
    pub declined: Vec<(InstanceId, Declined)>,
    pub units: Vec<LirUnit<Value>>,
    pub table: LayoutTable,
    /// The static data of every constant a unit reads (`ReadStatic`).
    pub statics: Vec<hirsel_lir::statics::StaticData>,
    /// Original signature/placement proof owner of every unit. Specialization
    /// appends Value units without adding source instances: a unit index must
    /// be mapped through this table before indexing `instances`.
    pub unit_origins: Vec<InstanceId>,
    /// Each unit's own instance key, whose digest is its instance.
    pub unit_keys: Vec<hirsel_layout::instance::InstanceKey>,
    /// Explicit semantic owner of each native definition. None denotes a planned
    /// library body covered by the shared runtime node in ProgramRevision.
    pub semantic_owners: BTreeMap<DefPath, Option<DefPath>>,
    /// Source bodies available for inlining in every calling partition.
    pub inline_instances: std::collections::BTreeSet<ContentHash>,
    /// Per unit imported under a nonempty plan: the instance, the
    /// definition, the reference import that kept every check, and the unit
    /// as discharged, for `validate_discharge`. The unit then continues
    /// through `units` like any other.
    pub discharged: Vec<DischargedUnit>,
    /// Accepted source sites keyed by native definition, including derived forms.
    pub native_discharge: Discharge,
}

/// One unit imported twice, as `Lowered::discharged` describes.
pub struct DischargedUnit {
    pub instance: InstanceId,
    pub definition: DefPath,
    pub reference: LirUnit<Value>,
    pub discharged: LirUnit<Value>,
}

#[derive(Clone, Debug)]
pub enum LowerError {
    Instance(InstanceError),
    Normalize(InstanceId, NormalizeError),
    Import(InstanceId, PassError),
    /// A library body's builder failed.
    Library(InstanceId, PassError),
    /// A constant a unit reads has no static encoding (or no value).
    Static(DefPath, crate::statics::StaticError),
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Instance(error) => write!(f, "instance: {error:?}"),
            Self::Normalize(id, error) => write!(f, "normalize instance {}: {error:?}", id.0),
            Self::Import(id, error) => write!(f, "import instance {}: {error}", id.0),
            Self::Library(id, error) => write!(f, "library instance {}: {error}", id.0),
            Self::Static(path, error) => write!(f, "static {path}: {error:?}"),
        }
    }
}

/// Expand, normalize and import a whole program for one target.
pub fn lower(program: &Program, target: &TargetSpec) -> Result<Lowered, LowerError> {
    lower_with_options(program, target, &crate::flat_walk::Options::default())
}

/// Import with a matrix-selectable variable-flat loop rewrite.
pub fn lower_with_options(
    program: &Program,
    target: &TargetSpec,
    flat_walk: &crate::flat_walk::Options,
) -> Result<Lowered, LowerError> {
    lower_discharging(program, target, flat_walk, &Discharge::default())
}

/// `lower_with_options` under a PS-13 check-removal plan: each planned
/// definition's units import without its licensed checks, after a reference
/// import that keeps them all (`Lowered::discharged`).
pub fn lower_discharging(
    program: &Program,
    target: &TargetSpec,
    flat_walk: &crate::flat_walk::Options,
    discharge: &Discharge,
) -> Result<Lowered, LowerError> {
    lower_with_roots(program, target, flat_walk, discharge, true, &[])
}

/// Batch reference import with explicit specialization policy and library roots.
/// Demand queries import individual instances through `import_instance`. The
/// diagnostic batch pipeline also supplies an independent middle-end test oracle.
pub fn lower_with_roots(
    program: &Program,
    target: &TargetSpec,
    flat_walk: &crate::flat_walk::Options,
    discharge: &Discharge,
    scoped_specialization: bool,
    roots: &[crate::library::Request],
) -> Result<Lowered, LowerError> {
    hirsel_intern::timings::count(hirsel_intern::timings::Count::NativeAppendExpand);
    // Source calls retain their admissions, targets and failure ordering.
    // Building an append result directly in its consumer would change all
    // three under LANG-10, including mutations observed during cleanup.
    let native_discharge = discharge.clone();
    let mut discharged = Vec::new();
    // The library's native representations lie over the program's owners.
    let owners = crate::library::owners(&program.types, &program.signatures).over(&program.owners);
    let mut layouts = LayoutBuilder::new(target, &owners);
    layouts.prepare_native_controls();
    let mut instances =
        crate::instance::expand(program, &mut layouts).map_err(LowerError::Instance)?;
    for root in roots {
        crate::library::instantiate(&mut layouts, &mut instances, root.clone())
            .map_err(LowerError::Instance)?;
    }
    let mut plans = Vec::new();
    let mut declined = Vec::new();
    let mut units = Vec::new();
    // Importing a unit or building a library body may require further
    // library instances, which join the end of the list.
    let mut next = 0;
    while next < instances.len() {
        let id = InstanceId(next as u32);
        next += 1;
        let definition = instances.view(id).key.definition().clone();
        let sites = discharge.sites(&definition);
        let imported = import_instance(program, &mut instances, id, sites, &mut layouts)?;
        plans.push(imported.plan);
        declined.extend(imported.declined.into_iter().map(|d| (id, d)));
        if let Some(reference) = imported.discharged {
            discharged.push(reference);
        }
        for request in imported.required {
            crate::library::instantiate(&mut layouts, &mut instances, request)
                .map_err(LowerError::Instance)?;
        }
        units.push(imported.unit);
    }
    let statics = statics(program, &instances, &units, &mut layouts)?;
    // Units name the table by digest: seal them once no layout is added.
    // The block header is the layout of a reuse token, which a
    // later stage creates after sealing.
    layouts.header();
    let table = layouts.finish();
    for unit in &mut units {
        unit.header.target = table.target;
    }
    let units = units
        .into_iter()
        .map(|u| crate::flat_walk::rewrite(u, &table, flat_walk))
        .collect();
    let semantic_owners = instances
        .ids()
        .map(|id| {
            let path = instances.view(id).key.definition().clone();
            let owner = if instances.roles[id.index()].is_library() {
                None
            } else {
                Some(path.clone())
            };
            (path, owner)
        })
        .collect();
    let mut lowered = Lowered {
        instances,
        plans,
        declined,
        units,
        table,
        statics,
        unit_origins: Vec::new(),
        unit_keys: Vec::new(),
        semantic_owners,
        inline_instances: Default::default(),
        discharged,
        native_discharge,
    };
    let specialized = crate::closure_calls::specialize_scoped(
        &mut lowered,
        program,
        target,
        scoped_specialization,
    );
    specialized.assert_origins(&lowered);
    lowered.unit_origins = specialized.origins;
    lowered.unit_keys = specialized.keys;
    lowered.inline_instances = specialized.inline;
    let registry = lowered.operation_registry(program);
    prune_pretransfer(
        &mut lowered.units,
        &lowered.table,
        &registry.pretransfer_adapters,
    );
    crate::arena_credit::summarize(&mut lowered.units, &mut lowered.table);
    Ok(lowered)
}

/// Finish code-target-dependent cleanup before any strict C qualification.
/// The validator independently reconstructs necessity; this only removes the
/// speculative caller-owned branch for dispatch proven ordinary by the producer.
pub fn prune_pretransfer(
    units: &mut [LirUnit<Value>],
    table: &LayoutTable,
    inventory: &[hirsel_lir::pretransfer::AdapterReference],
) {
    for unit in units {
        let required = hirsel_lir::pretransfer::required(unit, table, inventory);
        for (call, required) in unit.tables.calls.iter_mut().zip(required) {
            if !required {
                call.pretransfer_failure = None;
            }
        }
        *unit = crate::insert::Editor::new(unit.clone()).finish();
    }
}

/// The sole miss import of one source/library instance. Discovery and
/// publication are explicit outputs; no cache, effect replay or whole-program
/// traversal participates in this operation. Native validation remains required.
pub struct InstanceImport {
    pub unit: LirUnit<Value>,
    pub plan: Option<LoopPlan>,
    pub declined: Vec<Declined>,
    pub required: Vec<crate::library::Request>,
    pub discharged: Option<DischargedUnit>,
}

pub fn import_instance(
    program: &Program,
    instances: &mut Instances,
    id: InstanceId,
    sites: Option<&BTreeSet<Discharged>>,
    layouts: &mut dyn crate::library::builder::Layouts,
) -> Result<InstanceImport, LowerError> {
    hirsel_intern::timings::count(hirsel_intern::timings::Count::NativeImports);
    if instances.roles[id.index()].is_library() {
        let unit = crate::library::build(program, layouts, instances, id)
            .map_err(|e| LowerError::Library(id, e))?;
        return Ok(InstanceImport {
            unit,
            plan: None,
            declined: Vec::new(),
            required: Vec::new(),
            discharged: None,
        });
    }
    let (plan, declined) =
        plan_instance(program, instances, id, layouts).map_err(|e| LowerError::Normalize(id, e))?;
    let none = BTreeSet::new();
    let reference = if sites.is_some() {
        Some(
            import_body(
                program,
                instances,
                id,
                plan.as_ref(),
                &none,
                layouts,
                &mut Vec::new(),
            )
            .map_err(|e| LowerError::Import(id, e))?,
        )
    } else {
        None
    };
    let mut required = Vec::new();
    let mut unit = import_body(
        program,
        instances,
        id,
        plan.as_ref(),
        sites.unwrap_or(&none),
        layouts,
        &mut required,
    )
    .map_err(|e| LowerError::Import(id, e))?;
    let discharged = reference.map(|reference| DischargedUnit {
        instance: id,
        definition: instances.view(id).key.definition().clone(),
        reference,
        discharged: unit.clone(),
    });
    let proven = crate::spare_push::prove(&unit, &|l| layouts.record(l).kind.clone());
    crate::spare_push::commit(&mut unit, &proven, &mut required, &program.types, layouts)
        .map_err(LowerError::Instance)?;
    let fields = crate::spare_push::prove_fields(&unit);
    crate::spare_push::commit_fields(&mut unit, &fields);
    Ok(InstanceImport {
        unit,
        plan,
        declined,
        required,
        discharged,
    })
}

impl Lowered {
    pub fn operation_registry(
        &self,
        program: &Program,
    ) -> hirsel_layout::operation::OperationRegistry {
        invocation_registry(
            program,
            &self.instances,
            &self.semantic_owners,
            &self.units,
            &self.unit_origins,
        )
    }
}

/// Bind expected bodies from the independent source instance and specialization
/// plan. Candidate anchor coordinates never select the expected inventory.
pub(crate) fn invocation_registry(
    program: &Program,
    instances: &Instances,
    owners: &BTreeMap<DefPath, Option<DefPath>>,
    units: &[LirUnit<Value>],
    origins: &[InstanceId],
) -> hirsel_layout::operation::OperationRegistry {
    let mut registry = program.operation_registry();
    assert_eq!(
        units.len(),
        origins.len(),
        "independent unit/body association"
    );
    for (unit, &origin) in units.iter().zip(origins) {
        if instances.roles[origin.index()].is_library() {
            continue;
        }
        let view = instances.view(origin);
        let owner = owners
            .get(&unit.header.definition)
            .and_then(Option::as_ref)
            .unwrap_or_else(|| view.key.definition());
        let original = program
            .definitions
            .get(owner)
            .expect("original source selection exists outside candidate units");
        let rows = registry
            .invocations
            .get(owner)
            .cloned()
            .expect("original source invocation inventory exists");
        let evidence = crate::invocations::bind_source(
            original,
            rows,
            &Symbol {
                kind: SymbolKind::Function,
                definition: unit.header.definition.clone(),
                instance: Some(unit.header.instance),
            },
            view.body,
        )
        .expect("independent source instance plan is valid");
        registry
            .invocations
            .insert(evidence.body.definition.clone(), evidence.invocations);
        registry.invocation_bodies.push(evidence.body);
    }
    bind_pretransfer_inventory(&mut registry, program, instances)
        .expect("independent adapter instance plan is valid");
    registry
        .invocation_bodies
        .sort_by(|a, b| (&a.definition, a.instance).cmp(&(&b.definition, b.instance)));
    registry
}

/// Bind materialized keyed adapters from the independent instance plan, never
/// candidate calls, symbols, anchors or ABI tables. Both batch and one-instance
/// import use this exact inventory before target-dependent edge cleanup and C.
pub fn bind_pretransfer_inventory(
    registry: &mut hirsel_layout::operation::OperationRegistry,
    program: &Program,
    instances: &Instances,
) -> Result<(), String> {
    if !std::sync::Arc::ptr_eq(&registry.types, &program.types)
        || !registry.pretransfer_adapters.is_empty()
        || !registry.pretransfer_actuals.is_empty()
    {
        return Err(
            "pretransfer inventory belongs to another or already bound declaration scope".into(),
        );
    }
    for id in instances.ids() {
        let view = instances.view(id);
        let Some(primitive) = hirsel_lir::pretransfer::primitive(view.key.definition()) else {
            continue;
        };
        let Some(body) = instances.ids().find(|other| {
            instances.view(*other).key.definition() == &primitive
                && instances.actuals[other.index()].arguments
                    == instances.actuals[id.index()].arguments
        }) else {
            return Err("materialized adapter has no independently planned primitive".into());
        };
        let reference = crate::invocations::keyed_adapter(
            view,
            &instances.actuals[id.index()].arguments,
            instances.view(body),
            &instances.actuals[body.index()].arguments,
            instances.abis[id.index()],
        )
        .map_err(str::to_owned)?;
        let operation = program
            .operations
            .get(&primitive)
            .ok_or("materialized adapter has no independent primitive declaration")?;
        let arguments = hirsel_layout::operation::collect_arguments(
            &program.types,
            &instances.actuals[body.index()].arguments,
        )
        .map_err(|error| format!("materialized adapter actuals: {error:?}"))?;
        registry.pretransfer_actuals.push((
            reference.adapter.clone(),
            reference.abi,
            operation.clone(),
            arguments,
        ));
        registry.pretransfer_adapters.push(reference);
    }
    Ok(())
}

/// The definition of every static empty sequence; its instance names the
/// sequence's payload layout.
pub fn empty_sequence() -> DefPath {
    hirsel_syntax::DefPath::core("$intrinsic.EmptySequence")
}

/// The definition of every boxed nullary variant's static block; its instance
/// names the payload layout and the variant
/// (`crate::statics::nullary_variant_instance`).
pub fn nullary_variant() -> DefPath {
    hirsel_syntax::DefPath::core("$intrinsic.NullaryVariant")
}

/// The static data of each constant the units read, once per constant, of
/// each static empty sequence (its zero count, at its payload layout) and of
/// each boxed nullary variant (its tag, at its payload layout).
/// Static data read by one demanded native unit, using the same encoder as
/// program assembly. The caller publishes content-named layout bindings.
pub fn instance_statics(
    program: &Program,
    instances: &Instances,
    unit: &LirUnit<Value>,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<Vec<hirsel_lir::statics::StaticData>, LowerError> {
    statics(program, instances, std::slice::from_ref(unit), layouts)
}

fn statics(
    program: &Program,
    instances: &Instances,
    units: &[LirUnit<Value>],
    layouts: &mut LayoutBuilder<'_>,
) -> Result<Vec<hirsel_lir::statics::StaticData>, LowerError> {
    let mut read = BTreeMap::new();
    let mut empty = BTreeMap::new();
    let mut nullary = BTreeMap::new();
    let empty_path = empty_sequence();
    let nullary_path = nullary_variant();
    for unit in units {
        for inst in &unit.instructions {
            if let Inst::ReadStatic { symbol, result } = inst {
                let symbol = &unit.tables.symbols[symbol.index()];
                if symbol.definition == empty_path {
                    let ty = unit.values[result.index()].ty;
                    let LayoutKind::Block { payload } =
                        layouts.record(unit.types[ty.index()].layout).kind
                    else {
                        unreachable!("an empty sequence read as a block")
                    };
                    empty.insert(symbol.instance, (symbol.clone(), payload));
                } else if symbol.definition == nullary_path {
                    let ty = unit.values[result.index()].ty;
                    let LayoutKind::Box { payload } =
                        layouts.record(unit.types[ty.index()].layout).kind
                    else {
                        unreachable!("a nullary variant read as a box")
                    };
                    nullary.insert(symbol.instance, (symbol.clone(), payload));
                } else if symbol.kind == SymbolKind::Constant {
                    read.insert(symbol.definition.clone(), symbol.clone());
                }
            }
        }
    }
    let empty = empty
        .into_values()
        .map(|(symbol, payload)| {
            let record = layouts.record(payload);
            // Even an empty sequence contains its complete header and alignment
            // padding before element zero: a counted sequence's count and capacity
            // (runtime ABI 5, both zero), Sequential Packed storage's count and
            // used-byte count. LLVM and H retain this exact static extent.
            let extent = 16usize.next_multiple_of(record.align as usize);
            Ok(hirsel_lir::statics::StaticData::plain(
                symbol,
                payload,
                vec![0; extent],
            ))
        })
        .collect::<Vec<_>>();
    let nullary = nullary.into_values().map(|(symbol, payload)| {
        crate::statics::nullary_variant(layouts.table(), symbol, payload).ok_or_else(|| {
            LowerError::Static(
                nullary_variant(),
                crate::statics::StaticError("a nullary variant its layout does not have".into()),
            )
        })
    });
    let nullary = nullary.collect::<Vec<_>>();
    empty
        .into_iter()
        .chain(nullary)
        .chain(read.into_values().map(|symbol| {
            let definition = symbol.definition.clone();
            let static_error = |e| LowerError::Static(definition.clone(), e);
            let Some(value) = program.constants.get(&symbol.definition) else {
                return Err(static_error(crate::statics::StaticError(
                    "a constant without a materialized value".into(),
                )));
            };
            crate::statics::encode_functions(
                layouts,
                &program.types,
                symbol,
                value,
                &mut |path, arguments| {
                    let actuals = Actuals {
                        binder: program.binders.get(path).copied(),
                        arguments: arguments.to_vec(),
                        outer: vec![],
                    };
                    let adapter = crate::library::keyed::request(program, path, arguments.to_vec())
                        .map_err(|e| {
                            crate::statics::StaticError(format!("static adapter: {e:?}"))
                        })?;
                    let callable_path = adapter.as_ref().map(|r| &r.path).unwrap_or(path);
                    let callable_actuals = adapter.as_ref().map(|r| &r.actuals).unwrap_or(&actuals);
                    let id = instances
                        .find(program, callable_path, callable_actuals)
                        .or_else(|error| {
                            if arguments.is_empty() {
                                instances
                                    .ids()
                                    .find(|id| instances.view(*id).key.definition() == path)
                                    .ok_or(error)
                            } else {
                                Err(error)
                            }
                        })
                        .map_err(|e| {
                            crate::statics::StaticError(format!("static callable instance: {e:?}"))
                        })?;
                    Ok(Symbol {
                        kind: SymbolKind::Function,
                        definition: callable_path.clone(),
                        instance: Some(instances.view(id).key.digest()),
                    })
                },
            )
            .map_err(static_error)
        }))
        .collect()
}

/// The loop plan of one instance, with layout facts of its own types.
pub fn plan_instance(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    layouts: &mut dyn crate::library::builder::Layouts,
) -> Result<(Option<LoopPlan>, Vec<Declined>), NormalizeError> {
    let actuals = &instances.actuals[id.index()];
    let types = &*program.types;
    let abi = &layouts.table().fn_abis[instances.abis[id.index()].0 as usize];
    let passing: Vec<Passing> = abi.parameters.iter().map(|p| p.convention).collect();
    let mut failed = None;
    let mut facts = |ty: TypeId| -> Facts {
        let none = Facts {
            plain: false,
            copyable: false,
        };
        let Ok(t) = substitute(types, &program.memo, ty, actuals, false) else {
            failed.get_or_insert("substitution");
            return none;
        };
        let Ok(layout) = layouts.layout(types, t) else {
            failed.get_or_insert("layout");
            return none;
        };
        let r = layouts.record(layout);
        let trivial = r.drop.is_empty() && r.finalizer.is_none();
        Facts {
            plain: trivial && r.copy == CopyRecipe::Bits,
            copyable: r.copy != CopyRecipe::NotCopyable,
        }
    };
    crate::normalize::plan(program, instances, id, &passing, &mut facts)
}

/// Tri-state facts joined at merges (definitely no, definitely yes, either).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Tri {
    No,
    Yes,
    Maybe,
}

impl Tri {
    fn join(self, other: Self) -> Self {
        if self == other { self } else { Tri::Maybe }
    }
    fn may(self) -> bool {
        self != Tri::No
    }
}

/// `Element` flags. Dead storage, an inactive loan, no view: what most of a
/// state is.
const DEAD: u8 = 1;
/// Joined with a `DEAD` element, this one stays as it is.
const ABSORBS: u8 = 2;
/// Storage holding nothing: uninitialized on every path.
const EMPTY: u8 = 4;

impl Element for Tri {
    fn flags(&self) -> u8 {
        match self {
            Tri::No => DEAD | ABSORBS,
            Tri::Maybe => ABSORBS,
            Tri::Yes => 0,
        }
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// One field-path step as the validator compares them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
enum Step {
    Payload,
    Field(u32),
    Variant(u32, u32),
    Index,
}

fn overlap(a: &[Step], b: &[Step]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| x == y || (*x == Step::Index && *y == Step::Index))
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct SlotState {
    live: Tri,
    init: Tri,
    moved: BTreeMap<Vec<Step>, Tri>,
}

impl SlotState {
    const DEAD: Self = Self {
        live: Tri::No,
        init: Tri::No,
        moved: BTreeMap::new(),
    };
    /// Whether the whole content may still be present (for a whole drop).
    fn present(&self) -> Tri {
        if self.init == Tri::No || self.moved.get(&Vec::new()) == Some(&Tri::Yes) {
            Tri::No
        } else {
            self.init
        }
    }
}

impl SlotState {
    fn join(&mut self, other: &SlotState) {
        self.live = self.live.join(other.live);
        self.init = self.init.join(other.init);
        let keys: BTreeSet<_> = self
            .moved
            .keys()
            .chain(other.moved.keys())
            .cloned()
            .collect();
        for key in keys {
            let l = self.moved.get(&key).copied().unwrap_or(Tri::No);
            let r = other.moved.get(&key).copied().unwrap_or(Tri::No);
            self.moved.insert(key, l.join(r));
        }
    }
}

impl Element for SlotState {
    fn flags(&self) -> u8 {
        let dead = self.live == Tri::No && self.init == Tri::No && self.moved.is_empty();
        // Joined with dead storage, a `Yes` becomes `Maybe` and nothing else
        // changes.
        let absorbs = self.live != Tri::Yes
            && self.init != Tri::Yes
            && self.moved.values().all(|t| *t != Tri::Yes);
        u8::from(dead) * DEAD + u8::from(absorbs) * ABSORBS + u8::from(self.init == Tri::No) * EMPTY
    }
    fn absorbs(own: u8, other: u8) -> bool {
        own & ABSORBS != 0 && other & DEAD != 0
    }
}

/// The abstract state at one program point: per slot, liveness, initialization
/// and moved parts; per loan, whether it is active. It mirrors exactly what the
/// LIR validator derives, so a state-specialized cleanup is valid. Clones share
/// structure (`PVec`), so keeping the state of every edge, comparing two and
/// hashing one cost what changed between them.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct State {
    slots: PVec<SlotState>,
    loans: PVec<Tri>,
    /// Per TIR local, whether a joined view temporary holds a view.
    views: PVec<Tri>,
}

impl State {
    fn join(&mut self, other: &State) {
        // Slots and loans created later in a pass are dead and inactive here.
        fn padded<T: Element>(a: &mut PVec<T>, b: &PVec<T>, fill: T) -> PVec<T> {
            if a.len() < b.len() {
                a.resize(b.len(), fill.clone());
            }
            let mut b = b.clone();
            if b.len() < a.len() {
                b.resize(a.len(), fill);
            }
            b
        }
        let slots = padded(&mut self.slots, &other.slots, SlotState::DEAD);
        self.slots.join(&slots, SlotState::join);
        let loans = padded(&mut self.loans, &other.loans, Tri::No);
        self.loans.join(&loans, |a, b| *a = a.join(*b));
        self.views.join(&other.views, |a, b| *a = a.join(*b));
    }
    fn live(&mut self, slot: SlotId) {
        self.slots[slot.index()] = SlotState {
            live: Tri::Yes,
            init: Tri::No,
            moved: BTreeMap::new(),
        };
    }
    fn dead(&mut self, slot: SlotId) {
        self.slots[slot.index()] = SlotState::DEAD;
    }
    /// A write of `path` (Store/Replace): the part is present again.
    fn write(&mut self, slot: SlotId, path: &[Step]) {
        let s = &mut self.slots[slot.index()];
        if path.is_empty() {
            s.init = Tri::Yes;
            s.moved.clear();
        } else {
            s.moved
                .retain(|q, _| !(q.len() >= path.len() && overlap(q, path)));
        }
    }
    /// A move or drop of `path`.
    fn take(&mut self, slot: SlotId, path: &[Step]) {
        let s = &mut self.slots[slot.index()];
        if path.is_empty() {
            s.init = Tri::No;
            s.moved.clear();
        } else {
            s.moved
                .retain(|q, _| !(q.len() > path.len() && overlap(q, path)));
            s.moved.insert(path.to_vec(), Tri::Yes);
        }
    }
}

/// A resolved TIR place: its slot root, validator steps and LIR projection.
#[derive(Clone, Debug)]
struct PlaceRef {
    /// A joined borrowed SSA value supplies read-only backing instead of a slot.
    value: Option<ValueId>,
    source_place: Option<u32>,
    slot: SlotId,
    steps: Vec<Step>,
    projection: Vec<ProjectionStep<tir::LocalId>>,
    ty: LirTypeId,
    /// No index or variant step and no block crossing: preparable once.
    fixed: bool,
    /// The place lies inside a block's payload (an element of a list, a
    /// field behind a box) or at a computed position of inline storage, and
    /// is named through a temporary (a call's or a block's result, perhaps a
    /// view of a block-local it extends), which nothing writes again.
    crosses: bool,
    /// The loan a borrowed read of the place is inside.
    loan: Option<tir::LoanId>,
    /// A variable-size part of a flat value: `ty` is its length-prefixed
    /// encoding, this its source type, which only flat-aware operations
    /// produce (LIR schema revision 4).
    source: Option<(LirTypeId, TypeId)>,
    /// The site of the origin binding that named this place: an index or tag
    /// check of it fails there, as in the interpreter (docs/spec/diagnostics.md: the operation
    /// naming the place, not the enclosing statement).
    site: Option<Span>,
}

/// What is known of a place at one reaching version of its slot. Facts flow
/// only along single-predecessor TIR edges, so wherever one holds, the point
/// that established it dominates.
#[derive(Clone, Debug)]
enum Fact {
    /// After a variant switch: the place holds one of `variants`.
    Variant {
        slot: SlotId,
        projection: Vec<ProjectionStep<tir::LocalId>>,
        version: ValueId,
        variants: Vec<u32>,
    },
    /// A field of a proven variant, prepared once at its arm's `BindFields`.
    Prepared {
        slot: SlotId,
        projection: Vec<ProjectionStep<tir::LocalId>>,
        version: ValueId,
        address: AddressId,
    },
}

/// Narrow what `facts` know of a place's variant to `allowed`.
fn narrow(
    facts: &mut Vec<Fact>,
    slot: SlotId,
    projection: &[ProjectionStep<tir::LocalId>],
    version: ValueId,
    allowed: &[u32],
) {
    for fact in facts.iter_mut() {
        if let Fact::Variant {
            slot: s,
            projection: q,
            version: v,
            variants,
        } = fact
            && *s == slot
            && *v == version
            && q == projection
        {
            variants.retain(|variant| allowed.contains(variant));
            return;
        }
    }
    facts.push(Fact::Variant {
        slot,
        projection: projection.to_vec(),
        version,
        variants: allowed.to_vec(),
    });
}

/// Whether a pattern matches every value of its place's type: a failed match
/// of a variant pattern over such fields can only be its own tag test.
fn irrefutable(m: &tir::MatchTable, id: tir::MatchPatternId) -> bool {
    use tir::MatchPatternKind as P;
    match &m.patterns[id.0 as usize].kind {
        P::Wildcard | P::Binding { .. } | P::Literal(tir::Constant::Unit) => true,
        P::Tuple(fields) | P::Record { fields, .. } => {
            fields.iter().all(|(_, sub)| irrefutable(m, *sub))
        }
        P::Literal(_) | P::Range { .. } | P::Variant { .. } => false,
    }
}

/// One cleanup action still to run on an exit or failure path.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum Action {
    /// A TIR cleanup node, anchored at its `node_anchors` entry.
    Tir { node: u32 },
    /// Release an owned slot no TIR action names, anchored at a reserved
    /// `(cleanup, action)` pair.
    Release { slot: SlotId, anchor: (u32, u32) },
    /// End a loan left active where TIR names no cleanup.
    EndLoan { loan: LoanId },
}

/// One block of a failure-cleanup chain: loan ends, then a drop (the last
/// link has none and propagates). It names everything the block's content
/// and its validity depend on: the instructions, and the state of the
/// dropped slot before the drop. An action that does nothing leaves no trace.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct Link {
    ends: Vec<(LoanId, bool)>,
    drop: Option<(AddressId, bool, CleanupAnchorId, SlotState)>,
}

/// Builder-side block, laid out into the unit's arenas by `finish`.
struct BBlock {
    kind: BlockKind,
    params: Vec<ValueId>,
    /// Version joins, after `params`: one parameter per slot.
    phis: Vec<(SlotId, ValueId)>,
    /// Joined views, after `phis`: one parameter per joined view temporary.
    view_phis: Vec<(usize, ValueId)>,
    insts: Vec<Inst>,
    term: Option<Terminator>,
    anchor: SourceAnchorId,
}

struct BEdge {
    target: BlockId,
    args: Vec<ValueId>,
    /// Slot versions at the edge's source, for the target's version joins.
    versions: PVec<Option<ValueId>>,
    /// Joined views at the edge's source, for the target's view joins.
    views: PVec<Option<ValueId>>,
}

/// Arenas of one emission pass.
#[derive(Default)]
struct Out {
    types: Vec<LirType>,
    type_index: HashMap<LirType, LirTypeId>,
    values: Vec<ValueInfo>,
    slots: Vec<SlotInfo>,
    addresses: Vec<AddressInfo>,
    fixed_addresses: HashMap<(SlotId, Vec<ProjectionStep>), AddressId>,
    blocks: Vec<BBlock>,
    edges: Vec<BEdge>,
    tables: hirsel_lir::ir::SideTables,
    pools: hirsel_lir::ir::Pools,
    symbol_index: HashMap<Symbol, SymbolId>,
    operation_index: HashMap<OperationId, OperationRef>,
    source_anchor_index: HashMap<SourceAnchor, SourceAnchorId>,
    cleanup_anchor_index: HashMap<CleanupAnchor, CleanupAnchorId>,
    constant_index: HashMap<ConstantData, ConstantId>,
    parameters: Vec<ValueId>,
}

/// What a failure successor must do besides cleanup.
struct FailureContext {
    /// The TIR cleanup chain the current operation fails into; None derives
    /// the cleanup from the state (operations TIR cannot make fail).
    cleanup: Option<tir::CleanupId>,
    /// Actions that run before that chain (the rest of a normal cleanup list).
    before: Vec<Action>,
    /// Owned values the operation has not consumed yet.
    pending: Vec<ValueId>,
}

/// The importer as an inline lowering sees it: the calling unit at the call
/// being replaced. A lowering declines
/// (returns false) before it emits anything.
pub struct ImportCx<'i> {
    im: &'i mut (dyn Inline + 'i),
}

/// The importer operations an inline lowering is built from.
trait Inline {
    fn operand(&mut self, operand: &tir::Operand) -> R<ValueId>;
    fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId>;
    fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)>;
    fn tir_layout(&mut self, ty: TypeId) -> R<LayoutId>;
    fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId>;
    fn layout_record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord;
    fn new_value(&mut self, layout: LayoutId, holding: Option<Holding>) -> ValueId;
    fn literal(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId;
    fn emit(&mut self, inst: Inst);
    fn consume(&mut self, value: ValueId);
    fn pend(&mut self, value: ValueId);
    fn failure(&mut self, rule: Option<String>) -> R<FailureId>;
    fn finish(&mut self, destination: tir::PlaceId, next: tir::BlockId, value: ValueId) -> R<()>;
    fn intern_symbol(&mut self, symbol: &Symbol) -> SymbolId;
    fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId>;
    fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()>;
    fn construct_inline(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId>;
    fn codec_schema(
        &mut self,
        ty: TypeId,
    ) -> R<Option<hirsel_types::serialization::SerializableSchema>>;
    fn layout_table(&self) -> &hirsel_lir::layout_types::LayoutTable;
    fn add_codec(&mut self, plan: hirsel_lir::codec::CodecPlan) -> hirsel_lir::ir::CodecId;
    fn direct_call(
        &mut self,
        symbol: SymbolId,
        abi: FnAbiId,
        arguments: Vec<CallArgument>,
    ) -> R<Option<ValueId>>;
    /// The direct-call symbol and `FnAbi` of the nongeneric function `path`.
    fn function(&mut self, path: &DefPath) -> R<(SymbolId, FnAbiId)>;
}

impl ImportCx<'_> {
    pub fn read_field(&mut self, value: ValueId, field: u32) -> R<ValueId> {
        self.im.read_field(value, field)
    }
    /// The value of a call argument as the call would pass it: a `borrow`
    /// argument is a view under its TIR loan, a `take` argument is moved
    /// out of its place.
    pub fn operand(&mut self, operand: &tir::Operand) -> R<ValueId> {
        self.im.operand(operand)
    }
    /// The address of a TIR place (a `mut` argument's, say) and the
    /// version of its root slot that reaches the call.
    pub fn place_address(&mut self, place: tir::PlaceId) -> R<(AddressId, ValueId)> {
        self.im.place_address(place)
    }
    /// The layout of a TIR place: the call's destination, say.
    pub fn place_layout(&mut self, place: tir::PlaceId) -> R<LayoutId> {
        self.im.place_layout(place)
    }
    pub fn record(&self, layout: LayoutId) -> &hirsel_lir::layout_types::LayoutRecord {
        self.im.layout_record(layout)
    }
    /// A freshly produced value of `layout`: owned if the layout carries a
    /// credit, plain otherwise.
    pub fn fresh(&mut self, layout: LayoutId) -> ValueId {
        self.im.new_value(layout, None)
    }
    pub fn value(&mut self, layout: LayoutId, holding: Holding) -> ValueId {
        self.im.new_value(layout, Some(holding))
    }
    /// A literal produced by an inline lowering (including a unit result).
    pub fn constant(&mut self, layout: LayoutId, value: ConstantValue) -> ValueId {
        self.im.literal(layout, value)
    }
    /// Append an instruction at the call.
    pub fn push(&mut self, inst: Inst) {
        self.im.emit(inst);
    }
    /// An owned value an instruction just consumed: the call's failure
    /// cleanup no longer releases it.
    pub fn consumed(&mut self, value: ValueId) {
        self.im.consume(value);
    }
    /// An owned value the lowering produced and has not stored yet: a
    /// later failure of the call releases it.
    pub fn pending(&mut self, value: ValueId) {
        self.im.pend(value);
    }
    /// A failure edge at the call, into the call's own cleanup.
    pub fn failure(&mut self, rule: Option<&str>) -> R<FailureId> {
        self.im.failure(rule.map(str::to_owned))
    }
    /// Store the call's result and continue after the call. The last thing
    /// a lowering does.
    pub fn finish(&mut self, call: &crate::library::CallShape<'_>, value: ValueId) -> R<()> {
        self.im.finish(call.destination, call.next, value)
    }
    pub fn symbol(&mut self, symbol: &Symbol) -> SymbolId {
        self.im.intern_symbol(symbol)
    }
    /// Construct an inline library value, retaining failure ownership until stored.
    pub fn construct(
        &mut self,
        layout: LayoutId,
        payload: LayoutId,
        constructor: ConstructorKind,
        values: &[ValueId],
    ) -> R<ValueId> {
        self.im
            .construct_inline(layout, payload, constructor, values)
    }
    /// Read field zero's sequence count under the caller's existing loan.
    pub fn container_len(&mut self, call: &crate::library::CallShape<'_>) -> R<()> {
        self.im.container_len(call)
    }
    /// Construct an erased iterator environment and require its step body.
    pub fn iterator_closure(
        &mut self,
        call: &crate::library::CallShape<'_>,
        helper: &DefPath,
    ) -> R<ValueId> {
        self.im.iterator_closure(call, helper)
    }

    /// The concrete SER-1 schema of a type of the importing definition, under
    /// this instance's actuals; None when it is not serializable.
    pub fn codec_schema(
        &mut self,
        ty: TypeId,
    ) -> R<Option<hirsel_types::serialization::SerializableSchema>> {
        self.im.codec_schema(ty)
    }
    pub fn layout_table(&self) -> &hirsel_lir::layout_types::LayoutTable {
        self.im.layout_table()
    }
    /// Add a codec plan to the unit's codec table.
    /// The direct-call symbol and `FnAbi` of the nongeneric function `path`.
    pub fn function(&mut self, path: &DefPath) -> R<(SymbolId, FnAbiId)> {
        self.im.function(path)
    }
    pub fn codec(&mut self, plan: hirsel_lir::codec::CodecPlan) -> hirsel_lir::ir::CodecId {
        self.im.add_codec(plan)
    }
}

pub fn import(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    plan: Option<&LoopPlan>,
    discharge: &BTreeSet<Discharged>,
    layouts: &mut LayoutBuilder<'_>,
    required: &mut Vec<crate::library::Request>,
) -> R<LirUnit<Value>> {
    import_body(program, instances, id, plan, discharge, layouts, required)
}

fn import_body(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    plan: Option<&LoopPlan>,
    discharge: &BTreeSet<Discharged>,
    layouts: &mut dyn crate::library::builder::Layouts,
    required: &mut Vec<crate::library::Request>,
) -> R<LirUnit<Value>> {
    let view = instances.view(id);
    let def: &tir::Definition = view.definition().map_err(InstanceError::from)?;
    let body = &def.bodies[view.body.0 as usize];
    let actuals = &instances.actuals[id.index()];
    let types = &*program.types;
    let aliases = view_temporaries(body);
    let joined = joined_views(body, &aliases);
    let mut im = Importer {
        program,
        instances,
        layouts,
        id,
        plan,
        discharge,
        source_anchors: tir::source_anchors(def),
        // Reference and discharged imports name the same canonical source
        // place, independently of which guards the proof plan licenses.
        source_places: {
            let mut canonical = HashMap::new();
            body.places
                .iter()
                .enumerate()
                .map(|(index, place)| {
                    *canonical
                        .entry((place.root, place.projection.clone()))
                        .or_insert(index as u32)
                })
                .collect()
        },
        def,
        body,
        body_id: view.body.0,
        actuals,
        types,
        type_memo: HashMap::new(),
        static_layout_keys: HashMap::new(),
        receipt_values: HashMap::new(),
        required,
        finalizer_memo: std::collections::HashSet::new(),
        out: Out::default(),
        local_slot: Vec::new(),
        aliases,
        slices: slice_views::Slices::default(),
        encoding_views: BTreeSet::new(),
        observations: discarded_callable_reads(body, types),
        nullary: nullary_temporaries(body),
        node_anchors: node_anchors(body),
        joined,
        views: PVec::default(),
        entry_views: Vec::new(),
        loop_slots: Vec::new(),
        holes: None,
        loan_map: Vec::new(),
        elided_loans: body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter_map(|i| {
                if let tir::InstructionKind::BeginLoan {
                    id: loan, place, ..
                } = i.kind
                    && instances
                        .elided
                        .get(&id)
                        .is_some_and(|locals| locals.contains(&body.places[place.0 as usize].root))
                {
                    Some(loan)
                } else {
                    None
                }
            })
            .collect(),
        builders: flat_builder::Builders::default(),
        fields: flat_field::Fields::default(),
        tir_block: Vec::new(),
        call_block: tir::BlockId(0),
        versioned: Vec::new(),
        constant_version: PVec::default(),
        prologue: BlockId(0),
        prologue_failure: None,
        unreachable: None,
        propagate: None,
        current: BlockId(0),
        invocation_live: false,
        invocation_return: None,
        state: State {
            slots: PVec::default(),
            loans: PVec::default(),
            views: PVec::default(),
        },
        versions: PVec::default(),
        failure: FailureContext {
            cleanup: None,
            before: Vec::new(),
            pending: Vec::new(),
        },
        chain_memo: HashMap::new(),
        scratch: HashMap::new(),
        incoming: Vec::new(),
        entry_versions: Vec::new(),
        facts: Vec::new(),
        entry_facts: Vec::new(),
        tir_preds: Vec::new(),
        rpo: Vec::new(),
        capture_versions: Vec::new(),
        outer_scope: false,
        span: Span::default(),
        statement: (0, 0),
        origin_sites: HashMap::new(),
    };
    im.builders = im.builder_webs()?;
    let loans: Vec<_> = im.builders.loans.iter().copied().collect();
    im.elided_loans.extend(loans);
    im.fields = im.field_webs()?;
    let loans: Vec<_> = im.fields.loans.iter().copied().collect();
    im.elided_loans.extend(loans);
    byte_views::aliases(body, types, &mut im.aliases);
    im.slices = slice_views::discover(body, types, &im.aliases);
    for (l, selected) in im.slices.locals.iter().enumerate() {
        if *selected {
            im.joined[l] = true;
            im.aliases[l] = None;
        }
    }
    for inst in body.blocks.iter().flat_map(|b| &b.instructions) {
        if let tir::InstructionKind::BeginLoan { id, place, .. } = inst.kind
            && im.slices.locals[body.places[place.0 as usize].root.0 as usize]
            && !im.slices.sources.values().any(|slice| slice.loan == id)
        {
            im.elided_loans.insert(id);
        }
    }
    for (local, joined) in im.joined.iter().enumerate() {
        if *joined {
            im.aliases[local] = None;
        }
    }
    im.cfg()?;
    let mut entry: Vec<Option<State>> = vec![None; body.blocks.len()];
    // Abstract states only grow (towards Maybe), so this reaches a fixpoint.
    for _ in 0..1000 {
        let used = im.pass(&entry)?;
        if converged(&used, &entry) {
            return im.finish();
        }
        entry = used;
    }
    malformed("abstract state did not converge")
}

/// Whether two passes agree on every block's entry state. The states of one
/// pass share structure with each other and none with the other pass's, so
/// the comparison remembers the parts it has already found equal.
fn converged(used: &[Option<State>], entry: &[Option<State>]) -> bool {
    let (mut slots, mut loans, mut views) = Default::default();
    used.len() == entry.len()
        && used.iter().zip(entry).all(|pair| match pair {
            (Some(a), Some(b)) => {
                a.slots.matches(&b.slots, &mut slots)
                    && a.loans.matches(&b.loans, &mut loans)
                    && a.views.matches(&b.views, &mut views)
            }
            (None, None) => true,
            _ => false,
        })
}

struct Importer<'a> {
    invocation_live: bool,
    invocation_return: Option<tir::BlockId>,
    source_anchors: Vec<hirsel_span::Anchor>,
    source_places: Vec<u32>,
    /// Payload layouts are complete before construction; later rows cannot
    /// change their keys. Repeated empty constructions share this graph walk.
    static_layout_keys: HashMap<LayoutId, hirsel_lir::layout_types::LayoutKey>,
    receipt_values: HashMap<ValueId, SlotId>,
    program: &'a Program,
    instances: &'a Instances,
    layouts: &'a mut dyn crate::library::builder::Layouts,
    id: InstanceId,
    plan: Option<&'a LoopPlan>,
    /// PS-13 sites of this definition that import without their check.
    discharge: &'a BTreeSet<Discharged>,
    def: &'a tir::Definition,
    body: &'a tir::Body,
    body_id: u32,
    actuals: &'a Actuals,
    /// The session interner.
    types: &'a TypeInterner,
    /// TIR types already substituted by the instance's actuals.
    type_memo: HashMap<TypeId, TypeId>,
    /// Library instances this unit requires; `lower` registers them.
    required: &'a mut Vec<crate::library::Request>,
    /// Store types already searched for library resources.
    finalizer_memo: std::collections::HashSet<TypeId>,
    out: Out,
    local_slot: Vec<Option<SlotId>>,
    /// A view temporary: the place it views, read through instead of storage.
    aliases: Vec<Option<tir::PlaceId>>,
    slices: slice_views::Slices,
    encoding_views: BTreeSet<usize>,
    /// Unread callable borrow temporaries evaluate their source without a store.
    observations: Vec<bool>,
    /// Per TIR local, the nullary variant it only ever holds (`nullary_temporaries`).
    nullary: Vec<Option<u32>>,
    /// Per TIR cleanup node, the `(cleanup, ordinal)` its drop anchors at.
    node_anchors: Vec<(u32, u32)>,
    /// A joined view temporary: assigned views of places under loans on
    /// several paths, never stored. Its value is SSA, joined at merges into a
    /// borrowed block parameter covered by the union of its incoming loans.
    joined: Vec<bool>,
    /// Per TIR local, the joined view it holds here.
    views: PVec<Option<ValueId>>,
    /// Per single-predecessor TIR block, the joined views its edge carries.
    entry_views: Vec<Option<PVec<Option<ValueId>>>>,
    /// Self-tail loop slot of each TIR parameter ordinal (LANG-10).
    loop_slots: Vec<Option<SlotId>>,
    /// Constructor-tail root and hole slots (LANG-10, LIR 59).
    holes: Option<holes::Holes>,
    loan_map: Vec<LoanId>,
    elided_loans: std::collections::HashSet<tir::LoanId>,
    /// Lists built in their flat encoding (LIR schema revision 22).
    builders: flat_builder::Builders,
    /// Byte lists built in place in a builder's element (revision 24).
    fields: flat_field::Fields,
    tir_block: Vec<Option<BlockId>>,
    /// The TIR block whose call terminator is being lowered.
    call_block: tir::BlockId,
    /// Slots written somewhere after entry: candidates for version joins.
    versioned: Vec<bool>,
    /// The single version of a slot written only at entry (parameters).
    constant_version: PVec<Option<ValueId>>,
    prologue: BlockId,
    prologue_failure: Option<BlockId>,
    unreachable: Option<BlockId>,
    propagate: Option<BlockId>,
    current: BlockId,
    state: State,
    versions: PVec<Option<ValueId>>,
    failure: FailureContext,
    /// Failure-cleanup blocks by what they run and where they continue.
    chain_memo: HashMap<(Link, Option<BlockId>), BlockId>,
    /// The outer scope's disposal scratch slots by the types disposed. Only a
    /// failure-cleanup block makes them live, and no failure there disposes
    /// again, so every exit's disposal of the same shape shares its slots,
    /// and with them the links of the chains that release them. A shape's
    /// slots are made together, so they release in their nested order.
    scratch: HashMap<Vec<LirTypeId>, Vec<SlotId>>,
    /// Per TIR block, the join of states reaching it in this pass.
    incoming: Vec<Option<State>>,
    /// Per single-predecessor TIR block, the versions its edge carries.
    entry_versions: Vec<Option<PVec<Option<ValueId>>>>,
    /// Facts holding here, and per single-predecessor TIR block, those its edge carries.
    facts: Vec<Fact>,
    entry_facts: Vec<Option<Vec<Fact>>>,
    tir_preds: Vec<u32>,
    rpo: Vec<tir::BlockId>,
    /// Per capture ordinal, the version naming its environment content.
    capture_versions: Vec<ValueId>,
    /// Whether a scratch slot lives in the synthetic scope enclosing the
    /// body's scopes (ordinal `body.scopes.len()`).
    outer_scope: bool,
    /// Site and TIR statement of the operation being imported.
    span: Span,
    statement: (u32, u32),
    /// Per TIR place an origin event named, the site of the first one.
    origin_sites: HashMap<tir::PlaceId, Span>,
}

/// The anchor of each TIR cleanup node: its position in the first cleanup
/// (by id) whose chain runs it. Chains share suffixes, so one action has a
/// `(cleanup, ordinal)` in every chain through it; naming it by one of them
/// lets every failure site that runs it share the block that does.
/// Each node is walked once.
fn node_anchors(body: &tir::Body) -> Vec<(u32, u32)> {
    let mut out: Vec<Option<(u32, u32)>> = vec![None; body.cleanup_nodes.len()];
    for (c, cleanup) in body.cleanups.iter().enumerate() {
        let mut next = cleanup.head;
        let mut ordinal = 0;
        while let Some(node) = next {
            let slot = &mut out[node.0 as usize];
            if slot.is_some() {
                break;
            }
            *slot = Some((c as u32, ordinal));
            ordinal += 1;
            next = body.cleanup_nodes[node.0 as usize].parent;
        }
    }
    out.into_iter()
        .map(|a| a.unwrap_or((u32::MAX, 0)))
        .collect()
}

/// Temporaries that only ever hold a view: assigned once by a borrowed use of
/// a place under a direct or inherited loan (`_1 = use(Borrow _0@L0)`),
/// never written, moved or lent mutably afterwards. Their reads go to the
/// viewed place itself. A variant part is a pattern binding, read after its
/// match tested the tag;
/// the loan keeps the root's variant, so each later read's tag check holds
/// as the first did (`let Obj(fields) = v else ..`, then a `match` on a key
/// of `for (key, _) in fields`). An element (`_7 = use(Borrow _0[_5]@L0)`, a loop's
/// binding matched on) is a view only while its index stays put: no path from
/// the assignment writes an index local before the view's storage ends.
/// A nested `match` on a pattern binding views a part of another view
/// temporary (`_3 = use(Borrow _1.Ok)`): that part has no loan of its own but
/// stays under its root's, and the enclosing match proved its variant, so it
/// is a view too when its root is one.
fn view_temporaries(body: &tir::Body) -> Vec<Option<tir::PlaceId>> {
    let (assigned, disqualified, _) = view_assignments(body);
    let n = body.locals.len();
    let single = |l: usize| match assigned[l].as_slice() {
        [Some(view)] if view_storage(body, l) && !disqualified[l] => Some(*view),
        _ => None,
    };
    let mut out: Vec<Option<tir::PlaceId>> = (0..n)
        .map(|l| match single(l) {
            Some(ViewSource::Loaned(viewed)) => Some(viewed),
            Some(ViewSource::Indexed(viewed, at))
                if body.places[viewed.0 as usize].loan.is_some()
                    && index_stable(body, l, viewed, at) =>
            {
                Some(viewed)
            }
            _ => None,
        })
        .collect();
    // Nested views join once their root is a view; each pass adds at least
    // one or stops, and a cycle never resolves to a loaned root.
    loop {
        let mut changed = false;
        for l in 0..n {
            if out[l].is_some() {
                continue;
            }
            let (viewed, indexed) = match single(l) {
                Some(ViewSource::Nested(viewed) | ViewSource::Forwarded(viewed)) => (viewed, None),
                Some(ViewSource::Indexed(viewed, at)) => (viewed, Some(at)),
                _ => continue,
            };
            if out[body.places[viewed.0 as usize].root.0 as usize].is_some()
                && indexed.is_none_or(|at| index_stable(body, l, viewed, at))
            {
                out[l] = Some(viewed);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // A view of a view is fine; a cycle is not possible in validated TIR, but
    // never follow one.
    for l in 0..n {
        let mut seen = BTreeSet::new();
        let mut at = l;
        while let Some(p) = out[at] {
            if !seen.insert(at) {
                out[l] = None;
                break;
            }
            at = body.places[p.0 as usize].root.0 as usize;
        }
    }
    out
}

/// Temporaries a view temporary would be but for being assigned a view on
/// more than one path (`if c { &a.f } else { &b.f }`): joined views. A whole
/// move of a joined view (into a `match` scrutinee, or an outer `if` of a
/// nested one) assigns that same view, and a joined view may move only into
/// another.
fn joined_views(body: &tir::Body, aliases: &[Option<tir::PlaceId>]) -> Vec<bool> {
    let (assigned, _, pinned) = view_assignments(body);
    let root = |p: &tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    // Eligibility is separate from selection. A single direct-loan temporary
    // is a valid SSA input when an administrative relocation connects it to a
    // join, even though by itself it would use the static-alias optimization.
    let mut eligible: Vec<bool> = assigned
        .iter()
        .enumerate()
        .map(|(local, sources)| {
            view_storage(body, local)
                && !pinned[local]
                && !sources.is_empty()
                && sources.iter().all(Option::is_some)
        })
        .collect();
    let mut dependencies = vec![];
    for (destination, sources) in assigned.iter().enumerate() {
        for source in sources.iter().flatten() {
            let place = match source {
                ViewSource::Loaned(_) => continue,
                ViewSource::Indexed(place, _) if body.places[place.0 as usize].loan.is_some() => {
                    continue;
                }
                ViewSource::Moved(place)
                | ViewSource::Forwarded(place)
                | ViewSource::Nested(place)
                | ViewSource::Indexed(place, _) => place,
            };
            dependencies.push((
                root(place),
                destination,
                matches!(source, ViewSource::Moved(_)),
            ));
        }
    }
    loop {
        let mut changed = false;
        for &(source, destination, moving) in &dependencies {
            if !eligible[source] && eligible[destination] {
                eligible[destination] = false;
                changed = true;
            }
            if moving && !eligible[destination] && eligible[source] {
                eligible[source] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut joined: Vec<bool> = assigned
        .iter()
        .enumerate()
        .map(|(local, sources)| {
            eligible[local]
                && (sources.len() > 1
                    // Keep a single stable source as an address alias. In
                    // particular, a nested loop element's match must retain
                    // the outer owner's fixed origin after the element's
                    // access loan ends. A real join can still promote this
                    // alias through the dependency closure below.
                    || aliases[local].is_none() && matches!(
                        sources.as_slice(),
                        [Some(
                            ViewSource::Moved(_)
                                | ViewSource::Forwarded(_)
                                | ViewSource::Nested(_)
                                | ViewSource::Indexed(_, _)
                        )]
                    ))
        })
        .collect();
    loop {
        let mut changed = false;
        for &(source, destination, moving) in &dependencies {
            if eligible[source]
                && eligible[destination]
                && (joined[destination] || moving && joined[source])
            {
                if !joined[source] {
                    joined[source] = true;
                    changed = true;
                }
                if !joined[destination] {
                    joined[destination] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    joined
}

fn view_storage(body: &tir::Body, l: usize) -> bool {
    matches!(
        body.locals[l].storage,
        tir::Storage::Temporary | tir::Storage::Local
    )
}

/// A discarded callable view still observes bounds/tags at its assignment.
/// Its borrowed SSA result needs no owning storage or credit. Function locals
/// cannot be index components in validated TIR; every other use is counted here.
fn discarded_callable_reads(body: &tir::Body, types: &TypeInterner) -> Vec<bool> {
    use tir::InstructionKind as K;
    let mut assigned = vec![0u32; body.locals.len()];
    let mut candidate = vec![false; body.locals.len()];
    let mut used = vec![false; body.locals.len()];
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    for rv in &body.rvalues {
        for o in rvalue_operands(rv) {
            used[root(o.place)] = true;
        }
    }
    for block in &body.blocks {
        for i in &block.instructions {
            match &i.kind {
                K::Assign { destination, value } => {
                    let r = root(*destination);
                    assigned[r] += 1;
                    candidate[r] = body.places[destination.0 as usize].projection.is_empty()
                        && body.locals[r].storage == tir::Storage::Temporary
                        && matches!(
                            types.get(body.locals[r].ty).map(|t| &t.kind),
                            Some(hirsel_types::TypeKind::Function(_))
                        )
                        && matches!(&body.rvalues[value.0 as usize], tir::Rvalue::Use(o)
                            if o.usage == UseKind::Borrow);
                }
                K::BindOrigin { source, place, .. } => {
                    used[root(*source)] = true;
                    used[root(*place)] = true;
                }
                K::BindFields { source, fields } => {
                    used[root(*source)] = true;
                    for p in fields {
                        used[root(*p)] = true;
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    for p in [*destination, value.place, *displaced] {
                        used[root(p)] = true;
                    }
                }
                K::Snapshot { destination, value } => {
                    used[root(*destination)] = true;
                    used[root(value.place)] = true;
                }
                K::Defuse { place }
                | K::EndOrigin(place)
                | K::BeginLoan { place, .. }
                | K::Drop { place, .. } => used[root(*place)] = true,
                K::Check { condition, .. } => used[root(condition.place)] = true,
                K::StorageLive(_)
                | K::StorageDead { .. }
                | K::EndScope(_)
                | K::EndLoan { .. }
                | K::RunCleanup(_)
                | K::Charge(_) => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    used[root(*destination)] = true;
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    used[root(value.place)] = true;
                }
                for a in arguments {
                    used[root(a.value.place)] = true;
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => used[root(value.place)] = true,
            _ => {}
        }
    }
    for table in &body.matches {
        used[root(table.scrutinee)] = true;
        for p in &table.patterns {
            used[root(p.place)] = true;
        }
    }
    for node in &body.cleanup_nodes {
        if let tir::CleanupAction::Drop { place, .. } = node.action {
            used[root(place)] = true;
        }
    }
    candidate
        .into_iter()
        .enumerate()
        .map(|(i, yes)| yes && assigned[i] == 1 && !used[i])
        .collect()
}

/// Temporaries and bindings that only ever hold one nullary variant: assigned once, whole, by
/// `Construct Variant` with no fields or by a move or copy of another such temporary, and
/// never written, replaced, lent mutably or a call's destination otherwise. Per local, the
/// variant's ordinal. A `box` of one is a static block (MEM-7).
fn nullary_temporaries(body: &tir::Body) -> Vec<Option<u32>> {
    use tir::InstructionKind as K;
    let n = body.locals.len();
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
    // Each local's assignments, and whether another write rules it out.
    let mut assigned: Vec<Vec<&tir::Rvalue>> = vec![Vec::new(); n];
    let mut written = vec![false; n];
    for block in &body.blocks {
        for i in &block.instructions {
            match &i.kind {
                K::Assign { destination, value } => {
                    let r = root(*destination);
                    if whole(*destination) {
                        assigned[r].push(&body.rvalues[value.0 as usize]);
                    } else {
                        written[r] = true;
                    }
                }
                K::Replace {
                    destination,
                    displaced,
                    ..
                } => {
                    written[root(*destination)] = true;
                    written[root(*displaced)] = true;
                }
                K::Snapshot { destination, .. } => written[root(*destination)] = true,
                K::BeginLoan {
                    place,
                    mutable: true,
                    ..
                } => written[root(*place)] = true,
                _ => {}
            }
        }
        if let tir::Terminator::Call { returns, .. } = &block.terminator
            && let Some(tir::CallReturn { destination, .. }) = returns
        {
            written[root(*destination)] = true;
        }
    }
    for rv in &body.rvalues {
        for o in rvalue_operands(rv) {
            if o.usage == UseKind::Mut {
                written[root(o.place)] = true;
            }
        }
    }
    let temporary = |l: usize| {
        matches!(
            body.locals[l].storage,
            tir::Storage::Temporary | tir::Storage::Local
        ) && !written[l]
            && assigned[l].len() == 1
    };
    let mut out: Vec<Option<u32>> = (0..n)
        .map(|l| match assigned[l].as_slice() {
            [
                tir::Rvalue::Construct {
                    constructor: tir::Constructor::Variant { variant, .. },
                    fields,
                    ..
                },
            ] if temporary(l) && fields.is_empty() => Some(*variant),
            _ => None,
        })
        .collect();
    // Moves and copies of one join once their source is one; each pass adds one or stops.
    loop {
        let mut changed = false;
        for l in 0..n {
            if out[l].is_none()
                && temporary(l)
                && let [tir::Rvalue::Use(o)] = assigned[l].as_slice()
                && matches!(o.usage, UseKind::Take | UseKind::Copy)
                && whole(o.place)
                && let Some(variant) = out[root(o.place)]
            {
                out[l] = Some(variant);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// What a borrowed whole assignment views.
#[derive(Clone, Copy)]
enum ViewSource {
    /// A place under its own loan, with no computed element.
    Loaned(tir::PlaceId),
    /// A loanless part of another local, with no index: a view only while
    /// that local is one.
    Nested(tir::PlaceId),
    /// A whole borrow of another view, preserving its existing permissions.
    Forwarded(tir::PlaceId),
    /// An element under a direct or inherited loan, assigned at `(block,
    /// instruction)`: a view only while its index stays put.
    Indexed(tir::PlaceId, (usize, usize)),
    /// A whole move of another local: the view it holds, if it holds one.
    Moved(tir::PlaceId),
}

/// Per local, the view each assignment gives it (None: not a view), whether
/// any other use rules a view temporary out, and whether a use other than a
/// whole move into another local does (a joined view moves on as one).
#[allow(clippy::type_complexity)]
fn view_assignments(body: &tir::Body) -> (Vec<Vec<Option<ViewSource>>>, Vec<bool>, Vec<bool>) {
    use tir::InstructionKind as K;
    let n = body.locals.len();
    let mut assigned: Vec<Vec<Option<ViewSource>>> = vec![Vec::new(); n];
    let mut disqualified = vec![false; n];
    let mut moved = vec![false; n];
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
    let operand = |o: &tir::Operand, disqualified: &mut Vec<bool>| {
        if matches!(o.usage, UseKind::Take | UseKind::Mut) {
            disqualified[root(o.place)] = true;
        }
    };
    for (b, block) in body.blocks.iter().enumerate() {
        for (k, i) in block.instructions.iter().enumerate() {
            match &i.kind {
                K::Assign { destination, value } => {
                    let r = root(*destination);
                    match &body.rvalues[value.0 as usize] {
                        tir::Rvalue::Use(o)
                            if o.usage == UseKind::Borrow && whole(*destination) =>
                        {
                            let viewed = &body.places[o.place.0 as usize];
                            let indexed = viewed
                                .projection
                                .iter()
                                .any(|p| matches!(p, tir::Projection::Index(_)));
                            assigned[r].push(match viewed.loan {
                                Some(_) if !indexed => Some(ViewSource::Loaned(o.place)),
                                Some(_) => Some(ViewSource::Indexed(o.place, (b, k))),
                                None if indexed => Some(ViewSource::Indexed(o.place, (b, k))),
                                None if !indexed && !viewed.projection.is_empty() => {
                                    Some(ViewSource::Nested(o.place))
                                }
                                None => Some(ViewSource::Forwarded(o.place)),
                            });
                        }
                        tir::Rvalue::Use(o)
                            if o.usage == UseKind::Take
                                && whole(*destination)
                                && whole(o.place) =>
                        {
                            assigned[r].push(Some(ViewSource::Moved(o.place)));
                            moved[root(o.place)] = true;
                        }
                        other => {
                            assigned[r].push(None);
                            for o in rvalue_operands(other) {
                                operand(o, &mut disqualified);
                            }
                        }
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    disqualified[root(*destination)] = true;
                    disqualified[root(*displaced)] = true;
                    operand(value, &mut disqualified);
                }
                K::Snapshot { destination, value } => {
                    disqualified[root(*destination)] = true;
                    operand(value, &mut disqualified);
                }
                K::Check { condition, .. } => operand(condition, &mut disqualified),
                K::BeginLoan {
                    place,
                    mutable: true,
                    ..
                } => disqualified[root(*place)] = true,
                _ => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(tir::CallReturn { destination, .. }) = returns {
                    disqualified[root(*destination)] = true;
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    operand(value, &mut disqualified);
                }
                for a in arguments {
                    operand(&a.value, &mut disqualified);
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => operand(value, &mut disqualified),
            _ => {}
        }
    }
    let pinned = disqualified.clone();
    for (d, m) in disqualified.iter_mut().zip(moved) {
        *d |= m;
    }
    (assigned, disqualified, pinned)
}

/// Whether no path from the assignment of view `l` (at `(block,
/// instruction)`) writes, moves, ends or lends mutably a local indexing
/// `viewed` before `l`'s storage ends, so every read of the view reaches the
/// element it was assigned.
fn index_stable(body: &tir::Body, l: usize, viewed: tir::PlaceId, at: (usize, usize)) -> bool {
    use tir::InstructionKind as K;
    let indices: BTreeSet<usize> = body.places[viewed.0 as usize]
        .projection
        .iter()
        .filter_map(|p| match p {
            tir::Projection::Index(i) => Some(i.0 as usize),
            _ => None,
        })
        .collect();
    let root = |p: &tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let hit = |p: &tir::PlaceId| indices.contains(&root(p));
    let used = |o: &tir::Operand| matches!(o.usage, UseKind::Take | UseKind::Mut) && hit(&o.place);
    // The view's scope and its ancestors: ending any of them ends the view.
    let mut scopes = BTreeSet::new();
    let mut s = Some(body.locals[l].scope);
    while let Some(id) = s {
        if !scopes.insert(id.0) {
            break;
        }
        s = body.scopes[id.0 as usize].parent;
    }
    let mut seen = vec![false; body.blocks.len()];
    let mut work = vec![(at.0, at.1 + 1)];
    while let Some((b, from)) = work.pop() {
        let block = &body.blocks[b];
        let mut ended = false;
        for i in &block.instructions[from.min(block.instructions.len())..] {
            let writes = match &i.kind {
                K::StorageDead { place, .. } | K::Drop { place, .. } if root(place) == l => {
                    ended = true;
                    break;
                }
                K::EndScope(s) if scopes.contains(&s.0) => {
                    ended = true;
                    break;
                }
                K::Assign { destination, value } => {
                    hit(destination)
                        || rvalue_operands(&body.rvalues[value.0 as usize])
                            .into_iter()
                            .any(used)
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => hit(destination) || hit(displaced) || used(value),
                K::Snapshot { destination, value } => hit(destination) || used(value),
                K::Check { condition, .. } => used(condition),
                K::StorageDead { place, .. } | K::Drop { place, .. } => hit(place),
                K::BeginLoan {
                    place,
                    mutable: true,
                    ..
                } => hit(place),
                _ => false,
            };
            if writes {
                return false;
            }
        }
        if ended {
            continue;
        }
        let writes = match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                returns.as_ref().is_some_and(|r| hit(&r.destination))
                    || matches!(callee, tir::Callee::Indirect { value, .. } if used(value))
                    || arguments.iter().any(|a| used(&a.value))
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => used(value),
            _ => false,
        };
        if writes {
            return false;
        }
        for next in body.successors(&block.terminator) {
            let n = next.0 as usize;
            if !seen[n] {
                seen[n] = true;
                work.push((n, 0));
            }
        }
    }
    true
}

/// A joined view parameter is covered by the union of the loans of the views
/// it joins (joined ones transitively) and declares the least of them; one of
/// plain views is plain.
fn join_view_holdings(values: &mut [ValueInfo], incoming: &BTreeMap<ValueId, Vec<ValueId>>) {
    let mut loans: BTreeMap<ValueId, BTreeSet<LoanId>> = BTreeMap::new();
    loop {
        let mut changed = false;
        for (&p, args) in incoming {
            let mut union = loans.get(&p).cloned().unwrap_or_default();
            for a in args {
                match (incoming.contains_key(a), values[a.index()].holding) {
                    (true, _) => union.extend(loans.get(a).into_iter().flatten()),
                    (false, Holding::Borrowed(l)) => {
                        union.insert(l);
                    }
                    (false, _) => {}
                }
            }
            if loans.get(&p) != Some(&union) {
                loans.insert(p, union);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (p, union) in loans {
        if let Some(&least) = union.first() {
            values[p.index()].holding = Holding::Borrowed(least);
        }
    }
}

fn rvalue_operands(value: &tir::Rvalue) -> Vec<&tir::Operand> {
    match value {
        tir::Rvalue::Use(o) | tir::Rvalue::Unary { value: o, .. } => vec![o],
        tir::Rvalue::Binary { left, right, .. } => vec![left, right],
        tir::Rvalue::Construct {
            constructor: tir::Constructor::SortedMap { by },
            fields,
            ..
        } => std::iter::once(by)
            .chain(fields.iter().map(|field| &field.value))
            .collect(),
        tir::Rvalue::Construct { fields, .. } => fields.iter().map(|f| &f.value).collect(),
        tir::Rvalue::Closure { captures, .. } => captures.iter().collect(),
        _ => Vec::new(),
    }
}

fn lir_binary(op: hirsel_syntax::BinaryOp) -> Option<BinaryOp> {
    use hirsel_syntax::BinaryOp as S;
    Some(match op {
        S::Add => BinaryOp::Add,
        S::Sub => BinaryOp::Sub,
        S::Mul => BinaryOp::Mul,
        S::Div => BinaryOp::Div,
        S::Rem => BinaryOp::Rem,
        S::Shl => BinaryOp::Shl,
        S::Shr => BinaryOp::Shr,
        S::BitAnd => BinaryOp::BitAnd,
        S::BitOr => BinaryOp::BitOr,
        S::BitXor => BinaryOp::BitXor,
        S::Eq => BinaryOp::Eq,
        S::Ne => BinaryOp::Ne,
        S::Lt => BinaryOp::Lt,
        S::Le => BinaryOp::Le,
        S::Gt => BinaryOp::Gt,
        S::Ge => BinaryOp::Ge,
        S::And | S::Or | S::Range | S::RangeInclusive => return None,
    })
}

pub(crate) fn repr_of(kind: &LayoutKind) -> ValueRepr {
    match kind {
        LayoutKind::Unit => ValueRepr::Unit,
        LayoutKind::Scalar(k) => ValueRepr::Scalar(*k),
        LayoutKind::Block { .. } | LayoutKind::Box { .. } | LayoutKind::Shared { .. } => {
            ValueRepr::Block
        }
        LayoutKind::Callable { .. } => ValueRepr::Callable,
        LayoutKind::Handle
        | LayoutKind::HostCapability { .. }
        | LayoutKind::SealedRegion { .. } => ValueRepr::Handle,
        LayoutKind::Path => ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::U64),
        LayoutKind::Arena => ValueRepr::Arena,
        LayoutKind::PayloadReceipt { .. } => ValueRepr::PayloadReceipt,
        LayoutKind::Header => ValueRepr::Token,
        _ => ValueRepr::Aggregate,
    }
}

/// Whether `path` is the core library's `module.name`.
fn core_operation(path: &DefPath, module: &str, name: &str) -> bool {
    let item = &path.0;
    item.package.name_is("core") && item.modules == [module] && item.name == name
}

/// A length-prefixed FlatFormatV0 encoding: a dynamic-size Flat record whose
/// one field, its content, follows the u64 byte-length prefix (offset 8) or
/// the prefix and the element count of a list of variable elements (16).
fn flat_prefixed(
    layouts: &dyn crate::library::builder::Layouts,
    layout: LayoutId,
) -> Option<(u64, LayoutId)> {
    let r = layouts.record(layout);
    match (&r.kind, &r.size, r.fields.as_slice()) {
        (LayoutKind::Flat, SizeSpec::Dynamic(_), [only]) if matches!(only.offset, 8 | 16) => {
            Some((only.offset, only.layout))
        }
        _ => None,
    }
}

/// The element of a flat sequence encoding (List, Array, String, Bytes).
fn flat_sequence(
    layouts: &dyn crate::library::builder::Layouts,
    layout: LayoutId,
) -> Option<LayoutId> {
    match flat_prefixed(layouts, layout)? {
        (16, element) => Some(element),
        (_, element) => flat_fixed(layouts, element).then_some(element),
    }
}

fn flat_fixed(layouts: &dyn crate::library::builder::Layouts, layout: LayoutId) -> bool {
    matches!(layouts.record(layout).size, SizeSpec::Fixed(_))
}

impl Importer<'_> {
    fn site(&self, span: Span) -> SiteRange {
        SiteRange {
            source: self
                .source_anchors
                .iter()
                .position(|a| *a == span.anchor)
                .map_or(u32::MAX, |i| i as u32),
            start: span.start,
            end: span.end,
        }
    }
}

enum Step2 {
    Skip,
    Inline(Inst),
    Drop(DropStep),
}

struct DropStep {
    address: AddressId,
    if_live: bool,
    anchor: CleanupAnchorId,
    slot: SlotId,
    steps: Vec<Step>,
}

impl DropStep {
    fn parts(&self) -> (AddressId, bool, CleanupAnchorId) {
        (self.address, self.if_live, self.anchor)
    }
    fn apply(&self, im: &mut Importer<'_>) {
        im.state.take(self.slot, &self.steps);
        if self.steps.is_empty() {
            im.versions[self.slot.index()] = None;
        }
    }
}

fn st_maybe(s: &SlotState) -> bool {
    s.init != Tri::Yes || s.moved.values().any(|t| t.may())
}

/// The address of a computed-place loan before its `BeginLoan` is emitted.
const UNPREPARED: AddressId = AddressId(u32::MAX);

/// What a call is declared to do: everything but control (callees are opaque
/// here; effect inference narrows later stages).
pub(crate) const CALL_EFFECTS: Effects = Effects::READS
    .union(Effects::WRITES)
    .union(Effects::OWNS)
    .union(Effects::MAY_FAIL)
    .union(Effects::ALLOCATES)
    .union(Effects::OBSERVABLE)
    .union(Effects::BARRIER);
