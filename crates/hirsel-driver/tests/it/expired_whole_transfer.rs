//! Whole transfers retain expiry; independent validation judges every later use.
use hirsel_driver::{compiler_adapters, native};
use hirsel_engine::*;
use hirsel_types::{Signature, SignatureProvider, TypeInterner, TypeMemo};
use std::sync::{Arc, Mutex};

const POSITIVES: &[(&str, &str)] = &[
    (
        "take",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-take-ok.hir"),
    ),
    (
        "copy",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-copy-ok.hir"),
    ),
    (
        "call",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-call-ok.hir"),
    ),
    (
        "return",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-return-ok.hir"),
    ),
];
const NEGATIVES: &[(&str, &str)] = &[
    (
        "taken-brand-call",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-taken-brand-call-bad.hir"),
    ),
    (
        "generic-call",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-generic-call-bad.hir"),
    ),
    (
        "return",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-return-bad.hir"),
    ),
    (
        "match",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-match-bad.hir"),
    ),
    (
        "copy-match",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-copy-match-bad.hir"),
    ),
    (
        "brand-call",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-brand-call-bad.hir"),
    ),
    (
        "read-call",
        include_str!("../../../../conformance/PROV/PROV-1-expired-whole-read-call-bad.hir"),
    ),
    (
        "field-read",
        include_str!("../../../../conformance/PROV/PROV-1-expired-field-read-bad.hir"),
    ),
    (
        "field-take",
        include_str!("../../../../conformance/PROV/PROV-1-expired-field-take-bad.hir"),
    ),
    (
        "field-copy",
        include_str!("../../../../conformance/PROV/PROV-1-expired-field-copy-bad.hir"),
    ),
    (
        "ended-owner",
        include_str!("../../../../conformance/PROV/PROV-1-ended-owner-use-bad.hir"),
    ),
    (
        "stale-brand",
        include_str!("../../../../conformance/PROV/PROV-1-index-container-bad.hir"),
    ),
];

// Retain the pre-publication body even when J rejects it. No diagnostics are
// removed: V receives the actual rejected program, not an accepted substitute.
struct CaptureModes {
    inner: Arc<dyn ModesPhase>,
    body: Mutex<Option<AnalyzedBody>>,
}
impl ModesPhase for CaptureModes {
    fn check_modes(
        &self,
        t: &TypeInterner,
        m: &TypeMemo,
        body: Arc<TypedBody>,
        signature: &Signature,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<hirsel_types::QueryOutput<AnalyzedBody>, QueryError> {
        let output = self
            .inner
            .check_modes(t, m, body, signature, products, provider)?;
        if signature.definition.path().0.name == "probe" {
            *self.body.lock().unwrap() = Some(output.value.clone());
        }
        Ok(output)
    }
}

#[test]
fn whole_transfers_and_expired_reads_agree_with_independent_validation() {
    with_compiler_stack(|| {
        let mut failures = vec![];
        let mut cases = vec![];
        for &(name, source) in POSITIVES {
            cases.push((true, name.to_owned(), source.to_owned()));
        }
        for &(name, source) in NEGATIVES {
            cases.push((false, name.to_owned(), source.to_owned()));
            // Prove that rejection depends on expiry, not a malformed graph
            // or a pre-existing mismatch in the receiving contract.
            let live = if name == "ended-owner" {
                source.replace(
                    "  let value = mk(fresh())",
                    "  let owner = fresh()\n  let value = mk(owner)",
                )
            } else {
                assert!(source.contains("  r[0] = 3u64\n"), "{name}");
                source.replace("  r[0] = 3u64\n", "")
            };
            cases.push((true, format!("live-{name}"), live));
        }
        for (accepted, name, source) in cases {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let source = format!("{source}\nfn main() {{}}\n");
                let mut adapters = compiler_adapters();
                let capture = Arc::new(CaptureModes {
                    inner: adapters.modes.take().unwrap(),
                    body: Mutex::new(None),
                });
                adapters.modes = Some(capture.clone());
                let lower = adapters.lowering.clone().unwrap();
                let t = hirsel_engine::registry::session_types();
                let mut engine =
                    QueryEngine::new(default_environment(), adapters).with_types(t.clone());
                let file = engine.sources.add("expired.hir", source.clone()).unwrap();
                let parsed = engine.parse_file(file, &default_context()).unwrap();
                let mut signatures = hirsel_validate::SignatureMap::new(t.clone());
                signatures.extend_signatures(
                    hirsel_engine::registry::BuiltinRegistry::new()
                        .signatures()
                        .map(|s| (**s).clone()),
                );
                for item in &parsed.value.items {
                    let signature = engine.item_signature(&item.item.definition).unwrap();
                    signatures.insert(item.item.definition.clone(), (*signature.value).clone());
                }
                let mut probe = entry_path(&default_context());
                probe.0.name = "probe".into();
                match engine.mode_definition(&probe) {
                    Ok(output) => assert!(
                        accepted && output.diagnostics.is_empty(),
                        "{name}: unexpectedly accepted; {:?}",
                        output.diagnostics
                    ),
                    Err(QueryError::Diagnostics(ds)) => {
                        assert!(!accepted, "{name}: {ds:?}");
                        assert_eq!(ds.len(), 1, "{name}: {ds:?}");
                        assert_eq!(ds[0].rule.as_str(), "PROV-1", "{name}: {ds:?}");
                        let expected_line: u32 = source
                            .lines()
                            .find_map(|s| s.strip_prefix("// expect: reject PROV-1 at line "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        let at = engine.sources.resolve(ds[0].primary);
                        assert_eq!(
                            source[..at.start as usize].lines().count() as u32,
                            expected_line,
                            "{name}: {ds:?}"
                        );
                    }
                    Err(error) => panic!("{name}: {error:?}"),
                }
                let analyzed = capture.body.lock().unwrap().take().expect(&name);
                let declared = signatures.get(&probe).unwrap();
                let comptime = engine.prepare_comptime(&analyzed, declared).unwrap();
                let definition = lower
                    .lower(&t, engine.type_memo(), &analyzed, &comptime, declared)
                    .unwrap()
                    .value;
                let checked = hirsel_validate::validate(&definition, &t, &signatures, &mut vec![]);
                if accepted {
                    assert!(
                        checked.violations.is_empty(),
                        "{name}: {:?}",
                        checked.violations
                    );
                } else {
                    assert!(
                        checked.violations.iter().any(|v| matches!(
                            v.kind,
                            hirsel_validate::ViolationKind::InvalidOrigin(_)
                                | hirsel_validate::ViolationKind::TypeMismatch {
                                    entity: "return" | "assignment" | "call argument"
                                }
                        )),
                        "{name}: {:?}",
                        checked.violations
                    );
                }
                eprintln!(
                    "{name}: J={} independent V={}",
                    accepted,
                    checked.violations.is_empty()
                );
            }));
            if outcome.is_err() {
                failures.push(name);
            }
        }
        assert!(failures.is_empty(), "failed cases: {failures:?}");
    });
}

#[test]
fn whole_transfers_execute_on_h_llvm_and_cranelift() {
    with_compiler_stack(|| {
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
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("expired-whole-transfer-15");
        std::fs::create_dir_all(&scratch).unwrap();
        let tools =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        for &(name, source) in POSITIVES {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let source = format!(
                "{source}\npub fn main() -> Result[(), ()] {{ probe(take [0u64]); Ok(()) }}\n"
            );
            let file = engine.sources.add("execute.hir", source).unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&default_context());
            let config = RunConfiguration {
                entry: entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let execution =
                run_observed_checked(&mut engine, &config, SimulationConfig::default()).unwrap();
            let reference =
                native::Observation::of_reference(execution.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            assert!(reference.failures.is_empty());
            let input = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(input).unwrap();
            let case_dir = scratch.join(name);
            std::fs::create_dir_all(&case_dir).unwrap();
            let outcomes = native::differential(
                &validated,
                &entry,
                &reference,
                &configurations,
                &tools,
                &case_dir,
            );
            assert_eq!(outcomes.len(), 3);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, native::Outcome::Agree),
                    "{name}/{configuration}: {outcome:?}"
                );
                eprintln!("{name}/{configuration}: agree");
            }
        }
    });
}
