//! Force cancellation while a Region is queued and after the child takes it.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};
const SOURCE: &str = r#"fn make(take source:Shared[Bytes])->Region[unique List[Shared[Bytes]]]{
 arena.with(fn(a:Arena)->Region[unique List[Shared[Bytes]]]{
  let root:unique List[Shared[Bytes]] in a=[copy source]
  arena.seal(take a,take root)
 })
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let output=io.stdout(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 let last=task.scope(tasks,fn(s:Scope)->Shared[Bytes]{
  let child_root=copy sentinel
  let owner=make(take child_root)
  let child=task.async(s,fn()->u64{
   let mut owned=take owner
   let _=out.print(output,"owned\n")
   region.with(mut owned,fn(view:unique List[Shared[Bytes]])->u64{
    shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
   })
  })
  let retained=match shared.thaw(take sentinel){
   Error(root)=>take root,
   Ok(data)=>{failure.check(false,Bounds());shared.freeze(take data)},
  }
  task.cancel(s,child)
  failure.check(task.await(s,take child)==3u64,Bounds())
  retained
 })
 let released=match shared.thaw(take last){Ok(data)=>data==b"abc",Error(root)=>false,}
 failure.check(released,Bounds())
 Ok(())
}
"#;
struct CaptureCut {
    library: Library,
    current: u64,
    started: bool,
    announced: usize,
    reads: usize,
    after_own: bool,
    canceled: bool,
    retained: usize,
}
impl OperationsAdapter for CaptureCut {
    fn operations(&self) -> &dyn Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn Operations {
        &mut self.library
    }
    fn enter_task(&mut self, id: u64, shielded: bool) {
        self.current = id;
        if id != 0 {
            self.started = true;
        }
        self.library.enter_task(id, shielded)
    }
    fn choose_task(&mut self, runnable: &[u64]) -> Result<usize, Error> {
        if !self.canceled {
            let child_first = self.after_own && self.announced == 0;
            if let Some(chosen) = runnable.iter().position(|id| (*id != 0) == child_first) {
                return Ok(chosen);
            }
        }
        self.library.choose_task(runnable)
    }
    fn call_resolved(
        &mut self,
        operation: OperationId,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.current != 0 && call.path.is_core("out.print") {
            self.announced += 1;
            assert_eq!(self.announced, 1, "one original child activation");
            assert_eq!(self.canceled, !self.after_own);
        }
        if self.current == 0 && call.path.is_core("shared.thaw") && !self.canceled {
            assert_eq!(self.started, self.after_own);
            assert_eq!(self.announced, usize::from(self.after_own));
            assert_eq!(self.reads, 0);
            self.retained += 1;
        }
        if self.current == 0 && call.path.is_core("task.cancel") {
            assert_eq!(self.started, self.after_own);
            assert_eq!(self.announced, usize::from(self.after_own));
            assert_eq!(self.retained, 1);
            self.canceled = true;
        }
        if self.current != 0 && call.path.is_core("region.with") {
            assert!(self.canceled);
            self.reads += 1;
        }
        self.library.call_resolved(operation, call, cx)
    }
}
fn capture_cut(after_own: bool) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-task-result-cut.hir", SOURCE.into())
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
            let mut host = CaptureCut {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                current: 0,
                started: false,
                announced: 0,
                reads: 0,
                after_own,
                canceled: false,

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
            assert!(host.started);
            assert_eq!(host.announced, 1);
            assert_eq!(host.reads, 1);
            assert_eq!(
                execution.stdout,
                if after_own {
                    b"owned\n".to_vec()
                } else {
                    vec![]
                }
            );
            assert!(host.canceled);
            assert_eq!(host.retained, 1);
        }
    });
}

#[test]
fn sealed_regions_33_cancel_while_region_is_in_a_queued_capture() {
    capture_cut(false)
}
#[test]
fn sealed_regions_33_cancel_after_child_takes_the_region() {
    capture_cut(true)
}
