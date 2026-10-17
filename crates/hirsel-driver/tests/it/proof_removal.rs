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
                if let Some(f) = inst.failure()
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
fn a_bounds_check_has_no_removal_form_and_stays() {
    with_compiler_stack(|| {
        let (reference, _) = ready(BUMP, None);
        let text = sidecar(BUMP, &[(BOUNDS, "proved")], |_| {});
        let (ready, report) = ready(BUMP, Some(&text));
        assert_eq!(checks(&ready), checks(&reference));
        assert_eq!(
            report[0],
            format!("proof {BOUNDS}: kept (no removal form for a MEM-14 failure-edge obligation)")
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
