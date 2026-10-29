use super::*;
use hirsel_product_codec::{TirBodyView, TirView};

/// Check arena references, provenance and scopes directly on live or checked
/// archived rows. A missing source/type binding fails the read with `None`;
/// malformed TIR obligations are returned in the ordinary diagnostic order.
/// This derives no signature, type, reachability or ownership verdict.
pub fn validate_structure_rows(
    definition: TirView<'_>,
    t: &TypeInterner,
) -> Option<Vec<Violation>> {
    let header = definition.header()?;
    // TypeIds carry no provenance: a row belongs to this definition exactly
    // when its closure table lists it.
    let mut members = crate::hashed::HashSet::default();
    for n in 0..definition.type_count() {
        let id = *definition.type_id(n)?;
        if t.contains(id) {
            members.insert(id);
        }
    }
    let owned_type = |id: TypeId| members.contains(&id);
    let mut errors = Vec::new();
    let mut add = |body: Option<BodyId>, block: Option<BlockId>, instruction: Option<u32>, kind| {
        let v = Violation {
            body,
            block,
            instruction,
            kind,
        };
        if !errors.contains(&v) {
            errors.push(v);
        }
    };
    if header.entry.0 as usize >= definition.body_count() {
        add(
            None,
            None,
            None,
            ViolationKind::InvalidId {
                arena: "body",
                index: header.entry.0,
            },
        );
    }
    // Caller body references are checked even at the structural-only interpreter boundary.
    for n in 0..definition.caller_body_count() {
        let body = definition.caller_body(n)?;
        if body.0 as usize >= definition.body_count() {
            add(
                None,
                None,
                None,
                ViolationKind::InvalidId {
                    arena: "body",
                    index: body.0,
                },
            );
        }
    }
    let mut ordinals = BTreeSet::new();
    for n in 0..definition.contract_count() {
        let contract = definition.contract(n)?;
        let (ordinal, body) = &*contract;
        if !ordinals.insert(*ordinal) {
            add(
                None,
                None,
                None,
                ViolationKind::Duplicate {
                    entity: "contract",
                    index: *ordinal,
                },
            );
        }
        if body.0 as usize >= definition.body_count() {
            add(
                None,
                None,
                None,
                ViolationKind::InvalidId {
                    arena: "body",
                    index: body.0,
                },
            );
        }
    }
    for bi in 0..definition.body_count() {
        let body = definition.body(bi)?;
        let body_header = body.header()?;
        let bid = Some(BodyId(bi as u32));
        let mut refs = Vec::new();
        refs.push((None, None, "block", body_header.entry.0));
        let mut cursor_carriers = BTreeSet::new();
        for n in 0..body.borrow_cursor_count() {
            let cursor = body.borrow_cursor(n)?;
            refs.push((None, None, "local", cursor.carrier.0));
            refs.push((None, None, "rvalue", cursor.initializer.0));
            for place in cursor.cover.iter().flatten() {
                refs.push((None, None, "place", place.0));
            }
            let mut declarations = 0;
            for n in 0..body.block_count() {
                let block = body.block(n)?;
                for n in 0..block.instruction_count() {
                    let instruction = block.instruction(n)?;
                    if let InstructionKind::Assign { destination, value } = instruction.kind
                        && value == cursor.initializer
                        && (destination.0 as usize) < body.place_count()
                    {
                        let place = body.place(destination.0 as usize)?;
                        if place.root == cursor.carrier && place.projection.is_empty() {
                            declarations += 1;
                        }
                    }
                }
            }
            if !cursor_carriers.insert(cursor.carrier.0) || declarations != 1 {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "borrow cursor declaration",
                        index: cursor.carrier.0,
                    },
                );
            }
        }
        let mut type_roots = vec![body_header.result];
        let mut parameters = BTreeSet::new();
        for n in 0..body.parameter_count() {
            let p = body.parameter(n)?;
            refs.push((None, None, "local", p.0));
            if !parameters.insert(p.0) {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::Duplicate {
                        entity: "parameter",
                        index: p.0,
                    },
                );
            }
            if (p.0 as usize) < body.local_count() {
                let l = body.local(p.0 as usize)?;
                if !matches!(l.storage, Storage::Parameter | Storage::Capture) {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::WrongKind {
                            entity: "parameter",
                            index: p.0,
                        },
                    );
                }
            }
        }
        for i in 0..body.local_count() {
            let l = body.local(i)?;
            type_roots.push(l.ty);
            refs.push((None, None, "scope", l.scope.0));
            if l.storage == Storage::Parameter && !parameters.contains(&(i as u32)) {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "unbound parameter",
                        index: i as u32,
                    },
                );
            }
            // Only a closure body has an environment; flow starts captures
            // live, so an entry capture would read storage nobody supplied.
            if l.storage == Storage::Capture && bi == header.entry.0 as usize {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "entry capture",
                        index: i as u32,
                    },
                );
            }
        }
        for n in 0..body.place_count() {
            let p = body.place(n)?;
            type_roots.push(p.ty);
            refs.push((None, None, "local", p.root.0));
            for projection in &p.projection {
                if let Projection::Index(l) = projection {
                    refs.push((None, None, "local", l.0));
                }
            }
        }
        for i in 0..body.scope_count() {
            let s = body.scope(i)?;
            if let Some(p) = s.parent {
                refs.push((None, None, "scope", p.0));
            }
            let mut seen = BTreeSet::new();
            let mut next = Some(ScopeId(i as u32));
            while let Some(id) = next {
                if !seen.insert(id.0) {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::ScopeCycle(ScopeId(i as u32)),
                    );
                    break;
                }
                next = if (id.0 as usize) < body.scope_count() {
                    body.scope(id.0 as usize)?.parent
                } else {
                    None
                };
            }
        }
        let mut loans = BTreeSet::new();
        let mut loan_refs = Vec::new();
        for n in 0..body.place_count() {
            if let Some(id) = body.place(n)?.loan {
                loan_refs.push((None, None, id));
            }
        }
        for b in 0..body.block_count() {
            let block = body.block(b)?;
            let terminator = block.terminator()?;
            let block_id = Some(BlockId(b as u32));
            for i in 0..block.instruction_count() {
                let inst = block.instruction(i)?;
                let ins = Some(i as u32);
                refs.push((block_id, ins, "cleanup", inst.on_failure.0));
                match &inst.kind {
                    // CT-4 placement: a call charge ending its block precedes a
                    // source call (never a compiler contract call), and an
                    // iteration charge opens a loop body block. One event may
                    // have several sites (an or-pattern arm lowers per alternative).
                    InstructionKind::Charge(event) => {
                        let misplaced = match event.kind {
                            hirsel_tir::SourceEventKind::Call => {
                                i + 1 == block.instruction_count()
                                    && !matches!(
                                        *terminator,
                                        Terminator::Call {
                                            callee: Callee::Definition { .. }
                                                | Callee::Indirect {
                                                    forward_caller: false,
                                                    ..
                                                },
                                            ..
                                        }
                                    )
                            }
                            hirsel_tir::SourceEventKind::Iteration => {
                                i != 0 || b == body_header.entry.0 as usize
                            }
                        };
                        if misplaced {
                            add(
                                bid,
                                block_id,
                                ins,
                                ViolationKind::WrongKind {
                                    entity: "charge placement",
                                    index: event.source,
                                },
                            );
                        }
                    }
                    InstructionKind::Defuse { place } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::BindOrigin { source, place, .. } => {
                        refs.push((block_id, ins, "place", place.0));
                        refs.push((block_id, ins, "place", source.0));
                    }
                    InstructionKind::BindFields { source, fields } => {
                        refs.push((block_id, ins, "place", source.0));
                        for field in fields {
                            refs.push((block_id, ins, "place", field.0));
                        }
                    }
                    InstructionKind::EndOrigin(place) => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::StorageLive(l) => refs.push((block_id, ins, "local", l.0)),
                    InstructionKind::EndScope(scope) => {
                        refs.push((block_id, ins, "scope", scope.0))
                    }
                    InstructionKind::StorageDead { place, .. } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "place", value.place.0));
                        refs.push((block_id, ins, "place", displaced.0));
                    }
                    InstructionKind::Assign { destination, value } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "rvalue", value.0));
                    }
                    InstructionKind::BeginLoan { id, place, .. } => {
                        if !loans.insert(id.0) {
                            add(
                                bid,
                                block_id,
                                ins,
                                ViolationKind::Duplicate {
                                    entity: "loan",
                                    index: id.0,
                                },
                            );
                        }
                        refs.push((block_id, ins, "place", place.0));
                    }
                    InstructionKind::EndLoan { loan: id, .. } => {
                        loan_refs.push((block_id, ins, *id))
                    }
                    InstructionKind::Drop { place, .. } => {
                        refs.push((block_id, ins, "place", place.0))
                    }
                    InstructionKind::RunCleanup(id) => refs.push((block_id, ins, "cleanup", id.0)),
                    InstructionKind::Check { condition, .. } => {
                        refs.push((block_id, ins, "place", condition.place.0))
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        refs.push((block_id, ins, "place", destination.0));
                        refs.push((block_id, ins, "place", value.place.0));
                    }
                }
            }
            for target in successors(body, &terminator)? {
                refs.push((block_id, None, "block", target.0));
            }
            let mut operand = |o: &Operand| refs.push((block_id, None, "place", o.place.0));
            match &*terminator {
                Terminator::Match { table, from } => {
                    if (table.0 as usize) < body.match_table_count() {
                        let t = body.match_table(table.0 as usize)?;
                        if *from as usize > t.arm_count() {
                            add(bid, block_id, None, ViolationKind::MatchCoverage);
                        }
                    } else {
                        add(
                            bid,
                            block_id,
                            None,
                            ViolationKind::InvalidId {
                                arena: "match table",
                                index: table.0,
                            },
                        );
                    }
                }
                Terminator::Branch { condition, .. } => operand(condition),
                Terminator::Switch { value, arms, .. } => {
                    operand(value);
                    let mut tags = BTreeSet::new();
                    for (tag, _) in arms {
                        if !tags.insert(*tag) {
                            add(
                                bid,
                                block_id,
                                None,
                                ViolationKind::Duplicate {
                                    entity: "switch arm",
                                    index: *tag,
                                },
                            );
                        }
                    }
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    on_failure,
                } => {
                    if let Callee::Indirect { value: o, .. } = callee {
                        operand(o);
                    }
                    let (Callee::Definition {
                        arguments: generic_arguments,
                        ..
                    }
                    | Callee::Indirect {
                        arguments: generic_arguments,
                        ..
                    }) = callee;
                    type_roots.extend(generic_arguments.iter().filter_map(|a| {
                        if let GenericArgument::Type(t) = a {
                            Some(*t)
                        } else {
                            None
                        }
                    }));
                    for a in arguments {
                        operand(&a.value);
                    }
                    if let Some(r) = returns {
                        refs.push((block_id, None, "place", r.destination.0));
                    }
                    refs.push((block_id, None, "cleanup", on_failure.0));
                }
                Terminator::Return { value, cleanup } => {
                    operand(value);
                    refs.push((block_id, None, "cleanup", cleanup.0));
                }
                Terminator::Fail { cleanup, .. } => {
                    refs.push((block_id, None, "cleanup", cleanup.0))
                }
                _ => (),
            }
        }
        for table_index in 0..body.match_table_count() {
            let table = body.match_table(table_index)?;
            let mut has_entry = false;
            for n in 0..body.block_count() {
                if matches!(*body.block(n)?.terminator()?, Terminator::Match { table, from: 0 } if table.0 as usize == table_index)
                {
                    has_entry = true;
                    break;
                }
            }
            if !has_entry {
                add(
                    bid,
                    None,
                    None,
                    ViolationKind::WrongKind {
                        entity: "match table without entry",
                        index: table_index as u32,
                    },
                );
            }
            refs.push((None, None, "place", table.scrutinee()?.0));
            let mut used = vec![false; table.pattern_count()];
            for n in 0..table.arm_count() {
                let arm = table.arm(n)?;
                refs.push((None, None, "block", arm.target.0));
                if let Some(guard) = arm.guard {
                    refs.push((None, None, "block", guard.0));
                }
                if let Some(u) = used.get_mut(arm.pattern.0 as usize) {
                    *u = true;
                } else {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "match pattern",
                            index: arm.pattern.0,
                        },
                    );
                }
            }
            for index in (0..table.pattern_count()).rev() {
                let pattern = table.pattern(index)?;
                refs.push((None, None, "place", pattern.place.0));
                let mut fields = BTreeSet::new();
                for (field, child) in pattern.kind.fields() {
                    if !fields.insert(*field) {
                        add(
                            bid,
                            None,
                            None,
                            ViolationKind::Duplicate {
                                entity: "pattern field",
                                index: *field,
                            },
                        );
                    }
                    if child.0 as usize >= index {
                        add(
                            bid,
                            None,
                            None,
                            ViolationKind::InvalidId {
                                arena: "pattern child",
                                index: child.0,
                            },
                        );
                    } else if used[index] {
                        used[child.0 as usize] = true;
                    }
                }
            }
            for (index, used) in used.into_iter().enumerate() {
                if !used {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::WrongKind {
                            entity: "unused match pattern",
                            index: index as u32,
                        },
                    );
                }
            }
        }
        for n in 0..body.rvalue_count() {
            let rvalue = body.rvalue(n)?;
            for o in typing::operands(&rvalue) {
                refs.push((None, None, "place", o.place.0));
            }
            match &*rvalue {
                Rvalue::Closure { body, .. } => refs.push((None, None, "body", body.0)),
                Rvalue::Shape(t) => type_roots.push(*t),
                _ => (),
            }
        }
        for n in 0..body.cleanup_count() {
            let cleanup = body.cleanup(n)?;
            if let Some(head) = cleanup.head {
                if head.0 as usize >= body.cleanup_node_count() {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "cleanup node",
                            index: head.0,
                        },
                    );
                }
            }
        }
        for index in 0..body.cleanup_node_count() {
            let node = body.cleanup_node(index)?;
            if let Some(parent) = node.parent {
                if parent.0 as usize >= index {
                    add(
                        bid,
                        None,
                        None,
                        ViolationKind::InvalidId {
                            arena: "cleanup parent",
                            index: parent.0,
                        },
                    );
                }
            }
        }
        for n in 0..body.cleanup_node_count() {
            let c = body.cleanup_node(n)?;
            for action in [&c.action] {
                match action {
                    CleanupAction::Drop { place, .. }
                    | CleanupAction::CloseArena(place)
                    | CleanupAction::JoinTasks { scope: place, .. } => {
                        refs.push((None, None, "place", place.0))
                    }
                    CleanupAction::EndLoan { loan: id, .. } => loan_refs.push((None, None, *id)),
                    CleanupAction::ReleaseRegion(_) => (),
                }
            }
        }
        for (block, inst, id) in loan_refs {
            if !loans.contains(&id.0) {
                add(
                    bid,
                    block,
                    inst,
                    ViolationKind::InvalidId {
                        arena: "loan",
                        index: id.0,
                    },
                );
            }
        }
        for (block, inst, arena, index) in refs {
            let len = match arena {
                "local" => body.local_count(),
                "place" => body.place_count(),
                "scope" => body.scope_count(),
                "block" => body.block_count(),
                "rvalue" => body.rvalue_count(),
                "cleanup" => body.cleanup_count(),
                "body" => definition.body_count(),
                _ => unreachable!(),
            };
            if index as usize >= len {
                add(bid, block, inst, ViolationKind::InvalidId { arena, index });
            }
        }
        for ty in type_roots {
            if !owned_type(ty) {
                add(bid, None, None, ViolationKind::InvalidType(ty));
            }
        }
    }
    // Check the table's graph edges once; each body's root IDs were checked above.
    for n in 0..definition.type_count() {
        let id = *definition.type_id(n)?;
        if !owned_type(id) {
            add(None, None, None, ViolationKind::InvalidType(id));
            continue;
        }
        for edge in t.row(id).kind.graph_edges() {
            if !owned_type(edge.ty) {
                add(None, None, None, ViolationKind::InvalidType(edge.ty));
            }
        }
    }

    Some(errors)
}

fn successors(body: TirBodyView<'_>, terminator: &Terminator) -> Option<Vec<BlockId>> {
    Some(match terminator {
        Terminator::Goto(block) => vec![*block],
        Terminator::Match { table, from } => {
            let mut blocks = Vec::new();
            if (table.0 as usize) < body.match_table_count() {
                let table = body.match_table(table.0 as usize)?;
                for n in *from as usize..table.arm_count() {
                    let arm = table.arm(n)?;
                    blocks.push(arm.guard.unwrap_or(arm.target));
                }
            }
            blocks
        }
        Terminator::Branch { yes, no, .. } => vec![*yes, *no],
        Terminator::Switch {
            arms, otherwise, ..
        } => arms.iter().map(|a| a.1).chain([*otherwise]).collect(),
        Terminator::Call { returns, .. } => returns.iter().map(|r| r.next).collect(),
        Terminator::Return { .. } | Terminator::Fail { .. } | Terminator::Unreachable => vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::support::{self, Fixture};
    use hirsel_artifact_store::{Contents, Memory};
    use hirsel_product_codec::{CheckedTir, TypeRecords};
    use std::sync::Arc;

    fn archive(definition: &Definition, store: Arc<Memory>) -> CheckedTir {
        let records = TypeRecords::new(support::session(), store.clone());
        let bytes = records
            .encode_tir(definition, |_| Some((support::path("main"), 0)))
            .unwrap();
        let reference = store.put_content(CheckedTir::DOMAIN, &bytes).unwrap();
        CheckedTir::open(
            store.get_content(reference).unwrap(),
            reference,
            records,
            |_, _| Some(hirsel_span::Anchor::default()),
        )
        .unwrap()
    }

    #[test]
    fn indexed_structure_preserves_live_violations_and_order() {
        let clean = Fixture::unit().definition;
        let mut cases = vec![
            clean.clone(),
            Fixture::loan().definition,
            Fixture::call().definition,
        ];
        for mutate in [
            |d: &mut Definition| d.entry = BodyId(9),
            |d: &mut Definition| d.caller_bodies = vec![BodyId(9)],
            |d: &mut Definition| d.contracts = vec![(3, BodyId(9)), (3, BodyId(0))],
            |d: &mut Definition| d.bodies[0].entry = BlockId(9),
            |d: &mut Definition| d.bodies[0].parameters = vec![LocalId(0), LocalId(0), LocalId(9)],
            |d: &mut Definition| d.bodies[0].locals[0].storage = Storage::Capture,
            |d: &mut Definition| d.bodies[0].scopes[0].parent = Some(ScopeId(0)),
            |d: &mut Definition| d.bodies[0].places[0].root = LocalId(9),
            |d: &mut Definition| d.bodies[0].blocks[0].instructions[0].on_failure = CleanupId(9),
            |d: &mut Definition| {
                d.bodies[0].blocks[0].terminator = Terminator::Match {
                    table: MatchTableId(9),
                    from: 0,
                }
            },
            |d: &mut Definition| d.bodies[0].cleanups[0].head = Some(CleanupNodeId(9)),
            |d: &mut Definition| {
                d.bodies[0].borrow_cursors.push(BorrowCursor {
                    carrier: LocalId(9),
                    initializer: RvalueId(9),
                    cover: Some(vec![PlaceId(9)]),
                })
            },
        ] {
            let mut malformed = clean.clone();
            mutate(&mut malformed);
            assert!(!validate_structure(&malformed, support::interner()).is_empty());
            cases.push(malformed);
        }
        for definition in cases {
            let expected = validate_structure(&definition, support::interner());
            let archive = archive(&definition, Arc::new(Memory::default()));
            assert_eq!(
                validate_structure_rows(TirView::Archived(&archive), support::interner()),
                Some(expected)
            );
        }
    }

    #[test]
    fn indexed_structure_refuses_a_missing_type_child() {
        let definition = Fixture::unit().definition;
        let store = Arc::new(Memory::default());
        let checked = archive(&definition, store.clone());
        let missing = TypeRecords::new(support::session(), Arc::new(Memory::default()));
        let unbound = CheckedTir::open(
            store.get_content(checked.reference()).unwrap(),
            checked.reference(),
            missing,
            |_, _| Some(hirsel_span::Anchor::default()),
        )
        .unwrap();
        assert!(
            validate_structure_rows(TirView::Archived(&unbound), support::interner()).is_none()
        );
        assert_eq!(
            validate_structure_rows(TirView::Archived(&checked), support::interner()),
            Some(vec![])
        );
    }
}
