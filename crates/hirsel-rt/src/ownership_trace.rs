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

pub const ABI_VERSION: u32 = 74;
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
                    task: u32::MAX,
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
        task: unsafe { (*ctx).task_id },
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
            task: unsafe { (*ctx).task_id },
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
            task: unsafe { (*ctx).task_id },
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
            task: unsafe { (*ctx).task_id },
            frame,
        });
    }
}
