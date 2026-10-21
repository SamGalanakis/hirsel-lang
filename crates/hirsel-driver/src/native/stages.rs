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
    /// PS-13 checks accepted evidence licenses removing (`--proofs`).
    pub discharge: Discharge,
    /// Emit each unit's object during the build through an object store
    /// (`super::units`), instead of leaving every Ready unit to the link.
    pub objects: Option<super::units::UnitObjects>,
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
            discharge: Discharge::default(),
            objects: None,
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
    pub operations: hirsel_layout::operation::OperationRegistry,
    pub launch: hirsel_run_schema::Inputs,
    pub input: InputIdentity,
    pub target: TargetSpec,
    pub table: LayoutTable,
    /// The content key of each layout of `table` (`hirsel_lir::layout_keys`), which names
    /// the glue symbols units import.
    pub keys: hirsel_lir::LayoutKeys,
    pub manifest: Manifest,
    /// Every Ready unit; empty when the build emitted its units itself
    /// (`Options::objects`), whose objects are `unit_objects`.
    pub units: Vec<ValidatedUnit<Ready>>,
    /// What program assembly reads from each unit (`hirsel_lir::fragment`), in unit order.
    pub summaries: Vec<hirsel_lir::fragment::UnitSummary>,
    /// The units' objects when the build emitted them, with the backend and optimization
    /// level they were emitted for.
    pub unit_objects: Option<UnitObjectArtifacts>,
    /// Callable-target specializations and their known bodies, available in
    /// each calling partition without changing their source failure anchors.
    pub inline_instances: std::collections::BTreeSet<ContentHash>,
    /// The definitions some unit names as a C callback (PROG-4).
    pub callbacks: BTreeSet<DefPath>,
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
    /// Executables refuse to record or replay (PROG-4).
    pub foreign_unreplayable: bool,
    /// The launch entries that reach an escape from capability checking, which
    /// `admit.safe@1` refuses (CAP-6).
    pub(crate) escaping: BTreeSet<DefPath>,
    /// Emit fuel checks (`EmitInputs::fuel`), so the executable enforces a fuel budget
    /// (`budgets.limits@1`); without them it refuses one.
    pub fuel: bool,
    /// The failure table a program that isolates registers at launch (FAIL-2), bound with
    /// the revisions.
    pub(crate) failure_table: Option<super::observe::FailureTable>,
}

/// The objects a build emitted for its units (`Options::objects`).
#[derive(Clone, Debug)]
pub struct UnitObjectArtifacts {
    pub backend: super::Backend,
    pub opt: hirsel_lir::emission::OptLevel,
    /// The objects record debug intervals.
    pub debug: bool,
    pub objects: Vec<hirsel_lir::stage::ObjectArtifact>,
    /// How many of them the object store held.
    pub hits: usize,
}

impl ReadyProgram {
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
        if self.input != adapt::input_identity(input.snapshot()) {
            return Err(NativeError::new(
                StageName::Launch,
                "semantic binding belongs to another input",
            ));
        }
        self.revisions = self
            .launches
            .keys()
            .map(|entry| {
                input
                    .semantic_program_revision(entry)
                    .map(|revision| (entry.clone(), revision))
                    .map_err(|error| {
                        NativeError::new(StageName::Launch, format!("semantic revision: {error:?}"))
                    })
            })
            .collect::<Result<_, _>>()?;
        self.foreign_links = input.snapshot().foreign_links.clone();
        self.foreign_unreplayable = input.snapshot().foreign_unreplayable;
        self.escaping = self
            .launches
            .keys()
            .filter(|entry| input.unsafe_reach(entry).is_some())
            .cloned()
            .collect();
        self.exit_reports = self
            .launches
            .keys()
            .filter(|entry| self.functions.contains_key(*entry))
            .map(|entry| {
                super::exit_report::derive(input.snapshot(), self, entry)
                    .map(|report| (entry.clone(), report))
            })
            .collect::<Result<_, _>>()?;
        self.failure_table = Some({
            super::observe::failure_table(self, &super::observe::Sites::new(input.snapshot()))
        });
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
        self.units[index] = unit;
        Ok(())
    }
    /// The Ready units, as the heap executor and dumps read them.
    pub fn raw_units(&self) -> Vec<LirUnit<Ready>> {
        self.units.iter().map(|u| u.unit().clone()).collect()
    }
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
    let placement = hirsel_rc::placement::Options::for_instance(options.regions, instances, id);
    let placed = timed(Phase::Place, || {
        hirsel_rc::placement::place(value, table, &placement)
    })
    .map_err(|e| fail(StageName::Placement, e.to_string()))?;
    let placed = checkpoint(
        StageName::Placement,
        placed,
        inputs,
        validation,
        options.validation,
    )?;
    let proofs = Proofs::for_instance(instances, id);
    let mut unit = timed(Phase::Rc, || {
        hirsel_rc::insert::insert(placed, table, &proofs)
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
    let ready = seal(StageName::Ready, || validation.validate(ready, inputs))?;
    super::emit::check_unit_runtime_calls(ready.unit(), table, inputs.manifest)?;
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
    let _interval = timings::interval(Interval::Units);
    let target = &options.target;
    let import = timings::phase(Phase::Import);
    let mut lowered = hirsel_rc::import::lower_discharging(
        program,
        target,
        &hirsel_rc::flat_walk::Options {
            enabled: options.flat_walk,
        },
        &options.discharge,
    )
    .map_err(|e| NativeError::new(StageName::Import, e.to_string()))?;
    let discharged = timed(Phase::LirValidate, || {
        discharged(&mut lowered, &options.discharge)
    })?;
    timed(Phase::LirValidate, || {
        hirsel_lir::validate::check_result_arena_calls(&lowered.units, &lowered.table)
    })
    .map_err(|e| NativeError::new(StageName::Import, e))?;
    let value_units = std::mem::take(&mut lowered.units);
    let table = &lowered.table;
    let operations = program.operation_registry();
    drop(import);
    let inputs = Inputs {
        discharge: Some(&lowered.native_discharge),
        operations: &operations,
        target,
        layouts: table,
        manifest: &manifest,
    };
    let instances = &lowered.instances;
    let statics = std::mem::take(&mut lowered.statics);
    if let Some(objects) = &options.objects {
        return build_units(
            program,
            &manifest,
            options,
            objects,
            &operations,
            &lowered,
            value_units,
            statics,
            discharged,
        )
        .map(|built| built.finish(program, manifest, operations, options, lowered));
    }
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
    let keys = hirsel_lir::LayoutKeys::new(&lowered.table);
    let summaries = units
        .iter()
        .map(|u| {
            hirsel_lir::fragment::UnitSummary::of(u.unit(), &lowered.table, &keys, &manifest)
                .map_err(|e| NativeError::of(StageName::Emission, &u.unit().header.definition, e))
        })
        .collect::<Result<_, _>>()?;
    Ok(ReadyProgram {
        callbacks,
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
        summaries,
        unit_objects: None,
        inline_instances: lowered.inline_instances,
        statics,
        discharged,
        foreign_links: Vec::new(),
        foreign_unreplayable: false,
        escaping: BTreeSet::new(),
        fuel: false,
        failure_table: None,
    })
}

/// A program whose units the per-unit pipeline lowered or loaded (`Options::objects`),
/// before it owns the lowering's tables.
struct BuiltUnits {
    summaries: Vec<hirsel_lir::fragment::UnitSummary>,
    objects: UnitObjectArtifacts,
    keys: hirsel_lir::LayoutKeys,
    callbacks: BTreeSet<DefPath>,
    statics: Vec<hirsel_lir::statics::StaticData>,
    discharged: Discharge,
}

/// The per-unit pipeline (`super::units`) over a lowered program's Value units.
#[allow(clippy::too_many_arguments)]
fn build_units(
    program: &Program,
    manifest: &Manifest,
    options: &Options,
    objects: &super::units::UnitObjects,
    operations: &hirsel_layout::operation::OperationRegistry,
    lowered: &hirsel_rc::import::Lowered,
    units: Vec<LirUnit<hirsel_lir::stage::Value>>,
    statics: Vec<hirsel_lir::statics::StaticData>,
    discharged: Discharge,
) -> Result<BuiltUnits, NativeError> {
    let table = &lowered.table;
    let keys = hirsel_lir::LayoutKeys::new(table);
    let finalizers = Finalizers::planned(&lowered.instances, table);
    let callbacks: BTreeSet<DefPath> = units.iter().flat_map(callbacks).collect();
    let tracking = units.iter().any(|u| {
        u.terminators
            .iter()
            .any(|t| matches!(t, hirsel_lir::ir::Terminator::Recover { .. }))
    });
    let empty = BTreeSet::new();
    let inputs = Inputs {
        discharge: Some(&lowered.native_discharge),
        operations,
        target: &options.target,
        layouts: table,
        manifest,
    };
    let validation = hirsel_lir::validate::ValidationContext::new(&inputs);
    preflight(&validation)?;
    let emit = hirsel_lir::emission::EmitInputs {
        index: Some(validation.index()),
        input: &program.input,
        target: &options.target,
        layouts: table,
        keys: &keys,
        manifest,
        statics: &statics,
        inline_instances: &lowered.inline_instances,
        available_externally: &empty,
        callbacks: !callbacks.is_empty(),
        tracking,
        opt: objects.opt,
        measurement: false,
        fuel: objects.fuel,
    };
    let headers: Vec<(DefPath, ContentHash)> = units
        .iter()
        .map(|u| (u.header.definition.clone(), u.header.instance))
        .collect();
    let lowered_units = super::units::lower(
        &super::units::Program {
            options,
            objects,
            table,
            keys: &keys,
            operations,
            discharge: Some(&lowered.native_discharge),
            validation: &validation,
            emit: &emit,
            instances: &lowered.instances,
            finalizers: &finalizers,
            origins: &lowered.unit_origins,
        },
        units,
    )?;
    // A template's summary is in the fragment of every unit that copies it.
    let mut templates = BTreeMap::new();
    for unit in &lowered_units {
        if let Some(fragment) = &unit.fragment {
            for template in &fragment.templates {
                templates
                    .entry((template.definition.clone(), template.instance))
                    .or_insert_with(|| template.clone());
            }
        }
    }
    let mut summaries = Vec::with_capacity(lowered_units.len());
    let mut artifacts = Vec::new();
    let mut hits = 0;
    for (unit, header) in lowered_units.into_iter().zip(headers) {
        match unit.fragment {
            Some(fragment) => {
                hits += usize::from(unit.hit);
                summaries.push(fragment.summary);
                artifacts.push(fragment.object);
            }
            // A template no object copies is not in the program's code.
            None => summaries.extend(templates.remove(&header)),
        }
    }
    Ok(BuiltUnits {
        summaries,
        objects: UnitObjectArtifacts {
            backend: objects.backend,
            opt: objects.opt,
            debug: objects.debug,
            objects: artifacts,
            hits,
        },
        keys,
        callbacks,
        statics,
        discharged,
    })
}

impl BuiltUnits {
    fn finish(
        self,
        program: &Program,
        manifest: Manifest,
        operations: hirsel_layout::operation::OperationRegistry,
        options: &Options,
        lowered: hirsel_rc::import::Lowered,
    ) -> ReadyProgram {
        let instances = &lowered.instances;
        let finalizers = Finalizers::planned(instances, &lowered.table);
        let functions = instances
            .ids()
            .filter(|id| instances.roles[id.index()] == InstanceRole::Function)
            .map(|id| {
                let view = instances.view(id);
                (view.key.definition().clone(), view.key.digest())
            })
            .collect();
        ReadyProgram {
            callbacks: self.callbacks,
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
            target: options.target.clone(),
            finalizers: finalizers
                .0
                .iter()
                .map(|(layout, f)| (*layout, f.instance))
                .collect(),
            functions,
            keys: self.keys,
            table: lowered.table,
            manifest,
            units: Vec::new(),
            summaries: self.summaries,
            unit_objects: Some(self.objects),
            inline_instances: lowered.inline_instances,
            statics: self.statics,
            discharged: self.discharged,
            foreign_links: Vec::new(),
            foreign_unreplayable: false,
            escaping: BTreeSet::new(),
            fuel: options.objects.as_ref().is_some_and(|o| o.fuel),
            failure_table: None,
        }
    }
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

/// The definitions `unit` names as C callbacks: `FunctionRef`s at a pointer scalar (PROG-4).
fn callbacks<S>(unit: &hirsel_lir::ir::LirUnit<S>) -> impl Iterator<Item = DefPath> + '_ {
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

    #[test]
    fn unit_pipeline_is_identical_at_every_worker_count_and_validation_policy() {
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
            let checked =
                hirsel_engine::check_progress(&mut engine, file, &hirsel_engine::default_context())
                    .unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let exported = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(exported).unwrap();
            let program = adapt::program(&validated);
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let lowered = hirsel_rc::import::lower(&program, &target).unwrap();
            assert!(lowered.units.len() > 1);
            let manifest = adapt::manifest(validated.snapshot()).unwrap();
            let operations = program.operation_registry();
            let inputs = Inputs {
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
        });
    }
}
