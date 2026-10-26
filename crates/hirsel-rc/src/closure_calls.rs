//! Callable code identity flows independently of its environment and ownership.
//! Discovery reads SSA versions, never source names or layout equality as code evidence.
use crate::import::Lowered;
use crate::instance::InstanceId;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

mod request;
pub use request::{CallableRecipe, TaskCaptureRecipe, TaskCaptureRequest};

pub const MAX_PER_FUNCTION: usize = 16;
pub const MAX_SPECIALIZATIONS: usize = 256;
/// Core library bodies that take a callback and call it once with arguments
/// they own, so a known callback specializes them as an ordinary instance.
pub const HIGHER_ORDER: &[&str] = &[
    "arena.with",
    "arena.bounded",
    "task.scope",
    "task.scope_until",
    "task.isolate",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallableTarget {
    pub symbol: Symbol,
    pub abi: FnAbiId,
    pub environment: bool,
}
type Target = CallableTarget;

/// Immutable callable declaration facts. Keys describe original instances;
/// their bodies are not needed to discover a specialization request.
#[derive(Clone, Debug)]
pub struct CallableEntry {
    pub key: hirsel_layout::instance::InstanceKey,
    pub abi: FnAbiId,
    pub origin: UnitOrigin,
    pub op_free: bool,
}

/// One explicit request for an original Value with known callable parameters.
/// The consumer applies these same seeds when it transforms that selected body.
#[derive(Clone, Debug)]
pub struct CallableRequest {
    pub key: hirsel_layout::instance::InstanceKey,
    pub seeds: Vec<(usize, CallableTarget)>,
    pub spawn_flags: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Code {
    Pending,
    Known(Target),
    Dynamic,
}
impl Code {
    fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Pending, x) | (x, Self::Pending) => x.clone(),
            (Self::Known(a), Self::Known(b)) if a == b => self.clone(),
            _ => Self::Dynamic,
        }
    }
}

fn symbol(u: &mut LirUnit<Value>, target: &Symbol) -> SymbolId {
    if let Some(i) = u.tables.symbols.iter().position(|s| s == target) {
        return SymbolId(i as u32);
    }
    let id = SymbolId(u.tables.symbols.len() as u32);
    u.tables.symbols.push(target.clone());
    id
}

fn whole(u: &LirUnit<Value>, address: AddressId) -> Option<SlotId> {
    let address = &u.addresses[address.index()];
    if address.projection.len != 0 {
        return None;
    }
    match address.base {
        AddressBase::Slot(s) => Some(s),
        AddressBase::Address(a) => whole(u, a),
        AddressBase::Value(_) => None,
    }
}

/// Each equation is a constant or a join of incoming SSA identities. Every
/// node widens at most twice (pending -> known -> dynamic), including cycles.
fn discover(
    u: &LirUnit<Value>,
    entries: &HashMap<Symbol, CallableEntry>,
    seeds: &[(ValueId, Target)],
) -> Vec<Code> {
    hirsel_intern::timings::count(hirsel_intern::timings::Count::ClosureBodyScans);
    let mut inputs = vec![Vec::new(); u.values.len()];
    let mut constants = vec![Code::Dynamic; u.values.len()];
    let mut mutable = BTreeSet::new();
    for loan in &u.tables.loans {
        if loan.mutable
            && let Some(s) = whole(u, loan.address)
        {
            mutable.insert(s);
        }
    }
    let safe_whole = |a| whole(u, a).is_some_and(|s| !mutable.contains(&s));
    for edge in &u.tables.edges {
        for (&p, &a) in u.blocks[edge.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            constants[p.index()] = Code::Pending;
            inputs[p.index()].push(a);
        }
    }
    for inst in &u.instructions {
        let transfer = match *inst {
            Inst::Read {
                result,
                address,
                reaching,
                ..
            }
            | Inst::Copy {
                result,
                address,
                reaching,
                ..
            } if safe_whole(address) => Some((result, reaching)),
            Inst::Store {
                address,
                value,
                version,
            } if safe_whole(address) => Some((version, value)),
            Inst::Replace {
                address,
                value,
                version,
                ..
            } if safe_whole(address) => Some((version, value)),
            Inst::Weaken { result, value } => Some((result, value)),
            _ => None,
        };
        if let Some((to, from)) = transfer {
            constants[to.index()] = Code::Pending;
            inputs[to.index()].push(from);
        } else {
            for v in Inst::defs(inst) {
                constants[v.index()] = Code::Dynamic;
            }
            match *inst {
                Inst::Closure {
                    result, closure, ..
                } => {
                    if let Some(code) = u.tables.closures[closure.index()].code {
                        constants[result.index()] = Code::Known(Target {
                            symbol: u.tables.symbols[code.symbol.index()].clone(),
                            abi: code.abi,
                            environment: true,
                        });
                    }
                }
                Inst::FunctionRef { result, symbol } => {
                    let s = &u.tables.symbols[symbol.index()];
                    if let Some(entry) = entries.get(s) {
                        constants[result.index()] = Code::Known(Target {
                            symbol: s.clone(),
                            abi: entry.abi,
                            environment: false,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    for &(value, ref target) in seeds {
        constants[value.index()] = Code::Known(target.clone());
    }
    let mut users = vec![Vec::new(); u.values.len()];
    for (i, args) in inputs.iter().enumerate() {
        for a in args {
            users[a.index()].push(i);
        }
    }
    let mut codes = constants.clone();
    let mut queue: VecDeque<_> = (0..codes.len()).collect();
    let mut queued = vec![true; codes.len()];
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let code = inputs[i]
            .iter()
            .fold(constants[i].clone(), |c, v| c.join(&codes[v.index()]));
        if code != codes[i] {
            codes[i] = code;
            for &user in &users[i] {
                if !std::mem::replace(&mut queued[user], true) {
                    queue.push_back(user);
                }
            }
        }
    }
    codes
}

/// Original instance for each unit (specializations use its placement and
/// signature proofs), and source units replicated for partition-independent inlining.
pub struct Specializations {
    pub origins: Vec<InstanceId>,
    /// Every unit's own instance key: an original's, or the ordinary derived
    /// key its specialization was requested under. Its digest is the unit's
    /// instance.
    pub keys: Vec<hirsel_layout::instance::InstanceKey>,
    pub inline: BTreeSet<ContentHash>,
}

impl Specializations {
    /// Specializations append Value bodies, not source instances. Every body
    /// must retain an original callable/placement proof owner before import
    /// publishes the lowering to placement and insertion.
    pub(crate) fn assert_origins(&self, lowered: &Lowered) {
        let instances = &lowered.instances;
        let n = instances.len();
        assert_eq!(instances.roles.len(), n);
        assert_eq!(instances.callables.len(), n);
        assert_eq!(instances.abis.len(), n);
        assert_eq!(instances.actuals.len(), n);
        assert_eq!(instances.environments.len(), n);
        assert_eq!(self.origins.len(), lowered.units.len());
        assert_eq!(self.keys.len(), lowered.units.len());
        for ((unit, &origin), key) in lowered.units.iter().zip(&self.origins).zip(&self.keys) {
            let view = instances.view(origin);
            instances.callable(origin);
            assert_eq!(&unit.header.definition, view.key.definition());
            assert_eq!(unit.header.instance, key.digest());
        }
        for id in instances.ids() {
            assert_eq!(self.origins[id.index()], id);
        }
    }
}

pub fn specialize_scoped(
    lowered: &mut Lowered,
    program: &crate::instance::Program,
    target: &hirsel_lir::TargetSpec,
    scoped: bool,
) -> Specializations {
    let mut result = specialize(lowered);
    let registry = crate::import::invocation_registry(
        program,
        &lowered.instances,
        &lowered.semantic_owners,
        &lowered.units,
        &result.origins,
    );
    crate::import::prune_pretransfer(
        &mut lowered.units,
        &lowered.table,
        &registry.pretransfer_adapters,
    );
    if scoped {
        crate::scoped_consumer_9::specialize(lowered, program, target, &mut result);
    }
    result
}

/// Rewrite one selected Value using declaration facts and known callable
/// seeds. The request sink registers derived-instance metadata only: discovery
/// never opens another body or executes a dependency. A refused request leaves
/// that direct call intact. The caller owns deterministic specialization caps.
pub fn rewrite_instance(
    unit: &mut LirUnit<Value>,
    table: &hirsel_lir::layout_types::LayoutTable,
    entries: &HashMap<Symbol, CallableEntry>,
    seeds: &[(usize, CallableTarget)],
    spawn_flags: u64,
    request_specialization: impl FnMut(&CallableEntry, &CallableRequest) -> bool,
) -> BTreeSet<ContentHash> {
    let seeds = seeds
        .iter()
        .map(|(parameter, target)| (unit.parameters[*parameter], target.clone()))
        .collect::<Vec<_>>();
    rewrite_seeded_instance(
        unit,
        table,
        entries,
        &seeds,
        spawn_flags,
        request_specialization,
    )
}

/// The owned task_main adapter's first capture carries callable code while
/// retaining its original environment, move mask and TaskFinish ownership.
pub fn rewrite_task_capture(
    unit: &mut LirUnit<Value>,
    table: &hirsel_lir::layout_types::LayoutTable,
    entries: &HashMap<Symbol, CallableEntry>,
    request: &TaskCaptureRequest,
    request_specialization: impl FnMut(&CallableEntry, &CallableRequest) -> bool,
) -> Result<BTreeSet<ContentHash>, String> {
    if !unit.header.definition.is_core("$native.task_main")
        || unit.header.definition != *request.key.definition()
    {
        return Err("captured callable request is not the selected task_main adapter".into());
    }
    let value = *unit
        .captures
        .first()
        .ok_or("task_main has no callable capture")?;
    Ok(rewrite_seeded_instance(
        unit,
        table,
        entries,
        &[(value, request.target.clone())],
        0,
        request_specialization,
    ))
}

fn rewrite_seeded_instance(
    unit: &mut LirUnit<Value>,
    table: &hirsel_lir::layout_types::LayoutTable,
    entries: &HashMap<Symbol, CallableEntry>,
    seeds: &[(ValueId, CallableTarget)],
    spawn_flags: u64,
    mut request_specialization: impl FnMut(&CallableEntry, &CallableRequest) -> bool,
) -> BTreeSet<ContentHash> {
    let mut inline = BTreeSet::new();
    let codes = discover(unit, entries, seeds);
    if spawn_flags != 0 {
        crate::spawn::set_flags(unit, spawn_flags);
    }
    for call in 0..unit.tables.calls.len() {
        let site = unit.tables.calls[call].clone();
        match site.callee {
            Callee::Indirect(value) => {
                let Code::Known(target) = &codes[value.index()] else {
                    continue;
                };
                let expected = &table.fn_abis[site.abi.0 as usize];
                let actual = &table.fn_abis[target.abi.0 as usize];
                if expected.context != actual.context
                    || expected.destination != actual.destination
                    || expected.parameters != actual.parameters
                    || expected.result != actual.result
                {
                    continue;
                }
                // Iterator steps use a different physical callable layout;
                // Bound currently describes only ordinary closure callables.
                if !matches!(
                    table
                        .layout(unit.types[unit.values[value.index()].ty.index()].layout)
                        .map(|l| &l.kind),
                    Some(hirsel_lir::layout_types::LayoutKind::Callable { .. })
                ) {
                    continue;
                }
                let u = &mut *unit;
                let code = symbol(u, &target.symbol);
                u.tables.calls[call].callee = if target.environment {
                    Callee::Bound {
                        code,
                        closure: value,
                    }
                } else {
                    Callee::Direct(code)
                };
                u.tables.calls[call].abi = target.abi;
                if target.environment
                    && let Some(digest) = target.symbol.instance
                {
                    inline.insert(digest);
                }
            }
            Callee::Direct(s) => {
                let Some(source) = entries.get(&unit.tables.symbols[s.index()]) else {
                    continue;
                };
                // Library units are generated templates; source specializations
                // retain source failure anchors and external entry identity.
                // A core higher-order body whose callback is known at the call
                // is specialized like a source body (the collector's
                // `visit_fn_use`): its callback call becomes direct, so the
                // continuation plan can nest the callback's frame.
                let definition = source.key.definition();
                let spawn = definition.is_core("task.async");
                if source.origin != UnitOrigin::Source
                    && !spawn
                    && !HIGHER_ORDER.iter().any(|name| definition.is_core(name))
                {
                    continue;
                }
                let abi = &table.fn_abis[source.abi.0 as usize];
                let mut bound = Vec::new();
                for arg in site
                    .arguments
                    .get(&unit.pools.call_arguments)
                    .unwrap_or(&[])
                {
                    let ArgumentValue::Value(v) = arg.value else {
                        continue;
                    };
                    let Some(p) = abi
                        .parameters
                        .iter()
                        .position(|p| p.source == arg.parameter && p.convention != Passing::Mut)
                    else {
                        continue;
                    };
                    if let Code::Known(target) = &codes[v.index()] {
                        bound.push((p, target.clone()));
                    }
                }
                if bound.is_empty() {
                    continue;
                }
                let mut flags = 0;
                if spawn {
                    if crate::spawn::immediately_awaited(unit, call) {
                        flags |= crate::spawn::IMMEDIATE;
                    }
                    if bound.iter().any(|(p, t)| {
                        *p == 1 && entries.get(&t.symbol).is_some_and(|entry| entry.op_free)
                    }) {
                        flags |= crate::spawn::OP_FREE;
                    }
                }
                bound.sort_by_key(|(p, _)| *p);
                let effective = request::specialization_key(
                    &source.key,
                    bound.iter().map(|(parameter, target)| {
                        (
                            *parameter,
                            &target.symbol.definition,
                            target.symbol.instance,
                            target.environment,
                        )
                    }),
                    flags,
                );
                let digest = effective.digest();
                let request = CallableRequest {
                    key: effective,
                    seeds: bound,
                    spawn_flags: flags,
                };
                if !request_specialization(source, &request) {
                    continue;
                }
                let target = Symbol {
                    kind: SymbolKind::Function,
                    definition: source.key.definition().clone(),
                    instance: Some(digest),
                };
                let u = &mut *unit;
                u.tables.calls[call].callee = Callee::Direct(symbol(u, &target));
            }
            _ => {}
        }
    }
    inline
}

/// Existing code-target specialization without scoped capture elimination.
pub fn specialize(lowered: &mut Lowered) -> Specializations {
    let originals = lowered.units.clone();
    let op_free = crate::spawn::op_free(&originals, &lowered.table);
    let entries: HashMap<_, _> = originals
        .iter()
        .enumerate()
        .map(|(i, u)| {
            (
                Symbol {
                    kind: SymbolKind::Function,
                    definition: u.header.definition.clone(),
                    instance: Some(u.header.instance),
                },
                CallableEntry {
                    key: lowered.instances.view(InstanceId(i as u32)).key.clone(),
                    abi: u.header.abi,
                    origin: u.header.origin,
                    op_free: op_free.get(&Symbol {
                        kind: SymbolKind::Function,
                        definition: u.header.definition.clone(),
                        instance: Some(u.header.instance),
                    }) == Some(&true),
                },
            )
        })
        .collect();
    let originals_index = originals
        .iter()
        .enumerate()
        .map(|(i, u)| {
            (
                Symbol {
                    kind: SymbolKind::Function,
                    definition: u.header.definition.clone(),
                    instance: Some(u.header.instance),
                },
                i,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut origins: Vec<_> = (0..originals.len()).map(|i| InstanceId(i as u32)).collect();
    let mut keys: Vec<_> = (0..originals.len())
        .map(|i| lowered.instances.view(InstanceId(i as u32)).key.clone())
        .collect();
    let mut inline = BTreeSet::new();
    let mut counts = vec![0; originals.len()];
    let mut cache = BTreeMap::new();
    let mut seeds = vec![Vec::new(); originals.len()];
    let mut spawn_flags = vec![0; originals.len()];
    let mut at = 0;
    while at < lowered.units.len() {
        let codes = discover(&lowered.units[at], &entries, &seeds[at]);
        // task.async wraps its known callable in task_main's owned environment.
        // Carry only the code identity through that capture; the environment,
        // move mask, result slot and holding cleanup remain the original ones.
        // This is ordinary Bound-call specialization, not ahead eligibility:
        // an op-free recursive target still needs a separate physical proof.
        for closure in 0..lowered.units[at].tables.closures.len() {
            let site = &lowered.units[at].tables.closures[closure];
            let Some(code) = site.code else { continue };
            let Some(&original) =
                originals_index.get(&lowered.units[at].tables.symbols[code.symbol.index()])
            else {
                continue;
            };
            let source = &originals[original];
            if !source.header.definition.is_core("$native.task_main") {
                continue;
            }
            let Some(capture) = site
                .captures
                .get(&lowered.units[at].pools.captures)
                .and_then(|cs| cs.first())
            else {
                continue;
            };
            let Code::Known(target) = &codes[capture.value.index()] else {
                continue;
            };
            let Some(&value) = source.captures.first() else {
                continue;
            };
            let request = TaskCaptureRequest::new(
                &lowered.instances.view(InstanceId(original as u32)).key,
                target.clone(),
            )
            .expect("independently selected task_main capture");
            let digest = request.key.digest();
            let key = (original, digest);
            let specialized = if let Some(&i) = cache.get(&key) {
                i
            } else {
                if counts[original] >= MAX_PER_FUNCTION
                    || lowered.units.len() - originals.len() >= MAX_SPECIALIZATIONS
                {
                    continue;
                }
                counts[original] += 1;
                let i = lowered.units.len();
                let mut u = source.clone();
                u.header.instance = digest;
                lowered.units.push(u);
                origins.push(InstanceId(original as u32));
                keys.push(request.key.clone());
                seeds.push(vec![(value, target.clone())]);
                spawn_flags.push(0);
                inline.insert(digest);
                cache.insert(key, i);
                i
            };
            let target = Symbol {
                kind: SymbolKind::Function,
                definition: source.header.definition.clone(),
                instance: Some(lowered.units[specialized].header.instance),
            };
            let u = &mut lowered.units[at];
            let symbol = symbol(u, &target);
            u.tables.closures[closure].code.as_mut().unwrap().symbol = symbol;
        }
        let mut unit = lowered.units[at].clone();
        let direct = rewrite_seeded_instance(
            &mut unit,
            &lowered.table,
            &entries,
            &seeds[at].clone(),
            spawn_flags[at],
            |entry, request| {
                let original = originals_index[&Symbol {
                    kind: SymbolKind::Function,
                    definition: entry.key.definition().clone(),
                    instance: Some(entry.key.digest()),
                }];
                let source = &originals[original];
                let digest = request.key.digest();
                let key = (original, digest);
                if cache.contains_key(&key) {
                    return true;
                }
                if counts[original] >= MAX_PER_FUNCTION
                    || lowered.units.len() - originals.len() >= MAX_SPECIALIZATIONS
                {
                    return false;
                }
                counts[original] += 1;
                let i = lowered.units.len();
                let mut u = source.clone();
                u.header.instance = digest;
                lowered.units.push(u);
                origins.push(InstanceId(original as u32));
                keys.push(request.key.clone());
                seeds.push(
                    request
                        .seeds
                        .iter()
                        .map(|(p, target)| (source.parameters[*p], target.clone()))
                        .collect(),
                );
                spawn_flags.push(request.spawn_flags);
                inline.insert(digest);
                cache.insert(key, i);
                true
            },
        );
        inline.extend(direct);
        lowered.units[at] = unit;
        at += 1;
    }
    Specializations {
        origins,
        keys,
        inline,
    }
}

pub use crate::spawn::{OperationFreedom, settle_operation_freedom};
