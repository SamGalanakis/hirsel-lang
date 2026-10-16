//! A heap block that never escapes in optimized LLVM IR: the resource-failure
//! kernel's two-element list literal is built, checked, written and dropped in each iteration. Its count is the
//! alloc postcondition, so the uniqueness tests and the release fold, and the
//! allocation goes with its free, as clang deletes C's malloc/free pair. A list
//! that is returned keeps its allocation.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};

const KERNEL: &str = "pub type Resource { id: u8, closed: bool }
pub fn kernel(n: u64) -> u64 {
  let mut total: u64 = 0
  for i in 0u64..n {
    let mut resources: List[Resource] = [Resource(id: 1u8, closed: false), Resource(id: 2u8, closed: false)]
    for reverse in 0u64..2u64 {
      let j = 1u64 - reverse
      failure.check(!resources[j].closed, Bounds())
      resources[j].closed = true
      total = int.wrapping_add(total, if resources[j].id == 2u8 { i } else { 8u64 })
    }
    failure.check(resources[0u64].closed && resources[1u64].closed, Bounds())
  }
  total
}
pub fn keep(i: u64) -> List[Resource] {
  [Resource(id: int.truncate(i), closed: false), Resource(id: 2u8, closed: false)]
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  let _ = kernel(5u64)
  let _ = keep(3u64)
  Ok(())
}
";

/// The optimized IR of each target's definition whose link name contains `name`.
fn function_ir(name: &'static str) -> Vec<String> {
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("kernel.hir", KERNEL.to_owned()).unwrap();
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

#[test]
fn a_list_dropped_in_its_iteration_is_never_allocated() {
    for ir in function_ir("6kernelI") {
        for runtime in [
            "@hirsel_rt_v2_alloc",
            "@hirsel_rt_v2_free",
            "@hirsel_rt_v2_detach",
            "@hirsel_rt_v2_failure_begin",
        ] {
            assert!(!ir.contains(runtime), "{runtime}:\n{ir}");
        }
    }
}

#[test]
fn a_returned_list_keeps_its_allocation() {
    for ir in function_ir("4keepI") {
        assert_eq!(
            ir.matches("call ptr @hirsel_rt_v2_alloc").count(),
            1,
            "{ir}"
        );
    }
}
