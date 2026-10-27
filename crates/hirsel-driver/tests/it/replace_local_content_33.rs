//! Local replacement preserves fixed storage content independently of access rows.
use hirsel_driver::*;
use hirsel_engine::*;

const ORIGINAL: &str = include_str!("../fixtures/replace-local-content-33/original.hir");

fn named(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("replace-local-content-33.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    engine
}

#[test]
fn replace_local_content_33_original_generic_body_export() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("original.hir", ORIGINAL.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let modes = engine.mode_definition(&named("keep")).unwrap();
        assert!(modes.diagnostics.is_empty(), "{:?}", modes.diagnostics);
        // main deliberately does not instantiate keep: export the generic body itself.
        let text = engine.export_tir(&named("keep")).unwrap();
        engine.import_tir(&text).unwrap();
    });
}

fn runtime(source: &str) {
    use hirsel_driver::native::*;
    let mut engine = checked(source);
    let expected = Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 31,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(run.observation.as_ref().unwrap()),
        expected,
        "interpreter"
    );
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "checked H");
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let matrix = [
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
    let scratch = tempfile::Builder::new()
        .prefix("replace-local-content-33-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

fn source_check(source: &str, policy: LoweringPolicy) -> CheckOutput {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("main.hir");
    std::fs::write(&path, source).unwrap();
    std::fs::write(
        directory.path().join("main.hir.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema": 1, "package": {"name": "witness", "version": "1"},
            "sources": [{"path": "main.hir", "module": []}], "dependencies": [],
            "entry": {"module": [], "name": "main"}
        }))
        .unwrap(),
    )
    .unwrap();
    load_program(&path).unwrap().check(policy).unwrap()
}

#[test]
fn replace_local_content_33_retained_loan_blocks_escape() {
    with_compiler_stack(|| {
        let source = include_str!(
            "../../../../conformance/MODE/MODE-1-replace-local-content-33-retained-bad.hir"
        );
        for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
            let output = source_check(source, policy);
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|d| d.rule.as_str() == "MODE-1"
                        && output.sources.slice(d.primary) == Some("take current")),
                "{:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn replace_local_content_33_v_rejects_forged_displaced_lifetime() {
    with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        let mut engine = checked(ORIGINAL);
        let output = engine.lower_to_tir(&named("keep")).unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for signature in registry::BuiltinRegistry::new().signatures() {
            signatures.insert(signature.definition.clone(), signature.as_ref().clone());
        }
        signatures.insert(
            named("keep"),
            engine
                .signature(hirsel_syntax::DefId::intern(&named("keep")))
                .unwrap()
                .as_ref()
                .clone(),
        );
        assert!(
            hirsel_validate::validate(&output.value, engine.types(), &signatures, &mut vec![])
                .violations
                .is_empty()
        );
        let mut forged = output.value.as_ref().clone();
        let body = &mut forged.bodies[forged.entry.0 as usize];
        let displaced = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| match i.kind {
                hirsel_tir::InstructionKind::Replace { displaced, .. } => Some(displaced),
                _ => None,
            })
            .unwrap();
        let root = body.places[displaced.0 as usize].root;
        let mut row = engine
            .types()
            .get(body.locals[root.0 as usize].ty)
            .unwrap()
            .clone();
        assert!(row.modes.relation.has_environment());
        row.modes.relation = hirsel_types::Relation::global();
        let global = engine.types().intern(row);
        body.locals[root.0 as usize].ty = global;
        for place in &mut body.places {
            if place.root == root {
                place.ty = global;
            }
        }
        forged.types = hirsel_tir::definition_types(engine.types(), &forged);
        let report = hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "displaced storage content"
                }),
            "{report:?}"
        );
    });
}

const MUTATION: &str =
    include_str!("../../../../conformance/MODE/MODE-1-replace-local-content-33-mutation-bad.hir");

#[test]
fn replace_local_content_33_retained_loan_blocks_mutation() {
    with_compiler_stack(|| {
        for policy in [LoweringPolicy::Comptime, LoweringPolicy::Reachable] {
            let output = source_check(MUTATION, policy);
            let diagnostic = output
                .diagnostics
                .iter()
                .find(|d| {
                    d.rule.as_str() == "MODE-1"
                        && output.sources.slice(d.primary) == Some("touch(mut xs)")
                })
                .unwrap_or_else(|| panic!("{:?}", output.diagnostics));
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(|l| output.sources.slice(l.span) == Some("unique source")
                        && l.message.contains("abstract unique copy")),
                "{diagnostic:?}"
            );
            assert!(
                diagnostic
                    .secondary
                    .iter()
                    .any(|l| l.message.contains("declared source relation")
                        && l.message.contains("xs")),
                "{diagnostic:?}"
            );
            assert!(
                diagnostic
                    .notes
                    .iter()
                    .any(|n| n.contains("End `current`") && n.contains("concrete type")),
                "{diagnostic:?}"
            );
        }
    });
}

#[test]
fn replace_local_content_33_v_rejects_forged_mutation_acceptance() {
    with_compiler_stack(|| {
        use hirsel_types::SignatureProvider;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("mutation.hir", MUTATION.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        // Lower an otherwise identical body whose producer erased the written
        // relation, then restore the true declaration and parameter row.
        // Independent V must derive the retained dependency itself.
        let safe = MUTATION.replace("  touch(mut xs)\n  inspect(current)", "  let observed = inspect(current)\n  discard(take current)\n  touch(mut xs)\n  observed") + "\nfn discard[T](take x: T in x) {}\n";
        // Explicit valid control: the dependent value is consumed before mutation.
        let mut control = checked(&safe);
        let text = control.export_tir(&named("probe")).unwrap();
        control.import_tir(&text).unwrap();
        let typed = engine.check_definition(&named("probe")).unwrap();
        let related = typed
            .value
            .checked
            .bindings
            .iter()
            .find(|binding| {
                binding.owner == hirsel_types::NameSite::Item
                    && binding.binding
                        == hirsel_types::ResolvedName::Parameter(hirsel_types::ParamId(1))
            })
            .unwrap()
            .ty;
        let signature = engine
            .signature(hirsel_syntax::DefId::intern(&named("probe")))
            .unwrap();
        let mut erased = QueryEngine::new(default_environment(), compiler_adapters())
            .with_types(engine.types().clone());
        let source = MUTATION.replace("source: T in xs", "source: T");
        let file = erased.sources.add("erased.hir", source).unwrap();
        let check = check_progress(&mut erased, file, &default_context()).unwrap();
        assert!(check.diagnostics.is_empty(), "{:?}", check.diagnostics);
        let lowered = erased.lower_to_tir(&named("probe")).unwrap();
        let mut forged = lowered.value.as_ref().clone();
        forged.signature_hash = signature.fingerprint;
        forged.inferred_unique = signature.metadata.inferred_unique.clone();
        let body = &mut forged.bodies[forged.entry.0 as usize];
        let source = body.parameters[1];
        body.locals[source.0 as usize].ty = related;
        for place in &mut body.places {
            if place.root == source {
                place.ty = related;
            }
        }
        forged.types = hirsel_tir::definition_types(engine.types(), &forged);
        let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
        for signature in registry::BuiltinRegistry::new().signatures() {
            signatures.insert(signature.definition.clone(), signature.as_ref().clone());
        }
        for name in ["probe", "touch"] {
            signatures.insert(
                named(name),
                engine
                    .signature(hirsel_syntax::DefId::intern(&named(name)))
                    .unwrap()
                    .as_ref()
                    .clone(),
            );
        }
        let report = hirsel_validate::validate(&forged, engine.types(), &signatures, &mut vec![]);
        assert!(
            report.violations.iter().any(|v| matches!(
                v.kind,
                hirsel_validate::ViolationKind::InvalidOrigin(_)
                    | hirsel_validate::ViolationKind::LoanConflict { .. }
            )),
            "{report:?}"
        );
    });
}
