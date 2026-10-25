//! Compact-frame dispatch witness, diagnostic runtime only (resume-dispatch-51).
//!
//! A harness launch names a file with `HIRSEL_RT_COMPACT_TRACE`. Each compact
//! leave appends `leave <task> <state> <worker> <pooled>` once its idle worker
//! has taken back the stack the frame left (`pooled` is 1 when that worker's
//! pool holds it), and each activation appends `activate <task> <state>
//! <worker>` when a worker lends the frame a stack. `<task>` is the row index,
//! `<state>` the frame's saved state and `<worker>` the compute index, or
//! `home`. The trace is physical: no canonical event, tape entry or
//! observation depends on it.

use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// Start tracing this process's run into `file`.
pub(crate) fn open(file: std::fs::File) {
    *TRACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    ENABLED.store(true, Ordering::Relaxed);
}

fn line(text: std::fmt::Arguments<'_>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut trace = TRACE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(file) = trace.as_mut() {
        let _ = writeln!(file, "{text}");
    }
}

fn worker() -> String {
    crate::pool::compute_index().map_or_else(|| "home".into(), |index| index.to_string())
}

/// A compact frame left its stack, which its idle worker took back.
pub(crate) fn left(task: u32, state: u32, pooled: bool) {
    line(format_args!(
        "leave {task} {state} {} {}",
        worker(),
        u8::from(pooled)
    ));
}

/// A worker lent a compact frame a stack.
pub(crate) fn activated(task: u32, state: u32) {
    line(format_args!("activate {task} {state} {}", worker()));
}
