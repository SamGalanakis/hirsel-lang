use crate::{index::*, pack::index_benchmark_epoch};
use hirsel_syntax::ContentHash;
use std::os::{
    fd::AsRawFd,
    unix::net::{UnixListener, UnixStream},
};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};
fn entry(n: u64) -> Entry {
    let mut d = [0; 32];
    d[..8].copy_from_slice(&n.to_be_bytes());
    Entry {
        key: Key {
            kind: KeyKind::Content,
            digest: ContentHash(d),
        },
        location: Location {
            segment: ContentHash([n as u8; 32]),
            offset: 64,
            len: 128,
        },
    }
}
fn write(root: &Path, entries: &[Entry]) {
    let outcome = publish_delta(
        root,
        build(root, entries, crate::Durability::Rebuildable).unwrap(),
        &index_benchmark_epoch(root).unwrap(),
    )
    .unwrap();
    assert!(
        matches!(outcome, PublishOutcome::Published { .. }),
        "{outcome:?}"
    );
}
fn budget() -> MaintenanceBudget {
    MaintenanceBudget {
        max_input_bytes: 128 << 20,
        max_rows: 2_000_000,
        max_scratch_bytes: 256 << 20,
    }
}
// Go os/exec/exec.go:936-974 owns the wait and pipe-copy completion together;
// exec_test.go:470-521 checks each exchange even when parents run in parallel.
struct TestChild {
    process: std::process::Child,
    readers: [Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>; 2],
    reaped: bool,
}
impl TestChild {
    fn spawn(command: &mut Command) -> Self {
        let process = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = Self {
            process,
            readers: [None, None],
            reaped: false,
        };
        let pipes: [Box<dyn Read + Send>; 2] = [
            Box::new(child.process.stdout.take().unwrap()),
            Box::new(child.process.stderr.take().unwrap()),
        ];
        for (reader, mut pipe) in child.readers.iter_mut().zip(pipes) {
            *reader = Some(std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes)?;
                Ok(bytes)
            }));
        }
        child
    }
    fn finish(mut self) {
        let status = self.process.wait().unwrap();
        self.reaped = true;
        let [out, err] = self
            .readers
            .each_mut()
            .map(|reader| reader.take().unwrap().join().unwrap().unwrap());
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        assert!(status.success(), "child {status}:\n{out}\n{err}");
        assert!(
            out.contains("test result: ok. 1 passed; 0 failed;"),
            "child must execute exactly one test body:\n{out}\n{err}"
        );
    }
}
impl Drop for TestChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
        for reader in &mut self.readers {
            if let Some(reader) = reader.take() {
                let _ = reader.join();
            }
        }
    }
}
fn test_command(name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--nocapture", "--test-threads=1"]);
    command
}
fn child(root: &Path, mode: &str, id: u64) -> TestChild {
    TestChild::spawn(
        test_command("index::tests::races::cooperating_child")
            .arg("--ignored")
            .env("INDEX36_CHILD_ROOT", root)
            .env("INDEX36_CHILD_MODE", mode)
            .env("INDEX36_CHILD_ID", id.to_string()),
    )
}
fn accept(listener: &UnixListener) -> UnixStream {
    let mut p = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid stack pollfd and a finite timeout.
    assert_eq!(
        unsafe { libc::poll(&mut p, 1, 5000) },
        1,
        "child did not connect"
    );
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    stream
}
#[test]
#[ignore = "only launched by the bounded multiprocess parent"]
fn cooperating_child() {
    let root = std::path::PathBuf::from(std::env::var("INDEX36_CHILD_ROOT").unwrap());
    let mode = std::env::var("INDEX36_CHILD_MODE").unwrap();
    let id: u64 = std::env::var("INDEX36_CHILD_ID").unwrap().parse().unwrap();
    if mode == "append" {
        write(&root, &[entry(id)]);
        return;
    }
    if mode.starts_with("gc-") {
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join(".pack-lock"))
            .unwrap();
        // SAFETY: valid owned fd, finite nonblocking lock attempt.
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if mode == "gc-reader-gap" {
            assert_eq!(result, -1);
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
        } else {
            assert_eq!(result, 0);
            let live: Vec<_> = current_layer_ids_locked(&root)
                .unwrap()
                .iter()
                .map(|id| {
                    let mut name = String::from("L-");
                    use std::fmt::Write;
                    for byte in id.0 {
                        write!(name, "{byte:02x}").unwrap();
                    }
                    name
                })
                .collect();
            for path in fs::read_dir(root.join("index")).unwrap().flatten() {
                let name = path.file_name().to_string_lossy().into_owned();
                if name.starts_with("L-") && !live.contains(&name) {
                    fs::remove_file(path.path()).unwrap();
                }
            }
        }
        return;
    }
    let stale = load_pinned(&root).unwrap();
    let mut record = entry(if mode == "overwrite" { 1 } else { id });
    if mode == "overwrite" {
        record.location.offset = id as u32 * 64;
    }
    let epoch = index_benchmark_epoch(&root).unwrap();
    let delta = build(&root, &[record], crate::Durability::Rebuildable).unwrap();
    let mut socket = UnixStream::connect(root.join("barrier")).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    socket.write_all(&[id as u8]).unwrap();
    let mut go = [0];
    socket.read_exact(&mut go).unwrap();
    assert_eq!(go, [1]);
    assert_eq!(find(&stale, record.key), None);
    assert!(matches!(
        publish_delta(&root, delta, &epoch).unwrap(),
        PublishOutcome::Published { .. }
    ));
    socket.write_all(&[2]).unwrap();
}
#[test]
fn multiprocess_common_pin_publication_and_live_staging_gc_owner() {
    if !parent_body("multiprocess_common_pin_publication_and_live_staging_gc_owner") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = UnixListener::bind(root.join("barrier")).unwrap();
    let a = child(root, "barrier", 1);
    let b = child(root, "barrier", 2);
    let mut connections = [accept(&listener), accept(&listener)];
    let mut seen = Vec::new();
    for socket in &mut connections {
        let mut id = [0];
        socket.read_exact(&mut id).unwrap();
        seen.push(id[0]);
    }
    seen.sort();
    assert_eq!(seen, [1, 2]);
    // A cooperating GC holds publication exclusively, and only try-locks owners.
    let gc = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".pack-lock"))
        .unwrap();
    // SAFETY: valid owned fd; nonblocking flock has no pointers.
    assert_eq!(
        unsafe { libc::flock(gc.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let stages: Vec<_> = fs::read_dir(root.join("index/staging"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(stages.len(), 2);
    for stage in &stages {
        let owner = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(stage.join("owner.lock"))
            .unwrap();
        // SAFETY: valid separate fd, nonblocking lock only.
        assert_eq!(
            unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            -1
        );
        assert!(stage.join("layer").is_file());
    }
    drop(gc);
    // Serialize publication while both children retain the same absent pin.
    for socket in &mut connections {
        socket.write_all(&[1]).unwrap();
        let mut done = [0];
        socket.read_exact(&mut done).unwrap();
        assert_eq!(done, [2]);
    }
    a.finish();
    b.finish();
    let pin = load_pinned(root).unwrap();
    for id in [1, 2] {
        assert_eq!(find(&pin, entry(id).key), Some(entry(id).location));
    }
    assert_eq!(fs::read_dir(root.join("index/staging")).unwrap().count(), 0);
}
#[test]
fn multiprocess_append_survives_off_lock_compaction() {
    if !parent_body("multiprocess_append_survives_off_lock_compaction") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    write(root, &[entry(2)]);
    let path = root.to_owned();
    observe_compaction(move || {
        child(&path, "append", 3).finish();
    });
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 2, .. }
    ));
    let pin = load_pinned(root).unwrap();
    for n in [1, 2, 3] {
        assert_eq!(find(&pin, entry(n).key), Some(entry(n).location));
    }
}
#[test]
fn changed_compaction_inputs_conflict_once_without_overwriting_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    let old = fs::read(root.join("index/MANIFEST")).unwrap();
    write(root, &[entry(2)]);
    let path = root.to_owned();
    let expected = old.clone();
    observe_compaction(move || fs::write(path.join("index/MANIFEST"), old).unwrap());
    assert_eq!(compact(root, budget()).unwrap(), CompactOutcome::Conflict);
    assert_eq!(fs::read(root.join("index/MANIFEST")).unwrap(), expected);
}
#[test]
fn fully_shadowed_inputs_reuse_selected_inode_and_free_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    write(root, &[entry(1), entry(2)]);
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
    let pin = load_pinned(root).unwrap();
    for n in [1, 2] {
        assert_eq!(find(&pin, entry(n).key), Some(entry(n).location));
    }
}
#[test]
fn concurrently_appended_identical_output_cannot_create_duplicate_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    write(root, &[entry(2)]);
    let path = root.to_owned();
    observe_compaction(move || write(&path, &[entry(1), entry(2)]));
    assert_eq!(compact(root, budget()).unwrap(), CompactOutcome::Conflict);
    assert_eq!(current_layer_ids_locked(root).unwrap().len(), 3);
    let pin = load_pinned(root).unwrap();
    for n in [1, 2] {
        assert_eq!(find(&pin, entry(n).key), Some(entry(n).location));
    }
}

#[test]
fn multiprocess_gc_cannot_enter_reader_open_gap_and_unlinked_pin_survives() {
    if !parent_body("multiprocess_gc_cannot_enter_reader_open_gap_and_unlinked_pin_survives") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    let path = root.to_owned();
    observe_pin(move || {
        child(&path, "gc-reader-gap", 0).finish();
    });
    let old = load_pinned(root).unwrap();
    write(root, &[entry(2)]);
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 1, .. }
    ));
    child(root, "gc-unlisted", 0).finish();
    assert_eq!(
        fs::read_dir(root.join("index"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("L-"))
            .count(),
        1
    );
    assert_eq!(find(&old, entry(1).key), Some(entry(1).location));
    assert_eq!(find(&old, entry(2).key), None);
    let current = load_pinned(root).unwrap();
    for id in [1, 2] {
        assert_eq!(find(&current, entry(id).key), Some(entry(id).location));
    }
}

#[test]
fn multiprocess_same_address_precedence_follows_publication_order() {
    if !parent_body("multiprocess_same_address_precedence_follows_publication_order") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = UnixListener::bind(root.join("barrier")).unwrap();
    let a = child(root, "overwrite", 1);
    let b = child(root, "overwrite", 2);
    let mut connections = [accept(&listener), accept(&listener)];
    let mut expected = entry(1).location;
    for socket in &mut connections {
        let mut id = [0];
        socket.read_exact(&mut id).unwrap();
        expected.offset = u32::from(id[0]) * 64;
        socket.write_all(&[1]).unwrap();
        let mut done = [0];
        socket.read_exact(&mut done).unwrap();
        assert_eq!(done, [2]);
    }
    a.finish();
    b.finish();
    assert_eq!(
        find(&load_pinned(root).unwrap(), entry(1).key),
        Some(expected)
    );
}

#[test]
fn concurrent_writer_during_plane_construction_preserves_newer_locator_and_old_pin() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    write(root, &[entry(2)]);
    let old = load_pinned(root).unwrap();
    let path = root.to_owned();
    let mut newer = entry(1);
    newer.location.offset = 128;
    CONSTRUCTION_OBSERVER
        .with(|slot| *slot.borrow_mut() = Some(Box::new(move |_| write(&path, &[newer]))));
    assert!(matches!(
        compact(root, budget()).unwrap(),
        CompactOutcome::Published { layers: 2, .. }
    ));
    assert_eq!(find(&old, newer.key), Some(entry(1).location));
    let current = load_pinned(root).unwrap();
    assert_eq!(find(&current, newer.key), Some(newer.location));
    assert_eq!(find(&current, entry(2).key), Some(entry(2).location));
}

thread_local! {
    static PARENT_START: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}
fn parent_body(name: &str) -> bool {
    if std::env::var("INDEX53_PARENT").as_deref() == Ok(name) {
        if let Some(path) = std::env::var_os("INDEX53_PARENT_START") {
            let mut socket = UnixStream::connect(path).unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            socket.write_all(&[0]).unwrap();
            let mut go = [0];
            socket.read_exact(&mut go).unwrap();
            assert_eq!(go, [1]);
        }
        return true;
    }
    // A spawn in another libtest thread can transiently inherit this body's
    // flock descriptors until exec, even across distinct tempdirs. Give each
    // body its own descriptor table before creating any store fixtures.
    let mut command = test_command(&format!("index::tests::races::{name}"));
    command
        .env("INDEX53_PARENT", name)
        .env_remove("INDEX53_PARENT_START");
    PARENT_START.with(|slot| {
        if let Some(path) = slot.borrow().as_ref() {
            command.env("INDEX53_PARENT_START", path);
        }
    });
    TestChild::spawn(&mut command).finish();
    false
}
#[test]
fn multiprocess_parent_bodies_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parents");
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::scope(|scope| {
        let bodies: [fn(); 4] = [
            multiprocess_common_pin_publication_and_live_staging_gc_owner,
            multiprocess_append_survives_off_lock_compaction,
            multiprocess_gc_cannot_enter_reader_open_gap_and_unlinked_pin_survives,
            multiprocess_same_address_precedence_follows_publication_order,
        ];
        let handles: Vec<_> = bodies
            .into_iter()
            .map(|body| {
                let path = path.clone();
                scope.spawn(move || {
                    PARENT_START.with(|slot| *slot.borrow_mut() = Some(path));
                    body();
                })
            })
            .collect();
        let mut ready: Vec<_> = (0..4)
            .map(|_| {
                let mut socket = accept(&listener);
                let mut byte = [1];
                socket.read_exact(&mut byte).unwrap();
                assert_eq!(byte, [0]);
                socket
            })
            .collect();
        for socket in &mut ready {
            socket.write_all(&[1]).unwrap();
        }
        for handle in handles {
            handle.join().unwrap();
        }
    });
}

#[test]
fn dropped_child_is_killed_and_reaped() {
    let dir = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(dir.path().join("barrier")).unwrap();
    let child = child(dir.path(), "barrier", 1);
    let pid = child.process.id() as libc::pid_t;
    let mut socket = accept(&listener);
    let mut id = [0];
    socket.read_exact(&mut id).unwrap();
    assert_eq!(id, [1]);
    // The child is blocked awaiting go; cleanup must not wait for that exchange.
    drop(child);
    let mut status = 0;
    // SAFETY: our own child's PID and a valid status pointer, nonblocking only.
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
