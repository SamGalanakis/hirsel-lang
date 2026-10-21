use hirsel_driver::candidate::*;
use hirsel_engine::{CompileConfig, LoweringPolicy, Verify};
use hirsel_syntax::ContentHash;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
static IDS: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new(text: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "hirsel-candidate-txn-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("main.hir"), text).unwrap();
        Self(path)
    }
    fn entry(&self) -> PathBuf {
        self.0.join("main.hir")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn workspace() -> CandidateWorkspace {
    CandidateWorkspace::new(
        CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        },
        vec![],
        hirsel_interp::Limits::default(),
        Limits {
            retain_canonical_export: true,
            ..Limits::default()
        },
    )
    .unwrap()
}
fn submit(w: &CandidateWorkspace, base: &BaseLease, id: u64, text: &str) -> CandidateLease {
    let (key, old, digest) = base.sources().next().unwrap();
    w.submit(
        base,
        CandidateRequest {
            id: CandidateId {
                client: 1,
                nonce: id,
            },
            base: base.token().clone(),
            edits: vec![FileEdit {
                file: key.clone(),
                expected: digest,
                edits: vec![ByteEdit {
                    start: 0,
                    end: old.len() as u32,
                    replacement: text.into(),
                }],
            }],
            priority: Priority::Selected,
        },
    )
    .unwrap()
}
fn result(w: &CandidateWorkspace, c: &CandidateLease) -> Arc<CheckReceipt> {
    match w.result(c).unwrap() {
        CandidateResult::Ready(r) => r,
        CandidateResult::Pending => panic!("serial candidate unexpectedly pending"),
    }
}
#[test]
fn candidate_isolated_check_and_resident_promotion_do_not_write_sources() {
    let f = Fixture::new("pub fn main() -> u64 { 1 }\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let a = submit(&w, &base, 1, "pub fn main() -> u64 { 2 }\n");
    let b = submit(&w, &base, 2, "pub fn main() -> u64 { false }\n");
    let ra = result(&w, &a);
    let rb = result(&w, &b);
    assert_eq!(ra.completion, Completion::Complete, "{:?}", ra.loader_error);
    assert_eq!(ra.verdict, Verdict::Pass, "{:?}", ra.loader_error);
    assert_eq!(rb.verdict, Verdict::Reject);
    assert_eq!(
        ra.scope.policy,
        CheckPolicy {
            verify: Verify::On,
            lowering: LoweringPolicy::Comptime
        }
    );
    assert!(ra.checked_products.is_some());
    let promoted = w
        .promote(
            CandidateId {
                client: 7,
                nonce: 1,
            },
            &a,
            base.token(),
        )
        .unwrap();
    assert!(matches!(promoted, Promotion::Promoted(_)));
    assert_eq!(
        w.promote(
            CandidateId {
                client: 7,
                nonce: 1
            },
            &a,
            base.token()
        )
        .unwrap(),
        promoted
    );
    assert!(matches!(
        w.promote(
            CandidateId {
                client: 7,
                nonce: 2
            },
            &b,
            base.token()
        )
        .unwrap(),
        Promotion::Conflict { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(f.entry()).unwrap(),
        "pub fn main() -> u64 { 1 }\n"
    );
    assert_eq!(
        base.sources().next().unwrap().1,
        "pub fn main() -> u64 { 1 }\n"
    );
    assert!(!rb.output.diagnostics.is_empty());
    assert_eq!(w.counters().frozen_loads, 2);
    assert_eq!(w.counters().completed_results, 2);
}
#[test]
fn candidate_duplicate_waiters_cancel_independently_and_result_never_polls_disk() {
    let f = Fixture::new("pub fn main() -> u64 { 1 }\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let a = submit(&w, &base, 1, "pub fn main() -> u64 { 2 }\n");
    let b = submit(&w, &base, 2, "pub fn main() -> u64 { 2 }\n");
    w.cancel(&b).unwrap();
    std::fs::write(f.entry(), "broken").unwrap();
    std::fs::write(f.0.join("main.hir.json"), "{}").unwrap();
    assert_eq!(
        result(&w, &b).completion,
        Completion::Incomplete(IncompleteReason::Cancelled)
    );
    assert_eq!(result(&w, &a).verdict, Verdict::Pass);
    assert_eq!(w.counters().frozen_loads, 1);
    assert!(matches!(
        w.observe_disk(&a).unwrap(),
        DiskFreshness::Changed(_)
    ));
    assert_eq!(result(&w, &a).disk, DiskFreshness::Unobserved);
}
#[test]
fn candidate_digest_id_alias_and_resource_admission_are_atomic() {
    let f = Fixture::new("pub fn main() -> u64 { 1 }\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let (key, _, _) = base.sources().next().unwrap();
    let request = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 1,
        },
        base: base.token().clone(),
        edits: vec![FileEdit {
            file: key.clone(),
            expected: ContentHash::of_bytes(b"stale"),
            edits: vec![],
        }],
        priority: Priority::Selected,
    };
    assert!(matches!(
        w.submit(&base, request),
        Err(RequestError::StaleBytes)
    ));
    assert_eq!(w.head().unwrap(), *base.token());
    let a = submit(&w, &base, 1, "pub fn main() -> u64 { 2 }\n");
    let (key, old, digest) = base.sources().next().unwrap();
    let request = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 1,
        },
        base: base.token().clone(),
        edits: vec![FileEdit {
            file: key.clone(),
            expected: digest,
            edits: vec![ByteEdit {
                start: 0,
                end: old.len() as u32,
                replacement: "pub fn main() -> u64 { 3 }\n".into(),
            }],
        }],
        priority: Priority::Selected,
    };
    assert!(matches!(
        w.submit(&base, request),
        Err(RequestError::RequestIdConflict)
    ));
    assert_eq!(result(&w, &a).verdict, Verdict::Pass);
}

#[test]
fn candidate_promotion_barriers_retry_aba_and_old_diagnostics_survive_capture() {
    use std::sync::Barrier;
    let f = Fixture::new("pub fn main() -> u64 { 1 }\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let a = submit(&w, &base, 1, "pub fn main() -> u64 { 2 }\n");
    let b = submit(&w, &base, 2, "pub fn main() -> u64 { 3 }\n");
    let bad = submit(&w, &base, 3, "pub fn main() -> u64 { false }\n");
    assert_eq!(result(&w, &a).verdict, Verdict::Pass);
    assert_eq!(result(&w, &b).verdict, Verdict::Pass);
    let old = result(&w, &bad);
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = [(a.clone(), 1), (b.clone(), 2)]
        .into_iter()
        .map(|(candidate, nonce)| {
            let (w, barrier, token) = (w.clone(), barrier.clone(), base.token().clone());
            std::thread::spawn(move || {
                barrier.wait();
                w.promote(CandidateId { client: 77, nonce }, &candidate, &token)
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Promotion::Promoted(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Promotion::Conflict { .. }))
            .count(),
        1
    );
    let recaptured = w.capture(&f.entry()).unwrap();
    assert_eq!(recaptured.token().inputs, base.token().inputs);
    assert!(recaptured.token().generation > base.token().generation);
    assert!(matches!(
        w.promote(
            CandidateId {
                client: 78,
                nonce: 1
            },
            &a,
            base.token()
        )
        .unwrap(),
        Promotion::Conflict { .. }
    ));
    assert!(
        !old.output.diagnostics[0]
            .render_human(&old.output.sources)
            .is_empty()
    );
}

#[test]
fn candidate_manifest_syntax_failure_is_repairable_and_keeps_loader_category() {
    let f = Fixture::new("pub fn main( {\n");
    let manifest = f.0.join("hirsel.json");
    std::fs::write(&manifest,r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let w = workspace();
    let base = w.capture(&manifest).unwrap();
    let bad = submit(&w, &base, 1, "pub fn main( {\n");
    let bad = result(&w, &bad);
    assert_eq!(bad.verdict, Verdict::Reject);
    assert_eq!(bad.failure_class, Some(FailureClass::LoadQuery));
    let fixed = submit(&w, &base, 2, "pub fn main() -> u64 { 2 }\n");
    assert_eq!(result(&w, &fixed).verdict, Verdict::Pass);
    std::fs::write(&manifest, "{}").unwrap();
    assert!(matches!(w.capture(&manifest), Err(CaptureError::Load(_))));
}

#[test]
fn candidate_exhaustion_and_quota_never_become_success_or_cached_verdicts() {
    let f = Fixture::new(
        "fn helper() -> u64 { 1 + 2 }\nconst X:u64=comptime helper()\npub fn main()->u64{X}\n",
    );
    let config = CompileConfig {
        jobs: hirsel_intern::jobs::Jobs::new(1),
        ..CompileConfig::for_memory_cap(None)
    };
    let w = CandidateWorkspace::new(
        config.clone(),
        vec![],
        hirsel_interp::Limits {
            steps: Some(1),
            call_depth: 64,
        },
        Limits::default(),
    )
    .unwrap();
    let base = w.capture(&f.entry()).unwrap();
    let old = std::fs::read_to_string(f.entry()).unwrap();
    let a = submit(&w, &base, 1, &old);
    let actual = result(&w, &a);
    assert_eq!(
        actual.completion,
        Completion::Incomplete(IncompleteReason::ResourceLimit)
    );
    assert_eq!(actual.verdict, Verdict::Undecided);
    let b = submit(&w, &base, 2, &old);
    let _ = result(&w, &b);
    assert_eq!(
        w.counters().frozen_loads,
        2,
        "operational incomplete was cached"
    );
    let tiny = CandidateWorkspace::new(
        config.clone(),
        vec![],
        hirsel_interp::Limits::default(),
        Limits {
            bytes: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        tiny.capture(&f.entry()),
        Err(CaptureError::ResourceLimit)
    ));
    let limited = CandidateWorkspace::new(
        config,
        vec![],
        hirsel_interp::Limits::default(),
        Limits {
            leases: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let base = limited.capture(&f.entry()).unwrap();
    let a = submit(&limited, &base, 1, &old);
    let (key, text, digest) = base.sources().next().unwrap();
    let request = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 2,
        },
        base: base.token().clone(),
        edits: vec![FileEdit {
            file: key.clone(),
            expected: digest,
            edits: vec![ByteEdit {
                start: 0,
                end: text.len() as u32,
                replacement: old.clone(),
            }],
        }],
        priority: Priority::Selected,
    };
    assert!(matches!(
        limited.submit(&base, request),
        Err(RequestError::ResourceLimit)
    ));
    drop(a);
}

#[test]
fn candidate_identical_content_coalesces_but_alias_collisions_and_unsupported_edits_reject() {
    let f = Fixture::new("pub fn main() -> u64 { 1 }\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let (key, old, digest) = base.sources().next().unwrap();
    let a = submit(&w, &base, 1, "pub fn main() -> u64 { 2 }\n");
    let at = old.find("1").unwrap() as u32;
    let request = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 2,
        },
        base: base.token().clone(),
        edits: vec![FileEdit {
            file: key.clone(),
            expected: digest,
            edits: vec![ByteEdit {
                start: at,
                end: at + 1,
                replacement: "2".into(),
            }],
        }],
        priority: Priority::Background,
    };
    let b = w.submit(&base, request.clone()).unwrap();
    let retry = w.submit(&base, request).unwrap();
    assert_eq!(
        result(&w, &a).candidate_inputs,
        result(&w, &b).candidate_inputs
    );
    assert_eq!(result(&w, &retry).verdict, Verdict::Pass);
    assert_eq!(w.counters().frozen_loads, 1);
    let duplicate = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 3,
        },
        base: base.token().clone(),
        edits: vec![
            FileEdit {
                file: key.clone(),
                expected: digest,
                edits: vec![],
            },
            FileEdit {
                file: SourceKey(f.entry()),
                expected: digest,
                edits: vec![],
            },
        ],
        priority: Priority::Selected,
    };
    assert!(matches!(
        w.submit(&base, duplicate),
        Err(RequestError::DuplicateTarget)
    ));
    let unsupported = CandidateRequest {
        id: CandidateId {
            client: 1,
            nonce: 4,
        },
        base: base.token().clone(),
        edits: vec![FileEdit {
            file: SourceKey(f.0.join("main.hir.json")),
            expected: digest,
            edits: vec![],
        }],
        priority: Priority::Selected,
    };
    assert!(matches!(
        w.submit(&base, unsupported),
        Err(RequestError::UnsupportedEdit)
    ));
}

#[test]
fn candidate_borrow_siblings_keep_opposing_verdicts_and_retained_diagnostics() {
    let f = Fixture::new("fn identity[T](take v:T)->T{v}\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let good = submit(&w, &base, 1, "fn identity[T](take v:T)->T{v}\n");
    let bad = submit(&w, &base, 2, "fn identity[T](v:T)->T{v}\n");
    assert_eq!(result(&w, &good).verdict, Verdict::Pass);
    let rejected = result(&w, &bad);
    assert_eq!(rejected.verdict, Verdict::Reject);
    let rendered: Vec<_> = rejected
        .output
        .diagnostics
        .iter()
        .map(|d| d.render_human(&rejected.output.sources))
        .collect();
    w.promote(
        CandidateId {
            client: 33,
            nonce: 1,
        },
        &good,
        base.token(),
    )
    .unwrap();
    drop(good);
    drop(bad);
    drop(base);
    assert_eq!(
        rendered,
        rejected
            .output
            .diagnostics
            .iter()
            .map(|d| d.render_human(&rejected.output.sources))
            .collect::<Vec<_>>()
    );
}

#[path = "candidate_model.rs"]
mod model;

#[test]
fn candidate_public_history_agrees_with_independent_byte_map_model() {
    let original = "pub fn main() -> u64 { 1 }\n";
    let changed = "pub fn main() -> u64 { 2 }\n";
    let files = |text: &str| std::collections::BTreeMap::from([("main.hir".into(), text.into())]);
    let f = Fixture::new(original);
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let mut model = model::Model::new(base.token().workspace, files(original));
    let parent = model.head();
    let a = submit(&w, &base, 1, changed);
    let b = submit(&w, &base, 2, changed);
    assert!(model.submit(1, parent.clone(), files(changed)));
    assert!(model.submit(2, parent.clone(), files(changed)));
    w.cancel(&b).unwrap();
    model.cancel(2);
    assert_eq!(result(&w, &a).verdict, Verdict::Pass);
    model.publish(1, true);
    assert_eq!(model.result(1), Some(model::State::CompletePass));
    assert_eq!(model.result(2), Some(model::State::Cancelled));
    assert_eq!(
        result(&w, &b).completion,
        Completion::Incomplete(IncompleteReason::Cancelled)
    );
    let actual = w
        .promote(
            CandidateId {
                client: 1,
                nonce: 9,
            },
            &a,
            base.token(),
        )
        .unwrap();
    let expected = model.promote(9, 1, parent.clone());
    let (Promotion::Promoted(actual), model::Promotion::Promoted(expected)) = (actual, expected)
    else {
        panic!("promotion disagreement")
    };
    assert_eq!(actual.generation, expected.generation);
    let recaptured = w.capture(&f.entry()).unwrap();
    let recaptured_model = model.capture(files(original));
    assert_eq!(recaptured.token().generation, recaptured_model.generation);
    assert!(matches!(
        w.promote(
            CandidateId {
                client: 1,
                nonce: 10
            },
            &a,
            base.token()
        )
        .unwrap(),
        Promotion::Conflict { .. }
    ));
    assert!(matches!(
        model.promote(10, 1, parent),
        model::Promotion::Conflict(_)
    ));
}
/// Exercise the real eligible-store loader, including a producer with V off.
/// Malformed records must miss; swapping complete sibling records must not
/// substitute a verdict or a source map for a different input key.
#[test]
fn candidate_store_adoption_and_faults_equal_cache_free_checks() {
    use hirsel_engine::store::{EntryKey, MemStore, ResultStore};
    use std::sync::{Mutex, atomic::AtomicUsize};
    #[derive(Default)]
    struct FaultStore {
        backing: MemStore,
        mode: AtomicUsize,
        records: Mutex<Vec<Arc<[u8]>>>,
    }
    impl ResultStore for FaultStore {
        fn get(&self, key: &EntryKey) -> Option<Arc<[u8]>> {
            let bytes = self.backing.get(key)?;
            match self.mode.load(Ordering::Acquire) {
                1 => Some(Arc::from(&b"corrupt candidate store frame"[..])),
                2 => self
                    .records
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|other| other.as_ref() != bytes.as_ref())
                    .cloned()
                    .or(Some(bytes)),
                _ => Some(bytes),
            }
        }
        fn put(&self, key: &EntryKey, bytes: &[u8]) {
            self.backing.put(key, bytes);
            self.records.lock().unwrap().push(Arc::from(bytes));
        }
        fn name(&self) -> &'static str {
            "candidate-test-fault-store"
        }
    }
    let text = "fn helper()->u8{1u8}\nconst VALUE:u8=comptime helper()\npub fn main()->u8{VALUE}\n";
    let f = Fixture::new(text);
    let store = Arc::new(FaultStore::default());
    let config = CompileConfig {
        jobs: hirsel_intern::jobs::Jobs::new(1),
        store: store.clone(),
        ..CompileConfig::for_memory_cap(None)
    };
    hirsel_engine::with_compiler_stack(|| {
        let compiler = hirsel_driver::COMPILER
            .clone()
            .with_config(CompileConfig {
                verify: Verify::Off,
                ..config.clone()
            })
            .with_owned_engines();
        let mut program = compiler
            .load_program_source_with_images(&f.entry(), None, &hirsel_engine::StdImages::none())
            .unwrap();
        assert!(
            program
                .check(LoweringPolicy::Comptime)
                .unwrap()
                .diagnostics
                .is_empty()
        );
    });
    assert!(!store.backing.is_empty());
    let cold = workspace();
    let cold_base = cold.capture(&f.entry()).unwrap();
    let expected = result(&cold, &submit(&cold, &cold_base, 1, text));
    for fault in [0, 1, 2, 0] {
        store.mode.store(fault, Ordering::Release);
        let w = CandidateWorkspace::new(
            config.clone(),
            vec![],
            hirsel_interp::Limits::default(),
            Limits {
                retain_canonical_export: true,
                ..Limits::default()
            },
        )
        .unwrap();
        let base = w.capture(&f.entry()).unwrap();
        let candidate = submit(&w, &base, 1, text);
        let actual = result(&w, &candidate);
        if fault == 2 && actual.completion == Completion::Incomplete(IncompleteReason::ToolFailure)
        {
            assert_eq!(actual.verdict, Verdict::Undecided);
            assert!(actual.checked_products.is_none());
            assert_eq!(
                w.promote(
                    CandidateId {
                        client: 2,
                        nonce: 1
                    },
                    &candidate,
                    base.token()
                )
                .unwrap(),
                Promotion::Ineligible
            );
            continue; // injected wrong-key payload is explicitly refused, never accepted
        }
        assert_eq!(
            actual.verdict, expected.verdict,
            "fault{fault}: {:?}",
            actual.loader_error
        );
        assert_eq!(actual.completion, expected.completion);
        let actual = actual.checked_products.as_ref().unwrap();
        let expected = expected.checked_products.as_ref().unwrap();
        assert_eq!(actual.canonical, expected.canonical, "fault{fault}");
        assert_eq!(actual.digest, expected.digest, "fault{fault}");
        assert_eq!(actual.reads, expected.reads, "fault{fault}");
        if fault == 0 {
            assert!(
                w.counters().store_adoptions > 0,
                "eligible store was not used"
            );
        }
        assert_eq!(w.counters().dropped_engines, 1);
    }
}
#[test]
fn candidate_eight_results_release_unused_reserves_and_old_observations_have_backpressure() {
    let f = Fixture::new("pub fn main()->u64{0}\n");
    let w = workspace();
    let base = w.capture(&f.entry()).unwrap();
    let candidates: Vec<_> = (1..=8)
        .map(|i| submit(&w, &base, i, &format!("pub fn main()->u64{{{i}}}\n")))
        .collect();
    let receipts: Vec<_> = candidates.iter().map(|c| result(&w, c)).collect();
    assert!(receipts.iter().all(|r| r.verdict == Verdict::Pass));
    assert_eq!(w.counters().completed_results, 8);
    assert!(
        w.counters().retained_bytes < 8 * Limits::default().result_bytes / 2,
        "small results retained construction reservations"
    );
    let retained = w.counters().retained_bytes;
    let observation = result(&w, &candidates[0]);
    assert!(w.counters().retained_bytes > retained);
    drop(observation);
    assert_eq!(w.counters().retained_bytes, retained);
    let old = receipts[0]
        .checked_products
        .as_ref()
        .unwrap()
        .canonical
        .clone();
    w.promote(
        CandidateId {
            client: 9,
            nonce: 1,
        },
        &candidates[0],
        base.token(),
    )
    .unwrap();
    drop(candidates);
    drop(base);
    drop(receipts);
    let selected = w.head_lease().unwrap();
    let restored = submit(&w, &selected, 100, "pub fn main()->u64{1}\n");
    assert_eq!(
        result(&w, &restored)
            .checked_products
            .as_ref()
            .unwrap()
            .canonical,
        old
    );
    assert_eq!(w.counters().dropped_engines, w.counters().frozen_loads);
}
#[test]
fn candidate_immutable_image_selection_keeps_fallback_frozen_and_rejects_image_source_edits() {
    hirsel_engine::with_compiler_stack(|| {
        let f = Fixture::new("pub fn main()->u64{lib.value(1)}\n");
        let library = f.0.join("lib.hir");
        std::fs::write(&library, "pub fn value(x:u64)->u64{x+1}\n").unwrap();
        let lib_manifest = f.0.join("lib.json");
        std::fs::write(&lib_manifest,r#"{"schema":1,"package":{"name":"lib","version":"1"},"sources":[{"path":"lib.hir","module":["lib"]}],"dependencies":[]}"#).unwrap();
        let entry = f.0.join("hirsel.json");
        std::fs::write(&entry,serde_json::json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":"lib.json","package":{"name":"lib","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
        let image_path = f.0.join("lib.image");
        hirsel_driver::COMPILER
            .image_package(&lib_manifest, &image_path)
            .unwrap();
        let image = hirsel_engine::StdImage::open(&image_path).unwrap();
        let original_image = std::fs::read(&image_path).unwrap();
        let config = CompileConfig::for_memory_cap(None);
        let w = CandidateWorkspace::new(
            config.clone(),
            vec![image.clone()],
            hirsel_interp::Limits::default(),
            Limits::default(),
        )
        .unwrap();
        let base = w.capture(&entry).unwrap();
        let (key, text, expected) = base
            .sources()
            .find(|(key, _, _)| base.source_path(key) == Some(library.as_path()))
            .unwrap();
        assert!(matches!(
            w.submit(
                &base,
                CandidateRequest {
                    id: CandidateId {
                        client: 1,
                        nonce: 1
                    },
                    base: base.token().clone(),
                    priority: Priority::Selected,
                    edits: vec![FileEdit {
                        file: key.clone(),
                        expected,
                        edits: vec![ByteEdit {
                            start: 0,
                            end: text.len() as u32,
                            replacement: text.replace("x+1", "x+2")
                        }]
                    }]
                }
            ),
            Err(RequestError::UnsupportedEdit)
        ));
        let candidate = w
            .submit(
                &base,
                CandidateRequest {
                    id: CandidateId {
                        client: 1,
                        nonce: 2,
                    },
                    base: base.token().clone(),
                    priority: Priority::Selected,
                    edits: vec![],
                },
            )
            .unwrap();
        let actual = result(&w, &candidate);
        assert_eq!(actual.verdict, Verdict::Pass, "{:?}", actual.loader_error);
        let compiler = hirsel_driver::COMPILER
            .clone()
            .with_config(config.clone())
            .with_owned_engines();
        let images = hirsel_engine::StdImages::of(vec![image]);
        let mut cold = compiler
            .load_program_source_with_images(&entry, None, &images)
            .unwrap();
        assert!(cold.engine.imaged(&hirsel_syntax::PackageId {
            name: "lib".into(),
            version: "1".into()
        }));
        assert!(
            cold.check(LoweringPolicy::Comptime)
                .unwrap()
                .diagnostics
                .is_empty()
        );
        let expected = cold.engine.checked_products(&cold.entry).unwrap();
        assert_eq!(
            actual.checked_products.as_ref().unwrap().digest,
            expected.digest
        );
        // Recapture changed library source with the old immutable image: it
        // falls back to captured source and does not rebuild the image file.
        std::fs::write(&library, "pub fn value(x:u64)->u64{x+2}\n").unwrap();
        let next = w.capture(&entry).unwrap();
        let candidate = w
            .submit(
                &next,
                CandidateRequest {
                    id: CandidateId {
                        client: 1,
                        nonce: 3,
                    },
                    base: next.token().clone(),
                    priority: Priority::Selected,
                    edits: vec![],
                },
            )
            .unwrap();
        std::fs::write(&library, "pub fn value(x:u64)->u64{false}\n").unwrap();
        assert_eq!(result(&w, &candidate).verdict, Verdict::Pass);
        assert_eq!(std::fs::read(&image_path).unwrap(), original_image);
    });
}

#[test]
fn candidate_digest_only_receipt_commits_to_the_same_optional_full_export() {
    let f =
        Fixture::new("fn helper()->u8{1u8}\nconst N:u8=comptime helper()\npub fn main()->u8{N}\n");
    let detailed = workspace();
    let base = detailed.capture(&f.entry()).unwrap();
    let text = std::fs::read_to_string(f.entry()).unwrap();
    let full = result(&detailed, &submit(&detailed, &base, 1, &text));
    assert!(full.checked_products.as_ref().unwrap().canonical.is_some());
    let compact = CandidateWorkspace::new(
        CompileConfig::for_memory_cap(None),
        vec![],
        hirsel_interp::Limits::default(),
        Limits::default(),
    )
    .unwrap();
    let base = compact.capture(&f.entry()).unwrap();
    let minimal = result(&compact, &submit(&compact, &base, 1, &text));
    let a = full.checked_products.as_ref().unwrap();
    let b = minimal.checked_products.as_ref().unwrap();
    assert!(b.canonical.is_none());
    assert_eq!(a.digest, b.digest);
    assert_eq!(a.reads, b.reads);
    assert_eq!(full.verdict, minimal.verdict);
    assert_eq!(full.completion, minimal.completion);
    assert!(compact.counters().retained_bytes < detailed.counters().retained_bytes);
}
