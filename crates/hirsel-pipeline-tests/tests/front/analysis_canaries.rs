//! The out-of-process analysis canaries (`hirsel-analysis-canary`, PS-17 and
//! PS-18): each fixture is exported to a file, the analysis reads only that
//! file through the entry point its command runs and writes a findings
//! sidecar, and the sidecar must name exactly the expected sites. A change to
//! the seam that breaks an analysis outside the compiler fails here.

use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_proof_export::findings::Findings;

fn fixture(name: &str) -> &'static str {
    match name {
        "taint/leak.hir" => include_str!("fixtures/analysis/taint/leak.hir"),
        "taint/clean.hir" => include_str!("fixtures/analysis/taint/clean.hir"),
        "units/mismatch.hir" => include_str!("fixtures/analysis/units/mismatch.hir"),
        "units/same.hir" => include_str!("fixtures/analysis/units/same.hir"),
        _ => panic!("no fixture {name}"),
    }
}

/// The export of a fixture, as `hirsel export --tir` writes it.
fn export(name: &str) -> String {
    let source = fixture(name).to_string();
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add(name, source.clone()).unwrap();
        let result = check_progress(&mut engine, file, &default_context())
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(
            result.diagnostics.is_empty(),
            "{name}: {:?}",
            result.diagnostics
        );
        engine
            .export_tir(&entry_path(&default_context()))
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
    })
}

/// Run `analysis` over `name`'s export; each finding as `code line:column definition`.
fn findings(analysis: &str, name: &str) -> Vec<String> {
    let text = export(name);
    let dir = std::env::temp_dir().join(format!(
        "hirsel-canary-{analysis}-{}-{}",
        name.replace('/', "-"),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (input, output) = (dir.join("export.tir"), dir.join("findings.json"));
    std::fs::write(&input, &text).unwrap();
    let analyze = match analysis {
        "taint" => hirsel_analysis_canary::taint::analyze,
        _ => hirsel_analysis_canary::units::analyze,
    };
    hirsel_analysis_canary::run(
        analysis,
        analyze,
        input.to_str().unwrap(),
        output.to_str().unwrap(),
    )
    .unwrap();
    let sidecar = std::fs::read_to_string(&output).unwrap();
    let parsed = Findings::parse(&sidecar).unwrap_or_else(|e| panic!("{e}\n{sidecar}"));
    assert_eq!(parsed.to_json(), sidecar, "the sidecar is not canonical");
    let header = hirsel_proof_export::read_header(&text).unwrap();
    let imported = hirsel_proof_export::import(&text, &header.environment, &hirsel_types::TypeInterner::new(None)).unwrap();
    parsed.check(&imported).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    parsed
        .findings
        .iter()
        .map(|f| {
            format!(
                "{} {}:{} {}",
                f.code, f.location.line, f.location.column, f.definition
            )
        })
        .collect()
}

#[test]
fn a_secret_reaching_stdout_through_a_call_is_found() {
    assert_eq!(
        findings("taint", "taint/leak.hir"),
        ["TAINT-1 3:3 standalone@0/emit"]
    );
}

#[test]
fn a_secret_that_is_never_written_has_no_finding() {
    assert_eq!(findings("taint", "taint/clean.hir"), Vec::<String>::new());
}

#[test]
fn adding_metres_to_seconds_is_found() {
    assert_eq!(
        findings("units", "units/mismatch.hir"),
        ["UNIT-1 4:3 standalone@0/wrong"]
    );
}

#[test]
fn adding_metres_to_metres_has_no_finding() {
    assert_eq!(findings("units", "units/same.hir"), Vec::<String>::new());
}

/// Findings are keyed by the export they were computed from (PS-18).
#[test]
fn findings_for_another_export_are_stale() {
    let text = export("units/mismatch.hir");
    let other = export("units/same.hir");
    let header = hirsel_proof_export::read_header(&text).unwrap();
    let export = hirsel_proof_export::import(&text, &header.environment, &hirsel_types::TypeInterner::new(None)).unwrap();
    let other = hirsel_proof_export::import(&other, &header.environment, &hirsel_types::TypeInterner::new(None)).unwrap();
    let findings = Findings {
        export: other.digest(),
        analyzer: "units@0".into(),
        findings: vec![],
    };
    assert!(
        findings
            .check(&export)
            .unwrap_err()
            .starts_with("recorded for export")
    );
    assert!(findings.check(&other).is_ok());
}
