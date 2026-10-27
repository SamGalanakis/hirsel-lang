//! Native integration: source programs through
//! every native stage validator, then the heap executor and linked
//! executables, each compared with the reference interpreter's observation.
//!
//! The tests marked `ignore` link for AArch64 and run under qemu or run many
//! configurations; `tools/check-native.sh` runs them.
use crate::source_std;
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Configuration, Engine, Observation, Outcome, Partition, StageName, TargetSpec, Toolchain,
    differential, full_matrix, smoke_matrix, topology_matrix,
};
use hirsel_engine::{
    DefPath, DefinitionQueries, QueryEngine, RunConfiguration, SimulationConfig,
    ValidatedNativeInput, check_progress, default_context, default_environment, entry_path,
    run_observed_snapshot, with_compiler_stack,
};
use std::path::PathBuf;

/// A checked program with its frozen snapshot and reference observation.
struct Checked {
    validated: ValidatedNativeInput,
    entry: DefPath,
    reference: Observation,
}

fn checked_in(engine: &mut QueryEngine, source: &str) -> Checked {
    let context = default_context();
    if source.contains("std@1/") {
        let mut loaded = hirsel_driver::load_program_source(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/std_numeric.hir"),
            Some(source),
        )
        .unwrap();
        loaded.check_for_execution().unwrap();
        *engine = loaded.engine;
    } else {
        let file = engine.sources.add("native.hir", source.into()).unwrap();
        let output = check_progress(engine, file, &context).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    }
    checked_engine(engine)
}

fn checked_engine(engine: &mut QueryEngine) -> Checked {
    let entry = entry_path(&default_context());
    let config = RunConfiguration {
        entry: entry.clone(),
        hash_seed: 0,
        inputs: vec![],
    };
    let execution = run_observed_snapshot(engine, &config, SimulationConfig::default()).unwrap();
    let reference = Observation::of_reference(execution.observation.as_ref().unwrap());
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    Checked {
        validated,
        entry,
        reference,
    }
}

fn checked(source: &str) -> Checked {
    let mut engine = if source.contains("std@1/") {
        source_std::engine()
    } else {
        QueryEngine::new(default_environment(), compiler_adapters())
    };
    checked_in(&mut engine, source)
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runtime archives, built once per test binary.
fn toolchain(configurations: &[Configuration]) -> Toolchain {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-runtime");
    Toolchain::for_matrix(&dir, configurations).unwrap()
}

/// Every configuration agrees with the reference; returns the reference.
fn agree(name: &str, source: &str, configurations: &[Configuration]) -> Observation {
    let name = name.to_string();
    let configurations = configurations.to_vec();
    with_compiler_stack(move || {
        let program = checked(&source);
        let toolchain = toolchain(&configurations);
        let outcomes = differential(
            &program.validated,
            &program.entry,
            &program.reference,
            &configurations,
            &toolchain,
            &scratch(&name),
        );
        for (configuration, outcome) in outcomes {
            match outcome {
                Outcome::Agree => {}
                Outcome::Mismatch(actual) => panic!(
                    "{name} under {configuration}:\n{}\nreference:\n{}",
                    actual.render(),
                    program.reference.render()
                ),
                Outcome::Unavailable(error) => panic!("{name} under {configuration}: {error}"),
            }
        }
        program.reference
    })
}

/// Both emitters preserve linked initialized-field metadata, including record
/// padding and variant arms, in the actual object data consumed by the runtime.
#[test]
fn native_glue_preserves_linked_value_metadata_on_every_target() {
    use hirsel_core_contract::handle_scan::{VALUE, VALUE_FIELDS, VALUE_VARIANT, WORDS};
    use hirsel_driver::native::{self, Backend, Options};
    use object::{Object, ObjectSection, ObjectSymbol};
    with_compiler_stack(|| {
        let source = r#"type P { a: bool, b: u64 }
fn keep(x: Result[P, ()]) -> Result[P, ()] { x }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let x = keep(Ok(P(a: true, b: 42u64)))
  match x {
    Ok(p) => { let _ = p.b
      () }
    Error(e) => { () }
  }
  Ok(())
}
"#;
        let checked = checked(source);
        for target in TargetSpec::executed() {
            let ready = native::build(&checked.validated, &Options::new(target)).unwrap();
            let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &ready.table,
                &ready.finalizers,
            );
            let descriptors =
                hirsel_lir::rt_program::descriptors(&ready.table, &registry.layouts).unwrap();
            let mut kinds = std::collections::BTreeSet::new();
            for backend in [Backend::Llvm, Backend::Cranelift] {
                let emitted = native::emit_glue(&ready, backend, &registry).unwrap();
                let object = object::File::parse(emitted.bytes.as_slice()).unwrap();
                for descriptor in &descriptors {
                    let words = descriptor.scan.linked(|id| registry.index(id)).unwrap();
                    for op in words.chunks_exact(WORDS).filter(|op| op[0] == VALUE) {
                        kinds.insert(op[1]);
                    }
                    let name =
                        hirsel_lir::glue::descriptor_name(&ready.keys.key(descriptor.layout))
                            .replace("hirsel.layout.", "hirsel.scan.");
                    let symbol = object
                        .symbol_by_name(&name)
                        .unwrap_or_else(|| panic!("{backend:?}: missing {name}"));
                    let section = object
                        .section_by_index(symbol.section_index().unwrap())
                        .unwrap();
                    let actual = section
                        .data_range(symbol.address(), words.len() as u64 * 8)
                        .unwrap()
                        .unwrap();
                    let expected: Vec<_> = words.into_iter().flat_map(u64::to_le_bytes).collect();
                    assert_eq!(actual, expected, "{backend:?}: {name}");
                }
            }
            assert!(kinds.contains(&VALUE_FIELDS));
            assert!(kinds.contains(&VALUE_VARIANT));
        }
    });
}

/// A launch that hangs is killed at the toolchain's timeout, and that
/// configuration alone fails, by name, as a launch error.
#[test]
fn a_hanging_launch_fails_its_configuration_by_name() {
    with_compiler_stack(|| {
        let program = checked(HELLO);
        let matrix = smoke_matrix();
        let mut toolchain = toolchain(&matrix);
        // Every x86-64 executable runs under the wrapper, which sleeps instead.
        toolchain.wrapper = ["/bin/sh", "-c", "sleep 30", "sh"]
            .map(String::from)
            .to_vec();
        toolchain.launch_timeout = std::time::Duration::from_millis(300);
        let outcomes = differential(
            &program.validated,
            &program.entry,
            &program.reference,
            &matrix,
            &toolchain,
            &scratch("hanging-launch"),
        );
        let names: Vec<_> = matrix.iter().map(Configuration::name).collect();
        assert_eq!(
            outcomes.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            names.iter().collect::<Vec<_>>()
        );
        for (configuration, (name, outcome)) in matrix.iter().zip(&outcomes) {
            match (configuration.engine, outcome) {
                (Engine::Heap { .. }, Outcome::Agree) => {}
                (Engine::Native { .. }, Outcome::Unavailable(error)) => {
                    assert_eq!(error.stage, StageName::Launch, "{name}: {error}");
                    assert!(error.message.contains("launch timeout"), "{name}: {error}");
                }
                _ => panic!("{name}: {outcome:?}"),
            }
        }
    });
}

/// Output after the last newline reaches the executable's stdout (found by
/// the agree campaign: the launcher returned without flushing it).
const NO_NEWLINE: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let s = io.stdout(rt)?\n  let _ = out.print(s, \"b\\n\")\n  let _ = out.print(s, \"a\")\n  Ok(())\n}\n";

const HELLO: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"hi\\n\")\n  Ok(())\n}\n";

/// Recursion, a counted box, a match on a call's Result, `?` and an Error exit.
const CONTROL: &str = "type P { a: u64, b: box u64 }\nfn fib(n: u64) -> u64 {\n  if n < 2u64 { n } else { fib(n - 1u64) + fib(n - 2u64) }\n}\nfn half(x: u64) -> Result[u64, ()] {\n  if x % 2u64 == 0u64 { Ok(x / 2u64) } else { Error(()) }\n}\npub fn main(rt: Io, args: Args) -> Result[(), ()] {\n  let s = io.stdout(rt)? grant_error\n  let p = P(a: fib(10u64), b: box 7u64)\n  let _ = out.print(s, if p.a == 55u64 { \"fib ok\\n\" } else { \"fib bad\\n\" })\n  match half(p.a + 1u64) {\n    Ok(v) => { let _ = out.print(s, if v == 28u64 { \"half ok\\n\" } else { \"half bad\\n\" }) }\n    Error(e) => { let _ = out.print(s, \"half error\\n\") }\n  }\n  let w = half(p.a)?\n  let _ = out.print(s, \"unreachable\\n\")\n  Ok(())\n}\n\n@noalloc\nfn grant_error(refusal: GrantError) -> () uses {} { () }\n";

/// A resource whose finalizer fails after output: FAIL-3 cleanup with the
/// failure's rule and site.
const FINALIZER: &str = "resource type R { n: u64 }\ndrop(x) {\n  let _ = 10u64 / x.n\n  ()\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let s = io.stdout(rt)?\n  let r = R(n: 0)\n  let _ = out.print(s, \"before drop\\n\")\n  Ok(())\n}\n";

/// `float.sum_exact` from source to native: the exact sum is 1, the naive 0.
const EXACT_SUM: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: Array[f64] = [1e30f64, 1.0f64, -1e30f64]\n  let exact = std@1/f64.sum_exact(xs)\n  let _ = out.print(io.stdout(rt)?, if exact == 1.0f64 { \"exact\\n\" } else { \"inexact\\n\" })\n  Ok(())\n}\n";

/// Two instances of a generic function, a size parameter, a closure with a
/// capture and a self-tail loop: several units per program, so partitions
/// really divide it.
const GENERIC: &str = "fn keep[T: copyable](take a: T) -> T {\n  a\n}\nfn total[n: size](xs: Inline[u64, n], i: u64, acc: u64) -> u64 {\n  if i == n { acc } else { total(xs, i + 1u64, acc + xs[i]) }\n}\nfn apply(f: fn(x: u64) -> u64, x: u64) -> u64 {\n  f(x)\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let s = io.stdout(rt)?\n  let forty: u64 = 40u64\n  let base: u64 = keep(take forty)\n  let yes: bool = true\n  let flag: bool = keep(take yes)\n  let xs: Inline[u64, 3] = [1u64, 2u64, 3u64]\n  let sum = total(xs, 0u64, 0u64)\n  let add = fn(x: u64) -> u64 { x + base }\n  let got = apply(add, sum)\n  let _ = out.print(s, if flag && got == 46u64 { \"generic ok\\n\" } else { \"generic bad\\n\" })\n  Ok(())\n}\n";

/// A region-placed list read through its slot: the slot holds a pointer at
/// the payload, not at a header (found by the corpus matrix: natively the
/// count was read past the payload and every valid index failed MEM-14).
const LIST_READ: &str = "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: List[u8] = [7u8, 9u8]\n  let i: u64 = 1\n  let _ = out.print(io.stdout(rt)?, if xs[i] == 9u8 { \"nine\\n\" } else { \"bad\\n\" })\n  Ok(())\n}\n";

const SMOKE: &[(&str, &str)] = &[
    ("hello", HELLO),
    ("control", CONTROL),
    ("finalizer", FINALIZER),
    ("generic", GENERIC),
    ("no-newline", NO_NEWLINE),
];

#[test]
fn smoke_programs_agree_on_the_heap_executor_and_as_x86_64_executables() {
    let matrix = smoke_matrix();
    assert_eq!(agree("smoke-hello", HELLO, &matrix).output, b"hi\n");
    let control = agree("smoke-control", CONTROL, &matrix);
    assert_eq!(control.output, b"fib ok\nhalf ok\n");
    assert_eq!(control.exit_code, Some(1));
    let finalizer = agree("smoke-finalizer", FINALIZER, &matrix);
    assert_eq!(finalizer.exit_code, None);
    assert_eq!(finalizer.failures.len(), 1);
    assert_eq!(
        (
            finalizer.failures[0].rule.as_str(),
            finalizer.failures[0].line
        ),
        ("SEM-1", 3)
    );
    assert_eq!(
        agree("smoke-exact-sum", EXACT_SUM, &matrix).output,
        b"exact\n"
    );
    assert_eq!(
        agree("smoke-generic", GENERIC, &matrix).output,
        b"generic ok\n"
    );
    assert_eq!(
        agree("smoke-list-read", LIST_READ, &matrix).output,
        b"nine\n"
    );
}

/// `fn serve() -> Never { ..; serve() }` is the canonical server loop: its
/// diverging self-tail site is a loop, so a million restarts reuse one frame
/// (recursion would need well over the 8 MiB main-thread stack) and the only
/// exit is the failure edge.
const SERVE: &str = "fn serve(n: u64) -> Never {\n  let _ = 10u64 / n\n  serve(n - 1u64)\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let _ = out.print(io.stdout(rt)?, \"serving\\n\")\n  serve(1000000u64)\n}\n";

/// Native only: the reference interpreter's step limit is below a million
/// restarts, so this control checks the executable's own observation.
#[test]
fn a_diverging_self_tail_call_loops_a_million_times_within_the_stack() {
    use hirsel_driver::native::{Sites, launch, observe};
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("serve.hir", SERVE.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let entry = entry_path(&default_context());
        let target = x86();
        let program = build(&validated, &Options::new(target.clone())).unwrap();
        let tools = toolchain(&smoke_matrix());
        for (name, opt) in [("O0", OptLevel::None), ("O3", OptLevel::Speed)] {
            let dir = scratch(&format!("diverging-self-tail-{name}"));
            let exe = link(&program, &entry, opt, Partition::Whole, &tools, &dir).unwrap();
            let launched = launch(&exe, &target, &tools, &dir, false).unwrap();
            let serve = observe(&launched, &program, &Sites::new(validated.snapshot()))
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(serve.output, b"serving\n", "{name}");
            assert_eq!(serve.exit_code, None, "{name}");
            assert_eq!(
                serve
                    .failures
                    .iter()
                    .map(|f| (f.rule.as_str(), f.line))
                    .collect::<Vec<_>>(),
                [("SEM-1", 2)],
                "{name}"
            );
        }
    });
}

#[test]
#[ignore]
fn smoke_programs_agree_under_the_full_matrix_on_both_targets() {
    let matrix = full_matrix();
    for (name, source) in SMOKE {
        agree(&format!("full-{name}"), source, &matrix);
    }
    agree("full-exact-sum", EXACT_SUM, &matrix);
}

#[test]
#[ignore]
fn singleton_whole_and_twenty_random_partitions_observe_the_same() {
    let matrix = topology_matrix(20);
    assert_eq!(matrix.len(), 22);
    for (name, source) in SMOKE {
        agree(&format!("topology-{name}"), source, &matrix);
    }
    agree("topology-exact-sum", EXACT_SUM, &matrix);
    // The random partitions really differ from each other and from the two fixed ones.
    let assignments: std::collections::BTreeSet<Vec<usize>> = (1..=20)
        .map(|seed| Partition::Random(seed).assign(6))
        .collect();
    assert!(assignments.len() > 10, "{assignments:?}");
}

use hirsel_driver::native::{OptLevel, Options, build, build_program, emit, link};

fn x86() -> TargetSpec {
    TargetSpec::x86_64_unknown_linux_gnu()
}

/// Every stage's output is sealed by that stage's validator: a Ready program
/// whose manifest gives its operations no native disposition is refused by
/// the Ready validator, not by a pass, and nothing is emitted.
#[test]
fn the_ready_validator_refuses_what_a_pass_let_through() {
    with_compiler_stack(|| {
        let program = checked(HELLO);
        let adapted = hirsel_driver::native::program(&program.validated);
        let skeleton =
            hirsel_core_contract::Manifest::skeleton(adapted.operations.values().cloned());
        let Err(error) = build_program(&adapted, skeleton, &Options::new(x86())) else {
            panic!("an unclassified operation reached Ready")
        };
        assert_eq!(error.stage, StageName::Ready);
        assert!(error.validator, "{error}");
        assert!(error.message.contains("UnsupportedOperation"), "{error}");
        // Its accepted twin: the snapshot's own manifest.
        assert!(build(&program.validated, &Options::new(x86())).is_ok());
    });
}

/// A program native code cannot run yet is a tool outcome naming its stage;
/// the source verdict and the reference observation are untouched.
#[test]
fn a_native_gap_is_a_tool_outcome_not_a_source_verdict() {
    with_compiler_stack(|| {
        let grant = checked(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let net = io.net(rt)?; Ok(()) }",
        );
        assert_eq!(grant.reference.exit_code, Some(0));
        assert!(build(&grant.validated, &Options::new(x86())).is_ok());
        // Resource mutexes have native support. Keep this former gap as an
        // accepted control, and make the unavailable disposition explicit so
        // this boundary test does not depend on an unimplemented feature.
        let mutex = checked(
            "resource type Token { n: u8 }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; let store: Mutex[Token] = mutex.create(tasks, Token(n: 1u8)); Ok(()) }",
        );
        assert_eq!(mutex.reference.exit_code, Some(0));
        assert!(build(&mutex.validated, &Options::new(x86())).is_ok());
        let adapted = hirsel_driver::native::program(&grant.validated);
        let mut manifest = hirsel_driver::native::manifest(grant.validated.snapshot()).unwrap();
        let mut unavailable = manifest
            .rows()
            .find(|row| row.id.definition == DefPath::core("io.net"))
            .expect("the granted operation")
            .clone();
        unavailable.dispositions.native = hirsel_core_contract::Disposition::Unavailable(
            hirsel_core_contract::Unavailable::NotImplemented,
        );
        manifest.replace(unavailable).unwrap();
        let Err(error) = build_program(&adapted, manifest, &Options::new(x86())) else {
            panic!("an explicitly unavailable operation reached Ready")
        };
        assert_eq!(error.stage, StageName::Ready, "{error}");
        assert!(error.validator, "{error}");
        assert!(error.message.contains("UnsupportedOperation"), "{error}");
        assert!(error.message.contains("io.net"), "{error}");
        assert_eq!(grant.reference.exit_code, Some(0));
        let accepted = checked(
            r#"pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let directory = io.dir(rt, path.of("."))
  let text = std@1/u64.to_string(4u64)
  Ok(())
}
"#,
        );
        assert_eq!(accepted.reference.exit_code, Some(0));
        assert!(build(&accepted.validated, &Options::new(x86())).is_ok());
    });
}

/// Every runtime primitive names an ABI export. Each export has a registered
/// primitive or an explicit compatibility contract, and emitted calls match its
/// physical signature on both targets.
#[test]
fn every_runtime_primitive_row_matches_its_export() {
    use hirsel_core_contract::{Disposition, Engine as ManifestEngine};
    use hirsel_lir::layout_types::ScalarKind;
    use hirsel_rt_operations::{AbiExport, HIRSEL_ABI_EXPORTS, Physical};
    with_compiler_stack(|| {
        let program = checked(HELLO);
        let manifest = hirsel_driver::native::manifest(program.validated.snapshot()).unwrap();
        let inventory = hirsel_engine::registry::BuiltinRegistry::new().operation_inventory();
        manifest
            .check_coverage(
                inventory
                    .into_keys()
                    .map(|key| hirsel_core_contract::OperationId {
                        definition: key.definition,
                        declaration_digest: key.declaration_digest,
                    }),
            )
            .unwrap();
        let mut exports = std::collections::BTreeSet::new();
        for export in HIRSEL_ABI_EXPORTS {
            assert!(
                exports.insert(export.symbol),
                "duplicate ABI export: {}",
                export.symbol
            );
        }
        let mut used_exports = std::collections::BTreeSet::new();
        for row in manifest.rows() {
            let classified = hirsel_rt_operations::classify(row.clone()).unwrap();
            if matches!(
                classified.dispositions.native,
                Disposition::RuntimePrimitive(_)
            ) || matches!(row.dispositions.native, Disposition::RuntimePrimitive(_))
            {
                assert_eq!(
                    row.dispositions.native, classified.dispositions.native,
                    "{:?}",
                    row.id
                );
            }
            let Disposition::RuntimePrimitive(template) =
                row.dispositions.get(ManifestEngine::Native)
            else {
                continue;
            };
            let symbols: Vec<String> = if template.contains('{') {
                [ScalarKind::F32, ScalarKind::F64]
                    .into_iter()
                    .filter_map(|k| hirsel_lir::abi::resolve_symbol(template, &[], Some(k)))
                    .collect()
            } else {
                vec![template.clone()]
            };
            assert!(!symbols.is_empty(), "{template} resolves to no symbol");
            for symbol in symbols {
                assert!(
                    exports.contains(symbol.as_str()),
                    "{symbol} has no Hirsel ABI export row"
                );
                used_exports.insert(symbol);
            }
        }
        // aebd8629 retained the old clock entry for ABI compatibility while
        // generated calls moved to the export carrying their source site.
        let legacy_sleep = AbiExport {
            caller_site: false,
            symbol: "hirsel_rt_v2_clock_sleep_until",
            destination: true,
            result: Some((12, 4)),
            parameters: &[Physical::Pointer, Physical::Pointer],
        };
        assert_eq!(
            HIRSEL_ABI_EXPORTS
                .iter()
                .find(|export| export.symbol == legacy_sleep.symbol),
            Some(&legacy_sleep)
        );
        assert!(used_exports.contains("hirsel_rt_v2_clock_sleep_until_at_site"));
        assert!(used_exports.insert(legacy_sleep.symbol.into()));
        // Output calls now carry checked source codes; the original physical
        // export remains an explicit compatibility contract as well.
        let legacy_print = AbiExport {
            caller_site: false,
            symbol: "hirsel_rt_v2_out_print",
            destination: true,
            result: Some((12, 4)),
            parameters: &[
                Physical::Pointer,
                Physical::Pointer,
                Physical::I64,
                Physical::I64,
            ],
        };
        assert_eq!(
            HIRSEL_ABI_EXPORTS
                .iter()
                .find(|export| export.symbol == legacy_print.symbol),
            Some(&legacy_print)
        );
        assert!(used_exports.contains("hirsel_rt_v2_out_print_at_site"));
        assert!(used_exports.insert(legacy_print.symbol.into()));
        // Source-aware process calls use the _at_site exports. The original
        // public entries retain these physical compatibility contracts, derived
        // from process.rs: ChildWordResult, ChildPairResult and GroupResult.
        for legacy in [
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_proc_spawn",
                destination: true,
                result: Some((16, 8)),
                parameters: &[Physical::Pointer, Physical::Pointer],
            },
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_child_write",
                destination: true,
                result: Some((12, 4)),
                parameters: &[
                    Physical::Pointer,
                    Physical::Pointer,
                    Physical::Pointer,
                    Physical::I64,
                    Physical::I64,
                ],
            },
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_child_close_stdin",
                destination: false,
                result: None,
                parameters: &[Physical::Pointer, Physical::Pointer],
            },
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_child_wait",
                destination: true,
                result: Some((12, 4)),
                parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
            },
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_proc_spawn_pipeline",
                destination: true,
                result: Some((16, 8)),
                parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
            },
            AbiExport {
                caller_site: false,
                symbol: "hirsel_rt_v2_proc_wait_group",
                destination: true,
                result: Some((24, 8)),
                parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
            },
        ] {
            assert_eq!(
                HIRSEL_ABI_EXPORTS
                    .iter()
                    .find(|export| export.symbol == legacy.symbol),
                Some(&legacy)
            );
            assert!(used_exports.contains(&format!("{}_at_site", legacy.symbol)));
            assert!(used_exports.insert(legacy.symbol.into()));
        }
        assert_eq!(
            used_exports
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            exports,
            "every Hirsel ABI export has a registered primitive or compatibility contract"
        );
        // Calls of each, with each float family, on both targets.
        let calls = checked(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: Array[f64] = [1.0f64, 2.0f64]\n  let ys: Array[f32] = [1.0f32, 2.0f32]\n  let a = std@1/f64.sum(xs) + std@1/f64.sum_exact(xs)\n  let b = std@1/f32.sum(ys) + std@1/f32.sum_exact(ys)\n  let text = std@1/u64.to_string(4u64)\n  let decoded = string.from_bytes(b\"abc\")\n  let replaced = std@1/bytes.replace(b\"abc\", b\"b\", b\"x\")\n  for pos in string.positions(text) {\n    let part = string.slice(text, pos, pos)\n  }\n  let c = io.monotonic_clock(rt)?\n  match clock.monotonic(c) { Ok(now) => { let _ = clock.sleep_until(c, now) }, Error(_) => () }\n  let _ = out.print(io.stdout(rt)?, \"x\")\n  Ok(())\n}\n",
        );
        for target in [x86(), TargetSpec::aarch64_unknown_linux_gnu()] {
            let ready = build(&calls.validated, &Options::new(target)).unwrap();
            hirsel_driver::native::check_runtime_calls(&ready).unwrap();
        }
    });
}

/// A same-signature body edit rebuilt in a live engine gives the units and the
/// object a fresh engine gives, and the edit recomputes no other definition's
/// source checks (PROG-5's cutoff, by `recompute_count`).
#[test]
fn a_same_signature_body_edit_matches_a_fresh_build() {
    use hirsel_engine::QueryKind;
    let source = |body: &str| {
        format!(
            "fn weight(n: u64) -> u64 {{\n  {body}\n}}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\n  let _ = out.print(io.stdout(rt)?, if weight(3u64) == 6u64 {{ \"six\\n\" }} else {{ \"other\\n\" }})\n  Ok(())\n}}\n"
        )
    };
    with_compiler_stack(move || {
        let (before, after) = (source("n + n"), source("n * 2u64 + 1u64"));
        let digests = |program: &Checked| {
            let ready = build(&program.validated, &Options::new(x86())).unwrap();
            let units: Vec<_> = ready.units.iter().map(|u| u.unit().digest()).collect();
            let object = emit(&ready, OptLevel::Speed, |_| true).unwrap();
            (units, object.digest())
        };
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let first = checked_in(&mut engine, &before);
        assert_eq!(first.reference.output, b"six\n");
        let old = digests(&first);
        let main = first.entry.clone();
        let count = |engine: &QueryEngine| {
            [QueryKind::Check, QueryKind::Modes, QueryKind::Lower]
                .map(|kind| engine.recompute_count(&main, kind))
        };
        let main_before = count(&engine);
        let edited = checked_in(&mut engine, &after);
        assert_eq!(count(&engine), main_before, "the edit rechecked main");
        assert_eq!(edited.reference.output, b"other\n");
        let fresh = checked(&after);
        assert_eq!(edited.reference, fresh.reference);
        let (rebuilt, clean) = (digests(&edited), digests(&fresh));
        assert_eq!(rebuilt, clean);
        assert_ne!(rebuilt, old);
        // The rebuilt program also runs as the fresh one does.
        let matrix = smoke_matrix();
        let toolchain = toolchain(&matrix);
        let dir = scratch("body-edit");
        for program in [&edited, &fresh] {
            for (configuration, outcome) in differential(
                &program.validated,
                &program.entry,
                &fresh.reference,
                &matrix,
                &toolchain,
                &dir,
            ) {
                assert_eq!(outcome, Outcome::Agree, "{configuration}");
            }
        }
    });
}

/// The link manifest refuses a partition that leaves the entry out.
#[test]
fn linking_without_the_entry_unit_is_refused() {
    with_compiler_stack(|| {
        let program = checked(HELLO);
        let ready = build(&program.validated, &Options::new(x86())).unwrap();
        let matrix = smoke_matrix();
        let toolchain = toolchain(&matrix);
        let mut other = program.entry.clone();
        other.0.name = "absent".into();
        let error = link(
            &ready,
            &other,
            OptLevel::None,
            Partition::Whole,
            &toolchain,
            &scratch("no-entry"),
        )
        .unwrap_err();
        assert_eq!(error.stage, StageName::Launch);
        assert!(error.message.contains("entry contract"), "{error}");
    });
}

/// A hot loop calling a small helper in another unit.
const SMALL_HELPER: &str = "fn step(h: u64, value: u64) -> u64 {\n  let wide: u128 = int.widen(h) * 131u128 + int.widen(value)\n  int.truncate(wide)\n}\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let mut h: u64 = 0\n  for i in 0u64..1000u64 { h = step(h, i) }\n  let _ = out.print(io.stdout(rt)?, if h == 16055195744720194772u64 { \"mixed\\n\" } else { \"other\\n\" })\n  Ok(())\n}\n";

/// One object per unit still inlines a small callee across objects at O3: the
/// caller's object carries an `available_externally` copy, so it neither
/// imports nor calls the callee, whose own object still exports it. At O0
/// nothing is copied and the caller imports it (the control).
#[test]
fn a_small_callee_inlines_across_singleton_objects_at_o3() {
    use hirsel_driver::native::{OptLevel, Options, build, partition_objects};
    let observed = agree("small-helper", SMALL_HELPER, &topology_matrix(2));
    assert_eq!(observed.output, b"mixed\n");
    with_compiler_stack(|| {
        let program = checked(SMALL_HELPER);
        let ready = build(&program.validated, &Options::new(x86())).unwrap();
        let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
            &ready.table,
            &ready.finalizers,
        );
        let mut step = program.entry.clone();
        step.0.name = "step".into();
        let names = |symbols: &[hirsel_lir::stage::ObjectSymbol]| -> Vec<DefPath> {
            symbols
                .iter()
                .filter_map(|s| s.definition.clone())
                .collect()
        };
        for (opt, copied) in [(OptLevel::Speed, true), (OptLevel::None, false)] {
            let objects =
                partition_objects(&ready, opt, Partition::Singleton, &registry, false, None)
                    .unwrap();
            let caller = objects
                .iter()
                .find(|o| names(&o.exports).contains(&program.entry))
                .unwrap();
            assert_eq!(names(&caller.imports).contains(&step), !copied, "{opt:?}");
            assert!(!names(&caller.exports).contains(&step), "{opt:?}");
            let owners = objects
                .iter()
                .filter(|o| names(&o.exports).contains(&step))
                .count();
            assert_eq!(owners, 1, "{opt:?}");
        }
    });
}

/// A failing index in statement position fails at the index expression, by
/// line and column, in all three engines (the diagnostics part's primary site is the
/// operation that fails; corpus headers pin only the line).
#[test]
fn an_index_failure_site_is_the_index_expression_in_every_engine() {
    const INLINE: &str = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let xs: Inline[u8, 1] = [7u8]
  let i: u64 = 1
  let x = xs[i]
  Ok(())
}
";
    const LIST: &str = "pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let xs: List[u8] = [7u8]
  let i: u64 = 1
  let x = xs[i]
  Ok(())
}
";
    let matrix = smoke_matrix();
    for (name, source) in [("site-inline", INLINE), ("site-list", LIST)] {
        let observed = agree(name, source, &matrix);
        let [failure] = observed.failures.as_slice() else {
            panic!("{name}: {observed:?}")
        };
        assert_eq!(
            (failure.rule.as_str(), failure.line, failure.column),
            ("MEM-14", 4, 11),
            "{name}"
        );
    }
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn descriptor_indices_are_assigned_before_all_twenty_two_link_partitions() {
    let source = r#"
type Pair { first: box u64, second: box u64 }
fn leaf(n: u64) -> box u64 { box n }
fn pair(n: u64) -> box Pair { box Pair(first: leaf(n), second: leaf(n + 1u64)) }
fn same(a: box Pair, b: box Pair) -> bool { a == b }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
    let a = pair(40u64)
    let b = pair(40u64)
    let c = pair(41u64)
    let valid = same(a, b) && !same(a, c)
    let _ = out.print(io.stdout(rt)?, if valid { "indices ok\n" } else { "indices bad\n" })
    Ok(())
}
"#;
    let matrix = topology_matrix(20);
    assert_eq!(matrix.len(), 22);
    let observed = agree("descriptor-indices", source, &matrix);
    assert_eq!(observed.output, b"indices ok\n");
}

#[test]
fn native_main_records_closed_stdout_and_broken_pipe_as_output_errors() {
    use hirsel_driver::native::{OptLevel, Options, build, link};
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    with_compiler_stack(|| {
        let checked = checked(HELLO);
        let configurations = smoke_matrix();
        let tools = toolchain(&configurations);
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let program = build(&checked.validated, &Options::new(target)).unwrap();
        let root = scratch("closed-output");
        let executable = link(
            &program,
            &checked.entry,
            OptLevel::Speed,
            Partition::Whole,
            &tools,
            &root,
        )
        .unwrap();
        let tape = root.with_extension("tape");
        for broken_pipe in [false, true] {
            let mut command = Command::new(&executable);
            command
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_TAPE", &tape);
            if broken_pipe {
                use std::os::fd::{FromRawFd, OwnedFd};
                unsafe extern "C" {
                    fn pipe(fds: *mut i32) -> i32;
                    fn close(fd: i32) -> i32;
                }
                let mut fds = [0; 2];
                assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
                unsafe {
                    close(fds[0]);
                }
                command.stdout(std::process::Stdio::from(unsafe {
                    OwnedFd::from_raw_fd(fds[1])
                }));
            } else {
                // SAFETY: child-only pre-exec close of a standard descriptor.
                unsafe {
                    command.pre_exec(|| {
                        libc_close_stdout();
                        Ok(())
                    });
                }
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            let record = hirsel_rt::launch::parse_recording(
                &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
            )
            .unwrap();
            assert!(matches!(
                record.events[1].completion,
                hirsel_rt::io::Completion::Unit(Err(_))
            ));
            assert_eq!(record.events[1].fault.as_ref().unwrap().partial, 0);
        }
        // A valid argument admits the launch; the byte-invalid twin refuses
        // before entry and before creating its ordinary tape.
        use std::os::unix::ffi::OsStringExt;
        std::fs::remove_file(&tape).unwrap();
        for (argument, expected) in [
            (std::ffi::OsString::from("é"), 0),
            (std::ffi::OsString::from_vec(vec![0xff]), 3),
        ] {
            let output = Command::new(&executable)
                .arg(argument)
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_TAPE", &tape)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(expected), "{output:?}");
            if expected == 0 {
                assert_eq!(output.stdout, checked.reference.output);
                let record = hirsel_rt::launch::parse_recording(
                    &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
                )
                .unwrap();
                assert_eq!(record.arguments, ["é"]);
                std::fs::remove_file(&tape).unwrap();
            } else {
                assert!(!tape.exists());
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("argument 0 is not UTF-8")
                );
            }
        }
        // Invalid snapshot names and links affect operations, not launch admission.
        std::fs::write(
            root.join(std::ffi::OsString::from_vec(vec![0xff])),
            b"invalid",
        )
        .unwrap();
        std::os::unix::fs::symlink("../outside", root.join("escape")).unwrap();
        let output = Command::new(&executable)
            .env_clear()
            .env("HIRSEL_RT_ROOT", &root)
            .env("HIRSEL_RT_TAPE", &tape)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(output.stdout, checked.reference.output);
        let record = hirsel_rt::launch::parse_recording(
            &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
        )
        .unwrap();
        assert_eq!(record.tree.links["escape"], "../outside");
        std::fs::remove_file(tape).unwrap();
        // Keep the fd close local to the child without adding a production dependency.
        unsafe fn libc_close_stdout() {
            unsafe extern "C" {
                fn close(fd: i32) -> i32;
            }
            unsafe {
                close(1);
            }
        }
    });
}

/// The smoke matrix's native configuration with the flat walk on and off.
fn flat_walk_switch() -> [Configuration; 2] {
    let mut on = smoke_matrix().pop().unwrap();
    on.options.flat_walk = true;
    let mut off = on.clone();
    off.options.flat_walk = false;
    [on, off]
}

// The importer check and the executable comparison build separately, so the
// two cases run in parallel shards.
#[test]
fn flat_walk_switch_reaches_the_importer_of_a_flat_sequence_loop() {
    let [on, off] = flat_walk_switch();
    let source = r#"flat type Item { text: String, n: u64 }
flat type Items { items: List[Item] }
fn sum(xs: Items) -> u64 {
  let mut total: u64 = 0
  for x in xs.items { total += x.n }
  total
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let item = Item(text: "longer", n: 3)
  let _ = out.print(io.stdout(rt)?, std@1/u64.to_string(item.n))
  Ok(())
}
"#;
    // A source-level flat-list constructor is not native yet. Keep the loop
    // as a selected ordinary unit and prove this switch reaches the importer;
    // the executable compares observable flat-record construction on both.
    with_compiler_stack(|| {
        let checked = checked(source);
        let count = |options: &hirsel_driver::native::Options| {
            let ready = hirsel_driver::native::build(&checked.validated, options).unwrap();
            ready
                .units
                .iter()
                .flat_map(|u| &u.unit().instructions)
                .filter(|i| matches!(i, hirsel_lir::ir::Inst::FlatNext { .. }))
                .count()
        };
        assert_eq!(count(&on.options), 1);
        assert_eq!(count(&off.options), 0);
    });
}

#[test]
fn flat_walk_ablation_preserves_flat_sequence_output() {
    let executable_source = r#"flat type Item { text: String, n: u64 }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let item = Item(text: "longer", n: 3)
  let _ = out.print(io.stdout(rt)?, std@1/u64.to_string(item.n))
  Ok(())
}
"#;
    let observation = agree("flat-walk-ablation", executable_source, &flat_walk_switch());
    assert_eq!(observation.output, b"3");
}

#[test]
fn generated_main_binds_independent_seeds_and_refuses_mismatch_before_entry() {
    use hirsel_driver::native::{LaunchInputs, OptLevel, Options, Tape, build, launch_with, link};
    with_compiler_stack(|| {
        let configs = full_matrix();
        let tools = toolchain(&configs);
        for target in [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ] {
            for (kind, source) in [
                ("main", HELLO),
                (
                    "test",
                    "pub fn main(t: Test) -> Result[(), ()] { test.check(t, true); Ok(()) }",
                ),
            ] {
                let checked = checked(source);
                let dir = scratch(&format!("seed-{}-{kind}", target.triple));
                let tape = dir.with_extension("tape");
                let mut options = Options::new(target.clone());
                options.launch = LaunchInputs {
                    hash_seed: u64::MAX,
                    random_seed: 3,
                    test_seed: 5,
                    scheduler: None,
                    fault_seed: None,
                    fixture_file_faults: None,
                    fixture_monotonic: None,
                };
                let program = build(&checked.validated, &options).unwrap();
                let exe = link(
                    &program,
                    &checked.entry,
                    OptLevel::Speed,
                    Partition::Whole,
                    &tools,
                    &dir,
                )
                .unwrap();
                let recorded = launch_with(
                    &exe,
                    &target,
                    &tools,
                    &dir,
                    false,
                    Tape::Live {
                        record: Some(&tape),
                        revocations: None,
                    },
                )
                .unwrap();
                assert_eq!(recorded.exit, Some(0));
                assert_eq!(recorded.stdout, checked.reference.output);
                let record = hirsel_rt::launch::parse_recording(
                    &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    (
                        record.hash_seed,
                        record.random_seed,
                        record.test_seed,
                        record.run_identity
                    ),
                    (
                        u64::MAX,
                        3,
                        5,
                        hirsel_run_config::RunConfig::read_path(
                            &std::fs::canonicalize(dir.join("run.json")).unwrap()
                        )
                        .unwrap()
                        .identity()
                    )
                );
                let replay =
                    launch_with(&exe, &target, &tools, &dir, false, Tape::Replay(&tape)).unwrap();
                assert_eq!(replay.exit, Some(0));
                assert_eq!(replay.stdout, recorded.stdout);
                for field in 0..4 {
                    let mut forged = record.clone();
                    match field {
                        0 => forged.hash_seed = 1,
                        1 => forged.random_seed += 1,
                        2 => forged.test_seed += 1,
                        _ => forged.run_identity[0] ^= 1,
                    }
                    std::fs::write(&tape, hirsel_rt::launch::render_recording(&forged).unwrap())
                        .unwrap();
                    let refused =
                        launch_with(&exe, &target, &tools, &dir, false, Tape::Replay(&tape))
                            .unwrap();
                    assert_eq!(refused.exit, Some(3), "{refused:?}");
                    assert!(
                        refused.stdout.is_empty(),
                        "entry produced output before refusal"
                    );
                    // The launcher reads the refusal from its refusal file.
                    assert!(
                        refused
                            .record
                            .contains("\nrefused recording configuration mismatch"),
                        "{refused:?}"
                    );
                }
                std::fs::remove_file(tape).unwrap();
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    });
}

/// Root-free process entries keep the ordinary Result and Failure exit contract.
#[test]
fn zero_parameter_entries_agree_without_host_roots() {
    let matrix = smoke_matrix();
    for (name, source, exit) in [
        (
            "zero-ok",
            "pub fn main() -> Result[(), ()] { Ok(()) }",
            Some(0),
        ),
        (
            "zero-error",
            "pub fn main() -> Result[(), ()] { Error(()) }",
            Some(1),
        ),
        (
            "zero-failure",
            "pub fn main() -> Result[(), ()] { let _ = 1u64 / 0u64; Ok(()) }",
            None,
        ),
    ] {
        let observed = agree(name, source, &matrix);
        assert_eq!(observed.exit_code, exit);
        assert!(observed.output.is_empty());
        assert_eq!(observed.failures.len(), usize::from(exit.is_none()));
    }
}

#[test]
fn zero_parameter_native_launch_refuses_non_result_and_non_unit_success() {
    with_compiler_stack(|| {
        let toolchain = toolchain(&smoke_matrix());
        for (name, source) in [
            ("zero-scalar", "pub fn main() -> u32 { 0u32 }"),
            (
                "zero-nonunit",
                "pub fn main() -> Result[u32, ()] { Ok(0u32) }",
            ),
            ("zero-private", "fn main() -> Result[(), ()] { Ok(()) }"),
        ] {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("entry.hir", source.into()).unwrap();
            let output = check_progress(&mut engine, file, &context).unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let ready = build(&validated, &Options::new(x86())).unwrap();
            let error = link(
                &ready,
                &entry_path(&context),
                OptLevel::None,
                Partition::Whole,
                &toolchain,
                &scratch(name),
            )
            .unwrap_err();
            assert_eq!(error.stage, StageName::Launch);
            assert!(error.message.contains("no typed launch kind"), "{error}");
        }
    });
}

/// A harness launch without `HIRSEL_RT_ROOT` grants the root Io nothing: not the working
/// directory, not stdout.
#[test]
fn io_native_entry_without_a_harness_root_is_granted_no_projection() {
    with_compiler_stack(|| {
        let checked = checked(
            r#"pub fn main(rt: Io, args: Args) -> Result[(), String] {
  match io.dir(rt, path.of("")) { Error(NotGranted()) => (), _ => return Error("dir") }
  match io.stdout(rt) { Error(_) => (), Ok(_) => return Error("stdout") }
  Ok(())
}"#,
        );
        let ready = build(&checked.validated, &Options::new(x86())).unwrap();
        let toolchain = toolchain(&smoke_matrix());
        let dir = scratch("io-no-root");
        let executable = link(
            &ready,
            &checked.entry,
            OptLevel::Speed,
            Partition::Whole,
            &toolchain,
            &dir,
        )
        .unwrap();
        let output = std::process::Command::new(std::fs::canonicalize(executable).unwrap())
            .current_dir(&dir)
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    });
}

#[test]
fn root_free_native_entry_ignores_ambient_directory_and_argument_inputs() {
    use std::os::unix::ffi::OsStringExt;
    with_compiler_stack(|| {
        let checked = checked("pub fn main() -> Result[(), ()] { Ok(()) }");
        let ready = build(&checked.validated, &Options::new(x86())).unwrap();
        let toolchain = toolchain(&smoke_matrix());
        let dir = scratch("zero-ambient");
        let executable = link(
            &ready,
            &checked.entry,
            OptLevel::Speed,
            Partition::Whole,
            &toolchain,
            &dir,
        )
        .unwrap();
        let output = std::process::Command::new(executable)
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .env("HIRSEL_RT_ROOT", dir.join("absent"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    });
}

/// A corpus program cannot forge the launcher's record or process status. Agreement
/// must reject a mismatched status before accepting the record's semantic observation.
#[test]
fn agreement_rejects_exit_codes_that_disagree_with_the_run_outcome() {
    use hirsel_driver::native::{Launched, Options, Sites, build, observe};
    use hirsel_run_schema::{BudgetKind, Returned, RunOutcome, Site, Wait};
    with_compiler_stack(|| {
        let input = checked("pub fn main() -> Result[(), ()] { Ok(()) }");
        let program = build(
            &input.validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let sites = Sites::new(input.validated.snapshot());
        let waits = vec![Wait {
            spawn_path: vec![0],
            site: Site {
                definition: "standalone@0/main".into(),
                file: "native.hir".into(),
                line: 1,
                column: 1,
            },
            kind: "channel.receive".into(),
            targets: vec![0],
        }];
        for (record, outcome) in [
            (
                "status 0\nresult 00000000\naudit 0 0 0 0 0\n".into(),
                RunOutcome::Returned(Returned::Ok),
            ),
            (
                "status 0\nresult 01000000\naudit 0 0 0 0 0\n".into(),
                RunOutcome::Returned(Returned::Error),
            ),
            (
                format!(
                    "blocked\nwaits {}\naudit unavailable\n",
                    Wait::encode(&waits)
                ),
                RunOutcome::Blocked(waits),
            ),
            (
                "budget_exceeded fuel\naudit unavailable\n".into(),
                RunOutcome::BudgetExceeded(BudgetKind::Fuel),
            ),
        ] {
            let mut launched = Launched {
                audit_required: false,
                exit: Some(outcome.exit_code()),
                stdout: vec![],
                stderr: vec![],
                signal: None,
                record: format!(
                    "hirsel-observation {}\n{record}",
                    hirsel_rt::launch::OBSERVATION_VERSION
                ),
            };
            assert!(observe(&launched, &program, &sites).is_ok(), "{outcome:?}");
            launched.exit = Some((outcome.exit_code() + 1) % 7);
            let error = observe(&launched, &program, &sites).unwrap_err();
            assert!(
                error.message.contains("the process did not exit"),
                "{error:?}"
            );
        }
        // A signaled record matches only a process ended by that signal, never an exit status
        // that spells it.
        let mut launched = Launched {
            audit_required: false,
            exit: None,
            stdout: vec![],
            stderr: vec![],
            signal: Some(hirsel_run_schema::SignalKind::Interrupt.number()),
            record: format!(
                "hirsel-observation {}\nsignaled interrupt\naudit unavailable\n",
                hirsel_rt::launch::OBSERVATION_VERSION
            ),
        };
        let observation = observe(&launched, &program, &sites).unwrap();
        assert_eq!(
            observation.outcome(&sites, vec![]).unwrap(),
            RunOutcome::Signaled(hirsel_run_schema::SignalKind::Interrupt)
        );
        launched.signal = None;
        launched.exit =
            Some(RunOutcome::Signaled(hirsel_run_schema::SignalKind::Interrupt).exit_code());
        let error = observe(&launched, &program, &sites).unwrap_err();
        assert!(error.message.contains("was not ended by"), "{error:?}");
    });
}

/// Each template has one retained owner, independently of which worker compiled it.
/// Include root adapters, dictionary function references and ordinary source inlining.
#[test]
fn template_owners_and_objects_are_identical_at_one_and_eight_workers() {
    use hirsel_driver::native::{self, Backend, OptLevel, Options};
    use hirsel_intern::jobs::Jobs;
    use hirsel_lir::emission::{function_thunk_name, symbol_name};
    use hirsel_lir::ir::UnitOrigin;
    with_compiler_stack(|| {
        for (name, source) in [
            ("generic", GENERIC),
            ("list", LIST_READ),
            (
                "root",
                "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let mut xs: List[u64] = []; list.push(mut xs, 7u64); let _ = out.print(io.stdout(rt)?, if xs[0u64] == 7u64 { \"seven\\n\" } else { \"bad\\n\" }); Ok(()) }",
            ),
            ("finalizer", FINALIZER),
        ] {
            let checked = checked(source);
            let ready = native::build(&checked.validated, &Options::new(x86())).unwrap();
            let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &ready.table,
                &ready.finalizers,
            );
            let tools = Toolchain::in_tree_diagnostic(
                &scratch(&format!("owners-runtime-{name}")),
                &[&ready.target],
            )
            .unwrap();
            for (backend, opt) in [
                (Backend::Llvm, OptLevel::None),
                (Backend::Llvm, OptLevel::Default),
                (Backend::Cranelift, OptLevel::None),
            ] {
                let emit = |jobs| {
                    native::partition_backend_objects_with_jobs(
                        &ready,
                        backend,
                        opt,
                        Partition::Singleton,
                        &registry,
                        false,
                        None,
                        Jobs::new(jobs),
                    )
                    .unwrap()
                };
                let serial = emit(1);
                assert_eq!(serial, emit(2), "{name}: {backend:?} {opt:?}: jobs=2");
                let parallel = emit(8);
                assert_eq!(serial, parallel, "{name}: {backend:?} {opt:?}");
                assert_eq!(serial.len(), ready.units.len() + 1, "one glue object");
                if name == "root" && backend == Backend::Llvm {
                    assert!(
                        serial
                            .iter()
                            .flat_map(|o| &o.exports)
                            .any(|e| e.name.contains(".roots")),
                        "mutable library root adapter was not exercised"
                    );
                }
                for unit in &ready.units {
                    let header = &unit.unit().header;
                    if header.origin != UnitOrigin::Library
                        && !ready.inline_instances.contains(&header.instance)
                    {
                        continue;
                    }
                    let entry = symbol_name(&header.definition, Some(&header.instance));
                    let thunk = function_thunk_name(&header.definition, Some(&header.instance));
                    let symbols = std::iter::once(&entry).chain(
                        (ready.table.fn_abis[header.abi.0 as usize].shape
                            == hirsel_lir::abi_types::CallableShape::Definition)
                            .then_some(&thunk),
                    );
                    for symbol in symbols {
                        let owners = serial
                            .iter()
                            .filter(|o| o.exports.iter().any(|e| &e.name == symbol))
                            .count();
                        assert_eq!(owners, 1, "{name}: {backend:?} {opt:?} {symbol}");
                    }
                }
                let dir = scratch(&format!("owners-{name}-{backend:?}-{opt:?}"));
                let exe =
                    native::link_objects(&ready, &checked.entry, &parallel, &tools, &dir).unwrap();
                let launched = native::launch(&exe, &ready.target, &tools, &dir, true).unwrap();
                let actual = native::observe(
                    &launched,
                    &ready,
                    &native::Sites::new(checked.validated.snapshot()),
                )
                .unwrap();
                assert_eq!(actual, checked.reference, "{name}: {backend:?} {opt:?}");
            }
        }
    });
}

/// Historical migration link failures must keep their callable owner objects,
/// including when cached summaries replace Ready bodies on the second build.
fn original_callable_values_retain_their_thunk_owners_on_both_backends(path: &str) {
    use hirsel_driver::native::{self, Backend, OptLevel, Options};
    use hirsel_engine::store::{MemStore, ResultStore};
    use hirsel_lir::emission::{function_thunk_name, symbol_name};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
        let mut config = hirsel_driver::COMPILER.configuration();
        config.store = store.clone();
        config.verify = hirsel_engine::Verify::On;
        config.verify_store = hirsel_engine::store::VerifyStore::Off;
        let mut loaded = hirsel_driver::COMPILER
            .with_config(config)
            .with_owned_engines()
            .load_program(&root.join(path))
            .unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let _validated = loaded.engine.import_native_input(input).unwrap();
        let entry = loaded.entry.clone();
        for (backend, opt) in [
            (Backend::Llvm, OptLevel::None),
            (Backend::Llvm, OptLevel::Speed),
            (Backend::Cranelift, OptLevel::None),
        ] {
            let compile = |engine: &mut QueryEngine| {
                crate::native_object_products_19::compile_entry(
                    engine,
                    store.clone(),
                    &entry,
                    backend,
                    opt,
                    Options::new(x86()),
                    false,
                    hirsel_intern::jobs::Jobs::ONE,
                )
            };
            let cold = compile(&mut loaded.engine);
            let warm = compile(&mut loaded.engine);
            assert_eq!(
                cold.summaries, warm.summaries,
                "{path}: {backend:?} {opt:?}"
            );
            let objects = &warm;
            assert_eq!(objects.hits, warm.summaries.len());
            assert_eq!(cold.objects, objects.objects, "{path}: {backend:?} {opt:?}");
            let mut entries = std::collections::BTreeSet::new();
            for summary in &warm.summaries {
                entries.extend(summary.glue.entries.iter().cloned());
            }
            assert!(!entries.is_empty(), "{path}: no callable values exercised");
            for (definition, instance) in entries {
                let owner = warm
                    .summaries
                    .iter()
                    .position(|s| {
                        s.definition == definition && instance.is_none_or(|i| s.instance == i)
                    })
                    .unwrap_or_else(|| panic!("{path}: missing owner of {definition}"));
                for symbol in [
                    symbol_name(&definition, instance.as_ref()),
                    function_thunk_name(&definition, instance.as_ref()),
                ] {
                    let owners: Vec<_> = objects
                        .objects
                        .iter()
                        .enumerate()
                        .filter_map(|(i, o)| {
                            o.exports.iter().any(|s| s.name == symbol).then_some(i)
                        })
                        .collect();
                    assert_eq!(owners, [owner], "{path}: {backend:?} {opt:?}: {symbol}");
                    eprintln!("{path}: {backend:?} {opt:?}: {symbol} owner={definition}");
                }
            }
        }
    });
}

#[test]
fn original_callable_owners_core_operations() {
    original_callable_values_retain_their_thunk_owners_on_both_backends(
        "conformance/LANG/LANG-2-core-operation-values-run.hir",
    );
}

#[test]
fn original_callable_owners_generic_context() {
    original_callable_values_retain_their_thunk_owners_on_both_backends(
        "conformance/LANG/LANG-2-generic-callable-context-run.hir",
    );
}

#[test]
fn original_callable_owners_track_caller_run() {
    original_callable_values_retain_their_thunk_owners_on_both_backends(
        "conformance/LANG/LANG-9-track-caller-primitive-value-run.hir",
    );
}

#[test]
fn original_callable_owners_track_caller_fail() {
    original_callable_values_retain_their_thunk_owners_on_both_backends(
        "conformance/LANG/LANG-9-track-caller-primitive-value-fail.hir",
    );
}

#[test]
fn original_callable_owners_std_container() {
    original_callable_values_retain_their_thunk_owners_on_both_backends(
        "conformance/MODE/MODE-2-std-container-empty-run.hir",
    );
}

#[test]
fn copied_callee_edits_invalidate_warm_owner_objects_at_one_two_eight_workers() {
    use hirsel_driver::native::{self, Backend, OptLevel, Options, Validation};
    use hirsel_engine::store::{MemStore, ResultStore, VerifyStore};
    use std::sync::Arc;
    with_compiler_stack(|| {
        // Keep the caller's text and positions fixed. Only its copied callee changes.
        let before = "pub fn main()->Result[(),()]{failure.check(step(3u64)==4u64,Arithmetic());Ok(())}\nfn step(x:u64)->u64{x+1u64}\n";
        let after = before.replace("x+1u64", "x+2u64");
        let original = checked(before);
        let edited = checked(&after);
        assert_ne!(original.reference, edited.reference);
        for jobs in [1, 2, 8] {
            for validation in [Validation::Ready, Validation::EveryStage] {
                let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
                let dir = scratch(&format!("copied-edit-{jobs}-{validation:?}"));
                let tools = Toolchain::in_tree_diagnostic(&dir, &[&x86()]).unwrap();
                let compile = |text: &str, store: Arc<dyn ResultStore>, link: bool| {
                    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                        .with_result_store(store.clone());
                    let verification = match validation {
                        Validation::Ready => VerifyStore::Off,
                        Validation::EveryStage => VerifyStore::Recompute,
                    };
                    engine = engine.with_verify_store(verification);
                    let checked = checked_in(&mut engine, text);
                    let mut options = Options::new(x86());
                    options.validation = validation;
                    crate::native_object_products_19::compile_with_context(
                        &mut engine,
                        store,
                        &checked.entry,
                        Backend::Llvm,
                        OptLevel::Default,
                        options,
                        false,
                        hirsel_intern::jobs::Jobs::new(jobs),
                        verification,
                        |ready, artifacts| {
                            link.then(|| {
                                let exe = native::link_objects(
                                    ready,
                                    &checked.entry,
                                    artifacts,
                                    &tools,
                                    &dir,
                                )
                                .unwrap();
                                let bytes = std::fs::read(&exe).unwrap();
                                let launched =
                                    native::launch(&exe, &ready.target, &tools, &dir, true)
                                        .unwrap();
                                let actual = native::observe(
                                    &launched,
                                    ready,
                                    &native::Sites::new(checked.validated.snapshot()),
                                )
                                .unwrap();
                                assert_eq!(actual, edited.reference);
                                bytes
                            })
                        },
                    )
                };
                let (old, _) = compile(before, store.clone(), false);
                let (warm, _) = compile(before, store.clone(), false);
                assert_eq!(warm.hits, old.summaries.len());
                let (rebuilt, rebuilt_exe) = compile(&after, store, true);
                let (fresh, fresh_exe) = compile(&after, Arc::new(MemStore::default()), true);
                let artifacts = |ready: &crate::native_object_products_19::Objects| {
                    (ready.summaries.clone(), ready.objects.clone())
                };
                assert_eq!(artifacts(&old), artifacts(&warm));
                assert_eq!(artifacts(&rebuilt), artifacts(&fresh));
                let owner = |ready: &crate::native_object_products_19::Objects| {
                    ready
                        .summaries
                        .iter()
                        .position(|s| s.definition == original.entry)
                        .unwrap()
                };
                let old_owner = owner(&old);
                let new_owner = owner(&rebuilt);
                assert_eq!(
                    old.summaries[old_owner], rebuilt.summaries[new_owner],
                    "caller changed"
                );
                assert_ne!(
                    old.objects[old_owner], rebuilt.objects[new_owner],
                    "stale copied callee"
                );
                assert_eq!(rebuilt_exe.unwrap(), fresh_exe.unwrap());
            }
        }
    });
}

#[test]
fn intrinsic_static_objects_survive_unrelated_layouts_and_import_replay() {
    use hirsel_driver::native::{self, Backend, OptLevel, Options};
    use hirsel_engine::store::{MemStore, ResultStore};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let source = "type Tree = Leaf | Branch(left: box Tree, right: box Tree)\nfn leaf()->box Tree { box Leaf() }\nfn empty()->List[(u64,u64)] { [] }\npub fn main()->Result[(),()] { let xs=empty(); let tree=leaf(); failure.check(list.len(xs)==0u64, Arithmetic()); Ok(()) }\n";
        let compile = |text: &str, store: Arc<dyn ResultStore>, backend, opt| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = engine.sources.add("statics.hir", text.into()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            let input = engine.export_native_input().unwrap();
            let _validated = engine.import_native_input(input).unwrap();
            crate::native_object_products_19::compile(
                &mut engine,
                store,
                backend,
                opt,
                Options::new(x86()),
                false,
                hirsel_intern::jobs::Jobs::ONE,
            )
        };
        let decoy = format!("fn aaa()->(bool,u32) {{ (true,0u32) }}\n{source}");
        for (backend, opt) in [
            (Backend::Cranelift, OptLevel::None),
            (Backend::Llvm, OptLevel::None),
            (Backend::Llvm, OptLevel::Default),
        ] {
            let a = compile(source, Arc::new(MemStore::default()), backend, opt);
            let b = compile(&decoy, Arc::new(MemStore::default()), backend, opt);
            let objects = |ready: &crate::native_object_products_19::Objects| {
                ready
                    .summaries
                    .iter()
                    .zip(&ready.objects)
                    .map(|(s, o)| ((s.definition.clone(), s.instance), o.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let baseline = objects(&a);
            let shifted = objects(&b);
            for (key, object) in &baseline {
                assert!(
                    object == &shifted[key],
                    "{backend:?} {opt:?}: {}: {:?} versus {:?}",
                    key.0,
                    object.imports,
                    shifted[key].imports
                );
            }
            let store = Arc::new(MemStore::default());
            compile(source, store.clone(), backend, opt);
            let replay = compile(&decoy, store, backend, opt);
            assert!(replay.hits > 0);
            assert_eq!(objects(&replay), shifted, "{backend:?} {opt:?}: replay");
        }
    });
}

fn generated_failure_sources() -> Vec<(&'static str, String, &'static str, u32, u32)> {
    let make = |literal: &str, input: u64| {
        format!(
            "fn body()->Result[Body[fn(u64)->u64 uses {{}}],String]{{Ok(codegen.body({literal:?}))}}\nfn generated(x:u64)->u64 uses {{}}=comptime body()\npub fn main()->Result[(),()]{{let _=generated({input}u64);Ok(())}}\n"
        )
    };
    let division = "fn(x:u64)->u64 uses {} {\n  9u64/x\n}";
    let ownership = "fn(x:u64)->u64 uses {} {\n  let xs:unique List[u64]=unique [9u64]\n  xs[x]\n}";
    let closure = "fn(x:u64)->u64 uses {} {\n  let f=fn(y:u64)->u64 {\n    9u64/y\n  }\n  f(x)\n}";
    // CT-5-generator-cycle-ok's acyclic generator chain, with a runtime
    // failure in the second generated body instead of its successful sum.
    let nested = format!(
        "fn first()->Result[Body[fn(u64)->u64 uses {{}}],String]{{Ok(codegen.body(\"fn(x:u64)->u64 uses {{}} {{x+1u64}}\"))}}\nfn plus_one(x:u64)->u64 uses {{}}=comptime first()\nfn body()->Result[Body[fn(u64)->u64 uses {{}}],String]{{let _=plus_one(1u64);Ok(codegen.body({division:?}))}}\nfn generated(x:u64)->u64 uses {{}}=comptime body()\npub fn main()->Result[(),()]{{let _=generated(0u64);Ok(())}}\n"
    );
    vec![
        ("division", make(division, 0), "SEM-1", 2, 3),
        ("ownership", make(ownership, 1), "MEM-14", 3, 3),
        ("closure", make(closure, 0), "SEM-1", 3, 5),
        ("nested-generator", nested, "SEM-1", 2, 3),
    ]
}

#[test]
fn generated_body_failure_uses_generated_source_in_every_engine() {
    with_compiler_stack(|| {
        let mut configurations = smoke_matrix();
        configurations.push(Configuration {
            options: configurations[0].options.clone(),
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: hirsel_driver::native::Runtime::Diagnostic,
            },
        });
        let tools = toolchain(&configurations);
        let mut mismatches = vec![];
        for (name, source, rule, line, column) in generated_failure_sources() {
            let program = checked(&source);
            let [failure] = program.reference.failures.as_slice() else {
                panic!("{name}: {:?}", program.reference)
            };
            assert_eq!(
                (failure.rule.as_str(), failure.line, failure.column),
                (rule, line, column),
                "{name}"
            );
            assert!(failure.file.starts_with("<generated:"), "{failure:?}");
            for (configuration, outcome) in differential(
                &program.validated,
                &program.entry,
                &program.reference,
                &configurations,
                &tools,
                &scratch(&format!("generated-{name}")),
            ) {
                if outcome != Outcome::Agree {
                    mismatches.push(format!("{name}: {configuration}: {outcome:?}"));
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    });
}

#[test]
fn generated_body_sites_and_objects_do_not_depend_on_id_assignment() {
    use hirsel_driver::native::{self, Backend, OptLevel, Validation};
    use hirsel_engine::store::{MemStore, ResultStore};
    use std::sync::Arc;
    let Some(runs) = hirsel_intern::under_salts(
        "native::generated_body_sites_and_objects_do_not_depend_on_id_assignment",
        &[1, 2, 3],
        || {
            with_compiler_stack(|| {
                let mut transcript = String::new();
                for (_, source, _, _, _) in generated_failure_sources() {
                    let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
                    let mut engine = QueryEngine::new(default_environment(), compiler_adapters())
                        .with_result_store(store.clone());
                    let program = checked_in(&mut engine, &source);
                    transcript.push_str(&format!("{:?}\n", program.reference));
                    for backend in [Backend::Cranelift, Backend::Llvm] {
                        let mut options = native::Options::new(x86());
                        options.validation = Validation::EveryStage;
                        let ready = crate::native_object_products_19::compile(
                            &mut engine,
                            store.clone(),
                            backend,
                            OptLevel::None,
                            options,
                            true,
                            hirsel_intern::jobs::Jobs::new(2),
                        );
                        for (summary, object) in ready.summaries.iter().zip(&ready.objects) {
                            transcript.push_str(&format!("{:?}\n", object.digest()));
                            for site in &summary.sites {
                                transcript.push_str(&format!(
                                    "{:?} {:?}\n",
                                    site,
                                    hirsel_lir::emission::site_code(
                                        &summary.definition,
                                        &summary.implementation_revision,
                                        *site,
                                    ),
                                ));
                            }
                        }
                    }
                }
                transcript
            })
        },
    ) else {
        return;
    };
    assert!(!runs[0].is_empty());
    assert_eq!(runs[0], runs[1], "salt 2");
    assert_eq!(runs[0], runs[2], "salt 3");
}

#[test]
fn generated_body_source_mapping_is_independently_validated() {
    with_compiler_stack(|| {
        let text = "fn make()->Result[Body[fn(u64)->u64 uses {}],String]{Ok(codegen.body(\"fn(x:u64)->u64 uses {} {9u64/x}\"))}\nfn made(x:u64)->u64 uses {}=comptime make()\npub fn main()->Result[(),()]{let _=made(0u64);Ok(())}\n";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        checked_in(&mut engine, text);
        let input = engine.export_native_input().unwrap();
        let mut made = entry_path(&default_context());
        made.0.name = "made".into();
        let declaration = input.source_owners[&made].syntax.anchor;
        let generated = hirsel_tir::source_anchors(&input.definitions[&made])
            .into_iter()
            .find(|a| *a != declaration)
            .expect("generated anchor");
        let (file, base) = input.sources.anchor(declaration).unwrap();
        let mut forged = input.clone();
        forged.sources.move_anchor(generated, file, base);
        assert!(
            engine.import_native_input(forged).is_err(),
            "declaration cannot authorize a generated source mapping"
        );
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn generated_body_origins_follow_item_moves_without_stale_source_names() {
    with_compiler_stack(|| {
        let maker = "fn make()->Result[Body[fn(u64)->u64 uses {}],String]{Ok(codegen.body(\"fn(x:u64)->u64 uses {} {\\n  9u64/x\\n}\"))}\n";
        let made = "fn made(x:u64)->u64 uses {}=comptime make()\n";
        let main = "pub fn main()->Result[(),()]{let _=made(0u64);Ok(())}\n";
        let load = |engine: &mut QueryEngine, first: &str, second: &str| {
            let files = [("one.hir", first), ("two.hir", second)]
                .map(|(name, text)| engine.sources.add(name, text.into()).unwrap());
            for file in files {
                engine.parse_file(file, &default_context()).unwrap();
            }
            for file in files {
                let output = check_progress(engine, file, &default_context()).unwrap();
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            }
            let run = RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            };
            let execution =
                run_observed_snapshot(engine, &run, SimulationConfig::default()).unwrap();
            Observation::of_reference(execution.observation.as_ref().unwrap())
        };
        let mut warm = QueryEngine::new(default_environment(), compiler_adapters());
        let first = format!("{maker}{made}{main}");
        let before = load(&mut warm, &first, "");
        let moved = format!("// inserted source lines\n\n{made}");
        let kept = format!("{maker}{main}");
        let after = load(&mut warm, &kept, &moved);
        let mut fresh = QueryEngine::new(default_environment(), compiler_adapters());
        assert_eq!(after, load(&mut fresh, &kept, &moved));
        assert_eq!(
            before, after,
            "generated coordinates and source identity survive a declaration move"
        );
        let mut path = entry_path(&default_context());
        path.0.name = "made".into();
        let origin = warm.generated_source(&path).unwrap().site;
        assert_eq!(
            warm.sources
                .file(warm.sources.resolve(origin).file)
                .unwrap()
                .path
                .to_str(),
            Some("two.hir")
        );
        assert_eq!(warm.sources.location(origin).unwrap().line, 3);
    });
}

#[test]
fn generated_body_objects_rebind_debug_and_failures_after_source_moves() {
    use hirsel_driver::native::{self, Backend, OptLevel, Validation};
    use hirsel_engine::store::{MemStore, ResultStore, VerifyStore};
    use std::sync::Arc;
    with_compiler_stack(|| {
        let source = generated_failure_sources().remove(0).1;
        let (maker, rest) = source.split_once('\n').unwrap();
        let (made, main) = rest.split_once('\n').unwrap();
        let all = format!("{maker}\n{made}\n{main}");
        let shifted = format!("// inserted line\n\n{all}");
        let kept = format!("{maker}\n{main}");
        // Keep the moved item's owned leading trivia unchanged: a first
        // item's inserted comment would change its implementation revision.
        let moved = format!("// moved declaration\ntype Marker {{}}\n{made}\n");
        let versions = [
            (all.as_str(), ""),
            (shifted.as_str(), ""),
            (kept.as_str(), moved.as_str()),
        ];
        let load = |engine: &mut QueryEngine, first: &str, second: &str| {
            let files = [("one.hir", first), ("two.hir", second)]
                .map(|(name, text)| engine.sources.add(name, text.into()).unwrap());
            for file in files {
                engine.parse_file(file, &default_context()).unwrap();
            }
            for file in files {
                let output = check_progress(engine, file, &default_context()).unwrap();
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            }
            checked_engine(engine)
        };
        let matrix = smoke_matrix();
        let tools = toolchain(&matrix).with_runtime(native::Runtime::Diagnostic.build());
        let dwarf = std::path::PathBuf::from(std::env::var_os("LLVM_SYS_231_PREFIX").unwrap())
            .join("bin/llvm-dwarfdump");
        for (backend, opt) in [
            (Backend::Cranelift, OptLevel::None),
            (Backend::Llvm, OptLevel::Default),
        ] {
            let store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
            let mut warm_engine = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let mut baseline = None;
            for (version, (first, second)) in versions.iter().enumerate() {
                let warm = load(&mut warm_engine, first, second);
                let cold_store: Arc<dyn ResultStore> = Arc::new(MemStore::default());
                let mut fresh_engine = QueryEngine::new(default_environment(), compiler_adapters())
                    .with_result_store(cold_store.clone());
                let fresh = load(&mut fresh_engine, first, second);
                assert_eq!(warm.reference, fresh.reference);
                let compile = |engine: &mut QueryEngine,
                               store: Arc<dyn ResultStore>,
                               validation,
                               jobs,
                               link: bool| {
                    let mut options = native::Options::new(x86());
                    options.validation = validation;
                    let verification = match validation {
                        Validation::Ready => VerifyStore::Off,
                        Validation::EveryStage => VerifyStore::Recompute,
                    };
                    crate::native_object_products_19::compile_with_context(
                        engine,
                        store,
                        &warm.entry,
                        backend,
                        opt,
                        options,
                        true,
                        hirsel_intern::jobs::Jobs::new(jobs),
                        verification,
                        |ready, objects| {
                            if !link {
                                return;
                            }
                            let dir = scratch(&format!(
                                "generated-debug-{backend:?}-{version}-{jobs}-{validation:?}"
                            ));
                            let exe =
                                native::link_objects(ready, &warm.entry, objects, &tools, &dir)
                                    .unwrap();
                            let launched =
                                native::launch(&exe, &ready.target, &tools, &dir, true).unwrap();
                            assert_eq!(launched.exit, Some(2));
                            let actual = native::observe(
                                &launched,
                                ready,
                                &native::Sites::new(warm.validated.snapshot()),
                            )
                            .unwrap();
                            assert_eq!(actual, warm.reference, "{backend:?}, version={version}");
                            let verification = std::process::Command::new(&dwarf)
                                .arg("--verify")
                                .arg(&exe)
                                .output()
                                .unwrap();
                            assert!(
                                verification.status.success(),
                                "{}",
                                String::from_utf8_lossy(&verification.stdout)
                            );
                            let lines = std::process::Command::new(&dwarf)
                                .arg("--debug-line")
                                .arg(&exe)
                                .output()
                                .unwrap();
                            assert!(lines.status.success());
                            assert!(
                                String::from_utf8_lossy(&lines.stdout)
                                    .contains("<generated:standalone@0/generated>"),
                                "generated debug origin missing"
                            );
                        },
                    )
                    .0
                };
                let cold = compile(
                    &mut fresh_engine,
                    cold_store,
                    Validation::EveryStage,
                    1,
                    false,
                );
                let cold_objects = &cold.objects;
                if let Some(baseline) = &baseline {
                    assert!(
                        cold_objects == baseline,
                        "{backend:?}: version={version}: moving source must retain unit objects"
                    );
                } else {
                    baseline = Some(cold_objects.clone());
                }
                // O1 worker budgets, O2 warm=fresh, O3 moved resident inputs;
                // O4 EveryStage independently recomputes and compares hits.
                for (validation, jobs) in [
                    (Validation::Ready, 1),
                    (Validation::Ready, 2),
                    (Validation::Ready, 8),
                    (Validation::EveryStage, 2),
                ] {
                    let built = compile(&mut warm_engine, store.clone(), validation, jobs, true);
                    assert!(
                        &built.objects == cold_objects,
                        "{backend:?}: version={version}, jobs={jobs}, {validation:?}: warm and fresh object bytes differ"
                    );
                    if version > 0 || jobs > 1 {
                        assert!(built.hits > 0, "warm build must actually hit");
                    }
                }
            }
        }
    });
}

#[test]
fn generated_body_debug_preserves_nested_body_sites() {
    use hirsel_driver::native::{self, Backend, OptLevel};
    with_compiler_stack(|| {
        for (name, source, _, line, column) in generated_failure_sources().into_iter().skip(2) {
            let program = checked(&source);
            let sources = native::profile_sources(program.validated.snapshot());
            let ready = native::build(&program.validated, &native::Options::new(x86())).unwrap();
            for backend in [Backend::Cranelift, Backend::Llvm] {
                let objects = native::emit_backend_objects(
                    &ready,
                    backend,
                    OptLevel::None,
                    Partition::Whole,
                    false,
                    Some(&sources),
                )
                .unwrap();
                let mut found = false;
                let mut source_less_intrinsics = 0;
                for function in objects.iter().flat_map(|o| &o.debug) {
                    assert!(function.well_formed());
                    for range in &function.ranges {
                        let item = range.item as usize;
                        if function.owner_kinds[item]
                            == hirsel_lir::debug::DebugOwnerKind::SourceLessCoreIntrinsic
                        {
                            assert!(!sources.contains_key(&function.items[item]));
                            source_less_intrinsics += 1;
                            continue;
                        }
                        let source = &sources[&function.items[item]];
                        let (file, base) = source.source(function.sources[item]).unwrap();
                        let location = file.location(base + range.offset).unwrap();
                        found |= file.path.to_string_lossy().starts_with("<generated:")
                            && (location.line, location.column) == (line, column);
                    }
                }
                assert!(
                    source_less_intrinsics > 0,
                    "{backend:?}: intrinsic intervals missing"
                );
                assert!(
                    found,
                    "{backend:?}: {name}: generated failure debug coordinate {line}:{column} missing"
                );
            }
        }
    });
}

#[test]
fn nested_copied_prefix_brands_preserve_ownership_on_h_llvm_and_cranelift() {
    let source = r#"
type Inner { n: u64, other: String }
type R { a: Inner, b: String }
fn make_i(r: Inner) -> Option[u8 of r.other] { None }
fn inspect_i(r: Inner, p: Option[u8 of r.other]) {}
fn probe(take x: R) {
  let mut r = copy x
  r.a.n = 3
  let q = make_i(r.a)
  inspect_i(r.a, q)
  let s = copy r.a
  let qs = make_i(s)
  inspect_i(s, qs)
  failure.check(x.a.n == 1 && r.a.n == 3 && s.other == x.a.other, TextInvariant())
  failure.check(r.b == x.b, TextInvariant())
}
pub fn main() -> Result[(), ()] {
  probe(R(a: Inner(n: 1, other: "preserved"), b: "sibling"))
  Ok(())
}
"#;
    let mut configurations = smoke_matrix();
    configurations.truncate(2);
    configurations[0].engine = Engine::Heap { campaign: false };
    configurations.push(Configuration {
        options: configurations[0].options.clone(),
        engine: Engine::Cranelift {
            partition: Partition::Whole,
            runtime: hirsel_driver::native::Runtime::Diagnostic,
        },
    });
    assert_eq!(configurations.len(), 3);
    with_compiler_stack(|| {
        let program = checked(source);
        assert_eq!(
            program.reference.exit_code,
            Some(0),
            "{:?}",
            program.reference
        );
        assert!(
            program.reference.failures.is_empty(),
            "{:?}",
            program.reference
        );
        let outcomes = differential(
            &program.validated,
            &program.entry,
            &program.reference,
            &configurations,
            &toolchain(&configurations),
            &scratch("nested-copied-prefix-brands"),
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            eprintln!("nested copied prefix: {configuration}: {outcome:?}");
            assert!(
                matches!(outcome, Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

/// The current recording generation replays the full depth failure, and refuses
/// a mismatched binding before entry or a missing completion as divergence.
#[test]
fn native_logical_depth_recording_replays_and_rejects_corruption() {
    use hirsel_driver::native::{self, Backend, Observed, OptLevel, Options, Sites, Tape};
    with_compiler_stack(|| {
        let checked = checked(
            r#"
fn descend(n:u64)->u64 {if n==0u64 {0u64}else {1u64+descend(n-1u64)}}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?
 let _=out.print(stdout,"entered\n")
 let _=descend(1023u64)
 Ok(())
}
"#,
        );
        assert_eq!(checked.reference.output, b"entered\n");
        assert_eq!(checked.reference.failures.len(), 1);
        assert_eq!(checked.reference.failures[0].rule, "LANG-10");
        let tools = toolchain(&native::trace_roots_matrix());
        let ready = native::build(&checked.validated, &Options::new(x86())).unwrap();
        let sites = Sites::new(checked.validated.snapshot());
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = scratch(&format!("logical-depth-recording-{backend:?}"));
            let tape = dir.join("depth.tape");
            let objects = native::emit_backend_objects(
                &ready,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let runnable =
                native::link_runnable(&ready, &checked.entry, &objects, &tools, &dir, None)
                    .unwrap();
            let recorded = native::launch_runnable(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Live {
                    record: Some(&tape),
                    revocations: None,
                },
            )
            .unwrap();
            assert_eq!(
                native::observe(&recorded, &ready, &sites).unwrap(),
                checked.reference
            );
            let replay = native::launch_runnable(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Replay(&tape),
            )
            .unwrap();
            assert_eq!(
                native::observe(&replay, &ready, &sites).unwrap(),
                checked.reference
            );
            let recording = hirsel_rt::launch::parse_recording(
                &hirsel_rt::launch::read_recording_text(&tape).unwrap(),
            )
            .unwrap();
            assert!(!recording.events.is_empty());
            let mut forged = recording.clone();
            forged.run_identity[0] ^= 1;
            std::fs::write(&tape, hirsel_rt::launch::render_recording(&forged).unwrap()).unwrap();
            let refused = native::launch_runnable(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Replay(&tape),
            )
            .unwrap();
            assert!(refused.stdout.is_empty(), "entry ran before refusal");
            assert!(
                matches!(
                    native::observe_run(&refused, &ready, &sites).unwrap(),
                    Observed::Refused(_)
                ),
                "{refused:?}"
            );
            let mut forged = recording;
            forged.events.clear();
            std::fs::write(&tape, hirsel_rt::launch::render_recording(&forged).unwrap()).unwrap();
            let diverged = native::launch_runnable(
                &runnable,
                &ready.target,
                &tools,
                &dir,
                true,
                Tape::Replay(&tape),
            )
            .unwrap();
            assert!(
                matches!(
                    native::observe_run(&diverged, &ready, &sites).unwrap(),
                    Observed::Stopped(hirsel_run_schema::RunOutcome::Diverged(_))
                ),
                "{diverged:?}"
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    });
}

#[test]
fn copied_source_aggregate_calls_preserve_overflow_and_shared_inputs() {
    let source = r#"
type P { text: String, x: u64, y: u64 }
fn point(take text: String, x: u64) -> P { P(text: take text, x: x, y: x + 1u64) }
fn total(p: P) -> u64 { p.x + p.y }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let original = point("retained", 2u64)
  let saved = copy original
  let mut changed = copy original
  changed.text = "changed"
  failure.check(saved.text == "retained" && changed.text == "changed", TextInvariant())
  let _ = out.print(io.stdout(rt)?, if total(original) == 5u64 { "before\n" } else { "wrong\n" })
  let _ = point("overflow", 18446744073709551615u64)
  Ok(())
}
"#;
    let observation = agree("copied-aggregate-source", source, &topology_matrix(0));
    assert_eq!(observation.output, b"before\n");
    assert_eq!(observation.failures.len(), 1);
    assert_eq!(observation.failures[0].rule, "SEM-1");
    assert_eq!(observation.failures[0].line, 3);
}

#[test]
fn long_hash_inputs_preserve_aliases_and_independent_maps() {
    let source = r#"
fn lookup(m: Map[String, u64], text: String) -> u64 {
  match map.get(m, text) { Some(value) => value, None => 0 }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let mut first: Map[String, u64] = map.empty()
  let mut second: Map[String, u64] = map.empty()
  let mut key = "long key crossing more than two hash words"
  let retained = copy key
  map.insert(mut first, copy key, 17)
  map.insert(mut second, copy key, 29)
  let saved = copy first
  key = "a different key crossing two hash words"
  map.insert(mut first, copy key, 41)
  map.insert(mut second, copy retained, 53)
  failure.check(lookup(saved, retained) == 17, TextInvariant())
  failure.check(lookup(first, retained) == 17 && lookup(first, key) == 41, TextInvariant())
  failure.check(lookup(second, retained) == 53 && lookup(second, key) == 0, TextInvariant())
  let mut many: Map[String, u64] = map.empty()
  for i in 0u64..32 {
    let item = if i % 2 == 0 { copy retained } else { copy key }
    map.insert(mut many, copy item, i)
    failure.check(lookup(many, item) == i, TextInvariant())
  }
  let _ = out.print(io.stdout(rt)?, "hash aliases preserved\n")
  Ok(())
}
"#;
    let observation = agree("hash-stream-aliases", source, &topology_matrix(0));
    assert_eq!(observation.output, b"hash aliases preserved\n");
    assert!(observation.failures.is_empty(), "{observation:?}");
}

#[test]
fn scalar_map_queries_keep_independent_updates_and_saved_versions() {
    let source = r#"
fn read(m: Map[u32, u64], key: u32) -> u64 {
  match map.get(m, key) { Some(value) => value, None => 0 }
}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let mut first: Map[u32, u64] = map.empty()
  let mut second: Map[u32, u64] = map.empty()
  for key in 0u32..64u32 {
    map.insert(mut first, key, 100u64 + int.widen(key))
    map.insert(mut second, key, 200u64 + int.widen(key))
  }
  let saved = copy first
  for key in 0u32..64u32 {
    let earlier = read(first, key)
    failure.check(earlier == 100u64 + int.widen(key), TextInvariant())
    map.insert(mut first, key, earlier + 17)
    failure.check(read(first, key) == earlier + 17, TextInvariant())
    failure.check(read(saved, key) == earlier, TextInvariant())
    failure.check(read(second, key) == earlier + 100, TextInvariant())
    failure.check(map.contains(first, key), TextInvariant())
  }
  failure.check(read(first, 64) == 0 && !map.contains(first, 64), TextInvariant())
  let _ = out.print(io.stdout(rt)?, "scalar maps preserved\n")
  Ok(())
}
"#;
    let observation = agree("scalar-map-source", source, &topology_matrix(0));
    assert_eq!(observation.output, b"scalar maps preserved\n");
    assert!(observation.failures.is_empty(), "{observation:?}");
}
