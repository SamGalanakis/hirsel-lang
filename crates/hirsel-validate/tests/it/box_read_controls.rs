//! Hand-built #98 controls; no source checker or lowering is involved.
use super::*;

fn boxed_bool(nested: bool) -> Fixture {
    let mut f = Fixture::unit();
    let boolean = f
        .definition
        .types
        .push(ty(TypeKind::Primitive(Primitive::Bool)));
    let boxed = f.definition.types.push(ty(TypeKind::Box(boolean)));
    let payload = if nested { boxed } else { boolean };
    let p = f.slot(TypeKind::Box(payload), Storage::Parameter);
    f.body().parameters.push(LocalId(p));
    let projection = if nested {
        vec![Projection::Field(0), Projection::Field(0)]
    } else {
        vec![Projection::Field(0)]
    };
    let viewed = PlaceId(f.body().places.len() as u32);
    f.body().places.push(Place {
        root: LocalId(p),
        projection: projection.into(),
        ty: boolean,
        loan: None,
        binding: None,
    });
    f.body().matches.push(MatchTable {
        scrutinee: PlaceId(p),
        patterns: vec![
            MatchPattern {
                place: viewed,
                kind: MatchPatternKind::Literal(Constant::Bool(false)),
            },
            MatchPattern {
                place: viewed,
                kind: MatchPatternKind::Literal(Constant::Bool(true)),
            },
        ],
        arms: vec![
            MatchArm {
                pattern: MatchPatternId(0),
                guard: None,
                target: BlockId(1),
            },
            MatchArm {
                pattern: MatchPatternId(1),
                guard: None,
                target: BlockId(1),
            },
        ],
    });
    f.body().blocks[0].terminator = Terminator::Match {
        table: MatchTableId(0),
        from: 0,
    };
    f.body().blocks.push(block(vec![], ret(0, 0)));
    owner_signature(&mut f, PassingMode::Borrow);
    f
}

#[test]
fn box_payload_match_is_independently_typed_and_exhaustive() {
    for nested in [false, true] {
        let mut f = boxed_bool(nested);
        f.valid();
        f.body().matches[0].arms.pop();
        f.body().matches[0].patterns.pop();
        f.broken(vec![ViolationKind::MatchCoverage]);
    }
}

#[test]
fn box_pattern_cannot_observe_an_unrelated_same_typed_place() {
    let mut f = boxed_bool(false);
    let other = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Parameter);
    let other_root = f.body().places[other as usize].root;
    f.body().parameters.push(other_root);
    owner_signature(&mut f, PassingMode::Borrow);
    f.valid();
    for pattern in &mut f.body().matches[0].patterns {
        pattern.place = PlaceId(other);
    }
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(PlaceId(other)))
    );
}

#[test]
fn nested_structural_pattern_projects_only_box_payloads() {
    let mut f = boxed_bool(false);
    let p = f.body().matches[0].scrutinee;
    let boxed = f.body().places[p.0 as usize].ty;
    let tuple = f.slot(TypeKind::Tuple(vec![boxed].into()), Storage::Parameter);
    let tuple_root = f.body().places[tuple as usize].root;
    f.body().parameters = vec![tuple_root];
    f.body().places[p.0 as usize].root = tuple_root;
    f.body().places[p.0 as usize].projection = vec![Projection::Field(0)].into();
    let viewed = f.body().matches[0].patterns[0].place;
    f.body().places[viewed.0 as usize].root = tuple_root;
    f.body().places[viewed.0 as usize].projection =
        vec![Projection::Field(0), Projection::Field(0)].into();
    f.body().matches[0].scrutinee = PlaceId(tuple);
    for i in 0..2 {
        let pattern = MatchPatternId(f.body().matches[0].patterns.len() as u32);
        f.body().matches[0].patterns.push(MatchPattern {
            place: PlaceId(tuple),
            kind: MatchPatternKind::Tuple(vec![(0, MatchPatternId(i))]),
        });
        f.body().matches[0].arms[i as usize].pattern = pattern;
    }
    // The old parameter slot is now unused local storage, with no owned content.
    f.body().locals[p.0 as usize].storage = Storage::Temporary;
    owner_signature(&mut f, PassingMode::Borrow);
    f.valid();
    f.body().places[viewed.0 as usize].projection = vec![Projection::Field(0)].into();
    assert!(
        f.kinds()
            .contains(&ViolationKind::InvalidProjection(viewed))
    );
}
