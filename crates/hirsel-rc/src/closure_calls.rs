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

pub const MAX_PER_FUNCTION: usize = 16;
pub const MAX_SPECIALIZATIONS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Target {
    symbol: Symbol,
    abi: FnAbiId,
    environment: bool,
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
    entries: &HashMap<Symbol, (usize, FnAbiId)>,
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
                    if let Some(&(_, abi)) = entries.get(s) {
                        constants[result.index()] = Code::Known(Target {
                            symbol: s.clone(),
                            abi,
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
    pub inline: BTreeSet<ContentHash>,
}

pub fn specialize(lowered: &mut Lowered) -> Specializations {
    let originals = lowered.units.clone();
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
                (i, u.header.abi),
            )
        })
        .collect();
    let mut origins: Vec<_> = (0..originals.len()).map(|i| InstanceId(i as u32)).collect();
    let mut inline = BTreeSet::new();
    let mut counts = vec![0; originals.len()];
    let mut cache = BTreeMap::new();
    let mut seeds = vec![Vec::new(); originals.len()];
    let mut at = 0;
    while at < lowered.units.len() {
        let codes = discover(&lowered.units[at], &entries, &seeds[at]);
        for call in 0..lowered.units[at].tables.calls.len() {
            let site = lowered.units[at].tables.calls[call].clone();
            match site.callee {
                Callee::Indirect(value) => {
                    let Code::Known(target) = &codes[value.index()] else {
                        continue;
                    };
                    let expected = &lowered.table.fn_abis[site.abi.0 as usize];
                    let actual = &lowered.table.fn_abis[target.abi.0 as usize];
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
                        lowered
                            .table
                            .layout(
                                lowered.units[at].types
                                    [lowered.units[at].values[value.index()].ty.index()]
                                .layout
                            )
                            .map(|l| &l.kind),
                        Some(hirsel_lir::layout_types::LayoutKind::Callable { .. })
                    ) {
                        continue;
                    }
                    let u = &mut lowered.units[at];
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
                    let Some(&(original, _)) =
                        entries.get(&lowered.units[at].tables.symbols[s.index()])
                    else {
                        continue;
                    };
                    let source = &originals[original];
                    // Library units are generated templates; source specializations
                    // retain source failure anchors and external entry identity.
                    if source.header.origin != UnitOrigin::Source {
                        continue;
                    }
                    let abi = &lowered.table.fn_abis[source.header.abi.0 as usize];
                    let mut bound = Vec::new();
                    for arg in site
                        .arguments
                        .get(&lowered.units[at].pools.call_arguments)
                        .unwrap_or(&[])
                    {
                        let ArgumentValue::Value(v) = arg.value else {
                            continue;
                        };
                        let Some(p) = abi.parameters.iter().position(|p| {
                            p.source == arg.parameter && p.convention != Passing::Mut
                        }) else {
                            continue;
                        };
                        if let Code::Known(target) = &codes[v.index()] {
                            bound.push((p, target.clone()));
                        }
                    }
                    if bound.is_empty() {
                        continue;
                    }
                    bound.sort_by_key(|(p, _)| *p);
                    let encoded: Vec<u8> = bound
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
                    let digest = ContentHash::of_parts(
                        "hirsel.callable-target-specialization.v1",
                        [source.header.instance.0.as_slice(), encoded.as_slice()],
                    );
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
                        seeds.push(bound);
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
                    u.tables.calls[call].callee = Callee::Direct(symbol(u, &target));
                }
                _ => {}
            }
        }
        at += 1;
    }
    Specializations { origins, inline }
}
