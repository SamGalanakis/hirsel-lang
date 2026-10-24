#![cfg(feature = "ownership-trace")]
use hirsel_rt::context::{Config, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy};
use hirsel_rt::ownership_trace::{
    self, ABI_VERSION, hirsel_rt_v2_ownership_enter, hirsel_rt_v2_ownership_event,
    hirsel_rt_v2_ownership_leave,
};
use hirsel_run_schema::ownership_trace::native::{Log, Row};
use std::path::PathBuf;

struct Context(*mut hirsel_rt::TaskCtx);
impl Context {
    fn new() -> Self {
        let mut context = std::ptr::null_mut();
        assert_eq!(
            unsafe { hirsel_rt_v2_ctx_create(&Config::new(), &mut context) },
            0
        );
        Self(context)
    }
    fn start(&self, path: PathBuf, limit: usize) {
        unsafe { ownership_trace::start(self.0, [49; 32], path, limit) }.unwrap();
    }
}
impl Drop for Context {
    fn drop(&mut self) {
        unsafe { hirsel_rt_v2_ctx_destroy(self.0) };
    }
}

#[test]
fn ownership_runtime_records_actual_hook_calls_and_a_complete_quiescent_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let context = Context::new();
    context.start(path.clone(), 8);
    let frame = unsafe { hirsel_rt_v2_ownership_enter(context.0, ABI_VERSION, [7; 32].as_ptr()) };
    assert_ne!(frame, 0);
    unsafe {
        hirsel_rt_v2_ownership_event(context.0, frame, 0, 0, 2, 2, 42, 7);
        hirsel_rt_v2_ownership_event(context.0, frame, 1, 2, 2, 2, 43, 7);
        hirsel_rt_v2_ownership_leave(context.0, frame);
    }
    assert!(!path.exists(), "callbacks perform no observer I/O");
    drop(context);
    let log: Log = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(log.complete);
    assert_eq!(log.program, [49; 32]);
    assert_eq!(log.row_count, 4);
    assert_eq!(
        log.rows,
        [
            Row::Enter {
                task: 0,
                frame,
                unit: [7; 32]
            },
            Row::Event {
                task: 0,
                frame,
                marker: 0,
                action: 0,
                token: 2,
                owner: 2,
                site: 42,
                layout: 7
            },
            Row::Event {
                task: 0,
                frame,
                marker: 1,
                action: 2,
                token: 2,
                owner: 2,
                site: 43,
                layout: 7
            },
            Row::Leave { task: 0, frame },
        ]
    );
}

#[test]
fn ownership_runtime_overflow_and_extension_mismatch_are_sticky_diagnostic_failures() {
    for mismatch in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.json");
        let context = Context::new();
        context.start(path.clone(), 1);
        let version = ABI_VERSION + u32::from(mismatch);
        let frame = unsafe { hirsel_rt_v2_ownership_enter(context.0, version, [7; 32].as_ptr()) };
        assert_eq!(frame == 0, mismatch);
        unsafe {
            hirsel_rt_v2_ownership_event(context.0, frame, 0, 0, 2, 2, 42, 7);
            hirsel_rt_v2_ownership_leave(context.0, frame);
        }
        drop(context);
        let log: Log = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(
            !log.complete,
            "lost rows and an incompatible hook ABI never qualify"
        );
    }
}

#[test]
fn ownership_runtime_admission_refuses_replacement_and_oversized_buffers() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    let context = Context::new();
    assert!(
        unsafe { ownership_trace::start(context.0, [49; 32], first.clone(), usize::MAX) }.is_err()
    );
    context.start(first.clone(), 1);
    assert!(unsafe { ownership_trace::start(context.0, [49; 32], second.clone(), 1) }.is_err());
    drop(context);
    assert!(first.is_file());
    assert!(!second.exists());
}
