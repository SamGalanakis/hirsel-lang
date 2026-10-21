use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_run_config::RunConfig;
use hirsel_run_schema::{LaunchKind, ProgramRevision};
use hirsel_syntax::ContentHash;

const PROGRAM: &str =
    "pub fn main() -> Result[(), ()] { helper() }\nfn helper() -> Result[(), ()] { Ok(()) }\n";

fn engine(text: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    parse(&mut engine, "program.hir", text);
    engine
}
fn parse(engine: &mut QueryEngine, file: &str, text: &str) {
    let file = engine.sources.add(file, text.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
}
fn revision(engine: &mut QueryEngine) -> ProgramRevision {
    engine
        .semantic_program_revision(&entry_path(&default_context()))
        .unwrap()
}

#[test]
fn unchanged_entry_and_signature_bind_transitive_private_bodies() {
    with_compiler_stack(|| {
        let mut engine = engine(PROGRAM);
        let first = revision(&mut engine);
        let main = entry_path(&default_context());
        let signature = engine.item_signature(&main).unwrap().value.fingerprint;
        parse(
            &mut engine,
            "program.hir",
            &PROGRAM.replace("{ Ok(()) }", "{ Error(()) }"),
        );
        assert_eq!(
            signature,
            engine.item_signature(&main).unwrap().value.fingerprint
        );
        assert_ne!(first, revision(&mut engine));
    });
}

#[test]
fn constant_initializers_and_their_private_comptime_dependencies_are_bound() {
    with_compiler_stack(|| {
        let source = "fn compute() -> u64 { 41u64 }\nconst answer: u64 = compute()\nfn helper() -> u64 { answer }\npub fn main() -> Result[(), ()] { let n = helper(); Ok(()) }\n";
        let mut engine = engine(source);
        let first = revision(&mut engine);
        parse(
            &mut engine,
            "program.hir",
            &source.replace("41u64", "42u64"),
        );
        assert_ne!(first, revision(&mut engine));
        parse(
            &mut engine,
            "program.hir",
            &source.replace("= compute()", "= compute() + 1u64"),
        );
        assert_ne!(first, revision(&mut engine));
    });
}

#[test]
fn recursive_and_function_value_dependencies_have_private_implementations() {
    with_compiler_stack(|| {
        let source = "fn recurse(n: u64) -> u64 { if n == 0u64 { 1u64 } else { recurse(n - 1u64) } }\npub fn main() -> Result[(), ()] { let f = recurse; let n = f(2u64); Ok(()) }\n";
        let mut engine = engine(source);
        let first = revision(&mut engine);
        parse(
            &mut engine,
            "program.hir",
            &source.replace("{ 1u64 }", "{ 3u64 }"),
        );
        assert_ne!(first, revision(&mut engine));
    });
}

#[test]
fn file_order_positions_comments_and_compiler_provenance_do_not_change_revision() {
    with_compiler_stack(|| {
        let first = revision(&mut engine(PROGRAM));
        let mut reordered = QueryEngine::new(default_environment(), compiler_adapters());
        parse(
            &mut reordered,
            "/arbitrary/host/helper.hir",
            "\n\n// implementation\nfn helper() -> Result[(), ()] { /* body */ Ok(()) }\n",
        );
        parse(
            &mut reordered,
            "/other/host/entry.hir",
            "\n\npub fn main() -> Result[(), ()] { helper() }\n",
        );
        assert_eq!(first, revision(&mut reordered));
        let mut environment = default_environment();
        environment.compiler_version = "different compiler build".into();
        environment.package_manifest_hash = ContentHash([123; 32]);
        let mut different = QueryEngine::new(environment, compiler_adapters());
        parse(&mut different, "different.hir", PROGRAM);
        assert_eq!(first, revision(&mut different));
    });
}

#[test]
fn parsed_statement_boundaries_preserve_revision_across_formatting() {
    with_compiler_stack(|| {
        let source =
            "fn helper()->u64{let n=1u64;n}\npub fn main()->Result[(),()]{let _=helper();Ok(())}\n";
        let mut resident = engine(source);
        let entry = entry_path(&default_context());
        let before = resident.definition_items(&entry).unwrap()[0].clone();
        let first = revision(&mut resident);
        for text in [
            "fn helper() -> u64 {\n  let n = 1u64\n  n\n}\n\npub fn main() -> Result[(), ()] {\n  let _ = helper()\n  Ok(())\n}\n",
            "// moved entry\npub fn main()->Result[(),()]{let _=helper();Ok(())}\n\nfn helper()->u64{/* a\ncomment */let n=1u64;/* explanation */n}\n",
            "fn helper()->u64{\r\nlet n=1u64\r\nn\r\n}\r\npub fn main()->Result[(),()]{\r\nlet _=helper()\r\nOk(())\r\n}\r\n",
        ] {
            assert_eq!(first, revision(&mut engine(text)), "{text}");
            parse(&mut resident, "program.hir", text);
            let after = resident.definition_items(&entry).unwrap()[0].clone();
            assert_eq!(before.owner(), after.owner());
            assert_ne!(before.text_hash, after.text_hash);
            assert_eq!(first, revision(&mut resident), "resident: {text}");
        }
        // A newline can change the parsed program. Ignoring all trivia or all
        // semicolons would conflate a discarded expression with a block tail.
        let tail = "fn helper(){()}\npub fn main()->Result[(),()]{helper();Ok(())}\n";
        assert_ne!(
            revision(&mut engine(tail)),
            revision(&mut engine(&tail.replace("{()}", "{();}")))
        );
    });
}

#[test]
fn unrelated_source_edits_do_not_change_the_selected_closure() {
    with_compiler_stack(|| {
        let source = format!("{PROGRAM}fn unused() -> u64 {{ 1u64 }}\n");
        let mut engine = engine(&source);
        let first = revision(&mut engine);
        parse(&mut engine, "program.hir", &source.replace("1u64", "2u64"));
        assert_eq!(first, revision(&mut engine));
    });
}

#[test]
fn referenced_private_representations_and_implicit_finalizers_are_bound() {
    with_compiler_stack(|| {
        let source = "pub opaque type Hidden: () uses {} { value: u64 }\nresource type Token { value: u64 }\ndrop(token) { let n = 1u64; }\nfn helper() { let h = Hidden(value: 1); let t = Token(value: 0u64); }\npub fn main() -> Result[(), ()] { helper(); Ok(()) }\n";
        let mut engine = engine(source);
        let first = revision(&mut engine);
        parse(
            &mut engine,
            "program.hir",
            &source.replace("let n = 1u64", "let n = 2u64"),
        );
        assert_ne!(first, revision(&mut engine));
        parse(
            &mut engine,
            "program.hir",
            &source.replace(
                "Hidden: () uses {} { value: u64 }",
                "Hidden: () uses {} { value: u32 }",
            ),
        );
        assert_ne!(first, revision(&mut engine));
    });
}

#[test]
fn a_missing_dependency_refuses_and_its_complete_twin_binds() {
    with_compiler_stack(|| {
        let entry = entry_path(&default_context());
        let mut incomplete = engine("pub fn main() -> Result[(), ()] { missing() }\n");
        assert!(incomplete.semantic_program_revision(&entry).is_err());
        let mut complete = engine(PROGRAM);
        let revision = revision(&mut complete);
        let run = RunConfig::for_program(&revision, LaunchKind::Main)
            .validated()
            .unwrap();
        assert!(run.check_revision(&revision, LaunchKind::Main).is_ok());
    });
}

#[test]
fn singleton_whole_and_partitioned_native_builds_keep_the_semantic_revision() {
    with_compiler_stack(|| {
        use native::*;
        let mut engine = engine(PROGRAM);
        let semantic = revision(&mut engine);
        let run = RunConfig::for_program(&semantic, LaunchKind::Main)
            .validated()
            .unwrap();
        let snapshot = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(snapshot).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let mut options = Options::new(target.clone());
        options.launch = run.record().inputs.clone();
        let ready = build(&validated, &options).unwrap();
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("semantic-topologies");
        std::fs::create_dir_all(&root).unwrap();
        let toolchain = Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
        let entry = entry_path(&default_context());
        for partition in [
            Partition::Singleton,
            Partition::Whole,
            Partition::Random(17),
        ] {
            let dir = root.join(partition.name());
            std::fs::create_dir_all(&dir).unwrap();
            let executable =
                link(&ready, &entry, OptLevel::None, partition, &toolchain, &dir).unwrap();
            let result = launch(&executable, &target, &toolchain, &dir, true).unwrap();
            assert_eq!(result.exit, Some(0), "{partition:?}: {result:?}");
            assert_eq!(semantic, revision(&mut engine));
            assert!(run.check_revision(&semantic, LaunchKind::Main).is_ok());
        }
    });
}
