//! Direct V tables manufactured without source checking or lowering.
mod model;
use crate::support::*;
use hirsel_syntax::PassingMode;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;
use model::{Case, Domain, Pattern, Value};

fn domain_type(f: &mut Fixture, domain: &Domain) -> TypeId {
    let kind = match domain {
        Domain::Bool => TypeKind::Primitive(Primitive::Bool),
        Domain::Unit => TypeKind::Tuple(vec![].into()),
        Domain::Pair(a, b) => TypeKind::Tuple(vec![domain_type(f, a), domain_type(f, b)].into()),
        Domain::Sum(p) => {
            let payload = domain_type(f, p);
            let kind = TypeKind::Variant(
                ["A", "B"]
                    .into_iter()
                    .map(|name| Variant {
                        name: name.into(),
                        fields: vec![(None, payload)].into(),
                    })
                    .collect::<Vec<_>>()
                    .into(),
            );
            let representation = f.definition.types.push(ty(kind.clone()));
            let mut signature = capability_signature("Choice");
            let SignatureKind::Type(header) = &mut signature.kind else {
                unreachable!()
            };
            header.capability = None;
            header.authority = AuthoritySummary::inert();
            header.representation = Some(representation);
            f.signatures.insert(id("Choice"), signature);
            kind
        }
    };
    f.definition.types.push(ty(kind))
}
fn projected(f: &mut Fixture, parent: PlaceId, projection: Projection, ty: TypeId) -> PlaceId {
    let mut p = f.body().places[parent.0 as usize].clone();
    p.projection.push(projection);
    p.ty = ty;
    let id = PlaceId(f.body().places.len() as u32);
    f.body().places.push(p);
    id
}
fn exact(value: &Value) -> Pattern {
    match value {
        Value::Pair(a, b) => Pattern::Pair(Box::new(exact(a)), Box::new(exact(b))),
        Value::Sum(t, p) => Pattern::Sum(*t, Box::new(exact(p))),
        v => Pattern::Exact(v.clone()),
    }
}
fn pattern(
    f: &mut Fixture,
    nodes: &mut Vec<MatchPattern>,
    p: &Pattern,
    place: PlaceId,
) -> MatchPatternId {
    let place_ty = f.body().places[place.0 as usize].ty;
    let kind = match p {
        Pattern::Wild => MatchPatternKind::Wildcard,
        Pattern::Exact(Value::Bool(b)) => MatchPatternKind::Literal(Constant::Bool(*b)),
        Pattern::Exact(Value::Unit) => MatchPatternKind::Literal(Constant::Unit),
        Pattern::Exact(v) => return pattern(f, nodes, &exact(v), place),
        Pattern::Pair(a, b) => {
            let TypeKind::Tuple(ts) = interner().get(place_ty).unwrap().kind.clone() else {
                unreachable!()
            };
            let mut fields = vec![];
            for (i, p) in [a, b].into_iter().enumerate() {
                let child = projected(f, place, Projection::Field(i as u32), ts[i]);
                fields.push((i as u32, pattern(f, nodes, p, child)));
            }
            MatchPatternKind::Tuple(fields)
        }
        Pattern::Sum(tag, p) => {
            let TypeKind::Variant(vs) = interner().get(place_ty).unwrap().kind.clone() else {
                unreachable!()
            };
            let child = projected(
                f,
                place,
                Projection::VariantField {
                    variant: *tag,
                    field: 0,
                },
                vs[*tag as usize].fields[0].1,
            );
            let child = pattern(f, nodes, p, child);
            MatchPatternKind::Variant {
                definition: path("Choice"),
                variant: *tag,
                fields: vec![(0, child)],
            }
        }
        Pattern::Or(_) => unreachable!("TIR adapter distributes alternatives"),
    };
    let id = MatchPatternId(nodes.len() as u32);
    nodes.push(MatchPattern { place, kind });
    id
}
fn fixture(case: &Case) -> Fixture {
    let mut f = Fixture::unit();
    let ty = domain_type(&mut f, &case.domain);
    let scrutinee = PlaceId(param(&mut f, interner().get(ty).unwrap().kind.clone()));
    f.body().blocks[0].terminator = Terminator::Match {
        table: MatchTableId(0),
        from: 0,
    };
    if !case.arms.is_empty() {
        f.body().blocks.push(block(vec![], ret(0, 0)));
    }
    let mut table = MatchTable {
        scrutinee,
        patterns: vec![],
        arms: vec![],
    };
    for arm in &case.arms {
        for p in arm.pattern.expanded() {
            let pattern = pattern(&mut f, &mut table.patterns, &p, scrutinee);
            let guard = arm.guard.map(|value| {
                let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
                let rv = RvalueId(f.body().rvalues.len() as u32);
                f.body()
                    .rvalues
                    .push(Rvalue::Constant(Constant::Bool(value)));
                let guard = BlockId(f.body().blocks.len() as u32);
                f.body().blocks.push(block(
                    vec![inst(
                        InstructionKind::Assign {
                            destination: PlaceId(condition),
                            value: rv,
                        },
                        0,
                    )],
                    Terminator::Branch {
                        condition: copy(condition),
                        yes: BlockId(1),
                        no: BlockId(guard.0 + 1),
                    },
                ));
                f.body().blocks.push(block(
                    vec![],
                    Terminator::Match {
                        table: MatchTableId(0),
                        from: table.arms.len() as u32 + 1,
                    },
                ));
                guard
            });
            table.arms.push(MatchArm {
                pattern,
                guard,
                target: BlockId(1),
            });
        }
    }
    f.body().matches.push(table);
    owner_sig(&mut f, &[PassingMode::Borrow]);
    f
}
#[test]
fn match_coverage_22_direct_v_finite_truth_tables() {
    let mut accepted = 0;
    let cases = model::selected_cases();
    let count = cases.len();
    for case in cases {
        let truth = case.truth();
        let kinds = fixture(&case).kinds();
        assert_eq!(
            kinds.is_empty(),
            truth.exhaustive,
            "{}\n{}\n{kinds:?}",
            case.name,
            case.source()
        );
        assert!(
            kinds.iter().all(|k| *k == ViolationKind::MatchCoverage),
            "{}: {kinds:?}",
            case.name
        );
        // Or nodes are distributed before V. Dead source alternatives are a
        // source rule; V must derive whole-table coverage even for these tables.
        accepted += usize::from(kinds.is_empty());
        assert_eq!(truth.reachable_arms.len(), case.arms.len());
        let _ = truth.dead_alternatives;
    }
    if count == 96 {
        assert_eq!(accepted, 72);
    }
    eprintln!(
        "match_coverage_22: {count} direct V tables, {accepted} accepted, {} coverage rejects",
        count - accepted
    );
}
#[test]
fn match_coverage_22_tag_and_projection_forgeries() {
    let case = model::cases()
        .into_iter()
        .find(|c| c.name == "domain-2-matrix-0")
        .unwrap();
    let mut f = fixture(&case);
    f.valid();
    // Retag B(false) and B(true) as A payloads, updating their projections too.
    // All structural/type/ownership proofs remain valid; only B's coverage goes.
    for p in &mut f.body().matches[0].patterns {
        if let MatchPatternKind::Variant { variant, .. } = &mut p.kind {
            *variant = 0;
        }
    }
    for p in &mut f.body().places {
        let mut projection = p.projection.to_vec();
        for step in &mut projection {
            if let Projection::VariantField { variant, .. } = step {
                *variant = 0;
            }
        }
        p.projection = projection.into();
    }
    f.broken(vec![ViolationKind::MatchCoverage]);
    let mut f = fixture(&case);
    // A valid bool place with the wrong field path must fail independently.
    let place = f.body().matches[0].patterns[0].place;
    f.body().places[place.0 as usize].projection = vec![Projection::VariantField {
        variant: 1,
        field: 0,
    }]
    .into();
    assert_eq!(f.kinds(), vec![ViolationKind::InvalidProjection(place)]);
}
