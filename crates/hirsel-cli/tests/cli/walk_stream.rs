//! End-to-end cursor record/replay, including several reads in one iter.next.
use std::{fs, path::Path, process::Command};

#[test]
fn walk_stream_host_symlinks_empty_directories_late_error_and_replay_without_tree() {
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
    // A later invalid UTF-8 filename makes the child read fail portably with
    // FileFault, independently of the invoking user's permission privileges.
    fs::create_dir(tree.join("zz")).unwrap();
    use std::os::unix::ffi::OsStringExt;
    fs::write(
        tree.join("zz")
            .join(std::ffi::OsString::from_vec(vec![255])),
        b"",
    )
    .unwrap();
    let source = root.join("main.hir");
    fs::write(
        &source,
        r#"pub type AppError = Grant(GrantError) | Files(FileError) | Output(OutputError)
pub fn main(rt: Io) -> Result[(), AppError] {
  let _ = io.tasks(rt)?
  let output = io.stdout(rt)?
  let d = io.dir(rt, path.of("tree"))?
  let mut paths = file.walk(d)?
  let mut errors = 0u64
  while true {
    match iter.next(mut paths) {
      None() => break,
      Some(Ok(p)) => { out.print(output, path.text(p))?; out.print(output, "\n")? },
      Some(Error(_)) => { errors += 1u64; out.print(output, "ERROR\n")? },
    }
  }
  failure.check(errors == 1u64, Bounds())
  failure.check(match iter.next(mut paths) { None() => true, _ => false }, Bounds())
  Ok(())
}
"#,
    )
    .unwrap();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()
        .unwrap();
    fs::write(source.with_extension("hir.json"), serde_json::json!({"schema":1,"package":{"name":"walk_stream","version":"0"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = root.join("grants.json");
    let free = std::env::var_os("HIRSEL_WALK_FREE").is_some();
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
    assert_eq!(
        record.stdout,
        b"a.c\na/b\nalias\nescape\nself\nx\ny\nERROR\n"
    );
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
