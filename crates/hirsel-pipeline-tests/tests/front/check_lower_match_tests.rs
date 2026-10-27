//! Independent source/IR matrix oracles and retained-size regressions.
use crate::check_support::path;
use hirsel_engine::{DefinitionQueries, QueryEngine};
use hirsel_syntax::FileContext;
use hirsel_tir::*;
use hirsel_types::{Primitive, TypeKind};
use proptest::prelude::*;

fn engine(source: String) -> QueryEngine {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = FileContext {
        package: path("main").0.package,
        modules: vec![],
    };
    let file = engine.sources.add("matrix.hir", source).unwrap();
    let parsed = engine.parse_file(file, &context).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    engine
}
fn matrix_source(width: usize, rows: &[Vec<u8>]) -> String {
    let arms = rows
        .iter()
        .map(|row| {
            format!(
                "({})=>()",
                row.iter()
                    .map(|cell| match cell {
                        0 => "false",
                        1 => "true",
                        _ => "_",
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "fn main(p:({}))->(){{match p{{{arms}}}}}",
        vec!["bool"; width].join(",")
    )
}
/// Only the one-wildcard source is lowered. The candidate matrix is built by the
/// test, independently of source coverage and K pattern conversion.
fn matrix_ir(width: usize, rows: &[Vec<u8>]) -> (Definition, hirsel_validate::SignatureMap) {
    let mut engine = engine(format!(
        "fn main(p:({}))->(){{match p{{_=>()}}}}",
        vec!["bool"; width].join(",")
    ));
    let mut definition = (*engine.lower_to_tir(&path("main")).unwrap().value).clone();
    let signature = engine.item_signature(&path("main")).unwrap().value;
    let body = &mut definition.bodies[0];
    let table = &mut body.matches[0];
    let target = table.arms[0].target;
    let root = body.places[table.scrutinee.0 as usize].clone();
    let TypeKind::Tuple(types) = &engine.types().get(root.ty).unwrap().kind else {
        panic!("tuple source")
    };
    let mut columns = vec![];
    for (column, ty) in types.iter().enumerate() {
        assert_eq!(
            engine.types().get(*ty).unwrap().kind,
            TypeKind::Primitive(Primitive::Bool)
        );
        let mut place = root.clone();
        place.projection.push(Projection::Field(column as u32));
        place.ty = *ty;
        columns.push(PlaceId(body.places.len() as u32));
        body.places.push(place);
    }
    table.patterns.clear();
    table.arms.clear();
    for row in rows {
        let mut fields = vec![];
        for (column, cell) in row.iter().enumerate() {
            let id = MatchPatternId(table.patterns.len() as u32);
            table.patterns.push(MatchPattern {
                place: columns[column],
                kind: match cell {
                    0 => MatchPatternKind::Literal(Constant::Bool(false)),
                    1 => MatchPatternKind::Literal(Constant::Bool(true)),
                    _ => MatchPatternKind::Wildcard,
                },
            });
            fields.push((column as u32, id));
        }
        let pattern = MatchPatternId(table.patterns.len() as u32);
        table.patterns.push(MatchPattern {
            place: table.scrutinee,
            kind: MatchPatternKind::Tuple(fields),
        });
        table.arms.push(MatchArm {
            pattern,
            guard: None,
            target,
        });
    }
    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
    signatures.insert(signature.definition, (*signature).clone());
    (definition, signatures)
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn independently_generated_pattern_matrices_agree_with_body_coverage(
        rows in prop::collection::vec(prop::collection::vec(0u8..3, 4), 1..16)
    ) {
        let mut source = engine(matrix_source(4, &rows));
        let h = match source.check_definition(&path("main")) {
            Ok(checked) => { prop_assert!(checked.diagnostics.is_empty()); true }
            Err(hirsel_types::QueryError::Diagnostics(ds)) => {
                prop_assert!(ds.iter().all(|d| d.message == "match does not cover every value"), "{:?}", ds);
                false
            }
            Err(e) => return Err(TestCaseError::fail(format!("unexpected source result {e:?}"))),
        };
        let (definition, signatures) = matrix_ir(4, &rows);
        let report = hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![]);
        let v = report.violations.is_empty();
        prop_assert_eq!(h, v, "{:?}", report.violations);
        prop_assert!(report.violations.iter().all(|e| e.kind == hirsel_validate::ViolationKind::MatchCoverage));
    }
}
#[test]
fn missing_product_case_and_pattern_edge_mutations_fail_closed() {
    let rows = vec![vec![1, 2], vec![0, 1], vec![0, 0]];
    let (mut definition, signatures) = matrix_ir(2, &rows);
    hirsel_validate::assert_valid(&definition, signatures.types(), &signatures);
    let original = definition.clone();
    definition.bodies[0].matches[0].arms.pop();
    // Remove the now unused nodes to isolate the missing-case coverage obligation.
    definition.bodies[0].matches[0].patterns.truncate(6);
    assert!(
        hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::MatchCoverage)
    );
    let mut definition = original.clone();
    if let MatchPatternKind::Tuple(fields) = &mut definition.bodies[0].matches[0].patterns[2].kind {
        fields[0].1 = MatchPatternId(2);
    }
    assert!(
        !hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![])
            .violations
            .is_empty()
    );
    let mut definition = original;
    definition.bodies[0].matches[0].patterns[0].kind =
        MatchPatternKind::Literal(Constant::Integer(1));
    assert!(
        hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "match pattern"
                }
            ))
    );
}
fn pigeons(n: usize) -> Vec<Vec<u8>> {
    let holes = n - 1;
    let width = n * holes;
    let mut rows = vec![];
    for pigeon in 0..n {
        rows.push(
            (0..width)
                .map(|i| if i / holes == pigeon { 0 } else { 2 })
                .collect(),
        );
    }
    for a in 0..n {
        for b in a + 1..n {
            for hole in 0..holes {
                rows.push(
                    (0..width)
                        .map(|i| {
                            if i == a * holes + hole || i == b * holes + hole {
                                1
                            } else {
                                2
                            }
                        })
                        .collect(),
                );
            }
        }
    }
    rows
}
#[test]
fn adversarial_product_coverage_has_a_deterministic_work_budget() {
    for n in 2..=8 {
        let rows = pigeons(n);
        let cells = rows.len() * n * (n - 1);
        let (definition, signatures) = matrix_ir(n * (n - 1), &rows);
        let before = hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateMatchWork);
        let report =
            hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![]);
        let work =
            hirsel_intern::work::read(hirsel_intern::work::Counter::ValidateMatchWork) - before;
        eprintln!(
            "pigeonhole n={n}: {cells} cells, {} patterns, {} arms, {work} coverage work: {:?}",
            definition.bodies[0].matches[0].patterns.len(),
            rows.len(),
            report.violations
        );
        assert!(
            report.violations.iter().all(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::MatchToolingLimit { .. }
            )),
            "{:?}",
            report.violations
        );
        assert!(work <= hirsel_validate::MAX_MATCH_WORK + (cells + rows.len()) as u64 + 1);
    }
}
#[test]
fn source_product_tables_and_n_arm_matches_retain_linear_ir() {
    for n in [2, 4, 5] {
        let rows = pigeons(n);
        let mut source = engine(matrix_source(n * (n - 1), &rows));
        let definition = source.lower_to_tir(&path("main")).unwrap().value;
        let body = &definition.bodies[0];
        let cells = rows.len() * n * (n - 1);
        let nodes = body.matches[0].patterns.len();
        assert_eq!(nodes, cells + rows.len());
        assert_eq!(body.matches[0].arms.len(), rows.len());
        assert!(body.blocks.len() <= rows.len() + 2);
        assert!(
            !body
                .blocks
                .iter()
                .any(|b| matches!(b.terminator, Terminator::Unreachable))
        );
        eprintln!(
            "source pigeonhole n={n}: cells={cells} nodes={nodes} arms={} blocks={}",
            rows.len(),
            body.blocks.len()
        );
    }
    let mut counts = vec![];
    for n in [32, 128] {
        let arms = (0..n)
            .map(|i| format!("{i}u64=>()"))
            .collect::<Vec<_>>()
            .join(",");
        let mut source = engine(format!("fn main(n:u64){{match n{{{arms},_=>()}}}}"));
        let definition = source.lower_to_tir(&path("main")).unwrap().value;
        let body = &definition.bodies[0];
        let instructions: usize = body.blocks.iter().map(|b| b.instructions.len()).sum();
        counts.push(
            body.blocks.len()
                + body.matches[0].patterns.len()
                + body.matches[0].arms.len()
                + instructions,
        );
    }
    assert!(counts[1] <= 4 * counts[0]);
}

#[test]
fn guard_restarts_require_their_own_false_edge_and_preserve_earlier_coverage() {
    let mut source = engine(
        "fn main(b:bool)->u8 {match b {true=>1, false if false=>2, false if false=>3, false=>4}}"
            .into(),
    );
    let mut definition = (*source.lower_to_tir(&path("main")).unwrap().value).clone();
    let sig = source.item_signature(&path("main")).unwrap().value;
    let mut signatures = hirsel_validate::SignatureMap::new(source.types().clone());
    signatures.insert(sig.definition, (*sig).clone());
    hirsel_validate::assert_valid(&definition, signatures.types(), &signatures);
    let body = &definition.bodies[0];
    let restarts: Vec<_> = body
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, b)| {
            matches!(b.terminator, Terminator::Match { from, .. } if from > 0).then_some(index)
        })
        .collect();
    assert_eq!(restarts.len(), 2);
    let mut bypass = definition.clone();
    let target = bypass.bodies[0].matches[0].arms[1].target;
    bypass.bodies[0].blocks[target.0 as usize].terminator =
        Terminator::Goto(BlockId(restarts[0] as u32));
    assert!(
        hirsel_validate::validate(&bypass, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|e| e.kind == hirsel_validate::ViolationKind::MatchCoverage)
    );
    definition.bodies[0].entry = BlockId(restarts[0] as u32);
    assert!(
        hirsel_validate::validate(&definition, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|e| e.kind == hirsel_validate::ViolationKind::MatchCoverage)
    );
}

#[test]
fn nested_matches_reserve_independent_tables_and_diverging_matches_have_no_sentinel() {
    let mut source = engine(
        "fn main(a:bool,b:bool)->u8 {match a {true=>match b {true=>1,false=>2},false=>3}}".into(),
    );
    let definition = source.lower_to_tir(&path("main")).unwrap().value;
    assert_eq!(definition.bodies[0].matches.len(), 2);
    let mut source =
        engine("fn main(b:bool)->u8 {match b {true=>{return 1;},false=>{return 2;}}; 3}".into());
    let definition = source.lower_to_tir(&path("main")).unwrap().value;
    assert!(
        !definition.bodies[0]
            .blocks
            .iter()
            .any(|b| matches!(b.terminator, Terminator::Unreachable))
    );
}

#[test]
fn binding_metadata_cannot_disguise_guard_takes_or_commit_borrows() {
    let mut source =
        engine("fn main(take s:String)->u8 {match take s {x if x==\"yes\"=>1,_=>2}}".into());
    let definition = source.lower_to_tir(&path("main")).unwrap().value;
    let sig = source.item_signature(&path("main")).unwrap().value;
    let mut signatures = hirsel_validate::SignatureMap::new(source.types().clone());
    signatures.insert(sig.definition, (*sig).clone());
    hirsel_validate::assert_valid(&definition, signatures.types(), &signatures);
    let mut forged = (*definition).clone();
    let node = forged.bodies[0].matches[0]
        .patterns
        .iter_mut()
        .find(|n| matches!(n.kind, MatchPatternKind::Binding { .. }))
        .unwrap();
    if let MatchPatternKind::Binding { usage, .. } = &mut node.kind {
        *usage = hirsel_types::UseKind::Borrow;
    }
    assert!(
        hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::InvalidUse(_)))
    );
    let mut forged = (*definition).clone();
    let node = forged.bodies[0].matches[0]
        .patterns
        .iter_mut()
        .find(|n| matches!(n.kind, MatchPatternKind::Binding { .. }))
        .unwrap();
    if let MatchPatternKind::Binding { guard_usage, .. } = &mut node.kind {
        *guard_usage = hirsel_types::UseKind::Take;
    }
    assert!(
        hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|v| matches!(v.kind, hirsel_validate::ViolationKind::InvalidUse(_)))
    );
}

#[test]
fn a_failed_guard_cannot_replace_the_value_covered_by_earlier_arms() {
    let mut source =
        engine("fn main(b:bool)->u8 {match b {true=>1,false if false=>2,false=>3}}".into());
    let definition = source.lower_to_tir(&path("main")).unwrap().value;
    let sig = source.item_signature(&path("main")).unwrap().value;
    let mut signatures = hirsel_validate::SignatureMap::new(source.types().clone());
    signatures.insert(sig.definition, (*sig).clone());
    hirsel_validate::assert_valid(&definition, signatures.types(), &signatures);
    let mut forged = (*definition).clone();
    let body = &mut forged.bodies[0];
    let table = &body.matches[0];
    let scrutinee = table.scrutinee;
    let guard = table.arms[1].guard.unwrap();
    let ty = body.places[scrutinee.0 as usize].ty;
    let scope = body.locals[body.places[scrutinee.0 as usize].root.0 as usize].scope;
    let mut scratch = vec![];
    for _ in 0..2 {
        let root = LocalId(body.locals.len() as u32);
        body.locals.push(Local {
            ty,
            storage: Storage::Temporary,
            scope,
            span: body.blocks[guard.0 as usize].span,
        });
        let place = PlaceId(body.places.len() as u32);
        body.places.push(Place {
            root,
            projection: Default::default(),
            ty,
            loan: None,
            binding: None,
        });
        scratch.push(place);
    }
    let value = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(Rvalue::Constant(Constant::Bool(true)));
    let span = body.blocks[guard.0 as usize].span;
    for kind in [
        InstructionKind::StorageLive(body.places[scratch[0].0 as usize].root),
        InstructionKind::Assign {
            destination: scratch[0],
            value,
        },
        InstructionKind::StorageLive(body.places[scratch[1].0 as usize].root),
        InstructionKind::Replace {
            destination: scrutinee,
            value: Operand {
                place: scratch[0],
                usage: hirsel_types::UseKind::Take,
            },
            displaced: scratch[1],
            if_live: false,
        },
    ] {
        body.blocks[guard.0 as usize]
            .instructions
            .push(Instruction {
                kind,
                span,
                on_failure: CleanupId(0),
            });
    }
    // The guard remains false. Replacing false with true would otherwise resume
    // after the true arm and leave no arm to select despite whole-table coverage.
    assert!(
        hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![])
            .violations
            .iter()
            .any(|v| v.kind == hirsel_validate::ViolationKind::MatchCoverage)
    );
}
