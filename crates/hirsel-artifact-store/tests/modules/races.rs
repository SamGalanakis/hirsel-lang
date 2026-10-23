use crate::{index::*, pack::EpochLease};
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
    assert!(matches!(
        publish_delta(
            root,
            build(root, entries).unwrap(),
            &EpochLease(root.to_owned())
        )
        .unwrap(),
        PublishOutcome::Published { .. }
    ));
}
fn budget() -> MaintenanceBudget {
    MaintenanceBudget {
        max_input_bytes: 128 << 20,
        max_rows: 2_000_000,
        max_scratch_bytes: 256 << 20,
    }
}
fn child(root: &Path, mode: &str, id: u64) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([
        "--ignored",
        "--exact",
        "modules::races::cooperating_child",
        "--nocapture",
    ])
    .env("INDEX36_CHILD_ROOT", root)
    .env("INDEX36_CHILD_MODE", mode)
    .env("INDEX36_CHILD_ID", id.to_string())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    c
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
    let stale = load_pinned(&root).unwrap();
    let delta = build(&root, &[entry(id)]).unwrap();
    let mut socket = UnixStream::connect(root.join("barrier")).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    socket.write_all(&[id as u8]).unwrap();
    let mut go = [0];
    socket.read_exact(&mut go).unwrap();
    assert_eq!(go, [1]);
    assert_eq!(find(&stale, entry(id).key), None);
    assert!(matches!(
        publish_delta(&root, delta, &EpochLease(root.clone())).unwrap(),
        PublishOutcome::Published { .. }
    ));
    socket.write_all(&[2]).unwrap();
}
#[test]
fn multiprocess_common_pin_publication_and_live_staging_gc_owner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let listener = UnixListener::bind(root.join("barrier")).unwrap();
    let mut a = child(root, "barrier", 1).spawn().unwrap();
    let mut b = child(root, "barrier", 2).spawn().unwrap();
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
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    let pin = load_pinned(root).unwrap();
    for id in [1, 2] {
        assert_eq!(find(&pin, entry(id).key), Some(entry(id).location));
    }
    assert_eq!(fs::read_dir(root.join("index/staging")).unwrap().count(), 0);
}
#[test]
fn multiprocess_append_survives_off_lock_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, &[entry(1)]);
    write(root, &[entry(2)]);
    let path = root.to_owned();
    observe_compaction(move || {
        let output = child(&path, "append", 3).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
