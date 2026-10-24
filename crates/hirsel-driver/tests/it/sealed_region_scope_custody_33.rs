//! Arena callback adapters transfer Once captures only after admission.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{
    Error, OperationCall, OperationContext, OperationId, OperationResult, Operations,
    OperationsAdapter, Outcome, library::Library,
};

const SOURCE: &str = r#"
fn at_scope_limit(n:u64,take owner:Region[List[Shared[Bytes]]])->u64{
 if n==0u64 {
  arena.with(fn(a:Arena)->u64{
   let mut owned=take owner
   let got=region.with(mut owned,fn(view:List[Shared[Bytes]])->u64{
    shared.with(view[0],fn(data:Bytes)->u64{bytes.len(data)})
   })
   if FAIL_BODY {failure.check(false,Arithmetic())}
   got
  })
 } else {
  let got=at_scope_limit(n-1u64,take owner)
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
  at_scope_limit(RECURSION,take owner)
 })
 match outcome {
  Ok(n)=>failure.check(CASE==0u64&&n==3u64,Bounds()),
  Error(problem)=>{
   failure.check(CASE!=0u64,Bounds())
   failure.check(problem.site.definition=="standalone@0/at_scope_limit",Bounds())
   failure.check(list.len(problem.spawn)==0u64,Bounds())
   failure.check(problem.cause==None(),Bounds())
   if CASE==1u64 {
    failure.check(problem.rule=="SEM-1"&&problem.site.line==FAIL_LINE,Bounds())
    match problem.kind {Check()=>{},_=>failure.check(false,Bounds()),}
   } else {
    failure.check(problem.rule=="LANG-10"&&problem.site.line==SCOPE_LINE,Bounds())
    match problem.kind {Depth()=>{},_=>failure.check(false,Bounds()),}
   }
  },
 }
 let released=match shared.thaw(take sentinel){Ok(data)=>data==b"abc",Error(root)=>false,}
 failure.check(released,Bounds())
 Ok(())
}
"#;

struct ScopeCustody {
    library: Library,
    reads: usize,
}
impl OperationsAdapter for ScopeCustody {
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
            self.reads += 1;
        }
        self.library.call_resolved(operation, call, cx)
    }
}

fn scope_custody(case: u64) {
    with_compiler_stack(|| {
        let line = |needle: &str| {
            SOURCE
                .lines()
                .position(|line| line.contains(needle))
                .unwrap()
                + 1
        };
        // Three enclosing invocations, n+1 helpers, then arena primitive and
        // callback. The rejected case admits the primitive but no callback.
        let recursion = if case == 2 {
            usize::from(hirsel_core_contract::depth::FOREGROUND_LIMIT) - 5
        } else {
            0
        };
        let source = SOURCE
            .replace("RECURSION", &format!("{recursion}u64"))
            .replace("CASE", &format!("{case}u64"))
            .replace("FAIL_BODY", if case == 1 { "true" } else { "false" })
            .replace("FAIL_LINE", &format!("{}u64", line("if FAIL_BODY")))
            .replace("SCOPE_LINE", &format!("{}u64", line("arena.with")));
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-scope-custody.hir", source)
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
            let mut host = ScopeCustody {
                library: Library::new(&run, SimulationConfig::default()).unwrap(),
                reads: 0,
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
                "case={case} checked={checked}: {:?}",
                execution.outcome
            );
            assert_eq!(
                host.reads,
                usize::from(case != 2),
                "only an admitted scope body reads its captured owner"
            );
        }
    });
}

#[test]
fn sealed_regions_33_once_scope_capture_return_releases_exactly_one_owner() {
    scope_custody(0)
}
#[test]
fn sealed_regions_33_once_scope_capture_failure_releases_exactly_one_owner() {
    scope_custody(1)
}
#[test]
fn sealed_regions_33_rejected_scope_callback_keeps_capture_owner_with_caller() {
    scope_custody(2)
}
