//! An async child returns a unique result through nested scoped callbacks.
use hirsel_driver::COMPILER;
use hirsel_engine::*;

#[test]
fn unique_result_from_nested_scope_is_validated_before_async_crossing() {
    with_compiler_stack(|| {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.hir");
        let manifest = directory.path().join("hirsel.json");
        std::fs::write(
            &path,
            r#"
pub type Prepared { bytes: Bytes, }
pub type State { bytes: Bytes, }
fn collect(scope: Scope, state: State) -> Result[unique Prepared, u64] {
  Ok(Prepared(bytes: unique state.bytes))
}
pub fn prepare(tasks: Tasks, state: State) -> Result[unique Prepared, u64] {
  let frozen = shared.freeze(unique state)
  task.scope(tasks, fn(scope: Scope) -> Result[unique Prepared, u64] {
    let child: Task[unique Result[unique Prepared, u64]] of scope = task.async(
      scope,
      fn() -> unique Result[unique Prepared, u64] {
        unique task.scope(tasks,
          fn(child_scope: Scope) -> Result[unique Prepared, u64] {
            shared.with(frozen, fn(snapshot: State) -> Result[unique Prepared, u64] {
              collect(child_scope, snapshot)
            })
          }
        )
      }
    )
    take task.await(scope, take child)
  })
}
pub fn main()->u64{1u64}
"#,
        )
        .unwrap();
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"scope_result","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
        let mut program = COMPILER.load_program(&manifest).unwrap();
        program
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
    });
}
