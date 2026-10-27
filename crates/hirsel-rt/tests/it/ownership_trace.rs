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

#[test]
fn ownership_runtime_alias_hook_only_records_bounded_scalar_relations() {
    use hirsel_rt::ownership_trace::hirsel_rt_v2_ownership_alias;
    for (limit, complete) in [(3, true), (2, false)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.json");
        let context = Context::new();
        context.start(path.clone(), limit);
        let frame =
            unsafe { hirsel_rt_v2_ownership_enter(context.0, ABI_VERSION, [7; 32].as_ptr()) };
        unsafe {
            hirsel_rt_v2_ownership_alias(context.0, frame, 9, 12, 1 << 32, 42, 7);
            hirsel_rt_v2_ownership_leave(context.0, frame);
        }
        assert!(!path.exists(), "alias hooks perform no observer I/O");
        drop(context);
        let log: Log = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(log.complete, complete);
        if complete {
            assert_eq!(log.row_count, 3);
            assert_eq!(
                log.rows[1],
                Row::Alias {
                    task: 0,
                    frame,
                    marker: 9,
                    view: 12,
                    owner: 1 << 32,
                    site: 42,
                    layout: 7
                }
            );
            assert!(!log.rows.iter().any(|row| matches!(row, Row::Event { .. })));
        }
    }
}

#[test]
fn ownership_cleanup_scopes_nest_without_io_and_poison_overflow_or_bad_retirement() {
    use hirsel_rt::ownership_trace::{
        CleanupScope, hirsel_rt_v2_ownership_cleanup_begin, hirsel_rt_v2_ownership_cleanup_end,
    };
    for (limit, bad_order, complete) in [(6, false, true), (5, false, false), (6, true, false)] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.json");
        let context = Context::new();
        context.start(path.clone(), limit);
        let frame =
            unsafe { hirsel_rt_v2_ownership_enter(context.0, ABI_VERSION, [7; 32].as_ptr()) };
        let mut outer = std::mem::MaybeUninit::<CleanupScope>::uninit();
        let mut inner = std::mem::MaybeUninit::<CleanupScope>::uninit();
        unsafe {
            hirsel_rt_v2_ownership_cleanup_begin(
                context.0,
                outer.as_mut_ptr(),
                frame,
                10,
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
            );
            hirsel_rt_v2_ownership_cleanup_begin(
                context.0,
                inner.as_mut_ptr(),
                frame,
                11,
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
            );
            if bad_order {
                hirsel_rt_v2_ownership_cleanup_end(context.0, outer.as_mut_ptr());
            }
            hirsel_rt_v2_ownership_cleanup_end(context.0, inner.as_mut_ptr());
            hirsel_rt_v2_ownership_cleanup_end(context.0, outer.as_mut_ptr());
            hirsel_rt_v2_ownership_leave(context.0, frame);
        }
        assert!(!path.exists(), "cleanup hooks perform no observer I/O");
        drop(context);
        let log: Log = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(log.complete, complete);
        if complete {
            assert!(matches!(log.rows[1], Row::CleanupBegin { marker: 10, .. }));
            assert!(matches!(log.rows[2], Row::CleanupBegin { marker: 11, .. }));
            assert!(matches!(log.rows[3], Row::CleanupEnd { marker: 11, .. }));
            assert!(matches!(log.rows[4], Row::CleanupEnd { marker: 10, .. }));
        }
    }
}

#[test]
fn zero_sized_capture_retirement_uses_the_actual_declaration_and_live_mask() {
    use crate::support::{end_finalize_step, finalize_step, layout};
    use hirsel_rt::ownership_trace::{
        CleanupScope, hirsel_rt_v2_ownership_cleanup_begin, hirsel_rt_v2_ownership_cleanup_end,
    };
    unsafe extern "C" fn finish(_: *mut hirsel_rt::TaskCtx, _: *mut u8, _: u64, _: u64) -> u8 {
        hirsel_rt::STATUS_OK
    }
    let token = layout(0, 0, Some(finish), vec![]);
    let environment = layout(
        0,
        2,
        None,
        vec![
            finalize_step(1, 0, token),
            end_finalize_step(1, 0, token),
            finalize_step(0, 0, token),
            end_finalize_step(0, 0, token),
        ],
    );
    let layouts = [token as *const _, environment as *const _];
    for (mask, expected) in [
        (None, vec![11, 10]),
        (Some(1u64), vec![10]),
        (Some(2), vec![11]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.json");
        let config = Config {
            layouts: layouts.as_ptr(),
            layout_count: layouts.len() as u32,
            ..Config::new()
        };
        let mut ctx = std::ptr::null_mut();
        assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
        let context = Context(ctx);
        context.start(path.clone(), 16);
        let frame = unsafe { hirsel_rt_v2_ownership_enter(ctx, ABI_VERSION, [7; 32].as_ptr()) };
        // Both declaration fields have the same physical address and descriptor.
        let bindings = [0, 2, 10, 7, 0, 0, 2, 11, 7, 1];
        let mut scope = std::mem::MaybeUninit::<CleanupScope>::uninit();
        unsafe {
            let block = hirsel_rt::hirsel_rt_v2_alloc(ctx, 3, 0, 8, hirsel_rt::DOMAIN_HEAP, 0);
            assert!(!block.is_null());
            hirsel_rt_v2_ownership_cleanup_begin(
                ctx,
                scope.as_mut_ptr(),
                frame,
                9,
                block,
                bindings.as_ptr(),
                2,
            );
            let mask = mask.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
            assert_eq!(
                hirsel_rt::hirsel_rt_v2_release(ctx, block, 3, mask, 0, 0),
                0
            );
            hirsel_rt_v2_ownership_cleanup_end(ctx, scope.as_mut_ptr());
            hirsel_rt_v2_ownership_leave(ctx, frame);
        }
        drop(context);
        let log: Log = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert!(log.complete);
        let actual = log
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::CleanupDrop { field, .. } => Some(*field),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}
