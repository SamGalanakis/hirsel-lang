//! Scheduler-directed custody of a completed, unjoined Region result.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, Value, library::Library,
};

const RESULT_SOURCE: &str = r#"
fn make(take source:Shared[Bytes])->Region[unique List[Shared[Bytes]]]{
 arena.with(fn(a:Arena)->Region[unique List[Shared[Bytes]]]{
  let root:unique List[Shared[Bytes]] in a=[copy source]
  arena.seal(take a,take root)
 })
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 let last=task.scope(tasks,fn(s:Scope)->Shared[Bytes]{
  let child_root=copy sentinel
  let child=task.async(s,fn()->Region[unique List[Shared[Bytes]]]{make(take child_root)})
  let retained=match shared.thaw(take sentinel){
   Error(root)=>take root,
   Ok(data)=>{failure.check(false,Bounds());shared.freeze(take data)},
  }
  task.cancel(s,child)
  task.detach(s,take child)
  retained
 })
 let released=match shared.thaw(take last){Ok(data)=>data==b"abc",Error(root)=>false,}
 failure.check(released,Bounds())
 Ok(())
}
"#;
struct CompletedRegion {
    library: Library,
    current: u64,
    results: usize,
    canceled: bool,
    detached: bool,
    retained: usize,
}
impl OperationsAdapter for CompletedRegion {
    fn operations(&self) -> &dyn Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn Operations {
        &mut self.library
    }
    fn enter_task(&mut self, id: u64, shielded: bool) {
        self.current = id;
        self.library.enter_task(id, shielded);
    }
    fn choose_task(&mut self, runnable: &[u64]) -> Result<usize, Error> {
        if self.results == 0
            && let Some(child) = runnable.iter().position(|id| *id != 0)
        {
            return Ok(child);
        }
        self.library.choose_task(runnable)
    }
    fn task_finished(
        &mut self,
        id: u64,
        result: OperationResult,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        if id != 0
            && matches!(&result,OperationResult::Complete {value:Value::Handle{kind,..},..} if kind=="Region")
        {
            self.results += 1;
        }
        self.library.task_finished(id, result, cx)
    }
    fn call_resolved(
        &mut self,
        operation: OperationId,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.current == 0 && call.path.is_core("shared.thaw") && !self.canceled {
            assert_eq!(
                self.results, 1,
                "parent runs only after the child has published its Region result"
            );
            self.retained += 1;
        }
        if self.current == 0 && call.path.is_core("task.cancel") {
            assert_eq!(self.results, 1);
            assert_eq!(self.retained, 1);
            self.canceled = true;
        }
        if self.current == 0 && call.path.is_core("task.detach") {
            assert!(self.canceled);
            self.detached = true;
        }
        self.library.call_resolved(operation, call, cx)
    }
}
#[test]
fn sealed_regions_33_cancel_after_region_is_an_unjoined_task_result() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-task-result-cut.hir", RESULT_SOURCE.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let exported = engine.export_native_input().unwrap();
        engine.import_native_input(exported).unwrap();
        for checked in [false, true] {
            let runtime = if checked {
                engine.checked_runtime_types()
            } else {
                engine.runtime_types()
            }
            .unwrap();
            let run = RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            };
            let sources = engine.sources.clone();
            let mut host = CompletedRegion {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                current: 0,
                results: 0,
                canceled: false,
                detached: false,
                retained: 0,
            };
            let io = host.library.simulation.grant_root("Io").unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut engine,
                &sources,
                &run,
                vec![io],
                Default::default(),
                &mut host,
                &runtime,
            )
            .unwrap();
            assert!(
                matches!(execution.outcome, Outcome::Returned { exit_code: 0, .. }),
                "checked={checked}: {:?}",
                execution.outcome
            );
            assert_eq!(host.results, 1);
            assert!(host.canceled && host.detached);
            assert_eq!(host.retained, 1);
        }
    });
}
