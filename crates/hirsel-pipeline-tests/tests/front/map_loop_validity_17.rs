//! The retained operation loop and its strict independent-V controls.
use hirsel_engine::{
    DefinitionQueries, QueryEngine, check_snapshot, default_context, default_environment,
};

const TWO_OPS: &str = r#"fn visible_scalar(m: Map[u64,List[u64]], take expected: List[(u64,List[u64])]) -> bool {
  if map.len(m) != list.len(expected) { return false }
  let mut i: u64 = 0
  for (k,v) in m {
    if (k,v) != expected[i] { return false }
    i += 1
  }
  let mut forward: Map[u64,List[u64]] = map.empty()
  let mut reverse: Map[u64,List[u64]] = map.empty()
  for (k,v) in expected { map.insert(mut forward, copy k, copy v) }
  let mut j = list.len(expected)
  while j > 0 { j -= 1; let (k,v) = expected[j]; map.insert(mut reverse, copy k, copy v) }
  m == forward && true && ((m == reverse) == (i <= 1))
}
fn consume_scalar(take m: Map[u64,List[u64]]) -> Option[(u64,List[u64])] {
  let mut first: Option[(u64,List[u64])] = None
  for (k,v) in take m { first = Some((take k,take v)); break }
  take first
}
fn run_scalar(take ops: List[(u64,u64,u64)], take answers: List[Option[List[u64]]], take states: List[List[(u64,List[u64])]], id: u64) -> u64 {
  let mut m: unique Map[u64,List[u64]] = map.empty()
  let mut step: u64 = 0
  for (tag,k,v) in ops {
    let code = id * 1000 + (step + 1) * 10
    if tag == 0 {
      map.insert(mut m, unique copy k, [v,v+100])
      if map.get(m,k) != answers[step] { return code + 1 }
    } else if tag == 1 {
      if map.get(m,k) != answers[step] { return code + 2 }
    } else if tag == 2 {
      m = unique m
      if map.remove(mut m,k) != answers[step] { return code + 3 }
    } else {
      let mut first: Option[List[u64]] = None
      for (_,value) in m { first = Some(unique value); break }
      if first != answers[step] { return code + 4 }
    }
    if !visible_scalar(m,copy states[step]) { return code + 5 }
    step += 1
  }
  let last = states[step-1]
  let expected: Option[(u64,List[u64])] = if list.len(last) == 0 { None } else { Some(copy last[0]) }
  let saved = copy m
  if consume_scalar(take m) != expected { return id * 1000 + step * 10 + 6 }
  if !visible_scalar(saved,copy last) { return id * 1000 + step * 10 + 7 }
  if consume_scalar(take saved) != expected { return id * 1000 + step * 10 + 8 }
  0
}

fn main()->u64 { run_scalar([(2,4,0),(2,99,0)], [None,None], [[],[]], 1) }
"#;

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
    let file = engine.sources.add("map-loop.hir", source.into()).unwrap();
    let diagnostics = check_snapshot(&mut engine, file, &default_context()).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    engine
}

#[test]
fn retained_two_operation_loop() {
    let mut engine = checked(TWO_OPS);
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
}

#[test]
fn extraction_actions_remain_distinct() {
    for (source, expected) in [
        (
            "fn main(take xs: unique List[u64]) -> unique List[u64] { unique xs }",
            hirsel_types::UniqueAction::AlreadyUnique,
        ),
        (
            "fn main(xs: List[u64]) -> unique List[u64] { unique xs }",
            hirsel_types::UniqueAction::DeepCopy,
        ),
    ] {
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine.sources.add("extraction.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let result = engine
            .mode_definition(&hirsel_engine::entry_path(&default_context()))
            .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(result.value.ownership.unique.len(), 1);
        assert_eq!(result.value.ownership.unique[0].action, expected);
    }
}

#[test]
fn v_rejects_the_original_conditional_replacement_and_a_moved_owner() {
    use hirsel_tir::{InstructionKind, Rvalue};
    use hirsel_types::UseKind;
    use hirsel_validate::ViolationKind;
    let mut engine = checked(TWO_OPS);
    let mut path = hirsel_engine::entry_path(&default_context());
    path.0.name = "run_scalar".into();
    let original = engine.lower_to_tir(&path).unwrap().value;
    let signatures = engine.signature_snapshot();
    let signatures = signatures.validation(&path.0.package);
    hirsel_validate::assert_valid(&original, engine.types(), signatures);
    let mut conditional = (*original).clone();
    let body = &mut conditional.bodies[conditional.entry.0 as usize];
    let owner = body
        .rvalues
        .iter()
        .find_map(|r| match r {
            Rvalue::Unary {
                op: hirsel_syntax::UnaryOp::Unique,
                value,
            } if value.usage == UseKind::Copy
                && matches!(
                    engine
                        .types()
                        .get(body.places[value.place.0 as usize].ty)
                        .unwrap()
                        .kind,
                    hirsel_types::TypeKind::Builtin {
                        kind: hirsel_types::BuiltinType::Map,
                        ..
                    }
                ) =>
            {
                Some(body.places[value.place.0 as usize].root)
            }
            _ => None,
        })
        .unwrap();
    let mut changed = 0;
    for block in &mut body.blocks {
        for instruction in &mut block.instructions {
            if let InstructionKind::Replace {
                destination,
                if_live,
                ..
            } = &mut instruction.kind
                && body.places[destination.0 as usize].root == owner
            {
                assert!(!*if_live);
                *if_live = true;
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 1);
    let report = hirsel_validate::validate(&conditional, engine.types(), signatures, &mut vec![]);
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, ViolationKind::InvalidUse(_))),
        "{:?}",
        report.violations
    );

    let mut moved = (*original).clone();
    let body = &mut moved.bodies[moved.entry.0 as usize];
    let moved_operand = body
        .rvalues
        .iter_mut()
        .find_map(|r| match r {
            // The final saved = copy m must leave m available for its consume.
            Rvalue::Use(value)
                if value.usage == UseKind::Copy
                    && body.places[value.place.0 as usize].root == owner =>
            {
                Some(value)
            }
            _ => None,
        })
        .unwrap();
    moved_operand.usage = UseKind::Take;
    let report = hirsel_validate::validate(&moved, engine.types(), signatures, &mut vec![]);
    assert!(
        report.violations.iter().any(|v| matches!(
            v.kind,
            ViolationKind::UseAfterMove(_) | ViolationKind::ReadBeforeInit(_)
        )),
        "{:?}",
        report.violations
    );
}
