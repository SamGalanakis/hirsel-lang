use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_run_config::*;
use hirsel_run_schema::*;
use hirsel_run_schema::*;

#[derive(Default)]
struct Host(usize);
impl HostAuthority for Host {
    fn authorize(&mut self, _: &ValidatedRun) -> Result<(), RunRefusal> {
        self.0 += 1;
        Ok(())
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        Err(RunRefusal::MissingEvidence {
            reference: reference.into(),
        })
    }
}
/// One interner for every fixture, so forged signatures share its rows.
fn types() -> &'static std::sync::Arc<hirsel_types::TypeInterner> {
    static TYPES: std::sync::OnceLock<std::sync::Arc<hirsel_types::TypeInterner>> =
        std::sync::OnceLock::new();
    TYPES.get_or_init(hirsel_engine::registry::session_types)
}
fn fixture(parameters: &str) -> (hirsel_types::SignatureRef, ProgramRevision) {
    let source = format!("pub fn main({parameters}) -> Result[(), ()] {{ Ok(()) }}");
    let mut engine =
        QueryEngine::new(default_environment(), compiler_adapters()).with_types(types().clone());
    let file = engine.sources.add("admission.hir", source.clone()).unwrap();
    let context = default_context();
    check_snapshot(&mut engine, file, &context).unwrap();
    let signature = engine.item_signature(&entry_path(&context)).unwrap().value;
    let revision = engine
        .semantic_program_revision(&entry_path(&context))
        .unwrap();
    (signature, revision)
}

#[test]
fn no_record_denies_roots_before_host_admission_and_empty_record_is_admitted() {
    with_compiler_stack(|| {
        for parameters in [
            "rt: Io, args: Args",
            "rt: Io",
            "args: Args",
            "env: Env",
            "u: Unsafe",
        ] {
            let (signature, revision) = fixture(parameters);
            let mut host = Host::default();
            assert!(matches!(
                admit_run(
                    types(),
                    &signature,
                    &revision,
                    LaunchKind::Main,
                    None,
                    CodeReach::Safe,
                    &mut host,
                    0
                ),
                Err(RunRefusal::NotGranted { .. })
            ));
            assert_eq!(host.0, 0);
            let mut record = RunConfig::for_program(&revision, LaunchKind::Main);
            record.io = Some(IoGrants::default());
            record.args = Some(snapshot_args([]).unwrap());
            record.env = Some(snapshot_env([]).unwrap());
            record.unsafe_root = true;
            let record = record.validated().unwrap();
            assert!(
                admit_run(
                    types(),
                    &signature,
                    &revision,
                    LaunchKind::Main,
                    Some(&record),
                    CodeReach::Safe,
                    &mut host,
                    0
                )
                .is_ok()
            );
            assert_eq!(host.0, 1);
        }
        let (signature, revision) = fixture("");
        assert!(
            admit_run(
                types(),
                &signature,
                &revision,
                LaunchKind::Main,
                None,
                CodeReach::Safe,
                &mut Host::default(),
                0
            )
            .is_ok()
        );
    });
}

#[test]
fn wrong_program_entry_kind_and_expired_admission_have_no_host_effects() {
    with_compiler_stack(|| {
        let (signature, revision) = fixture("");
        for field in ["program", "entry", "launch", "expiry"] {
            let mut record = RunConfig::for_program(&revision, LaunchKind::Main);
            match field {
                "program" => record.program = hex(&[8; 32]),
                "entry" => record.entry = "standalone@0/other".into(),
                "launch" => record.launch = LaunchKind::Test,
                _ => {
                    record.admission = Some(Admission {
                        not_before: None,
                        expires: Some(4),
                        revocation: None,
                        code: None,
                    })
                }
            }
            let mut host = Host::default();
            assert!(
                admit_run(
                    types(),
                    &signature,
                    &revision,
                    LaunchKind::Main,
                    Some(&record.validated().unwrap()),
                    CodeReach::Safe,
                    &mut host,
                    5
                )
                .is_err()
            );
            assert_eq!(host.0, 0);
        }
        let record = RunConfig::for_program(&revision, LaunchKind::Main)
            .validated()
            .unwrap();
        assert!(
            admit_run(
                types(),
                &signature,
                &revision,
                LaunchKind::Main,
                Some(&record),
                CodeReach::Safe,
                &mut Host::default(),
                5
            )
            .is_ok()
        );
    });
}

#[test]
fn nominal_root_and_result_checks_refuse_lookalikes() {
    with_compiler_stack(|| {
        let (signature, _) = fixture("rt: Io");
        assert_eq!(
            entry_roots(types(), &signature, LaunchKind::Main).unwrap(),
            [Root::Io]
        );
        for name in ["Io", "Result"] {
            let mut forged = (*signature).clone();
            let hirsel_types::SignatureKind::Function(function) = &forged.kind else {
                unreachable!()
            };
            let ty = if name == "Io" {
                function.parameters[0].ty
            } else {
                function.result
            };
            let mut row = types().get(ty).unwrap().clone();
            let hirsel_types::TypeKind::Named { definition, .. } = &mut row.kind else {
                unreachable!()
            };
            let mut lookalike = definition.path().clone();
            lookalike.0.package.name = "user".into();
            *definition = hirsel_syntax::DefId::intern(&lookalike);
            let replacement = types().intern(row);
            let hirsel_types::SignatureKind::Function(function) = &mut forged.kind else {
                unreachable!()
            };
            if name == "Io" {
                function.parameters[0].ty = replacement;
            } else {
                function.result = replacement;
            }
            assert!(entry_roots(types(), &forged, LaunchKind::Main).is_err());
        }
    });
}

#[test]
fn record_path_must_be_absolute() {
    let file = tempfile::NamedTempFile::new_in(".").unwrap();
    let record = RunConfig::empty([0; 32], "probe@1/main".into(), LaunchKind::Main)
        .validated()
        .unwrap();
    std::fs::write(file.path(), record.canonical()).unwrap();
    let absolute = std::fs::canonicalize(file.path()).unwrap();
    assert_eq!(read_run_record(&absolute).unwrap(), record);
    let relative = std::path::Path::new(file.path().file_name().unwrap());
    assert!(relative.is_file());
    assert!(
        matches!(read_run_record(relative), Err(RunRefusal::Invalid { field, .. }) if field == "--hirsel-run")
    );
}

#[test]
fn unrelated_signature_refuses_before_authorizing_the_selected_program() {
    with_compiler_stack(|| {
        let (signature, revision) = fixture("");
        let mut wrong = (*signature).clone();
        let mut other = wrong.definition.path().clone();
        other.0.name = "other".into();
        wrong.definition = hirsel_syntax::DefId::intern(&other);
        let mut host = Host::default();
        assert!(
            matches!(admit_run(types(), &wrong, &revision, LaunchKind::Main, None, CodeReach::Safe, &mut host, 0), Err(RunRefusal::Binding { field }) if field == "entry.signature")
        );
        assert_eq!(host.0, 0);
        assert!(
            admit_run(
                types(),
                &signature,
                &revision,
                LaunchKind::Main,
                None,
                CodeReach::Safe,
                &mut host,
                0
            )
            .is_ok()
        );
        assert_eq!(host.0, 1);
    });
}
