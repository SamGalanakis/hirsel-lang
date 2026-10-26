//! Private summaries and explicit public preservation promises.
use hirsel_driver::*;
use hirsel_engine::*;

const INCIDENT: &str = r#"fn add(mut xs: List[u64]) {
  list.push(mut xs, 1)
}

fn need(take xs: unique List[u64]) {}

pub fn main() {
  let mut xs: unique List[u64] = []
  add(mut xs)
  need(take xs)
}
"#;

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("helper.hir", source.into()).unwrap();
    check_progress(&mut engine, file, &default_context()).unwrap()
}

#[test]
fn helper_unique_promise_19_original_handback_names_checked_recipe() {
    with_compiler_stack(|| {
        accepted(INCIDENT);
        rejected(
            &INCIDENT.replacen("fn add", "pub fn add", 1),
            "need(take xs)",
            true,
        );
    });
}

fn rejected(source: &str, primary: &str, recipe: bool) -> CheckOutput {
    let output = checked(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{source}: {:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "MODE-2", "{d:?}");
    assert_eq!(output.sources.slice(d.primary), Some(primary), "{d:?}");
    let at = output.sources.resolve(d.primary);
    assert_eq!(at.start as usize, source.find(primary).unwrap());
    assert_eq!(
        at.end as usize,
        source.find(primary).unwrap() + primary.len()
    );
    assert_eq!(
        d.notes
            .iter()
            .any(|n| n.contains("declare `@preserves_unique(")),
        recipe,
        "{d:?}"
    );
    assert!(d.fix.is_none());
    output
}

fn accepted(source: &str) {
    let mut engine = crate::source_std::engine();
    engine.verify = Verify::On;
    let file = engine.sources.add("accepted.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}: {:?}",
        output.diagnostics
    );
    // A detached snapshot forces V to check the declarations and their promises.
    let mut input = engine.export_native_input().unwrap();
    for definition in input.definitions.values_mut() {
        *definition = std::sync::Arc::new(definition.as_ref().clone());
    }
    engine.import_native_input(input).unwrap();
}

#[test]
fn helper_unique_promise_19_append_only_checked_promise() {
    with_compiler_stack(|| {
        accepted(&INCIDENT.replacen("fn add", "@preserves_unique(xs)\nfn add", 1));
        let renamed = INCIDENT
            .replace("add", "append_line")
            .replace("mut xs: List", "mut lines: List")
            .replace("mut xs, 1", "mut lines, 1");
        accepted(&renamed);
        accepted(&renamed.replacen(
            "fn append_line",
            "@preserves_unique(lines)\nfn append_line",
            1,
        ));
    });
}

#[test]
fn helper_unique_promise_19_share_installing_promise_rejected() {
    with_compiler_stack(|| {
        let source = "const EMPTY: List[u64] = []\nfn reset(mut out: List[u64]) { out = EMPTY }\nfn need(take xs: unique List[u64]) {}\nfn probe() { let mut xs: unique List[u64] = []; reset(mut xs); need(take xs) }";
        rejected(source, "need(take xs)", false);
        let claimed = source.replace("fn reset", "@preserves_unique(out)\nfn reset");
        rejected(&claimed, "}", false);
        accepted(&claimed.replace("out = EMPTY", "out = unique EMPTY"));
    });
}

#[test]
fn helper_unique_promise_19_field_mutation_and_failure_handback() {
    with_compiler_stack(|| {
        let source = "type State { lines: List[u64], n: u64 }\nfn add(mut lines: List[u64]) { list.push(mut lines,1) }\nfn need(mut state: unique State) {}\nfn probe() { let mut state: unique State = State(lines: [], n: 0); add(mut state.lines); need(mut state) }";
        accepted(source);
        accepted(&source.replace("fn add", "@preserves_unique(lines)\nfn add"));
        // Failure is an exit too: a later restoration cannot justify the promise.
        let source = "const EMPTY: List[u64] = []\n@preserves_unique(out)\nfn reset(mut out: List[u64], ok: bool) { out = EMPTY; failure.check(ok, Bounds()); out = unique out }";
        rejected(source, "failure.check(ok, Bounds())", false);
        accepted(&source.replace("out = EMPTY", "out = unique EMPTY"));
        let source = "fn add(mut out: List[u64], ok: bool) { list.push(mut out,1); failure.check(ok, Bounds()) }\nfn probe(mut out: unique List[u64], ok: bool) { add(mut out,ok) }";
        // The caller owes uniqueness on both the normal and failure handbacks.
        accepted(source);
        accepted(&source.replace("fn add", "@preserves_unique(out)\nfn add"));
    });
}

#[test]
fn helper_unique_promise_19_existing_promise_does_not_repair_other_shares() {
    with_compiler_stack(|| {
        let source = "@preserves_unique(xs)\nfn add(mut xs: List[List[u64]], take item: List[u64]) { list.push(mut xs,take item) }\nfn need(take xs: unique List[List[u64]]) {}\nfn probe() { let item: List[u64] = [1]; let mut xs: unique List[List[u64]] = []; add(mut xs,copy item); need(take xs) }";
        let output = rejected(source, "need(take xs)", false);
        assert!(
            output.diagnostics[0]
                .secondary
                .iter()
                .any(|l| l.message.contains("`item` input may share"))
        );
        accepted(&source.replace("add(mut xs,copy item)", "add(mut xs,unique copy item)"));
        let source = "@preserves_unique(xs)\nfn add(mut xs: List[u64]) { list.push(mut xs,1) }\nfn need(take xs: unique List[u64]) {}\nfn probe() { let mut xs: unique List[u64] = []; let saved = copy xs; add(mut xs); need(take xs); let _ = saved }";
        let output = rejected(source, "need(take xs)", false);
        assert!(
            output.diagnostics[0]
                .secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("copy xs"))
        );
        // A counted result may independently keep a share of the argument.
        let source = "fn view(mut xs: List[u64]) -> List[u64] in xs { copy xs }\nfn need(mut xs: unique List[u64]) {}\nfn probe() { let mut xs: unique List[u64] = []; let saved = view(mut xs); need(mut xs); let _ = saved }";
        let output = checked(source);
        assert!(!output.diagnostics.is_empty());
        assert!(
            output.diagnostics.iter().all(|d| !d
                .notes
                .iter()
                .any(|n| n.contains("declare `@preserves_unique("))),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn helper_unique_promise_19_imported_bodyless_and_indirect_contracts() {
    with_compiler_stack(|| {
        let mut engine = crate::source_std::engine();
        let mut context = default_context();
        context.modules = vec!["remote".into()];
        let file = engine
            .sources
            .add(
                "remote.hir",
                "pub fn add(mut xs: List[u64]) { list.push(mut xs,1) }".into(),
            )
            .unwrap();
        assert!(
            check_progress(&mut engine, file, &context)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let source = format!(
            "fn need(take xs: unique List[u64]) {{}}\nfn probe() {{ let mut xs: unique List[u64] = []; {}@{}/remote.add(mut xs); need(take xs) }}",
            context.package.name, context.package.version
        );
        let file = engine.sources.add("caller.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert!(
            d.notes
                .iter()
                .all(|n| !n.contains("declare `@preserves_unique("))
        );
        // A core bodyless operation already has a ledger promise, but a copied
        // counted input prevents its conditional handback guarantee.
        let output = checked(
            "fn probe() { let item: List[u64] = [1]; let mut xs: unique List[List[u64]] = []; list.push(mut xs,copy item); let frozen = shared.freeze(take xs) }",
        );
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MEM-11");
        assert_eq!(
            output.sources.slice(d.primary),
            Some("shared.freeze(take xs)")
        );
        assert!(
            d.notes
                .iter()
                .all(|n| !n.contains("declare `@preserves_unique("))
        );
        let source = "@preserves_unique(xs)\nfn add(mut xs: List[u64]) { list.push(mut xs,1) }\nfn need(take xs: unique List[u64]) {}\nfn probe() { let mut xs: unique List[u64] = []; let f: fn(mut List[u64]) -> () = add; f(mut xs); need(take xs) }";
        rejected(source, "need(take xs)", false);
    });
}

#[test]
fn helper_unique_promise_19_checked_repairs_execute() {
    with_compiler_stack(|| {
        let source = r#"
type State { lines: List[u64] }
@preserves_unique(lines)
fn add(mut lines: List[u64], ok: bool) {
  list.push(mut lines, 1)
  failure.check(ok, Bounds())
}
fn need(take lines: unique List[u64]) -> u64 { list.len(lines) }
fn stamp(mut xs: unique List[u64], other: List[u64]) { xs[0] = 9 }
fn weak(mut xs: List[u64], other: List[u64]) { xs[0] = 8 }
fn pair(mut xs: unique List[u64], mut ys: unique List[u64], other: List[u64]) {
  xs[0] = 6
  ys[0] = 7
}
pub fn main() -> Result[(), ()] {
  let mut xs: unique List[u64] = [1]
  stamp(mut xs, [2])
  let named: fn(mut unique List[u64], List[u64]) -> () = stamp
  named(mut xs, [2])
  let scoped = fn(mut out: unique List[u64], other: List[u64]) { stamp(mut out, other) }
  scoped(mut xs, [2])
  if xs[0] != 9 { return Error(()) }
  let saved_xs = copy xs
  weak(mut xs, copy xs)
  if xs[0] != 8 || saved_xs[0] != 9 { return Error(()) }
  let mut left: unique List[u64] = [1]
  let mut right: unique List[u64] = [2]
  pair(mut left, mut right, [3])
  if left[0] != 6 || right[0] != 7 { return Error(()) }
  let mut state: unique State = State(lines: [])
  add(mut state.lines, true)
  let saved = copy state
  let mut fresh = unique state
  add(mut fresh.lines, true)
  let State(lines: lines) = take fresh
  if saved.lines != [1] || need(take lines) != 2 { return Error(()) }
  Ok(())
}
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(&path, source).unwrap();
        let mut program = load_program(&path).unwrap();
        program.check_for_execution().unwrap();
        let config = RunConfiguration {
            entry: program.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let execution =
            run_observed_checked(&mut program.engine, &config, SimulationConfig::default())
                .unwrap();
        let reference = native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty());
        let input = program.engine.export_native_input().unwrap();
        let validated = program.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let scratch =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("helper-unique-promise-19");
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &program.entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

#[test]
fn helper_unique_promise_19_copy_argument_is_independent() {
    with_compiler_stack(|| {
        for promise in ["", "@preserves_unique(xs)\n"] {
            let source = format!(
                "{promise}fn add(mut xs: List[u64]) {{ list.push(mut xs,1) }}\nfn need(take xs: unique List[u64]) {{}}\nfn probe() {{ let mut xs: unique List[u64] = []; add(mut xs); need(copy xs) }}"
            );
            rejected(&source, "need(copy xs)", false);
        }
    });
}
