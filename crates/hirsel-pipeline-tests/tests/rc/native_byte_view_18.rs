//! Nested byte views keep the established source loan through their consumer.
//! Reference shapes: Rust std f6c1107f, core/src/ptr/mod.rs:490–567 and
//! alloctests/tests/vec.rs:257–292 (append contents; empty/nonempty and sharing).
//! No Rust implementation copied: Hirsel append borrows rather than drains its source.
use crate::rc_support as support;
use hirsel_driver::native::*;
use hirsel_engine::*;
use hirsel_lir::ir::{Holding, Inst};

// Byte-for-byte retained core-only-loan-original.hir, including its entry type.
const ORIGINAL: &str = "pub fn main() -> Result[(), u64] {\n  let outer = \"abc\"\n  let mut out: List[u8] = []\n  core@1/bytes.append(mut out, core@1/bytes.slice(core@1/string.as_bytes(outer), 0, core@1/string.byte_len(outer) - 1))\n  Ok(())\n}\n";

const VECTORS: &str = r#"
fn source() -> String {
  match string.from_bytes(bytes.from_list([97u8, 98u8, 99u8])) { Ok(text) => take text, Error(_) => "" }
}
fn append(text: String, mut out: List[u8]) {
  bytes.append(mut out, bytes.slice(string.as_bytes(text), 0, string.byte_len(text)))
}
pub fn main() -> Result[(), u64] {
  let outer = "abc"
  let mut out: List[u8] = []
  bytes.append(mut out, bytes.slice(string.as_bytes(outer), 0, string.byte_len(outer) - 1))
  failure.check(bytes.from_list(copy out) == b"ab", Arithmetic())
  let data = string.as_bytes(outer)
  let end = string.byte_len(outer) - 1
  let prefix = bytes.slice(data, 0, end)
  let mut named: List[u8] = []
  bytes.append(mut named, prefix)
  failure.check(bytes.from_list(copy named) == b"ab", Arithmetic())
  let empty = ""
  bytes.append(mut out, bytes.slice(string.as_bytes(empty), 0, string.byte_len(empty)))
  failure.check(bytes.from_list(copy out) == b"ab", Arithmetic())
  let preserved = copy out
  bytes.append(mut out, bytes.slice(string.as_bytes(outer), 1, 3))
  failure.check(bytes.from_list(copy out) == b"abbc", Arithmetic())
  failure.check(bytes.from_list(copy preserved) == b"ab", Arithmetic())
  failure.check(outer == "abc" && data == b"abc", Arithmetic())
  bytes.append(mut out, bytes.slice(string.as_bytes(source()), 1, 3))
  failure.check(bytes.from_list(copy out) == b"abbcbc", Arithmetic())
  append(source(), mut out)
  failure.check(bytes.from_list(copy out) == b"abbcbcabc", Arithmetic())
  Ok(())
}
"#;

fn failure_source(end: &str) -> String {
    format!(
        r#"resource type Witness {{ stdout: Stdout, data: List[u8] }}
drop(value) {{
  let _ = out.print(value.stdout, if list.len(value.data) == 1 && value.data[0] == 115u8 {{ "unchanged\n" }} else {{ "changed\n" }})
}}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let outer = "abc"
  let mut witness = Witness(stdout: io.stdout(rt)?, data: [115u8])
  bytes.append(mut witness.data, bytes.slice(string.as_bytes(outer), 2, {end}))
  failure.check(false, TextInvariant())
  Ok(())
}}
"#
    )
}

fn assert_observation(name: &str, observation: &Observation, failure: Option<&str>) {
    assert_eq!(
        observation.output,
        if failure.is_some() {
            b"unchanged\n".as_slice()
        } else {
            b""
        },
        "{name}: {observation:?}"
    );
    assert_eq!(
        observation.exit_code,
        failure.is_none().then_some(0),
        "{name}"
    );
    assert_eq!(
        observation
            .failures
            .iter()
            .map(|f| f.rule.as_str())
            .collect::<Vec<_>>(),
        failure.into_iter().collect::<Vec<_>>(),
        "{name}: {observation:?}"
    );
    assert!(!observation.blocked && observation.budget.is_none() && observation.signaled.is_none());
}

#[test]
fn nested_and_named_views_agree_on_all_executors() {
    with_compiler_stack(|| {
        let options = Options {
            validation: Validation::EveryStage,
            ..Options::new(support::target())
        };
        let mut matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Default,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            // --debug's original shipped runtime, alongside the diagnostic twin.
            Configuration {
                options: options.clone(),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            },
        ];
        // Standalone default/debug builds disable scoped specialization.
        // Keep their exact profile beside the specialized harness twins.
        let unspecialized: Vec<_> = matrix
            .iter()
            .cloned()
            .map(|mut configuration| {
                configuration.options.scoped_specialization = false;
                configuration
            })
            .collect();
        matrix.extend(unspecialized);
        let directory = tempfile::tempdir().unwrap();
        let toolchain = Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        let bounds = failure_source("1");
        let arithmetic = failure_source("1u64 / 0u64");
        for (name, source, failure) in [
            ("original", ORIGINAL, None),
            ("vectors", VECTORS, None),
            ("bounds-before-append", bounds.as_str(), Some("SEM-3")),
            ("argument-before-bounds", arithmetic.as_str(), Some("SEM-1")),
        ] {
            let mut engine =
                QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("byte-view.hir", source.into()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_observation(name, &reference, failure);
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let sites = Sites::new(validated.snapshot());
            let ready =
                build(&validated, &options).unwrap_or_else(|error| panic!("{name}: {error:?}"));
            let heap = run_heap(&ready, &entry, &sites).unwrap();
            assert_observation(name, &heap.observation, failure);
            assert_eq!(heap.counters.live_heap_bytes, 0, "{name}");
            let output = directory.path().join(name);
            std::fs::create_dir_all(&output).unwrap();
            let mut executions = 0;
            for (configuration, outcome) in
                differential(&validated, &entry, &reference, &matrix, &toolchain, &output)
            {
                assert_eq!(outcome, Outcome::Agree, "{name}: {configuration}");
                executions += 1;
            }
            assert_eq!(executions, 10, "{name}");
            eprintln!(
                "native_byte_view_18 {name}: reference + H zero live bytes + checked-heap injection + LLVM O0/O2 + Cranelift diagnostic/default"
            );
        }
    });
}

#[test]
fn nested_slice_keeps_borrowed_storage_and_rejects_outside_loan() {
    let (program, lowered) = support::lower_valid(ORIGINAL);
    let unit = support::unit_named(&lowered, "main");
    let (result, value) = unit
        .instructions
        .iter()
        .find_map(|inst| match inst {
            Inst::SliceView { result, value, .. } => Some((*result, *value)),
            _ => None,
        })
        .expect("nested expression uses a borrowed slice, without an owning copy");
    let Holding::Borrowed(loan) = unit.values[result.index()].holding else {
        panic!("slice result must borrow")
    };
    assert_eq!(unit.values[value.index()].holding, Holding::Borrowed(loan));
    assert!(
        !unit
            .instructions
            .iter()
            .any(|inst| matches!(inst, Inst::Copy { .. } | Inst::UniqueCopy { .. }))
    );
    let mut editor = hirsel_rc::insert::Editor::new(unit.clone());
    let (block, index) = editor
        .code
        .iter()
        .enumerate()
        .find_map(|(block, code)| {
            code.iter()
                .position(|inst| matches!(inst, Inst::SliceView { .. }))
                .map(|index| (block, index))
        })
        .unwrap();
    editor.code[block].insert(
        index,
        Inst::EndLoan {
            loan,
            if_active: false,
        },
    );
    let forged = editor.finish();
    let target = support::target();
    let manifest = support::manifest(&program);
    let operations = program.operation_registry();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &operations,
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
    };
    let violations = hirsel_lir::validate::check(&forged, &inputs);
    assert!(
        violations.iter().any(|violation| matches!(
            violation.kind,
            hirsel_lir::validate::ViolationKind::OutsideLoan(_)
        )),
        "{violations:?}"
    );
}

#[test]
fn a_live_source_alias_still_conflicts_with_mutation() {
    let source = r#"pub fn main() -> Result[(), u64] {
  let mut outer = "abc"
  let alias = string.as_bytes(outer)
  let mut out: List[u8] = []
  bytes.append(mut out, bytes.slice(string.as_bytes(outer), 0, 2))
  outer = "changed"
  failure.check(alias == b"abc", Arithmetic())
  Ok(())
}
"#;
    let error = support::try_compile(source).expect_err("live source loan rejects mutation");
    assert!(error.contains("OWN-3"), "{error}");
}
