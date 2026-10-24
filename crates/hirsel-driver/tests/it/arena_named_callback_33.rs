//! Named borrow-only scope callbacks receive their arena through the usual ABI.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Outcome, library::Library};

const SOURCE: &str = r#"
fn named(a:Arena)->u64{
 let xs:List[u64] in a=[7u64]
 xs[0]
}
pub fn main(){
 if BOUNDED {
  let small=arena.bounded(7u64,named)
  let failed=match small{Ok(_)=>false,Error(_)=>true,}
  failure.check(failed,Bounds())
  let enough=arena.bounded(8u64,named)
  let got=match enough{Ok(n)=>n,Error(_)=>0u64,}
  failure.check(got==7u64,Bounds())
 } else {
  failure.check(arena.with(named)==7u64,Bounds())
 }
}
"#;

fn named(bounded: bool) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let source = SOURCE.replace("BOUNDED", if bounded { "true" } else { "false" });
        let file = engine
            .sources
            .add("arena-named-callback.hir", source)
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        engine.import_native_input(input).unwrap();
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
            let mut library = Library::new(&run, SimulationConfig::default()).unwrap();
            let execution = hirsel_interp::run_with_runtime(
                &mut engine,
                &sources,
                &run,
                vec![],
                Default::default(),
                &mut library,
                &runtime,
            )
            .unwrap_or_else(|error| panic!("bounded={bounded} checked={checked}: {error:?}"));
            assert!(
                matches!(execution.outcome, Outcome::Returned { exit_code: 0, .. }),
                "bounded={bounded} checked={checked}: {:?}",
                execution.outcome
            );
        }
    });
}

#[test]
fn arena_named_callback_33_unbounded_named_borrow_only_body_executes() {
    named(false)
}
#[test]
fn arena_named_callback_33_bounded_named_borrow_only_body_keeps_its_budget() {
    named(true)
}
