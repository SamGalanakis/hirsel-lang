//! Mandatory self-tail loops. Only sites the source published in `Body.tail_sites` are
//! candidates, and each is re-verified here against the instance it belongs
//! to, with that instance's layouts: a site that fails stays an ordinary call
//! and is reported. `ConstructorHole` is reserved until spec #46 rules and is
//! rejected outright.
//!
//! A verified instance becomes one loop whose header is the TIR entry block.
//! A parameter every site passes back unchanged by borrow stays in its caller's
//! storage; every other parameter lives in an internal owned loop slot
//! (`SlotKind::LoopCarried`). A borrowed counted temporary listed in
//! `owned_loop_arguments` moves into that slot at the site, so the next
//! iteration borrows it from the loop and the slot releases it on the next
//! replacement and at the final exit (LANG-10: one count operation per call).
//! The import (`crate::import`) emits the slots, the prologue and the back
//! edges from this plan.
use crate::instance::{
    InstanceError, InstanceId, InstanceRole, Instances, Program, callee_actuals,
};
use hirsel_core_contract::Passing;
use hirsel_tir as tir;
use hirsel_types::UseKind;
use std::collections::BTreeSet;

/// Where a parameter lives in a normalized instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Home {
    /// The caller's storage: every site passes the parameter back unchanged.
    Parameter,
    /// An internal owned loop slot, written at every back edge.
    Loop,
}

/// One verified site: its call block and the counted temporaries that move
/// into loop slots there, by declared parameter ordinal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopSite {
    pub call_block: tir::BlockId,
    pub owned: Vec<(u32, tir::PlaceId)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopPlan {
    /// Per declared parameter ordinal.
    pub homes: Vec<Home>,
    pub sites: Vec<LoopSite>,
}

impl LoopPlan {
    pub fn site(&self, block: tir::BlockId) -> Option<&LoopSite> {
        self.sites.iter().find(|s| s.call_block == block)
    }
}

/// Why a published site stays a call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declined {
    pub call_block: tir::BlockId,
    pub reason: &'static str,
}

#[derive(Clone, Debug)]
pub enum NormalizeError {
    /// A `ConstructorHole` site: reserved until spec #46 rules.
    ConstructorHole {
        definition: hirsel_syntax::DefPath,
        call_block: tir::BlockId,
    },
    Instance(InstanceError),
}

/// Layout facts of a TIR type under the instance's actuals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Facts {
    /// Bits with no credit and no destruction work.
    pub plain: bool,
    /// Copyable data (a counted copy is one count operation).
    pub copyable: bool,
}

/// The plan for one instance, or None when no site survives verification.
pub fn plan(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    passing: &[Passing],
    facts: &mut dyn FnMut(hirsel_types::TypeId) -> Facts,
) -> Result<(Option<LoopPlan>, Vec<Declined>), NormalizeError> {
    let view = instances.view(id);
    let def: &tir::Definition = &view.definition;
    let body = &def.bodies[view.body.0 as usize];
    let mut sites = Vec::new();
    let mut declined = Vec::new();
    let mut seen = BTreeSet::new();
    for site in &body.tail_sites {
        if let tir::TailKind::ConstructorHole(_) = site.kind {
            return Err(NormalizeError::ConstructorHole {
                definition: def.path.path().clone(),
                call_block: site.call_block,
            });
        }
        if site.owner_body != view.body {
            continue;
        }
        let verdict = if !seen.insert(site.call_block.0) {
            Err("published twice")
        } else {
            verify(program, instances, id, site, passing, facts)
        };
        match verdict {
            Ok(s) => sites.push(s),
            Err(reason) => declined.push(Declined {
                call_block: site.call_block,
                reason,
            }),
        }
    }
    if sites.is_empty() {
        return Ok((None, declined));
    }
    let homes = (0..body.parameters.len() as u32)
        .map(|k| {
            let local = body.parameters[k as usize];
            let unchanged = sites.iter().all(|s| {
                let tir::Terminator::Call { arguments, .. } =
                    &body.blocks[s.call_block.0 as usize].terminator
                else {
                    return false;
                };
                arguments.iter().any(|a| {
                    let p = &body.places[a.value.place.0 as usize];
                    a.parameter.0 == k
                        && a.value.usage == UseKind::Borrow
                        && p.root == local
                        && p.projection.is_empty()
                })
            });
            if passing[k as usize] == Passing::Borrow && unchanged {
                Home::Parameter
            } else {
                Home::Loop
            }
        })
        .collect();
    Ok((Some(LoopPlan { homes, sites }), declined))
}

/// Re-verify one published site for this instance.
fn verify(
    program: &Program,
    instances: &Instances,
    id: InstanceId,
    site: &tir::TailSite,
    passing: &[Passing],
    facts: &mut dyn FnMut(hirsel_types::TypeId) -> Facts,
) -> Result<LoopSite, &'static str> {
    let view = instances.view(id);
    let def: &tir::Definition = &view.definition;
    let body = &def.bodies[view.body.0 as usize];
    if matches!(instances.roles[id.index()], InstanceRole::Closure { .. }) || view.body != def.entry
    {
        return Err("not a definition's entry body");
    }
    let Some(call) = body.blocks.get(site.call_block.0 as usize) else {
        return Err("call block outside the body");
    };
    let tir::Terminator::Call {
        callee:
            tir::Callee::Definition {
                path,
                arguments: generics,
            },
        arguments,
        returns,
        ..
    } = &call.terminator
    else {
        return Err("not a direct call");
    };
    if path != def.path.path() {
        return Err("calls another definition");
    }
    // The same instance: polymorphic recursion is a different instance.
    let actuals = &instances.actuals[id.index()];
    let callee = callee_actuals(
        &program.types,
        &program.memo,
        program.binders.get(path).copied(),
        generics,
        actuals,
        false,
        path,
    )
    .map_err(|_| "callee actuals")?;
    if instances.find(program, path, &callee).ok() != Some(id) {
        return Err("calls another instance");
    }
    // Every declared parameter exactly once, never by mut.
    let mut ordinals: Vec<u32> = arguments.iter().map(|a| a.parameter.0).collect();
    ordinals.sort_unstable();
    if ordinals != (0..passing.len() as u32).collect::<Vec<_>>() {
        return Err("arguments do not cover the parameters once");
    }
    let place = |p: tir::PlaceId| &body.places[p.0 as usize];
    let local = |p: tir::PlaceId| &body.locals[place(p).root.0 as usize];
    let mut owned = Vec::new();
    let mut loop_roots = BTreeSet::new();
    for a in arguments {
        let p = place(a.value.place);
        match a.value.usage {
            UseKind::Mut => return Err("mut argument"),
            UseKind::Borrow => {
                let f = facts(p.ty);
                let listed = site.owned_loop_arguments.iter().any(|(k, q)| {
                    *k == a.parameter
                        && place(*q).root == p.root
                        && place(*q).projection == p.projection
                });
                if f.plain {
                    continue;
                }
                if listed {
                    if !(p.projection.is_empty()
                        && local(a.value.place).storage == tir::Storage::Temporary
                        && f.copyable)
                    {
                        return Err("owned loop argument is not a whole counted temporary");
                    }
                    if !loop_roots.insert(p.root.0) {
                        return Err("one temporary carried twice");
                    }
                    owned.push((a.parameter.0, a.value.place));
                } else if local(a.value.place).storage != tir::Storage::Parameter || !f.copyable {
                    return Err("borrowed counted argument outlives the frame");
                }
            }
            UseKind::Take | UseKind::Copy => {}
        }
    }
    if owned.len() != site.owned_loop_arguments.len() {
        return Err("owned loop arguments do not match borrowed temporaries");
    }
    // The temporary has no other consumer among the arguments.
    for root in &loop_roots {
        if arguments
            .iter()
            .filter(|a| place(a.value.place).root.0 == *root)
            .count()
            != 1
        {
            return Err("owned loop temporary has a second consumer");
        }
    }
    // The continuation only forwards the result to Return: no work remains in
    // the frame. Owned content still present at the site is checked against
    // the import's abstract state, where it is an error. A diverging call has
    // no continuation: its loop is the canonical `fn serve() -> Never` shape.
    let Some(tir::CallReturn { destination, next }) = *returns else {
        return Ok(LoopSite {
            call_block: site.call_block,
            owned,
        });
    };
    let same = |a: tir::PlaceId, b: tir::PlaceId| {
        let (a, b) = (place(a), place(b));
        a.root == b.root && a.projection == b.projection
    };
    let mut values = vec![destination];
    let mut block = next;
    let mut visited = BTreeSet::new();
    while visited.insert(block.0) {
        let Some(b) = body.blocks.get(block.0 as usize) else {
            return Err("continuation outside the body");
        };
        for i in &b.instructions {
            use tir::InstructionKind as K;
            match &i.kind {
                K::Assign { destination, value } => {
                    match &body.rvalues[value.0 as usize] {
                        tir::Rvalue::Use(o) if values.iter().any(|v| same(*v, o.place)) => {}
                        tir::Rvalue::Constant(tir::Constant::Unit)
                            if facts(place(*destination).ty).plain
                                && facts(place(values[0]).ty).plain => {}
                        _ => return Err("continuation computes"),
                    }
                    values.push(*destination);
                }
                K::StorageLive(_)
                | K::StorageDead { .. }
                | K::EndScope(_)
                | K::BindOrigin { .. }
                | K::BindFields { .. }
                | K::Defuse { .. }
                | K::EndOrigin(_)
                | K::EndLoan { .. }
                | K::RunCleanup(_) => {}
                K::Drop { place: p, .. }
                    if facts(place(*p).ty).plain || loop_roots.contains(&place(*p).root.0) => {}
                K::Drop { if_live: true, .. } => {}
                _ => return Err("continuation runs an operation"),
            }
        }
        match &b.terminator {
            tir::Terminator::Goto(n) => block = *n,
            tir::Terminator::Return { value, .. } => {
                return if values.iter().any(|v| same(*v, value.place)) {
                    Ok(LoopSite {
                        call_block: site.call_block,
                        owned,
                    })
                } else {
                    Err("returns another value")
                };
            }
            _ => return Err("continuation branches"),
        }
    }
    Err("continuation loops")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PermuteError {
    /// `order` is not a permutation of the instance's parameter ordinals.
    NotAPermutation,
    /// A closure, or an instance some unit references as a value: its ABI is
    /// the erased callable's, fixed by its type.
    Escapes,
}

/// Reorder a helper instance's parameters: `order[i]` is the old ordinal now
/// passed at position `i`. One transform updates everything that names a
/// parameter ordinal together: the instance view's mapping, a new `FnAbi`
/// (conventions, layouts and physical slots move with their parameter), the
/// unit's parameter values and parameter/loop slot ordinals, caller-placed
/// destinations, and every call site's argument ordinals and ABI. Units are
/// resealed against the grown layout table.
pub fn permute_parameters(
    lowered: &mut crate::import::Lowered,
    id: InstanceId,
    order: &[u32],
) -> Result<hirsel_lir::abi_types::FnAbiId, PermuteError> {
    use hirsel_lir::ir::{ArgumentValue, Callee, DestinationClass, Inst, SlotKind, SymbolKind};
    let n = order.len();
    let mut new_of = vec![u32::MAX; n];
    for (i, &old) in order.iter().enumerate() {
        match new_of.get_mut(old as usize) {
            Some(slot) if *slot == u32::MAX => *slot = i as u32,
            _ => return Err(PermuteError::NotAPermutation),
        }
    }
    let old_abi_id = lowered.instances.abis[id.index()];
    let old_abi = lowered.table.fn_abis[old_abi_id.0 as usize].clone();
    if old_abi.parameters.len() != n {
        return Err(PermuteError::NotAPermutation);
    }
    if matches!(
        lowered.instances.roles[id.index()],
        InstanceRole::Closure { .. }
    ) {
        return Err(PermuteError::Escapes);
    }
    let digest = lowered.instances.view(id).key.digest();
    let path = lowered.instances.view(id).key.definition().clone();
    let names_instance = |s: &hirsel_lir::ir::Symbol| {
        s.kind == SymbolKind::Function && s.instance == Some(digest) && s.definition == path
    };
    // A function value of the instance keeps the erased callable ABI.
    for unit in &lowered.units {
        for inst in &unit.instructions {
            if let Inst::FunctionRef { symbol, .. } = inst
                && names_instance(&unit.tables.symbols[symbol.index()])
            {
                return Err(PermuteError::Escapes);
            }
        }
    }
    let mut abi = old_abi.clone();
    abi.parameters = order
        .iter()
        .enumerate()
        .map(|(i, &old)| {
            let mut p = old_abi.parameters[old as usize].clone();
            p.source = i as u32;
            p
        })
        .collect();
    for path in &mut abi.nonretaining {
        path.parameter = new_of[path.parameter as usize];
    }
    abi.nonretaining.sort();
    for entry in &mut abi.scoped_entries {
        entry.parameter.parameter = new_of[entry.parameter.parameter as usize];
    }
    abi.scoped_entries.sort();
    abi.scoped_contract =
        hirsel_lir::abi::scoped_contract_digest(&abi.nonretaining, &abi.nonretaining_captures);
    let order_bytes: Vec<u8> = order.iter().flat_map(|o| o.to_le_bytes()).collect();
    abi.key.callable = hirsel_syntax::ContentHash::of_parts(
        "hirsel-rc.permuted-parameters",
        [old_abi.key.callable.0.as_slice(), order_bytes.as_slice()],
    );
    let new_abi_id = hirsel_lir::abi_types::FnAbiId(lowered.table.fn_abis.len() as u32);
    lowered.table.fn_abis.push(abi);
    lowered.instances.abis[id.index()] = new_abi_id;
    for p in &mut lowered.instances.views[id.index()].parameters {
        p.instance = new_of[p.instance as usize];
    }
    let remap_destination = |d: &mut DestinationClass| {
        if let DestinationClass::Caller(ps) = d {
            for p in ps {
                *p = new_of[*p as usize];
            }
        }
    };
    // The helper's own unit.
    let unit = &mut lowered.units[id.index()];
    unit.header.abi = new_abi_id;
    let old_parameters = unit.parameters.clone();
    unit.parameters = order
        .iter()
        .map(|&old| old_parameters[old as usize])
        .collect();
    for slot in &mut unit.slots {
        match &mut slot.kind {
            SlotKind::Parameter(k) | SlotKind::LoopCarried(k) => *k = new_of[*k as usize],
            _ => {}
        }
    }
    for c in &mut unit.tables.constructs {
        remap_destination(&mut c.destination);
    }
    for c in &mut unit.tables.closures {
        remap_destination(&mut c.destination);
    }
    // Every call of it, in every unit.
    for unit in &mut lowered.units {
        for call in &mut unit.tables.calls {
            let Callee::Direct(symbol) = call.callee else {
                continue;
            };
            if !names_instance(&unit.tables.symbols[symbol.index()]) {
                continue;
            }
            call.abi = new_abi_id;
            let range = call.arguments;
            for a in &mut unit.pools.call_arguments
                [range.start as usize..(range.start + range.len) as usize]
            {
                a.parameter = new_of[a.parameter as usize];
                debug_assert!(matches!(
                    a.value,
                    ArgumentValue::Value(_) | ArgumentValue::Address { .. }
                ));
            }
        }
    }
    Ok(new_abi_id)
}
