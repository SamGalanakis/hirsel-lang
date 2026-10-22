//! Raw Packed storage witnesses, independent of the public std list wrappers.
use crate::rc_support::*;
use hirsel_heapcheck::{Config, NoOperations, Observed};

const TYPES: &str = "type V = Small(x: u8) | Big(x: u64, y: u64)\n";

fn raw_repeat(n: u64) -> String {
    format!(
        "{TYPES}fn filled(n: u64, take seed: Packed[V]) -> List[Packed[V]] {{
  let mut xs: List[Packed[V]] = []
  list.reserve(mut xs, n)
  for i in 0u64..n {{ list.push(mut xs, copy seed) }}
  xs
}}
fn run() -> u64 {{
  let seed: Packed[V] = Big(x: 1, y: 2)
  let xs = filled({n}, take seed)
  let alias = copy xs
  let mut sum = list.len(alias)
  for x in xs {{ sum = sum + x.Big.x + x.Big.y }}
  for x in alias {{ sum = sum + x.Big.x + x.Big.y }}
  sum
}}\n"
    )
}

#[test]
fn minimum_width_reservation_now_needs_two_outer_allocations() {
    // The frozen pre-change witness allocated three times (witness-02.log).
    // Static empty routing removes the initial header allocation. The seed is
    // inline and costs no allocation, and both retained aliases stay exact.
    let source = raw_repeat(64);
    let program = compile(&source);
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let ready = crate::rc_flat_builder::ready(&source, lowered);
    let run = hirsel_heapcheck::run(
        &ready,
        &path("run"),
        &[],
        &mut NoOperations,
        &Config {
            check_every_step: true,
            ..Default::default()
        },
    );
    let outcome = run.result.unwrap();
    assert!(outcome.observation.failures.is_empty());
    assert_eq!(
        outcome.observation.returned,
        Some(Observed::Integer {
            scalar: hirsel_lir::layout_types::ScalarKind::U64,
            bits: 448,
        })
    );
    eprintln!("original Packed64Big counters: {:?}", run.counters);
    assert_eq!(run.counters.allocations, 2);
    assert_eq!(run.counters.frees, 2);
    assert_eq!(run.counters.live_heap_bytes, 0);
}

#[test]
fn copied_prefix_uses_walks_without_index_or_slot_and_audits_every_failure() {
    use hirsel_lir::ir::{Inst, ProjectionStep};
    for count in [0, 1, 2, 3, 4, 5, u64::MAX] {
        let source = format!(
            "{TYPES}fn score(xs: List[Packed[V]]) -> u64 {{
  let mut s = list.len(xs)
  let mut i = 0u64
  for x in xs {{
    if i % 2 == 0 {{ s = s + int.widen(x.Small.x) }}
    else {{ s = s + x.Big.x + x.Big.y }}
    i += 1
  }}
  s
}}
fn value() -> u64 {{
  let mut xs: List[Packed[V]] = [Small(x: 7u8), Big(x: 11, y: 13), Small(x: 17u8), Big(x: 19, y: 23)]
  let alias = copy xs
  list.truncate(mut xs, {count})
  score(xs) * 100 + score(alias)
}}
pub fn main() -> Result[(), u64] {{ Error(value()) }}\n"
        );
        let program = compile(&source);
        let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
        let truncate = lowered
            .units
            .iter()
            .find(|u| u.header.definition == hirsel_syntax::DefPath::core("list.truncate"))
            .unwrap();
        assert_eq!(
            truncate
                .instructions
                .iter()
                .filter(|i| matches!(i, Inst::FlatNext { .. }))
                .count(),
            2
        );
        assert!(
            truncate
                .instructions
                .iter()
                .any(|i| matches!(i, Inst::SeqAppend { .. }))
        );
        assert!(
            !truncate
                .instructions
                .iter()
                .any(|i| matches!(i, Inst::SeqSetLen { .. }))
        );
        assert!(
            !truncate
                .pools
                .projections
                .iter()
                .any(|p| matches!(p, ProjectionStep::Index(_) | ProjectionStep::Slot(_)))
        );
        let ready = crate::rc_flat_builder::ready(&source, lowered);
        let actual = crate::rc_flat_builder::heap(&ready).unwrap();
        let retained = count.min(4) as usize;
        let values = [7u64, 24, 17, 42];
        let expected = (values[..retained].iter().sum::<u64>() + retained as u64) * 100 + 94;
        assert_eq!(actual, expected, "count={count}");
    }
}

#[test]
fn heap_rejects_forged_walk_offsets_and_sequential_length_writes() {
    use hirsel_lir::ir::{Inst, ProjectionStep};
    let source = raw_repeat(3);
    let program = compile(&source);
    let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
    let ready = crate::rc_flat_builder::ready(&source, lowered);
    let execute = |p: &hirsel_heapcheck::Program<hirsel_lir::stage::Ready>| {
        hirsel_heapcheck::run(p, &path("run"), &[], &mut NoOperations, &Config::default())
    };
    let accepted = execute(&ready).result.unwrap();
    assert!(accepted.observation.failures.is_empty());
    let mut wrong_offset = ready.clone();
    let mut forged = 0;
    for unit in &mut wrong_offset.units {
        for projection in &mut unit.pools.projections {
            if let ProjectionStep::FlatAt { index, offset } = projection {
                *offset = *index;
                forged += 1;
            }
        }
    }
    assert!(forged > 0);
    let error = execute(&wrong_offset).result.unwrap_err().to_string();
    assert!(
        error.contains("sequential element") && error.contains("bytes past the first"),
        "{error}"
    );
    let mut wrong_extent = ready;
    let reserve = wrong_extent
        .units
        .iter_mut()
        .flat_map(|u| &mut u.instructions)
        .find(|i| matches!(i, Inst::SeqReserve { .. }))
        .unwrap();
    let Inst::SeqReserve {
        address,
        capacity,
        version,
    } = *reserve
    else {
        unreachable!()
    };
    *reserve = Inst::SeqSetLen {
        address,
        len: capacity,
        version,
    };
    let error = execute(&wrong_extent).result.unwrap_err().to_string();
    assert!(
        error.contains("TypeConfusion") && error.contains("sequence write"),
        "{error}"
    );
}
