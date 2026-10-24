//! Bounded original-execution diagnostics, independent of program time and tapes.
//! Structure follows the existing measurement-only tasks/profile.rs sidecar, but
//! records public ABI endpoints before role acquisition. Buffers belong to the
//! run: a suspended operation may finish on a different worker. No logger lock,
//! allocation or observer I/O occurs at a call boundary. Replay ignores it.
use std::{
    cell::UnsafeCell,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Instant,
};

use crate::{Header, TaskCtx, context::RunShared};

const WORKERS: usize = 16;
const SLOTS: usize = 16384;
const TASKS: usize = 8192;
const NONE: usize = usize::MAX;

#[derive(Clone, Copy, Default)]
struct Row {
    task: usize,
    ordinal: u64,
    start_ns: u64,
    end_ns: u64,
    child: u64,
    kind: u8,
    status: u8,
}
struct Buffer {
    next: AtomicUsize,
    rows: Box<[UnsafeCell<Row>]>,
}
// SAFETY: a reservation has one issuer and one consuming completion. Workers
// never borrow another slot; the run's last owner reads only after quiescence.
unsafe impl Sync for Buffer {}

pub(crate) struct Observer {
    path: PathBuf,
    origin: Instant,
    buffers: Box<[Buffer]>,
    ordinals: Box<[AtomicU64]>,
    joins: Box<[AtomicUsize]>,
    dropped: AtomicU64,
}
impl Observer {
    pub(crate) fn from_env() -> Option<Self> {
        let path = std::env::var_os("HIRSEL_OPERATION_LATENCY")?;
        Some(Self {
            path: path.into(),
            origin: Instant::now(),
            buffers: (0..WORKERS)
                .map(|_| Buffer {
                    next: AtomicUsize::new(0),
                    rows: (0..SLOTS)
                        .map(|_| UnsafeCell::new(Row::default()))
                        .collect(),
                })
                .collect(),
            ordinals: (0..TASKS).map(|_| AtomicU64::new(0)).collect(),
            joins: (0..TASKS).map(|_| AtomicUsize::new(NONE)).collect(),
            dropped: AtomicU64::new(0),
        })
    }
    fn now(&self) -> u64 {
        self.origin.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
    }
    fn begin(&self, task: usize, worker: usize, kind: u8) -> Option<usize> {
        let Some(buffer) = self.buffers.get(worker) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let Some(ordinal) = self.ordinals.get(task) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let index = buffer.next.fetch_add(1, Ordering::Relaxed);
        if index >= buffer.rows.len() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        // SAFETY: fetch_add reserves this slot for this operation exclusively.
        unsafe {
            *buffer.rows[index].get() = Row {
                task,
                ordinal: ordinal.fetch_add(1, Ordering::Relaxed),
                start_ns: self.now(),
                kind,
                ..Row::default()
            };
        }
        Some(worker * SLOTS + index)
    }
    fn row(&self, ticket: usize) -> *mut Row {
        self.buffers[ticket / SLOTS].rows[ticket % SLOTS].get()
    }
    fn finish(&self, ticket: usize, status: u8) {
        let end = self.now();
        // SAFETY: the ticket is completed once, after the issuer resumes. No
        // reader observes rows until all workers have left the run.
        unsafe {
            (*self.row(ticket)).end_ns = end;
            (*self.row(ticket)).status = status;
        }
    }
    pub(crate) fn write(&self) {
        let result = (|| -> std::io::Result<()> {
            // create_new prevents silently replacing a different execution.
            let mut out = std::io::BufWriter::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&self.path)?,
            );
            writeln!(
                out,
                "{}",
                serde_json::json!({"schema":1,
                "clock":"std::time::Instant (host monotonic)", "sampling":"all",
                "worker_buffers":WORKERS,"slots_per_worker":SLOTS,
                "dropped":self.dropped.load(Ordering::Relaxed),
                "statuses":{"0":"censored","1":"success","2":"failure","3":"dead","4":"canceled"},
                "endpoints":{"1":"before spawn ABI through matching join ABI return",
                    "2":"before mutex take ABI through payload acquisition return; body excluded",
                    "3":"before file write ABI authority checks through delivery return"}})
            )?;
            for buffer in &self.buffers {
                for row in buffer
                    .rows
                    .iter()
                    .take(buffer.next.load(Ordering::Relaxed).min(SLOTS))
                {
                    // SAFETY: called only by the run's final, quiescent owner.
                    let row = unsafe { *row.get() };
                    writeln!(
                        out,
                        "{}",
                        serde_json::json!({"task":row.task,
                        "ordinal":row.ordinal,"kind":row.kind,"status":row.status,
                        "start_ns":row.start_ns,"end_ns":row.end_ns})
                    )?;
                }
            }
            out.flush()
        })();
        if let Err(error) = result {
            eprintln!("operation latency sidecar: {error}");
        }
    }
}

// SAFETY: ctx is the current task's stable context. Admission fixes replay
// before workers start; the run and its buffers outlive every suspended call.
unsafe fn observer(ctx: *mut TaskCtx) -> Option<*const Observer> {
    let run: &RunShared = unsafe { &*(*ctx).run_ptr() };
    if run.free_replay {
        return None;
    }
    run.operation_latency.as_ref().map(std::ptr::from_ref)
}
unsafe fn begin(ctx: *mut TaskCtx, kind: u8) -> Option<(*const Observer, usize)> {
    let observer = unsafe { observer(ctx)? };
    let task = unsafe { (*ctx).task_id as usize };
    let worker = unsafe { (*ctx).worker().id as usize };
    unsafe {
        (*observer)
            .begin(task, worker, kind)
            .map(|ticket| (observer, ticket))
    }
}
unsafe fn finish(span: Option<(*const Observer, usize)>, status: u8) {
    if let Some((observer, ticket)) = span {
        unsafe {
            (*observer).finish(ticket, status);
        }
    }
}

#[unsafe(export_name = "hirsel_rt_v2_mutex_take")]
unsafe extern "C" fn mutex_take(
    ctx: *mut TaskCtx,
    out: *mut u32,
    m: *const u64,
    payload: *mut u8,
) -> u8 {
    let span = unsafe { begin(ctx, 2) };
    let status = unsafe { crate::tasks::hirsel_rt_v2_mutex_take(ctx, out, m, payload) };
    unsafe {
        finish(
            span,
            if status != 0 {
                2
            } else if out.read() == 0 {
                1
            } else {
                3
            },
        );
    }
    status
}
#[unsafe(export_name = "hirsel_rt_v2_file_write")]
unsafe extern "C" fn file_write(
    ctx: *mut TaskCtx,
    destination: *mut u8,
    out: *mut crate::io::HandleResult,
    directory: *const u64,
    file: *mut u64,
    owner: *mut Header,
    start: u64,
    len: u64,
) -> u8 {
    let span = unsafe { begin(ctx, 3) };
    let status = unsafe {
        crate::io::hirsel_rt_v2_file_write(
            ctx,
            destination,
            out,
            directory,
            file,
            owner,
            start,
            len,
        )
    };
    let outcome = if status != 0 {
        2
    } else {
        let result = unsafe { out.read() };
        if result.tag == 0 {
            1
        } else if result.value & 0xffff_ffff == 1 {
            4
        } else {
            2
        }
    };
    unsafe {
        finish(span, outcome);
    }
    status
}
#[unsafe(export_name = "hirsel_rt_v2_task_spawn")]
unsafe extern "C" fn spawn(
    ctx: *mut TaskCtx,
    caller: u64,
    site: u64,
    destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    code: crate::tasks::TaskCode,
    environment: *mut Header,
    flags: u64,
) -> u8 {
    let span = unsafe { begin(ctx, 1) };
    let status = unsafe {
        crate::tasks::hirsel_rt_v2_task_spawn(
            ctx,
            caller,
            site,
            destination,
            out,
            scope,
            code,
            environment,
            flags,
        )
    };
    unsafe { spawned(span, out, status) };
    status
}

// Both native spawn entries reach the same ordinary Free implementation. The
// ahead entry validates its prefix before that fallback, so observe its public
// invocation too; calling the inner Rust function cannot traverse an export.
#[unsafe(export_name = "hirsel_rt_v2_task_spawn_ahead")]
unsafe extern "C" fn spawn_ahead(
    ctx: *mut TaskCtx,
    caller: u64,
    site: u64,
    destination: *mut u8,
    out: *mut u64,
    scope: *const u64,
    code: crate::tasks::TaskCode,
    environment: *mut Header,
    flags: u64,
    info: *const crate::tasks::ordered::ahead::SpawnAheadInfo,
) -> u8 {
    let span = unsafe { begin(ctx, 1) };
    let status = unsafe {
        crate::tasks::ordered::ahead::hirsel_rt_v2_task_spawn_ahead(
            ctx,
            caller,
            site,
            destination,
            out,
            scope,
            code,
            environment,
            flags,
            info,
        )
    };
    unsafe { spawned(span, out, status) };
    status
}

unsafe fn spawned(span: Option<(*const Observer, usize)>, out: *mut u64, status: u8) {
    if let Some((observer, ticket)) = span {
        let observer = unsafe { &*observer };
        if status == 0 {
            let child = unsafe { out.read() };
            if let Some(join) = observer.joins.get(child as u32 as usize) {
                // SAFETY: this reservation is still owned by this spawn.
                unsafe {
                    (*observer.row(ticket)).child = child;
                }
                join.store(ticket, Ordering::Release);
            } else {
                observer.dropped.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            observer.finish(ticket, 2);
        }
    }
}

#[unsafe(export_name = "hirsel_rt_v2_task_join")]
unsafe extern "C" fn join(
    ctx: *mut TaskCtx,
    caller: u64,
    site: u64,
    scope: *const u64,
    task: *mut u64,
    slot: *mut u8,
    size: u64,
) -> u8 {
    let child = unsafe { task.read() };
    let span = unsafe { observer(ctx) }.and_then(|observer| {
        let entry = unsafe { &*observer };
        let ticket = entry
            .joins
            .get(child as u32 as usize)?
            .swap(NONE, Ordering::AcqRel);
        if ticket == NONE {
            return None;
        }
        // SAFETY: consuming the join slot transfers its unique ticket to us.
        if unsafe { (*entry.row(ticket)).child } != child {
            return None;
        }
        Some((observer, ticket))
    });
    let status =
        unsafe { crate::tasks::hirsel_rt_v2_task_join(ctx, caller, site, scope, task, slot, size) };
    unsafe {
        finish(span, if status == 0 { 1 } else { 2 });
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observer(path: PathBuf) -> Observer {
        Observer {
            path,
            origin: Instant::now(),
            buffers: vec![Buffer {
                next: AtomicUsize::new(0),
                rows: (0..2).map(|_| UnsafeCell::new(Row::default())).collect(),
            }]
            .into(),
            ordinals: (0..2).map(|_| AtomicU64::new(0)).collect(),
            joins: (0..2).map(|_| AtomicUsize::new(NONE)).collect(),
            dropped: AtomicU64::new(0),
        }
    }

    #[test]
    fn resumed_completion_and_censored_calls_remain_distinct() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("operations");
        let observer = std::sync::Arc::new(observer(path.clone()));
        let completed = observer.begin(0, 0, 2).unwrap();
        let _censored = observer.begin(0, 0, 2).unwrap();
        let resumed = observer.clone();
        std::thread::spawn(move || resumed.finish(completed, 1))
            .join()
            .unwrap();
        assert!(observer.begin(0, 0, 2).is_none());
        assert!(observer.begin(0, WORKERS, 2).is_none());
        observer.write();
        let text = std::fs::read_to_string(path).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        assert_eq!(rows[0]["dropped"], 2);
        assert_eq!(rows[1]["status"], 1);
        assert!(rows[1]["end_ns"].as_u64().unwrap() >= rows[1]["start_ns"].as_u64().unwrap());
        assert_eq!(rows[2]["status"], 0);
        assert_eq!(rows[2]["end_ns"], 0);
        assert_eq!(rows[1]["ordinal"], 0);
        assert_eq!(rows[2]["ordinal"], 1);
    }
}
