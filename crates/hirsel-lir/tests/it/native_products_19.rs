use crate::support::{self, *};
use hirsel_artifact_store::{Contents, Memory};
use hirsel_lir::stage::{Ready, StageKind, Value};
use hirsel_lir::validate::{Inputs, NativeCache, NativeEnvironment};
use std::sync::Arc;

#[test]
fn native_products_19_only_actual_validation_can_issue_a_local_output() {
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let store = Arc::new(Memory::default());
    let cache = NativeCache::new(store.clone(), None);
    let subject = hash("native query action");
    let policy = hash("native policy");
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    let (checked, output) = cache
        .validate_and_publish(subject, policy, unit.clone(), &inputs, &[], b"query data")
        .unwrap();
    let output = output.unwrap();
    assert_eq!(output.metadata(), b"query data");
    let trace = hirsel_lir::construction::ValueTrace::default();
    let observe = trace.enter();
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    assert_eq!(
        trace.totals(),
        hirsel_lir::construction::ValueCounts::default(),
        "native descriptor acceptance constructs/decodes/clones/rebinds no Value"
    );
    drop(observe);
    assert!(
        cache
            .lookup(
                hash("changed action"),
                policy,
                StageKind::Value,
                &environment
            )
            .is_none()
    );
    assert!(
        cache
            .lookup(
                subject,
                hash("changed policy"),
                StageKind::Value,
                &environment
            )
            .is_none()
    );
    assert!(
        cache
            .lookup(subject, policy, StageKind::Ready, &environment)
            .is_none()
    );
    let (closure, body) = cache.read(&output, &target).unwrap();
    let binding = hirsel_lir::products::Binding::new(&target, &[&closure]).unwrap();
    let rebound = binding.materialize::<Value>(&body).unwrap();
    let local = Inputs {
        memo: Default::default(),
        layouts: &binding.table,
        ..inputs
    };
    assert!(hirsel_lir::validate::validate_value(rebound, &local).is_ok());
    // Copy all immutable content, including a producer's claimed metadata.
    // An independent reader without this validator's local association must V/C.
    let remote = Arc::new(Memory::default());
    for (reference, bytes) in store.entries() {
        assert_eq!(
            remote.put_content(reference.domain, &bytes),
            Some(reference)
        );
    }
    let foreign = NativeCache::new(remote, None);
    assert!(
        foreign
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    let mut malformed = checked.into_raw();
    malformed.header.target = hash("foreign target");
    assert!(
        cache
            .validate_and_publish(hash("refused"), policy, malformed, &local, &[], &[])
            .is_err()
    );
    assert!(
        cache
            .lookup(hash("refused"), policy, StageKind::Value, &environment)
            .is_none()
    );
    // The raw Ready stage also needs its own actual validation, not a Value receipt.
    let mut ready = unit.into_stage::<Ready>();
    ready.header.key = hirsel_lir::objects::UnitKey(hash("ready"));
    let original = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let (_, ready_output) = cache
        .validate_and_publish(subject, policy, ready, &original, &[], &[])
        .unwrap();
    assert_eq!(ready_output.unwrap().stage(), StageKind::Ready);
}

#[test]
fn native_products_19_manifest_discharge_and_target_are_independent_reads() {
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let cache = NativeCache::new(Arc::new(Memory::default()), None);
    let subject = hash("reads");
    let policy = hash("policy");
    cache
        .validate_and_publish(subject, policy, unit.clone(), &inputs, &[], &[])
        .unwrap()
        .1
        .unwrap();
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    let different_manifest = support::manifest(hirsel_core_contract::Disposition::Body);
    let changed = NativeEnvironment {
        manifest: &different_manifest,
        ..environment
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &changed)
            .is_none()
    );
    let mut discharge = hirsel_lir::discharge::Discharge::default();
    discharge.0.insert(
        unit.header.definition.clone(),
        std::collections::BTreeSet::from([hirsel_lir::discharge::Discharged {
            body: 0,
            block: 0,
            statement: 0,
            form: hirsel_lir::discharge::Form::Integer,
        }]),
    );
    let changed = NativeEnvironment {
        discharge: Some(&discharge),
        ..environment
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &changed)
            .is_none()
    );
    let target = hirsel_lir::TargetSpec::aarch64_unknown_linux_gnu();
    let changed = NativeEnvironment {
        target: &target,
        ..environment
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &changed)
            .is_none()
    );
}

#[test]
fn native_products_19_directory_reopen_copy_and_corruption() {
    use hirsel_artifact_store::Store;
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-output-19-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let local = Store::new(root.join("local"));
    let subject = hash("directory native action");
    let policy = hash("directory native policy");
    let cache = NativeCache::new(Arc::new(local.clone()), Some(local.clone()));
    let (_, output) = cache
        .validate_and_publish(subject, policy, unit, &inputs, &[], b"metadata")
        .unwrap();
    let output = output.unwrap();
    drop(cache);
    let reopened = NativeCache::new(Arc::new(local.clone()), Some(local.clone()));
    let current = reopened
        .lookup(subject, policy, StageKind::Value, &environment)
        .unwrap();
    assert_eq!(current.body(), output.body());
    assert_eq!(current.metadata(), output.metadata());
    let copied = Store::new(root.join("copied"));
    for reference in [output.body(), output.layouts()] {
        assert_eq!(
            copied
                .put(reference.domain, &local.get(reference).unwrap())
                .unwrap(),
            reference
        );
    }
    // Copy receipt content too, with no local receipt association. It remains ordinary bytes.
    fn copy_outputs(from: &std::path::Path, into: &std::path::Path) {
        std::fs::create_dir_all(into).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy_outputs(&entry.path(), &into.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), into.join(entry.file_name())).unwrap();
            }
        }
    }
    copy_outputs(
        &local.root().join("outputs"),
        &copied.root().join("outputs"),
    );
    let foreign = NativeCache::new(Arc::new(copied.clone()), Some(copied));
    assert!(
        foreign
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    // Body corruption must invalidate even an output minted in this process.
    std::fs::write(local.output_path(output.body()), b"truncated").unwrap();
    assert!(
        reopened
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    assert!(reopened.read(&output, &target).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_products_19_receipt_io_failure_retains_only_actual_process_acceptance() {
    use hirsel_artifact_store::Store;
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-receipt-io-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let blocked = root.join("not-a-directory");
    std::fs::write(&blocked, b"publication blocked").unwrap();
    let local = Store::new(blocked);
    let contents = Arc::new(Memory::default());
    let cache = NativeCache::new(contents.clone(), Some(local.clone()));
    let subject = hash("failed native receipt publication");
    let policy = hash("native policy");
    let (_, output) = cache
        .validate_and_publish(subject, policy, unit, &inputs, &[], &[])
        .unwrap();
    assert!(
        output.is_some(),
        "actual C acceptance remains usable in this process"
    );
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    drop(cache);
    let reopened = NativeCache::new(contents, Some(local));
    assert!(
        reopened
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none(),
        "receipt I/O failure cannot claim durable acceptance"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_products_19_derived_metadata_retains_actual_c_observations() {
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    let owner = unit.header.definition.clone();
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let operations = support::registry();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let store = Arc::new(Memory::default());
    let cache = NativeCache::new(store.clone(), None);
    let subject = hash("derived metadata publication");
    let policy = hash("derived metadata policy");
    let (_, output) = cache
        .validate_output(subject, policy, unit, &inputs, &[])
        .unwrap();
    let output = output.unwrap();
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    let published = cache.publish_output(&output, b"local equations").unwrap();
    assert_eq!(published.body(), output.body());
    assert_eq!(published.layouts(), output.layouts());
    assert_eq!(published.stage(), output.stage());
    assert_eq!(published.metadata(), b"local equations");
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    let mut discharge = hirsel_lir::discharge::Discharge::default();
    discharge.0.insert(
        owner,
        std::collections::BTreeSet::from([hirsel_lir::discharge::Discharged {
            body: 0,
            block: 0,
            statement: 0,
            form: hirsel_lir::discharge::Form::Integer,
        }]),
    );
    let changed = NativeEnvironment {
        discharge: Some(&discharge),
        ..environment
    };
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &changed)
            .is_none()
    );
    let foreign = NativeCache::new(store, None);
    assert!(
        foreign
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
}

#[test]
fn native_products_19_source_observation_is_independent_and_scope_bound() {
    use hirsel_artifact_store::Store;
    use hirsel_lir::operation::{OperationArgument, OperationDeclarations};
    use hirsel_lir::sealed_payload_source::SealSourceWitness;

    struct Declarations {
        registry: hirsel_layout::operation::OperationRegistry,
        witness: Option<SealSourceWitness>,
    }
    impl OperationDeclarations for Declarations {
        fn seal_source_witness(
            &self,
            _: &hirsel_syntax::DefPath,
            _: hirsel_syntax::ContentHash,
        ) -> Option<SealSourceWitness> {
            // Deliberately return the independently supplied row even for a
            // mismatching request, so the cache must check its scope itself.
            self.witness.clone()
        }
        fn derive(
            &self,
            operation: &hirsel_core_contract::OperationId,
            arguments: &[OperationArgument],
            target: &hirsel_lir::TargetSpec,
        ) -> Result<(hirsel_lir::FnAbi, hirsel_lir::LayoutTable), hirsel_lir::layout::LayoutError>
        {
            self.registry.derive(operation, arguments, target)
        }
    }

    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, table, manifest) = fixture.finish();
    // A separately supplied original-source observation may prove an empty
    // inventory. This identity is data, not permission to skip actual C.
    let witness = SealSourceWitness::from_expectations(
        hash("current private source output"),
        unit.header.definition.clone(),
        unit.header.implementation_revision,
        unit.header.instance,
        &[],
        &[],
    );
    let operations = Declarations {
        registry: support::registry(),
        witness: Some(witness.clone()),
    };
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = Inputs {
        memo: Default::default(),
        target: &target,
        layouts: &table,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let environment = NativeEnvironment {
        target: &target,
        manifest: &manifest,
        operations: &operations,
        discharge: None,
    };
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "native-source-observation-19-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let local = Store::new(root.clone());
    let cache = NativeCache::new(Arc::new(local.clone()), Some(local.clone()));
    let subject = hash("source observation query action");
    let policy = hash("source observation query policy");
    let (_, output) = cache
        .validate_output(subject, policy, unit.clone(), &inputs, &[])
        .unwrap();
    let output = output.unwrap();
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    cache
        .publish_output(&output, b"independent query metadata")
        .unwrap();
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    drop(cache);
    // The source read survives durable receipt encoding and a fresh process
    // authority table; neither the Value nor Ready body is materialized here.
    let reopened = NativeCache::new(Arc::new(local.clone()), Some(local));
    let accepted = reopened
        .lookup(subject, policy, StageKind::Value, &environment)
        .unwrap();
    assert_eq!(accepted.metadata(), b"independent query metadata");

    let unavailable = Declarations {
        registry: support::registry(),
        witness: None,
    };
    let changed = NativeEnvironment {
        operations: &unavailable,
        ..environment
    };
    assert!(
        reopened
            .lookup(subject, policy, StageKind::Value, &changed)
            .is_none()
    );
    for field in 0..6 {
        let mut changed_witness = witness.clone();
        match field {
            0 => changed_witness.source = hash("changed private source output"),
            1 => changed_witness.implementation_revision = hash("changed implementation"),
            2 => changed_witness.instance = hash("different type or size actuals"),
            3 => changed_witness.definition = path("other"),
            4 => changed_witness.inventory_rule += 1,
            5 => changed_witness.expectations = hash("changed semantic expectations"),
            _ => unreachable!(),
        }
        let changed_operations = Declarations {
            registry: support::registry(),
            witness: Some(changed_witness),
        };
        let changed = NativeEnvironment {
            operations: &changed_operations,
            ..environment
        };
        assert!(
            reopened
                .lookup(subject, policy, StageKind::Value, &changed)
                .is_none()
        );
        // A provider row for the wrong actual definition/instance/revision
        // cannot mint accepted native output, even when the raw unit passes C.
        if (1..=3).contains(&field) {
            let changed_inputs = Inputs {
                memo: Default::default(),
                operations: &changed_operations,
                ..inputs
            };
            let (_, refused) = reopened
                .validate_output(
                    hash("wrong source witness scope"),
                    policy,
                    unit.clone(),
                    &changed_inputs,
                    &[],
                )
                .unwrap();
            assert!(refused.is_none());
        }
    }
    // Absence also cannot be retroactively promoted to expected-empty.
    let absent_inputs = Inputs {
        memo: Default::default(),
        operations: &unavailable,
        ..inputs
    };
    let absent_subject = hash("unavailable source observation");
    reopened
        .validate_and_publish(
            absent_subject,
            policy,
            unit.clone(),
            &absent_inputs,
            &[],
            &[],
        )
        .unwrap()
        .1
        .unwrap();
    assert!(
        reopened
            .lookup(absent_subject, policy, StageKind::Value, &changed)
            .is_some()
    );
    assert!(
        reopened
            .lookup(absent_subject, policy, StageKind::Value, &environment)
            .is_none()
    );
    let mut malformed = unit;
    malformed.header.target = hash("invalid native target despite source witness");
    assert!(
        reopened
            .validate_output(hash("C still required"), policy, malformed, &inputs, &[])
            .is_err()
    );
    assert!(
        reopened
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn native_products_19_value_trace_tracks_real_construction_and_worker_scopes() {
    use hirsel_lir::construction::{ValueCounts, ValueTrace};
    let mut fixture = Lir::<Value>::new(&[], None);
    fixture.builder.ret(ENTRY, None);
    let (unit, _, _) = fixture.finish();
    let request = ValueTrace::default();
    let unrelated = ValueTrace::default();
    let outer = request.enter();
    let inner = unrelated.enter();
    // Cancellation may clean scopes in either order; dropping the outer
    // subscriber must not detach the inner or restore an expired subscriber.
    drop(outer);
    let _copy = unit.clone();
    assert_eq!(unrelated.totals().cloned, 1);
    assert_eq!(request.totals(), ValueCounts::default());
    drop(inner);
    let raw = unit.clone();
    assert_eq!(request.totals(), ValueCounts::default());
    let worker_request = request.clone();
    std::thread::spawn(move || {
        let _observe = worker_request.enter();
        let _created = hirsel_lir::LirUnit::<Value>::new(raw.header.clone());
        let copy = raw.clone();
        assert_eq!(copy, raw);
        let text = hirsel_codec::to_text(&copy);
        let decoded: hirsel_lir::LirUnit<Value> = hirsel_codec::from_text(&text).unwrap();
        let _retagged = decoded.into_stage::<Value>();
        let (erased, layouts, abis) = hirsel_lir::objects::erased(&copy);
        let rebound = hirsel_lir::objects::rebound(erased, &layouts, &abis).unwrap();
        assert_eq!(rebound, copy);
        let counts = worker_request.totals();
        assert_eq!(counts.created, 1);
        assert_eq!(counts.cloned, 2);
        assert_eq!(counts.decoded, 1);
        assert_eq!(counts.retagged, 1);
        assert_eq!(counts.rebound, 1);
        assert!(counts.decoded_bytes > 0 && counts.decoded_bytes <= text.len() as u64);
    })
    .join()
    .unwrap();
    assert_eq!(
        request.snapshot().keys().collect::<Vec<_>>(),
        vec![&(unit.header.definition.clone(), unit.header.instance)]
    );
    let after = request.totals();
    let _unobserved = unit.clone();
    assert_eq!(
        request.totals(),
        after,
        "a worker trace never leaks into its parent"
    );
}
