use crate::support;
use hirsel_interp::{structural::*, *};
use hirsel_syntax::BinaryOp as B;
use hirsel_types::{Primitive as P, TypeKind};
use std::cmp::Ordering;
use support::*;
fn int(n: u128) -> Value {
    Value::Integer(Integer::new(P::U64, n).unwrap())
}
fn nest(n: usize) -> Value {
    let mut v = int(3);
    for _ in 0..n {
        v = Value::Box(vec![Some(v)]);
    }
    v
}

#[test]
fn complete_preflight_precedes_mismatch_and_map_pairs_are_transparent() {
    let a = Value::Tuple(vec![Some(int(0)), Some(nest(MAX_DEPTH))]);
    let b = Value::Tuple(vec![Some(int(1)), Some(nest(MAX_DEPTH))]);
    assert_eq!(binary(B::Eq, &a, &b), Err(StructuralError::Depth));
    a.discard();
    b.discard();
    let map = Value::map(vec![Some(Value::Tuple(vec![
        Some(nest(MAX_DEPTH - 1)),
        Some(int(0)),
    ]))]);
    assert_eq!(preflight(&map), Ok(()));
    assert!(hash(&map, 0).is_ok());
    map.discard();
    let map = Value::map(vec![Some(Value::Tuple(vec![
        Some(nest(MAX_DEPTH)),
        Some(int(0)),
    ]))]);
    assert_eq!(hash(&map, 0), Err(StructuralError::Depth));
    map.discard();
}
#[test]
fn float_nan_and_signed_zero_keep_language_equality_inside_aggregates() {
    let a = Value::Tuple(vec![Some(value::f64_value(f64::NAN))]);
    assert_eq!(binary(B::Eq, &a, &a), Ok(Value::Bool(false)));
    assert_eq!(compare(&a, &a), Ok(None));
    assert_eq!(
        compare(
            &Value::Box(vec![Some(value::f32_value(-0.0))]),
            &Value::Box(vec![Some(value::f32_value(0.0))])
        ),
        Ok(Some(Ordering::Equal))
    );
}
#[test]
fn fixed_depth_boundary_applies_to_equality_hash_and_order_without_host_recursion() {
    let a = nest(MAX_DEPTH);
    let b = a.clone();
    assert_eq!(compare(&a, &b), Ok(Some(Ordering::Equal)));
    assert_eq!(hash(&a, 0), hash(&b, 0));
    a.discard();
    b.discard();
    let a = nest(MAX_DEPTH + 1);
    let b = a.clone();
    assert_eq!(compare(&a, &b), Err(StructuralError::Depth));
    assert_eq!(hash(&a, 0), Err(StructuralError::Depth));
    a.discard();
    b.discard();
    let a = nest(30_000);
    let b = a.clone();
    assert_eq!(compare(&a, &b), Err(StructuralError::Depth));
    a.discard();
    b.discard();
}
#[test]
fn tir_depth_failure_uses_mem3_site_and_runs_emitted_cleanup() {
    let mut b = Builder::new();
    let p = b.parameter(TypeKind::Tuple(vec![].into()));
    let q = b.parameter(TypeKind::Tuple(vec![].into()));
    let c = b.cleanup(vec![drop(q), drop(p)]);
    let result = b.binary(B::Eq, p, q, TypeKind::Primitive(P::Bool), 8, c);
    let e = execute(
        vec![b.finish("main", result)],
        vec![nest(30_000), nest(30_000)],
        Limits::default(),
    );
    let Outcome::Failed(f) = e.outcome else {
        panic!()
    };
    assert_eq!(f.rule.as_str(), "MEM-3");
    assert_eq!(f.kind, "Depth");
    assert_eq!(f.line, 8);
    assert_eq!(
        e.trace
            .iter()
            .filter(|e| matches!(e, TraceEvent::Cleanup { .. }))
            .count(),
        2
    );
}

#[test]
fn internal_set_order_sorts_structurally_then_compares_lexicographically() {
    // Spec #93: internal structural ordering only; source < stays scalar/bytes.
    let set = |xs: &[u128]| Value::set(xs.iter().map(|&x| Some(int(x))).collect());
    assert_eq!(
        compare(&set(&[256]), &set(&[2])),
        Ok(Some(Ordering::Greater))
    );
    assert_eq!(compare(&set(&[9, 1]), &set(&[2])), Ok(Some(Ordering::Less)));
    assert_eq!(compare(&set(&[1]), &set(&[1, 9])), Ok(Some(Ordering::Less)));
    assert_eq!(
        compare(&set(&[9, 1]), &set(&[1, 9])),
        Ok(Some(Ordering::Equal))
    );
}
