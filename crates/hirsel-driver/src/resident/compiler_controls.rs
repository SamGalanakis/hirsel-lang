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
fn checked_product_requests_record_private_layout_reads_as_the_cold_receipt_does() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        // std@1/adler32's in-place update of its opaque state.
        std::fs::write(
            &source,
            "pub opaque type State: plain + inert uses {} {\n  a: u32,\n  b: u32,\n}\n\
             const MOD: u32 = 65521\nconst NMAX: u64 = 5552\n\
             @noalloc\npub fn new() -> State uses {} {\n  State(a: 1, b: 0)\n}\n\
             @noalloc\npub fn update(mut state: State, data: Bytes) -> () uses {} {\n\
             let len = core@1/bytes.len(data)\nlet mut a = state.a\nlet mut b = state.b\nlet mut i: u64 = 0\n\
             while i < len {\nlet mut end = i + NMAX\nif end > len {\nend = len\n}\n\
             while i < end {\nlet byte: u32 = core@1/int.widen(data[i])\na += byte\nb += a\ni += 1\n}\n\
             a %= MOD\nb %= MOD\n}\nstate.a = a\nstate.b = b\n}\n\
             pub fn main()->u64{1}\n",
        )
        .unwrap();
        let mut selected = request(&entry);
        selected.operation = CompileOperation::CheckedProducts {
            lowering: LoweringPolicy::Comptime,
            canonical: true,
        };
        let mut host = Host::new(CompilerRequests::for_host(crate::COMPILER, config()));
        let output = host.request(&selected).unwrap().wait().unwrap();
        let CompileResult::CheckedProducts { products, .. } = &output.result else {
            panic!("checked-products root returned another operation")
        };
        let mut cold = crate::COMPILER
            .with_config(config())
            .with_owned_engines()
            .load_program(&entry)
            .unwrap();
        cold.check(LoweringPolicy::Comptime).unwrap();
        let expected = cold.engine.checked_products(&cold.entry).unwrap();
        assert_eq!(products.canonical, expected.canonical);
        assert_eq!(products.canonical_reads(), expected.canonical_reads());
        assert_eq!(products.digest, expected.digest);
    });
}

#[test]
fn checked_product_requests_match_the_cold_receipt_with_and_without_text() {
    hirsel_engine::with_compiler_stack(|| {
        for source in [ORIGINAL, "const VALUE:u8=7u8\npub fn main()->u8{VALUE}\n"] {
            let (_directory, entry, source_path) = fixture();
            std::fs::write(source_path, source).unwrap();
            let mut full = request(&entry);
            full.operation = CompileOperation::CheckedProducts {
                lowering: LoweringPolicy::Comptime,
                canonical: true,
            };
            let mut digest = full.clone();
            digest.operation = CompileOperation::CheckedProducts {
                lowering: LoweringPolicy::Comptime,
                canonical: false,
            };
            let products = |output: Arc<crate::requests::CompileOutput>| match &output.result {
                CompileResult::CheckedProducts { products, .. } => products.clone(),
                _ => panic!("checked-products root returned another operation"),
            };
            let mut host = Host::new(CompilerRequests::for_host(crate::COMPILER, config()));
            let full = products(host.request(&full).unwrap().wait().unwrap());
            let digest = products(host.request(&digest).unwrap().wait().unwrap());
            let mut cold = crate::COMPILER
                .with_config(config())
                .with_owned_engines()
                .load_program(&entry)
                .unwrap();
            cold.check(LoweringPolicy::Comptime).unwrap();
            let expected = cold.engine.checked_products(&cold.entry).unwrap();
            assert_eq!(full.digest, expected.digest);
            assert_eq!(full.canonical, expected.canonical);
            assert_eq!(full.canonical_reads(), expected.canonical_reads());
            assert_eq!(digest.digest, expected.digest);
            assert_eq!(digest.canonical, None);
            assert_eq!(digest.canonical_reads(), expected.canonical_reads());
        }
    });
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
fn selected_run_executes_tir_and_checked_after_host_drop() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        std::fs::write(&source, "pub fn main()->Result[(),()]{Ok(())}\n").unwrap();
        let mut selected = request(&entry);
        selected.operation = CompileOperation::Tir { definition: None };
        let budget = ProcessBudget::new(768 << 20);
        let mut host = Host::new(run_selector(budget.clone()));
        let output = host.request(&selected).unwrap().wait().unwrap();
        let CompileResult::Tir(input) = &output.result else {
            panic!("run returned another operation")
        };
        let input = input.clone();
        drop(output);
        drop(host);
        assert!(
            budget.reserved() > 0,
            "execution retains its admitted source products"
        );
        let mut prior = None;
        for checked in [false, true] {
            let launch = crate::HostLaunch::empty();
            let mut observed = None;
            let (outcome, callee) = crate::tir_execution::run_tir_on_host(
                &input,
                launch,
                crate::launch_binding::LaunchSelection::read(launch).unwrap(),
                Some(10000),
                Some(&mut observed),
                None,
                checked,
            )
            .unwrap();
            assert_eq!(
                outcome,
                crate::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            );
            assert!(callee.is_none());
            let observed = observed.expect("settled execution has an observation");
            if let Some(prior) = prior {
                assert_eq!(prior, observed);
            }
            prior = Some(observed);
        }
        drop(input);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn selected_run_callee_identity_and_cancellation_keep_the_shared_check_live() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        std::fs::write(
            &source,
            "fn callee()->Result[(),()]{Ok(())}\npub fn main()->Result[(),()]{Ok(())}\n",
        )
        .unwrap();
        let mut selected = request(&entry);
        selected.operation = CompileOperation::Tir { definition: None };
        let metadata = crate::COMPILER
            .with_config(config())
            .load_check_package_inputs(&entry, &selected.inputs, false)
            .unwrap();
        let mut callee = metadata.entry.clone();
        callee.0.name = "callee".into();
        drop(metadata);
        let mut host = Host::new(run_selector(ProcessBudget::new(768 << 20)));
        let pending_main = host.request(&selected).unwrap();
        selected.operation = CompileOperation::Tir {
            definition: Some(callee.clone()),
        };
        let selected_callee = host.request(&selected).unwrap();
        assert!(Arc::ptr_eq(
            &pending_main.subscription.runtime,
            &selected_callee.subscription.runtime
        ));
        drop(pending_main);
        let output = selected_callee.wait().unwrap();
        assert_eq!(output.entry, callee);
        let CompileResult::Tir(input) = &output.result else {
            panic!("callee returned another operation")
        };
        assert_eq!(input.entry, callee);
        assert_eq!(input.definition.path.path(), &callee);
    });
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

#[test]
fn selected_private_layout_cache_matches_cold_and_preserves_unread_reader_work() {
    hirsel_engine::with_compiler_stack(|| {
        for (opaque, reads_field) in [(true, true), (true, false), (false, true)] {
            let (_directory, entry, source) = fixture();
            let visibility = if opaque { "opaque " } else { "" };
            let body = if reads_field { "x.level" } else { "1" };
            let parameter = if reads_field { "x:Secret" } else { "" };
            let initial = format!(
                "pub {visibility}type Secret: data uses {{}} {{ level:u64, }}\nfn view({parameter})->u64{{{body}}}\npub fn main()->u64{{1}}\n"
            );
            std::fs::write(&source, &initial).unwrap();
            let selected = request(&entry);
            let budget = ProcessBudget::new(768 << 20);
            let mut configured = config();
            configured.store = Arc::new(hirsel_engine::store::MemStore::default());
            let make = |configured: CompileConfig| {
                CompilerRequests::new(
                    crate::COMPILER,
                    configured,
                    budget.clone(),
                    RequestLimits {
                        runtime_bytes: 192 << 20,
                        snapshot_bytes: 32 << 20,
                        transport_bytes: 32 << 20,
                    },
                )
            };
            let mut host = Host::new(make(configured));
            let first = host.request(&selected).unwrap().wait().unwrap();
            assert!(checked(&first).diagnostics.is_empty());
            let mut view = first.entry.clone();
            view.0.name = "view".into();
            drop(first);
            let mut counter_request = selected.clone();
            counter_request.operation = CompileOperation::RecomputeCounts {
                queries: [
                    hirsel_engine::QueryKind::Resolve,
                    hirsel_engine::QueryKind::Check,
                ]
                .map(|kind| (view.clone(), kind))
                .to_vec(),
            };
            let before = host.request(&counter_request).unwrap().wait().unwrap();
            let CompileResult::RecomputeCounts(before) = &before.result else {
                panic!("wrong counter result")
            };
            let before = before.clone();
            let mut edited = selected.clone();
            edited.inputs = selected
                .inputs
                .replace_sources(
                    &BTreeMap::from([(
                        source.clone(),
                        initial
                            .replace("level:u64", "level:u32")
                            .into_bytes()
                            .into(),
                    )]),
                    1 << 20,
                )
                .unwrap();
            let actual = host.request(&edited).unwrap().wait().unwrap();
            counter_request.inputs = edited.inputs.clone();
            let after = host.request(&counter_request).unwrap().wait().unwrap();
            let CompileResult::RecomputeCounts(after) = &after.result else {
                panic!("wrong counter result")
            };
            if !reads_field {
                assert_eq!(
                    after, &before,
                    "unread opaque representation preserves actual reader P work"
                );
                assert!(
                    actual.work.store_adoptions > 0,
                    "the immutable graph actually restored source products"
                );
            } else {
                assert!(
                    after[1].2 > before[1].2,
                    "private layout reads must invalidate the cached Check"
                );
            }
            let mut cold = Host::new(make(config()));
            let expected = cold.request(&edited).unwrap().wait().unwrap();
            let normalized = |output: &crate::requests::CompileOutput| {
                checked(output)
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.to_json(&checked(output).sources))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                normalized(&actual),
                normalized(&expected),
                "opaque={opaque},reads_field={reads_field}"
            );
            assert_eq!(checked(&actual).completion, checked(&expected).completion);
        }
    });
}

#[test]
fn unread_representation_edit_adopts_reader_bodies_and_matches_the_cold_receipt() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        let initial = "pub opaque type Secret: data uses {} { level:u64, }\nfn view()->u64{1}\npub fn main()->u64{1}\n";
        std::fs::write(&source, initial).unwrap();
        let selected = request(&entry);
        let mut host = Host::new(CompilerRequests::for_host(crate::COMPILER, config()));
        let first = host.request(&selected).unwrap().wait().unwrap();
        let mut view = first.entry.clone();
        view.0.name = "view".into();
        drop(first);
        let mut receipt = selected.clone();
        receipt.operation = CompileOperation::CheckedProducts {
            lowering: LoweringPolicy::Comptime,
            canonical: true,
        };
        let mut counter = selected.clone();
        counter.operation = CompileOperation::RecomputeCounts {
            queries: [
                hirsel_engine::QueryKind::Resolve,
                hirsel_engine::QueryKind::Check,
            ]
            .map(|kind| (view.clone(), kind))
            .to_vec(),
        };
        let counts = |host: &mut Host<CompilerRequests>, request: &CompileRequest| {
            let output = host.request(request).unwrap().wait().unwrap();
            match &output.result {
                CompileResult::RecomputeCounts(counts) => counts.clone(),
                _ => panic!("counter request returned another operation"),
            }
        };
        drop(host.request(&receipt).unwrap().wait().unwrap());
        let before = counts(&mut host, &counter);
        assert!(before.iter().all(|(_, _, count)| *count == 1), "{before:?}");
        let edited = initial.replace("level:u64", "level:u32");
        std::fs::write(&source, &edited).unwrap();
        let inputs = selected
            .inputs
            .replace_sources(
                &BTreeMap::from([(source.clone(), edited.into_bytes().into())]),
                1 << 20,
            )
            .unwrap();
        for request in [&mut receipt, &mut counter] {
            request.inputs = inputs.clone();
        }
        let mut checked = selected.clone();
        checked.inputs = inputs;
        drop(host.request(&checked).unwrap().wait().unwrap());
        let products = host.request(&receipt).unwrap().wait().unwrap();
        let CompileResult::CheckedProducts { products, .. } = &products.result else {
            panic!("checked-products root returned another operation")
        };
        let products = products.clone();
        assert_eq!(
            counts(&mut host, &counter),
            before,
            "a view that does not read the representation keeps its P work"
        );
        let mut cold = crate::COMPILER
            .with_config(config())
            .with_owned_engines()
            .load_program(&entry)
            .unwrap();
        cold.check(LoweringPolicy::Comptime).unwrap();
        let expected = cold.engine.checked_products(&cold.entry).unwrap();
        assert_eq!(products.canonical, expected.canonical);
        assert_eq!(products.canonical_reads(), expected.canonical_reads());
        assert_eq!(products.digest, expected.digest);
    });
}

#[test]
fn selected_run_bounds_lower_mailboxes_by_the_immutable_query_inventory() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        std::fs::write(
            &source,
            "fn a()->u8{1u8}\nfn b()->u8{2u8}\npub fn main()->Result[(),()]{Ok(())}\n",
        )
        .unwrap();
        let mut selected = request(&entry);
        selected.operation = CompileOperation::Tir { definition: None };
        let budget = ProcessBudget::new(768 << 20);
        let mut host = Host::new(run_selector(budget.clone()));
        let output = host.request(&selected).unwrap().wait().unwrap();
        let CompileResult::Tir(input) = &output.result else {
            panic!("run returned another operation");
        };
        assert_eq!(input.entry, output.entry);
        assert_eq!(input.definition.path.path(), &output.entry);
        drop(output);
        drop(host);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn selected_footprint_planner_admits_many_small_definitions_under_the_same_snapshot_bound() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        let mut text = String::new();
        for index in 0..256 {
            text.push_str(&format!("fn f{index}()->u8{{1u8}}\n"));
        }
        text.push_str("pub fn main()->u8{1u8}\n");
        std::fs::write(&source, text).unwrap();
        let selected = request(&entry);
        let budget = ProcessBudget::new(512 << 20);
        let mut host = Host::new(CompilerRequests::new(
            crate::COMPILER,
            config(),
            budget.clone(),
            RequestLimits {
                runtime_bytes: 192 << 20,
                snapshot_bytes: 4 << 20,
                transport_bytes: 16 << 20,
            },
        ));
        let output = host.request(&selected).unwrap().wait().unwrap();
        assert!(checked(&output).diagnostics.is_empty());
        assert_eq!(
            checked(&output).completion,
            hirsel_engine::CheckCompletion::Complete
        );
        assert!(output.work.recomputations > 0);
        drop(host);
        assert!(
            budget.reserved() > 0,
            "held check sources retain their exact admitted source owner"
        );
        drop(output);
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn source_transport_scratch_keeps_the_selected_provider_and_lives_through_detached_source_files() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, source) = fixture();
        let selected = request(&entry);
        let parent = tempfile::tempdir().unwrap();
        let budget = ProcessBudget::new(512 << 20);
        let mut host =
            Host::new(selector(budget.clone()).with_scratch_parent(parent.path().to_owned()));
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
        let output = host.request(&selected).unwrap().wait().unwrap();
        assert!(checked(&output).diagnostics.is_empty());
        assert_eq!(
            output.work.store_adoptions, 0,
            "none@1 must not claim a persistent adoption"
        );
        let held_file = checked(&output)
            .sources
            .anchors()
            .find_map(|(_, file, _)| {
                checked(&output)
                    .sources
                    .file(file)
                    .filter(|file| file.path == source)
                    .cloned()
            })
            .expect("checked source file");
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 1);
        drop(host);
        drop(output);
        assert_eq!(held_file.text(), ORIGINAL);
        assert_eq!(
            std::fs::read_dir(parent.path()).unwrap().count(),
            1,
            "an external source file retains its original graph transport"
        );
        assert!(budget.reserved() > 0);
        drop(held_file);
        assert_eq!(
            std::fs::read_dir(parent.path()).unwrap().count(),
            0,
            "last source owner removes its private request files"
        );
        assert_eq!(budget.reserved(), 0);
    });
}

#[test]
fn immutable_query_capacity_preserves_sixteen_real_coalesced_subscribers() {
    hirsel_engine::with_compiler_stack(|| {
        let (_directory, entry, _source) = fixture();
        let mut selected = request(&entry);
        let budget = ProcessBudget::new(512 << 20);
        let mut host = Host::new(selector(budget.clone()));
        let leases = (0..16)
            .map(|_| host.request(&selected).unwrap())
            .collect::<Vec<_>>();
        assert!(Arc::ptr_eq(
            &leases[0].subscription.runtime,
            &leases[15].subscription.runtime
        ));
        let outputs = leases
            .into_iter()
            .map(|lease| lease.wait().unwrap())
            .collect::<Vec<_>>();
        for output in &outputs {
            assert!(checked(output).diagnostics.is_empty());
        }
        selected.operation = CompileOperation::RecomputeCounts {
            queries: vec![(outputs[0].entry.clone(), hirsel_engine::QueryKind::Check)],
        };
        let counts = host.request(&selected).unwrap().wait().unwrap();
        let CompileResult::RecomputeCounts(counts) = &counts.result else {
            panic!("unexpected counter output");
        };
        assert_eq!(
            counts[0].2, 1,
            "sixteen subscriptions must share one real source check"
        );
        drop(outputs);
        drop(host);
    });
}
