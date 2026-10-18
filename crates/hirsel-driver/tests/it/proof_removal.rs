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
    })
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
        for name in ["fused", "producer$into"] {
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
/// execution cannot exercise. Run accepted arithmetic/projections/fusion on H
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
