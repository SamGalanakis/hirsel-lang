//! The per-unit native pipeline (design §1e). After import and the whole-program Value
//! passes, each unit is placed, counted, reused, readied, validated and emitted on its own,
//! in parallel and with no barrier: the finalizer facts cleanup reads come from the plan
//! (`Finalizers::planned`). Each unit's object is keyed before placement
//! (`hirsel_lir::objects::object_key`), so a unit the object store holds skips every stage
//! after the Value passes, and program assembly reads its fragment.
//!
//! Every unit owns one retained object, including library templates. Optimized LLVM
//! callers may copy reached templates and small callees as optimizer-only bodies;
//! their content keys enter the caller key. Cranelift and LLVM O0 import owner symbols.

use super::emit::Backend;
use super::stages::{NativeError, Options, StageName, timed};
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
use std::sync::Arc;

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
    /// Estimated active native bytes. None selects a share of the process cap;
    /// zero serializes work and still admits an oversized body alone.
    pub byte_budget: Option<u64>,
}

impl std::fmt::Debug for UnitObjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnitObjects")
            .field("backend", &self.backend)
            .field("opt", &self.opt)
            .field("debug", &self.debug)
            .field("fuel", &self.fuel)
            .field("jobs", &self.jobs.get())
            .field("byte_budget", &self.byte_budget)
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
/// names (`dir@1` by default, `none@1` or `sim@1`) on `HIRSEL_JOBS` workers
/// (default: the host's parallelism).
pub(crate) fn selected(
    target: &hirsel_lir::layout_types::TargetSpec,
    backend: Backend,
    opt: OptLevel,
    debug: bool,
    fuel: bool,
) -> Result<Option<UnitObjects>, NativeError> {
    // Writable storage is optional for an implicit default. Keep explicit
    // provider selection strict.
    let default = if hirsel_lir::objects::DirObjects::default_location("").is_some() {
        "dir@1"
    } else {
        "none@1"
    };
    let name = std::env::var_os("HIRSEL_OBJECTS").unwrap_or_else(|| default.into());
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
        byte_budget: None,
    }))
}

/// What every unit of one program reads.
pub(crate) struct Program<'a> {
    pub capture_plan: bool,
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

/// One unit of a program after `lower`, with its independently retained owner object.
pub(crate) struct LoweredUnit {
    pub key: UnitKey,
    pub calls: Vec<super::plan::Calls>,
    pub fragment: UnitFragment,
    /// The object store held the fragment.
    pub hit: bool,
}

/// The units of one program, lowered or loaded, in unit order.
pub(crate) struct LoweredUnits {
    pub units: Vec<LoweredUnit>,
    pub ahead: Vec<Arc<ValidatedUnit<Ready>>>,
    pub environments: std::collections::BTreeMap<ContentHash, Option<LayoutId>>,
}

pub(crate) fn lower(
    program: &Program<'_>,
    units: Vec<LirUnit<Value>>,
) -> Result<LoweredUnits, NativeError> {
    let ahead_indices = hirsel_lir::ahead::prospective(&units);
    let environments = hirsel_lir::ahead::environments(&units);
    let index: HashMap<(hirsel_syntax::DefPath, ContentHash), usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| ((u.header.definition.clone(), u.header.instance), i))
        .collect();
    let find = |s: &Symbol| {
        s.instance
            .and_then(|i| index.get(&(s.definition.clone(), i)).copied())
    };
    let copyable: Vec<bool> = units
        .iter()
        .map(|u| {
            program.objects.backend == Backend::Llvm
                && program.objects.opt != OptLevel::None
                && (u.header.origin == UnitOrigin::Library
                    || program.emit.inline_instances.contains(&u.header.instance)
                    || u.instructions.len() <= super::link::SMALL_UNIT_INSTRUCTIONS)
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
                if j != i && copyable[j] && seen.insert(j) {
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
    // Structural Value rows plus room for transformation scratch and the
    // backend context. These are admission estimates, not allocator/RSS meters.
    let structural: Vec<u64> = units
        .iter()
        .map(|unit| unit.retained_bytes() as u64)
        .collect();
    timings::count_peak(Count::NativeValueBytes, structural.iter().sum());
    // LLVM's tasks/e2e later-phase high-water growth substantially exceeds
    // eight times the structural rows. The larger factor is a conservative
    // empirical admission heuristic, not an isolated per-unit allocation bound:
    // allocator retention and other later-phase work also contribute to RSS.
    let (context, scratch_factor) = match program.objects.backend {
        Backend::Llvm => (32 << 20, 256),
        Backend::Cranelift => (16 << 20, 8),
    };
    let weights: Vec<u64> = copies
        .iter()
        .enumerate()
        .map(|(i, copied)| {
            copied
                .iter()
                .fold(structural[i], |sum, &j| sum.saturating_add(structural[j]))
                .saturating_mul(scratch_factor)
                .saturating_add(context)
        })
        .collect();
    let units = units
        .into_iter()
        .enumerate()
        .map(|(i, mut unit)| {
            unit.header.key = keys[i];
            unit
        })
        .collect();
    let bodies = super::unit_bodies::Bodies::new(units, &copies, |unit: &ValidatedUnit<Ready>| {
        unit.unit().retained_bytes() as u64
    });
    let ready = |i: usize| bodies.ready(i, |unit| stages(program, unit, program.origins[i]));
    let ahead = ahead_indices
        .iter()
        .map(|&i| ready(i))
        .collect::<Result<Vec<_>, _>>()?;
    let ahead_plan = hirsel_lir::ahead::AheadPlan::build(
        &ahead.iter().map(Arc::as_ref).collect::<Vec<_>>(),
        program.table,
        program.emit.target,
        &environments,
        program.objects.fuel,
        super::ahead::helper_identity(),
    );
    if !ahead_plan.roots.is_empty() {
        for key in &mut keys {
            key.0 = ContentHash::of_parts(
                "hirsel.object.ahead-plan.v1",
                [key.0.0.as_slice(), ahead_plan.identity.0.as_slice()],
            );
        }
    }
    // Task bodies routed through their dispatchers read the same prospective
    // bodies as the ahead plan, and so the same plans as the resume object.
    let continuation = match program.objects.backend {
        Backend::Cranelift => hirsel_lir::continuation::ContinuationPlans::build(
            &ahead.iter().map(Arc::as_ref).collect::<Vec<_>>(),
            &hirsel_lir::continuation::PlanInputs {
                table: program.table,
                target: program.emit.target,
                environments: &environments,
                runtime: super::continuation::runtime_identity(),
                classify: &super::continuation::classify,
            },
        ),
        Backend::Llvm => Default::default(),
    };
    if continuation.resumable() {
        for key in &mut keys {
            key.0 = ContentHash::of_parts(
                "hirsel.object.continuation-plan.v1",
                [key.0.0.as_slice(), continuation.identity.0.as_slice()],
            );
        }
    }
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
    let tasks: Vec<usize> = (0..keys.len()).collect();
    let cap = std::env::var("HIRSEL_MEMORY_LIMIT")
        .ok()
        .and_then(|text| hirsel_guard::parse_limit(text.as_bytes()))
        .unwrap_or(hirsel_guard::DEFAULT_LIMIT);
    let budget = program
        .objects
        .byte_budget
        .unwrap_or((cap / 8).min(128 << 20));
    let (results, admitted_peak) = super::admission::map(
        jobs,
        budget,
        tasks,
        |&i| weights[i],
        |i| {
            let result = (|| -> Result<LoweredUnit, NativeError> {
                let key = &keys[i];
                let stored = timed(Phase::StoreLookup, || program.objects.store.get(key));
                let cached = stored.as_ref().and_then(|bytes| {
                    UnitFragment::decode(bytes, &|k| layout_ids.get(k).copied(), &|k| {
                        abi_ids.get(k).copied()
                    })
                    .ok()
                });
                let hit = cached.is_some();
                if hit {
                    timings::count(Count::ObjectHits);
                } else {
                    timings::count(Count::ObjectMisses);
                }
                // Go cache verification (cmd/go/internal/cache/cache.go:144-152,470-490):
                // harness hits recompute and compare; production keeps the fast hit.
                if let Some(fragment) = cached
                    && program.options.validation == super::stages::Validation::Ready
                    && (!program.capture_plan
                        || super::plan::load_calls(
                            program.objects.store.as_ref(),
                            key,
                            stored.as_ref().unwrap(),
                        )
                        .is_some())
                {
                    let calls = if program.capture_plan {
                        super::plan::load_calls(
                            program.objects.store.as_ref(),
                            key,
                            stored.as_ref().unwrap(),
                        )
                        .unwrap()
                    } else {
                        Vec::new()
                    };
                    return Ok(LoweredUnit {
                        key: *key,
                        calls,
                        fragment,
                        hit: true,
                    });
                }
                let own = ready(i)?;
                let mut named: Vec<(String, usize)> = copies[i]
                    .iter()
                    .map(|&j| Ok((symbol_name(ready(j)?.unit()), j)))
                    .collect::<Result<_, NativeError>>()?;
                named.sort();
                let mut retained = vec![own];
                for &(_, j) in &named {
                    retained.push(ready(j)?);
                }
                let selected: Vec<_> = retained.iter().map(Arc::as_ref).collect();
                let own = selected[0];
                let definition = &own.unit().header.definition;
                // The native closure: every template the Ready code names is one the key
                // covers.
                for unit in &selected {
                    for symbol in references(unit.unit()) {
                        if let Some(j) = find(&symbol)
                            && copyable[j]
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
                let object = timed(Phase::Codegen, || emit(program, &selected, &ahead_plan, &continuation))?;
                let summary = |unit: &ValidatedUnit<Ready>| {
                    UnitSummary::of(
                        unit.unit(),
                        program.table,
                        program.keys,
                        program.emit.manifest,
                    )
                    .map(|mut summary| {
                        summary.add_static_references(program.emit.statics);
                        summary
                    })
                    .map_err(|e| NativeError::of(StageName::Emission, definition, e))
                };
                let calls = if program.capture_plan {
                    super::plan::calls(
                        &selected.iter().map(|u| u.unit()).collect::<Vec<_>>(),
                        program.table,
                        program.keys,
                    )
                } else {
                    Vec::new()
                };
                let fragment = UnitFragment {
                    summary: summary(own)?,
                    templates: selected[1..]
                        .iter()
                        .map(|u| summary(u))
                        .collect::<Result<_, _>>()?,
                    object,
                };
                if hit || program.objects.store.enabled() {
                    let bytes = fragment.encode(program.table, program.keys).map_err(|e| {
                        NativeError::of(
                            StageName::Emission,
                            definition,
                            format!("the unit's fragment: {e}"),
                        )
                    })?;
                    if hit && stored.as_deref() != Some(bytes.as_slice()) {
                        return Err(NativeError::of(
                            StageName::Emission,
                            definition,
                            format!(
                                "EveryStage recomputation differs from cached fragment for unit key {}",
                                key.hex()
                            ),
                        ));
                    }
                    if program.capture_plan {
                        super::plan::save_calls(
                            program.objects.store.as_ref(),
                            key,
                            &bytes,
                            &calls,
                        )?;
                    }
                    program.objects.store.put(key, &bytes);
                }
                Ok(LoweredUnit {
                    key: *key,
                    calls,
                    fragment,
                    hit,
                })
            })();
            bodies.finished(i, &copies[i]);
            result
        },
    );
    timings::count_peak(Count::NativeActiveBytes, admitted_peak);
    timings::count_peak(Count::NativeBudgetBytes, budget);
    timings::count_peak(Count::NativeReadyBytes, bodies.peak_bytes());
    Ok(LoweredUnits {
        units: results.into_iter().collect::<Result<_, _>>()?,
        ahead,
        environments,
    })
}

/// The object key of `unit`, the program's unit `index`, with the templates `copies`
/// (their own keys) copied in.
fn key(
    program: &Program<'_>,
    unit: &LirUnit<Value>,
    index: usize,
    copies: &[ContentHash],
) -> Result<UnitKey, NativeError> {
    timings::count(Count::LateKeys);
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
        memo: Default::default(),
        target: &options.target,
        layouts: table,
        manifest: program.emit.manifest,
        operations: program.operations,
        discharge: program.discharge,
    };
    super::stages::lower_unit(
        unit,
        id,
        options,
        program.instances,
        program.finalizers,
        inputs,
        program.validation,
    )
}

/// One object of `selected`, the first the object's own unit.
fn emit(
    program: &Program<'_>,
    selected: &[&ValidatedUnit<Ready>],
    ahead: &hirsel_lir::ahead::AheadPlan,
    continuation: &hirsel_lir::continuation::ContinuationPlans,
) -> Result<hirsel_lir::stage::ObjectArtifact, NativeError> {
    let profile = hirsel_lir::debug::Profile {
        debug: program.objects.debug,
        sources: None,
    };
    let definition = selected[0].unit().header.definition.clone();
    let copies = selected[1..]
        .iter()
        .map(|u| u.unit().header.instance)
        .collect();
    let inputs = EmitInputs {
        ahead: Some(ahead),
        continuation: Some(continuation),
        available_externally: &copies,
        ..program.emit.clone()
    };
    match program.objects.backend {
        Backend::Llvm => {
            hirsel_llvm::emit_object_profile(selected, &inputs, profile).map_err(|e| e.to_string())
        }
        Backend::Cranelift => hirsel_cranelift::emit_object_profile(selected, &inputs, profile)
            .map_err(|e| e.to_string()),
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

/// Every function the unit names directly, by a function reference or as closure code.
fn references<S>(unit: &LirUnit<S>) -> impl Iterator<Item = Symbol> + '_ {
    let calls = unit.tables.calls.iter().filter_map(|c| match c.callee {
        Callee::Direct(s) | Callee::Bound { code: s, .. } => Some(s),
        _ => None,
    });
    let closures = unit
        .tables
        .closures
        .iter()
        .filter_map(|site| site.code.map(|code| code.symbol));
    let refs = unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::FunctionRef { symbol, .. } => Some(symbol),
        _ => None,
    });
    calls
        .chain(refs)
        .chain(closures)
        .filter_map(|s| unit.tables.symbols.get(s.index()).cloned())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Store {
        rows: Mutex<BTreeMap<UnitKey, Arc<[u8]>>>,
        puts: AtomicUsize,
        tamper: AtomicBool,
    }
    impl ObjectStore for Store {
        fn get(&self, key: &UnitKey) -> Option<Arc<[u8]>> {
            let bytes = self.rows.lock().unwrap().get(key).cloned()?;
            if self.tamper.load(Ordering::Relaxed) {
                // Fragment metadata still decodes; change the trailing object payload.
                let mut bytes = bytes.to_vec();
                *bytes.last_mut().unwrap() ^= 1;
                Some(bytes.into())
            } else {
                Some(bytes)
            }
        }
        fn put(&self, key: &UnitKey, bytes: &[u8]) {
            if bytes.starts_with(b"HIRFRG02") {
                self.puts.fetch_add(1, Ordering::Relaxed);
            }
            self.rows.lock().unwrap().insert(*key, bytes.into());
        }
    }

    #[test]
    fn warm_ready_hits_skip_work_but_every_stage_recomputes_and_compares() {
        let run = || {
            hirsel_engine::with_compiler_stack(|| {
                use hirsel_intern::timings::{self, Count, Phase};
                timings::enable();
                let calls = || {
                    let report = timings::report();
                    let phases = [
                        Phase::Place,
                        Phase::Rc,
                        Phase::Reuse,
                        Phase::Ready,
                        Phase::Codegen,
                        Phase::LirValidate,
                    ];
                    (
                        phases.map(|phase| {
                            report
                                .phases
                                .iter()
                                .find(|p| p.phase == phase)
                                .unwrap()
                                .calls
                        }),
                        report
                            .counts
                            .iter()
                            .find(|(c, _)| *c == Count::Units)
                            .unwrap()
                            .1,
                    )
                };
                for workers in [1, 2, 8] {
                    let mut engine = hirsel_engine::QueryEngine::new(
                        hirsel_engine::default_environment(),
                        crate::compiler_adapters(),
                    );
                    let file = engine
                        .sources
                        .add(
                            "warm-object.hir",
                            "fn helper(x:u64)->u64{x+1u64}\nfn main()->u64{helper(7u64)}".into(),
                        )
                        .unwrap();
                    let output = hirsel_engine::check_progress(
                        &mut engine,
                        file,
                        &hirsel_engine::default_context(),
                    )
                    .unwrap();
                    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
                    let input = engine.export_native_input().unwrap();
                    let input = engine.import_native_input(input).unwrap();
                    let store = Arc::new(Store::default());
                    let mut options = Options::new(
                        hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu(),
                    );
                    options.validation = super::super::Validation::Ready;
                    options.objects = Some(UnitObjects {
                        store: store.clone(),
                        backend: Backend::Cranelift,
                        opt: OptLevel::None,
                        debug: false,
                        fuel: false,
                        jobs: hirsel_intern::jobs::Jobs::new(workers),
                        byte_budget: None,
                    });
                    let before = calls();
                    let cold = super::super::build(&input, &options)
                        .unwrap()
                        .unit_objects
                        .unwrap();
                    let after = calls();
                    let cold_calls: [_; 6] = std::array::from_fn(|i| after.0[i] - before.0[i]);
                    let units = after.1 - before.1;
                    assert!(units > 0);
                    assert!(cold_calls.iter().all(|c| *c > 0));
                    assert!(!cold.objects.is_empty());
                    assert_eq!(cold.hits, 0);
                    let puts = store.puts.load(Ordering::Relaxed);
                    assert!(puts > 0);
                    let warm = super::super::build(&input, &options)
                        .unwrap()
                        .unit_objects
                        .unwrap();
                    assert_eq!(warm.hits, cold.objects.len());
                    assert_eq!(warm.objects, cold.objects);
                    assert_eq!(store.puts.load(Ordering::Relaxed), puts);
                    let after_warm = calls();
                    assert_eq!(after_warm.1, after.1, "Ready hit lowered a unit");
                    assert_eq!(
                        &after_warm.0[..5],
                        &after.0[..5],
                        "Ready hit ran the pipeline"
                    );
                    options.validation = super::super::Validation::EveryStage;
                    let checked = super::super::build(&input, &options)
                        .unwrap()
                        .unit_objects
                        .unwrap();
                    let after_checked = calls();
                    let checked_calls: [_; 6] =
                        std::array::from_fn(|i| after_checked.0[i] - after_warm.0[i]);
                    assert_eq!(after_checked.1 - after_warm.1, units);
                    assert_eq!(&checked_calls[..5], &cold_calls[..5]);
                    assert_eq!(
                        checked_calls[5] - cold_calls[5],
                        4 * units,
                        "warm EveryStage did not restore all four intermediate checkpoints"
                    );
                    assert_eq!(checked.hits, cold.objects.len());
                    assert_eq!(checked.objects, cold.objects);
                    assert_eq!(store.puts.load(Ordering::Relaxed), 2 * puts);
                    store.tamper.store(true, Ordering::Relaxed);
                    let error = match super::super::build(&input, &options) {
                        Ok(_) => panic!("EveryStage accepted changed cached object bytes"),
                        Err(error) => error,
                    };
                    assert_eq!(error.stage, StageName::Emission);
                    assert!(error.definition.is_some());
                    assert!(
                        error.message.contains(
                            "EveryStage recomputation differs from cached fragment for unit key"
                        ),
                        "{error}"
                    );
                    assert_eq!(store.puts.load(Ordering::Relaxed), 2 * puts);
                }
                "checked".to_owned()
            })
        };
        if let Some(outputs) = hirsel_intern::under_salts(
            "native::units::cache_tests::warm_ready_hits_skip_work_but_every_stage_recomputes_and_compares",
            &[1],
            run,
        ) {
            assert_eq!(outputs, ["checked"]);
        }
    }
    #[test]
    fn retained_owner_workers_match_at_one_two_eight_and_both_validation_policies() {
        let run = || {
            hirsel_engine::with_compiler_stack(|| {
                let mut engine = hirsel_engine::QueryEngine::new(
                    hirsel_engine::default_environment(),
                    crate::compiler_adapters(),
                );
                let file = engine.sources.add("owner-workers.hir", "fn step(x:u64)->u64{x+1u64}\nfn main()->u64 {let mut xs:List[u64]=[];list.push(mut xs,7u64);step(xs[0u64])}\n".into()).unwrap();
                let checked = hirsel_engine::check_progress(
                    &mut engine,
                    file,
                    &hirsel_engine::default_context(),
                )
                .unwrap();
                assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
                let input = engine.export_native_input().unwrap();
                let input = engine.import_native_input(input).unwrap();
                let mut artifacts = String::new();
                for (backend, opt) in [
                    (Backend::Cranelift, OptLevel::None),
                    (Backend::Llvm, OptLevel::None),
                    (Backend::Llvm, OptLevel::Default),
                ] {
                    let mut baseline = None;
                    for workers in [1, 2, 8] {
                        for budget in [0, 1, 128 << 20, u64::MAX] {
                            for validation in [
                                super::super::Validation::Ready,
                                super::super::Validation::EveryStage,
                            ] {
                                let mut options = Options::new(
                                    hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu(),
                                );
                                options.validation = validation;
                                options.objects = Some(UnitObjects {
                                    store: Arc::new(hirsel_lir::objects::NoObjects),
                                    backend,
                                    opt,
                                    debug: false,
                                    fuel: false,
                                    jobs: hirsel_intern::jobs::Jobs::new(workers),
                                    byte_budget: Some(budget),
                                });
                                let ready = super::super::build(&input, &options).unwrap();
                                let objects = ready.unit_objects.unwrap().objects;
                                assert_eq!(objects.len(), ready.summaries.len());
                                assert!(
                                    ready
                                        .summaries
                                        .iter()
                                        .any(|s| s.origin == UnitOrigin::Library)
                                );
                                for summary in &ready.summaries {
                                    let symbol = hirsel_lir::emission::symbol_name(
                                        &summary.definition,
                                        Some(&summary.instance),
                                    );
                                    assert_eq!(
                                        objects
                                            .iter()
                                            .filter(|o| o.exports.iter().any(|e| e.name == symbol))
                                            .count(),
                                        1,
                                        "{backend:?} {opt:?} {workers} {validation:?} {symbol}"
                                    );
                                }
                                let actual = (ready.summaries, objects);
                                if let Some(expected) = &baseline {
                                    assert_eq!(
                                        &actual, expected,
                                        "{backend:?} {opt:?} {workers} {validation:?}"
                                    )
                                } else {
                                    baseline = Some(actual)
                                }
                            }
                        }
                    }
                    artifacts.push_str(&format!("{backend:?} {opt:?} {:?}", baseline.unwrap()));
                }
                artifacts
            })
        };
        if let Some(outputs) = hirsel_intern::under_salts(
            "native::units::cache_tests::retained_owner_workers_match_at_one_two_eight_and_both_validation_policies",
            &[1, 2, 0x5eed],
            run,
        ) {
            assert_eq!(outputs.len(), 3);
            assert!(
                outputs.windows(2).all(|pair| pair[0] == pair[1]),
                "salt changed complete summaries or objects"
            );
        }
    }
}
