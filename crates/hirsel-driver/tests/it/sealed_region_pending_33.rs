//! Force a real channel Pending while region.with retains its read loan.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};

const SOURCE: &str = r#"
fn make(take source:Shared[Bytes])->Region[List[Shared[Bytes]]]{
 arena.with(fn(a:Arena)->Region[List[Shared[Bytes]]]{
  let root:List[Shared[Bytes]] in a=[copy source]
  arena.seal(take a,take root)
 })
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 task.scope(tasks,fn(s:Scope){
  let child_root=copy sentinel
  let child=task.async(s,fn()->bool{
   let caught:Result[u64,Failure]=task.isolate(tasks,fn()->u64{
    let mut owner=make(take child_root)
    region.with(mut owner,fn(view:List[Shared[Bytes]])->u64{
     let first_length=shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
     let (sender,receiver):(Sender[u64],Receiver[u64])=channel.open(tasks)
     let mut gate=take receiver
     let canceled=channel.receive(tasks,mut gate)==Error(Canceled(Requested()))
     let _=take sender
     failure.check(canceled,Bounds())
     let resumed_length=shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
     failure.check(first_length==3u64&&resumed_length==first_length,Bounds())
     if FAIL_AFTER_CANCEL {failure.check(false,Arithmetic())}
     resumed_length
    })
   })
   match caught {Ok(n)=>!FAIL_AFTER_CANCEL&&n==3u64,Error(problem)=>FAIL_AFTER_CANCEL&&problem.rule=="SEM-1",}
  })
  // The host scheduler holds the parent until the child's receive is Pending.
  // make consumed the child's independent source root: only its Region retains it.
  let retained=match shared.thaw(take sentinel){
   Error(root)=>take root,
   Ok(data)=>{failure.check(false,Bounds());shared.freeze(take data)},
  }
  task.cancel(s,child)
  failure.check(task.await(s,take child),Bounds())
  let released=match shared.thaw(take retained){Ok(data)=>data==b"abc",Error(root)=>false,}
  failure.check(released,Bounds())
 })
 Ok(())
}
"#;

struct HeldRead {
    library: Library,
    current: u64,
    receive: Option<u64>,
    pending: bool,
    canceled: bool,
    refused_while_pending: usize,
    reads_after_cancel: usize,
    polls: usize,
}
impl OperationsAdapter for HeldRead {
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
        if !self.pending
            && let Some(child) = runnable.iter().position(|id| *id != 0)
        {
            return Ok(child);
        }
        self.library.choose_task(runnable)
    }
    fn call_resolved(
        &mut self,
        operation: OperationId,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let receiving = self.current != 0 && call.path.is_core("channel.receive");
        if self.current == 0 && call.path.is_core("shared.thaw") && !self.canceled {
            assert!(
                self.pending,
                "the read callback really suspended before the parent"
            );
            self.refused_while_pending += 1;
        }
        if self.current == 0 && call.path.is_core("task.cancel") {
            assert!(self.pending);
            assert_eq!(self.refused_while_pending, 1);
            self.canceled = true;
        }
        if self.current != 0 && self.canceled && call.path.is_core("shared.with") {
            self.reads_after_cancel += 1;
        }
        let result = self.library.call_resolved(operation, call, cx)?;
        if receiving && let Some(OperationResult::Park { token, .. }) = &result {
            assert!(
                self.receive.replace(*token).is_none(),
                "one receive, not a restarted callback"
            );
        }
        Ok(result)
    }
    fn parked(&mut self, token: u64) {
        if self.receive == Some(token) {
            assert!(!self.pending);
            self.pending = true;
        }
        self.library.parked(token);
    }
    fn poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if self.receive == Some(token) {
            self.polls += 1;
        }
        self.library.poll(token, cx)
    }
}

fn pending(fail_after_cancel: bool) {
    with_compiler_stack(|| {
        let source = SOURCE.replace(
            "FAIL_AFTER_CANCEL",
            if fail_after_cancel { "true" } else { "false" },
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("sealed-pending.hir", source).unwrap();
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
            let mut host = HeldRead {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                current: 0,
                receive: None,
                pending: false,
                canceled: false,
                refused_while_pending: 0,
                reads_after_cancel: 0,
                polls: 0,
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
            assert!(host.pending && host.canceled);
            assert_eq!(host.refused_while_pending, 1);
            assert_eq!(
                host.reads_after_cancel, 1,
                "one callback resumes with its original valid view"
            );
            assert!(host.polls != 0, "the suspended receive was actually polled");
        }
    });
}
#[test]
fn sealed_regions_33_pending_read_cancellation_returns_with_a_live_view() {
    pending(false);
}
#[test]
fn sealed_regions_33_pending_read_failure_unwinds_the_loan_and_owner() {
    pending(true);
}
