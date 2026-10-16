//! Scalar call results in optimized LLVM IR: a callee whose result is a plain
//! scalar returns `{ status, value }` in registers, so the erased stage loop of
//! `bench/native/programs/closure-iter-erased.hir` calls through its code
//! pointers with no out-cell, on both targets.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

/// The optimized IR of the definition whose link name contains every part of
/// `name`: a function-reference thunk (`_H1T`) only when `name` asks for one.
fn function_ir(source: &str, target: TargetSpec, name: &[&str]) -> String {
    let source = source.to_owned();
    let name: Vec<String> = name.iter().map(|s| s.to_string()).collect();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("scalar.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(&validated, &Options::new(target)).unwrap();
        let ir = emit_ir(&ready, OptLevel::Speed).unwrap();
        let start = ir
            .lines()
            .position(|l| {
                l.starts_with("define")
                    && name.iter().all(|n| l.contains(n.as_str()))
                    && (name.iter().any(|n| n.contains("_H1T")) || !l.contains("@_H1T"))
            })
            .unwrap_or_else(|| panic!("{name:?} is defined\n{ir}"));
        let lines: Vec<&str> = ir.lines().skip(start).collect();
        let end = lines.iter().position(|l| *l == "}").unwrap();
        lines[..=end].join("\n")
    })
}

fn targets() -> [TargetSpec; 2] {
    [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ]
}

/// The erased witness: a list of stages, each a function value and its
/// environment word, called through the list in a loop.
const ERASED: &str = "type Stage { call: fn(u64, u64) -> u64 uses {}, environment: u64 }
fn stage_add(value: u64, environment: u64) -> u64 { value + environment }
fn stage_multiply(value: u64, environment: u64) -> u64 { value * environment }
fn workload(n: u64) -> u64 {
  let stages: List[Stage] = [Stage(call: stage_add, environment: 3u64), Stage(call: stage_multiply, environment: 2u64)]
  let mut total: u64 = 0
  for i in 0u64..n {
    let value = stages[0u64].call(i, stages[0u64].environment)
    if value % 2u64 == 0u64 { total += stages[1u64].call(value, stages[1u64].environment) }
  }
  total
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  failure.check(workload(4u64) == 20u64, TextInvariant())
  Ok(())
}
";

/// Lines of `function` that call through a code pointer (`call ... %reg(`).
fn indirect_calls(function: &str) -> Vec<&str> {
    function
        .lines()
        .map(str::trim)
        .filter(|l| {
            l.contains("call ")
                && l.split_once("call ")
                    .and_then(|(_, rest)| rest.split_once('('))
                    .is_some_and(|(callee, _)| callee.contains(" %"))
        })
        .collect()
}

/// Stack slots whose address reaches a call other than the runtime allocator's
/// out-pointer: the result cells a scalar call would need without registers.
fn result_cells(function: &str) -> Vec<String> {
    let slots: Vec<&str> = function
        .lines()
        .filter_map(|l| l.trim().split_once(" = alloca ").map(|(name, _)| name))
        .collect();
    slots
        .into_iter()
        .filter(|slot| {
            function.lines().any(|l| {
                l.contains("call ")
                    && !l.contains("@hirsel_rt_v2_alloc(")
                    && l.split(|c: char| !(c.is_alphanumeric() || "%._".contains(c)))
                        .any(|word| word == *slot)
            })
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_erased_stage_call_returns_its_scalar_in_registers() {
    for target in targets() {
        let ir = function_ir(ERASED, target, &["8workload"]);
        let calls = indirect_calls(&ir);
        assert!(!calls.is_empty(), "the stage calls stay indirect\n{ir}");
        for call in &calls {
            assert!(
                call.contains("call { i8, i64 } %"),
                "a scalar result returns with its status\n{call}\n{ir}"
            );
        }
        // No out-cell: no stack slot's address reaches a call.
        assert_eq!(result_cells(&ir), Vec::<String>::new(), "{ir}");
    }
}

/// The function-reference thunk a stage's code pointer names returns the same
/// pair as the definition it forwards to.
#[test]
fn a_function_reference_thunk_returns_the_pair() {
    for target in targets() {
        let ir = function_ir(ERASED, target, &["@_H1T", "10stage__add"]);
        assert!(ir.contains("{ i8, i64 } @_H1T"), "{ir}");
        assert!(!ir.contains(" alloca "), "{ir}");
    }
}

/// The scoped witness (`closure-iter-scoped.hir`): captured closures passed to
/// `apply`, which LLVM inlines. Its loop keeps no result cell, and no returned
/// pair survives inlining: each return site's constant status reaches the
/// caller's test, so the status branches fold away as they did through a cell.
#[test]
fn a_scoped_closure_loop_folds_its_inlined_results() {
    let source = "fn apply(value: u64, f: fn(u64) -> u64 uses {}) -> u64 uses {f} { f(value) }
fn workload(n: u64) -> u64 {
  let offset: u64 = 3
  let factor: u64 = 2
  let add = fn(value: u64) -> u64 uses {} { value + offset }
  let multiply = fn(value: u64) -> u64 uses {} { value * factor }
  let mut total: u64 = 0
  for i in 0u64..n {
    let value = apply(i, add)
    if value % 2u64 == 0u64 { total += apply(value, multiply) }
  }
  total
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  failure.check(workload(4u64) == 20u64, TextInvariant())
  Ok(())
}
";
    for target in targets() {
        let ir = function_ir(source, target, &["8workload"]);
        assert_eq!(result_cells(&ir), Vec::<String>::new(), "{ir}");
        // The workload's own return builds a pair; no inlined one is merged or read.
        assert!(
            !ir.contains("phi { i8, i64 }") && !ir.contains("extractvalue { i8, i64 }"),
            "an inlined result pair survives\n{ir}"
        );
    }
}
