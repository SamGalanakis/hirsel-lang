//! P1 to P6: one Ready program from a validated snapshot. Each stage's output
//! is used only after that stage's validator in `hirsel-lir` accepted it; a
//! rejected unit, a pass error and an unavailable feature are all
//! `NativeError`s that name the stage and the definition.
use super::adapt;
use hirsel_core_contract::Manifest;
use hirsel_engine::ValidatedNativeInput;
use hirsel_lir::discharge::Discharge;
use hirsel_lir::ir::LirUnit;
use hirsel_lir::layout_types::{LayoutId, LayoutTable, TargetSpec};
use hirsel_lir::stage::{InputIdentity, Ready, Stage, ValidatedUnit};
use hirsel_lir::validate::{
    Inputs, Rejected, validate_placed, validate_rc, validate_ready, validate_reuse, validate_value,
};
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceRole, Program};
use hirsel_syntax::{ContentHash, DefPath};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Compiler choices and explicit launch inputs. Compiler choices and the hash
/// seed preserve observations; capability seeds select their own inputs.
#[derive(Clone, Debug)]
pub struct Options {
    /// Explicit launch inputs; placement and capability seeds are independent.
    pub launch: hirsel_run_config::Inputs,
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
}

impl Options {
    /// The default configuration for a target: every optimization on.
    pub fn new(target: TargetSpec) -> Self {
        Self {
            launch: Default::default(),
            target,
            regions: true,
            elide: true,
            reuse: true,
            flat_walk: true,
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
    fn of(stage: StageName, definition: &DefPath, message: impl Into<String>) -> Self {
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
    pub launch: hirsel_run_config::Inputs,
    pub input: InputIdentity,
    pub target: TargetSpec,
    pub table: LayoutTable,
    pub manifest: Manifest,
    pub units: Vec<ValidatedUnit<Ready>>,
    /// Callable-target specializations and their known bodies, available in
    /// each calling partition without changing their source failure anchors.
    pub inline_instances: std::collections::BTreeSet<ContentHash>,
    /// Per resource layout, the instance digest of its drop body's unit.
    pub finalizers: BTreeMap<LayoutId, ContentHash>,
    /// Instance digest of each non-generic function, for entry lookup.
    pub functions: BTreeMap<DefPath, ContentHash>,
    pub(crate) launches: BTreeMap<DefPath, super::link::LaunchKind>,
    pub(crate) revisions: BTreeMap<DefPath, hirsel_run_config::ProgramRevision>,
    /// The static data of every constant a unit reads (CT-2).
    pub statics: Vec<hirsel_lir::statics::StaticData>,
    /// The planned PS-13 sites some unit removed, as `validate_discharge`
    /// confirmed.
    pub discharged: Discharge,
    /// Planned definitions that kept every check (`Lowered::undischarged`).
    pub undischarged: BTreeSet<DefPath>,
}

impl ReadyProgram {
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
        Ok(())
    }
    /// The Ready units, as the heap executor and dumps read them.
    pub fn raw_units(&self) -> Vec<LirUnit<Ready>> {
        self.units.iter().map(|u| u.unit().clone()).collect()
    }
}

/// A unit sealed at its stage by lir's non-generic per-stage validator, or every
/// violation it reported.
fn seal<S: Stage>(
    stage: StageName,
    validated: Result<ValidatedUnit<S>, Box<Rejected<S>>>,
) -> Result<ValidatedUnit<S>, NativeError> {
    validated.map_err(|rejected| {
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

/// Lower a validated snapshot to a Ready program for `options.target`.
pub fn build(
    validated: &ValidatedNativeInput,
    options: &Options,
) -> Result<ReadyProgram, NativeError> {
    let manifest = adapt::manifest(validated.snapshot())
        .map_err(|e| NativeError::new(StageName::Manifest, e))?;
    let program = adapt::program(validated);
    let mut ready = build_program(&program, manifest, options)?;
    ready.bind_revisions(validated)?;
    Ok(ready)
}

/// `build` over an already adapted program.
pub fn build_program(
    program: &Program,
    manifest: Manifest,
    options: &Options,
) -> Result<ReadyProgram, NativeError> {
    let target = &options.target;
    let mut lowered = hirsel_rc::import::lower_discharging(
        program,
        target,
        &hirsel_rc::flat_walk::Options {
            enabled: options.flat_walk,
        },
        &options.discharge,
    )
    .map_err(|e| NativeError::new(StageName::Import, e.to_string()))?;
    let discharged = discharged(&mut lowered, &options.discharge)?;
    hirsel_lir::validate::check_result_arena_calls(&lowered.units, &lowered.table)
        .map_err(|e| NativeError::new(StageName::Import, e))?;
    let table = &lowered.table;
    let operations = program.operation_registry();
    let inputs = Inputs {
        operations: &operations,
        target,
        layouts: table,
        manifest: &manifest,
    };
    let instances = &lowered.instances;
    let statics = lowered.statics;
    let mut rc = Vec::with_capacity(lowered.units.len());
    for (index, unit) in lowered.units.into_iter().enumerate() {
        let id = lowered.unit_origins[index];
        let definition = unit.header.definition.clone();
        let fail = |stage, message: String| NativeError::of(stage, &definition, message);
        let value = seal(StageName::Import, validate_value(unit, &inputs))?.into_raw();
        let placement = hirsel_rc::placement::Options::for_instance(options.regions, instances, id);
        let placed = hirsel_rc::placement::place(value, table, &placement)
            .map_err(|e| fail(StageName::Placement, e.to_string()))?;
        let placed = seal(StageName::Placement, validate_placed(placed, &inputs))?.into_raw();
        let proofs = Proofs::for_instance(instances, id);
        let mut unit = hirsel_rc::insert::insert(placed, table, &proofs)
            .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
        if options.elide {
            // Elision rewrites a validated unit, so insertion's output is
            // sealed first and elision's again below.
            unit = seal(StageName::Perceus, validate_rc(unit, &inputs))?.into_raw();
            unit = hirsel_rc::elide::elide(unit, table)
                .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
        }
        rc.push(seal(StageName::Perceus, validate_rc(unit, &inputs))?.into_raw());
    }
    let finalizers = Finalizers::new(instances, &rc);
    let mut units = Vec::with_capacity(rc.len());
    for unit in rc {
        let definition = unit.header.definition.clone();
        let fail = |stage, message: String| NativeError::of(stage, &definition, message);
        let reuse = hirsel_rc::reuse::Options {
            reuse: options.reuse,
        };
        let reused = hirsel_rc::reuse::reuse(unit, table, &reuse)
            .map_err(|e| fail(StageName::Reuse, e.to_string()))?;
        let reused = seal(StageName::Reuse, validate_reuse(reused, &inputs))?.into_raw();
        let ready = hirsel_rc::ready::ready(reused, table, &finalizers)
            .map_err(|e| fail(StageName::Ready, e.to_string()))?;
        units.push(seal(StageName::Ready, validate_ready(ready, &inputs))?);
    }
    let functions = instances
        .ids()
        .filter(|id| instances.roles[id.index()] == InstanceRole::Function)
        .map(|id| {
            let unit = units[id.index()].unit();
            (unit.header.definition.clone(), unit.header.instance)
        })
        .collect();
    Ok(ReadyProgram {
        operations,
        launch: options.launch.clone(),
        revisions: BTreeMap::new(),
        launches: program
            .signatures
            .iter()
            .filter_map(|(p, s)| super::link::launch_kind(s).map(|k| (p.clone(), k)))
            .collect(),
        input: program.input.clone(),
        target: target.clone(),
        finalizers: finalizers
            .0
            .iter()
            .map(|(layout, f)| (*layout, f.instance))
            .collect(),
        functions,
        table: lowered.table,
        manifest,
        units,
        inline_instances: lowered.inline_instances,
        statics,
        discharged,
        undischarged: lowered.undischarged,
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
