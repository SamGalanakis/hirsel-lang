//! Rejected seal admission leaves its grant/root with the caller's cleanup.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};

const SOURCE: &str = r#"
fn at_limit(n:u64,take a:Arena in a,take root:List[Shared[Bytes]])->Region[List[Shared[Bytes]]]{
 if n==0u64 {
  arena.seal(take a,take root)
 } else {
  let owned=at_limit(n-1u64,take a,take root)
  failure.check(n>0u64,Bounds())
  take owned
 }
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 let outcome:Result[u64,Failure]=task.isolate(tasks,fn()->u64{
  arena.with(fn(a:Arena)->u64{
   let retained:List[Shared[Bytes]] in a=[copy sentinel]
   let root:List[Shared[Bytes]] in heap=[copy sentinel]
   let mut owned=at_limit(RECURSION,take a,take root)
   region.with(mut owned,fn(view:List[Shared[Bytes]])->u64{
    shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
   })
  })
 })
 match outcome {
  Ok(n)=>failure.check(!REJECTED&&n==3u64,Bounds()),
  Error(problem)=>{
   failure.check(REJECTED,Bounds())
   failure.check(problem.rule=="LANG-10",Bounds())
   failure.check(problem.site.line==SEAL_LINE,Bounds())
   failure.check(problem.site.definition=="standalone@0/at_limit",Bounds())
   failure.check(list.len(problem.spawn)==0u64,Bounds())
   failure.check(problem.cause==None(),Bounds())
   match problem.kind {Depth()=>{},_=>failure.check(false,Bounds()),}
  },
 }
 let released=match shared.thaw(take sentinel){Ok(data)=>data==b"abc",Error(root)=>false,}
 failure.check(released,Bounds())
 Ok(())
}
"#;

struct SealEntry {
    library: Library,
    entries: usize,
}
impl OperationsAdapter for SealEntry {
    fn operations(&self) -> &dyn Operations {
        &self.library
    }
    fn operations_mut(&mut self) -> &mut dyn Operations {
        &mut self.library
    }
    fn call_resolved(
        &mut self,
        operation: OperationId,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        if call.path.is_core("arena.seal") {
            self.entries += 1;
        }
        self.library.call_resolved(operation, call, cx)
    }
}

fn entry(rejected: bool) {
    with_compiler_stack(|| {
        // main + isolate primitive/callback + arena primitive/callback = five.
        // The helper has n+1 ordinary frames; its post-call check prevents tail reuse.
        let limit = usize::from(hirsel_core_contract::depth::FOREGROUND_LIMIT);
        let recursion = limit - if rejected { 6 } else { 7 };
        let seal_line = SOURCE
            .lines()
            .position(|line| line.contains("arena.seal"))
            .unwrap()
            + 1;
        let source = SOURCE
            .replace("RECURSION", &format!("{recursion}u64"))
            .replace("REJECTED", if rejected { "true" } else { "false" })
            .replace("SEAL_LINE", &format!("{seal_line}u64"));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("sealed-entry.hir", source).unwrap();
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
            let mut host = SealEntry {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                entries: 0,
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
                "rejected={rejected} checked={checked}: {:?}",
                execution.outcome
            );
            assert_eq!(
                host.entries,
                usize::from(!rejected),
                "rejection occurs before the library takes custody"
            );
        }
    });
}
#[test]
fn sealed_regions_33_rejected_seal_entry_keeps_caller_cleanup_owners() {
    entry(true)
}
#[test]
fn sealed_regions_33_last_admitted_seal_entry_transfers_one_owner() {
    entry(false)
}
