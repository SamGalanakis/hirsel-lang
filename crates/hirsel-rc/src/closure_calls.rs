//! Callable code identity flows independently of its environment and ownership.
//! Discovery reads SSA versions, never source names or layout equality as code evidence.
use crate::import::Lowered;
use crate::instance::InstanceId;
use hirsel_core_contract::Passing;
use hirsel_lir::abi_types::FnAbiId;
use hirsel_lir::ir::*;
use hirsel_lir::stage::Value;
use hirsel_syntax::ContentHash;
use std::collections::{BTreeSet, HashMap, VecDeque};

pub const MAX_PER_FUNCTION: usize = 16;
pub const MAX_SPECIALIZATIONS: usize = 256;

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
    seeds: &[(usize, Target)],
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
    for &(p, ref target) in seeds {
        constants[u.parameters[p].index()] = Code::Known(target.clone());
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
            assert_eq!(&unit.header.definition, key.definition());
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
                let spawn = source.key.definition().is_core("task.async");
                if source.origin != UnitOrigin::Source && !spawn {
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
                    if flags == 0 {
                        continue;
                    }
                }
                bound.sort_by_key(|(p, _)| *p);
                let mut encoded: Vec<u8> = bound
                    .iter()
                    .flat_map(|(p, t)| {
                        let mut b = (*p as u64).to_le_bytes().to_vec();
                        b.extend(t.symbol.definition.fingerprint().0);
                        b.push(u8::from(t.symbol.instance.is_some()));
                        b.extend(t.symbol.instance.unwrap_or_default().0);
                        b.push(u8::from(t.environment));
                        b
                    })
                    .collect();
                if spawn {
                    encoded.extend(flags.to_le_bytes());
                }
                let effective = source.key.derived(
                    "hirsel.callable-target-specialization.v1",
                    &[encoded.as_slice()],
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

/// Existing batch consumer of the sole local callable rewrite. The request
/// sink retains the original per-function/global caps and discovery order.
pub fn specialize(lowered: &mut Lowered) -> Specializations {
    let originals = lowered.units.clone();
    let op_free = crate::spawn::op_free(&originals, &lowered.table);
    let mut origins: Vec<_> = (0..originals.len()).map(|i| InstanceId(i as u32)).collect();
    let mut keys: Vec<_> = origins
        .iter()
        .map(|&id| lowered.instances.view(id).key.clone())
        .collect();
    let symbol_of = |unit: &LirUnit<Value>| Symbol {
        kind: SymbolKind::Function,
        definition: unit.header.definition.clone(),
        instance: Some(unit.header.instance),
    };
    let index = originals
        .iter()
        .enumerate()
        .map(|(i, unit)| (symbol_of(unit), i))
        .collect::<HashMap<_, _>>();
    let entries = originals
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            let symbol = symbol_of(unit);
            let entry = CallableEntry {
                key: keys[i].clone(),
                abi: unit.header.abi,
                origin: unit.header.origin,
                op_free: op_free.get(&symbol) == Some(&true),
            };
            (symbol, entry)
        })
        .collect();
    let mut inline = BTreeSet::new();
    let mut counts = vec![0; originals.len()];
    let mut cache = BTreeSet::new();
    let mut seeds = vec![Vec::new(); originals.len()];
    let mut spawn_flags = vec![0; originals.len()];
    let units = &mut lowered.units;
    let mut at = 0;
    while at < units.len() {
        let mut unit = units[at].clone();
        let own_seeds = seeds[at].clone();
        let own_flags = spawn_flags[at];
        let direct = rewrite_instance(
            &mut unit,
            &lowered.table,
            &entries,
            &own_seeds,
            own_flags,
            |source, request| {
                let source_symbol = Symbol {
                    kind: SymbolKind::Function,
                    definition: source.key.definition().clone(),
                    instance: Some(source.key.digest()),
                };
                let original = index[&source_symbol];
                let digest = request.key.digest();
                let key = (original, digest);
                if cache.contains(&key) {
                    return true;
                }
                if counts[original] >= MAX_PER_FUNCTION
                    || units.len() - originals.len() >= MAX_SPECIALIZATIONS
                {
                    return false;
                }
                counts[original] += 1;
                let mut specialized = originals[original].clone();
                specialized.header.instance = digest;
                units.push(specialized);
                origins.push(InstanceId(original as u32));
                keys.push(request.key.clone());
                seeds.push(request.seeds.clone());
                spawn_flags.push(request.spawn_flags);
                inline.insert(digest);
                cache.insert(key);
                true
            },
        );
        inline.extend(direct);
        units[at] = unit;
        at += 1;
    }
    Specializations {
        origins,
        keys,
        inline,
    }
}
