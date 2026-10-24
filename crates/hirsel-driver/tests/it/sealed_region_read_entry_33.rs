//! Read callback admission retires its root loan before releasing the owner.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};

const SOURCE: &str = r#"
fn at_read_limit(n:u64,take owner:Region[List[Shared[Bytes]]])->u64{
 if n==0u64 {
  let mut owned=take owner
  region.with(mut owned,fn(view:List[Shared[Bytes]])->u64{7u64})
 } else {
  let got=at_read_limit(n-1u64,take owner)
  failure.check(n>0u64,Bounds())
  got
 }
}
pub fn main(rt:Io)->Result[(),GrantError]{
 let tasks=io.tasks(rt)?
 let built:unique Bytes=unique b"abc"
 let sentinel:Shared[Bytes]=shared.freeze(take built)
 let outcome:Result[u64,Failure]=task.isolate(tasks,fn()->u64{
  let owner=arena.with(fn(a:Arena)->Region[List[Shared[Bytes]]]{
   let root:List[Shared[Bytes]] in a=[copy sentinel]
   arena.seal(take a,take root)
  })
  at_read_limit(RECURSION,take owner)
 })
 match outcome {
  Ok(n)=>failure.check(!REJECTED&&n==7u64,Bounds()),
  Error(problem)=>{
   failure.check(REJECTED,Bounds())
   failure.check(problem.rule=="LANG-10",Bounds())
   failure.check(problem.site.line==READ_LINE,Bounds())
   failure.check(problem.site.definition=="standalone@0/at_read_limit",Bounds())
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

struct ReadEntry {
    library: Library,
    entries: usize,
}
impl OperationsAdapter for ReadEntry {
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
        if call.path.is_core("region.with") {
            self.entries += 1;
        }
        self.library.call_resolved(operation, call, cx)
    }
}

fn read_entry(rejected: bool) {
    with_compiler_stack(|| {
        // main plus isolate primitive/callback: three live invocations.
        // n+1 helper frames, one reader primitive and one callback follow.
        let limit = usize::from(hirsel_core_contract::depth::FOREGROUND_LIMIT);
        let recursion = limit - if rejected { 5 } else { 6 };
        let line = SOURCE
            .lines()
            .position(|line| line.contains("region.with"))
            .unwrap()
            + 1;
        let source = SOURCE
            .replace("RECURSION", &format!("{recursion}u64"))
            .replace("REJECTED", if rejected { "true" } else { "false" })
            .replace("READ_LINE", &format!("{line}u64"));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("sealed-read-entry.hir", source).unwrap();
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
            let mut host = ReadEntry {
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
                host.entries, 1,
                "primitive enters before callback admission and owner cleanup"
            );
        }
    });
}

#[test]
fn sealed_regions_33_reader_rejected_callback_retires_the_loan_before_owner_cleanup() {
    read_entry(true)
}
#[test]
fn sealed_regions_33_reader_last_admitted_callback_returns_before_owner_cleanup() {
    read_entry(false)
}
