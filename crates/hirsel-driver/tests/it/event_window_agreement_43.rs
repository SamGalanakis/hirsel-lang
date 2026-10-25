//! MODE-2 plain hand-back and CAP-2 generic cursor authority, using history's
//! real source/export/import and four-engine runner (own-selector-27).
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_tir::*;
use hirsel_types::*;

#[path = "../support/history.rs"]
#[allow(dead_code)]
mod history;

const ADVANCE: &str = include_str!("../support/event_window_agreement_43/advance.hir");
const KEEP: &str = include_str!("../support/event_window_agreement_43/keep.hir");

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("event-window.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(checked.completion, CheckCompletion::Complete);
    assert!(
        checked.diagnostics.is_empty(),
        "{source}\n{:?}",
        checked.diagnostics
    );
    engine
}

fn check_export(source: &str) {
    let mut engine = checked(source);
    let input = engine.export_native_input().unwrap();
    engine.import_native_input(input).unwrap();
    println!("EW43 source / V export / V import accepted");
}

fn rejected(source: &str, rule: &str, primary: &str) {
    for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(&path, source).unwrap();
        // An explicit standalone manifest avoids an unrelated std precheck.
        let manifest = dir.path().join("hirsel.json");
        std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
        let mut program = load_program(&manifest).unwrap();
        let output = program.check(policy).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|d| d.severity == hirsel_diag::Severity::Error)
            .collect();
        assert!(!errors.is_empty(), "{policy:?}: {source}");
        for error in errors {
            assert_eq!(error.rule.as_str(), rule, "{source}\n{error:?}");
            assert_eq!(
                output.sources.slice(error.primary),
                Some(primary),
                "{error:?}"
            );
            assert_eq!(
                output.sources.resolve(error.primary).start,
                if primary == "take xs" {
                    (source.find("for item in take xs").unwrap() + "for item in ".len()) as u32
                } else {
                    source.rfind(primary).unwrap() as u32
                }
            );
        }
        println!("EW43 {policy:?} rejected {rule} at {primary}");
    }
}

fn runtime(name: &str, source: &str) {
    let directory = tempfile::tempdir().unwrap();
    let runner = history::Runner::new(directory.path()).unwrap();
    let report = runner.run(name, source);
    assert!(report.validated, "{source}\n{report:?}");
    assert_eq!(report.cells.len(), 4, "{report:?}");
    assert!(report.agrees(), "{report:?}");
    for cell in report.cells {
        println!(
            "EW43 {name} {} {:?} {:?}",
            cell.engine, cell.category, cell.observation
        );
        assert_eq!(cell.category, history::Category::Observation, "{cell:?}");
        let observed = cell.observation.unwrap();
        assert_eq!(observed.exit_code, Some(0), "{source}\n{observed:?}");
        assert!(
            observed.output.is_empty()
                && observed.failures.is_empty()
                && !observed.blocked
                && observed.budget.is_none()
                && observed.signaled.is_none()
                && observed.roots.is_empty(),
            "{observed:?}"
        );
    }
}

#[test]
fn original_plain_owned_loop_rejected_at_source() {
    with_compiler_stack(|| rejected(KEEP, "CAP-1", "take xs"));
}

#[test]
fn handback_direct_named_indirect_and_staging_four_engines() {
    runtime(
        "handback-matrix",
        r#"
fn bump(mut x: u64) uses {} { x = x + 10 }
fn direct(take xs: unique List[u64]) -> unique List[u64] uses {} {
  let mut owned = take xs
  for i in 0u64..list.len(owned) { bump(mut owned[i]) }
  take owned
}
fn indirect[T: plain](take xs: unique List[T], change: fn(mut T)->() uses {}) -> unique List[T] uses {} {
  let mut owned = take xs
  for i in 0u64..list.len(owned) { change(mut owned[i]) }
  take owned
}
fn staged(take xs: unique List[u64], change: fn(mut u64)->() uses {}) -> unique List[u64] uses {} {
  let mut owned = take xs
  for i in 0u64..list.len(owned) {
    let mut value = owned[i]
    change(mut value)
    owned[i] = value
  }
  take owned
}
fn expect(take xs: unique List[u64], n: u64)->Result[(),u64] uses {} {
  if list.len(xs) != n { return Error(1) }
  for i in 0u64..n { if xs[i] != i+11 { return Error(2) } }
  let mut returned = take xs
  list.push(mut returned, 99)
  if list.len(returned) != n+1 || returned[n] != 99 { return Error(3) }
  Ok(())
}
pub fn main()->Result[(),u64] uses {} {
  for n in 0u64..3u64 {
    let mut a: unique List[u64] = []
    let mut b: unique List[u64] = []
    let mut c: unique List[u64] = []
    let mut d: unique List[u64] = []
    for i in 0u64..n {
      list.push(mut a, i+1); list.push(mut b, i+1)
      list.push(mut c, i+1); list.push(mut d, i+1)
    }
    expect(direct(take a), n)?
    expect(indirect(take b, bump), n)?
    expect(indirect(take c, fn(mut x:u64) uses {} { x=x+10 }), n)?
    expect(staged(take d, bump), n)?
  }
  Ok(())
}
"#,
    );
}

#[test]
fn owned_borrowed_and_indexed_loop_controls_four_engines() {
    runtime(
        "loop-matrix",
        r#"
fn inert[T: plain + inert](take xs: unique List[T])->unique List[T] uses {} {
  let mut out: List[T] = []
  for item in take xs { list.push(mut out, item) }
  take out
}
fn selected[T: plain](take xs: unique List[T])->unique List[T] uses {xs} {
  let mut out: List[T] = []
  for item in take xs { list.push(mut out, item) }
  take out
}
fn indexed[T: plain](take xs: unique List[T])->unique List[T] uses {} {
  let mut out: List[T] = []
  for i in 0u64..list.len(xs) { list.push(mut out, xs[i]) }
  take out
}
fn borrowed[T: plain](xs: List[T])->unique List[T] uses {} {
  let mut out: List[T] = []
  for item in xs { list.push(mut out, item) }
  take out
}
fn expect(take xs: unique List[u64], n:u64)->Result[(),u64] uses {} {
  if list.len(xs)!=n {return Error(1)}
  for i in 0u64..n {if xs[i]!=i+1 {return Error(2)}}
  let mut returned=take xs
  list.push(mut returned,99)
  if returned[n]!=99 {return Error(3)}
  Ok(())
}
pub fn main()->Result[(),u64] uses {} {
  for n in 0u64..3u64 {
    let mut a:unique List[u64]=[]
    let mut b:unique List[u64]=[]
    let mut c:unique List[u64]=[]
    let mut d:unique List[u64]=[]
    for i in 0u64..n {
      list.push(mut a,i+1); list.push(mut b,i+1)
      list.push(mut c,i+1); list.push(mut d,i+1)
    }
    expect(inert(take a),n)?
    expect(selected(take b),n)?
    expect(indexed(take c),n)?
    expect(borrowed(d),n)?
    if list.len(d)!=n {return Error(4)}
  }
  Ok(())
}
"#,
    );
}

#[test]
fn authority_bearing_plain_element_still_requires_selector() {
    with_compiler_stack(|| {
        // Dir is both plain and authority-bearing; u64 instantiation cannot
        // discharge the generic definition's missing inert evidence.
        let source = KEEP
            .replace("pub fn main()", "pub fn main(d: Dir)")
            .replace("List[u64] = [1u64]", "List[Dir] = [d]");
        rejected(&source, "CAP-1", "take xs");
        check_export(&source.replace("uses {}", "uses {xs}"));
    });
}

fn signatures(input: &NativeInput) -> hirsel_validate::SignatureMap {
    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
    signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
    signatures
}

#[test]
fn v_rejects_erased_generic_cursor_uses() {
    with_compiler_stack(|| {
        let mut engine = checked(&KEEP.replace("uses {}", "uses {xs}"));
        let input = engine.export_native_input().unwrap();
        let (path, original) = input
            .definitions
            .iter()
            .find(|(p, _)| p.0.name == "keep")
            .unwrap();
        let mut signatures = signatures(&input);
        let SignatureKind::Function(function) = &mut signatures.get_mut(path).unwrap().kind else {
            panic!()
        };
        function.uses = Uses::Only(AuthoritySummary::inert());
        let violations =
            hirsel_validate::validate(original, &input.types, &signatures, &mut vec![]).violations;
        assert!(
            violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "body uses"
                }
            )),
            "{violations:?}"
        );
        println!("EW43 V erased uses rejected: {violations:?}");
    });
}

#[test]
fn v_rejects_lost_owner_modes_and_alias_before_consume() {
    with_compiler_stack(|| {
        // Port the existing unique-empty-builder-29 forged cursor vectors:
        // a retained count before consumption must defeat the unique receipt.
        for source in [
            "fn build(take xs: List[String])->List[String] { let mut out:List[String]=[]; for item in take xs { list.push(mut out,take item) }; take out }",
            "fn build(take xs: unique List[String])->List[String] { let alias=copy xs; let mut out:List[String]=[]; for item in take xs { list.push(mut out,take item) }; take out }",
            "fn build(take xs: List[u64], change:fn(mut u64)->() uses {})->List[u64] uses {} { let mut owned=take xs; change(mut owned[0]); take owned }",
        ] {
            let mut engine = checked(source);
            let input = engine.export_native_input().unwrap();
            let (path, original) = input
                .definitions
                .iter()
                .find(|(p, _)| p.0.name == "build")
                .unwrap();
            let mut forged = (**original).clone();
            let mut signatures = signatures(&input);
            let SignatureKind::Function(function) = &mut signatures.get_mut(path).unwrap().kind
            else {
                panic!()
            };
            let mut row = input.types.row(function.result).clone();
            row.modes.uniqueness = Uniqueness::Unique;
            function.result = input.types.intern(row);
            forged.bodies[0].result = function.result;
            forged.types = definition_types(&input.types, &forged);
            let violations =
                hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![])
                    .violations;
            assert!(
                violations.iter().any(|v| matches!(
                    v.kind,
                    hirsel_validate::ViolationKind::TypeMismatch {
                        entity: "value modes"
                    }
                )),
                "{source}\n{violations:?}"
            );
            println!("EW43 V lost modes / alias rejected: {violations:?}");
        }
    });
}

#[test]
fn counted_mutable_handback_still_loses_uniqueness() {
    with_compiler_stack(|| {
        rejected(
            "fn advance(take xs: unique List[String], change: fn(mut String)->() uses {})->unique List[String] uses {} { let mut owned=take xs; change(mut owned[0]); take owned }",
            "MODE-2",
            "owned",
        )
    });
}

#[test]
fn retained_count_before_plain_mutable_handback_rejected() {
    with_compiler_stack(|| {
        rejected(
            "fn advance(take xs: unique List[u64], change: fn(mut u64)->() uses {})->unique List[u64] uses {} { let mut owned=take xs; let alias=copy owned; change(mut owned[0]); take owned }",
            "MODE-2",
            "owned",
        )
    });
}

// Same actual-worker tool sealing / returned-log transport as event-window-39.
// No toolchain copies: inventories retain hashes, sizes, modes and symlink trees.
fn tool_seal(directory: &std::path::Path, phase: &str) {
    let status = std::process::Command::new("python3")
        .arg(directory.join("seal.py"))
        .arg(phase)
        .arg(directory)
        .arg(std::env::current_exe().unwrap())
        .status()
        .unwrap();
    assert!(status.success(), "{phase} actual tool inventory");
}

#[test]
fn replay_mutable_handback_and_generic_cursor() {
    use hirsel_driver::native::*;
    with_compiler_stack(|| {
        let source = r#"
fn keep[T: plain](take xs: unique List[T])->unique List[T] uses {xs} {
  let mut out:List[T]=[]
  for item in take xs {list.push(mut out,item)}
  take out
}
fn advance(take xs:unique List[u64], change:fn(mut u64)->() uses {})->unique List[u64] uses {} {
  let mut owned=take xs
  change(mut owned[0])
  take owned
}
pub fn main()->Result[(),u64] uses {} {
  let xs:unique List[u64]=[1,2]
  let mut received=keep(advance(take xs,fn(mut x:u64) uses {} {x=41}))
  if received[0]!=41 || received[1]!=2 {return Error(1)}
  list.push(mut received,43)
  if received[2]!=43 {return Error(2)}
  Ok(())
}
"#;
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("main.hir"), source).unwrap();
        std::fs::write(
            directory.path().join("seal.py"),
            include_str!("../support/event_window_agreement_43/seal.py"),
        )
        .unwrap();
        tool_seal(directory.path(), "before");
        let mut engine = checked(source);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let matrix = [
            Configuration {
                options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
                engine: Engine::Native {
                    opt: OptLevel::None,
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
            Configuration {
                options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
                engine: Engine::Cranelift {
                    partition: Partition::Whole,
                    runtime: Runtime::Diagnostic,
                },
            },
        ];
        let toolchain = Toolchain::for_matrix(&directory.path().join("runtime"), &matrix).unwrap();
        let entry = entry_path(&default_context());
        let outcomes = runner_oracle_in(
            &validated,
            &entry,
            &matrix,
            &toolchain,
            directory.path(),
            &Fixture::default(),
            true,
        );
        assert_eq!(outcomes.len(), 2);
        for (configuration, outcome) in outcomes {
            println!("EW43 replay {configuration} {outcome:?}");
            assert!(matches!(outcome, Outcome::Agree), "{outcome:?}");
        }
        tool_seal(directory.path(), "after");
    });
}

#[test]
fn failed_callback_preserves_failure_and_cleanup_four_engines() {
    let directory = tempfile::tempdir().unwrap();
    let runner = history::Runner::new(directory.path()).unwrap();
    let report = runner.run(
        "failed-handback",
        r#"
fn advance(take xs: unique List[u64], change:fn(mut u64)->() uses {})->unique List[u64] uses {} {
  let mut owned=take xs
  change(mut owned[0])
  take owned
}
pub fn main()->Result[(),u64] uses {} {
  let xs:unique List[u64]=[1,2]
  let received=advance(take xs,fn(mut x:u64) uses {} {x=19; let zero=x-19; x=10/zero})
  Ok(())
}
"#,
    );
    assert_eq!(report.cells.len(), 4, "{report:?}");
    assert!(report.agrees(), "{report:?}");
    assert!(report.disagreements(&[], true).is_empty(), "{report:?}");
    for cell in &report.cells {
        println!("EW43 failure {} {:?}", cell.engine, cell.observation);
    }
}

#[test]
fn independent_plain_inert_bounds_matrix() {
    with_compiler_stack(|| {
        // The other two cells of the independent plain/inert bound matrix.
        let unbounded = "fn keep[T](take xs: List[T])->List[T] uses {} { let mut out:List[T]=[]; for item in take xs {list.push(mut out,take item)}; take out }";
        rejected(unbounded, "CAP-1", "take xs");
        runtime(
            "inert-moves-and-unbounded-borrows",
            r#"
fn keep[T: inert](take xs: List[T])->List[T] uses {} {
  let mut out:List[T]=[]
  for item in take xs {list.push(mut out,take item)}
  take out
}
fn count[T](xs:List[T])->u64 uses {} {
  let mut n:u64=0
  for item in xs {n=n+1}
  n
}
pub fn main()->Result[(),u64] uses {} {
  let empty:List[String]=[]
  let a=keep(take empty)
  let b=keep(["one"])
  let c=keep(["one","two"])
  if count(a)!=0 || count(b)!=1 || count(c)!=2 {return Error(1)}
  if b[0]!="one" || c[0]!="one" || c[1]!="two" {return Error(2)}
  Ok(())
}
"#,
        );
    });
}

#[test]
fn original_nearest_twins_recovered_explicit_packages() {
    with_compiler_stack(|| {
        // Preserve unique-empty-builder-29's exact five sources, rules and
        // spans. Their core-only packages need no implicit std dependency.
        for (index, (source, expected_rules, primary)) in [
            (include_str!("../../../../conformance/MODE/MODE-2-unique-empty-builder-29-aliased-source-bad.hir"), &["MODE-2"][..], "unique []"),
            (include_str!("../../../../conformance/MODE/MODE-2-unique-empty-builder-29-shared-source-bad.hir"), &["MODE-2"][..], "unique []"),
            (include_str!("../../../../conformance/MODE/MODE-2-unique-empty-builder-29-copied-item-bad.hir"), &["MODE-2"][..], "unique []"),
            (include_str!("../../../../conformance/MODE/MODE-2-unique-empty-builder-29-borrowed-source-bad.hir"), &["MODE-2", "MODE-1"][..], "unique []"),
            (include_str!("../../../../conformance/MODE/MODE-2-unique-empty-builder-29-nonfresh-initializer-bad.hir"), &["MODE-2"][..], "copy source"),
        ].into_iter().enumerate() {
            for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
                let directory = tempfile::tempdir().unwrap();
                std::fs::write(directory.path().join("main.hir"), source).unwrap();
                let manifest = directory.path().join("hirsel.json");
                std::fs::write(&manifest, r#"{"schema":1,"package":{"name":"standalone","version":"0"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
                let mut program = load_program(&manifest).unwrap();
                let output = program.check(policy).unwrap();
                assert_eq!(output.completion,CheckCompletion::Complete);
                let errors: Vec<_> = output.diagnostics.iter().filter(|d| d.severity == hirsel_diag::Severity::Error).collect();
                assert_eq!(errors.iter().map(|d| d.rule.as_str()).collect::<Vec<_>>(), expected_rules, "{source}\n{errors:?}");
                assert_eq!(output.sources.slice(errors[0].primary), Some(primary));
                let resolved = output.sources.resolve(errors[0].primary);
                let start = source.find(primary).unwrap() as u32;
                assert_eq!(resolved.start, start);
                assert_eq!(resolved.end, start + primary.len() as u32);
                println!("EW43 recovered nearest fixture={index} policy={policy:?} rules={expected_rules:?} primary={primary}");
            }
        }
    });
}
