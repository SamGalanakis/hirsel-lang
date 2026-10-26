//! Compact-frame dispatch witness, diagnostic runtime only (resume-dispatch-51).
//!
//! A harness launch names a file with `HIRSEL_RT_COMPACT_TRACE`. Each compact
//! leave appends `leave <task> <state> <worker> <pooled>` once its idle worker
//! has taken back the stack the frame left (`pooled` is 1 when that worker's
//! pool holds it), and each activation appends `activate <task> <state>
//! <worker>` when a worker lends the frame a stack. A task that suspends on its
//! own stack appends `hold <task> <worker>` once saved, and `resume <task>
//! <worker>` when a worker switches back into a saved stack (a task's first
//! start included), so a reader bounds the stacks suspended tasks hold
//! (Go's `TestStackMem`, `runtime/stack_test.go:24-79`); each resume point
//! appends `point <task> <state> <worker> <saves>`. `<task>` is the row index,
//! `<state>` the frame's saved state and `<worker>` the compute index, or
//! `home`. A resumable ahead body whose save continues in its ordinary
//! dispatcher first appends `ahead <task> <state> <worker>`. The trace is
//! physical: no canonical event, tape entry or observation depends on it.

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

/// A resumable ahead body saved at a boundary handed its frame, at `state`, to
/// its ordinary dispatcher (`ahead <task> <state> <worker>`).
pub(crate) fn ahead(task: u32, state: u32) {
    line(format_args!("ahead {task} {state} {}", worker()));
}

/// A worker lent a compact frame a stack.
pub(crate) fn activated(task: u32, state: u32) {
    line(format_args!("activate {task} {state} {}", worker()));
}

/// A resume point's decision: whether the task's frame saves there.
pub(crate) fn point(task: u32, state: u32, saves: bool) {
    line(format_args!(
        "point {task} {state} {} {}",
        worker(),
        u8::from(saves)
    ));
}

/// A task suspended on its own stack, which it keeps.
pub(crate) fn held(task: u32) {
    line(format_args!("hold {task} {}", worker()));
}

/// A worker switched back into a task's saved stack.
pub(crate) fn resumed(task: u32) {
    line(format_args!("resume {task} {}", worker()));
}

/// An entered runtime invocation returns Pending through its certified frames.
pub(crate) fn runtime_wait(task: u32, state: u32, kind: u32, phase: &str, invocation: u64) {
    line(format_args!(
        "runtime {task} {state} {} {kind} {phase} {invocation}",
        worker()
    ));
}

/// A certified source finalizer returned Pending with its original nested PC.
/// This is physical evidence only; it adds no language or replay event.
pub(crate) fn finalizer(task: u32, state: u32) {
    line(format_args!("finalizer {task} {state} {}", worker()));
}
