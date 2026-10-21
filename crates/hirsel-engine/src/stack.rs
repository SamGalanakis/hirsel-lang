//! Stack headroom for compiler work; nested work reuses its current compiler stack.
pub use hirsel_intern::jobs::COMPILER_STACK_BYTES;
thread_local! { static COMPILER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

/// Run compiler work on a stack sized for recursive language operations.
/// Engines may move between threads; phase adapters are immutable shared handles.
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
            .unwrap_or_else(|error| hirsel_guard::spawn_failed(COMPILER_STACK_BYTES, error));
        thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
