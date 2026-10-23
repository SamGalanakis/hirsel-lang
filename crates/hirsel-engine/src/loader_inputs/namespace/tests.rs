use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
static IDS: AtomicU64 = AtomicU64::new(1);
struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hirsel-namespace-{}-{}",
            std::process::id(),
            IDS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        std::fs::write(self.0.join(path), bytes).unwrap();
    }
    fn dir(&self, path: &str) {
        std::fs::create_dir_all(self.0.join(path)).unwrap();
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn plan(tree: &Tree) -> CapturePlan {
    CapturePlan {
        roots: vec![Root {
            id: "app".into(),
            path: tree.0.clone(),
            editable: true,
        }],
        excluded: BTreeSet::new(),
        failed_probes: vec![],
        entry: tree.0.join("main.hir"),
        std_manifest: None,
        environment: b"explicit-test-environment".to_vec(),
        limits: Limits {
            bytes: 1 << 20,
            nodes: 1000,
            aliases: 100,
            depth: 32,
            link_hops: 40,
            edits: 100,
            attempts: 2,
        },
    }
}
fn key(path: &str) -> Key {
    Key {
        root: "app".into(),
        path: path.into(),
    }
}
fn capture(tree: &Tree) -> NamespaceLease {
    NamespaceLease::capture(&plan(tree), &DiskCapture, &LeaseBudget::new(16 << 20)).unwrap()
}
fn create(base: &NamespaceLease, path: &str, bytes: &[u8]) -> Change {
    let Inspection::Absent { absent, .. } = base.inspect("app", path).unwrap() else {
        panic!("expected absence")
    };
    Change::Create {
        key: key(path),
        absent,
        bytes: bytes.into(),
    }
}
fn replace(base: &NamespaceLease, path: &str, bytes: &[u8]) -> Change {
    let Inspection::Present {
        present,
        bytes: Some(expected_bytes),
        ..
    } = base.inspect("app", path).unwrap()
    else {
        panic!("expected present file")
    };
    Change::Replace {
        key: key(path),
        present,
        expected_bytes,
        bytes: bytes.into(),
    }
}
fn delete(base: &NamespaceLease, path: &str) -> Change {
    let Inspection::Present {
        present,
        bytes: Some(expected_bytes),
        ..
    } = base.inspect("app", path).unwrap()
    else {
        panic!("expected present file")
    };
    Change::Delete {
        key: key(path),
        present,
        expected_bytes,
    }
}
fn error<T>(result: io::Result<T>) -> io::Error {
    match result {
        Ok(_) => panic!("expected failure"),
        Err(e) => e,
    }
}
fn overlay_error(base: &NamespaceLease, changes: &[Change]) -> OverlayError {
    match base.overlay(changes) {
        Ok(_) => panic!("expected refusal"),
        Err(e) => e,
    }
}
fn unavailable_error<T>(result: io::Result<T>) {
    assert!(
        error(result)
            .get_ref()
            .is_some_and(|e| e.is::<InputUnavailable>())
    );
}
struct Trap {
    calls: AtomicUsize,
    enabled: AtomicBool,
}
impl Trap {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            enabled: AtomicBool::new(true),
        }
    }
    fn hit(&self) {
        assert!(
            self.enabled.load(Ordering::SeqCst),
            "post-capture host access"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
    }
}
impl CaptureSource for Trap {
    fn facts(&self, p: &Path) -> io::Result<Facts> {
        self.hit();
        DiskCapture.facts(p)
    }
    fn read(&self, p: &Path, n: usize) -> io::Result<Arc<[u8]>> {
        self.hit();
        DiskCapture.read(p, n)
    }
    fn directory(&self, p: &Path, n: usize) -> io::Result<Vec<DirectoryEntry>> {
        self.hit();
        DiskCapture.directory(p, n)
    }
    fn read_link(&self, p: &Path, n: usize) -> io::Result<PathBuf> {
        self.hit();
        DiskCapture.read_link(p, n)
    }
    fn case_sensitive(&self, p: &Path) -> io::Result<bool> {
        self.hit();
        DiskCapture.case_sensitive(p)
    }
    fn failed_probe(&self, p: &Operation, n: usize) -> io::Result<()> {
        self.hit();
        DiskCapture.failed_probe(p, n)
    }
}

// The independent oracle materializes specified bytes/topology, not namespace
// internals. A separate harness process runs all four ordinary DiskInputs ops.
fn query(inputs: &dyn LoaderInputs, root: &Path, relative: &str) -> serde_json::Value {
    let p = root.join(relative);
    let failure = |e: io::Error| serde_json::json!({"error": format!("{:?}", e.kind())});
    let read = inputs
        .read(&p)
        .map(|b| serde_json::json!(b.as_ref()))
        .unwrap_or_else(failure);
    let stat = inputs
        .metadata(&p)
        .map(|k| serde_json::json!(format!("{k:?}")))
        .unwrap_or_else(failure);
    let canonical = inputs
        .canonicalize(&p)
        .map(|p| serde_json::json!(p.strip_prefix(root).unwrap().to_str().unwrap()))
        .unwrap_or_else(failure);
    let list = inputs
        .directory(&p)
        .map(|entries| {
            serde_json::json!(
                entries
                    .into_iter()
                    .map(|e| (e.name.into_string().unwrap(), format!("{:?}", e.kind)))
                    .collect::<Vec<_>>()
            )
        })
        .unwrap_or_else(failure);
    serde_json::json!([read, stat, canonical, list])
}
const QUERIES: &[&str] = &[
    "main.hir",
    "mods",
    "mods/new.hir",
    "mods/sibling.hir",
    "missing/deep.hir",
    "main.hir/child",
    "empty",
    "alias",
    "diralias/new.hir",
    "diralias/../main.hir",
    "dangling",
];
#[test]
fn independent_disk_oracle_child() {
    let Some(root) = std::env::var_os("HIRSEL_NAMESPACE_ORACLE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    if std::env::var_os("HIRSEL_NAMESPACE_FROZEN_CHILD").is_some() {
        let trap = Trap::new();
        let tree = Tree(root.clone());
        let base =
            NamespaceLease::capture(&plan(&tree), &trap, &LeaseBudget::new(16 << 20)).unwrap();
        let before = trap.calls.load(Ordering::SeqCst);
        trap.enabled.store(false, Ordering::SeqCst);
        let expected: Vec<_> = QUERIES.iter().map(|p| query(&base, &root, p)).collect();
        std::fs::remove_dir_all(&root).unwrap();
        std::env::set_current_dir("/").unwrap();
        let compiler = crate::Compiler::new(crate::compiler::tests::phases, [0; 32]);
        let config = crate::CompileConfig::for_memory_cap(None);
        let _defaults_trap = crate::config::forbid_host_defaults();
        // Mark a pure interval for an optional syscall trace. Owned fixture
        // deletion/cwd mutation above are deliberately outside the interval.
        eprintln!("NAMESPACE_FROZEN_BEGIN");
        // Process env differs already at launch; capture uses explicit context.
        for (relative, before) in QUERIES.iter().zip(expected) {
            assert_eq!(
                query(&base, &root, relative),
                before,
                "post-removal query {relative}"
            );
        }
        let unknown = Path::new("/uncaptured");
        unavailable_error(base.read(unknown));
        unavailable_error(base.metadata(unknown));
        unavailable_error(base.canonicalize(unknown));
        unavailable_error(base.directory(unknown));
        for op in [
            Operation::Read(PathBuf::from("/uncaptured")),
            Operation::Metadata(PathBuf::from("/uncaptured")),
            Operation::Canonicalize(PathBuf::from("/uncaptured")),
            Operation::Directory(PathBuf::from("/uncaptured")),
        ] {
            unavailable_error(base.existing(op.path(), &op));
        }
        assert_eq!(before, trap.calls.load(Ordering::SeqCst));
        // Positive graph loading after root removal, plus derived standalone
        // failure with uncaptured std, cover direct helper escape sites.
        let loaded = base.load(&compiler, config.clone(), false).unwrap();
        assert_eq!(loaded.entry.0.package.name, "app");
        assert_eq!(loaded.files.len(), 1);
        let (without_sidecar, _) = base.overlay(&[delete(&base, "main.hir.json")]).unwrap();
        assert!(
            matches!(without_sidecar.load(&compiler,config,false),Err(crate::ToolError::Io(e)) if e.get_ref().is_some_and(|e| e.is::<InputUnavailable>()))
        );
        eprintln!("NAMESPACE_FROZEN_END");
        assert!(
            error(base.toolchain_std_manifest().unwrap())
                .get_ref()
                .is_some_and(|e| e.is::<InputUnavailable>())
        );
        return;
    }
    let results: Vec<_> = QUERIES
        .iter()
        .map(|p| query(&super::super::DiskInputs, &root, p))
        .collect();
    std::fs::write(
        root.join("oracle-output.json"),
        serde_json::to_vec(&results).unwrap(),
    )
    .unwrap();
}
fn run_child(root: &Path, frozen: bool) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .arg("--exact")
        .arg("loader_inputs::namespace::tests::independent_disk_oracle_child")
        .arg("--nocapture")
        .env("HIRSEL_NAMESPACE_ORACLE_ROOT", root)
        .env("HIRSEL_IMAGE", "/does-not-exist")
        .env("HIRSEL_RESULTS", "dir@1")
        .env("HIRSEL_RESULTS_DIR", "/does-not-exist")
        .current_dir("/");
    if frozen {
        child.env("HIRSEL_NAMESPACE_FROZEN_CHILD", "1");
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
#[cfg(unix)]
fn aliases(tree: &Tree) {
    use std::os::unix::fs::symlink;
    symlink("main.hir", tree.0.join("alias")).unwrap();
    symlink("mods", tree.0.join("diralias")).unwrap();
    symlink("not-present", tree.0.join("dangling")).unwrap();
}
#[test]
fn materialized_directory_vectors_and_old_snapshot_isolation() {
    let original = Tree::new();
    original.dir("mods");
    original.dir("empty");
    original.write("main.hir", b"one");
    original.write("mods/sibling.hir", b"sibling");
    aliases(&original);
    let base = capture(&original);
    let additions = [
        create(&base, "mods/new.hir", "α\r\n".as_bytes()),
        replace(&base, "alias", b"two"),
    ];
    let (candidate, delta) = base.overlay(&additions).unwrap();
    let mut reversed = additions.to_vec();
    reversed.reverse();
    let (reverse, _) = base.overlay(&reversed).unwrap();
    assert!(candidate.same_namespace(&reverse));
    assert_eq!(candidate.inputs_digest(), reverse.inputs_digest());
    assert_eq!(delta.changed_objects.len(), 2);
    assert_eq!(delta.changed_directory_tables, vec![key("mods")]);
    assert_eq!(delta.alias_resolution_changes.len(), 3);
    let cold = Tree::new();
    cold.dir("mods");
    cold.dir("empty");
    cold.write("main.hir", b"two");
    cold.write("mods/sibling.hir", b"sibling");
    cold.write("mods/new.hir", "α\r\n".as_bytes());
    aliases(&cold);
    run_child(&cold.0, false);
    let expected: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(cold.0.join("oracle-output.json")).unwrap()).unwrap();
    for (path, expected) in QUERIES.iter().zip(expected) {
        assert_eq!(
            query(&candidate, &original.0, path),
            expected,
            "query {path}"
        );
    }
    assert_eq!(&*base.read(&original.0.join("alias")).unwrap(), b"one");
    assert!(matches!(
        base.inspect("app", "mods/new.hir").unwrap(),
        Inspection::Absent { .. }
    ));
    let (deleted, _) = candidate
        .overlay(&[
            delete(&candidate, "mods/new.hir"),
            delete(&candidate, "mods/sibling.hir"),
        ])
        .unwrap();
    assert_eq!(
        deleted.metadata(&original.0.join("mods")).unwrap(),
        InputKind::Directory
    );
    assert!(
        deleted
            .directory(&original.0.join("mods"))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        &*candidate.read(&original.0.join("mods/new.hir")).unwrap(),
        "α\r\n".as_bytes()
    );
    // Independent directory oracle after deleting the last member.
    std::fs::remove_file(cold.0.join("mods/new.hir")).unwrap();
    std::fs::remove_file(cold.0.join("mods/sibling.hir")).unwrap();
    assert_eq!(
        deleted.directory(&original.0.join("mods")).unwrap(),
        super::super::DiskInputs
            .directory(&cold.0.join("mods"))
            .unwrap()
    );
    eprintln!(
        "namespace cold comparisons: 11 four-operation queries + 1 empty-directory comparison"
    );
}
#[test]
fn all_probe_kinds_survive_removed_roots_and_changed_process_context() {
    let tree = Tree::new();
    tree.dir("mods");
    tree.dir("empty");
    tree.write("main.hir", b"frozen");
    aliases(&tree);
    tree.write("main.hir.json",br#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#);
    run_child(&tree.0, true);
}
#[test]
fn symlink_before_dotdot_cross_root_and_dangling_target() {
    let tree = Tree::new();
    tree.dir("real/sub");
    tree.write("main.hir", b"lexical");
    tree.write("real/main.hir", b"resolved");
    std::os::unix::fs::symlink("real/sub", tree.0.join("jump")).unwrap();
    std::os::unix::fs::symlink("jump", tree.0.join("jump2")).unwrap();
    std::os::unix::fs::symlink("main.hir", tree.0.join("a")).unwrap();
    std::os::unix::fs::symlink("main.hir", tree.0.join("b")).unwrap();
    std::os::unix::fs::symlink("loop", tree.0.join("loop")).unwrap();
    std::os::unix::fs::symlink("/outside-root/value", tree.0.join("escape")).unwrap();
    let external = Tree::new();
    external.write("lib.hir", b"external");
    std::os::unix::fs::symlink(external.0.join("lib.hir"), tree.0.join("mounted")).unwrap();
    let mut p = plan(&tree);
    p.roots.push(Root {
        id: "dep".into(),
        path: external.0.clone(),
        editable: false,
    });
    let base = NamespaceLease::capture(&p, &DiskCapture, &LeaseBudget::new(16 << 20)).unwrap();
    assert_eq!(
        &*base.read(&tree.0.join("jump2/../main.hir")).unwrap(),
        b"resolved"
    );
    assert_eq!(
        base.canonicalize(&tree.0.join("jump2/../main.hir"))
            .unwrap(),
        std::fs::canonicalize(tree.0.join("jump2/../main.hir")).unwrap()
    );
    assert_eq!(&*base.read(&tree.0.join("mounted")).unwrap(), b"external");
    unavailable_error(base.read(&tree.0.join("escape")));
    assert_eq!(
        error(base.read(&tree.0.join("loop"))).raw_os_error(),
        Some(libc::ELOOP)
    );
    assert_eq!(
        overlay_error(
            &base,
            &[replace(&base, "a", b"new"), replace(&base, "b", b"new")]
        ),
        OverlayError::DuplicateTarget
    );
    assert_eq!(
        overlay_error(&base, &[delete(&base, "a")]),
        OverlayError::Unsupported
    );
    let (child, _) = base.overlay(&[delete(&base, "main.hir")]).unwrap();
    assert_eq!(
        error(child.read(&tree.0.join("a"))).kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        child.inspect("app", "a").unwrap(),
        Inspection::DanglingSymlink
    );
    assert_eq!(&*base.read(&tree.0.join("a")).unwrap(), b"lexical");
}
#[test]
fn absence_tokens_errors_and_atomic_preconditions() {
    let tree = Tree::new();
    tree.dir("empty");
    tree.dir("excluded");
    tree.write("main.hir", b"");
    tree.write("excluded/secret", b"secret");
    let fifo_path = tree.0.join("fifo");
    let fifo_name = std::ffi::CString::new(fifo_path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: terminated fixture name and ordinary owner-only mode. This
    // creates a FIFO entry; neither test nor capture opens it for bytes.
    assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
    let mut p = plan(&tree);
    p.excluded.insert(tree.0.join("excluded"));
    let base = NamespaceLease::capture(&p, &DiskCapture, &LeaseBudget::new(16 << 20)).unwrap();
    assert_eq!(
        base.inspect("app", "missing/deep.hir").unwrap(),
        Inspection::MissingParent
    );
    for path in ["excluded", "excluded/secret"] {
        unavailable_error(base.metadata(&tree.0.join(path)));
    }
    assert_eq!(
        error(base.metadata(&tree.0.join("main.hir/child"))).kind(),
        io::ErrorKind::NotADirectory
    );
    assert_eq!(
        error(base.directory(&tree.0.join("not-present"))).kind(),
        io::ErrorKind::NotFound
    );
    unavailable_error(base.read(&fifo_path));
    unavailable_error(base.metadata(&fifo_path));
    assert!(
        base.directory(&tree.0)
            .unwrap()
            .iter()
            .any(|e| e.name == "fifo" && e.kind == InputKind::Other)
    );
    let original = base.digest();
    let mut bad = create(&base, "empty/new.hir", b"new");
    if let Change::Create { absent, .. } = &mut bad {
        *absent = ContentHash::of_bytes(b"");
    }
    assert_eq!(
        overlay_error(&base, &[create(&base, "another.hir", b"valid"), bad]),
        OverlayError::Precondition
    );
    assert_eq!(base.digest(), original);
    assert!(matches!(
        base.inspect("app", "another.hir").unwrap(),
        Inspection::Absent { .. }
    ));
    let mut wrong_present = replace(&base, "main.hir", b"new");
    if let Change::Replace { present, .. } = &mut wrong_present {
        *present = ContentHash::default();
    }
    assert_eq!(
        overlay_error(&base, &[wrong_present]),
        OverlayError::Precondition
    );
    let missing_delete = Change::Delete {
        key: key("not-present.hir"),
        present: ContentHash::default(),
        expected_bytes: ContentHash::of_bytes(b""),
    };
    assert_eq!(
        overlay_error(&base, &[missing_delete]),
        OverlayError::Precondition
    );
    let mut stale = replace(&base, "main.hir", b"new");
    if let Change::Replace { expected_bytes, .. } = &mut stale {
        *expected_bytes = ContentHash::default();
    }
    assert_eq!(overlay_error(&base, &[stale]), OverlayError::StaleBytes);
    let old_absence = create(&base, "empty/new.hir", b"new");
    let (changed, _) = base
        .overlay(&[replace(&base, "main.hir", b"change")])
        .unwrap();
    assert_eq!(
        overlay_error(&changed, &[old_absence]),
        OverlayError::Precondition
    );
    assert_eq!(
        overlay_error(&base, &[create(&base, "bad.hir", &[255])]),
        OverlayError::InvalidUtf8
    );
    for path in ["", ".", "../x", "x/../y", "x//y", "/x", "x\\y", "x\0y"] {
        assert!(base.inspect("app", path).is_err(), "{path:?}");
    }
}

struct Faulty {
    mode: u8,
    reads: AtomicUsize,
}
impl CaptureSource for Faulty {
    fn facts(&self, p: &Path) -> io::Result<Facts> {
        DiskCapture.facts(p)
    }
    fn read(&self, p: &Path, n: usize) -> io::Result<Arc<[u8]>> {
        if self.mode == 1 {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        if self.mode == 2 {
            let bytes = DiskCapture.read(p, n)?;
            let replacement: &[u8] = if self.reads.fetch_add(1, Ordering::SeqCst).is_multiple_of(2)
            {
                b"two"
            } else {
                b"one"
            };
            std::fs::write(p, replacement)?;
            return Ok(bytes);
        }
        DiskCapture.read(p, n)
    }
    fn directory(&self, p: &Path, n: usize) -> io::Result<Vec<DirectoryEntry>> {
        DiskCapture.directory(p, n)
    }
    fn read_link(&self, p: &Path, n: usize) -> io::Result<PathBuf> {
        DiskCapture.read_link(p, n)
    }
    fn case_sensitive(&self, _: &Path) -> io::Result<bool> {
        Ok(self.mode != 3)
    }
    fn failed_probe(&self, _: &Operation, _: usize) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "fixed permission failure",
        ))
    }
}
#[test]
fn capture_race_failures_non_utf8_and_case_policy_refuse_atomically() {
    let tree = Tree::new();
    tree.write("main.hir", b"one");
    let budget = LeaseBudget::new(16 << 20);
    let p = plan(&tree);
    assert!(matches!(
        NamespaceLease::capture(
            &p,
            &Faulty {
                mode: 2,
                reads: AtomicUsize::new(0)
            },
            &budget
        ),
        Err(CaptureError::InputsChanged)
    ));
    assert_eq!(budget.retained(), (0, 0));
    assert!(matches!(
        NamespaceLease::capture(
            &p,
            &Faulty {
                mode: 1,
                reads: AtomicUsize::new(0)
            },
            &budget
        ),
        Err(CaptureError::Io(_, io::ErrorKind::PermissionDenied, _))
    ));
    assert!(matches!(
        NamespaceLease::capture(
            &p,
            &Faulty {
                mode: 3,
                reads: AtomicUsize::new(0)
            },
            &budget
        ),
        Err(CaptureError::InvalidPlan(_))
    ));
    use std::os::unix::ffi::OsStringExt;
    std::fs::write(tree.0.join(std::ffi::OsString::from_vec(vec![255])), b"x").unwrap();
    assert!(matches!(
        NamespaceLease::capture(&p, &DiskCapture, &budget),
        Err(CaptureError::InvalidPlan(_))
    ));
    assert_eq!(budget.retained(), (0, 0));
}
#[test]
fn fixed_error_probes_and_unavailable_operations_are_distinct() {
    let tree = Tree::new();
    tree.write("main.hir", b"one");
    let mut p = plan(&tree);
    let outside = PathBuf::from("/not-captured/fixed-probe");
    p.failed_probes.push(Operation::Metadata(outside.clone()));
    let base = NamespaceLease::capture(
        &p,
        &Faulty {
            mode: 0,
            reads: AtomicUsize::new(0),
        },
        &LeaseBudget::new(16 << 20),
    )
    .unwrap();
    assert_eq!(
        error(base.metadata(&outside)).kind(),
        io::ErrorKind::PermissionDenied
    );
    unavailable_error(base.read(&outside));
    unavailable_error(base.canonicalize(&outside));
    unavailable_error(base.directory(&outside));
    unavailable_error(base.metadata(&outside.join("child")));
    assert_eq!(base.usage().fixed_probes, 1);
}
#[test]
fn exact_counts_hardlinks_limits_and_live_lease_release() {
    let tree = Tree::new();
    tree.dir("empty");
    tree.write("main.hir", b"one");
    tree.write("linked.hir", b"linked");
    let outside = Tree::new();
    std::fs::hard_link(tree.0.join("linked.hir"), outside.0.join("hardlink")).unwrap();
    std::os::unix::fs::symlink("main.hir", tree.0.join("alias")).unwrap();
    let budget = LeaseBudget::new(16 << 20);
    let p = plan(&tree);
    let base = NamespaceLease::capture(&p, &DiskCapture, &budget).unwrap();
    let u = base.usage();
    assert_eq!(
        (
            u.roots,
            u.directories,
            u.directory_entries,
            u.aliases,
            u.file_bytes,
            u.fixed_probes
        ),
        (1, 2, 4, 1, 9, 0)
    );
    assert_eq!(u.nodes, 5 + tree.0.ancestors().skip(1).count());
    assert_eq!(
        overlay_error(&base, &[replace(&base, "linked.hir", b"new")]),
        OverlayError::Unsupported
    );
    let (child, _) = base
        .overlay(&[replace(&base, "main.hir", b"longer")])
        .unwrap();
    assert_eq!(
        budget.retained(),
        (u.charged_bytes + child.usage().charged_bytes, 2)
    );
    let clone = base.clone();
    drop(base);
    assert_eq!(budget.retained().1, 2);
    drop(clone);
    assert_eq!(budget.retained(), (child.usage().charged_bytes, 1));
    drop(child);
    assert_eq!(budget.retained(), (0, 0));
    let exact = LeaseBudget::new(u.charged_bytes);
    let base = NamespaceLease::capture(&p, &DiskCapture, &exact).unwrap();
    assert_eq!(
        overlay_error(&base, &[replace(&base, "main.hir", b"two")]),
        OverlayError::Limit
    );
    assert_eq!(&*base.read(&tree.0.join("main.hir")).unwrap(), b"one");
    for which in 0..5 {
        let mut p = p.clone();
        match which {
            0 => p.limits.bytes = 1,
            1 => p.limits.nodes = 1,
            2 => p.limits.aliases = 0,
            3 => p.limits.depth = 0,
            _ => p.limits.attempts = 0,
        }
        assert!(NamespaceLease::capture(&p, &DiskCapture, &budget).is_err());
    }
}
#[test]
fn explicit_toolchain_context_loads_sidecar_graph_without_host_discovery() {
    let tree = Tree::new();
    tree.dir("std");
    tree.write("main.hir", b"pub fn main()->u64{1}\n");
    tree.write("std/lib.hir", b"pub fn value()->u64{2}\n");
    tree.write("std/hirsel.json",br#"{"schema":1,"package":{"name":"std","version":"1"},"sources":[{"path":"lib.hir","module":[]}],"dependencies":[]}"#);
    let mut p = plan(&tree);
    p.std_manifest = Some(tree.0.join("std/hirsel.json"));
    let trap = Trap::new();
    let base = NamespaceLease::capture(&p, &trap, &LeaseBudget::new(16 << 20)).unwrap();
    trap.enabled.store(false, Ordering::SeqCst);
    let compiler = crate::Compiler::new(crate::compiler::tests::phases, [0; 32]);
    let config = || crate::CompileConfig {
        jobs: hirsel_intern::jobs::Jobs::new(1),
        ..crate::CompileConfig::for_memory_cap(None)
    };
    let _defaults_trap = crate::config::forbid_host_defaults();
    let loaded = base.load(&compiler, config(), false).unwrap();
    assert_eq!(loaded.entry.0.package.name, "standalone");
    assert_eq!(loaded.files.len(), 2);
    let sidecar = br#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#;
    let (child, _) = base
        .overlay(&[create(&base, "main.hir.json", sidecar)])
        .unwrap();
    std::fs::remove_dir_all(&tree.0).unwrap();
    let loaded = child.load(&compiler, config(), false).unwrap();
    assert_eq!(loaded.entry.0.package.name, "app");
    assert_eq!(loaded.files.len(), 1);
    let loaded = base.load(&compiler, config(), false).unwrap();
    assert_eq!(loaded.entry.0.package.name, "standalone");
    assert_eq!(loaded.files.len(), 2);
    let (grandchild, _) = child.overlay(&[delete(&child, "main.hir.json")]).unwrap();
    assert_eq!(
        grandchild
            .load(&compiler, config(), false)
            .unwrap()
            .entry
            .0
            .package
            .name,
        "standalone"
    );
    let (malformed, _) = child
        .overlay(&[replace(&child, "main.hir.json", b"{")])
        .unwrap();
    assert!(matches!(
        malformed.load(&compiler, config(), false),
        Err(crate::ToolError::Runtime(_))
    ));
}

#[test]
fn program_capture_rejects_invalid_selection_and_releases_reservation() {
    let tree = Tree::new();
    tree.write("main.hir", b"invalid source is repairable");
    tree.write("main.hir.json",br#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#);
    let p = plan(&tree);
    let budget = LeaseBudget::new(16 << 20);
    let compiler = crate::Compiler::new(crate::compiler::tests::phases, [0; 32]);
    let base = NamespaceLease::capture_program(
        &p,
        &DiskCapture,
        &budget,
        &compiler,
        crate::CompileConfig::for_memory_cap(None),
    )
    .unwrap();
    assert_eq!(budget.retained().1, 1);
    drop(base);
    assert_eq!(budget.retained(), (0, 0));
    tree.write("main.hir.json", b"{");
    assert!(matches!(
        NamespaceLease::capture_program(
            &p,
            &DiskCapture,
            &budget,
            &compiler,
            crate::CompileConfig::for_memory_cap(None)
        ),
        Err(CaptureError::InvalidSelection(_))
    ));
    assert_eq!(budget.retained(), (0, 0));
    tree.write("main.hir.json",br#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"missing.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#);
    assert!(matches!(
        NamespaceLease::capture_program(
            &p,
            &DiskCapture,
            &budget,
            &compiler,
            crate::CompileConfig::for_memory_cap(None)
        ),
        Err(CaptureError::InvalidSelection(_))
    ));
    assert_eq!(budget.retained(), (0, 0));
}
#[test]
fn missing_parent_no_mkdir_mount_boundaries_and_exact_limit() {
    let tree = Tree::new();
    tree.dir("nested/deep");
    tree.write("nested/deep/file.hir", b"x");
    tree.write("main.hir", b"main");
    let base = capture(&tree);
    let bad = Change::Create {
        key: key("missing/deep.hir"),
        absent: ContentHash::default(),
        bytes: Arc::from(&b"new"[..]),
    };
    assert_eq!(overlay_error(&base, &[bad]), OverlayError::MissingParent);
    assert!(!tree.0.join("missing").exists());
    let budget = LeaseBudget::new(16 << 20);
    let mut p = plan(&tree);
    p.limits.nodes = base.usage().nodes;
    assert!(NamespaceLease::capture(&p, &DiskCapture, &budget).is_ok());
    for path in ["nested", "nested/deep", "nested/deep/file.hir"] {
        assert_eq!(
            query(&base, &tree.0, path),
            query(&super::super::DiskInputs, &tree.0, path)
        );
    }
    p.roots.push(Root {
        id: "overlap".into(),
        path: tree.0.join("nested"),
        editable: false,
    });
    assert!(matches!(
        NamespaceLease::capture(&p, &DiskCapture, &budget),
        Err(CaptureError::InvalidPlan(_))
    ));
    // A read-only single-file root has no authority over sibling directory
    // membership or a missing leaf under its incomplete ancestor edges.
    let p = CapturePlan {
        roots: vec![Root {
            id: "file".into(),
            path: tree.0.join("main.hir"),
            editable: false,
        }],
        ..plan(&tree)
    };
    let single = NamespaceLease::capture(&p, &DiskCapture, &budget).unwrap();
    assert_eq!(&*single.read(&tree.0.join("main.hir")).unwrap(), b"main");
    unavailable_error(single.directory(&tree.0));
    unavailable_error(single.read(&tree.0.join("sibling.hir")));
    assert_eq!(
        overlay_error(
            &base,
            &[
                create(&base, "new.hir", b"new"),
                create(&base, "new.hir", b"new")
            ]
        ),
        OverlayError::DuplicateTarget
    );
}
