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
    assert!(
        cache
            .lookup(subject, policy, StageKind::Value, &environment)
            .is_some()
    );
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
