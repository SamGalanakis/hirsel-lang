//! Step counts expose the quadratic substring scan that output-only vectors
//! cannot distinguish. Inputs double together, so a nested scan grows by four.
use crate::source_std::tir;
use hirsel_engine::*;
use hirsel_interp::{Limits, Outcome, Value, library::Library};
use std::path::PathBuf;

#[test]
fn two_way_adversarial_search_scales_linearly() {
    with_compiler_stack(|| {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let source = r#"
@noalloc
pub fn search(source: String, needle: String) -> bool uses {} {
  std@1/string.contains(source, needle)
}
pub fn main() -> () { let _ = search("a", "b") }
"#;
        let prepared = tir(&fixture, Some(source), Some("search")).unwrap();
        let runtime = &prepared.runtime;
        let sources = &prepared.sources;
        let configuration = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        // Long period: a nearly matching prefix, mismatch at the last byte.
        // Left mismatch: the right half matches, then the left half differs.
        // Both ordinary a/b and the fingerprint collision A/0x01 must work.
        for (hay_byte, other_byte, prefix_mismatch) in
            [(b'a', b'b', false), (b'a', b'b', true), (b'A', 1, false)]
        {
            let mut counts = Vec::new();
            for size in [256, 512, 1024, 2048] {
                let haystack = String::from_utf8(vec![hay_byte; size * 2]).unwrap();
                let mut needle = vec![hay_byte; size];
                needle[if prefix_mismatch { 0 } else { size - 1 }] = other_byte;
                let needle = String::from_utf8(needle).unwrap();
                let mut library = Library::new(&configuration, Default::default()).unwrap();
                let execution = hirsel_interp::run_with_runtime(
                    &mut prepared.provider(),
                    sources,
                    &configuration,
                    vec![Value::String(haystack), Value::String(needle)],
                    Limits {
                        steps: Some(10_000_000),
                        ..Default::default()
                    },
                    &mut library,
                    runtime,
                )
                .unwrap();
                assert!(
                    matches!(
                        execution.outcome,
                        Outcome::Returned {
                            value: Value::Bool(false),
                            ..
                        }
                    ),
                    "size {size}: {:?}",
                    execution.outcome
                );
                counts.push(execution.steps);
            }
            for pair in counts.windows(2) {
                assert!(
                    pair[1] < pair[0] * 3,
                    "doubling input must stay below quadratic growth: {counts:?}"
                );
            }
        }
    });
}
