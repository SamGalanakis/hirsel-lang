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
    DefPath, QueryEngine, RunConfiguration, SimulationConfig, ValidatedNativeInput, check_progress,
    default_context, default_environment, entry_path, run_observed_snapshot, with_compiler_stack,
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
    let entry = entry_path(&context);
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
                    assert_eq!(error.stage, StageName::Launch, "{name}");
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
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("unavailable.hir", "resource type Token { n: u8 }\npub fn main(rt: Io, args: Args) -> Result[(), GrantError] { let tasks = io.tasks(rt)?; let store: Mutex[Token] = mutex.create(tasks, Token(n: 1u8)); Ok(()) }".into()).unwrap();
        let progress = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(progress.diagnostics.is_empty());
        assert_eq!(
            progress.completion,
            hirsel_engine::CheckCompletion::Complete
        );
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let Err(error) = build(&input, &Options::new(x86())) else {
            panic!("a Mutex of a resource requires the native M2 Mutex")
        };
        assert!(
            (StageName::Import..=StageName::Ready).contains(&error.stage),
            "{error}"
        );
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

/// Every operation the runtime classifies as a runtime primitive names only
/// symbols exported at the Hirsel ABI, and each such call a program makes has
/// exactly its export's physical signature, on both targets.
#[test]
fn every_runtime_primitive_row_matches_its_export() {
    use hirsel_core_contract::{Disposition, Engine as ManifestEngine};
    use hirsel_lir::layout_types::ScalarKind;
    use hirsel_rt::operations::HIRSEL_ABI_EXPORTS;
    with_compiler_stack(|| {
        let program = checked(HELLO);
        let manifest = hirsel_driver::native::manifest(program.validated.snapshot()).unwrap();
        let mut primitives = 0;
        for row in manifest.rows() {
            let Disposition::RuntimePrimitive(template) =
                row.dispositions.get(ManifestEngine::Native)
            else {
                continue;
            };
            primitives += 1;
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
                    HIRSEL_ABI_EXPORTS.iter().any(|e| e.symbol == symbol),
                    "{symbol} has no Hirsel ABI export row"
                );
            }
        }
        assert_eq!(
            primitives, 152,
            "Io, Args, Env, Test, host streams, text, scalar/position cursors, network streams, listeners and datagrams, processes, streaming children and groups, random draws, timer waits, and the task (scope deadlines and task sets included), channel, mutex, atomic and isolation runtime entries"
        );
        // Calls of each, with each float family, on both targets.
        let calls = checked(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\n  let xs: Array[f64] = [1.0f64, 2.0f64]\n  let ys: Array[f32] = [1.0f32, 2.0f32]\n  let a = std@1/f64.sum(xs) + std@1/f64.sum_exact(xs)\n  let b = std@1/f32.sum(ys) + std@1/f32.sum_exact(ys)\n  let text = std@1/u64.to_string(4u64)\n  let decoded = string.from_bytes(b\"abc\")\n  let replaced = std@1/bytes.replace(b\"abc\", b\"b\", b\"x\")\n  for pos in string.positions(text) {\n    let part = string.slice(text, pos, pos)\n  }\n  let _ = out.print(io.stdout(rt)?, \"x\")\n  Ok(())\n}\n",
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

/// The GEN-5 verdict and its sites depend only on the edge set: contributing
/// the instance edges in the order each of the 22 partitions groups their
/// owners gives the same growing sites, for a finite program (none) and for a
/// growing one (its one site).
#[test]
fn partitions_preserve_the_gen5_verdict_and_sites() {
    use hirsel_engine::InstanceGraph;
    let growing = "fn first[T: copyable](x:T) { second((copy x,copy x)) }
fn second[U: copyable](x:U) { third(x) }
fn third[V: copyable](x:V) { first(x) }
";
    with_compiler_stack(move || {
        let finite = checked(GENERIC).validated.snapshot().instances.clone();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("growing.hir", growing.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.diagnostics[0].rule.as_str(), "GEN-5");
        let growing = engine.gen5_source_graph().unwrap();
        for (graph, sites) in [(finite, 0), (growing, 1)] {
            let expected = graph.growing_sites();
            assert_eq!(expected.len(), sites);
            let mut owners: Vec<&DefPath> = graph.edges.iter().map(|e| &e.owner).collect();
            owners.sort();
            owners.dedup();
            for configuration in topology_matrix(20) {
                let Engine::Native { partition, .. } = configuration.engine else {
                    unreachable!()
                };
                let objects = partition.assign(owners.len());
                let mut edges = graph.edges.clone();
                edges.sort_by_key(|e| objects[owners.binary_search(&&e.owner).unwrap()]);
                let reordered = InstanceGraph { edges };
                assert_eq!(reordered.growing_sites(), expected, "{}", partition.name());
            }
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
            let record =
                hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap())
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
                let record =
                    hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap())
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
        let record =
            hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap()).unwrap();
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
                let record =
                    hirsel_rt::launch::parse_recording(&std::fs::read_to_string(&tape).unwrap())
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
