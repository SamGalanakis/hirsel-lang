//! A recording of a run that opens a std@1/store and runs transactions holds every byte
//! the store read (its objects, and through them every value a body saw), so it replays
//! exactly with the store's directory gone.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Each run counts itself in the store, aborts one transaction, and replays a stream
/// call it already made in an earlier run.
const PROGRAM: &str = r#"pub type AppError = Grant(GrantError) | Files(FileError) | Store(store.StoreError) | Output(OutputError)

fn runs(tx: store.Tx) -> u64 {
  match store.tx_get(tx, b"runs") {
    Some(b) => int.widen(b[0]),
    None() => 0,
  }
}

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  let d = io.dir(rt, path.of("data")) ? Files
  let mut s = store.open(d, store.default_config()) ? Store
  let counted: Result[u64, store.TxFailure[String]] = store.transact(
    mut s,
    fn(mut tx: store.Tx) -> Result[u64, String] {
      let n = runs(tx) + 1
      store.tx_put(mut tx, b"runs", bytes.from_list([int.truncate(n)]))
      Ok(n)
    }
  )
  let aborted: Result[u64, store.TxFailure[String]] = store.transact(
    mut s,
    fn(mut tx: store.Tx) -> Result[u64, String] {
      store.tx_put(mut tx, b"runs", b"\x00")
      Error("abort")
    }
  )
  let first = store.CallRequest(stream: b"s", epoch: 1, position: 1, digest: b"d", acknowledged: 0)
  let called: Result[u64, store.CallFailure[String]] = store.call(
    mut s,
    first,
    fn(mut tx: store.Tx) -> Result[u64, String] {
      Ok(runs(tx) * 10)
    }
  )
  store.close(take s) ? Store
  match (counted, aborted, called) {
    (Ok(n), Error(Aborted(_)), Ok(c)) => {
      out.print(output, u64.to_string(n)) ? Output
      out.print(output, " ") ? Output
      out.print(output, u64.to_string(c)) ? Output
      out.print(output, "\n") ? Output
    },
    _ => out.print(output, "unexpected\n") ? Output,
  }
  Ok(())
}
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("hirsel-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("main.hir"), PROGRAM).unwrap();
        let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        std::fs::write(
            root.join("main.hir.json"),
            serde_json::json!({"schema":1,"package":{"name":"stored","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("grants.json"),
            serde_json::json!({"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"data","host":root.join("data"),"access":"read_write"}]},"args":[]}).to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn run(&self, args: &[&std::ffi::OsStr]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .arg("run")
            .arg(self.0.join("main.hir.json"))
            .args(args)
            .output()
            .unwrap()
    }
    fn live(&self, tape: Option<&Path>) -> Output {
        let grants = self.0.join("grants.json");
        let mut args = vec!["--grants".as_ref(), grants.as_os_str()];
        if let Some(tape) = tape {
            args.extend(["--record".as_ref(), tape.as_os_str()]);
        }
        self.run(&args)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn assert_printed(output: &Output, expected: &str, what: &str) {
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{what}: {output:?}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected, "{what}");
}

#[test]
fn a_store_recording_replays_every_read_without_its_directory() {
    let fixture = Fixture::new("replay");
    assert_printed(&fixture.live(None), "1 10\n", "first live run");
    // The recorded run recovers the first run's objects, so its tape holds their bytes.
    let tape = fixture.0.join("run.tape");
    assert_printed(&fixture.live(Some(&tape)), "2 10\n", "recorded run");
    std::fs::rename(fixture.0.join("data"), fixture.0.join("gone")).unwrap();
    assert_printed(
        &fixture.run(&["--replay".as_ref(), tape.as_os_str()]),
        "2 10\n",
        "replay",
    );
}
