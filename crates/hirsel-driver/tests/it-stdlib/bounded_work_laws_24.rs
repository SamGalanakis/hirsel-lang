use crate::source_std::load_program_source;
use hirsel_driver::{native, *};
use hirsel_engine::*;
use std::path::Path;

fn load(source: &str) -> LoadedProgram {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
    let mut program = load_program_source(&path, Some(source)).unwrap();
    program
        .check_for_execution()
        .unwrap_or_else(|error| match error {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            error => panic!("{error}"),
        });
    program
}

#[test]
fn bounded_decoders_and_renderer_do_not_allocate_for_the_rejected_suffix() {
    with_compiler_stack(|| {
        for size in [128, 512, 2048] {
            let json = format!(
                r#"pub fn main() -> Result[(), ()] {{
 let mut cursor = json.reader(string.as_bytes({:?}))
 failure.check(json.read_owned_text_bounded(mut cursor, 8) == Error(Limit()), Bounds())
 failure.check(cursor.pos == {}u64, Bounds())
 Ok(())
}}"#,
                format!("\"{}\"", "a".repeat(size)),
                size + 2
            );
            let markdown = format!(
                r#"pub fn main() -> Result[(), ()] {{
 let node = markdown.Node(kind: Text({:?}), children: [])
 failure.check(markdown.render_html_bounded(node, markdown.default_options(), 8) == Error(markdown.OutputLimit(limit: 8)), Bounds())
 Ok(())
}}"#,
                "<".repeat(size)
            );
            let stdin = r#"pub fn main(rt: Io) -> Result[(), GrantError] {
 let stream = io.stdin(rt)?
 failure.check(input.read_line_bounded(stream, 8) == Error(LineTooLong()), Bounds())
 Ok(())
}"#;
            for (name, source) in [
                ("json", json.as_str()),
                ("markdown", markdown.as_str()),
                ("stdin", stdin),
            ] {
                let mut program = load(source);
                let input = program.engine.export_native_input().unwrap();
                let validated = program.engine.import_native_input(input).unwrap();
                let ready = native::build(
                    &validated,
                    &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
                )
                .unwrap();
                let counters = if name == "stdin" {
                    let mut input = Stdin {
                        remaining: size,
                        reads: 0,
                    };
                    let result = hirsel_heapcheck::run_ready(
                        &ready.heap_program(),
                        &program.entry,
                        &[hirsel_heapcheck::Arg::Handle(0)],
                        &mut input,
                        &hirsel_heapcheck::Config {
                            context_handles: vec![(0, "Io".into())],
                            ..Default::default()
                        },
                    );
                    let audit = result.result.unwrap();
                    assert!(audit.observation.failures.is_empty() && !audit.observation.blocked);
                    assert!(audit.observation.returned.is_some());
                    assert_eq!(input.reads, 9, "cap eight plus one overflow probe");
                    result.counters
                } else {
                    let result = native::run_heap(
                        &ready,
                        &program.entry,
                        &native::Sites::new(validated.snapshot()),
                    )
                    .unwrap();
                    assert_eq!(
                        result.observation.exit_code,
                        Some(0),
                        "{name}/{size}: {:?}",
                        result.observation
                    );
                    assert!(result.observation.failures.is_empty());
                    result.counters
                };
                assert_eq!(counters.live_heap_bytes, 0);
                // Eight output bytes plus bounded cursor/sink bookkeeping.
                // Input literals and the provider's bytes are preexisting data.
                assert!(
                    counters.peak_live_heap_bytes <= 512,
                    "{name}/{size}: {:?}",
                    counters
                );
                eprintln!("BOUNDED_ALLOCATION {name}/{size} {:?}", counters);
            }
        }
    });
}

#[test]
fn sparse_toposort_work_grows_linearly_with_vertices_and_edges() {
    with_compiler_stack(|| {
        let mut counts = Vec::new();
        for n in [8, 16, 32] {
            let edges = (1..n)
                .map(|v| format!("toposort.Edge(from: {}, to: {v})", v - 1))
                .collect::<Vec<_>>()
                .join(",");
            let expected = (0..n)
                .map(|v| format!("{v}u64"))
                .collect::<Vec<_>>()
                .join(",");
            let mut program = load(&format!(
                r#"pub fn main() -> Result[(), ()] {{
 failure.check(toposort.order({n}, [{edges}], {n}, {}) == Ok([{expected}]), Bounds())
 Ok(())
}}"#,
                n - 1
            ));
            let run = RunConfiguration {
                entry: program.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let (result, profile) = hirsel_interp::with_step_profile(&program.entry, || {
                run_observed_snapshot(&mut program.engine, &run, Default::default())
            });
            let result = result.unwrap();
            assert!(
                matches!(
                    result.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "{result:?}"
            );
            // Include all callees: a quadratic helper cannot hide outside order.
            let steps: u64 = profile.values().sum();
            assert!(steps > 0);
            counts.push(steps);
        }
        for pair in counts.windows(2) {
            assert!(pair[1] > pair[0]);
            assert!(
                pair[1] * 10 <= pair[0] * 25,
                "quadratic sparse traversal: {counts:?}"
            );
        }
        eprintln!("TOPOSORT_WORK {counts:?}");
    });
}

struct Stdin {
    remaining: usize,
    reads: usize,
}
impl hirsel_heapcheck::Operations for Stdin {
    fn call(
        &mut self,
        op: &hirsel_core_contract::OperationId,
        args: &[hirsel_heapcheck::OpValue],
        _: &mut Vec<u8>,
    ) -> hirsel_heapcheck::OpOutcome {
        use hirsel_heapcheck::{OpOutcome as O, OpValue as V};
        let ok = |value| V::Variant {
            tag: 0,
            value: Box::new(value),
        };
        let path = &op.definition.0;
        match (
            path.modules.first().map(String::as_str),
            path.name.as_str(),
            args,
        ) {
            (Some("io"), "stdin", [V::Handle(0)]) => O::Value(ok(V::Handle(1))),
            (
                Some("stdin"),
                "read",
                [
                    V::Handle(1),
                    V::Elements {
                        scalar: hirsel_lir::layout_types::ScalarKind::U8,
                        bits: buffer,
                    },
                    V::Bits { bits: max, .. },
                ],
            ) => {
                assert_eq!(*max, 1, "bounded reader's overflow probe");
                let count = usize::from(self.remaining != 0);
                self.remaining -= count;
                self.reads += 1;
                let mut bytes = buffer.clone();
                bytes.extend(std::iter::repeat_n(u128::from(b'a'), count));
                O::Updated {
                    value: ok(V::Bits {
                        scalar: hirsel_lir::layout_types::ScalarKind::U64,
                        bits: count as u128,
                    }),
                    arguments: vec![(
                        1,
                        V::Elements {
                            scalar: hirsel_lir::layout_types::ScalarKind::U8,
                            bits: bytes,
                        },
                    )],
                }
            }
            _ => panic!("unexpected operation {op:?}: {args:?}"),
        }
    }
}
