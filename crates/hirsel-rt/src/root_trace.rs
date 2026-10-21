//! `--trace-roots` (S-14), diagnostic runtime only: at every scheduling point, the count of
//! every live `Shared` root, keyed by the running task's spawn path and that task's
//! scheduling-point ordinal. A root is named by its freeze ordinal in the run, so the
//! interpreter's trace (`hirsel_interp::library`) names the same roots and the two can be
//! compared line by line. Counts are exact, overflowed ones included: both engines must have the same holders of every root at every
//! operation.
//!
//! A harness launch enables it with `HIRSEL_RT_TRACE_ROOTS` (a file receiving one line per
//! point: `<path> <ordinal>: <root>=<count> ...`, the root task's path being `root`). It reads
//! header counts, so it needs the diagnostic configuration, where every free reaches the
//! checked runtime path that retires a freed root here.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Header;
use crate::context::TaskCtx;

static ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Option<Trace>> = Mutex::new(None);

struct Trace {
    file: std::fs::File,
    /// Live roots in freeze order: their ordinal and block address.
    roots: Vec<(u64, usize)>,
    frozen: u64,
    /// Each task's scheduling points so far, by spawn path.
    points: BTreeMap<Vec<u32>, u64>,
}

fn with(f: impl FnOnce(&mut Trace)) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut trace = TRACE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(trace) = trace.as_mut() {
        f(trace);
    }
}

/// Start tracing this process's run into `file`.
pub(crate) fn open(file: std::fs::File) {
    *TRACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Trace {
        file,
        roots: Vec::new(),
        frozen: 0,
        points: BTreeMap::new(),
    });
    ENABLED.store(true, Ordering::Relaxed);
}

/// `shared.freeze` made `block` a root.
pub(crate) fn frozen(block: *mut Header) {
    with(|t| {
        t.frozen += 1;
        // Storage of a retired root may hold this one now.
        t.roots.retain(|&(_, b)| b != block.addr());
        t.roots.push((t.frozen, block.addr()));
    });
}

/// `block` stopped being a root: its last root was thawed, or its storage was freed.
pub(crate) fn retired(block: *mut Header) {
    with(|t| t.roots.retain(|&(_, b)| b != block.addr()));
}

/// A scheduling point of the running task: one line of every live root's count.
///
/// # Safety
/// `ctx` is live and every traced root block is live.
pub(crate) unsafe fn point(ctx: &mut TaskCtx) {
    with(|t| {
        // A run without tasks yet is its root task.
        let path = if ctx.run_ref().tasks.is_some() {
            crate::tasks::current_path(ctx)
        } else {
            Vec::new()
        };
        let ordinal = t.points.entry(path.clone()).or_insert(0);
        *ordinal += 1;
        let mut line = if path.is_empty() {
            "root".to_string()
        } else {
            path.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".")
        };
        line.push_str(&format!(" {ordinal}:"));
        for &(root, block) in &t.roots {
            // SAFETY: a traced root is live until it is retired.
            let count = unsafe { crate::roots::count(ctx, block as *mut Header) };
            line.push_str(&format!(" {root}={count}"));
        }
        line.push('\n');
        if t.file.write_all(line.as_bytes()).is_err() {
            crate::fault("the root trace could not be written");
        }
    });
}
