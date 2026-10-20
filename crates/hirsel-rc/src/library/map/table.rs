//! Map storage maintenance. Rebuilds move entries and reuse saved hashes;
//! only a cold immutable image needs to acquire hashes for this run's seed.
use super::*;

pub(super) fn entry_type(b: &mut Body<'_, '_>) -> R<TypeId> {
    let k = b.cx.type_actual(0)?;
    let v = if b.cx.arguments.len() == 2 {
        b.cx.type_actual(1)?
    } else {
        b.cx.store.push(Type::unit())
    };
    Ok(add_type(
        &mut b.cx.store,
        TypeKind::Named {
            definition: native_path("MapEntry"),
            arguments: vec![GenericArgument::Type(k), GenericArgument::Type(v)].into(),
        },
    ))
}

fn map_type(b: &mut Body<'_, '_>) -> R<TypeId> {
    let k = b.cx.type_actual(0)?;
    let v = if b.cx.arguments.len() == 2 {
        b.cx.type_actual(1)?
    } else {
        b.cx.store.push(Type::unit())
    };
    Ok(if b.cx.arguments.len() == 2 {
        builtin(b, BuiltinType::Map, &[k, v])
    } else {
        builtin(b, BuiltinType::Set, &[k])
    })
}
pub(super) fn entry_value(
    b: &mut Body<'_, '_>,
    layout: LayoutId,
    value: Option<ValueId>,
) -> R<ValueId> {
    let definition = b.symbol(Symbol {
        kind: SymbolKind::Type,
        definition: native_path("MapEntry"),
        instance: None,
    });
    construct(
        b,
        layout,
        ConstructorKind::Variant {
            definition,
            variant: u32::from(value.is_some()),
        },
        &value.into_iter().collect::<Vec<_>>(),
    )
}
fn append(b: &mut Body<'_, '_>, list: AddressId, ty: TypeId, value: ValueId) -> R<()> {
    let list_ty = builtin(b, BuiltinType::List, &[ty]);
    let unit = b.cx.store.push(Type::unit());
    let push = helper(
        b,
        "list.push",
        &[(PassingMode::Mut, list_ty), (PassingMode::Take, ty)],
        unit,
        Allocation::MayAllocate,
        vec![GenericArgument::Type(ty)],
    )?;
    let list = mut_argument(b, 0, list)?;
    call(b, push, &[list, argument(1, Passing::Take, value)])?;
    Ok(())
}
fn maintenance(b: &mut Body<'_, '_>, name: &str) -> R<()> {
    let map = map_type(b)?;
    let unit = b.cx.store.push(Type::unit());
    let actuals = b.cx.arguments.clone();
    let name = if b.cx.arguments.len() == 1 {
        match name {
            "map_prepare" => "set_prepare",
            "map_warm" => "set_warm",
            _ => name,
        }
    } else {
        name
    };
    let direct = helper(
        b,
        name,
        &[(PassingMode::Mut, map)],
        unit,
        Allocation::MayAllocate,
        actuals,
    )?;
    let map = mut_argument(b, 0, b.parameters[0].address)?;
    call(b, direct, &[map])?;
    Ok(())
}
// Rebuilding needs two immutable lists and one mutable index. Keeping the
// mutation in its own parameter also keeps borrowed entry addresses stable.
fn update_index(b: &mut Body<'_, '_>) -> R<()> {
    let entry = entry_type(b)?;
    let entries_ty = builtin(b, BuiltinType::List, &[entry]);
    let word = add_type(&mut b.cx.store, TypeKind::Primitive(Primitive::U64));
    let words_ty = builtin(b, BuiltinType::List, &[word]);
    let unit = b.cx.store.push(Type::unit());
    let actuals = b.cx.arguments.clone();
    let direct = helper(
        b,
        if b.cx.arguments.len() == 2 {
            "map_index"
        } else {
            "set_index"
        },
        &[
            (PassingMode::Borrow, entries_ty),
            (PassingMode::Borrow, words_ty),
            (PassingMode::Mut, words_ty),
        ],
        unit,
        Allocation::MayAllocate,
        actuals,
    )?;
    let container = b.parameters[0].address;
    let index = field(b, container, 1)?;
    let entries = field(b, container, 0)?;
    let hashes = field(b, container, 2)?;
    let (entries, entries_loan) = b.borrow(entries, false)?;
    let (hashes, hashes_loan) = b.borrow(hashes, false)?;
    let index = mut_argument(b, 2, index)?;
    let symbol = b.symbol(direct.symbol);
    b.call(
        Callee::Direct(symbol),
        vec![],
        direct.abi,
        vec![
            argument(0, Passing::Borrow, entries),
            argument(1, Passing::Borrow, hashes),
            index,
        ],
        &[entries_loan, hashes_loan],
    );
    Ok(())
}

/// Charge a missing upsert before invoking its initializer. The reservation
/// belongs to this Map even if the initializer fails (MEM-6 never refunds).
pub(super) fn prepare_append(b: &mut Body<'_, '_>) -> R<()> {
    maintenance(b, "map_prepare")?;
    let entries = field(b, b.parameters[0].address, 0)?;
    b.arena_push(entries)?;
    Ok(())
}
pub(super) fn append_map(
    b: &mut Body<'_, '_>,
    hash: ValueId,
    key: AddressId,
    value: AddressId,
) -> R<()> {
    append_map_inner(b, hash, key, value, false)
}
pub(super) fn append_prepared(
    b: &mut Body<'_, '_>,
    hash: ValueId,
    key: AddressId,
    value: AddressId,
) -> R<()> {
    append_map_inner(b, hash, key, value, true)
}
pub(super) fn append_set(b: &mut Body<'_, '_>, hash: ValueId, key: AddressId) -> R<()> {
    let unit_ty = b.cx.store.push(Type::unit());
    let layout = b.cx.layout(unit_ty)?;
    let (slot, value) = temporary(b, layout)?;
    let unit = construct(b, layout, ConstructorKind::Tuple, &[])?;
    b.store(value, unit)?;
    b.hold(slot, value);
    append_map_inner(b, hash, key, value, false)?;
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    Ok(())
}
fn append_map_inner(
    b: &mut Body<'_, '_>,
    hash: ValueId,
    key: AddressId,
    value: AddressId,
    prepared: bool,
) -> R<()> {
    if !prepared {
        maintenance(b, "map_prepare")?;
    }
    let container = b.parameters[0].address;
    let entries = field(b, container, 0)?;
    let hashes = field(b, container, 2)?;
    let live = field(b, container, 3)?;
    let count = b.read_bits(live)?;
    let one = b.u64(1)?;
    let count = b.add(ArithMode::Wrapping, count, one);
    let element = b.cx.element(b.address_layout(entries))?;
    let pair = b.record(element).tag.as_ref().unwrap().variants[1].fields[0].layout;
    let reserved = if prepared {
        let n = len(b, entries)?;
        let next = b.add(ArithMode::Wrapping, n, one);
        b.seq_reserve(entries, next)?;
        Some((b.slot_place(entries, n)?, next))
    } else {
        None
    };
    let key = b.take(key)?;
    let value = b.take(value)?;
    let pair = construct(b, pair, ConstructorKind::Tuple, &[key, value])?;
    let value = entry_value(b, element, Some(pair))?;
    if let Some((slot, next)) = reserved {
        b.store(slot, value)?;
        b.seq_set_len(entries, next)?;
    } else {
        let ty = entry_type(b)?;
        append(b, entries, ty, value)?;
    }
    overwrite(b, live, count)?;
    let word = add_type(&mut b.cx.store, TypeKind::Primitive(Primitive::U64));
    append(b, hashes, word, hash)?;
    update_index(b)
}

/// Return the first live physical slot at or after `first`, or the slot count.
/// The borrowed loop uses this once per iteration, so holes are visited once.
pub(super) fn advance(b: &mut Body<'_, '_>, entries: AddressId, first: ValueId) -> R<ValueId> {
    let count = len(b, entries)?;
    // Borrowed cursors use a raw scalar layout for their position word.
    // Preserve that layout through the scan and the final cursor write.
    let word = b.layout_of(first);
    let converted = b.fresh(word);
    b.push(Inst::Recast {
        result: converted,
        value: count,
    });
    let count = converted;
    let i = b.fresh(word);
    let scan = b.block(BlockKind::Body, &[i]);
    let inspect = b.block(BlockKind::Body, &[]);
    let live = b.block(BlockKind::Body, &[]);
    let absent = b.block(BlockKind::Body, &[]);
    let result = b.fresh(word);
    let done = b.block(BlockKind::Body, &[result]);
    b.jump(scan, &[first]);
    b.at(scan);
    let more = b.compare(BinaryOp::Lt, i, count)?;
    let yes = b.edge(inspect, &[]);
    let no = b.edge(done, &[count]);
    b.terminate(Terminator::Branch {
        condition: more,
        yes,
        no,
    });
    b.at(inspect);
    let entry = b.slot_place(entries, i)?;
    let (view, loan) = b.borrow(entry, false)?;
    let yes = b.edge(live, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm {
            value: 1,
            edge: yes,
        }],
    );
    let otherwise = b.edge(absent, &[]);
    b.terminate(Terminator::Switch {
        value: view,
        arms,
        otherwise,
    });
    b.at(live);
    b.end_loan(loan);
    b.jump(done, &[i]);
    b.at(absent);
    b.end_loan(loan);
    let one = b.constant(word, ConstantValue::Integer(1));
    let next = b.add(ArithMode::Wrapping, i, one);
    b.jump(scan, &[next]);
    b.at(done);
    Ok(result)
}
pub(super) fn map_advance(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let container = b.parameters[0].address;
    let entries = field(&mut b, container, 0)?;
    let first = b.read_bits(b.parameters[1].address)?;
    let result = advance(&mut b, entries, first)?;
    b.ret(Some(result));
    Ok(b.finish())
}

fn displaced(b: &mut Body<'_, '_>, address: AddressId) -> R<(SlotId, AddressId)> {
    let layout = b.address_layout(address);
    let slot = b.slot(layout, SlotKind::Displaced);
    b.push(Inst::StorageLive { slot });
    let old = b.place(AddressBase::Slot(slot), &[], layout, None)?;
    Ok((slot, old))
}
fn exchange(
    b: &mut Body<'_, '_>,
    address: AddressId,
    value: ValueId,
    (slot, old): (SlotId, AddressId),
) -> R<(SlotId, AddressId)> {
    let root = b.root(address).unwrap();
    let version = b.version(root);
    b.set_reaching(root, version);
    let displaced_version = b.version(slot);
    b.set_reaching(slot, displaced_version);
    let failure = b.failure(None);
    b.push(Inst::Replace {
        address,
        value,
        displaced: slot,
        version,
        displaced_version,
        if_live: false,
        failure,
    });
    b.hold(slot, old);
    Ok((slot, old))
}
fn retire(b: &mut Body<'_, '_>, slot: SlotId, address: AddressId) {
    drop_place(b, address);
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
}

pub(super) fn remove_set(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    remove(cx, false)
}
pub(super) fn remove_map(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    remove(cx, true)
}
fn remove(cx: &mut Cx<'_>, is_map: bool) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let probe = probe_call(&mut b)?;
    let container = b.parameters[0].address;
    let entries = field(&mut b, container, 0)?;
    let count = len(&mut b, entries)?;
    let hit = b.compare(BinaryOp::Lt, probe.ordinal, count)?;
    let found = b.block(BlockKind::Body, &[]);
    let missing = b.block(BlockKind::Body, &[]);
    b.branch(hit, found, missing);
    b.at(missing);
    let result = if is_map { option(&mut b, None)? } else { hit };
    b.ret(Some(result));
    b.at(found);
    let bucket = warm_for_removal(&mut b, &probe, count)?;
    let entries = field(&mut b, container, 0)?;
    let entry = b.slot_place(entries, probe.ordinal)?;
    let element = b.address_layout(entry);
    let removed = displaced(&mut b, entry)?;
    let none = entry_value(&mut b, element, None)?;
    let (slot, removed) = exchange(&mut b, entry, none, removed)?;
    // The displaced entry is a complete owner through all metadata writes.
    let live = field(&mut b, container, 3)?;
    let count = b.read_bits(live)?;
    let one = b.u64(1)?;
    let count = sub(&mut b, count, one);
    overwrite(&mut b, live, count)?;
    let index = field(&mut b, container, 1)?;
    let capacity = len(&mut b, index)?;
    let zero = b.u64(0)?;
    let indexed = b.compare(BinaryOp::Ne, capacity, zero)?;
    let mark = b.block(BlockKind::Body, &[]);
    let done_v = b.version(b.parameters[0].slot);
    let done = b.block(BlockKind::Body, &[done_v]);
    let before = b.reaching(b.parameters[0].slot)?;
    let yes = b.edge(mark, &[]);
    let no = b.edge(done, &[before]);
    b.terminate(Terminator::Branch {
        condition: indexed,
        yes,
        no,
    });
    b.at(mark);
    let bucket = b.slot_place(index, bucket)?;
    let tombstone = b.u64(1 << 32)?;
    overwrite(&mut b, bucket, tombstone)?;
    let after = b.reaching(b.parameters[0].slot)?;
    b.jump(done, &[after]);
    b.at(done);
    b.set_reaching(b.parameters[0].slot, done_v);
    let pair = live_pair(&mut b, removed)?;
    let key = field(&mut b, pair, 0)?;
    let value = field(&mut b, pair, 1)?;
    let key_layout = b.address_layout(key);
    let (key_slot, key_owner) = temporary(&mut b, key_layout)?;
    // Keep a returned value live while dropping the removed key.
    let layout = b.address_layout(value);
    let (value_slot, owner) = temporary(&mut b, layout)?;
    let value = b.take(value)?;
    let key = b.take(key)?;
    b.forget_held(removed);
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    b.store(owner, value)?;
    b.hold(value_slot, owner);
    b.store(key_owner, key)?;
    b.hold(key_slot, key_owner);
    retire(&mut b, key_slot, key_owner);
    let value = b.take(owner)?;
    b.push(Inst::StorageDead {
        slot: value_slot,
        if_live: false,
    });
    let result = if is_map {
        option(&mut b, Some(value))?
    } else {
        hit
    };
    b.ret(Some(result));
    Ok(b.finish())
}

pub(super) fn map_prepare(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let container = b.parameters[0];
    let entries = field(&mut b, container.address, 0)?;
    let hashes = field(&mut b, container.address, 2)?;
    let n = len(&mut b, entries)?;
    let cached = len(&mut b, hashes)?;
    let word = b.u64_layout()?;
    let one = b.u64(1)?;
    let zero = b.u64(0)?;
    let i = b.fresh(word);
    let version = b.version(container.slot);
    let scan = b.block(BlockKind::Body, &[i, version]);
    let fill = b.block(BlockKind::Body, &[]);
    let filled = b.block(BlockKind::Body, &[]);
    let before = b.reaching(container.slot)?;
    b.jump(scan, &[cached, before]);
    b.at(scan);
    b.set_reaching(container.slot, version);
    let more = b.compare(BinaryOp::Lt, i, n)?;
    b.branch(more, fill, filled);
    b.at(fill);
    let entry = b.slot_place(entries, i)?;
    let (view, loan) = b.borrow(entry, false)?;
    let some = b.block(BlockKind::Body, &[]);
    let none = b.block(BlockKind::Body, &[]);
    let hash_value = b.fresh(word);
    let append_hash = b.block(BlockKind::Body, &[hash_value]);
    let edge = b.edge(some, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm { value: 1, edge }],
    );
    let otherwise = b.edge(none, &[]);
    b.terminate(Terminator::Switch {
        value: view,
        arms,
        otherwise,
    });
    b.at(some);
    b.end_loan(loan);
    let pair = live_pair(&mut b, entry)?;
    let key = field(&mut b, pair, 0)?;
    let key_hash = hash(&mut b, key)?;
    b.jump(append_hash, &[key_hash]);
    b.at(none);
    b.end_loan(loan);
    b.jump(append_hash, &[zero]);
    b.at(append_hash);
    let ty = add_type(&mut b.cx.store, TypeKind::Primitive(Primitive::U64));
    append(&mut b, hashes, ty, hash_value)?;
    let next = b.add(ArithMode::Wrapping, i, one);
    let after = b.reaching(container.slot)?;
    b.jump(scan, &[next, after]);
    b.at(filled);
    b.set_reaching(container.slot, version);
    if b.cx.path == native_path("map_warm") || b.cx.path == native_path("set_warm") {
        // Only cold immutable/decoded maps enter this body for removal. Build
        // their first index once; retain every physical ordinal.
        update_index(&mut b)?;
        unit(&mut b);
        return Ok(b.finish());
    }
    let live = field(&mut b, container.address, 3)?;
    let live = b.read_bits(live)?;
    let dead = sub(&mut b, n, live);
    let enough = b.compare(BinaryOp::Ge, dead, live)?;
    let sixteen = b.u64(16)?;
    let large = b.compare(BinaryOp::Ge, n, sixteen)?;
    let compact = binary(&mut b, BinaryOp::BitAnd, enough, large);
    let compact_block = b.block(BlockKind::Body, &[]);
    let end_v = b.version(container.slot);
    let done = b.block(BlockKind::Body, &[end_v]);
    let before = b.reaching(container.slot)?;
    let yes = b.edge(compact_block, &[]);
    let no = b.edge(done, &[before]);
    b.terminate(Terminator::Branch {
        condition: compact,
        yes,
        no,
    });
    b.at(compact_block);
    compact_entries(&mut b, n, live)?;
    let after = b.reaching(container.slot)?;
    b.jump(done, &[after]);
    b.at(done);
    b.set_reaching(container.slot, end_v);
    unit(&mut b);
    Ok(b.finish())
}

fn compact_entries(b: &mut Body<'_, '_>, n: ValueId, live: ValueId) -> R<()> {
    let container = b.parameters[0];
    // Discard ordinals before moving data. A later failed preparation leaves a
    // cold index, whose keyed reads can still use the saved hashes.
    let index = field(b, container.address, 1)?;
    let index_layout = b.address_layout(index);
    let old = displaced(b, index)?;
    let empty = construct(b, index_layout, ConstructorKind::Sequence, &[])?;
    let (slot, old) = exchange(b, index, empty, old)?;
    retire(b, slot, old);
    // Pin the owner and both backing blocks before moving an entry. Detaches
    // can fail; after these loans begin, compaction only moves initialized data.
    let (_, parent_loan) = b.borrow(container.address, true)?;
    let parent_failures = b.ed.unit.tables.failures.len();
    let entries = field(b, container.address, 0)?;
    let hashes = field(b, container.address, 2)?;
    let (_, entries_loan) = b.borrow(entries, true)?;
    let entries_failures = b.ed.unit.tables.failures.len();
    let (_, hashes_loan) = b.borrow(hashes, true)?;
    let hashes_failures = b.ed.unit.tables.failures.len();
    let root = container.slot;
    let word = b.u64_layout()?;
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let from = b.fresh(word);
    let to = b.fresh(word);
    let version = b.version(root);
    let scan = b.block(BlockKind::Body, &[from, to, version]);
    let inspect = b.block(BlockKind::Body, &[]);
    let complete = b.block(BlockKind::Body, &[]);
    let before = b.reaching(root)?;
    b.jump(scan, &[zero, zero, before]);
    b.at(scan);
    b.set_reaching(root, version);
    let more = b.compare(BinaryOp::Lt, from, n)?;
    b.branch(more, inspect, complete);
    b.at(inspect);
    let entry = b.slot_place(entries, from)?;
    let (view, loan) = b.borrow(entry, false)?;
    let yes = b.block(BlockKind::Body, &[]);
    let no = b.block(BlockKind::Body, &[]);
    let edge = b.edge(yes, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm { value: 1, edge }],
    );
    let otherwise = b.edge(no, &[]);
    b.terminate(Terminator::Switch {
        value: view,
        arms,
        otherwise,
    });
    b.at(no);
    b.end_loan(loan);
    let next = b.add(ArithMode::Wrapping, from, one);
    b.jump(scan, &[next, to, version]);
    b.at(yes);
    b.end_loan(loan);
    let same = b.compare(BinaryOp::Eq, from, to)?;
    let move_entry = b.block(BlockKind::Body, &[]);
    let next_v = b.version(root);
    let next = b.block(BlockKind::Body, &[next_v]);
    let yes = b.edge(next, &[version]);
    let no = b.edge(move_entry, &[]);
    b.terminate(Terminator::Branch {
        condition: same,
        yes,
        no,
    });
    b.at(move_entry);
    let target = b.slot_place(entries, to)?;
    let source_hash = b.slot_place(hashes, from)?;
    let target_hash = b.slot_place(hashes, to)?;
    let hash = b.read_bits(source_hash)?;
    let removed = displaced(b, entry)?;
    let target_old = displaced(b, target)?;
    let none = entry_value(b, b.address_layout(entry), None)?;
    let (slot, old) = exchange(b, entry, none, removed)?;
    let value = b.take(old)?;
    b.push(Inst::StorageDead {
        slot,
        if_live: false,
    });
    let (slot, old) = exchange(b, target, value, target_old)?;
    retire(b, slot, old);
    overwrite(b, target_hash, hash)?;
    let after = b.reaching(root)?;
    b.jump(next, &[after]);
    b.at(next);
    b.set_reaching(root, next_v);
    let from = b.add(ArithMode::Wrapping, from, one);
    let to = b.add(ArithMode::Wrapping, to, one);
    b.jump(scan, &[from, to, next_v]);
    b.at(complete);
    b.set_reaching(root, version);
    b.end_loan(hashes_loan);
    b.end_loan(entries_loan);
    b.end_loan(parent_loan);
    protect(b, parent_failures, parent_loan);
    protect(b, entries_failures, entries_loan);
    protect(b, hashes_failures, hashes_loan);
    // All slots in the suffix are None; retire that initialized suffix before
    // shortening the physical sequence. Hash words carry no owners.
    let at = b.fresh(word);
    let tail_v = b.version(root);
    let tail = b.block(BlockKind::Body, &[at, tail_v]);
    let drop = b.block(BlockKind::Body, &[]);
    let done = b.block(BlockKind::Body, &[]);
    b.jump(tail, &[live, version]);
    b.at(tail);
    b.set_reaching(root, tail_v);
    let more = b.compare(BinaryOp::Lt, at, n)?;
    b.branch(more, drop, done);
    b.at(drop);
    let entries = field(b, container.address, 0)?;
    let entry = b.slot_place(entries, at)?;
    let (slot, local) = temporary(b, b.address_layout(entry))?;
    let value = b.take(entry)?;
    b.store(local, value)?;
    b.hold(slot, local);
    retire(b, slot, local);
    let hashes = field(b, container.address, 2)?;
    let hash = b.slot_place(hashes, at)?;
    let _ = b.take(hash)?;
    let next = b.add(ArithMode::Wrapping, at, one);
    let after = b.reaching(root)?;
    b.jump(tail, &[next, after]);
    b.at(done);
    b.set_reaching(root, tail_v);
    // Changing the count invalidates element loans. Finish those first and
    // prepare each holder again after the compaction's version joins.
    let entries = field(b, container.address, 0)?;
    b.seq_set_len(entries, live)?;
    let hashes = field(b, container.address, 2)?;
    b.seq_set_len(hashes, live)?;
    Ok(())
}

pub(super) fn map_index_update(cx: &mut Cx<'_>) -> R<LirUnit<Value>> {
    let mut b = Body::new(cx)?;
    let container = b.parameters[2];
    let entries = b.parameters[0].address;
    let hashes = b.parameters[1].address;
    let index = container.address;
    let n = len(&mut b, entries)?;
    let capacity = len(&mut b, index)?;
    let eight = b.u64(8)?;
    let seven = b.u64(7)?;
    let one = b.u64(1)?;
    let zero = b.u64(0)?;
    let small = b.compare(BinaryOp::Le, n, eight)?;
    let unindexed = b.compare(BinaryOp::Eq, capacity, zero)?;
    let skip = binary(&mut b, BinaryOp::BitAnd, small, unindexed);
    let done = b.block(BlockKind::Body, &[]);
    let check = b.block(BlockKind::Body, &[]);
    b.branch(skip, done, check);
    b.at(done);
    unit(&mut b);
    b.at(check);
    let maximum = b.u64(u64::from(u32::MAX))?;
    let excessive = b.compare(BinaryOp::Gt, n, maximum)?;
    let trap = b.block(BlockKind::Body, &[]);
    let admitted = b.block(BlockKind::Body, &[]);
    b.branch(excessive, trap, admitted);
    b.at(trap);
    b.terminate(Terminator::InvariantFault {
        reason: "Map entry count exceeds u32::MAX".into(),
    });
    b.at(admitted);
    let used = binary(&mut b, BinaryOp::Mul, n, eight);
    let limit = binary(&mut b, BinaryOp::Mul, capacity, seven);
    let rebuild = b.compare(BinaryOp::Ge, used, limit)?;
    let slow = b.block(BlockKind::Body, &[]);
    let fast = b.block(BlockKind::Body, &[]);
    b.branch(rebuild, slow, fast);
    b.at(fast);
    let ordinal = sub(&mut b, n, one);
    put(&mut b, hashes, index, ordinal, capacity)?;
    unit(&mut b);
    b.at(slow);
    // A decoded/static image can have any size and an empty index.
    let sixteen = b.u64(16)?;
    let word = b.u64_layout()?;
    let wanted = b.fresh(word);
    let size = b.block(BlockKind::Body, &[wanted]);
    let grow = b.block(BlockKind::Body, &[]);
    let ready = b.block(BlockKind::Body, &[]);
    b.jump(size, &[sixteen]);
    b.at(size);
    let limit = binary(&mut b, BinaryOp::Mul, wanted, seven);
    let fits = b.compare(BinaryOp::Lt, used, limit)?;
    b.branch(fits, ready, grow);
    b.at(grow);
    let twice = b.add(ArithMode::Wrapping, wanted, wanted);
    b.jump(size, &[twice]);
    b.at(ready);
    b.seq_reserve(index, wanted)?;
    let i = b.fresh(word);
    let zero_v = b.version(container.slot);
    let zeros = b.block(BlockKind::Body, &[i, zero_v]);
    let store = b.block(BlockKind::Body, &[]);
    let filled = b.block(BlockKind::Body, &[]);
    let before = b.reaching(container.slot)?;
    b.jump(zeros, &[zero, before]);
    b.at(zeros);
    b.set_reaching(container.slot, zero_v);
    let more = b.compare(BinaryOp::Lt, i, wanted)?;
    b.branch(more, store, filled);
    b.at(store);
    let slot = b.slot_place(index, i)?;
    let initialized = b.compare(BinaryOp::Lt, i, capacity)?;
    let replace = b.block(BlockKind::Body, &[]);
    let initialize = b.block(BlockKind::Body, &[]);
    let joined_v = b.version(container.slot);
    let joined = b.block(BlockKind::Body, &[joined_v]);
    b.branch(initialized, replace, initialize);
    b.at(replace);
    overwrite(&mut b, slot, zero)?;
    let after = b.reaching(container.slot)?;
    b.jump(joined, &[after]);
    b.at(initialize);
    b.set_reaching(container.slot, zero_v);
    b.store(slot, zero)?;
    let after = b.reaching(container.slot)?;
    b.jump(joined, &[after]);
    b.at(joined);
    b.set_reaching(container.slot, joined_v);
    let next = b.add(ArithMode::Wrapping, i, one);
    b.jump(zeros, &[next, joined_v]);
    b.at(filled);
    b.set_reaching(container.slot, zero_v);
    b.seq_set_len(index, wanted)?;
    let i = b.fresh(word);
    let all_v = b.version(container.slot);
    let all = b.block(BlockKind::Body, &[i, all_v]);
    let inspect = b.block(BlockKind::Body, &[]);
    let done = b.block(BlockKind::Body, &[]);
    let before = b.reaching(container.slot)?;
    b.jump(all, &[zero, before]);
    b.at(all);
    b.set_reaching(container.slot, all_v);
    let more = b.compare(BinaryOp::Lt, i, n)?;
    b.branch(more, inspect, done);
    b.at(inspect);
    let entry = b.slot_place(entries, i)?;
    let (view, loan) = b.borrow(entry, false)?;
    let occupied = b.block(BlockKind::Body, &[]);
    let absent = b.block(BlockKind::Body, &[]);
    let next_v = b.version(container.slot);
    let next = b.block(BlockKind::Body, &[next_v]);
    let edge = b.edge(occupied, &[]);
    let arms = PoolRange::push(
        &mut b.ed.unit.pools.switch_arms,
        [SwitchArm { value: 1, edge }],
    );
    let otherwise = b.edge(absent, &[]);
    b.terminate(Terminator::Switch {
        value: view,
        arms,
        otherwise,
    });
    b.at(absent);
    b.end_loan(loan);
    b.jump(next, &[all_v]);
    b.at(occupied);
    b.end_loan(loan);
    put(&mut b, hashes, index, i, wanted)?;
    let after = b.reaching(container.slot)?;
    b.jump(next, &[after]);
    b.at(next);
    b.set_reaching(container.slot, next_v);
    let inc = b.add(ArithMode::Wrapping, i, one);
    b.jump(all, &[inc, next_v]);
    b.at(done);
    b.set_reaching(container.slot, all_v);
    unit(&mut b);
    Ok(b.finish())
}
fn put(
    b: &mut Body<'_, '_>,
    hashes: AddressId,
    index: AddressId,
    ordinal: ValueId,
    capacity: ValueId,
) -> R<()> {
    let at = b.slot_place(hashes, ordinal)?;
    let hash = b.read_bits(at)?;
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let ordinal_mask = b.u64(u64::from(u32::MAX))?;
    let tag_mask = b.u64(0xffff_ffff_0000_0000)?;
    let mask = sub(b, capacity, one);
    let start = binary(b, BinaryOp::BitAnd, hash, mask);
    let tag = binary(b, BinaryOp::BitAnd, hash, tag_mask);
    let encoded = b.add(ArithMode::Wrapping, ordinal, one);
    let encoded = binary(b, BinaryOp::BitOr, encoded, tag);
    let word = b.u64_layout()?;
    let position = b.fresh(word);
    let root = b.root(index).unwrap();
    let version = b.version(root);
    let scan = b.block(BlockKind::Body, &[position, version]);
    let taken = b.block(BlockKind::Body, &[]);
    let vacant = b.block(BlockKind::Body, &[]);
    let before = b.reaching(root)?;
    b.jump(scan, &[start, before]);
    b.at(scan);
    b.set_reaching(root, version);
    let at = b.slot_place(index, position)?;
    let old = b.read_bits(at)?;
    let old_ordinal = binary(b, BinaryOp::BitAnd, old, ordinal_mask);
    let empty = b.compare(BinaryOp::Eq, old_ordinal, zero)?;
    b.branch(empty, vacant, taken);
    b.at(taken);
    let next = b.add(ArithMode::Wrapping, position, one);
    let next = binary(b, BinaryOp::BitAnd, next, mask);
    b.jump(scan, &[next, version]);
    b.at(vacant);
    overwrite(b, at, encoded)
}

/// Cache a cold image's seed on its first successful removal. Missing removals
/// never detach or charge a borrowed map. Ordinary removals reuse the probe's
/// bucket; only the newly created index needs a word-only bucket search.
fn warm_for_removal(b: &mut Body<'_, '_>, probe: &Probe, n: ValueId) -> R<ValueId> {
    let root = b.parameters[0].slot;
    let container = b.parameters[0].address;
    let hashes = field(b, container, 2)?;
    let cached = len(b, hashes)?;
    let cold = b.compare(BinaryOp::Ne, cached, n)?;
    let index = field(b, container, 1)?;
    let capacity = len(b, index)?;
    let zero = b.u64(0)?;
    let eight = b.u64(8)?;
    let empty = b.compare(BinaryOp::Eq, capacity, zero)?;
    let large = b.compare(BinaryOp::Gt, n, eight)?;
    let missing_index = binary(b, BinaryOp::BitAnd, empty, large);
    let cold = binary(b, BinaryOp::BitOr, cold, missing_index);
    let warm = b.block(BlockKind::Body, &[]);
    let word = b.u64_layout()?;
    let bucket = b.fresh(word);
    let version = b.version(root);
    let done = b.block(BlockKind::Body, &[bucket, version]);
    let before = b.reaching(root)?;
    let yes = b.edge(warm, &[]);
    let no = b.edge(done, &[probe.bucket, before]);
    b.terminate(Terminator::Branch {
        condition: cold,
        yes,
        no,
    });
    b.at(warm);
    maintenance(b, "map_warm")?;
    let index = field(b, container, 1)?;
    let capacity = len(b, index)?;
    let zero = b.u64(0)?;
    let one = b.u64(1)?;
    let indexed = b.compare(BinaryOp::Ne, capacity, zero)?;
    let search = b.block(BlockKind::Body, &[]);
    let after = b.reaching(root)?;
    let yes = b.edge(search, &[]);
    let no = b.edge(done, &[zero, after]);
    b.terminate(Terminator::Branch {
        condition: indexed,
        yes,
        no,
    });
    b.at(search);
    let mask = sub(b, capacity, one);
    let encoded = b.add(ArithMode::Wrapping, probe.ordinal, one);
    let ordinal_mask = b.u64(u64::from(u32::MAX))?;
    let first = binary(b, BinaryOp::BitAnd, probe.hash, mask);
    let position = b.fresh(word);
    let scan = b.block(BlockKind::Body, &[position]);
    let next = b.block(BlockKind::Body, &[]);
    b.jump(scan, &[first]);
    b.at(scan);
    let at = b.slot_place(index, position)?;
    let value = b.read_bits(at)?;
    let ordinal = binary(b, BinaryOp::BitAnd, value, ordinal_mask);
    let matched = b.compare(BinaryOp::Eq, ordinal, encoded)?;
    let yes = b.edge(done, &[position, after]);
    let no = b.edge(next, &[]);
    b.terminate(Terminator::Branch {
        condition: matched,
        yes,
        no,
    });
    b.at(next);
    let increment = b.add(ArithMode::Wrapping, position, one);
    let increment = binary(b, BinaryOp::BitAnd, increment, mask);
    b.jump(scan, &[increment]);
    b.at(done);
    b.set_reaching(root, version);
    Ok(bucket)
}
