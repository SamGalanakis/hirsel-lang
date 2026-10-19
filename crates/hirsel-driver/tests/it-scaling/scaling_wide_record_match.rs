use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_intern::work::{self, Counter};

/// A loop over a match whose N arms each assign a different field of one
/// N-field record through `?` (GAP 5). Type rows copied and lowered, V's flow
/// steps and ownership joins grow linearly with N, rather than with its
/// square or cube.
fn wide_record_match(fields: usize) -> String {
    let declared: String = (1..=fields).map(|k| format!("f{k}: u64, ")).collect();
    let initial: String = (1..=fields).map(|k| format!("f{k}: 0, ")).collect();
    let arms: String = (0..fields)
        .map(|k| format!("{k} => {{ r.f{} = next(mut pos)? }},\n", k + 1))
        .collect();
    format!(
        "pub type R {{ {declared} }}
fn next(mut pos: u64) -> Result[u64, String] uses {{}} {{
  pos += 1
  if pos > 1000 {{ return Error(\"end\") }}
  Ok(pos)
}}
fn fill(count: u64) -> Result[R, String] uses {{}} {{
  let mut r = R({initial})
  let mut pos: u64 = 0
  while pos < count {{
    match pos % {fields} {{
{arms}      _ => (),
    }}
  }}
  Ok(take r)
}}"
    )
}

#[test]
fn wide_record_match_arms_check_in_linear_work() {
    const COUNTERS: [Counter; 3] = [
        Counter::CopiedTypeRows,
        Counter::ValidateFlowSteps,
        Counter::OwnershipValueJoins,
    ];
    with_compiler_stack(|| {
        let measure = |fields: usize| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("wide-record-match.hir", wide_record_match(fields))
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            let fill = &parsed
                .value
                .items
                .iter()
                .map(|syntax| &syntax.item)
                .find(|item| item.definition.0.name == "fill")
                .unwrap()
                .definition;
            let copied = work::read(Counter::ValidateCopiedRows);
            let before = COUNTERS.map(work::read);
            let output = engine.lower_to_tir(fill).unwrap();
            assert!(output.diagnostics.is_empty());
            let after = COUNTERS.map(work::read);
            let mut work = [0; 3];
            for i in 0..3 {
                work[i] = after[i] - before[i];
            }
            (
                work,
                output.value.types.len() as u64,
                work::read(Counter::ValidateCopiedRows) - copied,
            )
        };
        let (small_work, small_types, small_copied) = measure(40);
        let (large_work, large_types, large_copied) = measure(120);
        eprintln!(
            "40 fields: {small_work:?} {small_types} {small_copied}; \
             120 fields: {large_work:?} {large_types} {large_copied}"
        );
        // Three times the arms: linear work triples; quadratic work is nine times.
        for (i, counter) in COUNTERS.iter().enumerate() {
            assert!(
                large_work[i] <= 4 * small_work[i] + 4096,
                "{counter:?}: 40 fields did {} and 120 fields did {}",
                small_work[i],
                large_work[i]
            );
        }
        assert!(
            large_types <= 4 * small_types,
            "40 fields lowered {small_types} type rows and 120 fields {large_types}"
        );
        // V's flow-state copies stay well below quadratic: an ended local's
        // origins are forgotten and partitions keep only typed paths.
        assert!(
            large_copied <= 6 * small_copied,
            "V copied {small_copied} rows for 40 fields and {large_copied} for 120"
        );
    });
}
