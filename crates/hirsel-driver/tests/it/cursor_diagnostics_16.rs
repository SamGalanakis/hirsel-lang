//! Expectations derive from LANG-15's fixed cover and owned refill contract.
use hirsel_driver::*;
use hirsel_engine::*;

const TAGGED: &str = "// λ cursor witness\ntype Value = Leaf | Tagged(value: box Value)\nfn unboxed(value: box Value) -> Value in value uses {} {\n  match value { Leaf() => Leaf(), Tagged(value: inner) => Tagged(value: copy inner) }\n}\nfn probe(value: Value) {\n  let mut cursor = value\n  match cursor { Tagged(value: inner) => { cursor = unboxed(inner) }, _ => () }\n}\n";
const PAIR: &str = "// λ computed pair\ntype Value = Leaf | Array(List[Value])\nfn probe(a: Value, b: Value) {\n  let mut left: Value in {a, b} = a\n  match (left, b) { (Array(xs), Array(_)) => { left = xs[0u64] }, _ => () }\n}\n";

fn check(
    source: &str,
    expected: &[(&str, &str)],
    labels: &[(&str, &str, &str)],
) -> Vec<serde_json::Value> {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cursor.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert_eq!(errors.len(), expected.len(), "{source}: {errors:?}");
        for (d, (rule, primary)) in errors.iter().zip(expected) {
            assert_eq!(d.rule.as_str(), *rule, "{d:?}");
            assert_eq!(output.sources.slice(d.primary), Some(*primary));
            assert!(d.fix.is_none());
            let json = d.to_json(&output.sources);
            let human = d.render_human(&output.sources);
            let at = output.sources.resolve(d.primary);
            let start = source.find(*primary).unwrap();
            assert_eq!(
                (at.start as usize, at.end as usize),
                (start, start + primary.len())
            );
            assert_eq!(d.secondary.len(), labels.len(), "{json}");
            for (label, (needle, text, explanation)) in d.secondary.iter().zip(labels) {
                let start = source.find(*needle).unwrap() + needle.find(*text).unwrap();
                let at = output.sources.resolve(label.span);
                assert_eq!(
                    (at.start as usize, at.end as usize),
                    (start, start + text.len()),
                    "{json}"
                );
                assert_eq!(output.sources.slice(label.span), Some(*text));
                assert!(label.message.contains(explanation), "{json}");
                assert!(human.contains(&label.message), "{human}");
            }
            for note in json["notes"].as_array().unwrap() {
                let note = note.as_str().unwrap();
                assert!(
                    human.contains(note.strip_prefix("help: ").unwrap_or(note)),
                    "{human}"
                );
                assert!(
                    !note.contains("copy ")
                        && !note.contains("declare the result")
                        && !note.contains("broaden"),
                    "{json}"
                );
            }
            let warm = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                warm.diagnostics
                    .iter()
                    .map(|d| d.to_json(&warm.sources))
                    .collect::<Vec<_>>(),
                output
                    .diagnostics
                    .iter()
                    .map(|d| d.to_json(&output.sources))
                    .collect::<Vec<_>>()
            );
            eprintln!("{json}");
        }
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect()
    })
}

#[test]
fn cursor_diagnostics_16_copy_take_roots_and_aliases() {
    for refill in ["copy a", "take owned"] {
        check(
            &format!("// é\nfn f(a: Bytes, take owned: Bytes) {{ let mut c = a; c = {refill} }}"),
            &[("LANG-15", &format!("c = {refill}"))],
            &[
                ("let mut c", "c", "read-only borrow cursor"),
                (&format!("c = {refill}"), refill, "owned value"),
            ],
        );
    }
    check(
        "// é\nfn f(a: Bytes, b: Bytes) { let alias = a; let mut c = alias; c = b }",
        &[("MODE-1", "c = b")],
        &[
            ("let mut c", "c", "rooted in `a`"),
            ("b: Bytes", "Bytes", "view of `b`"),
        ],
    );
    check(
        "// é\nfn f(a: Bytes, b: Bytes) { let alias = a; let mut c: Bytes in {a,b} = alias; c = b }",
        &[],
        &[],
    );
    check(
        "// é\nfn f(a: Bytes) { let alias = a; let mut c = alias; c = a }",
        &[],
        &[],
    );
}

#[test]
fn cursor_diagnostics_16_inferred_pair_and_local_signature() {
    check(
        &PAIR.replace(": Value in {a, b}", ""),
        &[("MODE-1", "left = xs[0u64]")],
        &[
            ("let mut left", "left", "rooted in `a`"),
            ("Array(xs)", "xs", "view of `xs`"),
            ("(left, b)", "(left, b)", "distinct storage origin"),
        ],
    );
    check(
        "// λ local signature\nfn probe(a: Bytes) {\n fn owned(x: Bytes) -> Bytes in x { copy x }\n let mut c = a\n c = owned(a)\n}",
        &[("LANG-15", "c = owned(a)")],
        &[
            ("let mut c", "c", "read-only borrow cursor"),
            ("owned(a)", "owned(a)", "owned value"),
            (
                "-> Bytes in x",
                "Bytes in x",
                "result relation does not turn an owned return",
            ),
        ],
    );
}

#[test]
fn cursor_diagnostics_16_deterministic_across_session_salts() {
    let Some(runs) = hirsel_intern::under_salts(
        "cursor_diagnostics_16::cursor_diagnostics_16_deterministic_across_session_salts",
        &[1, 2, 3],
        || {
            serde_json::to_string(&check(
                PAIR,
                &[("MODE-1", "left = xs[0u64]")],
                &[
                    ("let mut left", "left", "rooted in `a`, `b`"),
                    ("Array(xs)", "xs", "view of `xs`"),
                    ("(left, b)", "(left, b)", "distinct storage origin"),
                ],
            ))
            .unwrap()
        },
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|run| run == &runs[0]));
}

fn check_recovery(source: &str, expected: &[(&str, &str)]) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("cursor.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        let actual: Vec<_> = output
            .diagnostics
            .iter()
            .map(|d| (d.rule.as_str(), output.sources.slice(d.primary).unwrap()))
            .collect();
        assert_eq!(actual, expected, "{source}");
        for d in &output.diagnostics {
            let json = d.to_json(&output.sources);
            let human = d.render_human(&output.sources);
            assert_eq!(json["rule"], d.rule.as_str());
            assert_eq!(json["message"], d.message);
            assert_eq!(json["severity"], "error");
            let at = output.sources.resolve(d.primary);
            assert_eq!(json["primary"]["start"], at.start);
            assert_eq!(json["primary"]["end"], at.end);
            assert!(human.contains(d.rule.as_str()), "{human}");
            assert!(human.contains(&d.message), "{human}");
            assert!(!human.contains("rooted in )"), "{human}");
        }
        let warm = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(
            warm.diagnostics
                .iter()
                .map(|d| d.to_json(&warm.sources))
                .collect::<Vec<_>>(),
            output
                .diagnostics
                .iter()
                .map(|d| d.to_json(&output.sources))
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn cursor_diagnostics_16_invalid_cover_recovery() {
    check_recovery(
        "fn f(a:List[u64])->u64 {let mut c:List[u64] in env=a; c=a; list.len(c)}",
        &[("LANG-15", "a")],
    );
    check_recovery(
        "fn f(a:List[u64])->u64 {let alias=a; let mut c:List[u64] in env=alias; c=a; list.len(c)}",
        &[("LANG-15", "alias")],
    );
    for cover in ["env", "{env,a}", "heap"] {
        check_recovery(
            &format!(
                "fn f(a:List[u64],b:List[u64])->u64 {{let mut c:List[u64] in {cover}=a; c=b; c=a; c=copy b; list.len(c)}}"
            ),
            &[("LANG-15", "a")],
        );
    }
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in b=a; c=b; c=a; list.len(c)}",
        &[("MODE-1", "a")],
    );
}

#[test]
fn cursor_diagnostics_16_invalid_cover_prerequisites() {
    check_recovery(
        "fn f(a:List[u64])->u64 {let mut c:List[u64] in {heap,a}=a; c=a; list.len(c)}",
        &[("LANG-7", "=")],
    );
    for (cover, rule) in [
        ("missing", "LANG-4"),
        ("a.nope", "LANG-4"),
        ("later", "LANG-2"),
    ] {
        check_recovery(
            &format!(
                "fn f(a:List[u64])->u64 {{let mut c:List[u64] in {cover}=a; let later=a; c=a; list.len(c)}}"
            ),
            &[(rule, &format!("List[u64] in {cover}"))],
        );
    }
    check_recovery(
        "fn f(a:List[u64])->u64{let mut c:List[u64] in a=a;c=a;list.len(c)}",
        &[],
    );
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in env=a; c=take b; list.len(c)}",
        &[("LANG-15", "a"), ("OWN-1", "take b")],
    );
    check_recovery(
        "fn f(a:List[u64],b:List[u64])->u64 {let mut c:List[u64] in env=a; c=a; let independent=take b; list.len(c)+list.len(independent)}",
        &[("LANG-15", "a"), ("OWN-1", "take b")],
    );
}

#[test]
fn cursor_diagnostics_16_matched_owned_producer() {
    let source = "// λ matched owned result\nfn f(a:List[u64],m:Map[u64,List[u64]])->u64{let mut c:List[u64] in {a,m}=a;let Some(next)=map.get(m,0u64) else{return 0};c=take next;list.len(c)}\npub fn main(){}";
    let diagnostics = check(
        source,
        &[("LANG-15", "c=take next;")],
        &[
            ("let mut c", "c", "read-only borrow cursor"),
            ("take next", "take next", "owned value"),
            ("map.get(m,0u64)", "map.get(m,0u64)", "owned result"),
            (
                "map.get(m,0u64)",
                "map.get",
                "result relation does not turn an owned return",
            ),
        ],
    );
    let diagnostic = &diagnostics[0];
    assert!(
        diagnostic["secondary"][3]["message"]
            .as_str()
            .unwrap()
            .contains("-> Option[List[u64]] in m"),
        "{diagnostic}"
    );
    let notes = diagnostic["notes"].to_string();
    assert!(
        notes.contains("cover") && notes.contains("`take`"),
        "{diagnostic}"
    );
    assert!(
        notes.contains("owned state") && notes.contains("borrowed"),
        "{diagnostic}"
    );
}

#[test]
fn cursor_diagnostics_16_owned_binding_chain_and_controls() {
    for (binding, refill) in [
        (
            "let Some(next)=map.get(m,0u64) else{return 0};",
            "take next",
        ),
        (
            "let receipt=map.get(m,0u64);let Some(next)=take receipt else{return 0};let moved=take next;",
            "take moved",
        ),
    ] {
        let source = format!(
            "fn f(a:List[u64],m:Map[u64,List[u64]])->u64{{let mut c:List[u64] in {{a,m}}=a;{binding}c={refill};list.len(c)}}"
        );
        check(
            &source,
            &[("LANG-15", &format!("c={refill};"))],
            &[
                ("let mut c", "c", "read-only borrow cursor"),
                (&format!("c={refill};"), refill, "owned value"),
                ("map.get(m,0u64)", "map.get(m,0u64)", "owned result"),
                (
                    "map.get(m,0u64)",
                    "map.get",
                    "result relation does not turn an owned return",
                ),
            ],
        );
    }
    // A borrowed item still needs the map in its fixed cover.
    check_recovery(
        "fn f(a:List[u64],m:Map[u64,List[u64]])->u64{let mut c=a;for (key,item) in m {if key==0u64 {c=item;return list.len(c)}};0}",
        &[("MODE-1", "c=item;")],
    );
    // A local owned temporary's view does not originate in a or m.
    check_recovery(
        "fn f(a:List[u64],m:Map[u64,List[u64]])->u64{let mut c:List[u64] in {a,m}=a;let next=[1u64];c=next;list.len(c)}",
        &[("MODE-1", "c=next;")],
    );
    check_recovery(
        "fn f(a:List[u64],m:Map[u64,List[u64]])->u64{let mut c:List[u64] in {a,m}=a;let Some(next)=map.get(m,0u64) else{return 0};c=take next;let independent=take a;list.len(c)}",
        &[("LANG-15", "c=take next;"), ("OWN-1", "take a")],
    );
    // Do not attribute a replaced mutable value to its old call producer.
    let ds = check(
        "fn owned(a:Bytes)->Bytes in a{copy a}\nfn f(a:Bytes){let mut c=a;let mut next=owned(a);next=copy a;c=take next}",
        &[("LANG-15", "c=take next")],
        &[
            ("let mut c", "c", "read-only borrow cursor"),
            ("take next", "take next", "owned value"),
        ],
    );
    assert_eq!(ds.len(), 1);
}

fn cursor_lookup_recipe() -> String {
    let docs = include_str!("../../../../docs/spec/language.md");
    let recipe = docs
        .split("// cursor-owned-refill borrowing recipe\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let helper = recipe.split("pub fn main()").next().unwrap();
    format!(
        "{helper}\nfn owned_length(a:List[u64],m:Map[u64,List[u64]])->u64 {{let mut c:List[u64] in {{a,m}}=copy a;let Some(next)=map.get(m,0u64) else{{return 0}};c=take next;list.len(c)}}\n"
    )
}

#[test]
fn cursor_diagnostics_16_lookup_recipes_check_export_and_observe() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let source = format!(
            "{}{}",
            cursor_lookup_recipe(),
            r"
pub fn main()->Result[(),u64] {
  let a=[9u64,8u64,7u64]
  let mut m:Map[u64,List[u64]]=map.empty()
  if borrowed_length(a,m)!=0u64 || owned_length(a,m)!=0u64 {return Error(1)}
  map.insert(mut m,1u64,[6u64])
  if borrowed_length(a,m)!=0u64 || owned_length(a,m)!=0u64 {return Error(2)}
  map.insert(mut m,0u64,[])
  if borrowed_length(a,m)!=0u64 || owned_length(a,m)!=0u64 {return Error(3)}
  map.insert(mut m,0u64,[4u64,5u64])
  if borrowed_length(a,m)!=2u64 || owned_length(a,m)!=2u64 {return Error(4)}
  Ok(())
}
"
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("lookup.hir", source.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let entry = entry_path(&default_context());
        for name in ["borrowed_length", "owned_length", "main"] {
            let mut path = entry.clone();
            path.0.name = name.into();
            let tir = engine.export_tir(&path).unwrap();
            engine.import_tir(&tir).unwrap();
        }
        let run = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            Default::default(),
        )
        .unwrap();
        let reference = Observation::of_reference(run.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0));
        assert!(reference.failures.is_empty());
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.validation = Validation::EveryStage;
        let matrix = vec![
            Configuration {
                options: options.clone(),
                engine: Engine::Heap { campaign: true },
            },
            Configuration {
                options: options.clone(),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options,
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let directory = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("cursor-owned-refill-repairs-21");
        std::fs::create_dir_all(&directory).unwrap();
        let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
        let input = engine.export_native_input().unwrap();
        let input = engine.import_native_input(input).unwrap();
        let outcomes = differential(&input, &entry, &reference, &matrix, &toolchain, &directory);
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
        eprintln!(
            "lookup recipes: 4 hit/miss vectors; reference + 3 H/LLVM/Cranelift agreements; 3 TIR round trips"
        );
    });
}
