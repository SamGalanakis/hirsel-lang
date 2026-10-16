//! Pushes into reserved storage (FACT-3, `hirsel_rc::spare_push`): a proven
//! push appends without its capacity read or growth branch, an unproven one
//! keeps them, and H's slot bound catches a reservation forged smaller.
use crate::rc_support as support;

use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_heapcheck::{Config, NoOperations, Observed};
use hirsel_interp::observation::ObservedNode;
use hirsel_lir::ir::{
    ArgumentValue, Callee, ConstantValue, Inst, LirUnit, Terminator, UnitOrigin, ValueId,
};
use hirsel_lir::stage::{Ready, Value};
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::import::Lowered;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::InstanceId;
use hirsel_rc::library::core_path;
use hirsel_rc::placement::Options;

use support::*;

/// `main` returns `value()` as its `Err` payload, so both executors report it.
const ENTRY: &str = "pub fn main() -> Result[(), u64] {\n  Error(value())\n}\n";

/// A weighted sum of a list's elements and its length.
const SUM: &str = "fn sum(xs: List[u64]) -> u64 {
  let mut s: u64 = list.len(xs)
  let mut i: u64 = 0
  while i < list.len(xs) {
    s = s + xs[i] * (i + 1)
    i = i + 1
  }
  s
}
";

fn source(body: &str) -> String {
    format!("{SUM}{body}{ENTRY}")
}

/// The callees `value` calls directly, by definition path.
fn pushes(lowered: &Lowered) -> (usize, usize) {
    let u = unit_named(lowered, "value");
    let named = |path: &hirsel_syntax::DefPath| {
        u.terminators
            .iter()
            .filter(|t| match t {
                Terminator::Call { call, .. } => match u.tables.calls[call.index()].callee {
                    Callee::Direct(s) => u.tables.symbols[s.index()].definition == *path,
                    _ => false,
                },
                _ => false,
            })
            .count()
    };
    (
        named(&hirsel_rc::spare_push::spare_path()),
        named(&core_path("list.push")),
    )
}

/// Every stage to Ready; C accepts every unit at every stage.
fn ready(source: &str, lowered: Lowered) -> hirsel_heapcheck::Program<Ready> {
    let program = compile(source);
    let table = lowered.table.clone();
    let manifest = manifest(&program);
    let target = target();
    let operations = program.operation_registry();
    let inputs = Inputs {
        operations: &operations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let checked = |stage: &str, v: Vec<hirsel_lir::validate::Violation>, d: String| {
        assert!(v.is_empty(), "{source}\n{stage}\n{d}\n{v:#?}");
    };
    let mut rc = Vec::new();
    for (i, u) in lowered.units.iter().enumerate() {
        checked("value", hirsel_lir::validate::check(u, &inputs), dump(u));
        let id = InstanceId(i as u32);
        let options = Options::for_instance(true, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u.clone(), &table, &options).unwrap();
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let r = hirsel_rc::insert::insert(placed, &table, &proofs)
            .unwrap_or_else(|e| panic!("{source}\ninsert: {e:?}"));
        checked("rc", hirsel_lir::validate::check(&r, &inputs), dump(&r));
        rc.push(hirsel_rc::elide::elide(r, &table).unwrap());
    }
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let units: Vec<LirUnit<Ready>> = rc
        .into_iter()
        .map(|u| hirsel_rc::ready::ready(u, &table, &finalizers).unwrap())
        .collect();
    for u in &units {
        checked("ready", hirsel_lir::validate::check(u, &inputs), dump(u));
    }
    hirsel_heapcheck::Program {
        target,
        layouts: table,
        units,
        statics: lowered.statics,
    }
}

/// H's `value()`, auditing every injected failure site, or the violation.
fn heap(program: &hirsel_heapcheck::Program<Ready>) -> Result<u64, String> {
    let campaign = hirsel_heapcheck::fault_campaign(
        program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    if let Some((site, v)) = campaign.first_violation() {
        return Err(format!("site {site:?}: {v}"));
    }
    let observation = campaign
        .clean
        .result
        .map_err(|v| v.to_string())?
        .observation;
    assert!(
        observation.failures.is_empty(),
        "{:?}",
        observation.failures
    );
    match observation.returned {
        Some(Observed::Variant { fields, .. }) => match fields.into_iter().next().flatten() {
            Some(Observed::Integer { bits, .. }) => Ok(bits as u64),
            other => panic!("returned {other:?}"),
        },
        other => panic!("returned {other:?}"),
    }
}

/// The interpreter's `value()`.
fn reference(source: &str) -> u64 {
    let source = source.to_string();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let config = RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let o = run.observation.expect("an observation");
        assert!(o.failures.is_empty(), "{source}\n{:?}", o.failures);
        let value = o.returned.expect("a returned value");
        let payload = match &value.nodes[0] {
            ObservedNode::Aggregate { fields, .. } => fields[0].expect("an Err payload"),
            other => panic!("{source}\nreturned {other:?}"),
        };
        match &value.nodes[payload] {
            ObservedNode::Integer(i) => i.bits() as u64,
            other => panic!("{source}\nreturned {other:?}"),
        }
    })
}

/// Lower `body`, expect `spare` proven and `kept` ordinary pushes in
/// `value`, and agree with the interpreter under H's fault campaign.
fn check(body: &str, spare: usize, kept: usize) {
    let source = source(body);
    let (_, lowered) = lower(&source);
    assert_eq!(pushes(&lowered), (spare, kept), "{source}");
    let spare_units = lowered
        .units
        .iter()
        .filter(|u| {
            u.header.origin == UnitOrigin::Library
                && u.header.definition == hirsel_rc::spare_push::spare_path()
        })
        .count();
    assert_eq!(spare_units, usize::from(spare > 0), "{source}");
    let h = heap(&ready(&source, lowered)).unwrap_or_else(|v| panic!("{source}\n{v}"));
    assert_eq!(
        h,
        reference(&source),
        "{source}\nH disagrees with the interpreter"
    );
}

/// A fresh list reserved for `n` with one push per index below `n`.
const RESERVED: &str = "fn value() -> u64 {
  let n: u64 = 40
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  for j in 0u64..n {
    list.push(mut xs, j * 3)
  }
  sum(xs)
}
";

#[test]
fn a_push_per_index_below_the_reservation_appends_into_spare_capacity() {
    check(RESERVED, 1, 0);
}

#[test]
fn equal_constant_bounds_and_smaller_loops_are_within_the_reservation() {
    check(
        "fn value() -> u64 {
  let mut xs: List[u64] = []
  list.reserve(mut xs, 30)
  for j in 0u64..20u64 {
    list.push(mut xs, j)
  }
  let mut ys: List[u64] = []
  list.reserve(mut ys, 30)
  for j in 0u64..30u64 {
    list.push(mut ys, j)
  }
  sum(xs) * 1000 + sum(ys)
}
",
        2,
        0,
    );
}

#[test]
fn conditional_pushes_breaks_and_rebuilt_lists_stay_within_the_reservation() {
    check(
        "fn value() -> u64 {
  let mut total: u64 = 0
  for k in 0u64..6u64 {
    let n = k * 5
    let mut xs: List[u64] = []
    list.reserve(mut xs, n)
    for j in 0u64..n {
      if j % 3 != 1 {
        list.push(mut xs, j + k)
      }
      if j == 20 {
        break
      }
    }
    total = total + sum(xs)
  }
  total
}
",
        1,
        0,
    );
}

/// A copy before the loop shares the reserved block; the first push
/// detaches it at the same capacity.
#[test]
fn a_list_shared_before_the_loop_detaches_with_its_capacity() {
    check(
        "fn value() -> u64 {
  let n: u64 = 25
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  let ys = copy xs
  for j in 0u64..n {
    list.push(mut xs, j)
  }
  sum(xs) * 1000 + list.len(ys)
}
",
        1,
        0,
    );
}

#[test]
fn unproven_pushes_keep_their_capacity_check() {
    // No reservation.
    check(
        "fn value() -> u64 {
  let mut xs: List[u64] = []
  for j in 0u64..20u64 {
    list.push(mut xs, j)
  }
  sum(xs)
}
",
        0,
        1,
    );
    // A loop past the reservation, and a constant one past it.
    check(
        "fn value() -> u64 {
  let n: u64 = 20
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  for j in 0u64..n + 9 {
    list.push(mut xs, j)
  }
  let mut ys: List[u64] = []
  list.reserve(mut ys, 10)
  for j in 0u64..19u64 {
    list.push(mut ys, j)
  }
  sum(xs) * 1000 + sum(ys)
}
",
        0,
        2,
    );
    // A list that already holds elements, and an index from one.
    check(
        "fn value() -> u64 {
  let n: u64 = 20
  let mut xs: List[u64] = []
  list.push(mut xs, 7)
  list.reserve(mut xs, n)
  for j in 0u64..n {
    list.push(mut xs, j)
  }
  let mut ys: List[u64] = []
  list.reserve(mut ys, n)
  for j in 1u64..n {
    list.push(mut ys, j)
  }
  sum(xs) * 1000 + sum(ys)
}
",
        0,
        3,
    );
    // Two pushes per index, and a push in an inner loop.
    check(
        "fn value() -> u64 {
  let n: u64 = 12
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  for j in 0u64..n {
    list.push(mut xs, j)
    list.push(mut xs, j + 1)
  }
  let mut ys: List[u64] = []
  list.reserve(mut ys, n)
  for j in 0u64..n {
    for k in 0u64..2u64 {
      list.push(mut ys, j + k)
    }
  }
  sum(xs) * 1000 + sum(ys)
}
",
        0,
        3,
    );
    // The list read inside the loop.
    check(
        "fn value() -> u64 {
  let n: u64 = 20
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  let mut seen: u64 = 0
  for j in 0u64..n {
    seen = seen + list.len(xs)
    list.push(mut xs, j)
  }
  sum(xs) * 1000 + seen
}
",
        0,
        1,
    );
}

/// A copy reserved for `bytes.len(data)` and pushed per index below
/// `bytes.len(data)`: the two reads are one slice's length, so the push is
/// spare (expr-compiler's `owned`). Twin: a bound read from another slice of a
/// different length keeps the capacity check, and so does the length of a
/// list, whose count a push can change between the reads.
#[test]
fn two_reads_of_one_slices_length_bound_the_reservation() {
    let bytes = "  let mut src: List[u8] = []
  for j in 0u64..30u64 {
    list.push(mut src, int.narrow(97u64 + j % 26u64))
  }
  let data = bytes.from_list(take src)
";
    check(
        &format!(
            "fn value() -> u64 {{
{bytes}  let mut xs: List[u8] = []
  list.reserve(mut xs, bytes.len(data))
  for i in 0u64..bytes.len(data) {{
    list.push(mut xs, data[i])
  }}
  list.len(xs) * 1000 + int.widen(xs[29])
}}
"
        ),
        1,
        1,
    );
    check(
        &format!(
            "fn value() -> u64 {{
{bytes}  let mut more: List[u8] = []
  for j in 0u64..40u64 {{
    list.push(mut more, int.narrow(65u64 + j % 26u64))
  }}
  let other = bytes.from_list(take more)
  let mut xs: List[u8] = []
  list.reserve(mut xs, bytes.len(data))
  for i in 0u64..bytes.len(other) {{
    list.push(mut xs, other[i])
  }}
  let mut ys: List[u8] = []
  list.reserve(mut ys, list.len(xs))
  for i in 0u64..list.len(xs) {{
    list.push(mut ys, xs[i])
    list.push(mut xs, 0u8)
  }}
  list.len(xs) * 1000 + list.len(ys)
}}
"
        ),
        0,
        5,
    );
}

/// The reserve's capacity argument of `value`, replaced by the constant
/// `small` defined before it: the push stays spare, the storage does not.
fn forge(lowered: &mut Lowered, small: u128) {
    let index = lowered
        .units
        .iter()
        .position(|u| u.header.definition == path("value"))
        .unwrap();
    let u: &mut LirUnit<Value> = &mut lowered.units[index];
    let constant: ValueId = u
        .instructions
        .iter()
        .find_map(|i| match *i {
            Inst::Const { result, constant }
                if u.tables.constants[constant.index()].value == ConstantValue::Integer(small) =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("the small constant");
    let reserve = core_path("list.reserve");
    let call = u
        .terminators
        .iter()
        .find_map(|t| match *t {
            Terminator::Call { call, .. }
                if matches!(u.tables.calls[call.index()].callee,
                    Callee::Direct(s) if u.tables.symbols[s.index()].definition == reserve) =>
            {
                Some(call)
            }
            _ => None,
        })
        .expect("the reserve");
    let range = u.tables.calls[call.index()].arguments;
    let arguments = &mut u.pools.call_arguments[range.start as usize..][..range.len as usize];
    let capacity = arguments.iter_mut().find(|a| a.parameter == 1).unwrap();
    capacity.value = ArgumentValue::Value(constant);
}

/// The proof is load-bearing: the spare append writes past the storage a
/// smaller reservation leaves, and H's slot bound rejects it. The same
/// forgery against an ordinary push grows and agrees.
#[test]
fn a_reservation_forged_smaller_than_the_loop_fails_in_h() {
    let body = "fn value() -> u64 {
  let small: u64 = 1
  let n: u64 = 40
  let mut xs: List[u64] = []
  list.reserve(mut xs, n)
  for j in 0u64..n {
    list.push(mut xs, j)
  }
  sum(xs) + small
}
";
    let source = source(body);
    let (_, mut lowered) = lower(&source);
    assert_eq!(pushes(&lowered), (1, 0));
    let honest = heap(&ready(&source, lower(&source).1)).unwrap();
    assert_eq!(honest, reference(&source));
    forge(&mut lowered, 1);
    let forged = heap(&ready(&source, lowered)).expect_err("a forged reservation");
    assert!(forged.contains("element slot"), "{forged}");

    let unproven = body.replace("0u64..n {", "0u64..n + 0 {");
    let source = self::source(&unproven);
    let (_, mut lowered) = lower(&source);
    assert_eq!(pushes(&lowered), (0, 1));
    forge(&mut lowered, 1);
    assert_eq!(heap(&ready(&source, lowered)).unwrap(), reference(&source));
}
