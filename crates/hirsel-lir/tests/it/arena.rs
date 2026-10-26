//! Arena destinations: what C checks so that a
//! forged arena destination is rejected. Each mutation is paired with its
//! valid twin and fails only by the rule it names.
use crate::support;

use hirsel_core_contract::Passing;
use hirsel_lir::effect::Opcode;
use hirsel_lir::ir::*;
use hirsel_lir::stage::{Placed, Value};
use support::*;

const TOKEN: (Passing, hirsel_lir::layout_types::LayoutId) = (Passing::Borrow, l::ARENA);

/// A block built in the arena whose token a parameter holds, then returned.
/// `mutant` 1 names a slot of another layout as the arena instance; 2 names
/// a local slot that never holds a token.
fn placed(mutant: u8) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[TOKEN], Some(l::BLK));
    let s = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
    let token = t.version_of(s);
    t.param(token);
    let other = match mutant {
        1 => Some(t.slot(l::I64, SlotKind::Local, ROOT)),
        2 => Some(t.slot(l::ARENA, SlotKind::Local, ROOT)),
        _ => None,
    };
    let slot = other.unwrap_or(s);
    t.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let one = t.int(ENTRY);
    let two = t.int(ENTRY);
    let symbol = t.symbol(SymbolKind::Type);
    let site = t.construct_site(
        ConstructorKind::Record(symbol),
        l::PAY,
        &[(0, one), (1, two)],
        Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)))),
    );
    t.builder.unit.tables.constructs[site.index()].destination = DestinationClass::Arena(slot);
    let v = t.owned(l::BLK);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result: v,
            construct: site,
            failure,
        },
    );
    if let Some(other) = other {
        t.builder.push(ENTRY, Inst::StorageLive { slot: other });
    }
    t.builder.ret(ENTRY, Some(v));
    t.check()
}

#[test]
fn an_arena_instance_names_a_slot_holding_a_token() {
    pair(
        |m| placed(u8::from(m)),
        |k| *k == K::Placement("an arena instance whose slot holds no token"),
    );
}

#[test]
fn an_arena_placement_reads_an_initialized_token() {
    pair(
        |m| placed(if m { 2 } else { 0 }),
        |k| matches!(k, K::Uninitialized(_) | K::DeadStorage(_)),
    );
}

/// A token passed on to a callee: the parameter's (valid), or one a call
/// returned (forged: only an opening, a region's token, a read or copy of
/// token storage, a parameter or a block parameter defines one).
fn passed(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[TOKEN], None);
    let token = t.plain(l::ARENA);
    t.param(token);
    let token = if mutant {
        let n = t.builder.block(BlockKind::Body, &[]);
        let fail = t.builder.propagate();
        let made = t
            .call(ENTRY, &[], Some(l::ARENA), &[], (n, &[]), fail)
            .expect("a result");
        let m = t.builder.block(BlockKind::Body, &[]);
        let fail = t.builder.propagate();
        t.call(
            n,
            &[TOKEN],
            None,
            &[ArgumentValue::Value(made)],
            (m, &[]),
            fail,
        );
        t.builder.ret(m, None);
        return t.check();
    } else {
        token
    };
    let n = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.propagate();
    t.call(
        ENTRY,
        &[TOKEN],
        None,
        &[ArgumentValue::Value(token)],
        (n, &[]),
        fail,
    );
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn an_arena_token_is_never_forged() {
    pair(passed, |k| *k == K::Type("forged arena token"));
}

/// An arena region's token passed to a callee before (valid) or after
/// (mutant) the region exits.
fn token_after_exit(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[], None);
    t.builder.unit.tables.regions.push(RegionInstance {
        scope: ROOT,
        anchor: SourceAnchorId(0),
        seal_demand: false,
    });
    let r = RegionInstanceId(0);
    let mark = t.owned(l::PTR);
    t.builder.push(
        ENTRY,
        Inst::RegionEnter {
            result: mark,
            region: r,
        },
    );
    let token = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaToken {
            result: token,
            mark,
            budget: None,
        },
    );
    let exit = |t: &mut Lir<Placed>, b: BlockId| {
        let failure = t.fails();
        t.builder.push(
            b,
            Inst::RegionExit {
                mark,
                region: r,
                failure,
            },
        );
    };
    let n = t.builder.block(BlockKind::Body, &[]);
    let failed = t.builder.block(BlockKind::FailureCleanup, &[]);
    if mutant {
        exit(&mut t, ENTRY);
        t.builder.term(failed, Terminator::Propagate);
        t.call(
            ENTRY,
            &[TOKEN],
            None,
            &[ArgumentValue::Value(token)],
            (n, &[]),
            failed,
        );
    } else {
        exit(&mut t, failed);
        t.builder.term(failed, Terminator::Propagate);
        t.call(
            ENTRY,
            &[TOKEN],
            None,
            &[ArgumentValue::Value(token)],
            (n, &[]),
            failed,
        );
        exit(&mut t, n);
    }
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn an_arena_token_is_used_only_while_its_region_is_entered() {
    pair(token_after_exit, |k| matches!(k, K::UseAfterRegionExit(_)));
}

/// `arena.with`'s library body opens its arena at entry (valid); an opening
/// in a later block (mutant) would leave exits that never entered it.
fn opened(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Value>::new(&[], None);
    t.builder.unit.header.origin = UnitOrigin::Library;
    let token = t.plain(l::ARENA);
    let n = t.builder.block(BlockKind::Body, &[]);
    let at = if mutant { n } else { ENTRY };
    t.builder.push(
        at,
        Inst::ArenaOpen {
            result: token,
            budget: None,
        },
    );
    t.builder.jump(ENTRY, n, &[]);
    t.builder.ret(n, None);
    t.check()
}

#[test]
fn an_arena_opens_only_at_a_library_entry() {
    pair(opened, |k| {
        *k == K::Placement("an arena opened outside the entry block")
    });
    // A source unit never opens one.
    let mut t = Lir::<Value>::new(&[], None);
    let token = t.plain(l::ARENA);
    t.builder.push(
        ENTRY,
        Inst::ArenaOpen {
            result: token,
            budget: None,
        },
    );
    t.builder.ret(ENTRY, None);
    assert!(t.check().contains(&K::Vocabulary(Opcode::ArenaOpen)));
}

/// An arena child is known from its construction; the forged twin changes
/// just its placement to Heap while leaving the parent's elision claim set.
fn child_credit(mutant: bool) -> Vec<K> {
    let mut t = Lir::<hirsel_lir::stage::Rc>::new(&[TOKEN], Some(l::PAIRBLK));
    let slot = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    t.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let left = t.new_block(ENTRY, &[]);
    let first = t.builder.unit.tables.constructs.len() - 1;
    let right = t.new_block(ENTRY, &[Undo::Release(left)]);
    for site in &mut t.builder.unit.tables.constructs[first..] {
        site.destination = DestinationClass::Arena(slot);
        site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
    }
    if mutant {
        t.builder.unit.tables.constructs[first].destination = DestinationClass::Heap;
        t.builder.unit.tables.constructs[first].placement =
            Some(Placement::Fixed(StorageOwner::Heap(TaskDomainId(0))));
    }
    let site = t.construct_site(
        ConstructorKind::Tuple,
        l::PAIR,
        &[(0, left), (1, right)],
        Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)))),
    );
    t.builder.unit.tables.constructs[site.index()].destination = DestinationClass::Arena(slot);
    t.builder.unit.tables.constructs[site.index()].arena_no_drop = true;
    let result = t.owned(l::PAIRBLK);
    let failure = t.fails();
    t.builder.push(
        ENTRY,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn an_elided_child_credit_walk_needs_same_arena_children() {
    pair(child_credit, |k| {
        *k == K::Placement("unproved arena child-credit elision")
    });
}

/// C proves a result summary from the actual returned block, not its claim.
fn result_summary(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[TOKEN], Some(l::BLK));
    let slot = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    t.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let result = t.new_block(ENTRY, &[]);
    if !mutant {
        let site = t.builder.unit.tables.constructs.last_mut().unwrap();
        site.destination = DestinationClass::Arena(slot);
        site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
    }
    t.table.fn_abis[t.builder.unit.header.abi.0 as usize].result_arena = Some(0);
    t.builder.ret(ENTRY, Some(result));
    t.check()
}

#[test]
fn a_result_arena_summary_needs_every_return_in_that_arena() {
    pair(result_summary, |k| {
        *k == K::Placement("unproved result arena")
    });
}

fn call_child(mutant: bool) -> Vec<K> {
    let mut t = Lir::<Placed>::new(&[TOKEN], Some(l::NICHEBLK));
    let slot = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(slot);
    t.param(version);
    t.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let address = t.at(ENTRY, slot, &[], &[]);
    let token = t.read(ENTRY, address, version, ReadMode::Bits, None);
    let next = t.builder.block(BlockKind::Body, &[]);
    let fail = t.builder.propagate();
    let child = t
        .call(
            ENTRY,
            &[TOKEN],
            Some(l::BLK),
            &[ArgumentValue::Value(token)],
            (next, &[]),
            fail,
        )
        .unwrap();
    let call = t.builder.unit.tables.calls.last().unwrap();
    if !mutant {
        t.table.fn_abis[call.abi.0 as usize].result_arena = Some(0);
    }
    let symbol = t.symbol(SymbolKind::Type);
    let site = t.construct_site(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        l::NICHE,
        &[(0, child), (1, token)],
        Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)))),
    );
    // The niche payload is (BLK, i64), not (BLK, Arena).
    let number = t.int(next);
    let range = t.builder.unit.tables.constructs[site.index()].fields;
    t.builder.unit.pools.fields[range.start as usize + 1].value = number;
    t.builder.unit.tables.constructs[site.index()].destination = DestinationClass::Arena(slot);
    t.builder.unit.tables.constructs[site.index()].arena_no_drop = true;
    let result = t.owned(l::NICHEBLK);
    let failure = t.fails();
    t.builder.push(
        next,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    t.builder.ret(next, Some(result));
    t.check()
}

#[test]
fn an_elided_call_child_needs_its_callees_result_arena_summary() {
    pair(call_child, |k| {
        *k == K::Placement("unproved arena child-credit elision")
    });
}

/// The complete-link-set check binds a call's claimed ABI to the actual
/// callee. Two signature rows with identical physical layouts do not let a
/// caller invent a summary for that callee.
#[test]
fn a_call_cannot_forge_its_callees_result_summary() {
    use hirsel_lir::validate::check_result_arena_calls;
    for mutant in [false, true] {
        let mut t = Lir::<Placed>::new(&[TOKEN], Some(l::BLK));
        let slot = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
        let version = t.version_of(slot);
        t.param(version);
        let address = t.at(ENTRY, slot, &[], &[]);
        let token = t.read(ENTRY, address, version, ReadMode::Bits, None);
        let own = t.builder.unit.header.abi;
        t.table.fn_abis[own.0 as usize].result_arena = Some(0);
        let call_abi = if mutant {
            let mut abi = t.table.fn_abis[own.0 as usize].clone();
            abi.result_arena = None;
            let id = hirsel_lir::FnAbiId(t.table.fn_abis.len() as u32);
            t.table.fn_abis.push(abi);
            id
        } else {
            own
        };
        let symbol = t.symbol(SymbolKind::Function);
        let next = t.builder.block(BlockKind::Body, &[]);
        let fail = t.builder.propagate();
        let result = t
            .call_to(
                ENTRY,
                Callee::Direct(symbol),
                own,
                &[TOKEN],
                Some(l::BLK),
                &[ArgumentValue::Value(token)],
                (next, &[]),
                fail,
            )
            .unwrap();
        t.builder.ret(next, Some(result));
        let (mut caller, table, _) = t.finish();
        let mut callee = caller.clone();
        let callee_name = path("callee");
        let instance = hash("callee instance");
        callee.header.definition = callee_name.clone();
        callee.header.instance = instance;
        callee.header.abi = call_abi;
        caller.tables.symbols[symbol.index()].definition = callee_name;
        caller.tables.symbols[symbol.index()].instance = Some(instance);
        callee.tables.symbols[symbol.index()] = caller.tables.symbols[symbol.index()].clone();
        let result = check_result_arena_calls(&[caller, callee], &table);
        assert_eq!(result.is_err(), mutant, "{result:?}");
    }
}

/// A single static store in a loop is not a stable token for the whole frame.
/// The unflagged twin retains dynamic checking; this conservative proof must
/// reject an elision claim based only on counting store instructions.
fn loop_token(entry_init: bool, mutant: bool) -> Vec<K> {
    let mut t = Lir::<hirsel_lir::stage::Rc>::new(&[TOKEN], None);
    let param = t.slot(l::ARENA, SlotKind::Parameter(0), ROOT);
    let version = t.version_of(param);
    t.param(version);
    let address = t.at(ENTRY, param, &[], &[]);
    let token = t.read(ENTRY, address, version, ReadMode::Bits, None);
    let slot = t.slot(l::ARENA, SlotKind::Local, ROOT);
    let address = t.at(ENTRY, slot, &[], &[]);
    t.builder.unit.tables.arenas.push(ExternalInstance { slot });
    let body = t.builder.block(BlockKind::Body, &[]);
    t.builder.jump(ENTRY, body, &[]);
    let init = if entry_init { ENTRY } else { body };
    t.builder.push(init, Inst::StorageLive { slot });
    t.store(init, address, token, slot);
    let child = t.new_block(body, &[]);
    let child_site = t.builder.unit.tables.constructs.last_mut().unwrap();
    child_site.destination = DestinationClass::Arena(slot);
    child_site.placement = Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0))));
    let n = t.int(body);
    let symbol = t.symbol(SymbolKind::Type);
    let site = t.construct_site(
        ConstructorKind::Variant {
            definition: symbol,
            variant: 1,
        },
        l::NICHE,
        &[(0, child), (1, n)],
        Some(Placement::Fixed(StorageOwner::Arena(ArenaInstanceId(0)))),
    );
    t.builder.unit.tables.constructs[site.index()].destination = DestinationClass::Arena(slot);
    t.builder.unit.tables.constructs[site.index()].arena_no_drop = mutant;
    let result = t.owned(l::NICHEBLK);
    let failure = t.fails();
    t.builder.push(
        body,
        Inst::Construct {
            result,
            construct: site,
            failure,
        },
    );
    t.release(body, result, &[]);
    if !entry_init {
        t.builder.push(
            body,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    t.builder.jump(body, body, &[]);
    t.check()
}

#[test]
fn a_store_in_a_loop_does_not_prove_a_frame_stable_arena_token() {
    assert!(
        loop_token(true, true).is_empty(),
        "an entry-initialized local token is stable"
    );
    pair(
        |mutant| loop_token(false, mutant),
        |k| *k == K::Placement("unproved arena child-credit elision"),
    );
}

/// Only the distinguished Heap literal is a constant allocator token; zero
/// integer bits do not become a token by being given the Arena representation.
#[test]
fn the_heap_literal_is_a_token_but_integer_bits_are_not() {
    pair(
        |mutant| {
            let mut t = Lir::<Placed>::new(&[], None);
            let result = t.plain(l::ARENA);
            let ty = t.builder.unit.values[result.index()].ty;
            t.builder.unit.tables.constants.push(ConstantData {
                ty,
                value: if mutant {
                    ConstantValue::Integer(0)
                } else {
                    ConstantValue::Heap
                },
            });
            t.builder.push(
                ENTRY,
                Inst::Const {
                    result,
                    constant: ConstantId(0),
                },
            );
            let next = t.builder.block(BlockKind::Body, &[]);
            let fail = t.builder.propagate();
            t.call(
                ENTRY,
                &[TOKEN],
                None,
                &[ArgumentValue::Value(result)],
                (next, &[]),
                fail,
            );
            t.builder.ret(next, None);
            t.check()
        },
        |k| matches!(k, K::Type("constant" | "forged arena token")),
    );
}
