//! Native list mutation and consuming loops: reference observations, independent
//! heap audits on every injected failure, and executed native targets.
use crate::rc_support as support;

use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    QueryEngine, RunConfiguration, check_progress, default_context, default_environment,
    entry_path, run_observed_snapshot, with_compiler_stack,
};
use hirsel_heapcheck::{Config, NoOperations, Observed};
use hirsel_interp::observation::ObservedNode;
use hirsel_lir::ir::{Callee, Inst, LirUnit, ProjectionStep, Terminator, UnitOrigin};
use hirsel_lir::layout_types::LayoutTable;
use hirsel_lir::stage::Ready;
use hirsel_lir::validate::Inputs;
use hirsel_rc::cleanup::Finalizers;
use hirsel_rc::insert::Proofs;
use hirsel_rc::instance::InstanceId;
use hirsel_rc::placement::Options;

use support::*;

/// `main` returns `value()` as its `Err` payload, so both executors report
/// it. It goes last, so the lines above keep their numbers.
const ENTRY: &str = "pub fn main() -> Result[(), u64] {\n  Error(value())\n}\n";

/// A weighted sum of a list's elements and its length: two lists agree on
/// it only when they hold the same elements in the same order.
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

struct Built {
    table: LayoutTable,
    units: Vec<LirUnit<Ready>>,
    statics: Vec<hirsel_lir::statics::StaticData>,
}

/// Every stage to Ready; C accepts every unit at every stage it reaches.
fn build(source: &str) -> Built {
    let program = compile(source);
    let lowered = hirsel_rc::import::lower(&program, &target())
        .unwrap_or_else(|e| panic!("{source}\nlower: {e:?}"));
    let table = lowered.table.clone();
    let manifest = manifest(&program);
    let target = target();
    let inputs = Inputs {
        operations: &program.operation_registry(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let checked =
        |stage: &str, unit: &dyn Fn() -> Vec<hirsel_lir::validate::Violation>, d: String| {
            let v = unit();
            assert!(v.is_empty(), "{source}\n{stage}\n{d}\n{v:#?}");
        };
    let mut rc = Vec::new();
    for (i, u) in lowered.units.iter().enumerate() {
        checked(
            "value",
            &|| hirsel_lir::validate::check(u, &inputs),
            dump(u),
        );
        let id = InstanceId(i as u32);
        let options = Options::for_instance(true, &lowered.instances, id);
        let placed = hirsel_rc::placement::place(u.clone(), &table, &options).unwrap();
        let proofs = Proofs::for_instance(&lowered.instances, id);
        let r = hirsel_rc::insert::insert(placed, &table, &proofs)
            .unwrap_or_else(|e| panic!("{source}\ninsert: {e:?}"));
        checked("rc", &|| hirsel_lir::validate::check(&r, &inputs), dump(&r));
        rc.push(hirsel_rc::elide::elide(r, &table).unwrap());
    }
    let finalizers = Finalizers::new(&lowered.instances, &rc);
    let units: Vec<_> = rc
        .into_iter()
        .map(|u| {
            let d = dump(&u);
            hirsel_rc::ready::ready(u, &table, &finalizers)
                .unwrap_or_else(|e| panic!("{source}\n{d}\nready: {e:?}"))
        })
        .collect();
    for u in &units {
        checked(
            "ready",
            &|| hirsel_lir::validate::check(u, &inputs),
            dump(u),
        );
    }
    Built {
        table,
        units,
        statics: lowered.statics,
    }
}

fn heap_program(b: &Built) -> hirsel_heapcheck::Program<Ready> {
    hirsel_heapcheck::Program {
        target: target(),
        layouts: b.table.clone(),
        units: b.units.clone(),
        statics: b.statics.clone(),
    }
}

/// H's returned value of `main`, with a clean audit on the clean run and,
/// with `campaign`, on every fault-injected path (one rerun per site, so a
/// long loop runs clean only).
fn heap(source: &str, b: &Built, campaign: bool) -> u64 {
    let program = heap_program(b);
    let fail = |site: String, v: String| -> ! {
        let dumps: Vec<String> = b.units.iter().map(dump).collect();
        panic!("{source}\nsite {site}: {v}\n{}", dumps.join("\n"));
    };
    let run = if campaign {
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        if let Some((site, v)) = campaign.first_violation() {
            fail(format!("{site:?}"), v.to_string());
        }
        campaign.clean
    } else {
        hirsel_heapcheck::run(
            &program,
            &path("main"),
            &[],
            &mut NoOperations,
            &Config::default(),
        )
    };
    let observation = run
        .result
        .unwrap_or_else(|v| fail("clean".into(), v.to_string()))
        .observation;
    assert!(
        observation.failures.is_empty(),
        "{source}\n{:?}",
        observation.failures
    );
    match observation.returned {
        Some(Observed::Variant { fields, .. }) => match fields.into_iter().next().flatten() {
            Some(Observed::Integer { bits, .. }) => bits as u64,
            other => panic!("{source}\nreturned {other:?}"),
        },
        other => panic!("{source}\nreturned {other:?}"),
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
        let _ = compile(&source);
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

/// The generated library bodies.
fn library_units(b: &Built) -> Vec<&LirUnit<Ready>> {
    b.units
        .iter()
        .filter(|u| u.header.origin == UnitOrigin::Library)
        .collect()
}

fn agree(body: &str) -> Built {
    agree_with(body, true)
}

fn agree_with(body: &str, campaign: bool) -> Built {
    let source = source(body);
    let b = build(&source);
    assert_eq!(
        library_units(&b).len(),
        2,
        "{source}\npush and growth instances expected"
    );
    let h = heap(&source, &b, campaign);
    let i = reference(&source);
    assert_eq!(h, i, "{source}\nH disagrees with the interpreter");
    b
}

/// Two u64 elements take 24 payload bytes of a 32-byte class: room for one.
const ROOM: &str = "fn value() -> u64 {
  let mut xs: List[u64] = []
  list.push(mut xs, 5)
  list.push(mut xs, 6)
  list.push(mut xs, 7)
  sum(xs)
}
";

/// A second holder makes the list shared before the third push.
const SHARED: &str = "fn value() -> u64 {
  let mut xs: List[u64] = []
  list.push(mut xs, 1)
  list.push(mut xs, 2)
  let ys = copy xs
  list.push(mut xs, 3)
  sum(xs) * 1000 + sum(ys)
}
";

/// Literal lists are static blocks.
const LITERAL: &str = "fn value() -> u64 {
  let mut xs: List[u64] = [4, 5, 6]
  list.push(mut xs, 7)
  let mut ys: List[u64] = [8]
  list.push(mut ys, 9)
  sum(xs) * 1000 + sum(ys)
}
";

/// Counted elements move into the list; a copy shares them.
const BOXED: &str = "type P {
  a: u64,
  b: u64,
}

fn value() -> u64 {
  let mut xs: List[box P] = []
  let mut i: u64 = 0
  while i < 20 {
    list.push(mut xs, box P(a: i, b: i * i))
    i = i + 1
  }
  let ys = copy xs
  list.push(mut xs, box P(a: 100, b: 1))
  list.len(xs) * 1000 + list.len(ys)
}
";

/// `n` pushes onto an empty list.
fn grow(n: u64) -> String {
    format!(
        "fn value() -> u64 {{
  let mut xs: List[u64] = []
  let mut i: u64 = 0
  while i < {n} {{
    list.push(mut xs, i * 3)
    i = i + 1
  }}
  sum(xs)
}}
"
    )
}

#[test]
fn list_push_appends_in_place_when_the_list_has_room() {
    agree(ROOM);
}

#[test]
fn list_push_grows_the_list_across_size_classes() {
    // Three growths with every failure site injected, then five more run
    // clean (H recounts every block after each step, so a long run is slow;
    // the large-block path is the native tests').
    agree(&grow(12));
    agree_with(&grow(60), false);
}

#[test]
fn list_push_detaches_a_shared_list_and_leaves_the_copy_alone() {
    agree(SHARED);
}

#[test]
fn list_push_detaches_a_literal_list() {
    agree(LITERAL);
}

#[test]
fn list_push_moves_boxed_elements_and_releases_each_once() {
    agree(BOXED);
}

/// The push body states writes through `Slot` steps and the sequence
/// instructions, and no count operation of its own: P4 adds the uniqueness
/// test before its first write, as for a user's `xs[i] = v`.
#[test]
fn list_push_body_writes_through_slots_and_p4_detaches_it() {
    let b = agree(
        "fn value() -> u64 {
  let mut xs: List[u64] = []
  list.push(mut xs, 5)
  sum(xs)
}
",
    );
    let probe = library_units(&b)
        .into_iter()
        .find(|u| u.header.definition == hirsel_syntax::DefPath::core("list.push"))
        .unwrap();
    let insts = &probe.instructions;
    assert!(library_units(&b).iter().any(|u| {
        u.instructions
            .iter()
            .any(|i| matches!(i, Inst::SeqReserve { .. }))
    }));
    assert!(insts.iter().any(|i| matches!(i, Inst::SeqSetLen { .. })));
    assert!(insts.iter().any(|i| matches!(i, Inst::IsUnique { .. })));
    assert!(
        probe
            .pools
            .projections
            .iter()
            .any(|s| matches!(s, ProjectionStep::Slot(_)))
    );
    // Its caller calls it directly: no operation call of `list.push` remains.
    let caller = b
        .units
        .iter()
        .find(|u| u.header.definition == path("value"))
        .unwrap();
    let direct = caller.terminators.iter().any(|t| match *t {
        Terminator::Call { call, .. } => matches!(
            caller.tables.calls[call.index()].callee,
            Callee::Direct(s) if caller.tables.symbols[s.index()].definition == hirsel_syntax::DefPath::core("list.push")
        ),
        _ => false,
    });
    assert!(direct, "{}", dump(caller));
}

/// `list.len`, `array.len` and `string.byte_len` are an inline `SeqLen`.
#[test]
fn length_operations_import_as_seq_len() {
    let source = source(
        "fn value() -> u64 {
  let xs: List[u64] = [1, 2, 3]
  let s: String = \"hello\"
  list.len(xs) * 100 + string.byte_len(s)
}
",
    );
    let b = build(&source);
    let value = b
        .units
        .iter()
        .find(|u| u.header.definition == path("value"))
        .unwrap();
    let lengths = value
        .instructions
        .iter()
        .filter(|i| matches!(i, Inst::SeqLen { .. }))
        .count();
    assert_eq!(lengths, 2, "{}", dump(value));
    assert!(
        !value
            .terminators
            .iter()
            .any(|t| matches!(t, Terminator::Call { .. })),
        "{}",
        dump(value)
    );
    assert_eq!(heap(&source, &b, true), reference(&source));
}

/// The push body native on every executed target, unoptimized and
/// optimized, in one object and in one object per unit: each executable
/// observes exactly what the interpreter's `list.push` does and leaves no
/// block live (the launcher's audit). Native runs need the pinned toolchain,
/// so `tools/check-native.sh` runs it.
#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn list_push_agrees_natively_on_both_targets() {
    use hirsel_driver::native::{
        Observation, OptLevel, Options, Partition, Sites, TargetSpec, Toolchain, build_program,
        launch, link, manifest, observe, program,
    };
    use hirsel_engine::SimulationConfig;
    let targets = TargetSpec::executed();
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("library-native");
    let toolchain =
        Toolchain::in_tree_diagnostic(&dir.join("runtime"), &targets.iter().collect::<Vec<_>>())
            .unwrap();
    let growth = grow(600);
    let mut cases: Vec<_> = [
        ("room", ROOM),
        ("shared", SHARED),
        ("literal", LITERAL),
        ("boxed", BOXED),
        ("growth", growth.as_str()),
    ]
    .into_iter()
    .map(|(name, body)| {
        (
            name,
            source(body).replace(
                ENTRY,
                "pub fn main(rt: Io, args: Args) -> Result[(), u64] {\n  Error(value())\n}\n",
            ),
        )
    })
    .collect();
    cases.push(("resources", RESOURCE_PUSH.to_owned()));
    cases.push(("consume-constant", CONST_CONSUME.to_owned()));
    cases.push((
        "cross-definition-reuse",
        include_str!(
            "../../../../conformance/DROP/DROP-2-failure-inner-scope-before-cursor-run.hir"
        )
        .to_owned(),
    ));
    cases.push(("consume-break", consume_resources("break", false)));
    cases.push((
        "consume-fail",
        consume_resources("let _ = 1 / x.zero", false),
    ));
    cases.push((
        "consume-drop-fail",
        consume_resources("let _ = 1 / x.zero", true),
    ));
    cases.push(("consume-literal", consume_plain("List[u64]", false, false)));
    cases.push(("consume-shared", consume_plain("List[u64]", true, false)));
    cases.push((
        "consume-shared-break",
        consume_plain("List[u64]", true, true),
    ));
    cases.push(("consume-array", consume_plain("Array[u64]", false, false)));
    cases.push((
        "consume-inline",
        consume_plain("Inline[u64,3]", false, false),
    ));
    for (name, source) in cases {
        let toolchain = &toolchain;
        let targets = &targets;
        let dir = dir.join(name);
        with_compiler_stack(move || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("test.hir", source.clone()).unwrap();
            check_progress(&mut engine, file, &default_context()).unwrap();
            let entry = entry_path(&default_context());
            let config = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let run =
                run_observed_snapshot(&mut engine, &config, SimulationConfig::default()).unwrap();
            let reference = Observation::of_reference(
                run.observation
                    .as_ref()
                    .unwrap_or_else(|| panic!("{source}\n{run:?}")),
            );
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let sites = Sites::new(validated.snapshot());
            let compiled = program(&validated);
            for target in targets {
                let rows = manifest(validated.snapshot()).unwrap();
                let mut ready = build_program(&compiled, rows, &Options::new(target.clone()))
                    .unwrap_or_else(|e| panic!("{name} on {}: {e}", target.triple));
                ready.bind_revisions(&validated).unwrap();
                assert!(
                    ready
                        .units
                        .iter()
                        .any(|u| u.unit().header.origin == UnitOrigin::Library),
                    "{name}: no library unit"
                );
                for opt in [OptLevel::None, OptLevel::Speed] {
                    for partition in [Partition::Whole, Partition::Singleton] {
                        let what =
                            format!("{name} on {} {opt:?} {}", target.triple, partition.name());
                        let _ = std::fs::remove_dir_all(&dir);
                        std::fs::create_dir_all(&dir).unwrap();
                        let observed = link(&ready, &entry, opt, partition, toolchain, &dir)
                            .and_then(|exe| launch(&exe, &ready.target, toolchain, &dir, true))
                            .and_then(|launched| observe(&launched, &ready, &sites))
                            .unwrap_or_else(|e| panic!("{what}: {e}"));
                        assert_eq!(
                            observed,
                            reference,
                            "{what}:\n{}\nreference:\n{}",
                            observed.render(),
                            reference.render()
                        );
                    }
                }
            }
        });
    }
}

/// Use the production pipeline and Io model, including every injected failure
/// site's terminal audit, for programs whose finalizers make ownership visible.
fn observed_heap(source: &str) -> hirsel_driver::native::HeapRun {
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source.clone()).unwrap();
        check_progress(&mut engine, file, &default_context()).unwrap();
        let entry = entry_path(&default_context());
        let config = RunConfiguration {
            entry: entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let run = run_observed_snapshot(&mut engine, &config, Default::default()).unwrap();
        let reference = hirsel_driver::native::Observation::of_reference(
            run.observation
                .as_ref()
                .unwrap_or_else(|| panic!("{source}\n{run:?}")),
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let sites = hirsel_driver::native::Sites::new(validated.snapshot());
        let ready = hirsel_driver::native::build(
            &validated,
            &hirsel_driver::native::Options::new(target()),
        )
        .unwrap_or_else(|e| panic!("{source}\n{e}"));
        let heap = hirsel_driver::native::heap_campaign(&ready, &entry, &sites)
            .unwrap_or_else(|e| panic!("{source}\n{e}"));
        assert_eq!(heap.observation, reference, "{source}");
        heap
    })
}

const RESOURCE_PUSH: &str = r#"resource type Trace { stdout: Stdout, text: String }
drop(x) { let _ = out.print(x.stdout, x.text) }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let stdout = io.stdout(rt)?
  let mut xs: List[Trace] = []
  list.push(mut xs, Trace(stdout: stdout, text: "a\n"))
  list.push(mut xs, Trace(stdout: stdout, text: "b\n"))
  list.push(mut xs, Trace(stdout: stdout, text: "c\n"))
  list.push(mut xs, Trace(stdout: stdout, text: "d\n"))
  Ok(())
}
"#;

#[test]
fn pushing_resources_moves_and_finalizes_each_once() {
    assert_eq!(
        observed_heap(RESOURCE_PUSH).observation.output,
        b"d\nc\nb\na\n"
    );
}

#[test]
fn forgetting_suppresses_finalization_and_owned_field_cleanup() {
    let source = r#"resource type Trace { stdout: Stdout, text: String }
drop(x) { let _ = out.print(x.stdout, "abort\n") }
fn commit(take x: Trace) { forget(take x) }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let s = io.stdout(rt)?
  commit(Trace(stdout: s, text: "owned"))
  Ok(())
}
"#;
    assert!(observed_heap(source).observation.output.is_empty());
    let (_, b) = lower(source);
    assert!(b.units.iter().any(|u| {
        u.instructions
            .iter()
            .any(|i| matches!(i, Inst::Forget { .. }))
    }));
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn a_million_appends_have_logarithmic_allocations_and_linear_relocation() {
    use hirsel_driver::native::*;
    let source = r#"pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let mut xs: List[u64] = []
  let mut i: u64 = 0
  while i < 1000000 {
    list.push(mut xs, i)
    i += 1
  }
  if list.len(xs) == 1000000 && xs[999999] == 999999 { Ok(()) } else { Error(()) }
}
"#;
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("list-million");
    std::fs::create_dir_all(&dir).unwrap();
    let target = target();
    let toolchain = Toolchain::in_tree_diagnostic(&dir.join("runtime"), &[&target]).unwrap();
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("million.hir", source.to_string())
        .unwrap();
    check_progress(&mut engine, file, &default_context()).unwrap();
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let entry = entry_path(&default_context());
    let ready = hirsel_driver::native::build(
        &validated,
        &hirsel_driver::native::Options::new(target.clone()),
    )
    .unwrap();
    let wrapper = dir.join("count.c");
    std::fs::write(&wrapper, r#"#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
static uint64_t grows, relocated;
hirsel_rt_v2_header* __real_hirsel_rt_v2_seq_grow(hirsel_rt_v2_ctx*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint64_t);
hirsel_rt_v2_header* __wrap_hirsel_rt_v2_seq_grow(hirsel_rt_v2_ctx* ctx, hirsel_rt_v2_header* block, uint64_t size, uint64_t align, uint64_t capacity) {
  ++grows;
  relocated += hirsel_rt_v2_capacity(block);
  return __real_hirsel_rt_v2_seq_grow(ctx, block, size, align, capacity);
}
typedef uint8_t (*entry_fn)(hirsel_rt_v2_ctx*, uint8_t*, uint8_t*, const uint64_t*, const uint64_t*);
int32_t __wrap_hirsel_rt_v2_launch(entry_fn entry, uint64_t result_size, const hirsel_rt_v2_layout *const *layouts, uint32_t layout_count, const hirsel_rt_v2_launch_config *launch) {
  hirsel_rt_v2_config config = {.abi_version=HIRSEL_RT_V2_ABI_VERSION, .flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC, .layouts=layouts, .layout_count=layout_count, .hash_seed=launch->hash_seed};
  hirsel_rt_v2_ctx* ctx = NULL;
  assert(hirsel_rt_v2_ctx_create(&config, &ctx) == 0);
  void* result = calloc(1, result_size);
  uint64_t io = 1, args = 2;
  assert(entry(ctx, NULL, result, &io, &args) == 0);
  assert(*(uint32_t*)result == 0);
  hirsel_rt_v2_stats stats;
  hirsel_rt_v2_ctx_stats(ctx, &stats);
  assert(grows > 0 && grows <= 24);
  assert(stats.backing_allocations <= 40);
  assert(relocated <= 32000000);
  assert(stats.live_blocks == 0);
  hirsel_rt_v2_audit audit;
  hirsel_rt_v2_task_exit(ctx, &audit);
  assert(audit.live_blocks == 0 && audit.active_regions == 0 && audit.retained_bytes == 0);
  printf("list-counts %llu %llu %llu\n", (unsigned long long)grows, (unsigned long long)stats.backing_allocations, (unsigned long long)relocated);
  free(result);
  hirsel_rt_v2_ctx_destroy(ctx);
  return 0;
}
"#).unwrap();
    for opt in [OptLevel::None, OptLevel::Speed] {
        let exe = link(&ready, &entry, opt, Partition::Whole, &toolchain, &dir).unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include");
        let mut command = std::process::Command::new(&toolchain.clang);
        command
            .arg(&wrapper)
            .arg("-DHIRSEL_RT_ENABLE_STATS")
            .arg("-I")
            .arg(root)
            .arg(dir.join("main.o"));
        let mut objects: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("unit-")
                    && p.extension().is_some_and(|e| e == "o")
            })
            .collect();
        objects.sort();
        command
            .args(objects)
            .arg(toolchain.archive(&target.triple).unwrap())
            .args([
                "-Wl,--wrap=hirsel_rt_v2_seq_grow",
                "-Wl,--wrap=hirsel_rt_v2_launch",
                "-lgcc_s",
                "-lutil",
                "-lrt",
                "-lpthread",
                "-lm",
                "-ldl",
                "-o",
            ])
            .arg(&exe);
        let linked = command.output().unwrap();
        assert!(
            linked.status.success(),
            "{}",
            String::from_utf8_lossy(&linked.stderr)
        );
        let run = std::process::Command::new(&exe).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let counts = String::from_utf8(run.stdout).unwrap();
        assert!(counts.starts_with("list-counts "), "{counts}");
        eprintln!("{opt:?}: {counts}");
    }
}

#[test]
fn consuming_cursors_record_uniqueness_where_their_transfer_needs_it() {
    use hirsel_layout::layout::OwnerRepresentation;
    use hirsel_types::{Primitive, TypeKind};
    let registry = hirsel_engine::registry::BuiltinRegistry::new();
    let signatures = registry
        .signatures()
        .map(|s| (s.definition.clone(), s.clone()))
        .collect();
    let owners = hirsel_rc::library::owners(&signatures);
    for (name, count) in [
        ("OwnedCursor", 3),
        ("OwnedMapCursor", 3),
        ("OwnedSortedMapCursor", 3),
    ] {
        let OwnerRepresentation::Defined(record) =
            &owners.nominals[&hirsel_syntax::DefPath::core(&format!("$intrinsic.{name}"))]
        else {
            panic!("cursor representation")
        };
        let TypeKind::Record(fields) = &record.types.get(record.private_shape).unwrap().kind else {
            panic!("cursor record")
        };
        assert_eq!(fields.len(), count);
        if count == 3 {
            assert_eq!(fields[2].name, "unique");
            assert_eq!(
                record.types.get(fields[2].ty).unwrap().kind,
                TypeKind::Primitive(Primitive::Bool)
            );
        }
    }
}

/// A hand-built resource with a counted field. Its library finalizer mutates
/// another field, so the later field release must use the call's new version.
fn handmade_finalizer(library: bool, wrong_convention: bool) {
    use hirsel_core_contract::Passing;
    use hirsel_heapcheck::build::{ANCHOR, CLEANUP, Layouts, fixture_unit};
    use hirsel_lir::abi_types::*;
    use hirsel_lir::ir::*;
    use hirsel_lir::layout_types::*;
    use hirsel_lir::stage::{Rc, StageKind};
    use hirsel_lir::testing::UnitBuilder;
    use hirsel_rc::cleanup::Finalizer;
    use std::collections::BTreeMap;
    let mut layouts = Layouts::new(target());
    let scalar = layouts.scalar(ScalarKind::U64);
    let empty = layouts.record(&[]);
    let block_layout = layouts.block(empty);
    let finalizer_path = path("drop_fixture");
    let resource = layouts.resource(&[block_layout, scalar], Some(finalizer_path.clone()));
    let (_, mut table) = layouts.finish();
    let mut abi = table.fn_abis[0].clone();
    let mut passing = if library {
        Passing::Mut
    } else {
        Passing::Borrow
    };
    if wrong_convention {
        passing = Passing::Borrow;
    }
    abi.parameters = vec![AbiParameter {
        source: 0,
        convention: passing,
        passing: if passing == Passing::Mut {
            ParameterPassing::ExclusiveAddress
        } else {
            ParameterPassing::ReadOnlyAddress
        },
        layout: resource,
        slots: vec![PhysicalSlot {
            ty: PhysicalType::Pointer,
            size: 8,
            align: 8,
        }],
        extension: ScalarExtension::None,
    }];
    table.fn_abis.push(abi);
    let mut f = UnitBuilder::<Rc>::new(fixture_unit(finalizer_path.clone()));
    f.unit.header.stage = StageKind::Rc;
    f.unit.header.abi = FnAbiId(1);
    f.unit.header.origin = if library {
        UnitOrigin::Library
    } else {
        UnitOrigin::Source
    };
    let body = f.block(BlockKind::Body, &[]);
    f.unit.entry = body;
    let failure_body = f.block(BlockKind::FailureCleanup, &[]);
    f.term(failure_body, Terminator::Propagate);
    let e = f.edge(failure_body, &[]);
    let failure = f.failure(e, None, &[]);
    let self_value = f.plain(ValueRepr::Aggregate, resource);
    f.parameter(self_value);
    let self_slot = f.slot(ValueRepr::Aggregate, resource, SlotKind::Parameter(0));
    let whole = f.address(
        AddressBase::Slot(self_slot),
        &[],
        ValueRepr::Aggregate,
        resource,
    );
    f.push(
        body,
        Inst::Address {
            address: whole,
            failure,
        },
    );
    if library {
        let field = f.address(
            AddressBase::Slot(self_slot),
            &[ProjectionStep::Field(1)],
            ValueRepr::Scalar(ScalarKind::U64),
            scalar,
        );
        let e = f.edge(failure_body, &[]);
        let failure = f.failure(e, None, &[]);
        f.push(
            body,
            Inst::Address {
                address: field,
                failure,
            },
        );
        let seven = f.plain(ValueRepr::Scalar(ScalarKind::U64), scalar);
        let constant = f.constant(
            ValueRepr::Scalar(ScalarKind::U64),
            scalar,
            ConstantValue::Integer(7),
        );
        f.push(
            body,
            Inst::Const {
                result: seven,
                constant,
            },
        );
        let version = f.plain(ValueRepr::Aggregate, resource);
        let displaced = f.slot(
            ValueRepr::Scalar(ScalarKind::U64),
            scalar,
            SlotKind::Temporary,
        );
        f.push(body, Inst::StorageLive { slot: displaced });
        let displaced_version = f.plain(ValueRepr::Scalar(ScalarKind::U64), scalar);
        let e = f.edge(failure_body, &[]);
        let failure = f.failure(e, None, &[]);
        f.push(
            body,
            Inst::Replace {
                address: field,
                value: seven,
                displaced,
                version,
                displaced_version,
                if_live: false,
                failure,
            },
        );
        f.push(
            body,
            Inst::StorageDead {
                slot: displaced,
                if_live: false,
            },
        );
    }
    f.term(body, Terminator::Return(None));
    let finalizer = f.finish_with_layouts(&table);
    let mut b = UnitBuilder::<Rc>::new(fixture_unit(path("main")));
    b.unit.header.stage = StageKind::Rc;
    let prologue = b.block(BlockKind::Body, &[]);
    let entry = b.block(BlockKind::Body, &[]);
    b.unit.entry = prologue;
    let propagated = b.block(BlockKind::FailureCleanup, &[]);
    b.term(propagated, Terminator::Propagate);
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    let slot = b.slot(ValueRepr::Aggregate, resource, SlotKind::Local);
    b.push(prologue, Inst::StorageLive { slot });
    let address = b.address(AddressBase::Slot(slot), &[], ValueRepr::Aggregate, resource);
    b.push(prologue, Inst::Address { address, failure });
    let e = b.edge(entry, &[]);
    b.term(prologue, Terminator::Jump(e));
    let record_symbol = b.symbol(SymbolKind::Type, path("record"));
    let storage = b.owned(ValueRepr::Block, block_layout);
    let heap = Placement::Fixed(StorageOwner::Heap(TaskDomainId(0)));
    let construct = b.construct(
        ConstructorKind::Record(record_symbol),
        empty,
        &[],
        heap.clone(),
    );
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    b.push(
        entry,
        Inst::Construct {
            result: storage,
            construct,
            failure,
        },
    );
    let zero = b.plain(ValueRepr::Scalar(ScalarKind::U64), scalar);
    let constant = b.constant(
        ValueRepr::Scalar(ScalarKind::U64),
        scalar,
        ConstantValue::Integer(0),
    );
    b.push(
        entry,
        Inst::Const {
            result: zero,
            constant,
        },
    );
    let value = b.owned(ValueRepr::Aggregate, resource);
    let construct = b.construct(
        ConstructorKind::Record(record_symbol),
        resource,
        &[(0, storage), (1, zero)],
        heap,
    );
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    b.push(
        entry,
        Inst::Construct {
            result: value,
            construct,
            failure,
        },
    );
    let version = b.plain(ValueRepr::Aggregate, resource);
    b.push(
        entry,
        Inst::Store {
            address,
            value,
            version,
        },
    );
    let e = b.edge(propagated, &[]);
    let failure = b.failure(e, None, &[]);
    b.push(
        entry,
        Inst::Drop {
            address,
            if_live: false,
            anchor: CLEANUP,
            failure,
        },
    );
    b.term(entry, Terminator::Return(None));
    let caller = b.finish_with_layouts(&table);
    let finalizers = Finalizers(BTreeMap::from([(
        resource,
        Finalizer {
            origin: finalizer.header.origin,
            definition: finalizer_path,
            instance: finalizer.header.instance,
            abi: FnAbiId(1),
        },
    )]));
    let result = hirsel_rc::cleanup::lower(caller, &table, &finalizers);
    if wrong_convention {
        assert!(matches!(result, Err(hirsel_rc::PassError::Malformed(_))));
        return;
    }
    let caller = result.unwrap();
    let finalizer = hirsel_rc::cleanup::lower(finalizer, &table, &Finalizers::default()).unwrap();
    let manifest = hirsel_core_contract::Manifest::skeleton([]);
    let target = target();
    let inputs = Inputs {
        operations: &(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    for unit in [&caller, &finalizer] {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(violations.is_empty(), "{}\n{violations:#?}", dump(unit));
    }
    let call = caller
        .tables
        .calls
        .iter()
        .find(|c| matches!(c.callee, Callee::Direct(_)))
        .unwrap();
    let args = call.arguments.get(&caller.pools.call_arguments).unwrap();
    assert_eq!(args[0].passing, passing);
    assert_eq!(
        matches!(args[0].value, ArgumentValue::Address { .. }),
        library
    );
    let program = hirsel_heapcheck::Program {
        target,
        layouts: table,
        units: vec![caller, finalizer],
        statics: vec![],
    };
    let run = hirsel_heapcheck::run(
        &program,
        &path("main"),
        &[],
        &mut NoOperations,
        &Config::default(),
    );
    run.result.unwrap_or_else(|v| panic!("{v}"));
    assert_eq!(run.counters.finalizers, 1);
    let _ = ANCHOR;
}

#[test]
fn library_finalizers_take_mut_self_and_source_finalizers_borrow_self() {
    handmade_finalizer(true, false);
    handmade_finalizer(false, false);
    handmade_finalizer(true, true);
}

#[test]
fn consuming_list_elements_leave_the_cursor_before_returning_to_the_loop() {
    let source = source(
        "fn value() -> u64 { let xs: List[u64] = [1, 2, 3]; let mut n: u64 = 0; for x in take xs { n += x }; n }\n",
    );
    let b = build(&source);
    assert_eq!(heap(&source, &b, true), reference(&source));
}

fn consume_resources(exit: &str, failing_drop: bool) -> String {
    let finalizer = if failing_drop {
        "let _ = 1 / x.zero"
    } else {
        ""
    };
    format!(
        r#"resource type Trace {{ stdout: Stdout, text: String, zero: u64 }}
drop(x) {{ let _ = out.print(x.stdout, x.text); {finalizer} }}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let stdout = io.stdout(rt)?
  let mut xs: List[Trace] = []
  list.push(mut xs, Trace(stdout: stdout, text: "a\n", zero: 0))
  list.push(mut xs, Trace(stdout: stdout, text: "b\n", zero: 0))
  list.push(mut xs, Trace(stdout: stdout, text: "c\n", zero: 0))
  for x in take xs {{ {exit} }}
  Ok(())
}}
"#
    )
}

#[test]
fn consuming_break_failure_and_failed_finalizers_release_the_unreached_suffix_once() {
    for (exit, failing_drop) in [
        ("break", false),
        ("let _ = 1 / x.zero", false),
        ("let _ = 1 / x.zero", true),
    ] {
        let run = observed_heap(&consume_resources(exit, failing_drop));
        assert_eq!(run.observation.output, b"a\nc\nb\n");
        assert_eq!(run.counters.finalizers, 4); // three resources and their cursor
    }
}

fn consume_plain(kind: &str, shared: bool, early: bool) -> String {
    let share = if shared { "let original = copy xs" } else { "" };
    let check = if shared {
        "sum(original) == 17"
    } else {
        "true"
    };
    let stop = if early { "break" } else { "" };
    let wanted = if early { 1 } else { 6 };
    format!(
        r#"{SUM}
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{
  let xs: {kind} = [1,2,3]
  {share}
  let mut n: u64 = 0
  for x in take xs {{ n += x; {stop} }}
  let _ = out.print(io.stdout(rt)?, if n == {wanted} && {check} {{ "ok\n" }} else {{ "bad\n" }})
  Ok(())
}}
"#
    )
}

#[test]
fn consuming_shared_static_array_and_inline_sources_obeys_their_allocation_contract() {
    for (kind, shared, early) in [
        ("List[u64]", false, false),
        ("List[u64]", true, false),
        ("List[u64]", true, true),
        ("Array[u64]", false, false),
        ("Inline[u64,3]", false, false),
    ] {
        let run = observed_heap(&consume_plain(kind, shared, early));
        assert_eq!(run.observation.output, b"ok\n");
        // One source block, or Inline open's single array. Consuming it
        // adds no backing allocation on shared, unique or literal paths.
        assert_eq!(
            run.counters.allocations, 1,
            "{kind} shared={shared} early={early}"
        );
    }
}

#[test]
fn an_immortal_sequence_is_consumed_without_allocation_or_detach() {
    use hirsel_lir::ir::{Symbol, SymbolKind};
    let source = source(
        "fn value() -> u64 { let xs: List[u64] = [1,2,3]; let mut n: u64 = 0; for x in take xs { n += x }; n }\n",
    );
    let mut b = build(&source);
    let symbol = Symbol {
        kind: SymbolKind::Constant,
        definition: path("static_fixture"),
        instance: None,
    };
    let mut layout = None;
    for unit in b
        .units
        .iter_mut()
        .filter(|u| u.header.definition == path("value"))
    {
        let at = unit.instructions.iter().position(|i| matches!(i, Inst::Construct { construct, .. } if unit.tables.constructs[construct.index()].constructor == hirsel_lir::ir::ConstructorKind::Sequence)).unwrap();
        let Inst::Construct {
            result, construct, ..
        } = unit.instructions[at]
        else {
            unreachable!()
        };
        layout = Some(unit.tables.constructs[construct.index()].layout);
        let id = hirsel_lir::ir::SymbolId(unit.tables.symbols.len() as u32);
        unit.tables.symbols.push(symbol.clone());
        unit.instructions[at] = Inst::ReadStatic { result, symbol: id };
        *unit = hirsel_rc::insert::Editor::new(unit.clone()).finish();
    }
    let mut program = heap_program(&b);
    // Count 3, capacity 3 (runtime ABI 5), then the elements.
    let bytes = [3u64, 3, 1, 2, 3]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect();
    program.statics.push(hirsel_lir::statics::StaticData::plain(
        symbol,
        layout.unwrap(),
        bytes,
    ));
    let source_program = compile(&source);
    let manifest = manifest(&source_program);
    let target = target();
    let inputs = Inputs {
        operations: &source_program.operation_registry(),
        target: &target,
        layouts: &b.table,
        manifest: &manifest,
    };
    for unit in &b.units {
        let violations = hirsel_lir::validate::check(unit, &inputs);
        assert!(violations.is_empty(), "{}\n{violations:?}", dump(unit));
    }
    let campaign = hirsel_heapcheck::fault_campaign(
        &program,
        &path("main"),
        &[],
        &mut || Box::new(NoOperations),
        &Config::default(),
    );
    assert!(
        campaign.first_violation().is_none(),
        "{:?}",
        campaign.first_violation()
    );
    assert_eq!(campaign.clean.counters.allocations, 0);
    assert!(
        b.units
            .iter()
            .filter(|u| u.header.origin == UnitOrigin::Library)
            .all(|u| !u
                .instructions
                .iter()
                .any(|i| matches!(i, Inst::UniqueCopy { .. })))
    );
    assert_eq!(
        campaign.clean.result.unwrap().observation.returned,
        Some(Observed::Variant {
            tag: 1,
            fields: vec![Some(Observed::Integer {
                scalar: hirsel_lir::layout_types::ScalarKind::U64,
                bits: 6
            })]
        })
    );
}

fn consume_inline_resources() -> String {
    let source = consume_resources("break", false);
    let begin = source.find("  let mut xs:").unwrap();
    let end = source.find("  for x").unwrap();
    format!(
        "{}  let xs: Inline[Trace,3] = [Trace(stdout:stdout,text:\"a\\n\",zero:0),Trace(stdout:stdout,text:\"b\\n\",zero:0),Trace(stdout:stdout,text:\"c\\n\",zero:0)]\n{}",
        &source[..begin],
        &source[end..]
    )
}

#[test]
fn inline_resources_move_into_the_cursor_once() {
    let run = observed_heap(&consume_inline_resources());
    assert_eq!(run.observation.output, b"a\nc\nb\n");
    assert_eq!(run.counters.finalizers, 4);
    assert_eq!(run.counters.allocations, 1);
}

const CONSUME_SHARED_BOXES: &str = r#"type P { n: u64 }
pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {
  let xs: List[box P] = [box P(n:1), box P(n:2)]
  let original = copy xs
  let mut n: u64 = 0
  for x in take xs { n += 1 }
  let _ = out.print(io.stdout(rt)?, if n == 2 && list.len(original) == 2 { "ok\n" } else { "bad\n" })
  Ok(())
}
"#;
#[test]
fn a_shared_source_copies_each_counted_element_without_detaching() {
    let run = observed_heap(CONSUME_SHARED_BOXES);
    assert_eq!(run.observation.output, b"ok\n");
    assert_eq!(run.counters.allocations, 3);
    assert_eq!(run.counters.dups, 3);
}

fn inline_drop(failing: bool) -> String {
    let mut source = consume_inline_resources().replace("for x in take xs { break }", "");
    if failing {
        source = source.replace(
            "out.print(x.stdout, x.text);  }",
            "out.print(x.stdout, x.text); let _ = 1 / x.zero }",
        );
        source = source
            .replace("text:\"a\\n\",zero:0", "text:\"a\\n\",zero:1")
            .replace("text:\"c\\n\",zero:0", "text:\"c\\n\",zero:1");
    }
    source
}

#[test]
fn inline_cleanup_drains_untouched_resources_and_continues_after_the_middle_fails() {
    for failing in [false, true] {
        let run = observed_heap(&inline_drop(failing));
        assert_eq!(run.observation.output, b"c\nb\na\n");
        assert_eq!(run.counters.finalizers, 3);
    }
}

const CONST_CONSUME: &str = r#"const data: List[u64] = [1,2,3]
pub fn main(rt: Io, args: Args) -> Result[(),GrantError] {
  let xs = data
  let mut n: u64 = 0
  for x in take xs { n += x }
  let _ = out.print(io.stdout(rt)?, if n == 6 { "ok\n" } else { "bad\n" })
  Ok(())
}
"#;
#[test]
fn a_materialized_static_source_consumes_without_allocation() {
    let run = observed_heap(CONST_CONSUME);
    assert_eq!(run.observation.output, b"ok\n");
    assert_eq!(run.counters.allocations, 0);
}
