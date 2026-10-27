//! P1 to P6: one Ready program from a validated snapshot. Production validates
//! the emitted unit at Ready; harnesses also validate each intermediate stage. A
//! rejected unit, a pass error and an unavailable feature are all
//! `NativeError`s that name the stage and the definition.
use super::adapt;
use hirsel_core_contract::Manifest;
use hirsel_engine::ValidatedNativeInput;
use hirsel_intern::timings::{self, Count, Interval, Phase};
use hirsel_lir::discharge::Discharge;
use hirsel_lir::ir::LirUnit;
use hirsel_lir::layout_types::{LayoutId, LayoutTable, TargetSpec};
use hirsel_lir::stage::{InputIdentity, Ready, Stage, ValidatedUnit};
use hirsel_lir::validate::{Inputs, Rejected};
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceRole, Program};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// When the independent LIR validator runs. Ready is mandatory in both modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Validation {
    /// Production: validate each emitted unit once, after all lowering.
    Ready,
    /// Harnesses: also validate every intermediate stage to localize pass faults.
    EveryStage,
}

/// Compiler choices and explicit launch inputs. Compiler choices and the hash
/// seed preserve observations; capability seeds select their own inputs.
#[derive(Clone, Debug)]
pub struct Options {
    /// Production judges Ready once; harnesses retain the stage ladder.
    pub validation: Validation,
    /// Explicit launch inputs; placement and capability seeds are independent.
    pub launch: hirsel_run_schema::Inputs,
    pub target: TargetSpec,
    /// Place compiler-local allocations in internal regions.
    pub regions: bool,
    /// Cancel a copy against the release insertion put after it.
    pub elide: bool,
    /// Drop specialization, reuse tokens and kept stores.
    pub reuse: bool,
    /// Walk variable flat sequences linearly instead of rescanning prefixes.
    pub flat_walk: bool,
    /// Explicit capture specialization; production enables it for release builds.
    pub scoped_specialization: bool,
    /// PS-13 checks accepted evidence licenses removing (`--proofs`).
    pub discharge: Discharge,
}

impl Options {
    /// The harness configuration: every optimization and every stage validator on.
    pub fn new(target: TargetSpec) -> Self {
        Self {
            validation: Validation::EveryStage,
            launch: Default::default(),
            target,
            regions: true,
            elide: true,
            reuse: true,
            flat_walk: true,
            scoped_specialization: true,
            discharge: Discharge::default(),
        }
    }
}

/// The native stage that stopped a build.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum StageName {
    /// P0: the snapshot's operation manifest.
    Manifest,
    /// P1/P2: instantiation, self-tail normalization, layout and SSA import.
    Import,
    /// P3.
    Placement,
    /// P4.
    Perceus,
    /// P5.
    Reuse,
    /// P6: cleanup lowering and legalization.
    Ready,
    /// The checked heap executor.
    Heap,
    /// P7.
    Emission,
    /// P8.
    Link,
    Launch,
}

impl StageName {
    pub fn name(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Import => "import",
            Self::Placement => "placement",
            Self::Perceus => "perceus",
            Self::Reuse => "reuse",
            Self::Ready => "ready",
            Self::Heap => "heap",
            Self::Emission => "emission",
            Self::Link => "link",
            Self::Launch => "launch",
        }
    }
}

/// A native tool outcome: the stage, the definition when one unit is at
/// fault, whether the stage's validator (rather than its pass) refused, and
/// the reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeError {
    pub stage: StageName,
    pub definition: Option<DefPath>,
    pub validator: bool,
    pub message: String,
}

impl NativeError {
    pub(crate) fn new(stage: StageName, message: impl Into<String>) -> Self {
        Self {
            stage,
            definition: None,
            validator: false,
            message: message.into(),
        }
    }
    pub(crate) fn of(stage: StageName, definition: &DefPath, message: impl Into<String>) -> Self {
        Self {
            definition: Some(definition.clone()),
            ..Self::new(stage, message)
        }
    }
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "native {}", self.stage.name())?;
        if self.validator {
            f.write_str(" validator")?;
        }
        if let Some(definition) = &self.definition {
            write!(f, " in {}", definition.0.name)?;
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for NativeError {}

/// A whole program at Ready: every unit sealed by the Ready validator against
/// this table, target, manifest and input.
pub struct ReadyProgram {
    /// Planned provenance: source definitions (including derived forms) name their
    /// source owner; library instances name the common semantic runtime bundle.
    pub(super) semantic_owners: BTreeMap<DefPath, Option<DefPath>>,
    /// The exact planned unit identities checked when binding the semantic input.
    pub(super) semantic_units: Option<BTreeSet<(DefPath, ContentHash)>>,
    /// Validation policy retained for subsequent inspection and emission.
    pub validation: Validation,
    pub operations: hirsel_layout::operation::OperationRegistry,
    pub launch: hirsel_run_schema::Inputs,
    pub input: InputIdentity,
    pub target: TargetSpec,
    pub table: LayoutTable,
    /// The content key of each layout of `table` (`hirsel_lir::layout_keys`), which names
    /// the glue symbols units import.
    pub keys: hirsel_lir::LayoutKeys,
    pub manifest: Manifest,
    /// Every independently validated Ready unit selected for inspection.
    pub units: Vec<ValidatedUnit<Ready>>,
    pub ahead_environments: BTreeMap<ContentHash, Option<LayoutId>>,
    /// What program assembly reads from each unit (`hirsel_lir::fragment`), in unit order.
    pub summaries: Vec<hirsel_lir::fragment::UnitSummary>,
    /// Callable-target specializations and their known bodies, available in
    /// each calling partition without changing their source failure anchors.
    pub inline_instances: std::collections::BTreeSet<ContentHash>,
    /// The definitions some unit names as a C callback (PROG-4).
    pub callbacks: BTreeSet<DefPath>,
    /// The definitions some unit makes an Entry value of (RUN-3): each one's
    /// function-reference thunk and full path, which the program descriptor lists.
    pub entries: BTreeMap<String, String>,
    /// Checked source facts, supplied through the engine's public admission API.
    pub(crate) callee_facts: Option<hirsel_rt::program::CalleeMetadata>,
    /// Per resource layout, the instance digest of its drop body's unit.
    pub finalizers: BTreeMap<LayoutId, ContentHash>,
    /// Instance digest of each non-generic function, for entry lookup.
    pub functions: BTreeMap<DefPath, ContentHash>,
    pub(crate) launches: BTreeMap<DefPath, super::link::LaunchKind>,
    pub(crate) revisions: BTreeMap<DefPath, hirsel_run_schema::ProgramRevision>,
    pub(crate) exit_reports: BTreeMap<DefPath, super::exit_report::Report>,
    /// The static data of every constant a unit reads (CT-2).
    pub statics: Vec<hirsel_lir::statics::StaticData>,
    /// The planned PS-13 sites some unit removed, as `validate_discharge`
    /// confirmed.
    pub discharged: Discharge,
    /// The externs' libraries, bound with the revisions (PROG-4).
    pub foreign_links: Vec<hirsel_engine::LinkInput>,
    /// The published data ABI, when foreign objects link (MEM-7, PROG-4).
    pub data_abi: Option<super::c_abi::DataAbi>,
    /// Executables refuse to record or replay (PROG-4).
    pub foreign_unreplayable: bool,
    /// Each extern's dispatch word (S-13, `EmitInputs::foreign`).
    pub foreign_dispatch: BTreeMap<DefPath, u64>,
    /// The launch entries that reach an escape from capability checking, which
    /// `admit.safe@1` refuses (CAP-6).
    pub(crate) escaping: BTreeSet<DefPath>,
    /// Emit fuel checks (`EmitInputs::fuel`), so the executable enforces a fuel budget
    /// (`budgets.limits@2`); without them it refuses one.
    pub fuel: bool,
    /// The failure table a program that isolates registers at launch (FAIL-2), bound with
    /// the revisions.
    pub(crate) failure_table: Option<super::observe::FailureTable>,
}

impl ReadyProgram {
    /// The checked heap program with independently planned invocation evidence,
    /// including each derived definition's original source owner.
    pub fn heap_program(&self) -> hirsel_heapcheck::Program<hirsel_lir::stage::Ready> {
        // Reference identities come from validated TIR and the independent planner's
        // derived-definition ownership, never from candidate units or their events.
        let mut invocations = self.operations.invocations.clone();
        for (definition, owner) in &self.semantic_owners {
            if let Some(reference) = owner
                .as_ref()
                .and_then(|owner| self.operations.invocations.get(owner))
            {
                invocations.insert(definition.clone(), reference.clone());
            }
        }
        hirsel_heapcheck::Program {
            invocation_bodies: self.operations.invocation_bodies.clone(),
            pretransfer_adapters: self.operations.pretransfer_adapters.clone(),
            invocations,
            target: self.target.clone(),
            layouts: self.table.clone(),
            units: self.raw_units(),
            statics: self.statics.clone(),
        }
    }

    /// Recheck the linked selection against the identities sealed by semantic binding.
    pub(crate) fn check_semantic_units(
        &self,
        objects: &[hirsel_lir::stage::ObjectArtifact],
    ) -> Result<(), NativeError> {
        let Some(covered) = &self.semantic_units else {
            return Err(NativeError::new(
                StageName::Link,
                "native units have no semantic binding",
            ));
        };
        for unit in &self.summaries {
            if !covered.contains(&(unit.definition.clone(), unit.instance)) {
                return Err(NativeError::of(
                    StageName::Link,
                    &unit.definition,
                    "linked unit outside the semantic closure covered by the run identity",
                ));
            }
        }
        let definitions: BTreeSet<_> = covered.iter().map(|(path, _)| path).collect();
        for export in objects.iter().flat_map(|object| &object.exports) {
            if export.kind == hirsel_lir::stage::ObjectSymbolKind::Function
                && export
                    .definition
                    .as_ref()
                    .is_none_or(|path| !definitions.contains(path))
            {
                return Err(NativeError::new(
                    StageName::Link,
                    format!(
                        "linked symbol `{}` outside the semantic closure covered by the run identity",
                        export.name
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Whether the program builds tracked arena blocks (MEM-6): it opens a bounded arena,
    /// which only `arena.bounded`'s recovery does.
    pub fn tracking(&self) -> bool {
        self.summaries.iter().any(|s| s.recovers)
    }

    /// The semantic program revision a run of `entry` binds to.
    pub fn revision(&self, entry: &DefPath) -> Option<&hirsel_run_schema::ProgramRevision> {
        self.revisions.get(entry)
    }
    /// Attach checked semantic code selection, independently of pass options and topology.
    /// Adapted-program clients retain and supply their validated source input here.
    pub fn bind_revisions(&mut self, input: &ValidatedNativeInput) -> Result<(), NativeError> {
        self.bind_source(&super::root_source::LinkSource::snapshot(input))
    }
    pub(crate) fn bind_source(
        &mut self,
        input: &super::root_source::LinkSource<'_>,
    ) -> Result<(), NativeError> {
        if self.input != input.input {
            return Err(NativeError::new(
                StageName::Launch,
                "semantic binding belongs to another input",
            ));
        }
        for unit in &self.summaries {
            let covered = match self.semantic_owners.get(&unit.definition) {
                Some(Some(owner)) => input.semantic.semantic_contains(owner),
                Some(None) => unit.origin == hirsel_lir::ir::UnitOrigin::Library,
                None => false,
            };
            if !covered {
                return Err(NativeError::of(
                    StageName::Link,
                    &unit.definition,
                    "native unit outside the semantic closure covered by the run identity",
                ));
            }
        }
        self.semantic_units = Some(
            self.summaries
                .iter()
                .map(|unit| (unit.definition.clone(), unit.instance))
                .collect(),
        );
        self.revisions = self
            .launches
            .keys()
            .map(|entry| {
                input
                    .semantic
                    .semantic_program_revision(entry)
                    .map(|revision| (entry.clone(), revision))
                    .map_err(|error| {
                        NativeError::new(StageName::Launch, format!("semantic revision: {error:?}"))
                    })
            })
            .collect::<Result<_, _>>()?;
        self.foreign_links = input.foreign_links.clone();
        if !self.foreign_links.is_empty() {
            self.data_abi = Some(super::c_abi::data_abi(
                input.types,
                input.signatures,
                input
                    .representations
                    .iter()
                    .map(|(path, record)| (path, record.as_ref())),
                &self.target,
            )?);
        }
        self.foreign_unreplayable = input.foreign_unreplayable;
        self.foreign_dispatch = super::foreign::dispatch(input);
        super::foreign::check_callbacks(self, input)?;
        self.escaping = self
            .launches
            .keys()
            .filter(|entry| input.semantic.unsafe_reach(entry).is_some())
            .cloned()
            .collect();
        self.exit_reports = self
            .launches
            .keys()
            .filter(|entry| self.functions.contains_key(*entry))
            .map(|entry| {
                super::exit_report::derive(input, self, entry).map(|report| (entry.clone(), report))
            })
            .collect::<Result<_, _>>()?;
        self.failure_table = Some({
            super::observe::failure_table(
                self,
                &super::observe::Sites::from_anchors(input.sources, input.anchors.clone()),
            )
        });
        Ok(())
    }
    /// Bind source-checked runner facts to the same native Entry table.
    pub(crate) fn bind_callees(
        &mut self,
        metadata: hirsel_rt::program::CalleeMetadata,
    ) -> Result<(), NativeError> {
        for path in metadata.facts.keys() {
            if !self.entries.values().any(|entry| entry == path) {
                return Err(NativeError::new(
                    StageName::Launch,
                    format!("callee facts outside native Entry table: {path}"),
                ));
            }
        }
        self.callee_facts = Some(metadata);
        Ok(())
    }

    /// Put `unit` at `index` in place of the unit there, with its summary.
    pub fn replace_unit(&mut self, index: usize, unit: ValidatedUnit<Ready>) -> Result<(), String> {
        self.summaries[index] = hirsel_lir::fragment::UnitSummary::of(
            unit.unit(),
            &self.table,
            &self.keys,
            &self.manifest,
        )?;
        self.summaries[index].add_static_references(&self.statics);
        self.units[index] = unit;
        Ok(())
    }
    /// The Ready units, as the heap executor and dumps read them.
    pub fn raw_units(&self) -> Vec<LirUnit<Ready>> {
        self.units.iter().map(|u| u.unit().clone()).collect()
    }
}

/// Workers for the per-unit stages: `HIRSEL_JOBS` when it is a positive count, else the
/// host's available parallelism. Nothing a build produces depends on the count.
fn unit_jobs() -> hirsel_intern::jobs::Jobs {
    std::env::var("HIRSEL_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse::<usize>().ok())
        .filter(|&jobs| jobs > 0)
        .map_or_else(
            hirsel_intern::jobs::Jobs::available,
            hirsel_intern::jobs::Jobs::new,
        )
}

/// Run `work` as `phase`.
pub(crate) fn timed<T>(phase: Phase, work: impl FnOnce() -> T) -> T {
    let _timing = timings::phase(phase);
    work()
}

/// A unit sealed at its stage by lir's non-generic per-stage validator, or
/// every violation it reported.
pub(crate) fn seal<S: Stage>(
    stage: StageName,
    validate: impl FnOnce() -> Result<ValidatedUnit<S>, Box<Rejected<S>>>,
) -> Result<ValidatedUnit<S>, NativeError> {
    timed(Phase::LirValidate, validate).map_err(|rejected| {
        let mut kinds: Vec<String> = rejected
            .violations
            .iter()
            .map(|v| match &v.kind {
                // Name the operation: its table index means nothing to a reader.
                hirsel_lir::validate::ViolationKind::UnsupportedOperation(op) => {
                    let path = rejected
                        .unit
                        .tables
                        .operations
                        .get(op.index())
                        .map(|id| &id.definition.0);
                    let name = path.map_or(String::new(), |p| {
                        p.modules
                            .iter()
                            .chain(std::iter::once(&p.name))
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(".")
                    });
                    format!("UnsupportedOperation: no native entry for operation {name:?}")
                }
                kind => format!("{kind:?} at {:?}", v.site),
            })
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds.truncate(8);
        NativeError {
            validator: true,
            ..NativeError::of(stage, &rejected.unit.header.definition, kinds.join("; "))
        }
    })
}

/// Validate the immutable program table once, including builds with no units.
fn preflight(context: &hirsel_lir::validate::ValidationContext<'_>) -> Result<(), NativeError> {
    timed(Phase::LirValidate, || context.preflight()).map_err(|errors| NativeError {
        validator: true,
        ..NativeError::new(StageName::Import, format!("layout table: {errors:?}"))
    })
}

/// An intermediate stage stays raw in production. Only the Ready validator
/// manufactures the seal emission requires; harnesses retain the full ladder.
pub(crate) fn checkpoint<S: Stage>(
    stage: StageName,
    unit: LirUnit<S>,
    inputs: &Inputs<'_>,
    context: &hirsel_lir::validate::ValidationContext<'_>,
    validation: Validation,
) -> Result<LirUnit<S>, NativeError> {
    match validation {
        Validation::Ready => Ok(unit),
        Validation::EveryStage => {
            seal(stage, || context.validate(unit, inputs)).map(ValidatedUnit::into_raw)
        }
    }
}

/// Lower one imported unit through placement, RC, reuse and Ready. Production
/// validates Ready once; harnesses use the identical passes with stage checkpoints.
pub(crate) fn lower_unit(
    unit: LirUnit<hirsel_lir::stage::Value>,
    id: hirsel_rc::instance::InstanceId,
    options: &Options,
    instances: &hirsel_rc::instance::Instances,
    finalizers: &Finalizers,
    inputs: &Inputs<'_>,
    validation: &hirsel_lir::validate::ValidationContext<'_>,
) -> Result<ValidatedUnit<Ready>, NativeError> {
    let facts = LocalRcFacts {
        placement: hirsel_rc::placement::Options::for_instance(options.regions, instances, id),
        proofs: Proofs::for_instance(instances, id),
        guard: None,
    };
    lower_local_unit(unit, options, &facts, finalizers, inputs, validation)
}

/// The complete local source facts the existing RC passes consume. A native
/// query records the independently accepted callable/placement inputs that
/// established them, instead of retaining every instance's source body.
#[derive(Clone, Debug)]
pub(crate) struct LocalRcFacts {
    pub placement: hirsel_rc::placement::Options,
    pub proofs: Proofs,
    pub guard: Option<hirsel_rc::seal_demand::UnitDemand>,
}

impl LocalRcFacts {
    pub fn canonical(&self) -> Vec<u8> {
        let mut bytes = vec![
            u8::from(self.placement.regions),
            u8::from(self.placement.noalloc),
            u8::from(self.proofs.noalloc),
        ];
        bytes.extend_from_slice(&(self.proofs.unique.len() as u64).to_le_bytes());
        for parameter in &self.proofs.unique {
            bytes.extend_from_slice(&parameter.to_le_bytes());
        }
        bytes
    }
}

/// One local miss transformation, with a reached input table and finalizers.
/// No other unit's Value or RC result is an implicit input. Ready validation
/// remains independent of every placement/RC/Reuse producer fact.
pub(crate) fn lower_local_unit(
    unit: LirUnit<hirsel_lir::stage::Value>,
    options: &Options,
    facts: &LocalRcFacts,
    finalizers: &Finalizers,
    inputs: &Inputs<'_>,
    validation: &hirsel_lir::validate::ValidationContext<'_>,
) -> Result<ValidatedUnit<Ready>, NativeError> {
    let raw = lower_local_raw(unit, options, facts, finalizers, inputs, validation)?;
    let ready = seal(StageName::Ready, || validation.validate(raw, inputs))?;
    super::emit::check_unit_runtime_calls(ready.unit(), inputs.layouts, inputs.manifest)?;
    Ok(ready)
}

/// The sole local placement/RC/Reuse/Ready transformation. The enclosing query
/// performs independent final C and publication at its own completion boundary.
/// Harnesses retain the same intermediate stage checkpoints.
pub(crate) fn lower_local_raw(
    unit: LirUnit<hirsel_lir::stage::Value>,
    options: &Options,
    facts: &LocalRcFacts,
    finalizers: &Finalizers,
    inputs: &Inputs<'_>,
    validation: &hirsel_lir::validate::ValidationContext<'_>,
) -> Result<LirUnit<Ready>, NativeError> {
    let table = inputs.layouts;
    let definition = unit.header.definition.clone();
    let fail = |stage, message: String| NativeError::of(stage, &definition, message);
    timings::count(Count::Units);
    let value = checkpoint(
        StageName::Import,
        unit,
        inputs,
        validation,
        options.validation,
    )?;
    let mut placed = timed(Phase::Place, || {
        hirsel_rc::placement::place(value, table, &facts.placement)
    })
    .map_err(|e| fail(StageName::Placement, e.to_string()))?;
    if let Some(guard) = &facts.guard {
        guard
            .apply(&mut placed)
            .map_err(|e| fail(StageName::Placement, e))?;
    }
    let placed = checkpoint(
        StageName::Placement,
        placed,
        inputs,
        validation,
        options.validation,
    )?;
    let mut unit = timed(Phase::Rc, || {
        hirsel_rc::insert::insert(placed, table, &facts.proofs)
    })
    .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
    if options.elide {
        unit = timed(Phase::Rc, || hirsel_rc::elide::elide(unit, table))
            .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
    }
    let rc = checkpoint(
        StageName::Perceus,
        unit,
        inputs,
        validation,
        options.validation,
    )?;
    let reuse = hirsel_rc::reuse::Options {
        reuse: options.reuse,
    };
    let reused = timed(Phase::Reuse, || {
        hirsel_rc::reuse::reuse_indexed(rc, &reuse, validation.index())
    })
    .map_err(|e| fail(StageName::Reuse, e.to_string()))?;
    let reused = checkpoint(
        StageName::Reuse,
        reused,
        inputs,
        validation,
        options.validation,
    )?;
    let ready = timed(Phase::Ready, || {
        hirsel_rc::ready::ready_indexed(reused, finalizers, validation.index())
    })
    .map_err(|e| fail(StageName::Ready, e.to_string()))?;
    Ok(ready)
}

/// Lower a validated snapshot to a Ready program for `options.target`.
pub fn build(
    validated: &ValidatedNativeInput,
    options: &Options,
) -> Result<ReadyProgram, NativeError> {
    let plan = (
        timings::interval(Interval::Plan),
        timings::phase(Phase::Plan),
    );
    let manifest = adapt::manifest(validated.snapshot())
        .map_err(|e| NativeError::new(StageName::Manifest, e))?;
    let program = adapt::program(validated);
    drop(plan);
    let mut ready = build_program(&program, manifest, options)?;
    let _plan = (
        timings::interval(Interval::Plan),
        timings::phase(Phase::Plan),
    );
    ready.bind_revisions(validated)?;
    Ok(ready)
}

/// `build` over an already adapted program.
pub fn build_program(
    program: &Program,
    manifest: Manifest,
    options: &Options,
) -> Result<ReadyProgram, NativeError> {
    build_program_roots(program, manifest, options, &[])
}

pub(super) fn build_program_roots(
    program: &Program,
    manifest: Manifest,
    options: &Options,
    roots: &[hirsel_rc::library::Request],
) -> Result<ReadyProgram, NativeError> {
    let _interval = timings::interval(Interval::Units);
    let target = &options.target;
    let import = timings::phase(Phase::Import);
    let mut lowered = hirsel_rc::import::lower_with_roots(
        program,
        target,
        &hirsel_rc::flat_walk::Options {
            enabled: options.flat_walk,
        },
        &options.discharge,
        options.scoped_specialization,
        roots,
    )
    .map_err(|e| NativeError::new(StageName::Import, e.to_string()))?;
    let discharged = timed(Phase::LirValidate, || {
        discharged(&mut lowered, &options.discharge)
    })?;
    timed(Phase::LirValidate, || {
        hirsel_lir::validate::check_result_arena_calls(&lowered.units, &lowered.table)
            .and_then(|()| hirsel_lir::validate::check_scoped_links(&lowered.units, &lowered.table))
    })
    .map_err(|e| NativeError::new(StageName::Import, e))?;
    let operations = lowered.operation_registry(program);
    let value_units = std::mem::take(&mut lowered.units);
    let ahead_environments = hirsel_lir::ahead::environments(&value_units);
    hirsel_intern::memory::checkpoint("global Value complete");
    let table = &lowered.table;
    drop(import);
    let inputs = Inputs {
        memo: Default::default(),
        discharge: Some(&lowered.native_discharge),
        operations: &operations,
        target,
        layouts: table,
        manifest: &manifest,
    };
    let instances = &lowered.instances;
    let statics = std::mem::take(&mut lowered.statics);
    let validation = hirsel_lir::validate::ValidationContext::new(&inputs);
    preflight(&validation)?;
    let finalizers = Finalizers::planned(instances, table);
    let tasks = value_units.into_iter().zip(&lowered.unit_origins).collect();
    // Inspection and heap builds use the same complete per-unit pipeline as object
    // builds. With one worker map runs inline; no stage collects whole-program RC.
    let units = hirsel_intern::jobs::map(
        hirsel_intern::jobs::Jobs::ONE,
        tasks,
        |(unit, _)| unit.instructions.len() as u64,
        |(unit, &id)| {
            let inputs = Inputs {
                memo: Default::default(),
                discharge: Some(&lowered.native_discharge),
                operations: &operations,
                target,
                layouts: table,
                manifest: &manifest,
            };
            lower_unit(
                unit,
                id,
                options,
                instances,
                &finalizers,
                &inputs,
                &validation,
            )
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let functions = instances
        .ids()
        .filter(|id| instances.roles[id.index()] == InstanceRole::Function)
        .map(|id| {
            let unit = units[id.index()].unit();
            (unit.header.definition.clone(), unit.header.instance)
        })
        .collect();
    let callbacks = units
        .iter()
        .flat_map(|unit| callbacks(unit.unit()))
        .collect();
    let entries = units.iter().flat_map(|unit| entries(unit.unit())).collect();
    let keys = hirsel_lir::LayoutKeys::new(&lowered.table);
    let summaries = units
        .iter()
        .map(|u| {
            hirsel_lir::fragment::UnitSummary::of(u.unit(), &lowered.table, &keys, &manifest)
                .map(|mut summary| {
                    summary.add_static_references(&statics);
                    summary
                })
                .map_err(|e| NativeError::of(StageName::Emission, &u.unit().header.definition, e))
        })
        .collect::<Result<_, _>>()?;
    Ok(ReadyProgram {
        semantic_owners: lowered.semantic_owners.clone(),
        semantic_units: None,
        validation: options.validation,
        callbacks,
        entries,
        callee_facts: None,
        operations,
        launch: options.launch.clone(),
        revisions: BTreeMap::new(),
        exit_reports: BTreeMap::new(),
        launches: program
            .signatures
            .iter()
            .filter_map(|(p, s)| {
                super::link::launch_kind(&program.types, s).map(|k| (p.clone(), k))
            })
            .collect(),
        input: program.input.clone(),
        target: target.clone(),
        finalizers: finalizers
            .0
            .iter()
            .map(|(layout, f)| (*layout, f.instance))
            .collect(),
        functions,
        keys,
        table: lowered.table,
        manifest,
        units,
        ahead_environments,
        summaries,
        inline_instances: lowered.inline_instances,
        statics,
        discharged,
        foreign_links: Vec::new(),
        data_abi: None,
        foreign_unreplayable: false,
        foreign_dispatch: BTreeMap::new(),
        escaping: BTreeSet::new(),
        fuel: false,
        failure_table: None,
    })
}

/// PS-13: every discharged unit removed exactly its planned sites, and every
/// planned site of a definition with units was removed by one of them; any
/// difference is a tool error. Returns the removed sites.
fn discharged(
    lowered: &mut hirsel_rc::import::Lowered,
    plan: &Discharge,
) -> Result<Discharge, NativeError> {
    let mut removed = Discharge::default();
    for unit in std::mem::take(&mut lowered.discharged) {
        let sites = plan.sites(&unit.definition).cloned().unwrap_or_default();
        let here =
            hirsel_lir::validate::validate_discharge(&unit.reference, &unit.discharged, &sites)
                .map_err(|errors| NativeError {
                    validator: true,
                    ..NativeError::of(
                        StageName::Import,
                        &unit.definition,
                        format!("PS-13 check removal: {}", errors.join("; ")),
                    )
                })?;
        removed.0.entry(unit.definition).or_default().extend(here);
    }
    for (definition, sites) in &removed.0 {
        if let Some(missed) = plan.0[definition].difference(sites).next() {
            return Err(NativeError {
                validator: true,
                ..NativeError::of(
                    StageName::Import,
                    definition,
                    format!(
                        "PS-13 check removal: accepted {:?} at body {} block {} statement {} has no check in the native program",
                        missed.form, missed.body, missed.block, missed.statement
                    ),
                )
            });
        }
    }
    Ok(removed)
}

/// The definitions `unit` makes callable values of, an Entry's among them, with their
/// thunks: `FunctionRef`s at any but a pointer scalar.
fn entries(unit: &hirsel_lir::ir::LirUnit<Ready>) -> impl Iterator<Item = (String, String)> + '_ {
    use hirsel_lir::ir::{Inst, ValueRepr};
    unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::FunctionRef { result, symbol }
            if unit.types[unit.values[result.index()].ty.index()].repr
                != ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Pointer) =>
        {
            let symbol = &unit.tables.symbols[symbol.index()];
            Some((
                hirsel_lir::emission::function_thunk_name(
                    &symbol.definition,
                    symbol.instance.as_ref(),
                ),
                symbol.definition.to_string(),
            ))
        }
        _ => None,
    })
}

/// The definitions `unit` names as C callbacks: `FunctionRef`s at a pointer scalar (PROG-4).
pub(crate) fn callbacks<S>(
    unit: &hirsel_lir::ir::LirUnit<S>,
) -> impl Iterator<Item = DefPath> + '_ {
    use hirsel_lir::ir::{Inst, ValueRepr};
    unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::FunctionRef { result, symbol }
            if unit.types[unit.values[result.index()].ty.index()].repr
                == ValueRepr::Scalar(hirsel_lir::layout_types::ScalarKind::Pointer) =>
        {
            Some(unit.tables.symbols[symbol.index()].definition.clone())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Operational budgets stop below LANG-10 and do not start user cleanup.
    #[test]
    fn native_logical_depth_does_not_turn_operational_budgets_into_failures() {
        use crate::native::{self, Backend, OptLevel, Options, Partition, Sites, Tape};
        use hirsel_run_schema::{BudgetKind, Budgets, IoGrants, LaunchKind, RunOutcome};
        hirsel_engine::with_compiler_stack(|| {
            let source = r#"
resource type Trace {stdout:Stdout}
drop(x) {let _=out.print(x.stdout,"cleanup\n")}
fn descend(n:u64)->u64 {if n==0u64 {0u64}else {1u64+descend(n-1u64)}}
pub fn main(rt:Io)->Result[(),GrantError] {
 let trace=Trace(stdout:io.stdout(rt)?)
 let mut xs:List[u64]=[]
 list.reserve(mut xs,32u64)
 list.push(mut xs,7u64)
 let _=descend(40u64)+list.len(xs)
 Ok(())
}
"#;
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            );
            let file = engine.sources.add("budget.hir", source.into()).unwrap();
            let checked =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = hirsel_engine::entry_path(&hirsel_engine::default_context());
            let config = hirsel_engine::RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference = hirsel_engine::run_observed_snapshot(
                &mut engine,
                &config,
                hirsel_engine::SimulationConfig::default(),
            )
            .unwrap();
            let reference =
                native::Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.output, b"cleanup\n");
            assert!(reference.failures.is_empty());
            let raw = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(raw).unwrap();
            let scratch = super::super::executable::Scratch::new().unwrap();
            let tools =
                native::Toolchain::for_matrix(&scratch.0, &native::trace_roots_matrix()).unwrap();
            let mut ready = native::build(
                &validated,
                &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            )
            .unwrap();
            ready.fuel = true;
            let sites = Sites::new(validated.snapshot());
            for backend in [Backend::Llvm, Backend::Cranelift] {
                let objects = native::emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    None,
                )
                .unwrap();
                for kind in [None, Some(BudgetKind::Fuel), Some(BudgetKind::Memory)] {
                    let dir = scratch
                        .0
                        .join(format!("logical-depth-budget-{backend:?}-{kind:?}"));
                    std::fs::create_dir_all(&dir).unwrap();
                    let mut record = hirsel_run_config::RunConfig::for_program(
                        ready.revision(&entry).unwrap(),
                        LaunchKind::Main,
                    );
                    record.inputs = ready.launch.clone();
                    record.io = Some(IoGrants {
                        stdout: Some("stdout.bytes@1".into()),
                        ..Default::default()
                    });
                    record.budgets = kind.map(|kind| Budgets {
                        provider: "budgets.limits@2".into(),
                        fuel: (kind == BudgetKind::Fuel).then_some(10),
                        memory_bytes: (kind == BudgetKind::Memory).then_some(1),
                    });
                    let run = record.validated().unwrap();
                    let runnable =
                        native::link_runnable(&ready, &entry, &objects, &tools, &dir, Some(&run))
                            .unwrap();
                    assert_eq!(
                        hirsel_run_config::RunConfig::read_path(&dir.join("run.json")).unwrap(),
                        run
                    );
                    let tape = dir.join("budget.tape");
                    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
                    let launched = super::super::link::launch_live(
                        &runnable,
                        &ready.target,
                        &tools,
                        &dir,
                        Tape::Live {
                            record: Some(&tape),
                            revocations: None,
                        },
                        super::super::link::LiveRun {
                            run: &run,
                            trace_events: None,
                            inherit: false,
                            stdout: &mut stdout,
                            stderr: &mut stderr,
                        },
                    )
                    .unwrap();
                    let recording = hirsel_rt::launch::parse_recording(
                        &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        recording.bound_run().unwrap(),
                        run,
                        "the runtime consumed a different budget profile"
                    );
                    assert_eq!(launched.stdout, stdout);
                    let observation = native::observe(&launched, &ready, &sites).unwrap();
                    assert_eq!(
                        observation.budget, kind,
                        "{backend:?}/{kind:?}: {observation:?}"
                    );
                    assert!(observation.failures.is_empty(), "{observation:?}");
                    if let Some(kind) = kind {
                        assert_eq!(
                            observation.outcome(&sites, vec![]).unwrap(),
                            RunOutcome::BudgetExceeded(kind)
                        );
                        assert!(
                            observation.output.is_empty(),
                            "user cleanup ran: {observation:?}"
                        );
                    } else {
                        assert_eq!(observation, reference);
                    }
                    std::fs::remove_dir_all(dir).unwrap();
                }
            }
        });
    }

    #[test]
    fn semantic_binding_rejects_unplanned_units_and_object_exports() {
        hirsel_engine::with_compiler_stack(|| {
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                crate::compiler_adapters(),
            );
            let file = engine.sources.add("semantic.hir", "fn helper()->u64 { 7u64 }\npub fn main()->Result[(),()] { let _ = helper(); Ok(()) }\n".into()).unwrap();
            let output =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            let input = engine.import_native_input(input).unwrap();
            let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
            let mut ready = build(&input, &options).unwrap();
            let mut objects = (0..ready.units.len())
                .map(|owner| {
                    super::super::emit::emit_registered(
                        &ready,
                        super::super::Backend::Cranelift,
                        hirsel_lir::emission::OptLevel::None,
                        |selected| selected == owner,
                        &BTreeSet::new(),
                        false,
                        None,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            ready.check_semantic_units(&objects).unwrap();
            let definition = ready.summaries[0].definition.clone();
            ready.summaries[0].definition = DefPath::core("unselected");
            assert!(ready.check_semantic_units(&objects).is_err());
            ready.summaries[0].definition = definition.clone();
            objects[0].exports[0].definition = Some(DefPath::core("unselected"));
            assert!(ready.check_semantic_units(&objects).is_err());
            ready
                .semantic_owners
                .insert(definition, Some(DefPath::core("unselected")));
            assert!(ready.bind_revisions(&input).is_err());
        });
    }

    #[test]
    fn unit_pipeline_is_identical_at_every_worker_count_and_validation_policy() {
        let run = || {
            hirsel_engine::with_compiler_stack(|| {
                let mut engine = hirsel_engine::QueryEngine::new(
                    hirsel_engine::default_environment(),
                    crate::compiler_adapters(),
                );
                let source = r#"
fn helper(x: u64) -> u64 { x + 1u64 }
fn main() -> u64 {
    let built: String = "value"
    let s: Shared[String] = shared.freeze(take built)
    let t = copy s
    match shared.thaw(take t) {
        Ok(text) => helper(1u64),
        Error(root) => helper(2u64),
    }
}
"#;
                let file = engine.sources.add("pipeline.hir", source.into()).unwrap();
                let checked = hirsel_engine::check_progress(
                    &mut engine,
                    file,
                    &hirsel_engine::default_context(),
                )
                .unwrap();
                assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
                let exported = engine.export_native_input().unwrap();
                let validated = engine.import_native_input(exported).unwrap();
                let program = adapt::program(&validated);
                let target = TargetSpec::x86_64_unknown_linux_gnu();
                let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
                assert!(lowered.units.len() > 1);
                let manifest = adapt::manifest(validated.snapshot()).unwrap();
                let operations = lowered.operation_registry(&program);
                let inputs = Inputs {
                    memo: Default::default(),
                    discharge: Some(&lowered.native_discharge),
                    operations: &operations,
                    target: &target,
                    layouts: &lowered.table,
                    manifest: &manifest,
                };
                let context = hirsel_lir::validate::ValidationContext::new(&inputs);
                preflight(&context).unwrap();
                let finalizers = Finalizers::planned(&lowered.instances, &lowered.table);
                let mut expected = None;
                for policy in [Validation::EveryStage, Validation::Ready] {
                    let mut options = Options::new(target.clone());
                    options.validation = policy;
                    for jobs in [1, 2, 8] {
                        let tasks = lowered
                            .units
                            .iter()
                            .cloned()
                            .zip(lowered.unit_origins.iter().copied())
                            .collect();
                        let results = hirsel_intern::jobs::map(
                            hirsel_intern::jobs::Jobs::new(jobs),
                            tasks,
                            |(u, _)| u.instructions.len() as u64,
                            |(unit, id)| {
                                let inputs = Inputs {
                                    memo: Default::default(),
                                    discharge: Some(&lowered.native_discharge),
                                    operations: &operations,
                                    target: &target,
                                    layouts: &lowered.table,
                                    manifest: &manifest,
                                };
                                lower_unit(
                                    unit,
                                    id,
                                    &options,
                                    &lowered.instances,
                                    &finalizers,
                                    &inputs,
                                    &context,
                                )
                                .map(|unit| {
                                    (
                                        hirsel_codec::to_text(unit.unit()),
                                        unit.receipt().raw().clone(),
                                    )
                                })
                            },
                        )
                        .into_iter()
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap();
                        if let Some(expected) = &expected {
                            assert_eq!(expected, &results, "jobs={jobs}, policy={policy:?}");
                        } else {
                            expected = Some(results);
                        }
                    }
                }
                format!("{:?}", expected.unwrap())
            })
        };
        if let Some(outputs) = hirsel_intern::under_salts(
            "native::stages::tests::unit_pipeline_is_identical_at_every_worker_count_and_validation_policy",
            &[1, 2, 0x5eed],
            run,
        ) {
            assert_eq!(outputs.len(), 3);
            assert!(
                outputs.windows(2).all(|pair| pair[0] == pair[1]),
                "salt changed complete units or receipts"
            );
        }
    }
    #[test]
    fn http_reject_lowering_has_valid_projections() {
        hirsel_engine::with_compiler_stack(|| {
            use hirsel_engine::DefinitionQueries;
            let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json");
            let mut package = crate::COMPILER
                .load_package_declarations(&manifest)
                .unwrap();
            let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package.package.clone(),
                modules: vec!["http".into()],
                name: "http_server_reject".into(),
            });
            // This private root is absent from ordinary program reachability.
            // Keep its independent V projection regression covered.
            package.engine.lower_to_tir(&root).unwrap();
        });
    }

    // A contract helper's closure is addressed and called in the same singleton
    // object. An untyped LLVM address declaration conflicts with the bound call.
    #[test]
    fn ordinary_store_delete_has_one_consistent_entry_abi() {
        hirsel_engine::with_compiler_stack(|| {
            use hirsel_engine::DefinitionQueries;
            let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json");
            let mut package = crate::COMPILER
                .load_package_declarations(&manifest)
                .unwrap();
            let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package.package.clone(),
                modules: vec!["store".into()],
                name: "tx_delete".into(),
            });
            let input = package.engine.native_input_for(&[root]).unwrap();
            for backend in [
                super::super::Backend::Cranelift,
                super::super::Backend::Llvm,
            ] {
                for opt in [
                    hirsel_lir::emission::OptLevel::None,
                    hirsel_lir::emission::OptLevel::Default,
                    hirsel_lir::emission::OptLevel::Speed,
                ] {
                    let options = super::super::Options::new(
                        hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu(),
                    );
                    let ready = super::build(&input, &options)
                        .unwrap_or_else(|e| panic!("{backend:?} {opt:?}: {e}"));
                    let sources = super::super::profile_sources(input.snapshot());
                    super::super::emit_backend_objects(
                        &ready,
                        backend,
                        opt,
                        super::super::Partition::Singleton,
                        false,
                        (opt == hirsel_lir::emission::OptLevel::None).then_some(&sources),
                    )
                    .unwrap_or_else(|e| panic!("{backend:?} {opt:?}: {e}"));
                }
            }
        });
    }
}

/// Program entries retained in portable unit summaries, including object-store hits.
pub(super) fn summary_entries(
    summaries: &[hirsel_lir::fragment::UnitSummary],
) -> BTreeMap<String, String> {
    summaries
        .iter()
        .flat_map(|s| s.glue.entries.iter())
        .map(|(path, instance)| {
            (
                hirsel_lir::emission::function_thunk_name(path, instance.as_ref()),
                path.to_string(),
            )
        })
        .collect()
}
