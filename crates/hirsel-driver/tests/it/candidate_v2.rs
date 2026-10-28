//! H1-H8/A01-A20 public-operation controls. Cold helpers use a separate
//! process and ordinary DiskInputs/NoneStore, never namespace lookup.
use hirsel_driver::candidate::v2::*;
use hirsel_driver::candidate::*;
use hirsel_engine::loader_inputs::namespace::{Limits as NL, Root};
use hirsel_engine::{CompileConfig, LoweringPolicy, ToolError};
use hirsel_syntax::ContentHash;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
const MAIN: &str = "pub fn main()->u64{1}\n";
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "candidate-v2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(p.join("mods")).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, text: &str) {
        std::fs::write(self.0.join(path), text).unwrap();
    }
    fn basic() -> Self {
        let t = Self::new();
        t.write("main.hir", MAIN);
        t.write(
            "hirsel.json",
            &manifest("app", "1", &[("main.hir", &[])], vec![]),
        );
        t
    }
    fn plan(&self) -> CapturePlan {
        CapturePlan {
            roots: vec![Root {
                id: "app".into(),
                path: self.0.clone(),
                editable: true,
            }],
            excluded: BTreeSet::new(),
            failed_probes: vec![],
            entry: self.0.join("hirsel.json"),
            std_manifest: None,
            environment: vec![],
            limits: NL {
                bytes: 16 << 20,
                nodes: 4096,
                aliases: 128,
                depth: 64,
                link_hops: 40,
                edits: 128,
                attempts: 3,
            },
        }
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn config(jobs: usize) -> CompileConfig {
    CompileConfig {
        jobs: hirsel_intern::jobs::Jobs::new(jobs),
        ..CompileConfig::for_memory_cap(None)
    }
}
fn workspace(jobs: usize) -> CandidateWorkspace {
    CandidateWorkspace::new(
        config(jobs),
        Default::default(),
        Limits {
            retain_canonical_export: true,
            ..Limits::default()
        },
    )
    .unwrap()
}
fn manifest(name: &str, version: &str, sources: &[(&str, &[&str])], deps: Vec<Value>) -> String {
    json!({"schema":1,"package":{"name":name,"version":version},"sources":sources.iter().map(|(p,m)|json!({"path":p,"module":m})).collect::<Vec<_>>(),"dependencies":deps,"entry":{"module":[],"name":"main"}}).to_string()
}
fn dependency(path: &str, name: &str, version: &str) -> Value {
    json!({"manifest":path,"package":{"name":name,"version":version},"expose":[]})
}
fn expected(w: &CandidateWorkspace, b: &BaseLease, p: &str) -> Value {
    match w.inspect_v2(b, "app", p).unwrap() {
        Inspection::Present {
            present,
            bytes: Some(bytes),
            ..
        } => json!({"present":hex(present),"bytes":hex(bytes)}),
        Inspection::Absent { absent, .. } => json!({"absent":hex(absent)}),
        other => panic!("{p}: {other:?}"),
    }
}
fn hex(h: ContentHash) -> String {
    h.0.iter().map(|b| format!("{b:02x}")).collect()
}
fn edit(w: &CandidateWorkspace, b: &BaseLease, kind: &str, p: &str, text: Option<&str>) -> Value {
    let mut v = json!({"kind":kind,"path":{"root":"app","path":p},"expected":expected(w,b,p)});
    if let Some(text) = text {
        v["text"] = json!(text);
    }
    v
}
fn manifest_edit(
    w: &CandidateWorkspace,
    b: &BaseLease,
    action: &str,
    p: &str,
    text: Option<&str>,
) -> Value {
    let mut v = edit(w, b, "manifest", p, text);
    v["action"] = json!(action);
    v
}
fn request(b: &BaseLease, id: u64, edits: Vec<Value>) -> Value {
    json!({"version":2,"op":"submit","id":id,"base":1,
    "token":{"workspace":b.token().workspace,"generation":b.token().generation,"inputs":hex(b.token().inputs)},"edits":edits})
}
fn submit(w: &CandidateWorkspace, b: &BaseLease, id: u64, edits: Vec<Value>) -> CandidateLease {
    w.submit_v2(b, &request(b, id, edits)).unwrap()
}
fn result(w: &CandidateWorkspace, c: &CandidateLease) -> Arc<CheckReceipt> {
    match w.result(c).unwrap() {
        CandidateResult::Ready(r) => r,
        _ => panic!("serial pending"),
    }
}
fn promote(w: &CandidateWorkspace, b: &BaseLease, c: &CandidateLease, id: u64) -> Promotion {
    w.promote(
        CandidateId {
            client: 9,
            nonce: id,
        },
        c,
        b.token(),
    )
    .unwrap()
}
fn create_import(w: &CandidateWorkspace, b: &BaseLease) -> Vec<Value> {
    vec![
        edit(
            w,
            b,
            "create_source",
            "mods/new.hir",
            Some("pub fn value()->u64{2}\n"),
        ),
        edit(
            w,
            b,
            "replace_source",
            "main.hir",
            Some("pub fn main()->u64{new.value()}\n"),
        ),
        manifest_edit(
            w,
            b,
            "replace",
            "hirsel.json",
            Some(&manifest(
                "app",
                "1",
                &[("main.hir", &[]), ("mods/new.hir", &["new"])],
                vec![],
            )),
        ),
    ]
}
#[test]
fn candidate_v2_h1_h2_atomic_create_delete_and_recomputed_scope() {
    let t = Tree::basic();
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let edits = create_import(&w, &b);
    let a = submit(&w, &b, 1, edits.clone());
    let mut reverse = edits.clone();
    reverse.reverse();
    let c = submit(&w, &b, 2, reverse);
    let ra = result(&w, &a);
    let rc = result(&w, &c);
    assert_eq!(ra.verdict, Verdict::Pass, "{:?}", ra.loader_error);
    assert_eq!(ra.completion, Completion::Complete);
    assert_eq!(ra.candidate_inputs, rc.candidate_inputs);
    assert!(Arc::ptr_eq(
        ra.checked_products.as_ref().unwrap(),
        rc.checked_products.as_ref().unwrap()
    ));
    assert!(
        ra.selection_v2().unwrap()["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["logical"].as_str().unwrap().contains("new.hir"))
    );
    assert_eq!(w.counters().frozen_loads, 1);
    assert!(!t.0.join("mods/new.hir").exists());
    assert!(matches!(
        w.inspect_v2(&b, "app", "mods/new.hir").unwrap(),
        Inspection::Absent { .. }
    ));
    let unassigned = submit(&w, &b, 3, edits[..2].to_vec());
    assert_eq!(result(&w, &unassigned).verdict, Verdict::Reject);
    assert!(matches!(promote(&w, &b, &a, 1), Promotion::Promoted(_)));
    let b1 = w.head_lease().unwrap();
    let deleted = edit(&w, &b1, "delete_source", "mods/new.hir", None);
    let missing = submit(&w, &b1, 4, vec![deleted.clone()]);
    let r = result(&w, &missing);
    assert_eq!(r.failure_class, Some(FailureClass::LoadIo));
    assert_eq!(r.verdict, Verdict::Undecided);
    assert!(r.selection_v2().is_none());
    assert!(r.scope.packages.is_empty());
    assert!(r.checked_products.is_none());
    let no_ref = manifest_edit(
        &w,
        &b1,
        "replace",
        "hirsel.json",
        Some(&manifest("app", "1", &[("main.hir", &[])], vec![])),
    );
    let unresolved = submit(&w, &b1, 5, vec![deleted.clone(), no_ref.clone()]);
    assert_eq!(result(&w, &unresolved).verdict, Verdict::Reject);
    let c = submit(
        &w,
        &b1,
        6,
        vec![
            deleted,
            no_ref,
            edit(&w, &b1, "replace_source", "main.hir", Some(MAIN)),
        ],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass, "{:?}", r.loader_error);
    assert!(matches!(r.dependencies, DependencyDelta::Known { .. }));
    assert!(matches!(promote(&w, &b1, &c, 2), Promotion::Promoted(_)));
    let b2 = w.head_lease().unwrap();
    assert_eq!(b2.token().inputs, b.token().inputs);
    assert!(matches!(
        promote(&w, &b, &a, 99),
        Promotion::Conflict { .. }
    ));
    assert!(matches!(
        w.inspect_v2(&b2, "app", "mods/new.hir").unwrap(),
        Inspection::Absent { .. }
    ));
    assert!(matches!(
        w.inspect_v2(&b2, "app", "mods").unwrap(),
        Inspection::Present {
            kind: hirsel_engine::loader_inputs::InputKind::Directory,
            ..
        }
    ));
    assert_eq!(std::fs::read_to_string(t.0.join("main.hir")).unwrap(), MAIN);
}
#[test]
fn candidate_v2_h6_manifest_and_graph_errors_are_incomplete() {
    let t = Tree::basic();
    t.write("dep.hir", MAIN);
    t.write(
        "dep.json",
        &manifest(
            "dep",
            "1",
            &[("dep.hir", &[])],
            vec![dependency("hirsel.json", "app", "1")],
        ),
    );
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let invalid = vec![
        "{".into(),
        "{\"schema\":1,\"schema\":1}".into(),
        manifest("app", "01", &[("main.hir", &[])], vec![]),
        manifest("app", "1", &[], vec![]),
        manifest("app", "1", &[("main.hir", &[])], vec![]).replace("\"schema\":1", "\"schema\":3"),
        manifest("app", "1", &[("main.hir", &[])], vec![])
            .replace("\"schema\":1", "\"unknown\":0,\"schema\":1"),
        manifest(
            "app",
            "1",
            &[("main.hir", &[])],
            vec![dependency("dep.json", "dep", "1")],
        ),
        manifest(
            "app",
            "1",
            &[("main.hir", &[])],
            vec![dependency("dep.json", "wrong", "1")],
        ),
    ];
    for (i, text) in invalid.iter().enumerate() {
        let c = submit(
            &w,
            &b,
            i as u64,
            vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some(text))],
        );
        let r = result(&w, &c);
        let oracle = Tree::basic();
        oracle.write("dep.hir", MAIN);
        oracle.write(
            "dep.json",
            &manifest(
                "dep",
                "1",
                &[("dep.hir", &[])],
                vec![dependency("hirsel.json", "app", "1")],
            ),
        );
        oracle.write("hirsel.json", text);
        assert_eq!(artifacts(&r, &t.0), cold(&oracle, 1, 0));
        assert_eq!(
            r.failure_class,
            Some(FailureClass::LoadManifest),
            "{text}: {:?}",
            r.loader_error
        );
        assert_eq!(
            r.completion,
            Completion::Incomplete(IncompleteReason::ToolFailure)
        );
        assert_eq!(r.verdict, Verdict::Undecided);
        assert!(r.checked_products.is_none());
        assert!(r.selection_v2().is_none());
        assert_eq!(promote(&w, &b, &c, i as u64), Promotion::Ineligible);
    }
}
#[test]
fn candidate_v2_h4_alternate_dependency_known_absence_unknown_and_duplicate_revision() {
    let t = Tree::basic();
    t.write("a.hir", "pub fn value()->u64{1}\npub fn main()->u64{1}\n");
    t.write("b.hir", "pub fn value()->u64{2}\npub fn main()->u64{1}\n");
    for (p, v, s) in [
        ("a.json", "1", "a.hir"),
        ("b.json", "2", "b.hir"),
        ("dup.json", "1", "b.hir"),
    ] {
        t.write(p, &manifest("lib", v, &[(s, &["lib"])], vec![]));
    }
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    for (i, p, v, expected_class) in [
        (0, "b.json", "2", None),
        (1, "absent.json", "1", Some(FailureClass::LoadIo)),
        (
            2,
            "../uncaptured/lib.json",
            "1",
            Some(FailureClass::Unavailable),
        ),
    ] {
        let text = manifest(
            "app",
            "1",
            &[("main.hir", &[])],
            vec![dependency(p, "lib", v)],
        );
        let c = submit(
            &w,
            &b,
            i,
            vec![
                manifest_edit(&w, &b, "replace", "hirsel.json", Some(&text)),
                edit(
                    &w,
                    &b,
                    "replace_source",
                    "main.hir",
                    Some("pub fn main()->u64{lib.value()}\n"),
                ),
            ],
        );
        let r = result(&w, &c);
        assert_eq!(r.failure_class, expected_class, "{:?}", r.loader_error);
        if expected_class.is_none() {
            let oracle = Tree::basic();
            for path in ["a.hir", "b.hir", "a.json", "b.json", "dup.json"] {
                oracle.write(path, &std::fs::read_to_string(t.0.join(path)).unwrap());
            }
            oracle.write("hirsel.json", &text);
            oracle.write("main.hir", "pub fn main()->u64{lib.value()}\n");
            assert_eq!(artifacts(&r, &t.0), cold(&oracle, 1, 0));

            assert_eq!(r.verdict, Verdict::Pass);
            assert!(
                r.scope
                    .packages
                    .iter()
                    .any(|p| p.name == "lib" && p.version == "2")
            );
        } else {
            assert!(r.checked_products.is_none());
            assert_eq!(r.verdict, Verdict::Undecided);
        }
    }
    let text = manifest(
        "app",
        "1",
        &[("main.hir", &[])],
        vec![
            dependency("a.json", "lib", "1"),
            dependency("dup.json", "lib", "1"),
        ],
    );
    let c = submit(
        &w,
        &b,
        8,
        vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some(&text))],
    );
    assert_eq!(
        result(&w, &c).failure_class,
        Some(FailureClass::LoadManifest)
    );
    let text = manifest(
        "app",
        "1",
        &[("main.hir", &[])],
        vec![
            dependency("a.json", "lib", "1"),
            dependency("a.json", "lib", "1"),
        ],
    );
    let c = submit(
        &w,
        &b,
        9,
        vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some(&text))],
    );
    assert_eq!(result(&w, &c).verdict, Verdict::Pass);
}
#[test]
fn candidate_v2_h3_sidecar_selection_uses_only_captured_std() {
    let t = Tree::new();
    t.write("main.hir", MAIN);
    t.write("std.hir", "pub fn unused()->u64{1}\n");
    let mut std: Value =
        serde_json::from_str(&manifest("std", "1", &[("std.hir", &[])], vec![])).unwrap();
    std.as_object_mut().unwrap().remove("entry");
    t.write("std.json", &std.to_string());
    let mut p = t.plan();
    p.entry = t.0.join("main.hir");
    p.std_manifest = Some(t.0.join("std.json"));
    let w = workspace(1);
    let b = w.capture_v2(p).unwrap();
    let text = manifest("app", "1", &[("main.hir", &[])], vec![]);
    let c = submit(
        &w,
        &b,
        1,
        vec![manifest_edit(
            &w,
            &b,
            "create",
            "main.hir.json",
            Some(&text),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass);
    assert_eq!(r.scope.packages.len(), 1);
    assert_eq!(r.scope.packages[0].name, "app");
    t.write("main.hir.json", &text);
    let old = submit(&w, &b, 2, vec![]);
    let r = result(&w, &old);
    assert!(r.scope.packages.iter().any(|p| p.name == "std"));
    assert!(matches!(promote(&w, &b, &c, 1), Promotion::Promoted(_)));
    let b1 = w.head_lease().unwrap();
    let c = submit(
        &w,
        &b1,
        3,
        vec![manifest_edit(&w, &b1, "delete", "main.hir.json", None)],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass);
    assert!(r.scope.packages.iter().any(|p| p.name == "std"));
    let mut p = t.plan();
    p.entry = t.0.join("main.hir");
    let b = w.capture_v2(p).unwrap();
    let c = submit(
        &w,
        &b,
        4,
        vec![manifest_edit(&w, &b, "delete", "main.hir.json", None)],
    );
    assert_eq!(
        result(&w, &c).failure_class,
        Some(FailureClass::Unavailable)
    );
}
#[test]
fn candidate_v2_h7_aliases_roles_preconditions_duplicates_and_utf8() {
    let t = Tree::basic();
    std::os::unix::fs::symlink("main.hir", t.0.join("alias.hir")).unwrap();
    t.write("other.txt", "not a source");
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let a = edit(&w, &b, "replace_source", "main.hir", Some(MAIN));
    let alias = edit(&w, &b, "replace_source", "alias.hir", Some(MAIN));
    assert_eq!(
        w.submit_v2(&b, &request(&b, 1, vec![a.clone(), alias.clone()]))
            .err()
            .unwrap(),
        "DuplicateTarget"
    );
    let mut stale = a.clone();
    stale["expected"]["present"] = json!("0".repeat(64));
    assert_eq!(
        w.submit_v2(&b, &request(&b, 2, vec![stale.clone(), alias]))
            .err()
            .unwrap(),
        "DuplicateTarget"
    );
    assert_eq!(
        w.submit_v2(&b, &request(&b, 2, vec![stale])).err().unwrap(),
        "PreconditionFailed"
    );
    let mut stale = a.clone();
    stale["expected"]["bytes"] = json!("0".repeat(64));
    assert_eq!(
        w.submit_v2(&b, &request(&b, 2, vec![stale])).err().unwrap(),
        "StaleBytes"
    );
    let forbidden = edit(&w, &b, "replace_source", "other.txt", Some(MAIN));
    assert!(
        w.submit_v2(&b, &request(&b, 2, vec![forbidden]))
            .err()
            .unwrap()
            .starts_with("UnsupportedEdit")
    );
    let mut absent = edit(&w, &b, "create_source", "mods/new.hir", Some(""));
    absent["path"]["path"] = json!("missing/deep.hir");
    assert!(
        w.submit_v2(&b, &request(&b, 2, vec![absent]))
            .err()
            .unwrap()
            .contains("MissingParent")
    );
    let new = edit(&w, &b, "create_source", "mods/new.hir", Some(""));
    let c = submit(&w, &b, 2, vec![new]);
    assert_eq!(result(&w, &c).verdict, Verdict::Pass);
    assert!(matches!(promote(&w, &b, &c, 1), Promotion::Promoted(_)));
    let b1 = w.head_lease().unwrap();
    assert!(
        matches!(w.inspect_v2(&b1,"app","mods/new.hir").unwrap(),Inspection::Present{bytes:Some(h),..} if h==ContentHash::of_bytes(b""))
    );
    let mut create = edit(&w, &b1, "create_source", "mods/new.hir", Some(MAIN));
    create["expected"] = expected(&w, &b, "mods/new.hir");
    assert_eq!(
        w.submit_v2(&b1, &request(&b1, 4, vec![create]))
            .err()
            .unwrap(),
        "PreconditionFailed"
    );
    let del = edit(&w, &b1, "delete_source", "mods/new.hir", None);
    let new = edit(&w, &b1, "replace_source", "mods/new.hir", Some(MAIN));
    assert_eq!(
        w.submit_v2(&b1, &request(&b1, 4, vec![del, new]))
            .err()
            .unwrap(),
        "DuplicateTarget"
    );
    for p in [
        "/main.hir",
        "./main.hir",
        "mods/../main.hir",
        "mods//a.hir",
        "mods\\a.hir",
        "",
    ] {
        let mut bad = a.clone();
        bad["path"]["path"] = json!(p);
        assert!(w.submit_v2(&b, &request(&b, 9, vec![bad])).is_err(), "{p}");
    }
    let text = "// α\r\npub fn main()->u64{1}\r\n";
    let t2 = Tree::basic();
    t2.write("main.hir", text);
    let b = w.capture_v2(t2.plan()).unwrap();
    let mut patch = edit(&w, &b, "edit_source", "main.hir", None);
    patch["byte_edits"] = json!([{"start":4,"end":5,"replacement":"A"}]);
    assert!(
        w.submit_v2(&b, &request(&b, 20, vec![patch]))
            .err()
            .unwrap()
            .contains("Utf8Boundary")
    );
    let mut patch = edit(&w, &b, "edit_source", "main.hir", None);
    patch["byte_edits"] = json!([{"start":3,"end":5,"replacement":"A"}]);
    let c = submit(&w, &b, 21, vec![patch]);
    assert_eq!(result(&w, &c).verdict, Verdict::Pass);
}
#[test]
fn candidate_v2_h8_frozen_library_sources_allow_edits() {
    hirsel_engine::with_compiler_stack(|| {
        let t = Tree::basic();
        t.write("lib.hir", "pub fn value(x:u64)->u64{x+1}\n");
        let mut m: Value =
            serde_json::from_str(&manifest("lib", "1", &[("lib.hir", &["lib"])], vec![])).unwrap();
        m.as_object_mut().unwrap().remove("entry");
        t.write("lib.json", &m.to_string());
        t.write(
            "hirsel.json",
            &manifest(
                "app",
                "1",
                &[("main.hir", &[])],
                vec![dependency("lib.json", "lib", "1")],
            ),
        );
        t.write("main.hir", "pub fn main()->u64{lib.value(1)}\n");
        let p = t.plan();
        let w = workspace(1);
        let b = w.capture_v2(p).unwrap();
        let a = submit(&w, &b, 1, vec![]);
        let ra = result(&w, &a);
        assert_eq!(ra.verdict, Verdict::Pass);
        assert!(!ra.selection_v2().unwrap()["sources"].is_null());
        let c = submit(
            &w,
            &b,
            2,
            vec![edit(
                &w,
                &b,
                "replace_source",
                "lib.hir",
                Some("pub fn value(x:u64)->u64{x+2}\n"),
            )],
        );
        std::fs::remove_file(t.0.join("lib.hir")).unwrap();
        let r = result(&w, &c);
        assert_eq!(r.verdict, Verdict::Pass, "{:?}", r.loader_error);
        assert!(!r.selection_v2().unwrap()["sources"].is_null());
        assert_eq!(result(&w, &a).selection_v2(), ra.selection_v2());
    });
}
#[path = "candidate_model.rs"]
mod model;
#[test]
fn candidate_v2_h5_h18_finite_model_waiters_aba_and_idempotence() {
    let t = Tree::basic();
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let original = BTreeMap::from([("main.hir".into(), MAIN.into())]);
    let mut model = model::Model::new(b.token().workspace, original.clone());
    let mb = model.head();
    let mut files = original;
    files.insert("mods/new.hir".into(), "pub fn value()->u64{2}\n".into());
    let edits = create_import(&w, &b);
    let a = submit(&w, &b, 1, edits.clone());
    let duplicate = submit(&w, &b, 2, edits.clone());
    let retry = submit(&w, &b, 1, edits.clone());
    model.submit(1, mb.clone(), files.clone());
    model.submit(2, mb.clone(), files.clone());
    assert_eq!(w.cancel(&a).unwrap(), CancelAck::Cancelled);
    model.cancel(1);
    assert_eq!(
        result(&w, &a).completion,
        Completion::Incomplete(IncompleteReason::Cancelled)
    );
    let r = result(&w, &duplicate);
    assert_eq!(r.verdict, Verdict::Pass);
    model.publish(2, true);
    assert_eq!(
        result(&w, &retry).completion,
        Completion::Incomplete(IncompleteReason::Cancelled)
    );
    assert_eq!(promote(&w, &b, &a, 10), Promotion::Ineligible);
    assert_eq!(
        model.promote(10, 1, mb.clone()),
        model::Promotion::Ineligible
    );
    let actual = promote(&w, &b, &duplicate, 11);
    let expected = model.promote(11, 2, mb.clone());
    assert!(matches!(actual, Promotion::Promoted(_)));
    assert!(matches!(expected, model::Promotion::Promoted(_)));
    assert_eq!(promote(&w, &b, &duplicate, 11), actual);
    assert_eq!(promote(&w, &b, &a, 11), Promotion::OperationIdConflict);
    let mut different = request(&b, 1, edits);
    different["edits"][0]["text"] = json!("different");
    assert_eq!(
        w.submit_v2(&b, &different).err().unwrap(),
        "RequestIdConflict"
    );
    let b2 = w.capture_v2(t.plan()).unwrap();
    let mm = model.capture(BTreeMap::from([("main.hir".into(), MAIN.into())]));
    assert_eq!(b.token().inputs, b2.token().inputs);
    assert_ne!(b.token().generation, b2.token().generation);
    assert!(matches!(
        promote(&w, &b, &duplicate, 12),
        Promotion::Conflict { .. }
    ));
    assert_eq!(model.promote(12, 2, mb), model::Promotion::Conflict(mm));
    assert_eq!(w.counters().frozen_loads, 1);
    let c = submit(&w, &b2, 90, vec![]);
    drop(c);
    assert_eq!(
        w.submit_v2(&b2, &request(&b2, 90, vec![])).err().unwrap(),
        "ExpiredRequestId"
    );
    let w = CandidateWorkspace::new(
        config(1),
        Default::default(),
        Limits {
            leases: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let b = w.capture_v2(t.plan()).unwrap();
    let live = submit(&w, &b, 1, vec![]);
    assert_eq!(
        w.submit_v2(&b, &request(&b, 2, vec![])).err().unwrap(),
        "ResourceLimit"
    );
    assert_eq!(result(&w, &live).verdict, Verdict::Pass);
}
#[test]
fn candidate_v2_duplicate_path_diagnostics_equal_independent_cold() {
    let t = Tree::basic();
    t.write("helper.hir", "fn collide()->u64{2}\n");
    t.write(
        "hirsel.json",
        &manifest(
            "app",
            "1",
            &[("main.hir", &[]), ("helper.hir", &[])],
            vec![],
        ),
    );
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let c = submit(
        &w,
        &b,
        1,
        vec![edit(
            &w,
            &b,
            "replace_source",
            "main.hir",
            Some("fn collide()->u64{1}\npub fn main()->u64{1}\n"),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Reject);
    let before = json!({"json":r.output.diagnostics.iter().map(|d|d.to_json(&r.output.sources)).collect::<Vec<_>>(),"human":r.output.diagnostics.iter().map(|d|d.render_human(&r.output.sources)).collect::<Vec<_>>()});
    let oracle = Tree::basic();
    oracle.write("helper.hir", "fn collide()->u64{2}\n");
    oracle.write("main.hir", "fn collide()->u64{1}\npub fn main()->u64{1}\n");
    oracle.write(
        "hirsel.json",
        &manifest(
            "app",
            "1",
            &[("main.hir", &[]), ("helper.hir", &[])],
            vec![],
        ),
    );
    assert_eq!(artifacts(&r, &t.0), cold(&oracle, 1, 0));
    assert!(before.to_string().contains("duplicate definition"));
}

#[test]
fn candidate_v2_h16_old_secondary_diagnostics_survive_deletion_and_pressure() {
    let t = Tree::basic();
    t.write("helper.hir", "pub fn value()->unique u32{1 / 0}\n");
    t.write(
        "hirsel.json",
        &manifest(
            "app",
            "1",
            &[("main.hir", &[]), ("helper.hir", &["helper"])],
            vec![],
        ),
    );
    let w = CandidateWorkspace::new(
        config(1),
        Default::default(),
        Limits {
            leases: 4,
            retain_canonical_export: true,
            ..Limits::default()
        },
    )
    .unwrap();
    let b = w.capture_v2(t.plan()).unwrap();
    let c = submit(
        &w,
        &b,
        1,
        vec![edit(
            &w,
            &b,
            "replace_source",
            "main.hir",
            Some("const VALUE:u32=comptime helper.value()\npub fn main()->u64{1}\n"),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Reject);
    assert!(
        r.output
            .diagnostics
            .iter()
            .flat_map(|d| &d.secondary)
            .any(|l| r
                .output
                .sources
                .file(r.output.sources.resolve(l.span).file)
                .is_some_and(|f| f.path.ends_with("helper.hir")))
    );
    let oracle = Tree::basic();
    oracle.write("helper.hir", "pub fn value()->unique u32{1 / 0}\n");
    oracle.write(
        "main.hir",
        "const VALUE:u32=comptime helper.value()\npub fn main()->u64{1}\n",
    );
    oracle.write(
        "hirsel.json",
        &manifest(
            "app",
            "1",
            &[("main.hir", &[]), ("helper.hir", &["helper"])],
            vec![],
        ),
    );
    assert_eq!(artifacts(&r, &t.0), cold(&oracle, 1, 0));
    let pressure1 = submit(&w, &b, 10, vec![]);
    let pressure2 = submit(&w, &b, 11, vec![]);
    let pressure3 = submit(&w, &b, 12, vec![]);
    assert_eq!(
        w.submit_v2(&b, &request(&b, 13, vec![])).err().unwrap(),
        "ResourceLimit"
    );
    let before = json!({"json":r.output.diagnostics.iter().map(|d|d.to_json(&r.output.sources)).collect::<Vec<_>>(),"human":r.output.diagnostics.iter().map(|d|d.render_human(&r.output.sources)).collect::<Vec<_>>()});
    assert!(before.to_string().contains("helper.hir"), "{before}");
    drop(pressure1);
    drop(pressure2);
    drop(pressure3);
    let del = submit(
        &w,
        &b,
        2,
        vec![
            edit(&w, &b, "delete_source", "helper.hir", None),
            manifest_edit(
                &w,
                &b,
                "replace",
                "hirsel.json",
                Some(&manifest("app", "1", &[("main.hir", &[])], vec![])),
            ),
        ],
    );
    assert_eq!(result(&w, &del).verdict, Verdict::Pass);
    assert!(matches!(promote(&w, &b, &del, 1), Promotion::Promoted(_)));
    drop(b);
    drop(c);
    drop(del);
    std::fs::remove_dir_all(&t.0).unwrap();
    let after = json!({"json":r.output.diagnostics.iter().map(|d|d.to_json(&r.output.sources)).collect::<Vec<_>>(),"human":r.output.diagnostics.iter().map(|d|d.render_human(&r.output.sources)).collect::<Vec<_>>()});
    assert_eq!(before, after);
    assert!(w.counters().retained_bytes > 0);
}
#[test]
fn candidate_receipts_bind_failed_lowerings_to_complete_current_checks() {
    hirsel_engine::with_compiler_stack(|| {
        for jobs in [1, 2] {
            for retention in [
                hirsel_engine::memo::Retention::Batch,
                hirsel_engine::memo::Retention::Resident,
            ] {
                let tree = Tree::basic();
                tree.write("helper.hir", "pub fn value()->unique u32{1 / 0}\n");
                tree.write(
                    "main.hir",
                    "const VALUE:u32=comptime helper.value()\npub fn main()->u64{1}\n",
                );
                tree.write(
                    "hirsel.json",
                    &manifest(
                        "app",
                        "1",
                        &[("main.hir", &[]), ("helper.hir", &["helper"])],
                        vec![],
                    ),
                );
                let compiler = hirsel_driver::COMPILER
                    .clone()
                    .with_config(CompileConfig {
                        retention,
                        store: Arc::new(hirsel_engine::store::NoneStore),
                        ..config(jobs)
                    })
                    .with_owned_engines();
                let mut program = compiler
                    .load_program_inputs(
                        &tree.0.join("hirsel.json"),
                        &hirsel_engine::loader_inputs::DiskInputs,
                        false,
                    )
                    .unwrap();
                let lowering = program.engine.adapters.lowering.take();
                let incomplete = program.check(LoweringPolicy::Comptime).unwrap();
                assert_ne!(
                    incomplete.completion,
                    hirsel_engine::CheckCompletion::Complete
                );
                assert!(program.engine.checked_products(&program.entry).is_err());
                program.engine.adapters.lowering = lowering;
                let rejected = program.check(LoweringPolicy::Comptime).unwrap();
                assert_eq!(
                    rejected.completion,
                    hirsel_engine::CheckCompletion::Complete
                );
                assert!(
                    rejected
                        .diagnostics
                        .iter()
                        .any(|d| d.rule == hirsel_diag::RuleId::of("CT-1"))
                );
                let mut failed = program.entry.clone();
                failed.0.name = "VALUE".into();
                let before = program
                    .engine
                    .recompute_count(&failed, hirsel_engine::QueryKind::Lower);
                let receipt = program.engine.checked_products(&program.entry).unwrap();
                assert_eq!(
                    program
                        .engine
                        .checked_products(&program.entry)
                        .unwrap()
                        .digest,
                    receipt.digest
                );
                assert_eq!(
                    program
                        .engine
                        .recompute_count(&failed, hirsel_engine::QueryKind::Lower),
                    before,
                    "receipt collection must not repeat a settled failed initializer"
                );
                assert!(
                    program
                        .replace_source(&tree.0.join("main.hir"), MAIN.into())
                        .unwrap()
                );
                assert!(
                    program.engine.checked_products(&program.entry).is_err(),
                    "the old failure cannot certify edited inputs"
                );
                let repaired = program.check(LoweringPolicy::Comptime).unwrap();
                assert_eq!(
                    repaired.completion,
                    hirsel_engine::CheckCompletion::Complete
                );
                assert!(
                    repaired.diagnostics.is_empty(),
                    "{:?}",
                    repaired.diagnostics
                );
                assert_ne!(
                    program
                        .engine
                        .checked_products(&program.entry)
                        .unwrap()
                        .digest,
                    receipt.digest
                );
            }
        }
    });
}
#[test]
fn candidate_v2_h19_removed_roots_never_discover_host_inputs() {
    // This test also runs in a separate hostile-environment process below.
    let t = Tree::basic();
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let edits = create_import(&w, &b);
    std::fs::remove_dir_all(&t.0).unwrap();
    assert!(matches!(
        w.inspect_v2(&b, "app", "mods/new.hir").unwrap(),
        Inspection::Absent { .. }
    ));
    let c = submit(&w, &b, 1, edits);
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass, "{:?}", r.loader_error);
    assert!(matches!(promote(&w, &b, &c, 1), Promotion::Promoted(_)));
    let b1 = w.head_lease().unwrap();
    assert!(matches!(
        w.inspect_v2(&b1, "app", "mods/new.hir").unwrap(),
        Inspection::Present { .. }
    ));
    let unknown = manifest(
        "app",
        "1",
        &[("main.hir", &[])],
        vec![dependency("../uncaptured/lib.json", "lib", "1")],
    );
    let c = submit(
        &w,
        &b1,
        2,
        vec![manifest_edit(
            &w,
            &b1,
            "replace",
            "hirsel.json",
            Some(&unknown),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.failure_class, Some(FailureClass::Unavailable));
    assert_eq!(
        r.completion,
        Completion::Incomplete(IncompleteReason::InputUnavailable)
    );
}
#[test]
fn candidate_v2_protocol_strict_latched_handles_and_observe_disk() {
    let t = Tree::basic();
    let mut c = protocol::Connection::new(workspace(1), t.0.clone()).unwrap();
    let reply=c.handle(&json!({"version":2,"op":"capture","entry":"hirsel.json","roots":[{"id":"app","path":"","editable":true}]}));
    assert_eq!(reply["version"], 2);
    assert!(reply.get("ok").is_some(), "{reply}");
    let base = reply["ok"]["base"].as_u64().unwrap();
    let token = reply["ok"]["token"].clone();
    assert_eq!(
        c.handle(&json!({"version":1,"op":"head"}))["error"],
        "UnsupportedVersion"
    );
    let facts = c.handle(
        &json!({"version":2,"op":"inspect","base":base,"paths":[{"root":"app","path":"main.hir"}]}),
    );
    let expected = facts["ok"]["nodes"][0]["expected"].clone();
    let req = json!({"version":2,"op":"submit","base":base,"token":token,"id":1,"edits":[{"kind":"replace_source","path":{"root":"app","path":"main.hir"},"expected":expected,"text":MAIN}]});
    for extra in ["text", "typo"] {
        let mut r = req.clone();
        r[extra] = json!(0);
        assert!(
            c.handle(&r)["error"]
                .as_str()
                .unwrap()
                .starts_with("InvalidRequest")
        );
    }
    for mutation in 0..4 {
        let mut r = req.clone();
        match mutation {
            0 => r["priority"] = json!("high"),
            1 => r["edits"][0]["expected"]["absent"] = json!("0".repeat(64)),
            2 => r["edits"][0]["text"] = json!(5),
            _ => r["edits"][0]["kind"] = json!("rename_source"),
        };
        assert!(
            c.handle(&r)["error"]
                .as_str()
                .unwrap()
                .starts_with("InvalidRequest"),
            "{r}"
        );
    }
    for raw in [
        r#"{"version":2,"op":"head","op":"head"}"#,
        r#"{"version":2,"x":{"a":1,"a":2}}"#,
        r#"{"version":2,"text":"\ud800"}"#,
    ] {
        assert!(protocol::decode_frame(raw.as_bytes()).is_err());
    }
    let bad = protocol::decode_wire_frame(br#"{"version":2,"op":"head","op":"head"}"#).unwrap();
    assert!(
        c.handle(&bad)["error"]
            .as_str()
            .unwrap()
            .starts_with("InvalidRequest")
    );
    assert!(protocol::decode_frame(br#"{"version":1,"op":"head","op":"head"}"#).is_ok());
    let mut malformed = req.clone();
    malformed["base"] = json!(999);
    malformed["edits"][0]["typo"] = json!(1);
    assert!(
        c.handle(&malformed)["error"]
            .as_str()
            .unwrap()
            .starts_with("InvalidRequest")
    );
    let id = c.handle(&req)["ok"]["lease"].as_u64().unwrap();
    let mut result = Value::Null;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        result = c.handle(&json!({"version":2,"op":"result","lease":id}));
        if result["ok"]["pending"] != true {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(result["ok"]["verdict"], "Pass", "{result}");
    assert_eq!(result["ok"]["scope_complete"], true);
    let obs = json!({"version":2,"op":"observe_disk","target":"base","handle":base});
    assert_eq!(c.handle(&obs)["ok"]["status"], "MatchesObserved");
    t.write("main.hir", "pub fn main()->u64{3}\n");
    assert_eq!(c.handle(&obs)["ok"]["status"], "Changed");
    assert_eq!(
        c.handle(&json!({"version":2,"op":"promote","base":base,"lease":id,"id":99}))["ok"]["promotion"],
        "Promoted"
    );
    let mut v1 = protocol::Connection::new(workspace(1), t.0.clone()).unwrap();
    assert!(
        v1.handle(&json!({"version":1,"op":"head"}))["error"]
            .as_str()
            .unwrap()
            .contains("UnknownBase")
    );
    assert_eq!(
        v1.handle(&json!({"version":2,"op":"head"}))["error"],
        "UnsupportedVersion"
    );
}
fn normalized(value: Value, root: &Path) -> Value {
    serde_json::from_str(
        &value
            .to_string()
            .replace(&root.to_string_lossy().to_string(), "<tree>"),
    )
    .unwrap()
}
fn artifacts(r: &CheckReceipt, root: &Path) -> Value {
    normalized(
        json!({"completion":format!("{:?}",r.completion),"verdict":format!("{:?}",r.verdict),"failure":r.failure_class.map(|v|format!("{v:?}")),
        "packages":r.scope.packages,"unsafe_ffi":format!("{:?}",r.output.unsafe_ffi),
        "selection":r.selection_v2().map(|s|json!({"entry":s["entry"],"sources":s["sources"]})),"diagnostics":r.output.diagnostics.iter().map(|d|d.to_json(&r.output.sources)).collect::<Vec<_>>(),"human":r.output.diagnostics.iter().map(|d|d.render_human(&r.output.sources)).collect::<Vec<_>>(),
        "canonical":r.checked_products.as_ref().and_then(|p|p.canonical.clone()),"reads":r.checked_products.as_ref().map(|p|p.canonical_reads().into_iter().map(|((o,k),v)|json!([o,k,hex(v)])).collect::<Vec<_>>())}),
        root,
    )
}
#[test]
fn candidate_v2_cold_child() {
    let Ok(entry) = std::env::var("HIRSEL_V2_COLD_ENTRY") else {
        return;
    };
    let output = std::env::var("HIRSEL_V2_COLD_OUTPUT").unwrap();
    let jobs = std::env::var("HIRSEL_V2_JOBS").unwrap().parse().unwrap();
    let salt = std::env::var("HIRSEL_V2_SALT").unwrap().parse().unwrap();
    hirsel_intern::salt_ids(salt);
    let entry = PathBuf::from(entry);
    let root = entry.parent().unwrap();
    let value = hirsel_engine::with_compiler_stack(|| {
        let compiler = hirsel_driver::COMPILER
            .clone()
            .with_config(config(jobs))
            .with_owned_engines();
        let loaded =
            compiler.load_program_inputs(&entry, &hirsel_engine::loader_inputs::DiskInputs, false);
        let mut program = match loaded {
            Ok(p) => p,
            Err(e) => {
                let class = match e {
                    ToolError::Io(_) => "LoadIo",
                    ToolError::Runtime(_) => "LoadManifest",
                    _ => panic!("{e}"),
                };
                return json!({"completion":"Incomplete(ToolFailure)","verdict":"Undecided","failure":class,"packages":[],"unsafe_ffi":"[]","selection":null,"diagnostics":[],"human":[],"canonical":null,"reads":null});
            }
        };
        let packages: Vec<_> = program
            .files
            .iter()
            .map(|(_, c)| c.package.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let selection = json!({"entry":format!("{:?}",program.entry),
            "sources":program.files.iter().map(|(f,c)|{let file=program.engine.sources.file(*f).unwrap();json!({"logical":file.logical_path(),"package":c.package,"modules":c.modules})}).collect::<Vec<_>>()});
        let checked = program.check(LoweringPolicy::Comptime).unwrap();
        assert_eq!(checked.completion, hirsel_engine::CheckCompletion::Complete);
        let rejected = checked
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error);
        let products = match program.engine.checked_products(&program.entry) {
            Ok(products) => Some(products),
            Err(hirsel_engine::QueryError::NotImplemented) if rejected => None,
            Err(hirsel_engine::QueryError::Diagnostics(diagnostics)) if rejected => {
                let diagnostics =
                    hirsel_engine::select_diagnostics(diagnostics, Some(&checked.sources));
                let json = |diagnostics: &[hirsel_diag::Diagnostic]| {
                    diagnostics
                        .iter()
                        .map(|diagnostic| diagnostic.to_json(&checked.sources))
                        .collect::<Vec<_>>()
                };
                assert_eq!(json(&diagnostics), json(&checked.diagnostics));
                None
            }
            Err(error) => panic!("cold checked products: {error:?}"),
        };
        normalized(
            json!({"completion":"Complete","verdict":if checked.diagnostics.iter().any(|d|d.severity==hirsel_diag::Severity::Error){"Reject"}else{"Pass"},"failure":null,"packages":packages,"unsafe_ffi":format!("{:?}",checked.unsafe_ffi),"selection":selection,
            "diagnostics":checked.diagnostics.iter().map(|d|d.to_json(&checked.sources)).collect::<Vec<_>>(),"human":checked.diagnostics.iter().map(|d|d.render_human(&checked.sources)).collect::<Vec<_>>(),
            "canonical":products.as_ref().and_then(|products|products.canonical.clone()),"reads":products.as_ref().map(|products|products.canonical_reads().into_iter().map(|((o,k),v)|json!([o,k,hex(v)])).collect::<Vec<_>>())}),
            root,
        )
    });
    std::fs::write(output, value.to_string()).unwrap();
}
fn cold(tree: &Tree, jobs: usize, salt: u64) -> Value {
    let out = tree.0.with_extension("cold.json");
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "candidate_v2::candidate_v2_cold_child",
            "--exact",
            "--nocapture",
        ])
        .env_remove("HIRSEL_ID_SALT")
        .env("HIRSEL_V2_COLD_ENTRY", tree.0.join("hirsel.json"))
        .env("HIRSEL_V2_COLD_OUTPUT", &out)
        .env("HIRSEL_V2_JOBS", jobs.to_string())
        .env("HIRSEL_V2_SALT", salt.to_string())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    let v = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    std::fs::remove_file(out).unwrap();
    v
}
#[test]
fn candidate_v2_independent_cold_jobs_salts_matrix() {
    if let Some(runs) = hirsel_intern::under_salts(
        "candidate_v2::candidate_v2_independent_cold_jobs_salts_matrix",
        &[0, 1, 7],
        || {
            let salt = std::env::var("HIRSEL_ID_SALT").unwrap().parse().unwrap();
            let mut comparisons = 0;
            for jobs in [1, 2, 8] {
                for case in 0..12 {
                    let t = Tree::basic();
                    let oracle = Tree::basic();
                    let w = workspace(jobs);
                    if (6..=8).contains(&case) {
                        for tree in [&t, &oracle] {
                            tree.write("mods/new.hir", "pub fn value()->u64{2}\n");
                            tree.write("main.hir", "pub fn main()->u64{new.value()}\n");
                            tree.write(
                                "hirsel.json",
                                &manifest(
                                    "app",
                                    "1",
                                    &[("main.hir", &[]), ("mods/new.hir", &["new"])],
                                    vec![],
                                ),
                            );
                        }
                    }
                    let plan = t.plan();
                    if (9..=11).contains(&case) {
                        for tree in [&t, &oracle] {
                            tree.write("lib.hir", "pub fn value(x:u64)->u64{x+1}\n");
                            let mut lib: Value = serde_json::from_str(&manifest(
                                "lib",
                                "1",
                                &[("lib.hir", &["lib"])],
                                vec![],
                            ))
                            .unwrap();
                            lib.as_object_mut().unwrap().remove("entry");
                            tree.write("lib.json", &lib.to_string());
                        }
                        if case >= 10 {
                            for tree in [&t, &oracle] {
                                tree.write(
                                    "hirsel.json",
                                    &manifest(
                                        "app",
                                        "1",
                                        &[("main.hir", &[])],
                                        vec![dependency("lib.json", "lib", "1")],
                                    ),
                                );
                                tree.write("main.hir", "pub fn main()->u64{lib.value(1)}\n");
                            }
                        }
                    }
                    let b = w.capture_v2(plan).unwrap();
                    let edits = match case {
                        0 | 1 => {
                            let edits = create_import(&w, &b);
                            oracle.write("mods/new.hir", "pub fn value()->u64{2}\n");
                            oracle.write("main.hir", "pub fn main()->u64{new.value()}\n");
                            if case == 0 {
                                oracle.write(
                                    "hirsel.json",
                                    &manifest(
                                        "app",
                                        "1",
                                        &[("main.hir", &[]), ("mods/new.hir", &["new"])],
                                        vec![],
                                    ),
                                );
                                edits
                            } else {
                                edits[..2].to_vec()
                            }
                        }
                        2 => {
                            oracle.write("hirsel.json", "{");
                            vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some("{"))]
                        }
                        3 => {
                            let m = manifest(
                                "app",
                                "1",
                                &[("main.hir", &[]), ("mods/new.hir", &["new"])],
                                vec![],
                            );
                            oracle.write("hirsel.json", &m);
                            vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some(&m))]
                        }
                        4 => {
                            oracle.write("main.hir", "pub fn main()->u64{false}\n");
                            vec![edit(
                                &w,
                                &b,
                                "replace_source",
                                "main.hir",
                                Some("pub fn main()->u64{false}\n"),
                            )]
                        }
                        6..=8 => {
                            std::fs::remove_file(oracle.0.join("mods/new.hir")).unwrap();
                            let mut edits =
                                vec![edit(&w, &b, "delete_source", "mods/new.hir", None)];
                            if case != 7 {
                                let m = manifest("app", "1", &[("main.hir", &[])], vec![]);
                                oracle.write("hirsel.json", &m);
                                edits.push(manifest_edit(
                                    &w,
                                    &b,
                                    "replace",
                                    "hirsel.json",
                                    Some(&m),
                                ));
                            }
                            if case == 6 {
                                oracle.write("main.hir", MAIN);
                                edits.push(edit(&w, &b, "replace_source", "main.hir", Some(MAIN)));
                            }
                            edits
                        }
                        9 => {
                            let mut dep = dependency("lib.json", "lib", "1");
                            dep["expose"] = json!([{"head":"alternate","module":["lib"]}]);
                            let m = manifest("app", "1", &[("main.hir", &[])], vec![dep]);
                            let text = "pub fn main()->u64{alternate.value(1)}\n";
                            oracle.write("hirsel.json", &m);
                            oracle.write("main.hir", text);
                            vec![
                                manifest_edit(&w, &b, "replace", "hirsel.json", Some(&m)),
                                edit(&w, &b, "replace_source", "main.hir", Some(text)),
                            ]
                        }
                        10 => vec![],
                        11 => {
                            let text = "pub fn value(x:u64)->u64{x+2}\n";
                            oracle.write("lib.hir", text);
                            vec![edit(&w, &b, "replace_source", "lib.hir", Some(text))]
                        }
                        _ => {
                            let text = "pub opaque type Secret: data uses {} { level:u32, }\nfn view(x:Secret)->u32{x.level}\npub fn main()->u64{1}\n";
                            oracle.write("main.hir", text);
                            vec![edit(&w, &b, "replace_source", "main.hir", Some(text))]
                        }
                    };
                    let c = submit(&w, &b, 1, edits);
                    std::fs::remove_dir_all(&t.0).unwrap();
                    let r = result(&w, &c);
                    assert_eq!(
                        artifacts(&r, &t.0),
                        cold(&oracle, jobs, salt),
                        "case {case} jobs {jobs} salt {salt}"
                    );
                    comparisons += 1;
                }
            }
            format!("comparisons={comparisons}")
        },
    ) {
        assert_eq!(runs, vec!["comparisons=36"; 3]);
    }
}
#[test]
fn candidate_v2_a14_a15_scope_entry_exposures_and_foreign_metadata() {
    let t = Tree::basic();
    t.write("lib.hir", "pub fn value()->u64{2}\n");
    let mut lib: Value =
        serde_json::from_str(&manifest("lib", "1", &[("lib.hir", &["lib"])], vec![])).unwrap();
    lib.as_object_mut().unwrap().remove("entry");
    lib["unsafe_ffi"] =
        json!({"link":["x"],"headers_checked":false,"threads":"home","wrappers":[]});
    t.write("lib.json", &lib.to_string());
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let mut m: Value = serde_json::from_str(&manifest(
        "renamed",
        "2",
        &[("main.hir", &[])],
        vec![dependency("lib.json", "lib", "1")],
    ))
    .unwrap();
    let c = submit(
        &w,
        &b,
        1,
        vec![manifest_edit(
            &w,
            &b,
            "replace",
            "hirsel.json",
            Some(&m.to_string()),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass);
    assert_eq!(r.output.unsafe_ffi.len(), 1);
    assert!(
        r.scope
            .packages
            .iter()
            .any(|p| p.name == "renamed" && p.version == "2")
    );
    m["entry"]["module"] = json!(["absent"]);
    let c = submit(
        &w,
        &b,
        2,
        vec![manifest_edit(
            &w,
            &b,
            "replace",
            "hirsel.json",
            Some(&m.to_string()),
        )],
    );
    let r = result(&w, &c);
    assert_ne!(r.verdict, Verdict::Pass);
    assert!(r.checked_products.is_none() || r.verdict == Verdict::Reject);
    lib["unsafe_ffi"]["wrappers"] = json!([{"module":[],"name":"missing","kind":"invalid-wrapper-kind","model":"deterministic"}]);
    let c = submit(
        &w,
        &b,
        3,
        vec![
            manifest_edit(
                &w,
                &b,
                "replace",
                "hirsel.json",
                Some(&manifest(
                    "app",
                    "1",
                    &[("main.hir", &[])],
                    vec![dependency("lib.json", "lib", "1")],
                )),
            ),
            manifest_edit(&w, &b, "replace", "lib.json", Some(&lib.to_string())),
        ],
    );
    assert_ne!(result(&w, &c).verdict, Verdict::Pass);
}
#[test]
fn candidate_v2_quotas_retained_namespace_and_request_storage_never_evict_live_data() {
    let t = Tree::basic();
    let limits = Limits {
        bytes: 128 << 10,
        input_bytes: 64 << 10,
        input_operations: 4096,
        result_bytes: 32 << 10,
        edit_bytes: 32 << 10,
        ..Limits::default()
    };
    let w = CandidateWorkspace::new(config(1), Default::default(), limits).unwrap();
    let mut p = t.plan();
    p.limits.bytes = 64 << 10;
    let b = w.capture_v2(p).unwrap();
    let old = expected(&w, &b, "main.hir");
    let mut live = vec![];
    let mut refusal = false;
    for id in 1..100 {
        let e = edit(
            &w,
            &b,
            "create_source",
            "mods/new.hir",
            Some(&format!("// {}\n{}", id, "x".repeat(4096))),
        );
        match w.submit_v2(&b, &request(&b, id, vec![e])) {
            Ok(c) => live.push(c),
            Err(e) => {
                assert_eq!(e, "ResourceLimit");
                refusal = true;
                break;
            }
        }
    }
    assert!(refusal && !live.is_empty());
    let high = w.counters().retained_bytes;
    assert!(high <= 128 << 10);
    assert_eq!(expected(&w, &b, "main.hir"), old);
    live.clear();
    assert!(w.counters().retained_bytes < high);
    // Tombstones remain bounded/charged after release. A smaller new request
    // can proceed without invalidating the old live base's tokens.
    let c = submit(&w, &b, 1000, vec![]);
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass);
    let bytes = w.counters().retained_bytes;
    assert!(matches!(promote(&w, &b, &c, 1), Promotion::Promoted(_)));
    drop(c);
    drop(b);
    assert!(w.counters().retained_bytes > 0);
    assert!(w.counters().retained_bytes <= bytes + 4096);
    assert_eq!(r.verdict, Verdict::Pass);
    let head = w.head_lease().unwrap();
    assert!(matches!(
        w.inspect_v2(&head, "app", "mods/new.hir").unwrap(),
        Inspection::Absent { .. }
    ));
}
#[test]
fn candidate_v2_a14_alias_assignment_and_a15_foreign_paths_and_transitive_refusal() {
    let t = Tree::basic();
    std::os::unix::fs::symlink("main.hir", t.0.join("alias.hir")).unwrap();
    t.write("lib.hir", "pub fn value()->u64{2}\n");
    t.write("middle.hir", "pub fn value()->u64{3}\n");
    t.write("one.a", "not a native image");
    t.write("two.a", "also never opened or linked");
    let mut lib: Value =
        serde_json::from_str(&manifest("lib", "1", &[("lib.hir", &["lib"])], vec![])).unwrap();
    lib.as_object_mut().unwrap().remove("entry");
    lib["unsafe_ffi"] =
        json!({"link":["one.a"],"headers_checked":false,"threads":"serial","wrappers":[]});
    t.write("lib.json", &lib.to_string());
    let mut middle: Value = serde_json::from_str(&manifest(
        "middle",
        "1",
        &[("middle.hir", &["middle"])],
        vec![dependency("lib.json", "lib", "1")],
    ))
    .unwrap();
    middle.as_object_mut().unwrap().remove("entry");
    t.write("middle.json", &middle.to_string());
    let w = workspace(1);
    let b = w.capture_v2(t.plan()).unwrap();
    let duplicate = manifest(
        "app",
        "1",
        &[("main.hir", &[]), ("alias.hir", &["alias"])],
        vec![],
    );
    let c = submit(
        &w,
        &b,
        1,
        vec![manifest_edit(
            &w,
            &b,
            "replace",
            "hirsel.json",
            Some(&duplicate),
        )],
    );
    assert_eq!(
        result(&w, &c).failure_class,
        Some(FailureClass::LoadManifest)
    );
    let select = manifest(
        "app",
        "1",
        &[("main.hir", &[])],
        vec![dependency("lib.json", "lib", "1")],
    );
    lib["unsafe_ffi"]["link"] = json!(["two.a"]);
    let c = submit(
        &w,
        &b,
        2,
        vec![
            manifest_edit(&w, &b, "replace", "hirsel.json", Some(&select)),
            manifest_edit(&w, &b, "replace", "lib.json", Some(&lib.to_string())),
        ],
    );
    std::fs::remove_file(t.0.join("one.a")).unwrap();
    std::fs::remove_file(t.0.join("two.a")).unwrap();
    let r = result(&w, &c);
    assert_eq!(r.verdict, Verdict::Pass);
    assert!(
        matches!(&r.output.unsafe_ffi[0].link[0],hirsel_engine::LinkInput::File(p) if p==&t.0.join("two.a"))
    );
    let transitive = manifest(
        "app",
        "1",
        &[("main.hir", &[])],
        vec![dependency("middle.json", "middle", "1")],
    );
    let c = submit(
        &w,
        &b,
        3,
        vec![manifest_edit(
            &w,
            &b,
            "replace",
            "hirsel.json",
            Some(&transitive),
        )],
    );
    let r = result(&w, &c);
    assert_eq!(r.failure_class, Some(FailureClass::LoadManifest));
    assert_eq!(r.verdict, Verdict::Undecided);
}
#[test]
fn candidate_v2_loader_error_bytes_are_bounded_results_and_never_promote() {
    let t = Tree::basic();
    let w = CandidateWorkspace::new(
        config(1),
        Default::default(),
        Limits {
            result_bytes: 4096,
            ..Limits::default()
        },
    )
    .unwrap();
    let b = w.capture_v2(t.plan()).unwrap();
    let text=json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[],"dependencies":[],"entry":{"module":[],"name":"main"},"unexpected".repeat(4096):0}).to_string();
    let c = submit(
        &w,
        &b,
        1,
        vec![manifest_edit(&w, &b, "replace", "hirsel.json", Some(&text))],
    );
    let r = result(&w, &c);
    assert_eq!(
        r.completion,
        Completion::Incomplete(IncompleteReason::ResourceLimit)
    );
    assert_eq!(r.verdict, Verdict::Undecided);
    assert!(r.checked_products.is_none());
    assert_eq!(promote(&w, &b, &c, 1), Promotion::Ineligible);
}
