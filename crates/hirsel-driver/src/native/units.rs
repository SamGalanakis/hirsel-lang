//! The per-unit native pipeline (design §1e). After import and the whole-program Value
//! passes, each unit is placed, counted, reused, readied, validated and emitted on its own,
//! in parallel and with no barrier: the finalizer facts cleanup reads come from the plan
//! (`Finalizers::planned`). Each unit's object is keyed before placement
//! (`hirsel_lir::objects::object_key`), so a unit the object store holds skips every stage
//! after the Value passes, and program assembly reads its fragment.
//!
//! A unit's object holds the unit and, `linkonce_odr`, the library and known callable
//! bodies it reaches, as a whole-program object holds every one it calls: a template is
//! never an object of its own. Their keys enter the unit's key. A missed unit lowers each
//! template it needs once, whichever unit asks first.

use super::emit::Backend;
use super::stages::{NativeError, Options, StageName, seal, timed};
use hirsel_intern::timings::{self, Count, Phase};
use hirsel_lir::LayoutKeys;
use hirsel_lir::emission::{EmitInputs, OptLevel};
use hirsel_lir::fragment::{UnitFragment, UnitSummary};
use hirsel_lir::ir::{Callee, Inst, LirUnit, Symbol, UnitOrigin};
use hirsel_lir::layout_types::{LayoutId, LayoutTable};
use hirsel_lir::objects::{ObjectStore, UnitKey, object_key};
use hirsel_lir::stage::{Ready, ValidatedUnit, Value};
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::{InstanceId, Instances};
use hirsel_syntax::ContentHash;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

/// How a build emits its units itself, one object per unit, through an object store.
#[derive(Clone)]
pub struct UnitObjects {
    pub store: Arc<dyn ObjectStore>,
    pub backend: Backend,
    pub opt: OptLevel,
    /// Record debug intervals, which a debug build's link turns into line tables.
    pub debug: bool,
    /// Emit fuel checks (`EmitInputs::fuel`).
    pub fuel: bool,
    /// How many units lower and emit at once.
    pub jobs: hirsel_intern::jobs::Jobs,
}

impl std::fmt::Debug for UnitObjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnitObjects")
            .field("backend", &self.backend)
            .field("opt", &self.opt)
            .field("debug", &self.debug)
            .field("fuel", &self.fuel)
            .field("jobs", &self.jobs.get())
            .finish_non_exhaustive()
    }
}

impl UnitObjects {
    /// The store at `name` (`hirsel_lir::objects::objects_by_name`) in the partition of
    /// `target`, `backend` and `opt`.
    pub fn by_name(
        name: &str,
        target: &hirsel_lir::layout_types::TargetSpec,
        backend: Backend,
        opt: OptLevel,
    ) -> Result<Arc<dyn ObjectStore>, String> {
        let partition = hirsel_lir::objects::partition(
            &target.triple,
            backend.name(),
            &format!("{opt:?}").to_lowercase(),
        );
        hirsel_lir::objects::objects_by_name(name, None, &partition)
    }
}

/// The per-unit pipeline a build of `target` selects: the object store `HIRSEL_OBJECTS`
/// names (`none@1`, `dir@1` or `sim@1`) on `HIRSEL_JOBS` workers (default: the host's
/// parallelism), or none (the whole-program link) without it.
pub(crate) fn selected(
    target: &hirsel_lir::layout_types::TargetSpec,
    backend: Backend,
    opt: OptLevel,
    debug: bool,
    fuel: bool,
) -> Result<Option<UnitObjects>, NativeError> {
    let Some(name) = std::env::var_os("HIRSEL_OBJECTS") else {
        return Ok(None);
    };
    let name = name.to_string_lossy();
    let store = UnitObjects::by_name(&name, target, backend, opt)
        .map_err(|e| NativeError::new(StageName::Emission, format!("HIRSEL_OBJECTS: {e}")))?;
    let jobs = match std::env::var("HIRSEL_JOBS") {
        Ok(text) => hirsel_intern::jobs::Jobs::new(text.parse().map_err(|_| {
            NativeError::new(
                StageName::Emission,
                format!("HIRSEL_JOBS={text} is not a positive count of workers"),
            )
        })?),
        Err(_) => hirsel_intern::jobs::Jobs::available(),
    };
    Ok(Some(UnitObjects {
        store,
        backend,
        opt,
        debug,
        fuel,
        jobs,
    }))
}

/// What every unit of one program reads.
pub(crate) struct Program<'a> {
    pub options: &'a Options,
    pub objects: &'a UnitObjects,
    pub table: &'a LayoutTable,
    pub keys: &'a LayoutKeys,
    /// The validators' operation declarations and PS-13 discharge (`Inputs`).
    pub operations: &'a (dyn hirsel_lir::operation::OperationDeclarations + Sync),
    pub discharge: Option<&'a hirsel_lir::discharge::Discharge>,
    /// The layout checks and identities every unit's validations share.
    pub validation: &'a hirsel_lir::validate::ValidationContext<'a>,
    pub emit: &'a EmitInputs<'a>,
    pub instances: &'a Instances,
    pub finalizers: &'a Finalizers,
    /// Per unit, the instance whose placement and proof facts it has.
    pub origins: &'a [InstanceId],
}

/// One unit of a program after `lower`: an object's own unit has its fragment, and a
/// template (a library or known callable body) has none, its summary being in the
/// fragments of the units that copy it.
pub(crate) struct LoweredUnit {
    pub fragment: Option<UnitFragment>,
    /// The object store held the fragment.
    pub hit: bool,
}

/// The units of one program, lowered or loaded, in unit order.
pub(crate) fn lower(
    program: &Program<'_>,
    units: Vec<LirUnit<Value>>,
) -> Result<Vec<LoweredUnit>, NativeError> {
    let index: HashMap<(hirsel_syntax::DefPath, ContentHash), usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| ((u.header.definition.clone(), u.header.instance), i))
        .collect();
    let find = |s: &Symbol| {
        s.instance
            .and_then(|i| index.get(&(s.definition.clone(), i)).copied())
    };
    let template: Vec<bool> = units
        .iter()
        .map(|u| {
            u.header.origin == UnitOrigin::Library
                || program.emit.inline_instances.contains(&u.header.instance)
        })
        .collect();
    // What each unit's code can name: its calls and function references, and the
    // finalizers its cleanup can call.
    let reached: Vec<Vec<usize>> = units
        .iter()
        .map(|u| {
            let finalizers = program
                .finalizers
                .reached(u, program.table)
                .into_iter()
                .filter_map(|(_, f)| index.get(&(f.definition.clone(), f.instance)).copied());
            references(u)
                .filter_map(|s| find(&s))
                .chain(finalizers)
                .collect()
        })
        .collect();
    // The templates each unit's object copies: those it reaches through templates.
    let copies: Vec<Vec<usize>> = (0..units.len())
        .map(|i| {
            let mut seen = BTreeSet::new();
            let mut pending: Vec<usize> = reached[i].clone();
            while let Some(j) = pending.pop() {
                if j != i && template[j] && seen.insert(j) {
                    pending.extend(reached[j].iter().copied());
                }
            }
            seen.into_iter().collect()
        })
        .collect();
    // A template's own key: its code with no copies. A unit's key names its copies' keys.
    let own: Vec<UnitKey> = units
        .iter()
        .enumerate()
        .map(|(i, u)| key(program, u, i, &[]))
        .collect::<Result<_, _>>()?;
    let mut keys = Vec::with_capacity(units.len());
    for i in 0..units.len() {
        let mut copied: Vec<ContentHash> = copies[i].iter().map(|&j| own[j].0).collect();
        copied.sort_unstable();
        keys.push(key(program, &units[i], i, &copied)?);
    }
    let lowered: Vec<OnceLock<Result<ValidatedUnit<Ready>, NativeError>>> =
        units.iter().map(|_| OnceLock::new()).collect();
    let units: Vec<std::sync::Mutex<Option<LirUnit<Value>>>> = units
        .into_iter()
        .map(|u| std::sync::Mutex::new(Some(u)))
        .collect();
    let ready = |i: usize| -> Result<&ValidatedUnit<Ready>, NativeError> {
        lowered[i]
            .get_or_init(|| {
                let unit = units[i]
                    .lock()
                    .expect("an unpoisoned unit")
                    .take()
                    .expect("each unit lowered once");
                stages(program, unit, program.origins[i])
            })
            .as_ref()
            .map_err(Clone::clone)
    };
    let layout_ids: HashMap<hirsel_lir::LayoutKey, LayoutId> = program
        .keys
        .all()
        .iter()
        .enumerate()
        .rev()
        .map(|(i, k)| (*k, LayoutId(i as u32)))
        .collect();
    let abi_ids: HashMap<ContentHash, hirsel_lir::abi_types::FnAbiId> = program
        .table
        .fn_abis
        .iter()
        .enumerate()
        .rev()
        .map(|(i, abi)| {
            (
                program.keys.fn_abi(abi),
                hirsel_lir::abi_types::FnAbiId(i as u32),
            )
        })
        .collect();
    let jobs = program.objects.jobs;
    let tasks: Vec<usize> = (0..keys.len()).filter(|&i| !template[i]).collect();
    let results = hirsel_intern::jobs::map(
        jobs,
        tasks,
        |&i| copies[i].len() as u64 + 1,
        |i| -> Result<(usize, LoweredUnit), NativeError> {
            let key = &keys[i];
            let stored = timed(Phase::StoreLookup, || program.objects.store.get(key));
            if let Some(bytes) = stored
                && let Ok(fragment) =
                    UnitFragment::decode(&bytes, &|k| layout_ids.get(k).copied(), &|k| {
                        abi_ids.get(k).copied()
                    })
            {
                timings::count(Count::ObjectHits);
                let hit = LoweredUnit {
                    fragment: Some(fragment),
                    hit: true,
                };
                return Ok((i, hit));
            }
            timings::count(Count::ObjectMisses);
            let own = ready(i)?;
            let definition = &own.unit().header.definition;
            let mut named: Vec<(String, usize)> = copies[i]
                .iter()
                .map(|&j| Ok((symbol_name(ready(j)?.unit()), j)))
                .collect::<Result<_, NativeError>>()?;
            named.sort();
            let mut selected = vec![own];
            for &(_, j) in &named {
                selected.push(ready(j)?);
            }
            // The native closure: every template the Ready code names is one the key
            // covers.
            for unit in &selected {
                for symbol in references(unit.unit()) {
                    if let Some(j) = find(&symbol)
                        && template[j]
                        && j != i
                        && !copies[i].contains(&j)
                    {
                        return Err(NativeError::of(
                            StageName::Emission,
                            definition,
                            format!(
                                "the native closure: the object names template {} that its key does not cover",
                                symbol.definition
                            ),
                        ));
                    }
                }
            }
            let object = timed(Phase::Codegen, || emit(program, &selected))?;
            let summary = |unit: &ValidatedUnit<Ready>| {
                UnitSummary::of(
                    unit.unit(),
                    program.table,
                    program.keys,
                    program.emit.manifest,
                )
                .map_err(|e| NativeError::of(StageName::Emission, definition, e))
            };
            let fragment = UnitFragment {
                summary: summary(own)?,
                templates: selected[1..]
                    .iter()
                    .map(|u| summary(u))
                    .collect::<Result<_, _>>()?,
                object,
            };
            let bytes = fragment.encode(program.table, program.keys).map_err(|e| {
                NativeError::of(
                    StageName::Emission,
                    definition,
                    format!("the unit's fragment: {e}"),
                )
            })?;
            program.objects.store.put(key, &bytes);
            let missed = LoweredUnit {
                fragment: Some(fragment),
                hit: false,
            };
            Ok((i, missed))
        },
    );
    let mut out: Vec<LoweredUnit> = (0..keys.len())
        .map(|_| LoweredUnit {
            fragment: None,
            hit: false,
        })
        .collect();
    for result in results {
        let (i, unit) = result?;
        out[i] = unit;
    }
    Ok(out)
}

/// The object key of `unit`, the program's unit `index`, with the templates `copies`
/// (their own keys) copied in.
fn key(
    program: &Program<'_>,
    unit: &LirUnit<Value>,
    index: usize,
    copies: &[ContentHash],
) -> Result<UnitKey, NativeError> {
    let options = program.options;
    let origin = program.origins[index];
    let proofs = Proofs::for_instance(program.instances, origin);
    let facts = vec![
        crate::COMPILER_IDENTITY.to_vec(),
        format!(
            "regions {} elide {} reuse {} flat_walk {} noalloc {} unique {:?} backend {} debug {} fuel {}",
            options.regions,
            options.elide,
            options.reuse,
            options.flat_walk,
            hirsel_rc::placement::noalloc(program.instances, origin),
            proofs.unique,
            program.objects.backend.name(),
            program.objects.debug,
            program.objects.fuel,
        )
        .into_bytes(),
        program.finalizers.facts(unit, program.table, program.keys),
    ];
    object_key(
        unit,
        program.emit,
        program.objects.backend.name(),
        program.objects.debug,
        copies,
        &facts,
    )
    .map_err(|e| NativeError::of(StageName::Emission, &unit.header.definition, e))
}

/// Placement through Ready for one unit, each stage's output validated.
fn stages(
    program: &Program<'_>,
    unit: LirUnit<Value>,
    id: InstanceId,
) -> Result<ValidatedUnit<Ready>, NativeError> {
    let options = program.options;
    let table = program.table;
    let inputs = &Inputs {
        target: &options.target,
        layouts: table,
        manifest: program.emit.manifest,
        operations: program.operations,
        discharge: program.discharge,
    };
    let definition = unit.header.definition.clone();
    let fail = |stage, message: String| NativeError::of(stage, &definition, message);
    timings::count(Count::Units);
    let value = seal(StageName::Import, || {
        program.validation.validate(unit, inputs)
    })?
    .into_raw();
    let placement =
        hirsel_rc::placement::Options::for_instance(options.regions, program.instances, id);
    let placed = timed(Phase::Place, || {
        hirsel_rc::placement::place(value, table, &placement)
    })
    .map_err(|e| fail(StageName::Placement, e.to_string()))?;
    let placed = seal(StageName::Placement, || {
        program.validation.validate(placed, inputs)
    })?
    .into_raw();
    let proofs = Proofs::for_instance(program.instances, id);
    let mut unit = timed(Phase::Rc, || {
        hirsel_rc::insert::insert(placed, table, &proofs)
    })
    .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
    if options.elide {
        unit = timed(Phase::Rc, || hirsel_rc::elide::elide(unit, table))
            .map_err(|e| fail(StageName::Perceus, e.to_string()))?;
    }
    let rc = seal(StageName::Perceus, || {
        program.validation.validate(unit, inputs)
    })?
    .into_raw();
    let reuse = hirsel_rc::reuse::Options {
        reuse: options.reuse,
    };
    let reused = timed(Phase::Reuse, || hirsel_rc::reuse::reuse(rc, table, &reuse))
        .map_err(|e| fail(StageName::Reuse, e.to_string()))?;
    let reused = seal(StageName::Reuse, || {
        program.validation.validate(reused, inputs)
    })?
    .into_raw();
    let ready = timed(Phase::Ready, || {
        hirsel_rc::ready::ready(reused, table, program.finalizers)
    })
    .map_err(|e| fail(StageName::Ready, e.to_string()))?;
    let ready = seal(StageName::Ready, || {
        program.validation.validate(ready, inputs)
    })?;
    super::emit::check_unit_runtime_calls(ready.unit(), table, program.emit.manifest)?;
    Ok(ready)
}

/// One object of `selected`, the first the object's own unit.
fn emit(
    program: &Program<'_>,
    selected: &[&ValidatedUnit<Ready>],
) -> Result<hirsel_lir::stage::ObjectArtifact, NativeError> {
    let profile = hirsel_lir::debug::Profile {
        debug: program.objects.debug,
        sources: None,
    };
    let definition = selected[0].unit().header.definition.clone();
    match program.objects.backend {
        Backend::Llvm => hirsel_llvm::emit_object_profile(selected, program.emit, profile)
            .map_err(|e| e.to_string()),
        Backend::Cranelift => {
            hirsel_cranelift::emit_object_profile(selected, program.emit, profile)
                .map_err(|e| e.to_string())
        }
    }
    .map_err(|message| NativeError::of(StageName::Emission, &definition, message))
}

fn symbol_of<S>(unit: &LirUnit<S>) -> Symbol {
    Symbol {
        kind: hirsel_lir::ir::SymbolKind::Function,
        definition: unit.header.definition.clone(),
        instance: Some(unit.header.instance),
    }
}

fn symbol_name<S>(unit: &LirUnit<S>) -> String {
    hirsel_lir::emission::symbol_name(&unit.header.definition, Some(&unit.header.instance))
}

/// The functions `unit` calls directly or names by `FunctionRef`.
fn references<S>(unit: &LirUnit<S>) -> impl Iterator<Item = Symbol> + '_ {
    let calls = unit.tables.calls.iter().filter_map(|c| match c.callee {
        Callee::Direct(s) | Callee::Bound { code: s, .. } => Some(s),
        _ => None,
    });
    let refs = unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::FunctionRef { symbol, .. } => Some(symbol),
        _ => None,
    });
    calls
        .chain(refs)
        .filter_map(|s| unit.tables.symbols.get(s.index()).cloned())
}
