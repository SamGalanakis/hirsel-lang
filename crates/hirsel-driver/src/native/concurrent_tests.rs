//! Overlap the real native run pipeline immediately before its scratch writes.
use super::*;
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, Barrier, Mutex},
};

struct Rendezvous {
    barrier: Barrier,
    paths: Mutex<Vec<PathBuf>>,
}

thread_local! {
    static RENDEZVOUS: RefCell<Option<Arc<Rendezvous>>> = const { RefCell::new(None) };
}

pub(super) fn before_link(path: &Path) {
    if let Some(rendezvous) = RENDEZVOUS.take() {
        rendezvous.paths.lock().unwrap().push(path.to_path_buf());
        rendezvous.barrier.wait();
    }
}

#[test]
fn concurrent_native_builds_keep_descriptors_and_observations_separate() {
    let inputs = super::super::executable::Scratch::new().unwrap();
    let rendezvous = Arc::new(Rendezvous {
        barrier: Barrier::new(2),
        paths: Mutex::new(Vec::new()),
    });
    let programs = [("ok", "Ok(())"), ("error", "Error(())")];
    let threads: Vec<_> = programs
        .into_iter()
        .map(|(name, returned)| {
            let source = inputs.0.join(format!("{name}.hir"));
            std::fs::write(
                &source,
                format!("pub fn main()->Result[(),()]{{{returned}}}\n"),
            )
            .unwrap();
            let rendezvous = rendezvous.clone();
            std::thread::spawn(move || {
                hirsel_engine::with_compiler_stack(move || {
                    RENDEZVOUS.set(Some(rendezvous));
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_path_native(&source, &mut stdout, &mut stderr)
                    }));
                    // A setup failure must release the other worker and preserve
                    // the original failure instead of hanging at the barrier.
                    if let Some(rendezvous) = RENDEZVOUS.take() {
                        rendezvous.barrier.wait();
                    }
                    let result = result.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
                    (result, stdout, stderr)
                })
            })
        })
        .collect();
    // Join both before asserting: never abandon the other concurrent build.
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let paths = rendezvous.paths.lock().unwrap();
    assert_eq!(paths.len(), 2, "both builds must reach the linking barrier");
    assert_ne!(
        paths[0], paths[1],
        "concurrent native builds share scratch; results: {results:?}"
    );
    for ((result, stdout, stderr), returned) in results.into_iter().zip([
        hirsel_run_schema::Returned::Ok,
        hirsel_run_schema::Returned::Error,
    ]) {
        assert_eq!(result.unwrap().unwrap(), RunOutcome::Returned(returned));
        assert!(stdout.is_empty(), "{stdout:?}");
        assert_eq!(
            stderr,
            if returned == hirsel_run_schema::Returned::Error {
                b"error: ()\n".as_slice()
            } else {
                b"".as_slice()
            }
        );
    }
    assert!(
        paths.iter().all(|path| !path.exists()),
        "scratch must be dropped after launch"
    );
}
