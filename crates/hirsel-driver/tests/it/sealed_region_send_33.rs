//! Suspend an entered send before delivery while the evaluator owns its Region.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    BlockedWait, Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};

const SOURCE: &str = r#"
fn discard[T](take value:T){}
fn make(take source:Shared[Bytes])->Region[unique List[Shared[Bytes]]]{
 arena.with(fn(a:Arena)->Region[unique List[Shared[Bytes]]]{
  let root:unique List[Shared[Bytes]] in a=[copy source]
  arena.seal(take a,take root)
 })
}
fn read(mut owner:Region[unique List[Shared[Bytes]]])->u64{
 region.with(mut owner,fn(view:unique List[Shared[Bytes]])->u64{
  shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
 })
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 let last=task.scope(tasks,fn(s:Scope)->Shared[Bytes]{
  let (sender,receiver):(Sender[Region[unique List[Shared[Bytes]]]],Receiver[Region[unique List[Shared[Bytes]]]])=channel.open(tasks)
  let child_root=copy sentinel
  let child=task.async(s,fn()->bool{
   let mut tx=take sender
   let owner=make(take child_root)
   match channel.send(tasks,mut tx,take owner){
    Ok(())=>!CLOSED,
    Error((returned,Closed()))=>{
     let mut owned=take returned
     read(mut owned)==3u64&&CLOSED
    },
    _=>false,
   }
  })
  // Parent is released only when the entered send is actually parked.
  let retained=match shared.thaw(take sentinel){
   Error(root)=>take root,
   Ok(data)=>{failure.check(false,Bounds());shared.freeze(take data)},
  }
  RECEIVER_BEFORE
  task.cancel(s,child)
  failure.check(task.await(s,take child),Bounds())
  RECEIVER_AFTER
  retained
 })
 let released=match shared.thaw(take last){Ok(data)=>data==b"abc",Error(root)=>false,}
 failure.check(released,Bounds())
 Ok(())
}
"#;

// These are snapshots only. OperationState retains the actual taken argument
// while Park is pending; the delegated completion keeps its consumed=[2] edge.
type DeferredSend = Box<
    dyn FnOnce(&mut Library, &mut OperationContext<'_>) -> Result<Option<OperationResult>, Error>,
>;
struct PendingSend {
    library: Library,
    current: u64,
    held: Option<DeferredSend>,
    pending: bool,
    canceled: bool,
    polls: usize,
    delivered: usize,
    retained: usize,
}
const SEND_TOKEN: u64 = u64::MAX;
impl OperationsAdapter for PendingSend {
    fn operations(&self) -> &dyn Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn Operations {
        &mut self.library
    }
    fn immediate(&self, _: OperationId) -> bool {
        false
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
        if self.current != 0 && call.path.is_core("channel.send") {
            assert!(!self.pending && self.held.is_none());
            let runtime = call.runtime.clone();
            let path = call.path.clone();
            let generics = call.generics.to_vec();
            let mut arguments = call.arguments.to_vec();
            let result_type = call.result_type;
            let site = call.site;
            self.held = Some(Box::new(move |library, cx| {
                library.call_resolved(
                    operation,
                    OperationCall {
                        runtime: &runtime,
                        path: &path,
                        generics: &generics,
                        arguments: &mut arguments,
                        result_type,
                        types: &runtime.types,
                        site,
                    },
                    cx,
                )
            }));
            return Ok(Some(OperationResult::Park {
                token: SEND_TOKEN,
                wait: BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().unwrap().clone(),
                    kind: "host-held channel.send".into(),
                    targets: vec![],
                },
            }));
        }
        if self.current == 0 && call.path.is_core("shared.thaw") && !self.canceled {
            assert!(self.pending && self.held.is_some());
            assert_eq!(self.delivered, 0);
            self.retained += 1;
        }
        if self.current == 0 && call.path.is_core("task.cancel") {
            assert!(self.pending && self.held.is_some());
            assert_eq!(self.retained, 1);
            self.canceled = true;
        }
        self.library.call_resolved(operation, call, cx)
    }
    fn parked(&mut self, token: u64) {
        if token == SEND_TOKEN {
            assert!(!self.pending);
            self.pending = true;
        } else {
            self.library.parked(token)
        }
    }
    fn watched(&self, token: u64) -> bool {
        token != SEND_TOKEN && self.library.watched(token)
    }
    fn poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if token != SEND_TOKEN {
            return self.library.poll(token, cx);
        }
        self.polls += 1;
        if !self.canceled {
            return Ok(None);
        }
        let result = self.held.take().expect("send completes once")(&mut self.library, cx)?;
        assert!(matches!(&result,Some(OperationResult::Complete{consumed,..}) if consumed==&[2]));
        self.delivered += 1;
        Ok(result)
    }
}
fn pending_send(closed: bool) {
    let source=SOURCE.replace("CLOSED",if closed{"true"}else{"false"})
        .replace("RECEIVER_BEFORE",if closed{"discard(take receiver)"}else{""})
        .replace("RECEIVER_AFTER",if closed{""}else{r#"
          let mut rx=take receiver
          let mut received=match channel.receive(tasks,mut rx){Ok(owner)=>owner,Error(_)=>{failure.check(false,Bounds());make(shared.freeze(unique b"bad"))},}
          failure.check(read(mut received)==3u64,Bounds())
        "#});
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-task-result-cut.hir", source)
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
            let mut host = PendingSend {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                current: 0,
                held: None,
                pending: false,
                polls: 0,
                delivered: 0,
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
            assert_eq!(host.delivered, 1);
            assert!(host.pending && host.polls != 0);
            assert!(host.held.is_none());
            assert!(host.canceled);
            assert_eq!(host.retained, 1);
        }
    });
}

#[test]
fn sealed_regions_33_pending_send_cancellation_delivers_to_open_receiver() {
    pending_send(false)
}
#[test]
fn sealed_regions_33_pending_send_closed_receiver_returns_the_owner() {
    pending_send(true)
}
