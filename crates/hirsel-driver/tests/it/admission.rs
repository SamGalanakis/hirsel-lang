use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_run_config::*;

#[derive(Default)]
struct Host(usize);
impl HostAuthority for Host {
    fn authorize(&mut self, _: &RunConfig) -> Result<(), RunRefusal> {
        self.0 += 1;
        Ok(())
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        Err(RunRefusal::MissingEvidence {
            reference: reference.into(),
        })
    }
}
fn fixture(parameters: &str) -> (hirsel_types::SignatureRef, ProgramRevision) {
    let source = format!("pub fn main({parameters}) -> Result[(), ()] {{ Ok(()) }}");
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
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
                admit_run(&signature, &revision, LaunchKind::Main, None, &mut host, 0),
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
                    &signature,
                    &revision,
                    LaunchKind::Main,
                    Some(&record),
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
                &signature,
                &revision,
                LaunchKind::Main,
                None,
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
                    })
                }
            }
            let mut host = Host::default();
            assert!(
                admit_run(
                    &signature,
                    &revision,
                    LaunchKind::Main,
                    Some(&record.validated().unwrap()),
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
                &signature,
                &revision,
                LaunchKind::Main,
                Some(&record),
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
            entry_roots(&signature, LaunchKind::Main).unwrap(),
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
            let mut row = forged.types.get(ty).unwrap().clone();
            let hirsel_types::TypeKind::Named { definition, .. } = &mut row.kind else {
                unreachable!()
            };
            definition.0.package.name = "user".into();
            let replacement = forged.types.push(row);
            let hirsel_types::SignatureKind::Function(function) = &mut forged.kind else {
                unreachable!()
            };
            if name == "Io" {
                function.parameters[0].ty = replacement;
            } else {
                function.result = replacement;
            }
            assert!(entry_roots(&forged, LaunchKind::Main).is_err());
        }
    });
}

#[test]
fn record_path_must_be_absolute() {
    assert!(
        matches!(read_run_record(std::path::Path::new("run.json")), Err(RunRefusal::Invalid { field, .. }) if field == "--hirsel-run")
    );
}

#[test]
fn unrelated_signature_refuses_before_authorizing_the_selected_program() {
    with_compiler_stack(|| {
        let (signature, revision) = fixture("");
        let mut wrong = (*signature).clone();
        wrong.definition.0.name = "other".into();
        let mut host = Host::default();
        assert!(
            matches!(admit_run(&wrong, &revision, LaunchKind::Main, None, &mut host, 0), Err(RunRefusal::Binding { field }) if field == "entry.signature")
        );
        assert_eq!(host.0, 0);
        assert!(admit_run(&signature, &revision, LaunchKind::Main, None, &mut host, 0).is_ok());
        assert_eq!(host.0, 1);
    });
}
