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

/// Allocation cannot fail (runtime ABI 3), so `tree`'s only failure paths are its two
/// recursive calls' LANG-10 guards: real non-tail recursion keeps one admission per
/// recursive call, which fails at 1,024 frames. Each admission compares the
/// selected 16-bit counter to its allowance; the two successful entries and
/// their four normal/failure retirements store that counter. Each call status
/// remains an `icmp eq i8`, reanchored at its site. A failure after the left
/// child is built releases it. Nothing else fails or releases.
#[test]
fn tree_has_no_failure_path() {
    for ir in function_ir("4tree") {
        let count = |needle: &str| ir.matches(needle).count();
        assert_eq!(count("@hirsel_rt_v2_depth_enter_at_site("), 0, "{ir}");
        assert_eq!(count("icmp ult i16"), 2, "{ir}");
        assert_eq!(count("store i16"), 6, "{ir}");
        assert_eq!(count("@hirsel_rt_v2_depth_leave("), 0, "{ir}");
        assert_eq!(count("call i8 @hirsel_rt_v2_failure_begin("), 2, "{ir}");
        assert_eq!(count("@hirsel_rt_v2_failure_reanchor("), 2, "{ir}");
        assert_eq!(count("@hirsel_rt_v2_failure_"), 4, "{ir}");
        assert_eq!(count("icmp eq i8"), 2, "{ir}");
        assert_eq!(count("@hirsel.release"), 2, "{ir}");
        assert!(ir.contains("call ptr @hirsel_rt_v2_arena_alloc("), "{ir}");
    }
}

fn check_placement(source: &'static str, accepted: bool) {
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(
                "placement.hir",
                format!("type Tree = Leaf | Branch(left: box Tree, right: box Tree)\n{source}"),
            )
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        if accepted {
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
        } else {
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "PROV-4"),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn nested_box_payloads_keep_the_receiving_arena() {
    for source in [
        "fn wrap(a: Arena, take child: (box Tree) in a) -> (box (box Tree)) in a { box take child }",
        "fn wrap(a: Arena, take child: (box Tree) in a) -> (box (box (box Tree))) in a { box (box take child) }",
    ] {
        check_placement(source, true);
    }
}

#[test]
fn nested_box_payloads_cannot_escape_to_heap_or_another_arena() {
    for source in [
        "fn wrap(a: Arena, take child: (box Tree) in a) -> box (box Tree) { box take child }",
        "fn wrap(a: Arena, b: Arena, take child: (box Tree) in a) -> (box (box Tree)) in b { box take child }",
        "fn keep(a: Arena, take xs: List[box Tree] in a) -> List[box Tree] { take xs }",
    ] {
        check_placement(source, false);
    }
}

#[test]
fn borrowed_box_payloads_may_read_arena_storage() {
    check_placement(
        "fn read(value: box (box Tree)) {}\nfn pass(a: Arena, value: (box (box Tree)) in a) { read(value) }",
        true,
    );
}

#[test]
fn borrowed_box_payloads_still_respect_an_explicit_arena() {
    check_placement(
        "fn read(a: Arena, value: (box (box Tree)) in a) {}\nfn pass(a: Arena, b: Arena, value: (box (box Tree)) in b) { read(a, value) }",
        false,
    );
}
