//! Real front graph controls for transport ownership and immutable revisions.
use super::*;
use crate::requests::{
    CompileOperation, CompileRequest, CompileResult, CompilerRequests, RequestLimits,
};
use hirsel_engine::{
    CompileConfig, LoweringPolicy, loader_inputs::InputLimits, requests::observe_inputs,
    runtime::ProcessBudget,
};
use hirsel_intern::jobs::Jobs;
use std::{collections::BTreeMap, path::PathBuf};

const ORIGINAL: &str = "pub fn main()->u8{1u8}\n";
fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("main.hir");
    let entry = directory.path().join("hirsel.json");
    std::fs::write(&source, ORIGINAL).unwrap();
    std::fs::write(&entry, r#"{"schema":1,"package":{"name":"request","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    (directory, entry, source)
}
fn config() -> CompileConfig {
    CompileConfig {
        jobs: Jobs::ONE,
        hold_bytes: 0,
        ..Default::default()
    }
}
fn selector(budget: Arc<ProcessBudget>) -> CompilerRequests {
    CompilerRequests::new(
        crate::COMPILER,
        config(),
        budget,
        RequestLimits {
            runtime_bytes: 192 << 20,
            snapshot_bytes: 16 << 20,
            transport_bytes: 16 << 20,
        },
    )
}
fn request(entry: &std::path::Path) -> CompileRequest {
    let compiler = crate::COMPILER.with_config(config()).with_owned_engines();
    let (discovery, inputs) = observe_inputs(
        InputLimits {
            bytes: 1 << 20,
            operations: 256,
        },
        3,
        |inputs| {
            compiler
                .load_check_package_inputs(entry, inputs, false)
                .map(|_| ())
        },
    )
    .unwrap();
    discovery.unwrap();
    CompileRequest {
        path: entry.to_owned(),
        inputs,
        operation: CompileOperation::Check {
            lowering: LoweringPolicy::Comptime,
        },
    }
}
fn checked(output: &crate::requests::CompileOutput) -> &hirsel_engine::CheckOutput {
    let CompileResult::Check(checked) = &output.result else {
        panic!("check root returned another operation")
    };
    checked
}

#[test]
fn real_compiler_requests_coalesce_cancel_independently_and_keep_frozen_revisions() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        let initial = request(&entry);
        let mut host = Host::new(selector(ProcessBudget::new(768 << 20)));
        let old = host.request(&initial).unwrap();
        let cancelled = host.request(&initial).unwrap();
        let also_old = host.request(&initial).unwrap();
        assert!(Arc::ptr_eq(
            &old.subscription.runtime,
            &also_old.subscription.runtime
        ));
        drop(cancelled);
        // New disk state must not enter an already selected immutable graph.
        std::fs::write(&source, "pub fn main()->u8{missing}\n").unwrap();
        let delivered: Arc<[u8]> = b"pub fn main()->u8{true}\n".as_slice().into();
        let mut changed = initial.clone();
        changed.inputs = initial
            .inputs
            .replace_sources(
                &BTreeMap::from([(source.clone(), delivered.clone())]),
                1 << 20,
            )
            .unwrap();
        let new = host.request(&changed).unwrap();
        assert!(!Arc::ptr_eq(
            &old.subscription.runtime,
            &new.subscription.runtime
        ));
        let old = old.wait().unwrap();
        let also_old = also_old.wait().unwrap();
        assert!(checked(&old).diagnostics.is_empty());
        assert!(checked(&also_old).diagnostics.is_empty());
        assert!(checked(&old).sources.anchors().any(|(_, file, _)| {
            checked(&old)
                .sources
                .file(file)
                .is_some_and(|file| file.path == source && file.text() == ORIGINAL)
        }));
        let new = new.wait().unwrap();
        assert!(!checked(&new).diagnostics.is_empty());
        assert!(checked(&new).sources.anchors().any(|(_, file, _)| {
            checked(&new).sources.file(file).is_some_and(|file| {
                file.path == source && file.text().as_bytes() == delivered.as_ref()
            })
        }));
    });
}

#[test]
fn real_compiler_host_shutdown_cancels_its_subscription_and_releases_the_allowance() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, _source) = fixture();
        let request = request(&entry);
        let budget = ProcessBudget::new(768 << 20);
        let mut host = Host::new(selector(budget.clone()));
        let lease = host.request(&request).unwrap();
        drop(host);
        assert!(matches!(lease.wait(), Err(RequestError::Cancelled)));
        // A new host must be able to use the same process allowance after the
        // previous host and its final lease released their graph owners.
        let mut restarted = Host::new(selector(budget));
        let completed = restarted.request(&request).unwrap().wait().unwrap();
        assert!(checked(&completed).diagnostics.is_empty());
    });
}

#[test]
fn real_compiler_requests_refuse_a_zero_process_allowance() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, _source) = fixture();
        let request = request(&entry);
        let mut host = Host::new(selector(ProcessBudget::new(0)));
        assert!(host.request(&request).is_err());
    });
}

#[test]
fn physical_inspection_counts_survive_revisions_eviction_and_detached_snapshot_owners() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        let original = request(&entry);
        let budget = ProcessBudget::new(600 << 20);
        let mut host = Host::new(selector(budget.clone()));
        let mut held = None;
        let mut previous = 0;
        for revision in 0..4 {
            let text: Arc<[u8]> = format!("pub fn main()->u8{{{}u8}}\n", revision + 1)
                .into_bytes()
                .into();
            let mut selected = original.clone();
            selected.inputs = original
                .inputs
                .replace_sources(&BTreeMap::from([(source.clone(), text)]), 1 << 20)
                .unwrap();
            let completed = host.request(&selected).unwrap().wait().unwrap();
            assert!(checked(&completed).diagnostics.is_empty());
            let definition = completed.entry.clone();
            drop(completed);
            selected.operation = CompileOperation::Inspect {
                definitions: vec![definition.clone()],
            };
            let inspected = host.request(&selected).unwrap().wait().unwrap();
            assert_eq!(
                inspected.work.recomputations, 0,
                "inspection must execute no compiler phase"
            );
            assert_eq!(
                inspected.work.store_adoptions, 0,
                "inspection must restore no compiler product"
            );
            let CompileResult::Inspect(inspection) = &inspected.result else {
                panic!("wrong inspection result")
            };
            let count = inspection.definitions[0]
                .recomputations
                .iter()
                .find(|(kind, _)| *kind == hirsel_engine::QueryKind::Check)
                .unwrap()
                .1;
            assert!(
                count > previous,
                "each changed body contributes its actual new Check work"
            );
            assert!(
                inspection
                    .memos
                    .iter()
                    .any(|(key, _)| key.kind == hirsel_engine::QueryKind::Check)
            );
            if revision == 0 {
                held = Some(inspection.clone());
            }
            selected.operation = CompileOperation::RecomputeCounts {
                queries: vec![(definition, hirsel_engine::QueryKind::Check)],
            };
            let repeated = host.request(&selected).unwrap().wait().unwrap();
            assert_eq!(repeated.work.recomputations, 0);
            let CompileResult::RecomputeCounts(counts) = &repeated.result else {
                panic!("wrong counter result")
            };
            assert_eq!(
                counts[0].2, count,
                "metadata reads must not count a producer twice"
            );
            previous = count;
        }
        drop(host);
        assert!(
            budget.reserved() > 0,
            "detached inspection retains its admitted read inventory"
        );
        drop(held);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn production_host_policy_admits_a_small_real_check_with_its_existing_cap() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, _source) = fixture();
        let request = request(&entry);
        let mut host = Host::new(CompilerRequests::for_host(crate::COMPILER, config()));
        let output = host.request(&request).unwrap().wait().unwrap();
        assert!(checked(&output).diagnostics.is_empty());
        assert_eq!(
            checked(&output).completion,
            hirsel_engine::CheckCompletion::Complete
        );
    });
}

fn run_selector(budget: Arc<ProcessBudget>) -> CompilerRequests {
    CompilerRequests::new(
        crate::COMPILER,
        config(),
        budget,
        RequestLimits {
            runtime_bytes: 256 << 20,
            snapshot_bytes: 64 << 20,
            transport_bytes: 32 << 20,
        },
    )
}

#[test]
fn selected_safe_check_preserves_rejection_and_library_checks_without_run() {
    hirsel_engine::with_compiler_stack(|| {
        for library in [false, true] {
            let (_directory, entry, source) = fixture();
            if library {
                std::fs::write(&entry, r#"{"schema":1,"package":{"name":"request","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[]}"#).unwrap();
            } else {
                std::fs::write(&source, "pub fn main()->u8{true}\n").unwrap();
            }
            let mut selected = request(&entry);
            selected.operation = CompileOperation::SafeCheck;
            let budget = ProcessBudget::new(768 << 20);
            let mut host = Host::new(run_selector(budget.clone()));
            let output = host.request(&selected).unwrap().wait().unwrap();
            let CompileResult::SafeCheck { checked, reach } = &output.result else {
                panic!("safe check returned another operation");
            };
            assert!(reach.is_none());
            assert_eq!(checked.diagnostics.is_empty(), library);
            let mut inspect = selected.clone();
            inspect.operation = CompileOperation::RecomputeCounts {
                queries: vec![(output.entry.clone(), hirsel_engine::QueryKind::Lower)],
            };
            let counters = host.request(&inspect).unwrap().wait().unwrap();
            let CompileResult::RecomputeCounts(counts) = &counters.result else {
                panic!("inspection returned another operation");
            };
            assert_eq!(
                counts[0].2, 0,
                "rejected and library checks never execute the Run closure"
            );
            drop(counters);
            drop(output);
            drop(host);
            assert_eq!(budget.reserved(), 0);
        }
    });
}

#[test]
fn selected_safe_check_reports_the_settled_entry_escape() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, _source) = fixture();
        let mut selected = request(&entry);
        selected.operation = CompileOperation::SafeCheck;
        let mut host = Host::new(run_selector(ProcessBudget::new(768 << 20)));
        let output = host.request(&selected).unwrap().wait().unwrap();
        let CompileResult::SafeCheck { checked, reach } = &output.result else {
            panic!("safe check returned another operation");
        };
        assert!(checked.diagnostics.is_empty());
        assert_eq!(
            reach
                .as_ref()
                .map(|(entry, escape)| (entry, escape.is_none())),
            Some((&output.entry, true))
        );
    });
}
