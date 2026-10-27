//! Opt-in native ownership diagnostics. Admission allocates a bounded row
//! buffer. Hooks only copy scalar rows: no allocation, locks, clocks, capability
//! calls, cancellation checks or scheduler entry occur at a language transition.
//! Default runtime builds compile out this module and its private run field.
use crate::TaskCtx;
use hirsel_run_schema::ownership_trace::{
    Buffer,
    native::{Log, Row, VERSION},
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const ABI_VERSION: u32 = 86;
const _: () = assert!(ABI_VERSION > crate::ABI_VERSION);
const LIMIT: usize = 65_536;

pub(crate) struct Observer {
    destination: std::path::PathBuf,
    program: [u8; 32],
    rows: Buffer<Row>,
    next_frame: AtomicU64,
    failed: AtomicBool,
}
impl Observer {
    pub(crate) fn from_env(program: [u8; 32]) -> Option<Self> {
        Some(Self::new(
            std::env::var_os("HIRSEL_OWNERSHIP_TRACE")?.into(),
            program,
            LIMIT,
        ))
    }
    fn new(destination: std::path::PathBuf, program: [u8; 32], limit: usize) -> Self {
        Self {
            destination,
            program,
            rows: Buffer::new(
                limit,
                Row::Leave {
                    task: u64::MAX,
                    frame: 0,
                },
            ),
            next_frame: AtomicU64::new(1),
            failed: AtomicBool::new(false),
        }
    }
    fn record(&self, row: Row) {
        // Overflow is diagnostic only. Buffer consumption remains poisoned
        // even though the compiled language operation continues unchanged.
        let _ = self.rows.push(row);
    }
    pub(crate) fn write(self) {
        let rows = self.rows.into_rows();
        let complete = rows.is_ok() && !self.failed.into_inner();
        let rows = rows.unwrap_or_default();
        let log = Log {
            version: VERSION,
            program: self.program,
            row_count: rows.len() as u64,
            rows,
            complete,
        };
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut file = std::io::BufWriter::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(self.destination)?,
            );
            serde_json::to_writer(&mut file, &log)?;
            std::io::Write::flush(&mut file)?;
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("ownership trace sidecar: {error}");
        }
    }
}

/// Install ownership diagnostics during embedding admission.
///
/// # Safety
/// `ctx` must be a live context exclusively owned by the caller, before any
/// language execution or worker starts. The run must remain quiescent here.
pub unsafe fn start(
    ctx: *mut TaskCtx,
    program: [u8; 32],
    destination: std::path::PathBuf,
    limit: usize,
) -> Result<(), &'static str> {
    if ctx.is_null() {
        return Err("null ownership trace context");
    }
    if limit > LIMIT {
        return Err("ownership trace buffer exceeds admission limit");
    }
    let run = unsafe { (*ctx).run() };
    if run.ownership_trace.is_some() {
        return Err("ownership trace already admitted");
    }
    run.ownership_trace = Some(Observer::new(destination, program, limit));
    Ok(())
}

// SAFETY: a compiled hook receives its current task's stable context. The
// observer is installed before workers start and outlives every hook; it is
// consumed only by RunShared's last, quiescent owner. No borrow crosses a save.
unsafe fn observer(ctx: *mut TaskCtx) -> Option<*const Observer> {
    if ctx.is_null() {
        return None;
    }
    unsafe {
        (*ctx)
            .run_ref()
            .ownership_trace
            .as_ref()
            .map(std::ptr::from_ref)
    }
}

/// Register a runtime-created canonical identity before its first observed
/// frame. The run owner is independent of a task's possibly saved context.
/// Each row is copied into the admitted buffer without scheduler calls.
pub(crate) fn task(run: &crate::context::RunShared, task: u64, path: &[u32]) {
    let Some(observer) = run.ownership_trace.as_ref() else {
        return;
    };
    let Ok(depth) = u32::try_from(path.len()) else {
        observer.failed.store(true, Ordering::Relaxed);
        return;
    };
    observer.record(Row::Task { task, depth });
    for (index, &ordinal) in path.iter().enumerate() {
        observer.record(Row::TaskPart {
            task,
            index: index as u32,
            ordinal,
        });
    }
}

/// Allocate a diagnostic frame id, used only by other ownership hooks.
/// # Safety
/// ctx is current; unit names 32 immutable bytes of the checked emitted unit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_enter(
    ctx: *mut TaskCtx,
    abi: u32,
    unit: *const u8,
) -> u64 {
    let Some(observer) = (unsafe { observer(ctx) }) else {
        return 0;
    };
    let observer = unsafe { &*observer };
    if abi != ABI_VERSION || unit.is_null() {
        observer.failed.store(true, Ordering::Relaxed);
        return 0;
    }
    let Ok(frame) = observer
        .next_frame
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
    else {
        observer.failed.store(true, Ordering::Relaxed);
        return 0;
    };
    let unit = unsafe { std::slice::from_raw_parts(unit, 32) }
        .try_into()
        .expect("32 bytes");
    observer.record(Row::Enter {
        task: unsafe { (*ctx).ownership_task },
        frame,
        unit,
    });
    frame
}

/// Record a transition after the compiler's checked placement commits it.
/// # Safety
/// ctx is current; every operand comes from that unit's checked trace plan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_event(
    ctx: *mut TaskCtx,
    frame: u64,
    marker: u32,
    action: u32,
    token: u64,
    owner: u64,
    site: u64,
    layout: u32,
) {
    if let Some(observer) = unsafe { observer(ctx) } {
        let observer = unsafe { &*observer };
        observer.record(Row::Event {
            task: unsafe { (*ctx).ownership_task },
            frame,
            marker,
            action,
            token,
            owner,
            site,
            layout,
        });
    }
}

/// Bind a checked borrowed callable view to the still-owned caller slot.
/// This copies a scalar relation and creates no language ownership event.
/// # Safety
/// ctx is current and frame belongs to this admitted diagnostic run.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_alias(
    ctx: *mut TaskCtx,
    frame: u64,
    marker: u32,
    view: u64,
    owner: u64,
    site: u64,
    layout: u32,
) {
    if let Some(observer) = unsafe { observer(ctx) } {
        let observer = unsafe { &*observer };
        observer.record(Row::Alias {
            task: unsafe { (*ctx).ownership_task },
            frame,
            marker,
            view,
            owner,
            site,
            layout,
        });
    }
}

/// Record the diagnostic frame's actual retirement.
/// # Safety
/// ctx is current and frame was returned by this run's ownership_enter hook.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_leave(ctx: *mut TaskCtx, frame: u64) {
    if let Some(observer) = unsafe { observer(ctx) } {
        let observer = unsafe { &*observer };
        observer.record(Row::Leave {
            task: unsafe { (*ctx).ownership_task },
            frame,
        });
    }
}

/// Borrowed native stack record. Addresses locate physical cells only during
/// the synchronous release; they are never written to the trace or used as ids.
#[repr(C)]
pub struct CleanupScope {
    previous: *mut CleanupScope,
    frame: u64,
    marker: u64,
    base: *mut u8,
    bindings: *const u64,
    count: u64,
}
/// # Safety
/// Current task; scope and four-word binding rows remain live until cleanup_end.
/// Each row is (offset, linked descriptor index, capture ordinal, local layout).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_cleanup_begin(
    ctx: *mut TaskCtx,
    scope: *mut CleanupScope,
    frame: u64,
    marker: u32,
    environment: *mut crate::alloc::Header,
    bindings: *const u64,
    count: u64,
) {
    unsafe {
        scope.write(CleanupScope {
            previous: (*ctx).ownership_cleanup,
            frame,
            marker: u64::from(marker),
            base: if environment.is_null() {
                std::ptr::null_mut()
            } else {
                crate::alloc::payload(environment)
            },
            bindings,
            count,
        });
        (*ctx).ownership_cleanup = scope;
        if let Some(observer) = observer(ctx) {
            (*observer).record(Row::CleanupBegin {
                task: (*ctx).ownership_task,
                frame,
                marker,
            });
        }
    }
}
/// # Safety
/// scope is the current live cleanup record on this task's native stack.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ownership_cleanup_end(
    ctx: *mut TaskCtx,
    scope: *mut CleanupScope,
) {
    unsafe {
        if (*ctx).ownership_cleanup != scope {
            if let Some(observer) = observer(ctx) {
                (*observer).failed.store(true, Ordering::Relaxed);
            }
            return;
        }
        (*ctx).ownership_cleanup = (*scope).previous;
        if let Some(observer) = observer(ctx) {
            (*observer).record(Row::CleanupEnd {
                task: (*ctx).ownership_task,
                frame: (*scope).frame,
                marker: (*scope).marker as u32,
            });
        }
    }
}
/// Called only after the actual finalizer and field continuation retire.
/// # Safety
/// layout/value come from the checked descriptor program's completed end step.
pub(crate) unsafe fn retired(ctx: &TaskCtx, layout: *const crate::release::Layout, value: *mut u8) {
    unsafe {
        let mut scope = ctx.ownership_cleanup;
        while !scope.is_null() {
            for index in 0..(*scope).count {
                let row = std::slice::from_raw_parts((*scope).bindings.add(index as usize * 4), 4);
                if !(*scope).base.is_null()
                    && (*scope).base.wrapping_add(row[0] as usize) == value
                    && ctx.descriptor(row[1] as u32) == layout
                {
                    if let Some(observer) = observer(std::ptr::from_ref(ctx).cast_mut()) {
                        (*observer).record(Row::CleanupDrop {
                            task: ctx.ownership_task,
                            frame: (*scope).frame,
                            marker: (*scope).marker as u32,
                            field: row[2] as u32,
                            layout: row[3] as u32,
                        });
                    }
                }
            }
            scope = (*scope).previous;
        }
    }
}
