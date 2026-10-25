//! Full-entry file errors. Install/body/check separation follows SQLite's public-domain
//! test/malloc_common.tcl:107-157 and test/ioerr.test:29-46 (014644d8).
//! No SQL/pager semantics: only Hirsel's documented file effect boundaries.
//! Upstream SQLite notice: The author disclaims copyright to this source code.
//! In place of a legal notice, here is a blessing:
//! May you do good and not evil.
//! May you find forgiveness for yourself and forgive others.
//! May you share freely, never taking more than you give.
// Hardware backends qualified by native_file_faults_33; these vectors port the
// same finite typed semantics to the independent lowered Heap provider.
use hirsel_driver::native::*;
use hirsel_engine::{RunConfiguration, run_observed_snapshot, with_compiler_stack};
use hirsel_run_schema::{Access, FileFault, FileFaultError as E, FileFaults, FileOperation as O};
use std::collections::{BTreeMap, VecDeque};
fn step(requested: Option<u64>, error: Option<E>, partial: u64) -> FileFault {
    FileFault {
        requested,
        error,
        partial,
    }
}
fn script(op: O, steps: Vec<FileFault>) -> FileFaults {
    BTreeMap::from([(op, VecDeque::from(steps))])
}
const HELPERS: &str = r#"
@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;
fn main_source(body: &str) -> String {
    format!(
        r#"pub fn main(rt: Io, args: Args) -> Result[(), FileError] {{
        let input = io.dir(rt, arg.path(args, 0u64)? argument_error)?
        let output = io.dir(rt, arg.path(args, 1u64)? argument_error)?
        {body}
        let _ = out.print(io.stdout(rt)? grant_error, "file faults checked\n")
        Ok(())
    }} {HELPERS}"#
    )
}
fn campaign(id: &str, source: &str, fixture: Fixture, allocation_campaign: bool) {
    campaign_status(id, source, fixture, allocation_campaign, 0);
}
fn campaign_status(id: &str, source: &str, fixture: Fixture, allocation_campaign: bool, exit: i32) {
    with_compiler_stack(|| {
        let scratch = tempfile::tempdir().unwrap();
        let root = std::env::var_os("HIRSEL_HEAP_FAULTS_EVIDENCE")
            .map(std::path::PathBuf::from)
            .map_or_else(|| scratch.path().join(id), |p| p.join(id));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("main.hir");
        std::fs::write(&path, source).unwrap();
        std::fs::write(
            root.join("faults.json"),
            serde_json::to_vec_pretty(&fixture.file_faults).unwrap(),
        )
        .unwrap();
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded
            .check_for_execution()
            .unwrap_or_else(|e| panic!("{e}"));
        let run = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        eprintln!("HEAP_FILE_FAULT_STARTED {id} reference");
        let reference = run_observed_snapshot(&mut loaded.engine, &run, (&fixture).into()).unwrap();
        let observed = reference.observation.as_ref().unwrap();
        let expected = Observation::of_reference(observed);
        assert_eq!(expected.exit_code, Some(exit), "{expected:?}");
        assert!(expected.failures.is_empty(), "{expected:?}");
        assert_eq!(expected.output, b"file faults checked\n");
        let files = observed
            .final_state
            .as_ref()
            .unwrap()
            .trees
            .values()
            .flat_map(|(_, files)| files.clone())
            .collect::<BTreeMap<_, _>>();
        eprintln!("HEAP_FILE_FAULT_PASS {id} reference");
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let ready = build(&validated, &options).unwrap();
        let sites = Sites::new(validated.snapshot());
        let world = HeapWorld {
            args: fixture.args.clone(),
            files: fixture.files.clone(),
            mounts: fixture.mounts.clone(),
            file_reads: fixture.file_reads.clone(),
            file_faults: fixture.file_faults.clone(),
            ..Default::default()
        };
        eprintln!("HEAP_FILE_FAULT_STARTED {id} heap");
        let heap = run_heap_in(&ready, &loaded.entry, &sites, &world).unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.files, Some(files));
        assert_eq!(heap.file_handles, Some(vec![]));
        eprintln!("HEAP_FILE_FAULT_PASS {id} heap files+handles");
        // Allocation perturbations remain distinct from scripted I/O errors.
        // The campaign's factory clones the initial queues for every variant.
        if allocation_campaign {
            eprintln!("HEAP_FILE_FAULT_STARTED {id} allocation-campaign");
            let audited = heap_campaign_in(&ready, &loaded.entry, &sites, &world).unwrap();
            assert_eq!(audited.observation, expected);
            eprintln!(
                "HEAP_FILE_FAULT_PASS {id} allocation-campaign sites={} choices={}",
                audited.sites, audited.choices
            );
        }
    });
}
fn fixture(faults: FileFaults) -> Fixture {
    Fixture {
        args: vec!["input".into(), "output".into()],
        files: BTreeMap::from([
            ("input/a".into(), b"abc".to_vec()),
            ("output/old".into(), b"old".to_vec()),
        ]),
        mounts: BTreeMap::from([
            ("input".into(), Access::Read),
            ("output".into(), Access::ReadWrite),
        ]),
        file_faults: faults,
        ..Default::default()
    }
}
#[test]
fn read_before_and_after_prefix() {
    campaign(
        "read",
        &main_source(
            r#"
        let mut f = file.open(input, path.of("a"))?
        let mut bytes: List[u8] = []
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(FileFault(23u32)), Arithmetic())
        failure.check(bytes == [], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Ok(2u64), Arithmetic())
        failure.check(bytes == [97u8,98u8], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(NotFound()), Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Error(Canceled(DeadlineExceeded())), Arithmetic())
        failure.check(bytes == [97u8,98u8], Arithmetic())
        failure.check(file.read(input, mut f, mut bytes, 2u64) == Ok(1u64), Arithmetic())
        failure.check(bytes == [97u8,98u8,99u8], Arithmetic())
    "#,
        ),
        fixture(script(
            O::Read,
            vec![
                step(Some(2), Some(E::Fault(23)), 0),
                step(Some(2), None, 0),
                step(Some(2), Some(E::NotFound), 0),
                step(Some(2), Some(E::Canceled(3)), 0),
            ],
        )),
        false,
    );
}
#[test]
fn atomic_prefix_drop_and_commit_refusal() {
    campaign(
        "atomic",
        &main_source(
            r#"
        { let mut f = file.create_atomic(output, path.of("old"))?
          failure.check(file.write_all(output, mut f, b"new") == Error(FileFault(7u32)), Arithmetic())
          let mut saw_prefix = false
          for p in take file.walk(output)? {
            if file.read_bytes(output, p?)? == b"ne" { saw_prefix = true }
          }
          failure.check(saw_prefix, Arithmetic()) }
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        let mut f = file.create_atomic(output, path.of("old"))?
        file.write_all(output, mut f, b"new")?
        failure.check(file.commit(output, take f) == Error(PermissionDenied()), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        let mut count = 0u64
        for entry in take file.walk(output)? { let _ = entry?; count = count + 1u64 }
        failure.check(count == 1u64, Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::WriteAll,
                VecDeque::from([step(Some(3), Some(E::Fault(7)), 2)]),
            ),
            (
                O::Commit,
                VecDeque::from([step(None, Some(E::PermissionDenied), 0)]),
            ),
        ])),
        false,
    );
}
#[test]
fn put_new_error_and_existing_paths() {
    campaign(
        "put-new",
        &main_source(
            r#"
        failure.check(file.put_new(output, path.of("new"), b"new", Durable()) == Error(FileFault(7u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new")) == Error(NotFound()), Arithmetic())
        file.put_new(output, path.of("old"), b"old", Durable())?
        failure.check(file.put_new(output, path.of("old"), b"different", Durable()) == Error(AlreadyExists()), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
        file.put_new(output, path.of("new"), b"new", Durable())?
        failure.check(file.read_bytes(output, path.of("new"))? == b"new", Arithmetic())
    "#,
        ),
        fixture(script(O::PutNew, vec![step(Some(3), Some(E::Fault(7)), 0)])),
        false,
    );
}
#[test]
fn append_partial_sync_and_cancellation() {
    campaign(
        "append",
        &main_source(
            r#"
        failure.check(file.write_bytes(input, path.of("denied"), b"abc") == Error(NotGranted()), Arithmetic())
        let mut f = file.open_append(output, path.of("old"))?
        failure.check(file.append(output, mut f, b"xyz") == Error(FileFault(7u32)), Arithmetic())
        failure.check(file.sync(output, mut f) == Error(FileFault(8u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"oldxy", Arithmetic())
        failure.check(file.write_bytes(output, path.of("old"), b"abc") == Error(Canceled(Parent())), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"oldxy", Arithmetic())
        failure.check(file.write_bytes(output, path.of("old"), b"abc") == Error(FileFault(9u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"a", Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::Append,
                VecDeque::from([step(Some(3), Some(E::Fault(7)), 2)]),
            ),
            (O::Sync, VecDeque::from([step(None, Some(E::Fault(8)), 0)])),
            (
                O::WriteBytes,
                VecDeque::from([
                    step(Some(3), Some(E::Canceled(1)), 0),
                    step(Some(3), Some(E::Fault(9)), 1),
                ]),
            ),
        ])),
        false,
    );
}

const PENDING: &str = r#"
resource type Joined { dir: Dir }
drop(x) { let _ = file.write_bytes(x.dir, path.of("joined"), b"yes") }
pub fn main(rt: Io, args: Args) -> Result[(), FileError] {
  failure.check(arg.len(args) == 4u64, Arithmetic())
  failure.check(arg.text(args, 2u64) == Ok("--hirsel-run"), Arithmetic())
  failure.check(arg.text(args, 3u64) == Ok("羊"), Arithmetic())
  failure.check(arg.text(args, 4u64) == Error(MissingArgument()), Arithmetic())
  let tasks = io.tasks(rt)? grant_error
  let d = io.dir(rt, arg.path(args, 0u64)? argument_error)?
  let output = io.dir(rt, arg.path(args, 1u64)? argument_error)?
  failure.check(file.write_bytes(d, path.of("denied"), b"no") == Error(NotGranted()), Arithmetic())
  failure.check(match io.dir(rt, path.of("ungranted")) { Error(NotGranted()) => true, _ => false }, Arithmetic())
  let problem = task.scope(tasks, fn(s: Scope) -> bool {
    let mut pending: TaskSet[Result[(), FileError]] of s = task.set(s)
    task.add(s, mut pending, task.async(s, fn() -> Result[(), FileError] {
      let guard = Joined(dir: output)
      let mut f = file.open(d, path.of("a"))?
      let mut bytes: List[u8] = []
      match file.read(d, mut f, mut bytes, 1u64) {
        Error(Canceled(Requested())) => {
          failure.check(list.len(bytes) == 0u64, Arithmetic())
          Ok(())
        },
        _ => { failure.check(false, Arithmetic()); Ok(()) },
      }
    }))
    task.add(s, mut pending, task.async(s, fn() -> Result[(), FileError] {
      let info = file.metadata(d, path.of("b"))?
      failure.check(info.kind == Regular(), Arithmetic())
      let mut f = file.open(d, path.of("b"))?
      let mut bytes: List[u8] = []
      let n = file.read(d, mut f, mut bytes, 1u64)?
      if n != 1u64 { Error(FileFault(7u32)) } else { Ok(()) }
    }))
    let slot = match task.await_any(s, pending) { Some(i) => i, None => 99u64 }
    failure.check(slot == 1u64, Arithmetic())
    let problem = match task.await_at(s, mut pending, slot) {
      Error(FileFault(7u32)) => true,
      _ => false,
    }
    task.cancel_all(s, take pending)
    if !problem { let _ = file.write_bytes(output, path.of("c"), b"queued") }
    problem
  })
  failure.check(problem, Arithmetic())
  failure.check(file.read_bytes(output, path.of("joined"))? == b"yes", Arithmetic())
  failure.check(file.read_bytes(output, path.of("c")) == Error(NotFound()), Arithmetic())
  let _ = out.print(io.stdout(rt)? grant_error, "file faults checked\n")
  Ok(())
}

@noalloc
fn grant_error(e: GrantError) -> FileError uses {} { NotGranted() }
@noalloc
fn argument_error(e: ArgError) -> FileError uses {} { FileFault(9u32) }
"#;
#[test]
fn pending_does_not_consume_fault_before_sibling_and_cancel() {
    let mut recipe = fixture(script(O::Read, vec![step(Some(1), Some(E::Fault(7)), 0)]));
    recipe.args.extend(["--hirsel-run".into(), "羊".into()]);
    recipe.files.insert("input/b".into(), vec![]);
    recipe
        .file_reads
        .insert("input/a".into(), hirsel_run_schema::FileReadScript::Pending);
    campaign("pending", PENDING, recipe, false);
}

#[test]
fn acquisitions_and_stream_write_errors() {
    campaign(
        "acquire-write",
        &main_source(
            r#"
        failure.check(match file.open(input, path.of("a")) { Error(PermissionDenied()) => true, _ => false }, Arithmetic())
        failure.check(file.read_bytes(input, path.of("a")) == Error(FileFault(5u32)), Arithmetic())
        failure.check(match file.create(output, path.of("new")) { Error(Canceled(ChildFailed())) => true, _ => false }, Arithmetic())
        failure.check(match file.open_append(output, path.of("old")) { Error(NotDirectory()) => true, _ => false }, Arithmetic())
        failure.check(match file.create_atomic(output, path.of("old")) { Error(CrossDevice()) => true, _ => false }, Arithmetic())
        let mut f = file.create(output, path.of("new"))?
        failure.check(file.write(output, mut f, b"xyz") == Error(FileFault(42u32)), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new"))? == b"xy", Arithmetic())
        failure.check(file.write(output, mut f, b"z") == Ok(1u64), Arithmetic())
        failure.check(file.read_bytes(output, path.of("new"))? == b"xyz", Arithmetic())
    "#,
        ),
        fixture(BTreeMap::from([
            (
                O::Open,
                VecDeque::from([step(None, Some(E::PermissionDenied), 0)]),
            ),
            (
                O::ReadBytes,
                VecDeque::from([step(None, Some(E::Fault(5)), 0)]),
            ),
            (
                O::Create,
                VecDeque::from([step(None, Some(E::Canceled(2)), 0)]),
            ),
            (
                O::OpenAppend,
                VecDeque::from([step(None, Some(E::NotDirectory), 0)]),
            ),
            (
                O::CreateAtomic,
                VecDeque::from([step(None, Some(E::CrossDevice), 0)]),
            ),
            (
                O::Write,
                VecDeque::from([step(Some(3), Some(E::Fault(42)), 2)]),
            ),
        ])),
        false,
    );
}

#[test]
fn zero_partial_full_pass_then_fail_and_empty_queues() {
    for partial in [0, 1, 3] {
        let prefix = &"xyz"[..partial];
        let source = main_source(&format!(
            r#"
            failure.check(file.write_bytes(output, path.of("old"), b"xyz") == Error(FileFault(71u32)), Arithmetic())
            failure.check(file.read_bytes(output, path.of("old"))? == b"{}", Arithmetic())
            file.write_bytes(output, path.of("old"), b"xyz")?
            failure.check(file.write_bytes(output, path.of("old"), b"abc") == Error(PermissionDenied()), Arithmetic())
            failure.check(file.read_bytes(output, path.of("old"))? == b"xyz", Arithmetic())
            file.write_bytes(output, path.of("old"), b"end")?
            failure.check(file.read_bytes(output, path.of("old"))? == b"end", Arithmetic())
        "#,
            if partial == 0 { "old" } else { prefix }
        ));
        campaign(
            &format!("prefix-{partial}"),
            &source,
            fixture(script(
                O::WriteBytes,
                vec![
                    step(Some(3), Some(E::Fault(71)), partial as u64),
                    step(Some(3), None, 0),
                    step(Some(3), Some(E::PermissionDenied), 0),
                ],
            )),
            partial == 1,
        );
    }
    campaign(
        "empty-queues",
        &main_source(
            r#"
        let mut f = file.open_append(output, path.of("old"))?
        file.append(output, mut f, b"x")?
        file.sync(output, mut f)?
        failure.check(file.read_bytes(output, path.of("old"))? == b"oldx", Arithmetic())
    "#,
        ),
        fixture(Default::default()),
        false,
    );
}

#[test]
fn tiny_commit_denied_and_affected_native_build() {
    let source = main_source(
        r#"
        let mut f = file.create_atomic(output, path.of("old"))?
        file.write_all(output, mut f, b"new")?
        failure.check(file.commit(output, take f) == Error(PermissionDenied()), Arithmetic())
        failure.check(file.read_bytes(output, path.of("old"))? == b"old", Arithmetic())
    "#,
    );
    let recipe = fixture(script(
        O::Commit,
        vec![step(None, Some(E::PermissionDenied), 0)],
    ));
    campaign("tiny-commit-denied", &source, recipe.clone(), false);
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(&path, source).unwrap();
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let matrix = [Configuration {
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine: Engine::Native {
                opt: OptLevel::None,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        }];
        let tools = Toolchain::for_matrix(dir.path(), &matrix).unwrap();
        let expected = Observation {
            exit_code: Some(0),
            blocked: false,
            budget: None,
            signaled: None,
            output: b"file faults checked\n".to_vec(),
            failures: vec![],
            roots: vec![],
        };
        eprintln!("HEAP_FILE_FAULT_STARTED tiny-commit-denied llvm-build");
        for (name, result) in differential_in(
            &validated,
            &loaded.entry,
            &expected,
            &matrix,
            &tools,
            dir.path(),
            &recipe,
        ) {
            assert!(matches!(result, Outcome::Agree), "{name}: {result:?}");
        }
        eprintln!("HEAP_FILE_FAULT_PASS tiny-commit-denied llvm-build");
    });
}

#[test]
#[ignore = "original full-main recovery, run once after provider qualification"]
fn treelearn_original_commit_denied() {
    // Preserve the original full-main 60 CPU-s / 2 GiB / 120 s cell caps.
    // Setup/build is supplied by Buck; only the runtime child is capped here.
    if std::env::var_os("HIRSEL_HEAP_TREELEARN_CHILD").is_none() {
        let cap = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(cap.path(), include_str!("../../../../tools/capped")).unwrap();
        let child = std::process::Command::new("python3")
            .arg(cap.path())
            .args(["--timeout", "120", "prlimit", "--cpu=60:61"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "heap_file_faults_38::treelearn_original_commit_denied",
                "--ignored",
                "--nocapture",
            ])
            .env("HIRSEL_MEMORY_LIMIT", "2G")
            .env("HIRSEL_HEAP_TREELEARN_CHILD", "1")
            .output()
            .unwrap();
        eprintln!(
            "{}{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            child.status.success(),
            "original capped cell: {:?}",
            child.status
        );
        return;
    }
    with_compiler_stack(|| {
        let root = tempfile::tempdir().unwrap();
        for (name, source) in [
            (
                "main.hir",
                include_str!("../../../../examples/treelearn/main.hir"),
            ),
            (
                "dataset.hir",
                include_str!("../../../../examples/treelearn/dataset.hir"),
            ),
            (
                "split.hir",
                include_str!("../../../../examples/treelearn/split.hir"),
            ),
            (
                "train.hir",
                include_str!("../../../../examples/treelearn/train.hir"),
            ),
            (
                "predict.hir",
                include_str!("../../../../examples/treelearn/predict.hir"),
            ),
            (
                "store.hir",
                include_str!("../../../../examples/treelearn/store.hir"),
            ),
        ] {
            std::fs::write(root.path().join(name), source).unwrap();
        }
        let mut manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../../examples/treelearn/hirsel.json"))
                .unwrap();
        manifest["dependencies"][0]["manifest"] = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
            .into();
        let path = root.path().join("hirsel.json");
        std::fs::write(&path, manifest.to_string()).unwrap();
        let rows: serde_json::Value = serde_json::from_str(include_str!(
            "../../../hirsel-cli/tests/support/decision_tree_35/cases.json"
        ))
        .unwrap();
        let row = rows["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "commit-denied")
            .unwrap();
        let files = BTreeMap::from([
            (
                "input/workflow.json".into(),
                row["input"].as_str().unwrap().as_bytes().to_vec(),
            ),
            ("input/model.json".into(), b"old\n".to_vec()),
        ]);
        let world = HeapWorld {
            args: vec![
                "train".into(),
                "input".into(),
                "workflow.json".into(),
                "model.json".into(),
                "4".into(),
            ],
            files: files.clone(),
            mounts: BTreeMap::from([("input".into(), Access::ReadWrite)]),
            file_faults: script(O::Commit, vec![step(None, Some(E::PermissionDenied), 0)]),
            ..Default::default()
        };
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        eprintln!("HEAP_FILE_FAULT_STARTED treelearn-original-commit-denied heap");
        let heap = run_heap_in(
            &ready,
            &loaded.entry,
            &Sites::new(validated.snapshot()),
            &world,
        )
        .unwrap();
        assert_eq!(
            heap.observation,
            Observation {
                exit_code: Some(2),
                blocked: false,
                budget: None,
                signaled: None,
                output: vec![],
                failures: vec![],
                roots: vec![]
            }
        );
        assert_eq!(heap.files, Some(files));
        assert_eq!(heap.file_handles, Some(vec![]));
        eprintln!(
            "HEAP_FILE_FAULT_PASS treelearn-original-commit-denied heap no-publication+handles"
        );
    });
}

#[test]
fn file_error_status_matches_reference() {
    let source = main_source(r#"
        failure.check(file.put_new(output, path.of("new"), b"new", Durable()) == Error(FileFault(7u32)), Arithmetic())
    "#).replace("Ok(())", "Error(FileFault(7u32))");
    campaign_status(
        "error-status",
        &source,
        fixture(script(O::PutNew, vec![step(Some(3), Some(E::Fault(7)), 0)])),
        false,
        1,
    );
}

#[test]
fn all_portable_errors_and_cancel_causes() {
    let mut body = String::new();
    let mut faults = Vec::new();
    for (error, spelling) in [
        (E::NotFound, "NotFound()"),
        (E::PermissionDenied, "PermissionDenied()"),
        (E::AlreadyExists, "AlreadyExists()"),
        (E::IsDirectory, "IsDirectory()"),
        (E::NotDirectory, "NotDirectory()"),
        (E::DirectoryNotEmpty, "DirectoryNotEmpty()"),
        (E::CrossDevice, "CrossDevice()"),
        (E::Canceled(0), "Canceled(Requested())"),
        (E::Canceled(1), "Canceled(Parent())"),
        (E::Canceled(2), "Canceled(ChildFailed())"),
        (E::Canceled(3), "Canceled(DeadlineExceeded())"),
    ] {
        body.push_str(&format!("failure.check(match file.open(input, path.of(\"a\")) {{ Error({spelling}) => true, _ => false }}, Arithmetic())\n"));
        faults.push(step(None, Some(error), 0));
    }
    campaign(
        "portable-errors",
        &main_source(&body),
        fixture(script(O::Open, faults)),
        false,
    );
}

#[test]
fn heap_launch_refuses_invalid_scripts_and_counts() {
    with_compiler_stack(|| {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.hir");
        std::fs::write(
            &path,
            main_source("file.write_bytes(output, path.of(\"old\"), b\"xyz\")?"),
        )
        .unwrap();
        let mut loaded = hirsel_driver::load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        // Recovery may select only the count-mismatch cell after the eight
        // unchanged schema cells passed. Normal tests always run the full matrix.
        let first = std::env::var("HIRSEL_HEAP_FAULTS_ADMISSION_FROM")
            .map_or(0, |v| v.parse::<usize>().unwrap());
        assert!(matches!(first, 0 | 8));
        for (faults, diagnostic) in [
            (script(O::Read, vec![]), "empty file fault queue"),
            (
                script(O::Read, vec![step(None, Some(E::Fault(7)), 0)]),
                "expected request count",
            ),
            (
                script(O::Commit, vec![step(Some(1), Some(E::Fault(7)), 0)]),
                "expected request count",
            ),
            (
                script(
                    O::WriteBytes,
                    vec![step(Some(3), None, 0), step(Some(3), Some(E::Fault(7)), 4)],
                ),
                "unsupported partial file fault",
            ),
            (
                script(O::Read, vec![step(Some(2), Some(E::Fault(7)), 1)]),
                "unsupported partial file fault",
            ),
            (
                script(O::PutNew, vec![step(Some(2), Some(E::Fault(7)), 1)]),
                "unsupported partial file fault",
            ),
            (
                script(O::Append, vec![step(Some(2), Some(E::Canceled(1)), 1)]),
                "unsupported partial file fault",
            ),
            (
                script(O::Open, vec![step(None, Some(E::Canceled(4)), 0)]),
                "invalid cancellation cause",
            ),
            (
                script(O::WriteBytes, vec![step(Some(1), Some(E::Fault(7)), 1)]),
                "Unsupported",
            ),
        ]
        .into_iter()
        .skip(first)
        {
            let recipe = fixture(faults);
            let run = RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            };
            let reference = run_observed_snapshot(&mut loaded.engine, &run, (&recipe).into());
            if diagnostic == "Unsupported" {
                let reference = reference.unwrap();
                let error = reference
                    .outcome
                    .err()
                    .expect("mismatched count accepted by reference");
                assert!(
                    format!("{error:?}").contains("file fault request count mismatch"),
                    "{error:?}"
                );
                assert!(reference.observation.is_none());
            } else {
                assert!(
                    reference.is_err(),
                    "malformed fixture accepted by reference"
                );
            }
            let world = HeapWorld {
                args: recipe.args,
                files: recipe.files,
                mounts: recipe.mounts,
                file_faults: recipe.file_faults,
                ..Default::default()
            };
            let error = run_heap_in(
                &ready,
                &loaded.entry,
                &Sites::new(validated.snapshot()),
                &world,
            )
            .err()
            .expect("malformed fixture accepted by Heap");
            assert!(error.to_string().contains(diagnostic), "{error}");
            eprintln!("HEAP_FILE_FAULT_PASS admission reference+heap {diagnostic}");
        }
    });
}
