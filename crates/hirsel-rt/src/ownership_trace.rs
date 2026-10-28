//! Opt-in native ownership diagnostics. Admission allocates a bounded row
//! buffer. Hooks only copy scalar rows: no allocation, locks, clocks, capability
//! calls, cancellation checks or scheduler entry occur at a language transition.
//! Default runtime builds compile out this module and its private run field.
use crate::TaskCtx;
use hirsel_run_schema::ownership_trace::{
    Buffer,
    native::{ChannelDirection, Log, Row, VERSION},
};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
pub(crate) mod results;

pub const ABI_VERSION: u32 = 102;
const _: () = assert!(ABI_VERSION > crate::ABI_VERSION);
const LIMIT: usize = 65_536;

/// Checked source intent, retained by the runtime's actual spawn command.
#[derive(Clone, Copy)]
pub(crate) struct SpawnBinding {
    pub(crate) task: u64,
    pub(crate) frame: u64,
    pub(crate) marker: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct QueueBinding {
    pub(crate) frame: u64,
    pub(crate) marker: u32,
}

/// Take the issuing task's checked intent without observing any language bytes.
pub(crate) unsafe fn take_spawn(ctx: *mut TaskCtx) -> Option<SpawnBinding> {
    unsafe { (*ctx).ownership_spawn.take() }
}

pub(crate) fn spawned(run: &crate::context::RunShared, binding: Option<SpawnBinding>, child: u64) {
    if let (Some(observer), Some(binding)) = (run.ownership_trace.as_ref(), binding) {
        observer.record(Row::Spawn {
            task: binding.task,
            frame: binding.frame,
            marker: binding.marker,
            child,
        });
    }
}

pub(crate) fn task_end(ctx: &TaskCtx) {
    if let Some(observer) = ctx.run_ref().ownership_trace.as_ref() {
        observer.record(Row::TaskEnd {
            task: ctx.ownership_task,
        });
    }
}

pub(crate) struct Observer {
    destination: std::path::PathBuf,
    program: [u8; 32],
    rows: Arc<Buffer<Row>>,
    next_frame: AtomicU64,
    next_queue: AtomicU64,
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
            rows: Arc::new(Buffer::new(
                limit,
                Row::Leave {
                    task: u64::MAX,
                    frame: 0,
                },
            )),
            next_frame: AtomicU64::new(1),
            next_queue: AtomicU64::new(1),
            failed: AtomicBool::new(false),
        }
    }
    fn record(&self, row: Row) {
        // Overflow is diagnostic only. Buffer consumption remains poisoned
        // even though the compiled language operation continues unchanged.
        let _ = self.rows.push(row);
    }
    pub(crate) fn write(self) {
        let rows = Arc::try_unwrap(self.rows)
            .ok()
            .and_then(|rows| rows.into_rows().ok());
        let complete = rows
            .as_ref()
            .is_some_and(|rows| !rows.contains(&Row::ObservationLost))
            && !self.failed.into_inner();
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

/// Copy the actual runtime-assigned task operation, without acquiring any
/// scheduler state, sampling a clock or allocating diagnostic storage.
/// # Safety
/// `ctx` belongs to the issuing task; `ordinal` was assigned by its runtime.
pub(crate) unsafe fn operation(ctx: *mut TaskCtx, ordinal: u64) {
    if let Some(observer) = unsafe { observer(ctx) } {
        unsafe { &*observer }.record(Row::Operation {
            task: unsafe { (*ctx).ownership_task },
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
        if action == 5 {
            let binding = SpawnBinding {
                task: unsafe { (*ctx).ownership_task },
                frame,
                marker,
            };
            if unsafe { (*ctx).ownership_spawn.replace(binding) }.is_some() {
                observer.failed.store(true, Ordering::Relaxed);
            }
        }
        if action == 8
            && unsafe {
                (*ctx)
                    .ownership_queue
                    .replace(QueueBinding { frame, marker })
            }
            .is_some()
        {
            observer.failed.store(true, Ordering::Relaxed);
        }
        if matches!(action, 9 | 10)
            && unsafe {
                (*ctx).ownership_result.replace(results::Binding {
                    task: (*ctx).ownership_task,
                    frame,
                    marker,
                })
            }
            .is_some()
        {
            observer.failed.store(true, Ordering::Relaxed);
        }
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
/// Current task; scope and five-word binding rows remain live until cleanup_end.
/// Each row is (offset, linked descriptor index, binding ordinal, local layout, declaration field).
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
pub(crate) unsafe fn retired(
    ctx: &TaskCtx,
    layout: *const crate::release::Layout,
    value: *mut u8,
    base: *mut u8,
    declaration: u32,
) {
    unsafe {
        let mut scope = ctx.ownership_cleanup;
        while !scope.is_null() {
            for index in 0..(*scope).count {
                let row = std::slice::from_raw_parts((*scope).bindings.add(index as usize * 5), 5);
                if !base.is_null()
                    && (*scope).base == base
                    && row[4] == u64::from(declaration)
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

#[cfg(test)]
#[path = "ownership_trace_tests.rs"]
mod tests;

/// Detached FIFO position, retained with its actual image storage. No pointer
/// leaves the runtime and no Rust-owned value lives in the stack scope below.
#[derive(Clone, Copy)]
pub(crate) struct QueueCleanupTag {
    queue: u64,
    layout: u32,
    first: u64,
}

pub(crate) struct QueueCleanupScope {
    previous: *mut QueueCleanupScope,
    queue: u64,
    layout: u32,
    item: u64,
    base: *mut u8,
}

/// # Safety
/// The current task exclusively owns ctx. If this returns true, scope remains
/// initialized, aligned and immovable on its fiber until queue_cleanup_end.
/// base is the live detached image at index of tag's FIFO range.
pub(crate) unsafe fn queue_cleanup_begin(
    ctx: *mut TaskCtx,
    scope: *mut QueueCleanupScope,
    tag: Option<QueueCleanupTag>,
    index: usize,
    base: *mut u8,
) -> bool {
    let Some(tag) = tag else {
        return false;
    };
    // SAFETY: ctx is the current live task; observer storage is run-owned.
    let Some(observer) = (unsafe { observer(ctx) }) else {
        return false;
    };
    let Some(item) = tag.first.checked_add(index as u64) else {
        // SAFETY: the admitted observer stays live through this task.
        unsafe {
            (*observer).failed.store(true, Ordering::Relaxed);
        }
        return false;
    };
    // SAFETY: caller reserves scope on this fiber and keeps its image alive.
    unsafe {
        scope.write(QueueCleanupScope {
            previous: (*ctx).ownership_queue_cleanup,
            queue: tag.queue,
            layout: tag.layout,
            item,
            base,
        });
        (*ctx).ownership_queue_cleanup = scope;
        (*observer).record(Row::QueueCleanupBegin {
            task: canonical((*ctx).ownership_task),
            queue: tag.queue,
            layout: tag.layout,
            item,
        });
    }
    true
}

/// # Safety
/// scope is the current initialized scope whose begin returned true, on the
/// same live task. Its complete ordinary image finalization has returned.
pub(crate) unsafe fn queue_cleanup_end(ctx: *mut TaskCtx, scope: *mut QueueCleanupScope) {
    // SAFETY: caller keeps both ctx and scope live until this pop completes.
    unsafe {
        if (*ctx).ownership_queue_cleanup != scope {
            if let Some(observer) = observer(ctx) {
                (*observer).failed.store(true, Ordering::Relaxed);
            }
            return;
        }
        (*ctx).ownership_queue_cleanup = (*scope).previous;
        if let Some(observer) = observer(ctx) {
            (*observer).record(Row::QueueCleanupEnd {
                task: canonical((*ctx).ownership_task),
                queue: (*scope).queue,
                item: (*scope).item,
            });
        }
    }
}

/// # Safety
/// root/step name the actual descriptor program's completed end-finalizer
/// step. base is that program's live image, not a token or persistent identity.
pub(crate) unsafe fn queue_retired(ctx: &TaskCtx, base: *mut u8, root: u32, step: u32) {
    // SAFETY: scopes are borrowed from this task's still-live fiber frames.
    unsafe {
        let mut scope = ctx.ownership_queue_cleanup;
        while !scope.is_null() {
            if (*scope).base == base && (*scope).layout == root {
                if let Some(observer) = ctx.run_ref().ownership_trace.as_ref() {
                    observer.record(Row::QueueCleanupDrop {
                        task: canonical(ctx.ownership_task),
                        queue: (*scope).queue,
                        item: (*scope).item,
                        step,
                    });
                }
                return;
            }
            scope = (*scope).previous;
        }
    }
}

/// A channel retains only a weak handle to diagnostic storage. It neither
/// extends the run nor keeps the buffer alive after its quiescent writer.
#[derive(Clone)]
pub(crate) struct QueueRecord {
    rows: Weak<Buffer<Row>>,
    queue: u64,
    layout: u32,
}
impl QueueRecord {
    pub(crate) fn cleanup_tag(&self, first: u64) -> QueueCleanupTag {
        QueueCleanupTag {
            queue: self.queue,
            layout: self.layout,
            first,
        }
    }
    fn record(&self, row: Row) {
        if let Some(rows) = self.rows.upgrade() {
            let _ = rows.push(row);
        }
    }
    pub(crate) fn sent(&self, task: u64, item: u64, status: u32, pending: bool) {
        self.record(Row::QueueSent {
            task: canonical(task),
            queue: self.queue,
            layout: self.layout,
            item,
            status,
            pending,
        });
    }
    pub(crate) fn transfer(
        &self,
        task: u64,
        binding: QueueBinding,
        item: u64,
        direction: ChannelDirection,
        status: u32,
    ) {
        self.record(Row::QueueTransfer {
            task: canonical(task),
            frame: binding.frame,
            marker: binding.marker,
            queue: self.queue,
            layout: self.layout,
            item,
            direction,
            status,
        });
    }
    pub(crate) fn canceled(&self, task: u64, binding: QueueBinding, status: u32) {
        self.record(Row::QueueCanceled {
            task: canonical(task),
            frame: binding.frame,
            marker: binding.marker,
            queue: self.queue,
            layout: self.layout,
            status,
        });
    }
    pub(crate) fn received(&self, task: u64, item: u64, status: u32) {
        self.record(Row::QueueReceived {
            task: canonical(task),
            queue: self.queue,
            layout: self.layout,
            item,
            status,
        });
    }
    pub(crate) fn published(&self, task: u64, item: u64) {
        self.record(Row::QueuePublished {
            task: canonical(task),
            queue: self.queue,
            layout: self.layout,
            item,
        });
    }
    pub(crate) fn detached(&self, task: u64, first: u64, end: u64) {
        self.record(Row::QueueDetached {
            task: canonical(task),
            queue: self.queue,
            layout: self.layout,
            first,
            end,
        });
    }
}
fn canonical(task: u64) -> u64 {
    if task as u32 == 0 { 0 } else { task }
}

/// The actual creation installs this identity before any endpoint is exposed.
/// IDs come from the admitted observer, never payloads, addresses or handles.
pub(crate) fn queue_created(
    run: &crate::context::RunShared,
    task: u64,
    cell: &crate::channels::ChannelCell,
) {
    let Some(observer) = &run.ownership_trace else {
        return;
    };
    let Ok(queue) = observer
        .next_queue
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
    else {
        observer.failed.store(true, Ordering::Relaxed);
        return;
    };
    let record = QueueRecord {
        rows: Arc::downgrade(&observer.rows),
        queue,
        layout: cell.element.descriptor,
    };
    if cell.ownership.set(record).is_err() {
        observer.failed.store(true, Ordering::Relaxed);
        return;
    }
    observer.record(Row::QueueCreated {
        task: canonical(task),
        queue,
        layout: cell.element.descriptor,
    });
}

/// # Safety
/// This context owns creation; neither endpoint has been exposed to another task.
pub(crate) unsafe fn queue_created_here(ctx: *mut TaskCtx, cell: &crate::channels::ChannelCell) {
    unsafe {
        queue_created((*ctx).run_ref(), (*ctx).ownership_task, cell);
    }
}
