//! The pre-change clone-based projection is intentionally independent of project.
//! Rust reference pin 1a2c4797: projection.rs:248-281, visitor.rs:13-78;
//! Hirsel's bounds, missing fields and holder identities remain authoritative.
use super::*;

fn old(root: &Shadow, steps: &[(usize, bool)]) -> Shadow {
    let mut node: Option<Shadow> = None;
    for &(index, indexed) in steps {
        let current = node.as_ref().unwrap_or(root);
        let mut part = old_part(current, index);
        if indexed {
            part.add_bound(&current.total());
        }
        node = Some(part);
    }
    node.unwrap_or_else(|| root.clone())
}
fn old_part(shadow: &Shadow, index: usize) -> Shadow {
    match shadow {
        Shadow::Uniform(_) => {
            record_clone(shadow);
            Shadow::Uniform(shadow.total())
        }
        Shadow::Ownership { content, .. } => {
            let mut part = old_part(content, index);
            part.add_bound(&shadow.total());
            part
        }
        Shadow::View { content, home } => {
            CLONED_NODES.with(|count| count.set(count.get() + 1));
            Shadow::View {
                content: Box::new(old_part(content, index)),
                home: home.clone(),
            }
        }
        Shadow::Parts(parts) => {
            let empty = Shadow::default();
            let part = parts.get(index).unwrap_or(&empty);
            record_clone(part);
            part.clone()
        }
    }
}
fn identical(actual: &Shadow, expected: &Shadow) {
    assert_eq!(actual, expected);
    match (actual, expected) {
        (
            Shadow::Ownership {
                content: a,
                holder: ah,
            },
            Shadow::Ownership {
                content: b,
                holder: bh,
            },
        ) => {
            assert!(Rc::ptr_eq(&ah.block, &bh.block));
            assert!(Rc::ptr_eq(&ah.token, &bh.token));
            identical(a, b);
        }
        (Shadow::View { content: a, .. }, Shadow::View { content: b, .. }) => identical(a, b),
        (Shadow::Parts(a), Shadow::Parts(b)) => {
            for (a, b) in a.iter().zip(b) {
                identical(a, b);
            }
        }
        _ => (),
    }
}
fn own(content: Shadow, holder: &Holder) -> Shadow {
    Shadow::Ownership {
        content: Box::new(content),
        holder: holder.clone(),
    }
}
fn view(content: Shadow, home: u32) -> Shadow {
    Shadow::View {
        content: Box::new(content),
        home: vec![home],
    }
}
fn trees() -> Vec<Shadow> {
    let h = Holder::new(true);
    let mut trees = vec![
        Shadow::default(),
        Shadow::of(vec![1, 4]),
        Shadow::Parts(vec![]),
    ];
    for _ in 0..3 {
        let prior = trees.clone();
        for (i, t) in prior.iter().enumerate() {
            trees.push(view(t.clone(), 7));
            trees.push(own(t.clone(), &h));
            trees.push(Shadow::Parts(vec![
                t.clone(),
                own(prior[(i + 1) % prior.len()].clone(), &h),
                view(Shadow::Parts(vec![]), 9),
            ]));
        }
    }
    trees
}
#[test]
fn nested_trees_and_mixed_paths_match_without_normalizing_holders() {
    let choices = [
        (0, false),
        (1, false),
        (2, false),
        (99, false),
        (0, true),
        (1, true),
        (99, true),
    ];
    let mut paths = vec![vec![]];
    for depth in 0..4 {
        let previous = paths.clone();
        for path in previous.iter().filter(|p| p.len() == depth) {
            for step in choices {
                let mut p = path.clone();
                p.push(step);
                paths.push(p);
            }
        }
    }
    let mut cases = 0;
    for tree in trees() {
        for path in &paths {
            identical(&tree.project(path.iter().copied()), &old(&tree, path));
            cases += 1;
        }
    }
    println!("differential cases {cases}");
    assert_eq!(cases, 537_792);
}
#[test]
fn empty_nodes_discard_deferred_leaf_bounds_but_keep_view_homes() {
    let h = Holder::new(true);
    let tree = own(
        Shadow::Parts(vec![view(Shadow::Parts(vec![]), 7), Shadow::of(vec![3])]),
        &h,
    );
    for path in [
        vec![(0, false), (1, false)],
        vec![(0, true), (1, true), (5, false)],
        vec![(1, true), (8, false)],
    ] {
        identical(&tree.project(path.iter().copied()), &old(&tree, &path));
    }
}
#[test]
fn terminal_holders_keep_identity_and_live_counts() {
    let h = Holder::new(true);
    let other = h.copy();
    let tree = Shadow::Parts(vec![
        own(Shadow::of(vec![1]), &h),
        own(Shadow::default(), &h),
    ]);
    assert!(h.shared_by_another());
    let projected = tree.project([(0, false)]);
    identical(&projected, &old(&tree, &[(0, false)]));
    drop(other);
    assert!(!h.shared_by_another());
    drop(tree);
    assert!(!h.shared_by_another());
    assert_eq!(
        h.block
            .holders
            .borrow()
            .iter()
            .filter(|w| w.strong_count() > 0)
            .count(),
        1
    );
    identical(&projected, &own(Shadow::of(vec![1]), &h));
}
#[test]
fn displaced_mutable_roots_keep_arena_region_first_error_and_site() {
    let shared = state(CheckedRuntime::default());
    let mut s = shared.borrow_mut();
    let binding = s.root(&Shadow::default());
    let region = s.instance(Kind::Region);
    let arena = s.arena();
    let outer = s.instance(Kind::MutableRoot(vec![binding, region, arena]));
    let inner = s.instance(Kind::MutableRoot(vec![outer]));
    s.site = Span {
        anchor: hirsel_span::Anchor::fresh(),
        start: 17,
        end: 23,
    };
    let tree = view(
        Shadow::Parts(vec![Shadow::of(vec![inner]), Shadow::of(vec![region])]),
        binding,
    );
    let path = [(0, true), (0, false)];
    let mut before = old(&tree, &path);
    let mut after = tree.project(path);
    s.caller_content(&mut before);
    s.caller_content(&mut after);
    identical(&after, &before);
    assert_eq!(after.total(), vec![binding, region, arena]);
    s.close(inner);
    s.close(outer);
    assert!(s.access(&after, "displaced").is_ok());
    for instance in [arena, region, binding] {
        s.close(instance);
        let Err(Error::Trapped(actual)) = s.access(&after, "displaced") else {
            panic!("ended projection must trap");
        };
        let Err(Error::Trapped(expected)) = s.access(&before, "displaced") else {
            panic!("oracle must trap");
        };
        assert_eq!(actual, expected);
        assert_eq!(actual.site, s.site);
        assert_eq!(
            actual.rule,
            if instance == arena {
                TrapRule::ArenaUseAfterEnd
            } else {
                TrapRule::RegionUseAfterScopeExit
            }
        );
    }
}

/// Logical clone work, separate from production instruction measurements.
fn nodes(shadow: &Shadow) -> usize {
    match shadow {
        Shadow::Uniform(_) => 1,
        Shadow::Parts(parts) => 1 + parts.iter().map(nodes).sum::<usize>(),
        Shadow::Ownership { content, .. } | Shadow::View { content, .. } => 1 + nodes(content),
    }
}
#[test]
fn bounded_deep_wide_projection_materializes_only_the_terminal_tree() {
    let mut tree = Shadow::of(vec![1]);
    for _ in 0..24 {
        tree = Shadow::Parts(vec![tree, Shadow::Parts(vec![Shadow::of(vec![2]); 64])]);
    }
    let steps = vec![(0, false); 24];
    CLONED_NODES.with(|count| count.set(0));
    let before_tree = old(&tree, &steps);
    let before = CLONED_NODES.with(Cell::get);
    CLONED_NODES.with(|count| count.set(0));
    let after = tree.project(steps);
    let after_count = CLONED_NODES.with(Cell::get);
    identical(&after, &before_tree);
    assert_eq!(after_count, 1);
    assert!(before > 10_000);
    println!("cloned nodes before={before} after={after_count}");
}

fn frame(tree: Shadow, projection: Vec<hirsel_tir::Projection>) -> crate::frame::Frame {
    use hirsel_tir::*;
    let types = hirsel_types::TypeInterner::new(None);
    let unit = types.intern(hirsel_types::Type::with_kind(
        hirsel_types::TypeKind::Tuple(vec![].into()),
    ));
    let path = DefPath::core("checked_shadow_cost_7");
    let body = Body {
        parameter_scope: hirsel_syntax::LexicalOwner::first(&path)
            .site(hirsel_types::NameSite::Item),
        arena_scope_grant: None,
        scoped_parameters: Vec::new(),
        borrow_cursors: Vec::new(),
        locals: (0..3)
            .map(|_| Local {
                ty: unit,
                storage: Storage::Parameter,
                scope: ScopeId(0),
                span: Span::default(),
            })
            .collect(),
        places: vec![Place {
            loan: None,
            binding: None,
            root: LocalId(0),
            projection: projection.into(),
            ty: unit,
        }],
        scopes: vec![Scope {
            parent: None,
            span: Span::default(),
        }],
        blocks: vec![],
        rvalues: vec![],
        cleanups: vec![],
        cleanup_nodes: vec![],
        entry: BlockId(0),
        parameters: vec![LocalId(0)],
        result: unit,
        tail_sites: vec![],
        matches: vec![],
    };
    let definition = Definition {
        caller_bodies: vec![],
        protocol_associations: vec![],
        types: vec![unit].into(),
        path: (&path).into(),
        inferred_unique: Default::default(),
        signature_hash: Default::default(),
        bodies: vec![body],
        entry: BodyId(0),
        contracts: vec![],
    };
    let mut frame =
        crate::frame::Frame::new(types, std::sync::Arc::new(definition), vec![Value::Unit])
            .unwrap();
    frame.check = Some(FrameCheck::enter(
        state(CheckedRuntime::default()),
        frame.body(),
        &frame.types,
        &frame.slots,
        vec![Argument::default()],
        None,
        false,
    ));
    frame.check.as_mut().unwrap().shadows[0] = tree;
    frame
}
#[test]
fn frame_indices_use_evaluated_slots_and_failed_later_indices_leave_holders_intact() {
    use hirsel_tir::{PlaceId, Projection};
    let h = Holder::new(true);
    let tree = Shadow::Parts(vec![
        Shadow::Parts(vec![own(Shadow::of(vec![1]), &h), Shadow::of(vec![2])]),
        Shadow::Parts(vec![own(Shadow::of(vec![3]), &h), Shadow::of(vec![4])]),
    ]);
    let mut f = frame(
        tree.clone(),
        vec![Projection::Index(LocalId(1)), Projection::Index(LocalId(2))],
    );
    let integer = |n| Value::Integer(crate::Integer::new(hirsel_types::Primitive::U64, n).unwrap());
    for first in [0, 1, 0] {
        f.slots[1] = Some(integer(first));
        f.slots[2] = Some(integer(0));
        identical(
            &f.shadow_of(PlaceId(0)).unwrap(),
            &old(&tree, &[(first as usize, true), (0, true)]),
        );
    }
    f.slots[2] = None;
    assert_error(f.shadow_of(PlaceId(0)), "invalid index operand");
    assert_error(f.displaced_shadow(PlaceId(9)), "place out of range");
    assert!(!h.shared_by_another());
    f.check = None;
    // Unchecked shadow_of bypasses validation, but displaced_shadow still reads
    // its place after shadow_of, in precisely that order.
    assert_eq!(f.shadow_of(PlaceId(9)).unwrap(), Shadow::default());
    assert_error(f.displaced_shadow(PlaceId(9)), "place out of range");
}
#[test]
fn frame_field_variant_index_and_mut_displacement_agree_with_old_projection() {
    use hirsel_tir::{PlaceId, Projection};
    let mut f = frame(
        Shadow::default(),
        vec![
            Projection::Field(0),
            Projection::VariantField {
                variant: 3,
                field: 1,
            },
            Projection::Index(LocalId(1)),
        ],
    );
    let check = f.check.as_mut().unwrap();
    let region = check.state.borrow_mut().instance(Kind::Region);
    let arena = check.state.borrow_mut().arena();
    let parameter = check.parameters[0];
    check.originals = vec![(parameter, vec![region, arena])];
    let tree = view(
        Shadow::Parts(vec![Shadow::Parts(vec![
            Shadow::default(),
            Shadow::Parts(vec![Shadow::of(vec![parameter])]),
        ])]),
        region,
    );
    check.shadows[0] = tree.clone();
    f.slots[1] = Some(Value::Integer(
        crate::Integer::new(hirsel_types::Primitive::U64, 0).unwrap(),
    ));
    f.mut_parameters = vec![LocalId(0)];
    let before = old(&tree, &[(0, false), (1, false), (0, true)]);
    identical(&f.shadow_of(PlaceId(0)).unwrap(), &before);
    let before = f.check.as_ref().unwrap().caller_content(0, before);
    identical(&f.displaced_shadow(PlaceId(0)).unwrap(), &before);
    assert_eq!(before.total(), vec![region, arena]);
    f.check.as_ref().unwrap().state.borrow_mut().close(arena);
    assert!(matches!(
        f.check
            .as_ref()
            .unwrap()
            .state
            .borrow()
            .access(&before, "arena escape"),
        Err(Error::Trapped(_))
    ));
}

fn assert_error(result: Result<Shadow, Error>, detail: &str) {
    assert_eq!(
        format!("{result:?}"),
        format!("{:?}", Err::<Shadow, _>(crate::invalid(detail)))
    );
}

thread_local! { static CLONED_NODES: Cell<usize> = const { Cell::new(0) }; }
pub(super) fn record_clone(shadow: &Shadow) {
    CLONED_NODES.with(|count| count.set(count.get() + nodes(shadow)));
}

#[test]
fn whole_local_take_keeps_holder_identity_and_checks_ended_bounds_before_extraction() {
    use hirsel_tir::{Operand, PlaceId, UseKind};
    let holder = Holder::new(true);
    let mut frame = frame(own(Shadow::default(), &holder), vec![]);
    frame.slots[0] = Some(Value::Bytes(vec![3, 7]));
    frame.check.as_mut().unwrap().copies.insert(0);
    frame.check.as_mut().unwrap().borrowed.insert(0);
    let operand = Operand {
        place: PlaceId(0),
        usage: UseKind::Take,
    };
    let (value, shadow) = frame.operand_shadow(&operand).unwrap();
    assert_eq!(value, Value::Bytes(vec![3, 7]));
    identical(&shadow, &own(Shadow::default(), &holder));
    assert!(frame.slots[0].is_none());
    let check = frame.check.as_mut().unwrap();
    assert_eq!(check.shadows[0], Shadow::default());
    assert!(!check.copies.contains(&0));
    assert!(!check.borrowed.contains(&0));
    let region = check.state.borrow_mut().instance(Kind::Region);
    check.shadows[0] = Shadow::of(vec![region]);
    check.state.borrow_mut().close(region);
    frame.slots[0] = Some(Value::Bytes(vec![9]));
    assert!(frame.operand_shadow(&operand).is_err());
    assert_eq!(frame.slots[0], Some(Value::Bytes(vec![9])));
}

#[test]
fn whole_mutable_inline_take_preserves_the_source_shadow_and_access_bookkeeping() {
    use hirsel_tir::{Operand, PlaceId, UseKind};
    let holder = Holder::new(true);
    let tree = own(Shadow::default(), &holder);
    let mut frame = frame(tree.clone(), vec![]);
    frame.mut_parameters.push(LocalId(0));
    frame.slots[0] = Some(Value::Unit);
    frame.check.as_mut().unwrap().copies.insert(0);
    frame.check.as_mut().unwrap().borrowed.insert(0);
    let (value, shadow) = frame
        .operand_shadow(&Operand {
            place: PlaceId(0),
            usage: UseKind::Take,
        })
        .unwrap();
    assert_eq!(value, Value::Unit);
    assert_eq!(shadow, Shadow::default());
    assert_eq!(frame.slots[0], Some(Value::Unit));
    let check = frame.check.as_ref().unwrap();
    identical(&check.shadows[0], &tree);
    assert!(check.copies.contains(&0));
    assert!(check.borrowed.contains(&0));
}
