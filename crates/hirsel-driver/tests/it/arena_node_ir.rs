//! The binarytrees arena node in optimized LLVM IR: the construction of
//! `tree` and the traversal of `check` from the measured program, on both targets.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

const TREE: &str = "type Tree = Leaf | Branch(left: box Tree, right: box Tree)
fn tree(a: Arena, depth: u64) -> (box Tree) in a {
  if depth == 0u64 { box Leaf() }
  else { box Branch(left: tree(a, depth - 1u64), right: tree(a, depth - 1u64)) }
}
fn check(t: box Tree) -> u64 {
  match t { Leaf() => 1u64, Branch(left: l, right: r) => 1u64 + check(l) + check(r) }
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let count = arena.with(fn(a: Arena) -> u64 { check(tree(a, 16u64)) })
  let _ = count
  Ok(())
}
";

/// The optimized IR of the program's module on each target.
fn module_ir() -> Vec<String> {
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("tree.hir", TREE.to_owned()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ]
        .into_iter()
        .map(|target| {
            let ready = build(&validated, &Options::new(target)).unwrap();
            emit_ir(&ready, OptLevel::Speed).unwrap()
        })
        .collect()
    })
}

/// The optimized IR of each target's definition whose link name contains `name`.
fn function_ir(name: &'static str) -> Vec<String> {
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("tree.hir", TREE.to_owned()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ]
        .into_iter()
        .map(|target| {
            let ready = build(&validated, &Options::new(target)).unwrap();
            let ir = emit_ir(&ready, OptLevel::Speed).unwrap();
            let lines: Vec<&str> = ir
                .lines()
                .skip_while(|l| {
                    !(l.starts_with("define") && l.contains(name) && !l.contains("@_H1T"))
                })
                .collect();
            let end = lines
                .iter()
                .position(|l| *l == "}")
                .expect("the definition");
            lines[..=end].join("\n")
        })
        .collect()
    })
}

/// Each child pointer of a `Branch` is one aligned word store, never bytes and shifts.
#[test]
fn branch_children_are_word_stores() {
    for ir in function_ir("4tree") {
        assert!(!ir.contains("store i8"), "{ir}");
        assert!(!ir.contains("lshr"), "{ir}");
    }
}

/// A node is its eight-byte header and 16-byte payload, bumped once from the word-aligned
/// cursor with one limit test: no extent stores and no alignment masks.
#[test]
fn arena_node_is_one_bump_without_extent() {
    for ir in function_ir("4tree") {
        assert!(!ir.contains("store i64 16,"), "{ir}");
        assert!(!ir.contains("store i64 8,"), "{ir}");
        assert!(!ir.contains("and i64"), "{ir}");
        let bumps = ir
            .lines()
            .filter(|l| l.contains("= add i64 %") && l.ends_with(", 24"))
            .count();
        assert_eq!(bumps, 2, "{ir}");
    }
}

/// The traversal tests each node's tag once: the arm's fields are prepared where its
/// tag test proved the variant, so no MEM-13 check follows the left recursive call, and
/// the only failure starts are the two checked additions (SEM-1).
#[test]
fn check_tests_each_tag_once() {
    for ir in function_ir("5check") {
        assert!(!ir.contains("icmp eq ptr"), "{ir}");
        assert_eq!(
            ir.matches("@hirsel_rt_v2_failure_begin(").count(),
            2,
            "{ir}"
        );
    }
}

/// The arena token reaches each recursive `tree` call by value, as the callee's own
/// parameter word, never through a caller stack slot. Construction tests the
/// distinguished heap token before accessing the arena record.
#[test]
fn arena_token_is_a_word_with_a_heap_test() {
    for ir in function_ir("4tree") {
        let define = ir.lines().next().expect("the define line");
        let (_, parameters) = define.split_once('(').expect("the parameter list");
        let token = parameters
            .split(", ")
            .nth(2)
            .and_then(|p| p.rsplit(' ').next())
            .expect("the token parameter");
        let calls: Vec<&str> = ir
            .lines()
            .filter(|l| l.contains("call") && l.contains("4tree"))
            .collect();
        assert_eq!(calls.len(), 2, "{ir}");
        for call in calls {
            assert!(
                call.contains(&format!(", ptr {token}, i64")),
                "{call}\n{ir}"
            );
        }
        assert!(ir.contains(&format!("icmp eq ptr {token}, null")), "{ir}");
        assert!(ir.contains("@hirsel_rt_v2_alloc("), "{ir}");
    }
}

/// Allocation cannot fail (runtime ABI 3), so `tree` has no failure path: its recursive
/// calls' statuses fold away, and nothing reanchors a failure or releases a built child.
#[test]
fn tree_has_no_failure_path() {
    for ir in function_ir("4tree") {
        assert!(!ir.contains("icmp eq i8"), "{ir}");
        assert!(!ir.contains("@hirsel_rt_v2_failure_"), "{ir}");
        assert!(!ir.contains("@hirsel.release"), "{ir}");
        assert!(ir.contains("call ptr @hirsel_rt_v2_arena_alloc("), "{ir}");
    }
}

/// `arena.with` enters its arena warm (`hirsel_rt_v2_arena_enter`), seeded from the pool, so
/// the first `box` in it bumps inline; no arena region enters cold.
#[test]
fn an_arena_enters_warm() {
    for ir in module_ir() {
        assert!(ir.contains("call void @hirsel_rt_v2_arena_enter("), "{ir}");
        assert!(
            !ir.contains("call void @hirsel_rt_v2_region_enter("),
            "{ir}"
        );
    }
}
