//! The host filesystem fixture of the io tests and of their interpreter-replay twins in
//! hirsel-pipeline-tests (`rt_io`): a disposable granted directory over the initial tree.
use super::library::{Io, Library};
use hirsel_rt::{io::*, io_real::RealIo};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
pub struct Disposable(PathBuf);

impl Disposable {
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "hirsel-l-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Disposable {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

pub fn initial() -> Tree {
    Tree::new(BTreeMap::from([
        ("old".into(), b"old".to_vec()),
        ("sub/é".into(), vec![0, 0xff]),
        ("sub/a".into(), b"A".to_vec()),
    ]))
    .unwrap()
}

pub fn setup() -> (Disposable, RealIo) {
    let temp = Disposable::new();
    let tree = initial();
    for d in &tree.directories {
        if !d.is_empty() {
            std::fs::create_dir_all(temp.path().join(d)).unwrap();
        }
    }
    for (p, b) in tree.files {
        std::fs::write(temp.path().join(p), b).unwrap();
    }
    let io = RealIo::grant(temp.path()).unwrap();
    (temp, io)
}

pub fn actions(real: &mut Library<RealIo>) {
    let out = real.stdout(Io(1)).unwrap().unwrap();
    let root = real.directory(Io(1), "").unwrap().unwrap();
    assert_eq!(real.print(out, "α\n"), Ok(Ok(())));
    let sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    assert_eq!(real.walk(sub), Ok(Ok(vec!["a".into(), "é".into()])));
    assert_eq!(real.read(sub, "é"), Ok(Ok(vec![0, 0xff])));
    assert_eq!(real.size(root, "old"), Ok(Ok(3)));
    let file = real.open(root, "sub/a").unwrap().unwrap();
    let file = real.narrow_file(sub, file).unwrap().unwrap();
    real.close_file(file).unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(71),
        partial: 2,
    });
    assert_eq!(
        real.write(root, "old", b"abcdef".to_vec()),
        Ok(Err(IoError::Fault(71)))
    );
    real.0.inject(Fault {
        error: IoError::Canceled(1),
        partial: 999,
    });
    assert_eq!(
        real.print(out, "not printed"),
        Ok(Err(IoError::Canceled(1)))
    );
    real.0.inject(Fault {
        error: IoError::Canceled(0),
        partial: 999,
    });
    assert_eq!(
        real.write(root, "old", b"no".to_vec()),
        Ok(Err(IoError::Canceled(0)))
    );
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"first")
        .unwrap()
        .unwrap();
    real.0.inject(Fault {
        error: IoError::Fault(72),
        partial: 2,
    });
    assert_eq!(
        real.write_all(root, &mut atomic, b"second"),
        Ok(Err(IoError::Fault(72)))
    );
    real.commit(root, atomic).unwrap().unwrap();
    let atomic = real.create_atomic(root, "abort").unwrap().unwrap();
    real.abort(atomic).unwrap().unwrap();
    let atomic = real.create_atomic(root, "cancel").unwrap().unwrap();
    real.0.inject(Fault {
        error: IoError::Canceled(2),
        partial: 99,
    });
    assert_eq!(real.commit(root, atomic), Ok(Err(IoError::Canceled(2))));
    assert_eq!(real.read(root, "missing"), Ok(Err(IoError::NotFound)));
    real.remove(sub, "a").unwrap().unwrap();
}

/// Discard the host rejection input to require the model to derive reachability
/// from the recorded publications. Round-trip the real on-disk tape format too.
pub fn identity_tape(events: &[Event]) -> Vec<Event> {
    let mut events = events.to_vec();
    for event in &mut events {
        if matches!(event.request, Request::NarrowFile { .. }) {
            event.fault = None;
        }
    }
    hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(&events).unwrap()).unwrap()
}
