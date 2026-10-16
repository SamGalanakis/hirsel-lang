//! The engine is single threaded; its whole lifetime belongs on this stack.
// Debug x86-64 GDB entry-to-entry rsp measurements on 1,024-level trees:
// parser 7,440 B, resolver 8,424 B, printer 9,496 B, authority 14,128 B,
// lowering 20,256 B per level (including its expression/inner/block helpers).
// Reserve 24 KiB per level, plus a fourfold margin for phase/helper changes.
pub const COMPILER_STACK_BYTES: usize = 128 * 1024 * 1024;
thread_local! { static COMPILER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Construct and use the engine inside this closure, including incremental edits.
/// Rc phase state stays on this thread; only the closure's input/output crosses it.
/// Nested calls reuse the same compiler thread. Panics retain their original payload.
pub fn with_compiler_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    if COMPILER_THREAD.get() {
        return work();
    }
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("hirsel-compiler".into())
            .stack_size(COMPILER_STACK_BYTES)
            .spawn_scoped(scope, || {
                COMPILER_THREAD.set(true);
                work()
            })
            .expect("compiler thread creation failed");
        thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
