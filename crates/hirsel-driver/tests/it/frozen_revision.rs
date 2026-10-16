use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_run_config::{LaunchKind, RunConfig, RunRefusal};

fn engine(text: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, text);
    engine
}
fn parse(engine: &mut QueryEngine, text: &str) {
    let file = engine.sources.add("frozen.hir", text.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
}
fn freeze(engine: &mut QueryEngine) -> ValidatedNativeInput {
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap()
}

#[test]
fn frozen_and_source_revisions_share_the_same_entry_closure_after_engine_drop() {
    with_compiler_stack(|| {
        let source = "fn compute() -> u64 { 41u64 }\nconst answer: u64 = compute()\nfn helper() -> u64 { answer }\nfn unused() -> u64 { 99u64 }\npub fn main() -> Result[(), ()] { let n = helper(); Ok(()) }\n";
        let mut engine = engine(source);
        let entry = entry_path(&default_context());
        let source_revision = engine.semantic_program_revision(&entry).unwrap();
        let mut helper = entry.clone();
        helper.0.name = "helper".into();
        let helper_revision = engine.semantic_program_revision(&helper).unwrap();
        let frozen = freeze(&mut engine);
        drop(engine);
        assert_eq!(
            source_revision,
            frozen.semantic_program_revision(&entry).unwrap()
        );
        assert_eq!(
            helper_revision,
            frozen.semantic_program_revision(&helper).unwrap()
        );
        let mut absent = entry;
        absent.0.name = "absent".into();
        assert!(frozen.semantic_program_revision(&absent).is_err());
    });
}

#[test]
fn frozen_private_revisions_remain_immutable_across_live_source_edits() {
    with_compiler_stack(|| {
        let source = "pub fn main() -> Result[(), ()] { helper() }\nfn helper() -> Result[(), ()] { Ok(()) }\n";
        let mut engine = engine(source);
        let entry = entry_path(&default_context());
        let old = freeze(&mut engine);
        let old_revision = old.semantic_program_revision(&entry).unwrap();
        parse(&mut engine, &source.replace("{ Ok(()) }", "{ Error(()) }"));
        let new = freeze(&mut engine);
        let new_revision = new.semantic_program_revision(&entry).unwrap();
        assert_eq!(
            new_revision,
            engine.semantic_program_revision(&entry).unwrap()
        );
        drop(engine);
        assert_eq!(old_revision, old.semantic_program_revision(&entry).unwrap());
        assert_ne!(old_revision, new_revision);
        let run = RunConfig::for_program(&old_revision, LaunchKind::Main)
            .validated()
            .unwrap();
        assert!(run.check_revision(&old_revision, LaunchKind::Main).is_ok());
        assert_eq!(
            run.check_revision(&new_revision, LaunchKind::Main),
            Err(RunRefusal::Binding {
                field: "program".into()
            })
        );
    });
}
