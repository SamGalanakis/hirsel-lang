//! MODE-2 explanations follow failed-flow facts; verdicts and primary sites stay fixed.
use hirsel_driver::*;
use hirsel_engine::*;

const KEY: &str = r#"// λ counted key
pub fn main() -> Result[(), ()] {
  let key = f"word"
  let mut counts: unique Map[String, u32] = map.empty()
  map.insert(mut counts, copy key, 1)
  std@1/map.update(mut counts, key, fn(mut n: unique u32) -> () uses {} { n += 1 })
  Ok(())
}
"#;

const LOOP: &str = r#"fn probe(take ops: List[u64]) {
  let mut m: unique Map[u64,List[u64]] = map.empty()
  for tag in ops {
    if tag == 0 { map.insert(mut m, 0, [1]); let _ = map.get(m,0) }
    else { let _ = map.remove(mut m,0) }
    for (_,value) in m { let _ = unique value; break }
  }
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

const BRANCH: &str = r#"fn probe(b: bool) {
  let mut m: unique Map[String,u64] = map.empty()
  let k = f"word"
  if b { map.insert(mut m, copy k, 1) }
  else { map.insert(mut m, copy k, 2) }
  let _ = map.remove(mut m,k)
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

const HANDBACK: &str = r#"fn edit(mut m: Map[u64,u64]) {}
fn probe() {
  let mut m: unique Map[u64,u64] = map.empty()
  edit(mut m)
  let _ = map.remove(mut m,0)
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

const LOOKUP: &str = r#"fn probe() {
  let mut m: unique Map[u64,List[u64]] = map.empty()
  map.insert(mut m, 0, [1])
  let saved = map.get(m,0)
  map.remove(mut m,0)
  let _ = saved
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

const LOOKUP_ENDED: &str = r#"fn probe() {
  let mut m: unique Map[u64,List[u64]] = map.empty()
  map.insert(mut m, 0, [1])
  let saved = map.get(m,0)
  let _ = saved
  let _ = map.remove(mut m,0)
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

const PLAIN: &str = r#"// λ counted key
pub fn main() -> Result[(), ()] {
  let key = 1u64
  let mut counts: unique Map[u64, u32] = map.empty()
  map.insert(mut counts, copy key, 1)
  std@1/map.update(mut counts, key, fn(mut n: unique u32) -> () uses {} { n += 1 })
  Ok(())
}
"#;

const KEY_REPAIR: &str = r#"// λ counted key
pub fn main() -> Result[(), ()] {
  let key = f"word"
  let mut counts: unique Map[String, u32] = map.empty()
  map.insert(mut counts, unique copy key, 1)
  std@1/map.update(mut counts, key, fn(mut n: unique u32) -> () uses {} { n += 1 })
  Ok(())
}
"#;

const HANDBACK_REPAIR: &str = r#"fn edit(mut m: Map[u64,u64]) {}
fn probe() {
  let mut m: unique Map[u64,u64] = map.empty()
  edit(mut m)
  m = unique m
  let _ = map.remove(mut m,0)
}
pub fn main() -> Result[(), ()] { Ok(()) }
"#;

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("loss.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    let warm = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect::<Vec<_>>(),
        warm.diagnostics
            .iter()
            .map(|d| d.to_json(&warm.sources))
            .collect::<Vec<_>>()
    );
    output
}

fn rejected(source: &str, rule: &str, primary: &str, causes: &[&str], message: &str) {
    let output = checked(source);
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "{source}: {errors:?}");
    let d = errors[0];
    eprintln!("{}", d.render_human(&output.sources));
    assert_eq!(d.rule.as_str(), rule);
    assert_eq!(output.sources.slice(d.primary), Some(primary));
    let at = output.sources.resolve(d.primary);
    let start = source.find(primary).unwrap();
    assert_eq!(
        (at.start as usize, at.end as usize),
        (start, start + primary.len())
    );
    assert!(d.message.contains(message), "{d:?}");
    assert!(!d.message.contains("map.empty()"));
    assert!(d.fix.is_none());
    for &cause in causes {
        let labels: Vec<_> = d
            .secondary
            .iter()
            .filter(|l| output.sources.slice(l.span) == Some(cause))
            .collect();
        assert!(
            !labels.is_empty(),
            "missing {cause}: {:?}",
            d.to_json(&output.sources)
        );
        for label in labels {
            let at = output.sources.resolve(label.span);
            assert_eq!(&source[at.start as usize..at.end as usize], cause);
            assert!(d.render_human(&output.sources).contains(&label.message));
        }
    }
    assert!(
        d.secondary
            .iter()
            .all(|l| output.sources.slice(l.span) != Some("map.empty()"))
    );
}

#[test]
fn uniqueness_loss_diagnostics_17_copied_counted_inputs_and_plain_twins() {
    with_compiler_stack(|| {
        rejected(
            KEY,
            "MODE-2",
            "std@1/map.update(mut counts, key, fn(mut n: unique u32) -> () uses {} { n += 1 })",
            &["copy key"],
            "lost its deep uniqueness",
        );
        let value = "fn probe() { let value: List[u64] = [1u64]; let mut m: unique Map[u64,List[u64]] = map.empty(); map.insert(mut m, 0, copy value); let _ = map.remove(mut m,0) }";
        rejected(
            value,
            "MODE-2",
            "map.remove(mut m,0)",
            &["copy value"],
            "lost its deep uniqueness",
        );
        for source in [KEY_REPAIR, PLAIN, LOOKUP_ENDED, HANDBACK_REPAIR] {
            let output = checked(source);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .all(|d| d.severity != hirsel_diag::Severity::Error),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_loop_branch_and_handback() {
    with_compiler_stack(|| {
        rejected(
            LOOP,
            "MODE-2",
            "map.remove(mut m,0)",
            &["map.get(m,0)"],
            "not proved deeply unique",
        );
        let output = checked(LOOP);
        assert!(
            output.diagnostics[0]
                .secondary
                .iter()
                .any(|l| l.message.contains("previous iteration"))
        );
        rejected(
            BRANCH,
            "MODE-2",
            "map.remove(mut m,k)",
            &["copy k"],
            "not proved deeply unique",
        );
        let output = checked(BRANCH);
        assert_eq!(
            output.diagnostics[0]
                .secondary
                .iter()
                .filter(|l| output.sources.slice(l.span) == Some("copy k"))
                .count(),
            2
        );
        let inferred = checked(HANDBACK);
        assert!(
            inferred.diagnostics.is_empty(),
            "{:?}",
            inferred.diagnostics
        );
        let public_handback = HANDBACK.replacen("fn edit", "pub fn edit", 1);
        rejected(
            &public_handback,
            "MODE-2",
            "map.remove(mut m,0)",
            &["edit(mut m)"],
            "lost its deep uniqueness",
        );
        let output = checked(&public_handback);
        assert!(output.diagnostics[0].notes.iter().any(
            |note| note.contains("@preserves_unique(m)")
                && note.contains("`edit`")
                && note.contains("only if")
        ));
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_existing_verdicts_and_fresh_replacements() {
    with_compiler_stack(|| {
        // A live lookup's exclusive-access diagnostic precedes MODE-2.
        rejected(
            LOOKUP,
            "MODE-1",
            "map.remove(mut m,0)",
            &["saved"],
            "borrowed",
        );
        let take = "type V2 { name: String }\nfn migrate(take config: V2) -> Result[unique V2, ()] { Ok(take config) }";
        let output = checked(take);
        assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
        assert_eq!(output.diagnostics[0].rule.as_str(), "MODE-2");
        assert_eq!(
            output.sources.slice(output.diagnostics[0].primary),
            Some("Ok(take config)")
        );
        let fresh = "fn edit(mut m: Map[u64,u64]) {}\nfn probe() { let mut m: unique Map[u64,u64] = map.empty(); edit(mut m); m = map.empty(); let _ = map.remove(mut m,0) }";
        let prior = "fn probe() { let mut m: unique Map[u64,List[u64]] = map.empty(); map.insert(mut m, 0, [1]); let saved = copy m; let _ = map.get(m,0); let _ = map.remove(mut m,0); let _ = saved }";
        rejected(
            prior,
            "MODE-2",
            "map.remove(mut m,0)",
            &["copy m", "map.get(m,0)"],
            "not proved deeply unique",
        );
        let plain_part = "type State { n: u64, items: List[u64] }\nfn edit(mut n: u64) {}\nfn need(mut s: unique State) {}\nfn probe() { let mut s: unique State = State(n: 0, items: [1]); let saved = copy s; edit(mut s.n); need(mut s); let _ = saved }";
        let output = checked(plain_part);
        assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert_eq!(output.sources.slice(d.primary), Some("need(mut s)"));
        assert!(
            d.secondary
                .iter()
                .any(|l| output.sources.slice(l.span) == Some("copy s"))
        );
        assert!(
            d.secondary
                .iter()
                .all(|l| output.sources.slice(l.span) != Some("edit(mut s.n)"))
        );
        let output = checked(fresh);
        assert!(
            output
                .diagnostics
                .iter()
                .all(|d| d.severity != hirsel_diag::Severity::Error),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_checked_repairs_execute() {
    with_compiler_stack(|| {
        // One executable exercises both deep-copy repairs plus no-share plain
        // keys. Each native backend builds once for the complete fixture.
        let source = r#"
pub fn main() -> Result[(), ()] {
  let key = f"word"
  let mut m: unique Map[String,u32] = map.empty()
  map.insert(mut m, unique copy key, 1)
  let changed = std@1/map.update(mut m, key, fn(mut n: unique u32) -> () uses {} { n += 1 })
  if !changed || map.get(m,key) != Some(2) { return Error(()) }
  let mut plain: unique Map[u64,u64] = map.empty()
  map.insert(mut plain, 1, 7)
  let saved = copy plain
  plain = unique plain
  if map.remove(mut plain,1) != Some(7) || map.get(saved,1) != Some(7) { return Error(()) }
  let source: List[List[String]] = [[f"a"], [f"b", f"c"]]
  let mut rows: unique List[List[String]] = []
  for line in source { list.push(mut rows, unique line) }
  list.push(mut rows[0], f"independent")
  if list.len(rows) != 2 { return Error(()) }
  if list.len(source[0]) != 1 || list.len(rows[0]) != 2 { return Error(()) }
  consume_rows(take rows)
  Ok(())
}
fn consume_rows(take rows: unique List[List[String]]) {}
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
        let scratch = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("uniqueness-loss-diagnostics-17");
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

// Owned-versus-borrowed and empty/multiple vectors follow Rust alloc vec
// into_iter.rs:265-300 and alloctests/tests/vec.rs:1040-1070 at
// f6c1107f8be13e48465e12c0fd26e6a39fe18e4f. Hirsel item modes remain
// independent of the source marker (unlike importing Rust's ownership rule).
fn loop_rows(marker: &str, elements: &str, append: &str, values: &str) -> String {
    format!(
        "fn need(take xs: unique List[String]) {{}}\nfn probe() {{ let mut source: List[{elements}] = {values}; let mut out: unique List[String] = []; for line in {marker}source {{ list.push(mut out, {append}) }} need(take out) }}"
    )
}

#[test]
fn uniqueness_loss_diagnostics_17_loop_item_contract_20() {
    with_compiler_stack(|| {
        for (marker, access, source_site) in [
            ("", "source is borrowed", "source"),
            ("take ", "consumes the sequence", "take source"),
            ("mut ", "element places", "mut source"),
        ] {
            for values in ["[]", "[f\"a\"]", "[f\"a\", f\"b\", f\"c\"]"] {
                let source = loop_rows(marker, "String", "copy line", values);
                rejected(
                    &source,
                    "MODE-2",
                    "need(take out)",
                    &["copy line", "line", source_site],
                    "not proved deeply unique",
                );
                let output = checked(&source);
                let d = &output.diagnostics[0];
                assert!(
                    d.secondary
                        .iter()
                        .any(|l| output.sources.slice(l.span) == Some("line")
                            && l.message
                                .contains("declared item uniqueness: not proved unique")
                            && l.message.contains(
                                "current item uniqueness at this input: not proved unique"
                            )),
                    "{d:?}"
                );
                assert!(
                    d.secondary
                        .iter()
                        .any(|l| output.sources.slice(l.span) == Some(source_site)
                            && l.message.contains(access)),
                    "{d:?}"
                );
                assert!(
                    d.notes
                        .iter()
                        .any(|n| n.contains("use `unique line`") && n.contains("per item")),
                    "{d:?}"
                );
                assert!(d.notes.iter().all(|n| !n.contains("@preserves_unique")));
                let repaired = loop_rows(marker, "String", "unique line", values);
                let output = checked(&repaired);
                assert!(
                    output.diagnostics.is_empty(),
                    "{repaired}: {:?}",
                    output.diagnostics
                );
            }
        }
        // unique-empty-builder-29 (4a4b9377): consuming a currently unique
        // owner preserves its items' flow proof despite unqualified rows.
        let owned = loop_rows("take ", "String", "take line", "[f\"a\"]")
            .replace("source: List", "source: unique List");
        let output = checked(&owned);
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let output = checked(&owned.replace(
            "list.push(mut out, take line)",
            "list.push(mut out, unique line)",
        ));
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        // The original literal witness retains its exact primary and causes.
        let incident = include_str!("../../../../examples/wc2/repros/incident-9.hir");
        rejected(
            incident,
            "MODE-2",
            "need(take out)",
            &["copy line", "line", "source"],
            "not proved deeply unique",
        );
        // The item storage promise is stronger even when the root is plain.
        let source = loop_rows("take ", "unique String", "take line", "[f\"a\", f\"b\"]");
        let output = checked(&source);
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        let copied = loop_rows("take ", "unique String", "copy line", "[f\"a\", f\"b\"]");
        let output = checked(&copied);
        let d = &output.diagnostics[0];
        assert_eq!(d.rule.as_str(), "MODE-2", "{d:?}");
        assert!(
            d.secondary
                .iter()
                .any(|l| l.message.contains("declared item uniqueness: unique")
                    && l.message
                        .contains("current item uniqueness at this input: not proved unique")),
            "{d:?}"
        );
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_loop_item_conditional_repairs_20() {
    with_compiler_stack(|| {
        let generic = "fn need[T](take xs: unique List[T]) {}\nfn probe[T: copyable](take source: List[T]) { let mut out: unique List[T] = []; for line in take source { list.push(mut out,copy line) } need(take out) }";
        let resource = "resource type Row { text: String } drop(x) {}\nfn need(take xs: unique List[Row]) {}\nfn probe(take source: List[Row]) { let mut out: unique List[Row] = []; for line in take source { list.push(mut out,take line) } need(take out) }";
        let arena = "fn need(a: Arena, take xs: unique List[String] in a) {}\nfn probe(a: Arena, take source: List[String] in a) { let mut out: unique List[String] in a = []; for line in take source { list.push(mut out,copy line) } need(a,take out) }";
        let noalloc = "@noalloc\nfn probe(take source: List[String]) { let mut out: unique List[String] = []; for line in take source { list.push(mut out,copy line) } need(take out) }\n@noalloc\nfn need(take xs: unique List[String]) {}";
        for source in [generic, resource, arena, noalloc] {
            let output = checked(source);
            assert_eq!(
                output.diagnostics[0].rule.as_str(),
                if source == arena {
                    "PROV-4"
                } else if source == noalloc {
                    "MEM-5"
                } else {
                    "MODE-2"
                }
            );
            let d = output
                .diagnostics
                .iter()
                .find(|d| d.rule.as_str() == "MODE-2")
                .unwrap_or_else(|| panic!("{source}: {:?}", output.diagnostics));
            assert_eq!(
                output.sources.slice(d.primary),
                Some(if source == arena {
                    "need(a,take out)"
                } else {
                    "need(take out)"
                }),
                "{d:?}"
            );
            assert!(
                d.notes
                    .iter()
                    .any(|n| n.contains("only if its element contract permits it")
                        && n.contains("arena bounds remain")
                        && n.contains("@noalloc")),
                "{d:?}"
            );
            assert!(
                d.notes.iter().all(|n| !n.contains("use `unique line`")),
                "{d:?}"
            );
        }
        // A resource must never receive a concrete deep-copy recipe.
        let invalid = resource.replace("take line", "unique line");
        let output = checked(&invalid);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "OWN-5"),
            "{:?}",
            output.diagnostics
        );
    });
}

// Ordinary counted copies stay distinct from confined related-call results.

fn uniqueness_repairs_21_accepted(source: &str) {
    let mut engine = crate::source_std::engine();
    engine.verify = Verify::On;
    let file = engine
        .sources
        .add("share-repair.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    assert!(
        output.diagnostics.is_empty(),
        "{source}\n{:?}",
        output.diagnostics
    );
    let mut input = engine.export_native_input().unwrap();
    for definition in input.definitions.values_mut() {
        *definition = std::sync::Arc::new(definition.as_ref().clone());
    }
    engine.import_native_input(input).unwrap();
}

#[test]
fn uniqueness_loss_diagnostics_17_persistent_alias_controls_21() {
    with_compiler_stack(|| {
        for body in [
            "let alias=copy xs;need(take xs);let _=alias",
            "{let alias=copy xs};need(take xs)",
            "let alias=copy xs;let _=alias;need(take xs)",
        ] {
            let source = format!(
                "fn need(take xs:unique List[u64]) {{}}\npub fn main(){{let xs:unique List[u64]=[1];{body}}}"
            );
            let output = checked(&source);
            let [d] = &output.diagnostics[..] else {
                panic!("{source}: {:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "MODE-2");
            assert_eq!(output.sources.slice(d.primary), Some("need(take xs)"));
            assert!(
                d.secondary
                    .iter()
                    .any(|l| output.sources.slice(l.span) == Some("copy xs")
                        && l.message.contains("shares counted storage"))
            );
            let notes = d.to_json(&output.sources)["notes"].to_string();
            assert!(notes.contains("does not restore"), "{d:?}");
            assert!(
                !notes.contains("scope that ends")
                    && !notes.contains("End live shares")
                    && !notes.contains("@preserves_unique"),
                "{d:?}"
            );
            uniqueness_repairs_21_accepted(&source.replace("need(take xs)", "need(unique xs)"));
        }
        // A read loan with a plain result never creates a counted owner.
        uniqueness_repairs_21_accepted(
            "fn read(xs:List[u64])->u64 {list.len(xs)}\nfn need(take xs:unique List[u64]) {}\npub fn main(){let xs:unique List[u64]=[1];let n=read(xs);need(take xs);let _=n}",
        );
        // The same spelled transfer is valid before sharing, and a mutable
        // binding can explicitly replace its root while preserving a snapshot.
        uniqueness_repairs_21_accepted(
            "fn need(take xs:unique List[u64]) {}\npub fn main(){let xs:unique List[u64]=[1];need(take xs)}",
        );
        uniqueness_repairs_21_accepted(
            "fn need(take xs:unique List[u64]) {}\npub fn main(){let mut xs:unique List[u64]=[1];let alias=copy xs;xs=unique xs;need(take xs);let _=alias}",
        );
        let unrelated = "fn unrelated(mut ys:List[u64]) {}\nfn need(take xs:unique List[u64]) {}\npub fn main(){let xs:unique List[u64]=[1];let alias=copy xs;let mut ys:List[u64]=[];unrelated(mut ys);need(take xs);let _=alias}";
        let output = checked(unrelated);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert!(
            d.secondary
                .iter()
                .all(|l| output.sources.slice(l.span) != Some("unrelated(mut ys)"))
        );
        assert!(d.notes.iter().all(|n| !n.contains("@preserves_unique")));
        uniqueness_repairs_21_accepted(&unrelated.replace("need(take xs)", "need(unique xs)"));
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_noalloc_unique_supplier_21() {
    with_compiler_stack(|| {
        let source = "@noalloc fn forward(take x:List[String])->unique List[String]{take x}\nfn need(take x:unique List[String]) {}\npub fn main(){let x:unique List[String]=[f\"a\"];let y=forward(take x);need(take y)}";
        let output = checked(source);
        let d = output
            .diagnostics
            .iter()
            .find(|d| d.rule.as_str() == "MODE-2")
            .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
        eprintln!("{}", d.render_human(&output.sources));
        let notes = d.to_json(&output.sources)["notes"].to_string();
        assert!(notes.contains("unavailable under `@noalloc`"), "{d:?}");
        assert!(
            !notes.contains("pass `unique x`") && !notes.contains("x = unique x"),
            "{d:?}"
        );
        let repaired = source.replace("take x:List[String]", "take x:unique List[String]");
        uniqueness_repairs_21_accepted(&repaired);
        // Caller updates must transfer an owner that was already unique.
        for invalid in [
            repaired.replace("forward(take x)", "forward(copy x)"),
            repaired.replace(
                "let y=forward(take x)",
                "let alias=copy x;let y=forward(take x);let _=alias",
            ),
        ] {
            let output = checked(&invalid);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-2"),
                "{invalid}: {:?}",
                output.diagnostics
            );
        }
        let output = checked(&source.replace("{take x}", "{unique x}"));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() == "MEM-5"),
            "{:?}",
            output.diagnostics
        );
    });
}

#[test]
fn uniqueness_loss_diagnostics_17_noncopyable_supplier_21() {
    with_compiler_stack(|| {
        let source = "resource type Row {text:String} drop(x) {}\nfn forward[T](take x:T)->unique T {take x}\nfn need(take x:unique Row) {}\npub fn main(){let x:unique Row=Row(text:f\"a\");let y=forward(take x);need(take y)}";
        let output = checked(source);
        let d = output
            .diagnostics
            .iter()
            .find(|d| d.rule.as_str() == "MODE-2")
            .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
        eprintln!("{}", d.render_human(&output.sources));
        let notes = d.to_json(&output.sources)["notes"].to_string();
        assert!(notes.contains("requires copyable data"), "{d:?}");
        assert!(
            !notes.contains("pass `unique x`") && !notes.contains("x = unique x"),
            "{d:?}"
        );
        uniqueness_repairs_21_accepted(&source.replace("take x:T", "take x:unique T"));
        let output = checked(&source.replace("{take x}", "{unique x}"));
        assert!(
            output
                .diagnostics
                .iter()
                .any(|d| d.rule.as_str() != "MODE-2"),
            "{:?}",
            output.diagnostics
        );
    });
}
