//! End-to-end cursor record/replay, including several reads in one iter.next.
use std::{fs, path::Path, process::Command};

#[test]
fn read_dir_stream_host_pages_early_drop_and_replay_without_tree() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let tree = root.join("tree");
    fs::create_dir_all(tree.join("a")).unwrap();
    fs::write(tree.join("a/b"), b"").unwrap();
    fs::write(tree.join("a.c"), b"").unwrap();
    fs::create_dir(tree.join("empty-0")).unwrap();
    fs::create_dir_all(tree.join("empty-1/deep")).unwrap();
    for (name, target) in [
        ("alias", "a"),
        ("self", "."),
        ("x", "y"),
        ("y", "x"),
        ("escape", "../outside"),
    ] {
        std::os::unix::fs::symlink(target, tree.join(name)).unwrap();
    }
    for n in 0..270 {
        fs::write(tree.join(format!("{n:04}")), b"").unwrap();
    }
    let source = root.join("main.hir");
    fs::write(
        &source,
        r#"pub type AppError = Grant(GrantError) | Files(FileError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let _ = io.tasks(rt)?
  let output = io.stdout(rt)?
  let d = io.dir(rt, path.of("tree"))?
  let mut first = file.read_dir(d, path.of(""))?
  for i in 0u64..10u64 {
    failure.check(match iter.next(mut first) { Some(Ok(_)) => true, _ => false }, Bounds())
  }
  let mut count = 0u64
  for item in take file.read_dir(d, path.of(""))? {
    let entry = item?
    if entry.kind == Regular() { count += 1u64 }
  }
  failure.check(count == 271u64, Bounds())
  out.print(output, "pages-ok\n")?
  Ok(())
}
"#,
    )
    .unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(source.with_extension("hir.json"), serde_json::json!({"schema":1,"package":{"name":"read_dir_stream","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    let free = std::env::var_os("HIRSEL_READ_DIR_FREE").is_some();
    let provider = if free {
        "tasks.free@1"
    } else {
        "tasks.cooperative@1"
    };
    fs::write(&grants, serde_json::json!({"io":{"stdout":"stdout.bytes@1","tasks":provider,"dirs":[{"path":"tree","host":tree,"access":"read"}]},"args":[]}).to_string()).unwrap();
    let tape = root.join("recording.tape");
    let record = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .args(["run", "--native"])
        .arg(&source)
        .arg("--grants")
        .arg(&grants)
        .args(["--cores", "8", "--record"])
        .arg(&tape)
        .output()
        .unwrap();
    assert!(record.status.success(), "{record:?}");
    assert_eq!(record.stdout, b"pages-ok\n");
    if free {
        assert!(fs::read(&tape).unwrap().starts_with(b"hirsel-free-tape 2"));
    }
    fs::rename(&tree, root.join("unavailable")).unwrap();
    let replay = Command::new(env!("CARGO_BIN_EXE_hirsel"))
        .arg("run")
        .arg(&source)
        .arg("--replay")
        .arg(&tape)
        .args(["--cores", "1"])
        .output()
        .unwrap();
    assert!(replay.status.success(), "{replay:?}");
    assert_eq!(replay.stdout, record.stdout);
}
