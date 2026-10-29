//! PS-12/PS-13 end to end: a sidecar's accepted evidence removes exactly its
//! checks from the native program, and every missing, stale, rejected or
//! unreplayable record keeps its check. `bump` has a bounds check (MEM-14),
//! an overflow check (SEM-1) and an `ensures` check (FACT-1).
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{
    Checker, Evidence, OptLevel, Options, Proofs, ReadyProgram, TargetSpec, build, emit_ir,
    parse_sidecar,
};
use hirsel_engine::*;
use hirsel_lir::Opcode;
use std::collections::BTreeMap;

const BUMP: &str = "fn bump(xs: List[u32], i: u64, y: u32) -> u32
  ensures result >= y
{
  xs[i] + y
}
pub fn main() -> Result[(), ()] {
  let n = bump([1u32, 2u32], 1u64, 3u32)
  Ok(())
}
";

const ENSURES: &str = "standalone@0/bump#FACT-1/ensures/0";
const OVERFLOW: &str = "standalone@0/bump#SEM-1/failure-edge/0";
const BOUNDS: &str = "standalone@0/bump#MEM-14/failure-edge/0";

fn hex(hash: &hirsel_syntax::ContentHash) -> String {
    let digits: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
    format!("#{digits}")
}

fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("bump.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine
}

/// A sidecar record per `(id, verdict)` for the export of `source`, from
/// the trusted checker `prover@1` within 1000 steps. `edit` adjusts each
/// record's JSON.
fn sidecar(source: &str, proved: &[(&str, &str)], edit: impl Fn(&mut serde_json::Value)) -> String {
    let mut engine = engine(source);
    let input = engine
        .proof_export_input(&entry_path(&default_context()))
        .unwrap();
    let export = hirsel_proof_export::build(&input);
    let revision = hex(&export.header.program_revision.expect("a launch entry"));
    let records: Vec<serde_json::Value> = proved
        .iter()
        .map(|(id, verdict)| {
            let obligation = export
                .definitions
                .iter()
                .flat_map(|d| &d.obligations)
                .find(|o| o.id.to_string() == *id)
                .unwrap_or_else(|| panic!("no obligation {id}"));
            let mut record = serde_json::json!({
                "revision": revision,
                "obligation": id,
                "checksum": hex(&obligation.checksum),
                "checker": "prover@1",
                "budget": {"unit": "steps", "count": 1000},
                "verdict": verdict,
            });
            edit(&mut record);
            record
        })
        .collect();
    serde_json::json!({"format": "hirsel.proof.sidecar.v1", "records": records}).to_string()
}

/// Build `source` for x86-64 with `sidecar`, trusting `prover@1`; the Ready
/// program and the proof report.
fn ready(source: &str, sidecar: Option<&str>) -> (ReadyProgram, Vec<String>) {
    let mut engine = engine(source);
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let proofs = sidecar.map(|text| {
        let evidence = Evidence {
            records: parse_sidecar(text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        Proofs::assess(
            &mut engine,
            &entry_path(&default_context()),
            &validated,
            &evidence,
        )
        .unwrap()
    });
    if let Some(proofs) = &proofs {
        options.discharge = proofs.discharge.clone();
    }
    let ready = build(&validated, &options).unwrap();
    let report = proofs.map_or_else(Vec::new, |p| p.report(&ready));
    (ready, report)
}

/// `bump`'s failure sites at Ready: (opcode, rule) to count.
fn checks(ready: &ReadyProgram) -> BTreeMap<(Opcode, String), usize> {
    let mut out = BTreeMap::new();
    for unit in ready.raw_units() {
        if unit.header.definition.0.name != "bump" {
            continue;
        }
        for block in &unit.blocks {
            for inst in block.instructions.get(&unit.instructions).unwrap() {
                if let hirsel_lir::ir::Inst::Address { address, .. } = inst {
                    let address = &unit.addresses[address.index()];
                    for (i, step) in address
                        .projection
                        .get(&unit.pools.projections)
                        .unwrap()
                        .iter()
                        .enumerate()
                    {
                        if address.unchecked.contains(&(i as u32)) {
                            continue;
                        }
                        let rule = match step {
                            hirsel_lir::ir::ProjectionStep::Index(_)
                            | hirsel_lir::ir::ProjectionStep::FlatAt { .. } => Some("MEM-14"),
                            hirsel_lir::ir::ProjectionStep::VariantField { .. } => Some("MEM-13"),
                            _ => None,
                        };
                        if let Some(rule) = rule {
                            *out.entry((Opcode::Address, rule.into())).or_default() += 1;
                        }
                    }
                } else if let Some(f) = inst.failure()
                    && let Some(rule) = &unit.tables.failures[f.index()].rule
                {
                    *out.entry((inst.opcode(), rule.clone())).or_default() += 1;
                }
            }
        }
    }
    out
}

fn without(
    mut checks: BTreeMap<(Opcode, String), usize>,
    opcode: Opcode,
    rule: &str,
) -> BTreeMap<(Opcode, String), usize> {
    let key = (opcode, rule.to_string());
    let n = checks[&key];
    assert!(n > 0);
    if n == 1 {
        checks.remove(&key);
    } else {
        checks.insert(key, n - 1);
    }
    checks
}

/// Unoptimized IR of `bump`'s definition.
fn bump_ir(ready: &ReadyProgram) -> String {
    let ir = emit_ir(ready, OptLevel::None).unwrap();
    let lines: Vec<&str> = ir
        .lines()
        .skip_while(|l| !(l.starts_with("define") && l.contains("4bumpI")))
        .collect();
    let end = lines.iter().position(|l| *l == "}").expect("bump's IR");
    lines[..=end].join("\n")
}

#[test]
fn every_check_stays_without_evidence() {
    with_compiler_stack(|| {
        let (ready, _) = ready(BUMP, None);
        let checks = checks(&ready);
        for (opcode, rule) in [
            (Opcode::Check, "FACT-1"),
            (Opcode::Binary, "SEM-1"),
            (Opcode::Address, "MEM-14"),
        ] {
            assert_eq!(checks.get(&(opcode, rule.into())), Some(&1), "{checks:?}");
        }
    })
}

#[test]
fn an_accepted_ensures_removes_exactly_its_check() {
    with_compiler_stack(|| {
        let (reference, _) = ready(BUMP, None);
        let text = sidecar(BUMP, &[(ENSURES, "proved")], |_| {});
        let (ready, report) = ready(BUMP, Some(&text));
        assert_eq!(
            checks(&ready),
            without(checks(&reference), Opcode::Check, "FACT-1")
        );
        assert_eq!(
            report,
            [
                format!("proof {ENSURES}: removed"),
                "proofs: 1 removed, 0 kept, 2 without evidence".into()
            ]
        );
    })
}

#[test]
fn an_accepted_overflow_check_imports_a_wrapping_add() {
    with_compiler_stack(|| {
        let (reference, _) = ready(BUMP, None);
        let text = sidecar(BUMP, &[(OVERFLOW, "proved")], |_| {});
        let (ready, _) = ready(BUMP, Some(&text));
        assert_eq!(
            checks(&ready),
            without(checks(&reference), Opcode::Binary, "SEM-1")
        );
        // Codegen control: the checked add's overflow intrinsic is gone.
        let (before, after) = (bump_ir(&reference), bump_ir(&ready));
        assert_eq!(before.matches("add.with.overflow").count(), 1, "{before}");
        assert_eq!(after.matches("add.with.overflow").count(), 0, "{after}");
    })
}

#[test]
fn an_accepted_bounds_check_removes_only_its_projection_guard() {
    with_compiler_stack(|| {
        let (reference, _) = ready(BUMP, None);
        let text = sidecar(BUMP, &[(BOUNDS, "proved")], |_| {});
        let (ready, report) = ready(BUMP, Some(&text));
        assert_eq!(
            checks(&ready),
            without(checks(&reference), Opcode::Address, "MEM-14")
        );
        assert_eq!(report[0], format!("proof {BOUNDS}: removed"));
        // The removed guard also leaves H's fault-injection domain.
        let mut engine = engine(BUMP);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let sites = hirsel_driver::native::Sites::new(validated.snapshot());
        let entry = entry_path(&default_context());
        let before_heap = hirsel_driver::native::run_heap(&reference, &entry, &sites).unwrap();
        let after_heap = hirsel_driver::native::run_heap(&ready, &entry, &sites).unwrap();
        assert_eq!(before_heap.observation, after_heap.observation);
        assert_eq!(before_heap.sites, after_heap.sites + 1);
        let before = bump_ir(&reference);
        let after = bump_ir(&ready);
        assert!(
            before.matches("icmp uge").count() > after.matches("icmp uge").count(),
            "{before}\n{after}"
        );
        // The second backend must consume V's exact projection evidence too.
        // Previously Cranelift emitted the guard even though Ready licensed
        // its removal, so the two Ready programs produced identical bounds IR.
        let bump_clif = |program: &ReadyProgram| {
            hirsel_driver::native::emit_clif(program)
                .unwrap()
                .split("; _H")
                .find(|function| {
                    function
                        .lines()
                        .next()
                        .is_some_and(|name| name.contains("4bumpI") && !name.starts_with("1T"))
                })
                .expect("bump's Cranelift IR")
                .to_owned()
        };
        let before = bump_clif(&reference);
        let after = bump_clif(&ready);
        assert!(
            before.matches("icmp uge").count() > after.matches("icmp uge").count(),
            "{before}\n{after}"
        );
    })
}

#[test]
fn immutable_callable_table_keeps_its_length_across_erased_calls() {
    with_compiler_stack(|| {
        let source = r#"
type Stage { call: fn(u64, u64) -> u64 uses {}, environment: u64 }
fn add(value: u64, environment: u64) -> u64 { value + environment }
fn multiply(value: u64, environment: u64) -> u64 { value * environment }
fn workload(n: u64) -> u64 {
  let stages: List[Stage] = [Stage(call: add, environment: 3), Stage(call: multiply, environment: 2)]
  let mut total: u64 = 0
  for i in 0u64..n {
    let value = stages[0].call(i, stages[0].environment)
    if value % 2u64 == 0u64 { total += stages[1].call(value, stages[1].environment) }
  }
  total
}
pub fn main() -> Result[(), ()] { let n = workload(10); Ok(()) }
"#;
        let workload_ir = |program: &ReadyProgram| {
            emit_ir(program, OptLevel::Speed)
                .unwrap()
                .split("\ndefine ")
                .find(|function| {
                    function
                        .lines()
                        .next()
                        .is_some_and(|line| line.contains("108workloadI"))
                })
                .expect("workload's LLVM IR")
                .split("\n}")
                .next()
                .unwrap()
                .to_owned()
        };
        let (mut program, _) = ready(source, None);
        let immutable = workload_ir(&program);
        assert!(
            !immutable.contains("@hirsel.fail.MEM-14.code"),
            "{immutable}"
        );
        // A C callback program disables the local-root memory assumption.
        program.callbacks.insert(entry_path(&default_context()));
        let callback = workload_ir(&program);
        assert!(callback.contains("@hirsel.fail.MEM-14.code"), "{callback}");
        // The same table handed to a mutable callee must retain its check.
        let changed = source
            .replace("let stages:", "let mut stages:")
            .replace("  let mut total:", "  clear(mut stages)\n  let mut total:")
            .replace(
                "fn workload",
                "fn clear(mut stages: List[Stage]) -> () { stages = [] }\nfn workload",
            );
        let (program, _) = ready(&changed, None);
        let mutable = workload_ir(&program);
        assert!(mutable.contains("@hirsel.fail.MEM-14.code"), "{mutable}");
        // A local closure captures the root; even borrowed captures conservatively
        // disable companion state because closure environments can escape analysis.
        let captured = source.replace(
            "  let mut total:",
            "  let count = fn() -> u64 { list.len(stages) }\n  let count_result = count()\n  let mut total:",
        );
        let (program, _) = ready(&captured, None);
        let capture = workload_ir(&program);
        assert!(capture.contains("@hirsel.fail.MEM-14.code"), "{capture}");
    });
}

#[test]
fn a_stale_sidecar_keeps_every_check() {
    with_compiler_stack(|| {
        let all = [(ENSURES, "proved"), (OVERFLOW, "proved")];
        let text = sidecar(BUMP, &all, |_| {});
        // The edited program has another revision and another TIR digest.
        let edited = BUMP.replace("xs[i] + y", "let k = i\n  xs[k] + y");
        let (reference, _) = ready(&edited, None);
        let (ready, report) = ready(&edited, Some(&text));
        assert_eq!(checks(&ready), checks(&reference));
        assert_eq!(
            report[..2],
            // Export order: sites, then `ensures` (PS-3).
            [
                format!("proof {OVERFLOW}: kept (stale: recorded for another program revision)"),
                format!("proof {ENSURES}: kept (stale: recorded for another program revision)"),
            ]
        );
    })
}

#[test]
fn unaccepted_records_keep_their_checks() {
    with_compiler_stack(|| {
        let (reference, _) = ready(BUMP, None);
        let cases: [(&str, fn(&mut serde_json::Value), &str); 4] = [
            (
                "proved",
                |r| r["checksum"] = format!("#{}", "0".repeat(64)).into(),
                "stale: the checksum differs from this export's",
            ),
            (
                "proved",
                |r| r["checker"] = "other@1".into(),
                "unreplayable: this build trusts no checker other@1",
            ),
            (
                "proved",
                |r| r["budget"]["unit"] = "seconds".into(),
                "unreplayable: budget 1000 seconds is not a positive deterministic count",
            ),
            ("refuted", |_| {}, "rejected: the verdict is refuted"),
        ];
        for (verdict, edit, why) in cases {
            let text = sidecar(BUMP, &[(ENSURES, verdict)], edit);
            let (ready, report) = ready(BUMP, Some(&text));
            assert_eq!(checks(&ready), checks(&reference), "{why}");
            assert_eq!(report[0], format!("proof {ENSURES}: kept ({why})"));
        }
    })
}

#[test]
fn malformed_sidecars_are_refused() {
    for text in [
        r#"{"format": "hirsel.proof.sidecar.v0", "records": []}"#,
        r#"{"format": "hirsel.proof.sidecar.v1"}"#,
        r#"{"format": "hirsel.proof.sidecar.v1", "records": [{"obligation": "x"}]}"#,
        r#"{"format": "hirsel.proof.sidecar.v1", "records": [], "trust": []}"#,
    ] {
        assert!(parse_sidecar(text).is_err(), "{text}");
    }
    assert_eq!(
        parse_sidecar(r#"{"format": "hirsel.proof.sidecar.v1", "records": []}"#),
        Ok(vec![])
    );
}

/// The PS-13 validator's control: hand-built discharged units that removed
/// too little, too much or added a check are refused.
#[test]
fn the_discharge_validator_refuses_any_difference_from_the_plan() {
    use hirsel_lir::validate::validate_discharge;
    with_compiler_stack(|| {
        let mut engine = engine(BUMP);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let text = sidecar(BUMP, &[(ENSURES, "proved")], |_| {});
        let evidence = Evidence {
            records: parse_sidecar(&text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        let proofs = Proofs::assess(
            &mut engine,
            &entry_path(&default_context()),
            &validated,
            &evidence,
        )
        .unwrap();
        let program = hirsel_driver::native::program(&validated);
        let lowered = hirsel_rc::import::lower_discharging(
            &program,
            &TargetSpec::x86_64_unknown_linux_gnu(),
            &Default::default(),
            &proofs.discharge,
        )
        .unwrap();
        let unit = lowered
            .discharged
            .iter()
            .find(|u| u.discharged != u.reference)
            .expect("a discharged unit");
        let plan = proofs.discharge.sites(&unit.definition).unwrap();
        assert_eq!(
            validate_discharge(&unit.reference, &unit.discharged, plan).as_ref(),
            Ok(plan)
        );
        let none = Default::default();
        let refused = |reference, discharged, plan| {
            validate_discharge(reference, discharged, plan).unwrap_err()
        };
        // Kept a licensed check.
        let errors = refused(&unit.reference, &unit.reference, plan);
        assert!(errors[0].contains("was not removed"), "{errors:?}");
        // Removed a check nothing licensed.
        let errors = refused(&unit.reference, &unit.discharged, &none);
        assert!(
            errors[0].contains("without accepted evidence"),
            "{errors:?}"
        );
        // Gained a check the reference lacks.
        let errors = refused(&unit.discharged, &unit.reference, &none);
        assert!(
            errors[0].contains("not in the reference import"),
            "{errors:?}"
        );
    })
}

/// Corpus execution cannot observe check removal. Inspect every non-Add
/// arithmetic form and its independent, unproved neighbour in LIR.
#[test]
fn accepted_integer_operators_remove_only_their_own_failure_edges() {
    with_compiler_stack(|| {
        for (ty, expression) in [
            ("u32", "x - y"),
            ("i32", "x - y"),
            ("u32", "x * y"),
            ("i32", "x * y"),
            ("u32", "x / y"),
            ("i32", "x / y"),
            ("u32", "x % y"),
            ("i32", "x % y"),
            ("u32", "x << y"),
            ("i32", "x << y"),
            ("u32", "x >> y"),
            ("i32", "x >> y"),
            ("u32", "-x"),
            ("i32", "-x"),
        ] {
            let source = format!(
                "fn bump(x: {ty}, y: {ty}) -> {ty} {{ let n = {expression}; n + y }}\npub fn main() -> Result[(), ()] {{ let n = bump(6{ty}, 2{ty}); Ok(()) }}\n"
            );
            let (reference, _) = ready(&source, None);
            let record = sidecar(&source, &[(OVERFLOW, "proved")], |_| {});
            let (accepted, report) = ready(&source, Some(&record));
            let opcode = if expression == "-x" && ty == "i32" {
                Opcode::Unary
            } else {
                Opcode::Binary
            };
            assert_eq!(
                checks(&accepted),
                without(checks(&reference), opcode, "SEM-1"),
                "{ty} {expression}"
            );
            assert!(report[0].ends_with(": removed"), "{report:?}");
            emit_ir(&accepted, OptLevel::None).unwrap();
            let record = sidecar(&source, &[(OVERFLOW, "proved")], |r| {
                r["checksum"] = format!("#{}", "0".repeat(64)).into()
            });
            let (forged, report) = ready(&source, Some(&record));
            assert_eq!(checks(&forged), checks(&reference));
            assert!(report[0].contains("kept (stale:"), "{report:?}");
        }
    });
}

const FORMS: &str = include_str!("../../../../conformance/FACT/FACT-3-proof-removal-forms-run.hir");

fn all_records(source: &str) -> String {
    let mut engine = engine(source);
    let input = engine
        .proof_export_input(&entry_path(&default_context()))
        .unwrap();
    let export = hirsel_proof_export::build(&input);
    let ids: Vec<_> = export
        .definitions
        .iter()
        .flat_map(|d| &d.obligations)
        .filter(|o| {
            o.id.kind == hirsel_proof_export::ObligationKind::FailureEdge
                && matches!(o.id.rule.as_str(), "SEM-1" | "MEM-13" | "MEM-14")
        })
        .map(|o| o.id.to_string())
        .collect();
    assert!(!ids.is_empty());
    sidecar(
        source,
        &ids.iter()
            .map(|id| (id.as_str(), "proved"))
            .collect::<Vec<_>>(),
        |_| {},
    )
}

#[test]
fn tag_and_bounds_evidence_is_exact_to_the_projected_place() {
    with_compiler_stack(|| {
        let source = "type Choice = Small(value: u64) | Large(value: u64, extra: u64)\nfn bump(xs: List[u64], i: u64, j: u64, x: Packed[Choice]) -> u64 { xs[i] + xs[j] + x.Small.value }\npub fn main() -> Result[(), ()] { let x: Packed[Choice] = Small(value: 3u64); let n = bump([5u64], 0u64, 0u64, x); Ok(()) }\n";
        let (reference, _) = ready(source, None);
        for (id, rule) in [
            (BOUNDS, "MEM-14"),
            ("standalone@0/bump#MEM-13/failure-edge/0", "MEM-13"),
        ] {
            let text = sidecar(source, &[(id, "proved")], |_| {});
            let (accepted, report) = ready(source, Some(&text));
            assert_eq!(
                checks(&accepted),
                without(checks(&reference), Opcode::Address, rule)
            );
            assert_eq!(report[0], format!("proof {id}: removed"));
            emit_ir(&accepted, OptLevel::None).unwrap();
            let text = sidecar(source, &[(id, "proved")], |r| {
                r["checksum"] = format!("#{}", "0".repeat(64)).into()
            });
            let (forged, _) = ready(source, Some(&text));
            assert_eq!(checks(&forged), checks(&reference));
        }
    });
}

#[test]
fn append_fusion_keeps_original_sites_in_callers_and_derived_builders() {
    with_compiler_stack(|| {
        let ids = [
            "standalone@0/fused#SEM-1/failure-edge/0",
            "standalone@0/producer#SEM-1/failure-edge/0",
        ];
        let (reference, _) = ready(FORMS, None);
        let text = sidecar(FORMS, &ids.map(|id| (id, "proved")), |_| {});
        let (accepted, report) = ready(FORMS, Some(&text));
        assert!(
            report[..2].iter().all(|l| l.ends_with(": removed")),
            "{report:?}"
        );
        // Append retirement keeps the proof on each original definition.
        assert!(
            reference.raw_units().iter().all(|unit| !unit
                .header
                .definition
                .0
                .name
                .ends_with("$into"))
        );
        for name in ["fused", "producer"] {
            let count = |p: &ReadyProgram| {
                p.raw_units()
                    .iter()
                    .filter(|u| u.header.definition.0.name == name)
                    .flat_map(|u| {
                        u.instructions
                            .iter()
                            .filter(|i| {
                                matches!(
                                    i,
                                    hirsel_lir::ir::Inst::Binary {
                                        op: hirsel_lir::ir::BinaryOp::Sub,
                                        ..
                                    }
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .count()
            };
            assert_eq!(count(&reference), 1, "{name} must have native code");
            assert_eq!(count(&accepted), 0, "{name}");
        }
        let text = sidecar(FORMS, &ids.map(|id| (id, "proved")), |r| {
            r["checksum"] = format!("#{}", "0".repeat(64)).into()
        });
        let (forged, report) = ready(FORMS, Some(&text));
        assert!(
            report[..2].iter().all(|l| l.contains("kept (stale:")),
            "{report:?}"
        );
        assert_eq!(forged.discharged, Default::default());
    });
}

/// The sidecar changes LIR and code generation, which ordinary corpus
/// execution cannot exercise. Run accepted arithmetic/projections/append on H
/// and on both native targets at O0 and O3, retaining main's result checks.
#[test]
fn accepted_removal_forms_run_on_heap_and_both_native_targets() {
    use hirsel_driver::native::{
        Observation, Partition, Sites, Toolchain, launch, link, observe, run_heap,
    };
    with_compiler_stack(|| {
        let mut engine = engine(FORMS);
        let entry = entry_path(&default_context());
        let execution = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let expected = Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(expected.exit_code, Some(0), "{expected:?}");
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let text = all_records(FORMS);
        let evidence = Evidence {
            records: parse_sidecar(&text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        let proofs = Proofs::assess(&mut engine, &entry, &validated, &evidence).unwrap();
        let targets = [
            TargetSpec::x86_64_unknown_linux_gnu(),
            TargetSpec::aarch64_unknown_linux_gnu(),
        ];
        let root =
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("proof-removal-forms");
        std::fs::create_dir_all(&root).unwrap();
        let tools =
            Toolchain::in_tree_diagnostic(&root, &targets.iter().collect::<Vec<_>>()).unwrap();
        for target in targets {
            let mut options = Options::new(target.clone());
            options.discharge = proofs.discharge.clone();
            let program = build(&validated, &options).unwrap();
            let sites = Sites::new(validated.snapshot());
            assert_eq!(
                run_heap(&program, &entry, &sites).unwrap().observation,
                expected
            );
            for opt in [OptLevel::None, OptLevel::Speed] {
                let dir = root.join(format!("{}-{opt:?}", target.triple));
                std::fs::create_dir_all(&dir).unwrap();
                let exe = link(&program, &entry, opt, Partition::Whole, &tools, &dir).unwrap();
                let launched = launch(&exe, &target, &tools, &dir, false).unwrap();
                assert_eq!(
                    observe(&launched, &program, &sites).unwrap(),
                    expected,
                    "{} {opt:?}",
                    target.triple
                );
            }
        }
    });
}

#[test]
fn nested_projection_evidence_removes_only_the_licensed_guard_kind() {
    with_compiler_stack(|| {
        let source = "type Choice = Small(values: Inline[u64, 1]) | Large(value: u64, extra: u64)\nfn bump(x: Packed[Choice], i: u64) -> u64 { x.Small.values[i] }\npub fn main() -> Result[(), ()] { let x: Packed[Choice] = Small(values: [3u64]); let n = bump(x, 0u64); Ok(()) }\n";
        let (reference, _) = ready(source, None);
        for (id, rule) in [
            (BOUNDS, "MEM-14"),
            ("standalone@0/bump#MEM-13/failure-edge/0", "MEM-13"),
        ] {
            let text = sidecar(source, &[(id, "proved")], |_| {});
            let (accepted, _) = ready(source, Some(&text));
            let before = checks(&reference);
            let after = checks(&accepted);
            let other = if rule == "MEM-13" { "MEM-14" } else { "MEM-13" };
            assert!(
                before[&(Opcode::Address, rule.into())]
                    > after
                        .get(&(Opcode::Address, rule.into()))
                        .copied()
                        .unwrap_or(0)
            );
            assert_eq!(
                before.get(&(Opcode::Address, other.into())),
                after.get(&(Opcode::Address, other.into()))
            );
            emit_ir(&accepted, OptLevel::None).unwrap();
        }
    });
}

#[test]
fn flat_walk_keeps_the_proved_bounds_guard_unchecked() {
    with_compiler_stack(|| {
        let source = "flat type Item { text: String, value: u64 }\nflat type Items { items: List[Item] }\nfn bump(xs: Items) -> u64 { let mut n = 0u64; for x in xs.items { n += x.value }; n }\npub fn main() -> Result[(), ()] { let n = bump(Items(items: [Item(text: \"x\", value: 7u64)])); Ok(()) }\n";
        let (reference, _) = ready(source, None);
        let mut text: serde_json::Value = serde_json::from_str(&all_records(source)).unwrap();
        text["records"]
            .as_array_mut()
            .unwrap()
            .retain(|r| r["obligation"].as_str().unwrap().contains("#MEM-14/"));
        let (accepted, _) = ready(source, Some(&text.to_string()));
        assert!(
            accepted
                .raw_units()
                .iter()
                .any(|u| u.header.definition.0.name == "bump"
                    && u.instructions
                        .iter()
                        .any(|i| matches!(i, hirsel_lir::ir::Inst::FlatNext { .. }))),
            "the accepted unit must actually use a flat walk"
        );
        assert!(
            checks(&reference)
                .get(&(Opcode::Address, "MEM-14".into()))
                .copied()
                .unwrap_or(0)
                > 0
        );
        assert_eq!(
            checks(&accepted).get(&(Opcode::Address, "MEM-14".into())),
            None
        );
        emit_ir(&accepted, OptLevel::None).unwrap();
    });
}

#[test]
fn borrowed_variant_views_keep_their_parent_and_child_obligations_separate() {
    with_compiler_stack(|| {
        let source = "type Inner = A(value: u64) | B(value: u64, other: u64)\ntype Outer = Node(inner: Packed[Inner]) | Leaf()\nfn bump(x: Packed[Outer]) -> u64 { let inner = x.Node.inner; inner.A.value }\npub fn main() -> Result[(), ()] { let x: Packed[Outer] = Node(inner: A(value: 3u64)); let n = bump(x); Ok(()) }\n";
        let (reference, _) = ready(source, None);
        for id in [
            "standalone@0/bump#MEM-13/failure-edge/0",
            "standalone@0/bump#MEM-13/failure-edge/1",
        ] {
            let text = sidecar(source, &[(id, "proved")], |_| {});
            let (accepted, _) = ready(source, Some(&text));
            assert_eq!(
                checks(&accepted),
                without(checks(&reference), Opcode::Address, "MEM-13")
            );
            emit_ir(&accepted, OptLevel::None).unwrap();
        }
    });
}

/// Assessment and reporting retain no query controller, Ready program or
/// privilege to substitute an export with a different source body.
#[test]
fn immutable_export_assessment_preserves_sidecar_decisions_after_controller_drop() {
    with_compiler_stack(|| {
        let entry = entry_path(&default_context());
        let text = sidecar(BUMP, &[(ENSURES, "proved")], |_| {});
        let evidence = Evidence {
            records: parse_sidecar(&text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        let mut engine = engine(BUMP);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let expected = Proofs::assess(&mut engine, &entry, &validated, &evidence).unwrap();
        let export = hirsel_proof_export::build(&engine.proof_export_input(&entry).unwrap());
        drop(engine);
        let actual = Proofs::assess_export(&export, &validated, &evidence).unwrap();
        assert_eq!(actual.discharge, expected.discharge);
        assert!(!actual.discharge.is_empty());
        let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        options.discharge = actual.discharge.clone();
        let ready = build(&validated, &options).unwrap();
        assert_eq!(
            actual.report_discharged(&ready.discharged),
            expected.report(&ready)
        );
        let mut untrusted = evidence.clone();
        untrusted.trusted.clear();
        let kept = Proofs::assess_export(&export, &validated, &untrusted).unwrap();
        assert!(kept.discharge.is_empty());
        assert!(
            kept.report_discharged(&Default::default())
                .iter()
                .any(|line| line.contains("kept"))
        );
        let mut duplicate = evidence.clone();
        duplicate.records.push(duplicate.records[0].clone());
        assert!(Proofs::assess_export(&export, &validated, &duplicate).is_err());
    });
}

#[test]
fn settled_original_assessment_binds_exported_bodies_without_a_native_snapshot() {
    use std::sync::Arc;
    with_compiler_stack(|| {
        let entry = entry_path(&default_context());
        let text = sidecar(BUMP, &[(ENSURES, "proved"), (OVERFLOW, "proved")], |_| {});
        let evidence = Evidence {
            records: parse_sidecar(&text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        let mut producer = engine(BUMP);
        let export = hirsel_proof_export::build(&producer.proof_export_input(&entry).unwrap());
        let paths = export
            .definitions
            .iter()
            .map(|section| section.path.clone())
            .collect::<Vec<_>>();
        let store: Arc<dyn store::ResultStore> = Arc::new(store::MemStore::default());
        let original = producer
            .native_original_reader(&paths, store.clone())
            .unwrap();
        let missing = producer
            .native_original_context(&paths)
            .unwrap()
            .reader(Vec::new(), store)
            .unwrap();
        // The legacy snapshot is a test oracle only. The new assessment below
        // receives solely the original Lower reader and independently selected export.
        let input = producer.export_native_input().unwrap();
        let validated = producer.import_native_input(input).unwrap();
        let expected = Proofs::assess_export(&export, &validated, &evidence).unwrap();
        drop(validated);
        drop(producer);

        let trace = hirsel_lir::construction::ValueTrace::default();
        let _observe = trace.enter();
        let actual = Proofs::assess_original(&export, &original, &evidence).unwrap();
        assert_eq!(actual.discharge, expected.discharge);
        assert!(!actual.discharge.is_empty());
        assert_eq!(
            actual.report_discharged(&actual.discharge),
            expected.report_discharged(&expected.discharge)
        );
        assert_eq!(trace.totals(), Default::default());
        assert!(Proofs::assess_original(&export, &missing, &evidence).is_err());

        let mut modified = export.clone();
        modified
            .definitions
            .iter_mut()
            .find(|d| d.path.0.name == "bump")
            .unwrap()
            .tir
            .signature_hash = hirsel_syntax::ContentHash::default();
        assert!(Proofs::assess_original(&modified, &original, &evidence).is_err());
        let mut other = engine(BUMP);
        let foreign = other
            .native_original_reader(&paths, Arc::new(store::MemStore::default()))
            .unwrap();
        assert!(Proofs::assess_original(&export, &foreign, &evidence).is_err());

        let mut duplicate = evidence.clone();
        duplicate.records.push(duplicate.records[0].clone());
        assert!(Proofs::assess_original(&export, &original, &duplicate).is_err());
        let mut untrusted = evidence.clone();
        untrusted.trusted.clear();
        let kept = Proofs::assess_original(&export, &missing, &untrusted).unwrap();
        assert!(kept.discharge.is_empty());
        assert!(
            kept.report_discharged(&Default::default())
                .iter()
                .any(|line| line.contains("kept"))
        );
        let mut stale = evidence.clone();
        stale
            .records
            .iter_mut()
            .for_each(|r| r.revision = hirsel_syntax::ContentHash::default());
        assert!(
            Proofs::assess_original(&export, &missing, &stale)
                .unwrap()
                .discharge
                .is_empty()
        );
    });
}

#[test]
fn native_discovery_reports_actual_removals_on_cold_and_body_free_fresh_hits() {
    use hirsel_driver::native::queries::{
        INSTANCE_KIND,
        declarations::NativeDeclarations,
        instance_tasks::{DISCOVERY_KIND, INSTANCE_INPUT_KIND, InstanceTaskFactory, InstanceTasks},
        source_inputs::NativeInstanceInputs,
    };
    use hirsel_engine::runtime::*;
    use std::sync::{Arc, atomic::Ordering};
    with_compiler_stack(|| {
        let text = sidecar(BUMP, &[(ENSURES, "proved"), (OVERFLOW, "proved")], |_| {});
        let evidence = Evidence {
            records: parse_sidecar(&text).unwrap(),
            trusted: vec![Checker::parse("prover@1").unwrap()],
        };
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn store::ResultStore> =
            Arc::new(store::DirStore::new(dir.path().to_path_buf()));
        let mut expected = None;
        for mode in 0..3 {
            let mut producer = QueryEngine::new(default_environment(), compiler_adapters())
                .with_result_store(store.clone());
            let file = producer.sources.add("bump.hir", BUMP.into()).unwrap();
            let checked = check_progress(&mut producer, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = entry_path(&default_context());
            let export = hirsel_proof_export::build(&producer.proof_export_input(&entry).unwrap());
            let mut cache = NativeDiscoveryCache::new(producer.types().clone(), store.clone());
            let declarations = NativeDeclarations::collect(&mut producer, &mut cache, &[]).unwrap();
            let paths = declarations
                .context
                .sources
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let original = producer
                .native_original_reader(&paths, store.clone())
                .unwrap();
            let proofs = Proofs::assess_original(&export, &original, &evidence).unwrap();
            assert!(!proofs.discharge.is_empty());
            let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
            options.discharge = proofs.discharge.clone();
            options.validation = hirsel_driver::native::Validation::Ready;
            if mode == 2 {
                // This licensed site never becomes a native check. Discovery
                // must refuse before exposing any completed report product.
                options.discharge.0.values_mut().next().unwrap().insert(
                    hirsel_lir::discharge::Discharged {
                        body: u32::MAX,
                        block: 0,
                        statement: 0,
                        form: hirsel_lir::discharge::Form::Integer,
                    },
                );
            }
            if mode == 0 {
                // Compare against the established batch validator/report only
                // as a test oracle. It is dropped before new native demand.
                let input = producer.export_native_input().unwrap();
                let validated = producer.import_native_input(input).unwrap();
                let ready = build(&validated, &options).unwrap();
                expected = Some((ready.discharged.clone(), proofs.report(&ready)));
            }
            let inputs = NativeInstanceInputs::from_products(declarations, original).unwrap();
            drop(cache);
            drop(producer);
            let runtime_bytes = hirsel_intern::jobs::COMPILER_STACK_BYTES as u64 + (64 << 20);
            let budget = ProcessBudget::new(runtime_bytes + (32 << 20));
            let tasks = InstanceTasks::new(
                inputs,
                store.clone(),
                options,
                hirsel_syntax::ContentHash::of_bytes(b"actual native proof removals"),
                MemoryWeight {
                    scratch: 16 << 20,
                    retained: 1 << 20,
                    stack: 0,
                },
                budget.reserve(32 << 20).unwrap(),
            )
            .unwrap();
            let denied = tasks.failure("proof discovery runtime refusal");
            let errors = RuntimeErrors {
                resource: denied,
                cycle: denied,
                cancelled: denied,
                protocol: denied,
                panicked: denied,
            };
            let mut runtime = AdmittedRuntime::new(
                hirsel_intern::jobs::Jobs::new(1),
                budget.reserve(runtime_bytes).unwrap(),
                errors,
            )
            .unwrap();
            let trace = hirsel_lir::construction::ValueTrace::default();
            struct Traced(
                Arc<InstanceTaskFactory>,
                hirsel_lir::construction::ValueTrace,
            );
            struct Task(Box<dyn QueryTask>, hirsel_lir::construction::ValueTrace);
            impl QueryFactory for Traced {
                fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, store::Reference> {
                    Ok(Box::new(Task(self.0.create(id)?, self.1.clone())))
                }
            }
            impl QueryTask for Task {
                fn id(&self) -> QueryId {
                    self.0.id()
                }
                fn weight(&self) -> MemoryWeight {
                    self.0.weight()
                }
                fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
                    let _observe = self.1.enter();
                    self.0.poll(cx)
                }
            }
            let factory = Arc::new(Traced(
                Arc::new(InstanceTaskFactory::new(tasks.clone())),
                trace.clone(),
            ));
            for kind in [
                INSTANCE_KIND,
                INSTANCE_INPUT_KIND,
                DISCOVERY_KIND,
                hirsel_driver::native::queries::instance_tasks::CALLABLE_GRAPH_KIND,
                hirsel_driver::native::queries::instance::CALLABLE_VALUE_KIND,
            ] {
                runtime.register(kind, factory.clone()).unwrap();
            }
            let request = runtime.request(&[tasks.discovery_id()]);
            let result = runtime.wait(request);
            if mode == 2 {
                let error = result.unwrap_err();
                let message = store.get_content(error).unwrap();
                assert!(
                    String::from_utf8_lossy(&message)
                        .contains("accepted discharge has no native check"),
                    "{}",
                    String::from_utf8_lossy(&message)
                );
                continue;
            }
            let settled = result.unwrap_or_else(|error| {
                panic!(
                    "{}",
                    String::from_utf8_lossy(&store.get_content(error).unwrap())
                )
            })[0];
            let actual = tasks.discharged(settled).unwrap();
            assert_eq!(&actual, &expected.as_ref().unwrap().0);
            assert_eq!(
                proofs.report_discharged(&actual),
                expected.as_ref().unwrap().1
            );
            assert!(
                tasks
                    .discharged(ProductRef {
                        action: store::Action(Default::default()),
                        ..settled
                    })
                    .is_err()
            );
            if mode == 1 {
                assert_eq!(tasks.imports.load(Ordering::Relaxed), 0);
                assert_eq!(tasks.source_body_demands.load(Ordering::Relaxed), 0);
                assert_eq!(trace.totals(), Default::default());
            } else {
                assert!(tasks.imports.load(Ordering::Relaxed) > 0);
            }
        }
    });
}
