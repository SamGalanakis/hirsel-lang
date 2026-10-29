//! Readiness adapter controls. The linker returns a tiny script fixture; these
//! do not stand in for compiler/Cranelift/LLVM conformance or performance gates.
use hirsel_artifact_store::Domain;
use hirsel_driver::native::{object_queries::*, queries::*};
use hirsel_driver::resident::ExecutableOutput;
use hirsel_engine::{
    requests::OutputReadiness,
    store::{DirStore, ResultStore},
};
use hirsel_syntax::ContentHash;
use std::path::Path;
use std::sync::Arc;

struct NoNative;
impl NativeProducts for NoNative {
    fn instance(&mut self, _: &InstanceAddress) -> Result<Arc<InstanceProduct>, String> {
        panic!("readiness must not lower instances")
    }
    fn arena_summary(&mut self, _: &ComponentKey) -> Result<Arc<ArenaSummary>, String> {
        panic!("readiness must not summarize arenas")
    }
    fn ready(&mut self, _: &InstanceAddress) -> Result<Arc<ReadyProduct>, String> {
        panic!("readiness must not reconstruct Ready")
    }
    fn with_ready<R>(
        &mut self,
        _: &[Arc<ReadyProduct>],
        _: impl FnOnce(
            &[&hirsel_lir::stage::ValidatedUnit<hirsel_lir::stage::Ready>],
            &hirsel_lir::emission::EmitInputs<'_>,
        ) -> Result<R, String>,
    ) -> Result<R, String> {
        panic!("readiness must not materialize native bodies")
    }
}
const SCRIPT: &[u8] = b"#!/bin/sh\nprintf 'ready\\n'\nexit 0\n";
fn fixture(store: Arc<dyn ResultStore>) -> LinkedProduct {
    let manifest = store
        .put_content(
            Domain {
                kind: 999,
                schema: 1,
            },
            b"fixture manifest",
        )
        .unwrap();
    let key = LinkKey {
        roots: vec![].into(),
        target: ContentHash::of_bytes(b"fixture target"),
        linker_options: ContentHash::of_bytes(b"fixture linker"),
    };
    let inputs = LinkInputs {
        execution: vec![],
        fragments: vec![],
        static_manifest: manifest,
        runtime_archives: vec![],
        foreign_inputs: vec![],
        launch_manifest: manifest,
    };
    fn link(
        _: &LinkKey,
        _: &LinkInputs,
        _: &[hirsel_lir::stage::ObjectArtifact],
    ) -> Result<Vec<u8>, String> {
        Ok(SCRIPT.to_vec())
    }
    ObjectQueries::new(store, NoNative, link)
        .link(&key, &inputs)
        .unwrap()
        .as_ref()
        .clone()
}
fn output(store: Arc<dyn ResultStore>, destination: &Path) -> ExecutableOutput {
    ExecutableOutput {
        store,
        destination: destination.into(),
    }
}
#[test]
fn executable_readiness_uses_provider_and_repairs_missing_output_after_restart() {
    use std::os::unix::fs::MetadataExt;
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path().join("store");
    let destination = tree.path().join("out/program");
    let store = Arc::new(DirStore::new(root.clone()));
    let linked = fixture(store.clone());
    let mut ready = output(store, &destination);
    assert_eq!(ready.ready(&[linked.product.clone()]).unwrap(), destination);
    let inode = destination.metadata().unwrap().ino();
    ready.ready(&[linked.product.clone()]).unwrap();
    assert_eq!(destination.metadata().unwrap().ino(), inode);
    std::fs::remove_file(&destination).unwrap();
    drop(ready);
    let mut restarted = output(Arc::new(DirStore::new(root)), &destination);
    restarted.ready(&[linked.product.clone()]).unwrap();
    let child = std::process::Command::new(&destination).output().unwrap();
    assert!(child.status.success());
    assert_eq!(child.stdout, b"ready\n");
    assert!(child.stderr.is_empty());
}
#[test]
fn existing_destination_does_not_mask_corrupt_or_missing_linked_record() {
    let tree = tempfile::tempdir().unwrap();
    let root = tree.path().join("store");
    let destination = tree.path().join("program");
    let store = Arc::new(DirStore::new(root.clone()));
    let linked = fixture(store.clone());
    let mut ready = output(store, &destination);
    ready.ready(&[linked.product.clone()]).unwrap();
    // Physical path is used only to inject corruption, never by the adapter.
    let record = hirsel_artifact_store::Store::new(root).output_path(linked.product.record);
    std::fs::write(&record, b"truncated").unwrap();
    assert!(ready.ready(&[linked.product.clone()]).is_err());
    std::fs::remove_file(record).unwrap();
    assert!(ready.ready(&[linked.product.clone()]).is_err());
    assert_eq!(std::fs::read(destination).unwrap(), SCRIPT);
}
#[test]
fn executable_readiness_refuses_wrong_root_count_before_materialization() {
    let tree = tempfile::tempdir().unwrap();
    let store = Arc::new(DirStore::new(tree.path().join("store")));
    let linked = fixture(store.clone());
    let destination = tree.path().join("program");
    let mut ready = output(store, &destination);
    assert!(ready.ready(&[]).is_err());
    assert!(
        ready
            .ready(&[linked.product.clone(), linked.product])
            .is_err()
    );
    assert!(!destination.exists());
}
