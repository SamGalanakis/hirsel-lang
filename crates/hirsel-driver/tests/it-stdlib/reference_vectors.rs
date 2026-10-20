//! Shared, bounded-source batches of ordinary Hirsel assertions, without a step budget.
use hirsel_driver::*;
use hirsel_engine::*;
use std::{fmt::Write, path::Path};

pub struct Case {
    pub id: String,
    /// Hirsel statements in a function with `t: Test` in scope.
    pub body: String,
}

/// A Bytes literal from an upstream hexadecimal field; reject malformed data on the host.
pub fn bytes(hex: &str) -> String {
    assert!(hex.len().is_multiple_of(2));
    let mut literal = String::from("b\"");
    for pair in hex.as_bytes().chunks_exact(2) {
        let pair = std::str::from_utf8(pair).unwrap();
        let byte = u8::from_str_radix(pair, 16).unwrap();
        write!(literal, "\\x{byte:02x}").unwrap();
    }
    literal.push('"');
    literal
}

/// Partition by ordinal (not tcId) so noncontiguous upstream IDs cannot leave holes.
/// Each Rust test selects one shard; each batch is a separate checked program.
/// `preamble` can include an internal module verbatim plus a test-only adapter.
pub fn run(
    suite: &str,
    cases: &[Case],
    shard: usize,
    shards: usize,
    batch_size: usize,
    preamble: &str,
) {
    assert!(shards > 0 && shard < shards && batch_size > 0);
    let selected: Vec<_> = cases.iter().skip(shard).step_by(shards).collect();
    assert!(!selected.is_empty(), "{suite}: empty shard {shard}");
    with_compiler_stack(|| {
        for batch in selected.chunks(batch_size) {
            let mut source = String::from(preamble);
            for (i, case) in batch.iter().enumerate() {
                writeln!(
                    source,
                    "\n// {}\nfn vector_{i}(t: Test) -> () {{\n{}\n}}",
                    case.id, case.body
                )
                .unwrap();
            }
            source.push_str("\npub fn main(t: Test) -> Result[(), ()] {\n");
            for i in 0..batch.len() {
                writeln!(source, "vector_{i}(t)").unwrap();
            }
            source.push_str("Ok(())\n}\n");
            let ids: Vec<_> = batch.iter().map(|case| case.id.as_str()).collect();
            let fixture =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
            let mut loaded = load_program_source(&fixture, Some(&source))
                .unwrap_or_else(|error| panic!("{suite} {ids:?}: {error:?}"));
            loaded
                .check_for_execution()
                .unwrap_or_else(|error| panic!("{suite} {ids:?}: {error:?}\n{source}"));
            loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
                limits: hirsel_interp::Limits {
                    steps: None,
                    call_depth: 256,
                },
                ..Default::default()
            }));
            let run = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry,
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap_or_else(|error| panic!("{suite} {ids:?}: {error:?}"));
            assert!(
                matches!(
                    run.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{suite} {ids:?}: {:?}\n{source}",
                run.outcome
            );
        }
    });
}
