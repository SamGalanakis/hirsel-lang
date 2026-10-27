//! Stackful fibers for the cooperative task scheduler (CONC-1): guarded stacks and a context
//! switch for x86-64 and AArch64.
//!
//! A suspended fiber is one stack pointer. [`switch`] pushes the callee-saved registers of the
//! System V and AAPCS64 conventions (and the x87/SSE control words on x86-64) onto the current
//! stack, stores the stack pointer through `save`, loads `to` and pops the target's registers,
//! so a switch looks like an ordinary call to both sides. A fresh stack is laid out as if it
//! had been suspended just before entering `start`, which calls the fiber's entry with its two
//! arguments; the entry never returns (it switches away for the last time).
//!
//! Stacks are anonymous mappings reserved at [`STACK_BYTES`] without committing memory, with an
//! inaccessible guard of [`GUARD_BYTES`] below, so running out of stack faults like the main
//! thread's: environmental termination, never a language failure. The top of each mapping
//! holds the context of the task that runs on it ([`Stack::context`]). Valgrind is told about each
//! stack (`STACK_REGISTER`) so a switch is not mistaken for a huge frame, and, with the `tsan`
//! feature, ThreadSanitizer which fiber each switch resumes.

// Context-switch references (layout/ABI comparison, no assembly copied):
// corosensei 4b5db484, src/arch/x86_64.rs:374-402,407-510 (initial frame,
// switch-and-link, unwind linkage); src/arch/aarch64.rs:251-370 (AArch64 linkage).
// Boost.Context fe719426, src/asm/jump_x86_64_sysv_elf_gas.S:50-83,105-144
// saves MXCSR/x87 CW and the SysV callee-saved registers before exchanging RSP;
// src/asm/jump_arm64_aapcs_elf_gas.S:79-128 saves x19-x30 and d8-d15.
// Hirsel uses a scheduler-owned save slot and two entry words, not a parent-linked
// coroutine or transfer_t. Cross-fiber unwinding/CFI is intentionally absent:
// task failures settle through the scheduler rather than unwinding another stack.
use crate::context::TaskCtx;
use crate::environmental;

/// Usable bytes of a fiber stack: the default main-thread limit, reserved lazily.
pub(crate) const STACK_BYTES: usize = 8 << 20;
/// Inaccessible bytes below each stack, so a frame larger than a page still lands in them.
pub(crate) const GUARD_BYTES: usize = 64 << 10;

/// The admitting thread may have a host-selected stack limit.
pub(crate) fn native_stack_bytes() -> u64 {
    let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    let (mut stack, mut guard) = (0usize, 0usize);
    // SAFETY: attributes is initialized by pthread_getattr_np and destroyed once.
    unsafe {
        if libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) != 0 {
            environmental("the admission stack size could not be read");
        }
        let mut attributes = attributes.assume_init();
        let status = libc::pthread_attr_getstacksize(&attributes, &mut stack)
            | libc::pthread_attr_getguardsize(&attributes, &mut guard);
        libc::pthread_attr_destroy(&mut attributes);
        if status != 0 {
            environmental("the admission stack bounds could not be read");
        }
    }
    (stack as u64)
        .checked_add(guard as u64)
        .unwrap_or_else(|| environmental("the admission stack bounds overflowed"))
}

/// Bytes at the top of each mapping for its task's context.
const CONTEXT_BYTES: usize = size_of::<TaskCtx>().next_multiple_of(64);

/// Retain the common shallow call prefix when a stack returns to its worker.
/// Deeper pages remain mapped, but Linux may reclaim their contents. This follows
/// Go's sysUnusedOS (src/runtime/mem_linux.go:40-68); unlike Go we never replace a
/// mapping if advice is unsupported, since retaining idle pages is harmless.
const STACK_HOT_BYTES: usize = 64 << 10;

/// Idle stacks a worker keeps. A stack a compact frame leaves returns to the
/// worker it left on, and another worker activating the frame maps its own,
/// so unbounded pools grow toward one stack per task per worker. A full pool
/// unmaps down to half before it takes another, as Go's per-P stack cache
/// releases half to the global pool (`runtime/stack.go:301-317,519-521`).
const IDLE_STACKS: usize = 4;

#[derive(Default)]
pub(crate) struct StackPool(
    Vec<Stack>,
    Option<std::sync::Arc<crate::pool::cpu_scoped::ledger::Ledger>>,
);

impl StackPool {
    pub(crate) fn account(
        &mut self,
        ledger: &std::sync::Arc<crate::pool::cpu_scoped::ledger::Ledger>,
    ) {
        for stack in &mut self.0 {
            stack.account(Some(ledger));
        }
        self.1 = Some(ledger.clone());
    }
    pub(crate) fn take(&mut self) -> Stack {
        self.0.pop().unwrap_or_else(|| {
            let mut stack = Stack::new();
            stack.account(self.1.as_ref());
            stack
        })
    }

    /// The previous context has been destroyed and no continuation can resume.
    pub(crate) fn put(&mut self, stack: Stack) {
        if self.0.len() >= IDLE_STACKS {
            self.0.truncate(IDLE_STACKS / 2);
        }
        stack.release_cold_pages();
        self.0.push(stack);
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Stack> {
        self.0.iter()
    }
}

/// The entry a fresh fiber starts in, with the two words [`Stack::prepare`] stored.
pub(crate) type Entry = unsafe extern "C" fn(*mut u8, *mut u8) -> !;

/// One guarded stack mapping.
pub(crate) struct Stack {
    base: *mut u8,
    valgrind: u64,
    backing: Option<crate::pool::cpu_scoped::ledger::Backing>,
    #[cfg(feature = "tsan")]
    tsan: std::cell::Cell<*mut std::ffi::c_void>,
}

impl Stack {
    pub(crate) fn account(
        &mut self,
        ledger: Option<&std::sync::Arc<crate::pool::cpu_scoped::ledger::Ledger>>,
    ) {
        if self.backing.is_none() {
            self.backing = ledger.map(|ledger| ledger.stack((STACK_BYTES + GUARD_BYTES) as u64));
        }
    }
    pub(crate) fn new() -> Stack {
        let length = GUARD_BYTES + STACK_BYTES;
        // SAFETY: a fresh private anonymous mapping; the guard is then made inaccessible.
        let base = unsafe {
            let base = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            );
            if base == libc::MAP_FAILED {
                environmental("a task stack could not be mapped");
            }
            if libc::mprotect(base, GUARD_BYTES, libc::PROT_NONE) != 0 {
                environmental("a task stack guard could not be protected");
            }
            base.cast::<u8>()
        };
        let valgrind = crate::memcheck::stack_register(
            base.wrapping_add(GUARD_BYTES),
            base.wrapping_add(length),
        );
        Stack {
            base,
            valgrind,
            backing: None,
            #[cfg(feature = "tsan")]
            tsan: std::cell::Cell::new(tsan::create(
                base.wrapping_add(GUARD_BYTES),
                base.wrapping_add(length),
            )),
        }
    }

    /// The slot of the context of the task that runs on this stack, at the mapping's top.
    pub(crate) fn context(&self) -> *mut TaskCtx {
        self.base
            .wrapping_add(GUARD_BYTES + STACK_BYTES - CONTEXT_BYTES)
            .cast()
    }

    /// One past the highest usable byte, 16-aligned: below the context slot.
    fn top(&self) -> *mut u8 {
        self.context().cast()
    }

    fn release_cold_pages(&self) {
        // sysUnusedOS requires both ends page aligned: madvise rounds outwards.
        // Keep the top prefix, context and guard completely outside the advice.
        let length = STACK_BYTES - STACK_HOT_BYTES;
        // SAFETY: this exclusively owned stack is idle, its lower usable range
        // contains no live object, and both constants are page multiples.
        unsafe {
            let cold = self.base.add(GUARD_BYTES).cast();
            if libc::madvise(cold, length, libc::MADV_FREE) != 0 {
                libc::madvise(cold, length, libc::MADV_DONTNEED);
            }
        }
    }

    /// The bytes of this stack's usable pages that are resident, as `mincore(2)` reports
    /// them: the stack's share of the process's RSS (the scheduling rows' `rss_bytes_per_task`).
    pub(crate) fn resident_bytes(&self) -> u64 {
        // SAFETY: a page size query has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let mut pages = vec![0u8; STACK_BYTES.div_ceil(page)];
        // SAFETY: the usable range is this mapping's, page-aligned, and `pages` holds a byte
        // per page of it.
        let queried = unsafe {
            libc::mincore(
                self.base.add(GUARD_BYTES).cast(),
                STACK_BYTES,
                pages.as_mut_ptr().cast(),
            )
        };
        if queried != 0 {
            return 0;
        }
        pages.iter().filter(|&&p| p & 1 != 0).count() as u64 * page as u64
    }

    pub(crate) fn supports_ahead(&self, bytes: u64) -> bool {
        let bottom = self.base.addr().checked_add(GUARD_BYTES);
        bottom
            .and_then(|bottom| self.top().addr().checked_sub(bottom))
            .is_some_and(|available| {
                bytes <= 65536 && usize::try_from(bytes).is_ok_and(|n| n <= available)
            })
    }

    /// The stack pointer of a fiber that, when switched to, calls `entry(first, second)`.
    pub(crate) fn prepare(&self, entry: Entry, first: *mut u8, second: *mut u8) -> *mut u8 {
        // A reused stack's last fiber ended by switching away, never returning: a fresh
        // fiber starts with an empty shadow stack.
        #[cfg(feature = "tsan")]
        self.tsan.set(tsan::renew(
            self.base.wrapping_add(GUARD_BYTES),
            self.base.wrapping_add(GUARD_BYTES + STACK_BYTES),
            self.tsan.get(),
        ));
        let words = initial_frame(entry, first, second);
        // SAFETY: the frame lies in the top of this stack's usable bytes.
        unsafe {
            let sp = self.top().cast::<u64>().sub(words.len());
            sp.copy_from_nonoverlapping(words.as_ptr(), words.len());
            sp.cast()
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        crate::memcheck::stack_deregister(self.valgrind);
        #[cfg(feature = "tsan")]
        tsan::destroy(self.base.wrapping_add(GUARD_BYTES), self.tsan.get());
        // SAFETY: the mapping is this stack's and no fiber runs on it any more.
        unsafe { libc::munmap(self.base.cast(), GUARD_BYTES + STACK_BYTES) };
    }
}

/// Suspend the running fiber, storing its stack pointer through `save`, and resume the fiber
/// suspended at `to` (or start a prepared one).
///
/// # Safety
/// `to` is the stack pointer of a suspended or prepared fiber that no other switch resumes,
/// and `save` stays writable until this fiber is resumed.
#[inline(always)]
pub(crate) unsafe fn switch(save: *mut *mut u8, to: *mut u8) {
    // x86 also has naked ahead handoffs, so its annotation lives in the raw
    // switch. Elsewhere this inline call leaves no shadow frame to pop later.
    #[cfg(all(feature = "tsan", not(target_arch = "x86_64")))]
    // SAFETY: the fiber is the one suspended at `to`; flags 0 synchronizes the switch, as
    // the scheduler's handoff does.
    unsafe {
        #[cfg(test)]
        TSAN_SWITCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tsan::__tsan_switch_to_fiber(tsan::target(to), 0)
    };
    // SAFETY: caller contract.
    unsafe { fiber_switch(save, to) }
}

#[cfg(all(test, feature = "tsan"))]
pub(crate) static TSAN_SWITCHES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// ThreadSanitizer's fibers (compiler-rt 19 include/sanitizer/tsan_interface.h:150-172):
/// every stack mapping is a fiber of its own, and so is each thread's own stack, under the
/// thread's fiber. A suspended fiber is only its stack pointer, so a switch finds the fiber
/// to resume by the range that holds `to`; a thread leaving its own stack registers that
/// stack's range first.
#[cfg(feature = "tsan")]
mod tsan {
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::sync::Mutex;

    unsafe extern "C" {
        fn __tsan_get_current_fiber() -> *mut c_void;
        fn __tsan_create_fiber(flags: u32) -> *mut c_void;
        fn __tsan_destroy_fiber(fiber: *mut c_void);
        pub(crate) fn __tsan_switch_to_fiber(fiber: *mut c_void, flags: u32);
    }

    /// Each range's low address: its high address and its fiber, for the stack mappings and for
    /// the threads' own stacks. A thread's reported stack may span mappings (the main thread's
    /// reaches its stack limit), so a mapping is looked up first.
    static MAPPINGS: Mutex<BTreeMap<usize, (usize, usize)>> = Mutex::new(BTreeMap::new());
    static THREADS: Mutex<BTreeMap<usize, (usize, usize)>> = Mutex::new(BTreeMap::new());

    fn find(ranges: &Mutex<BTreeMap<usize, (usize, usize)>>, at: usize) -> Option<usize> {
        let ranges = ranges.lock().unwrap();
        let (_, &(high, fiber)) = ranges.range(..=at).next_back()?;
        (at < high).then_some(fiber)
    }

    pub(super) fn create(low: *mut u8, high: *mut u8) -> *mut c_void {
        // SAFETY: creating a fiber has no preconditions.
        let fiber = unsafe { __tsan_create_fiber(0) };
        MAPPINGS
            .lock()
            .unwrap()
            .insert(low as usize, (high as usize, fiber as usize));
        fiber
    }

    pub(super) fn renew(low: *mut u8, high: *mut u8, old: *mut c_void) -> *mut c_void {
        let fiber = create(low, high);
        // SAFETY: the old fiber is no longer resumed; the running one is left to leak.
        if unsafe { __tsan_get_current_fiber() } != old {
            unsafe { __tsan_destroy_fiber(old) };
        }
        fiber
    }

    pub(super) fn destroy(low: *mut u8, fiber: *mut c_void) {
        MAPPINGS.lock().unwrap().remove(&(low as usize));
        // SAFETY: the stack's fiber runs nowhere any more.
        unsafe { __tsan_destroy_fiber(fiber) };
    }

    /// The fiber suspended at `to`, after registering the calling thread's own stack when it
    /// runs there.
    pub(super) fn target(to: *mut u8) -> *mut c_void {
        let here = std::ptr::addr_of!(to) as usize;
        if find(&MAPPINGS, here).is_none() {
            // SAFETY: the calling thread's attributes describe its own live stack.
            unsafe {
                let mut attr: libc::pthread_attr_t = std::mem::zeroed();
                if libc::pthread_getattr_np(libc::pthread_self(), &mut attr) == 0 {
                    let (mut low, mut size) = (std::ptr::null_mut(), 0);
                    libc::pthread_attr_getstack(&attr, &mut low, &mut size);
                    libc::pthread_attr_destroy(&mut attr);
                    let low = low as usize;
                    let fiber = __tsan_get_current_fiber() as usize;
                    THREADS.lock().unwrap().insert(low, (low + size, fiber));
                }
            }
        }
        find(&MAPPINGS, to as usize)
            .or_else(|| find(&THREADS, to as usize))
            .unwrap_or_else(|| crate::fault("a fiber switch to a stack of no fiber"))
            as *mut c_void
    }
}

#[cfg(target_arch = "x86_64")]
fn initial_frame(entry: Entry, first: *mut u8, second: *mut u8) -> [u64; 8] {
    // From the stack pointer up: the control words fiber_switch restores (MXCSR's default
    // 0x1F80 and the x87 control word's 0x037F), r15, r14, r13, r12, rbx, rbp, the return
    // address. After the return the stack pointer is 16-aligned, as `fiber_start`'s call needs.
    [
        0x037F_u64 << 32 | 0x1F80,
        0,
        entry as usize as u64,
        second as u64,
        first as u64,
        0,
        0,
        fiber_start as usize as u64,
    ]
}

#[cfg(all(target_arch = "x86_64", feature = "tsan"))]
pub(crate) unsafe extern "C" fn tsan_switch_target(to: *mut u8) -> *mut std::ffi::c_void {
    #[cfg(test)]
    TSAN_SWITCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    tsan::target(to)
}

#[cfg(all(target_arch = "x86_64", feature = "tsan"))]
pub(crate) use tsan::__tsan_switch_to_fiber as tsan_switch_to_fiber;

// An ahead handoff jumps here directly. Annotate beneath every caller, with
// no instrumented Rust frame alive when the TSan fiber changes. The temporary
// words are removed before the original 64-byte frame is saved.
#[cfg(all(target_arch = "x86_64", feature = "tsan"))]
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn fiber_switch(_save: *mut *mut u8, _to: *mut u8) {
    core::arch::naked_asm!(
        "push rdi", "push rsi", "sub rsp, 8",
        "mov rdi, rsi", "call {target}",
        "mov rdi, rax", "xor esi, esi", "call {annotate}",
        "add rsp, 8", "pop rsi", "pop rdi", "jmp {switch}",
        target = sym crate::tsan_callbacks::hirsel_tsan_switch_target,
        annotate = sym crate::tsan_callbacks::hirsel_tsan_switch_to_fiber,
        switch = sym fiber_switch_raw,
    )
}

#[cfg(target_arch = "x86_64")]
macro_rules! x86_switch {
    ($name:ident, $update:literal) => {
        #[unsafe(naked)]
        pub(crate) unsafe extern "C" fn $name(_save: *mut *mut u8, _to: *mut u8) {
            core::arch::naked_asm!(
                $update,
                "push rbp",
                "push rbx",
                "push r12",
                "push r13",
                "push r14",
                "push r15",
                "sub rsp, 8",
                "stmxcsr [rsp]",
                "fnstcw [rsp + 4]",
                "mov [rdi], rsp",
                "mov rsp, rsi",
                "ldmxcsr [rsp]",
                "fldcw [rsp + 4]",
                "add rsp, 8",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop rbx",
                "pop rbp",
                "ret",
            )
        }
    };
}
#[cfg(all(target_arch = "x86_64", not(feature = "tsan")))]
x86_switch!(fiber_switch, "");
#[cfg(all(target_arch = "x86_64", feature = "tsan"))]
x86_switch!(
    fiber_switch_raw,
    "
    mov r11, qword ptr fs:[hirsel_tsan_idle@TPOFF]
    test r11, r11
    je 3f
    cmp rdi, r11
    jne 2f
    mov byte ptr fs:[hirsel_tsan_private@TPOFF], 1
    jmp 3f
    2:
    cmp rsi, [r11]
    jne 3f
    mov byte ptr fs:[hirsel_tsan_private@TPOFF], 0
    3:
"
);

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn fiber_start() {
    core::arch::naked_asm!("mov rdi, r12", "mov rsi, r13", "call r14", "ud2")
}

#[cfg(target_arch = "aarch64")]
fn initial_frame(entry: Entry, first: *mut u8, second: *mut u8) -> [u64; 20] {
    // From the stack pointer up: x19 .. x28, x29 (frame pointer), x30 (return address),
    // d8 .. d15. x19, x20 and x21 carry the entry's arguments and the entry.
    let mut words = [0u64; 20];
    words[0] = first as u64;
    words[1] = second as u64;
    words[2] = entry as usize as u64;
    words[11] = fiber_start as usize as u64;
    words
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn fiber_switch(_save: *mut *mut u8, _to: *mut u8) {
    core::arch::naked_asm!(
        "sub sp, sp, #160",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "stp d8, d9, [sp, #96]",
        "stp d10, d11, [sp, #112]",
        "stp d12, d13, [sp, #128]",
        "stp d14, d15, [sp, #144]",
        "mov x2, sp",
        "str x2, [x0]",
        "mov sp, x1",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "ldp d8, d9, [sp, #96]",
        "ldp d10, d11, [sp, #112]",
        "ldp d12, d13, [sp, #128]",
        "ldp d14, d15, [sp, #144]",
        "add sp, sp, #160",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn fiber_start() {
    core::arch::naked_asm!("mov x0, x19", "mov x1, x20", "blr x21", "brk #0")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stacks returned to one worker, as compact leaves return them, stay
    /// mapped only up to its idle bound; the rest are unmapped.
    #[test]
    fn a_worker_keeps_at_most_its_idle_stacks_mapped() {
        let mut pool = StackPool::default();
        for returned in 1..=4 * IDLE_STACKS {
            pool.put(Stack::new());
            assert!(pool.0.len() <= IDLE_STACKS, "{returned} returned");
        }
        let taken: Vec<_> = (0..pool.0.len()).map(|_| pool.take()).collect();
        assert!(!taken.is_empty() && pool.0.is_empty());
    }

    #[test]
    fn pooled_stacks_preserve_the_hot_prefix_and_can_start_another_fiber() {
        let stack = Stack::new();
        let base = stack.base;
        // Dirty both deep and shallow pages; no continuation refers to either.
        unsafe {
            base.add(GUARD_BYTES).write_volatile(17);
            stack.top().sub(1024).write_volatile(42);
        }
        let mut pool = StackPool::default();
        pool.put(stack);
        let stack = pool.take();
        assert_eq!(stack.base, base);
        assert_eq!(unsafe { stack.top().sub(1024).read_volatile() }, 42);
        let mut ping = Ping {
            main: std::ptr::null_mut(),
            fiber: std::ptr::null_mut(),
            trace: Vec::new(),
            depth: 0,
        };
        let p = &raw mut ping;
        unsafe {
            ping.fiber = stack.prepare(entry, p.cast(), 7usize as *mut u8);
            switch(&raw mut ping.main, ping.fiber);
            switch(&raw mut ping.main, ping.fiber);
        }
        assert_eq!(ping.trace, [7, 3]);
        assert_eq!(ping.depth, 10_000);
    }

    struct Ping {
        main: *mut u8,
        fiber: *mut u8,
        trace: Vec<u32>,
        depth: u64,
    }

    unsafe extern "C" fn entry(ping: *mut u8, tag: *mut u8) -> ! {
        let ping = ping.cast::<Ping>();
        // SAFETY: the test's Ping outlives the fiber, which only runs while main waits.
        unsafe {
            (*ping).trace.push(tag as usize as u32);
            // A float survives a switch in a callee-saved register or on the stack.
            let kept = std::hint::black_box(1.5f64);
            switch(&raw mut (*ping).fiber, (*ping).main);
            (*ping).trace.push(2 + (kept == 1.5) as u32);
            // Recursion well past a page proves the reserved stack is usable.
            (*ping).depth = recurse(10_000);
            switch(&raw mut (*ping).fiber, (*ping).main);
        }
        unreachable!("the test never resumes a finished fiber")
    }

    #[inline(never)]
    fn recurse(n: u64) -> u64 {
        let frame = std::hint::black_box([n; 16]);
        if n == 0 {
            0
        } else {
            recurse(n - 1) + frame[15] / n
        }
    }

    #[test]
    fn a_fiber_suspends_and_resumes_with_its_own_stack_and_registers() {
        let stack = Stack::new();
        let mut ping = Ping {
            main: std::ptr::null_mut(),
            fiber: std::ptr::null_mut(),
            trace: Vec::new(),
            depth: 0,
        };
        let p = &raw mut ping;
        // SAFETY: the fiber runs only between these switches, and the stack outlives it.
        unsafe {
            (*p).fiber = stack.prepare(entry, p.cast(), 7usize as *mut u8);
            let before = std::hint::black_box(3.25f64);
            switch(&raw mut (*p).main, (*p).fiber);
            (*p).trace.push(1);
            switch(&raw mut (*p).main, (*p).fiber);
            assert_eq!(std::hint::black_box(before), 3.25);
        }
        assert_eq!(ping.trace, [7, 1, 3]);
        assert_eq!(ping.depth, 10_000);
    }

    /// A saved native stack is the free scheduler's image: it must cross the
    /// ownership CAS after the assembly switch wrote its stack pointer. Eight
    /// independent fibers run together, then rotate to the next OS thread at
    /// every save. This tests actual started stacks, not task identifiers.
    #[test]
    fn exclusive_saved_images_migrate_eight_started_native_stacks() {
        use hirsel_free::frame;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier, mpsc};
        use std::time::Duration;
        const WIDTH: usize = 8;
        const SAVES: usize = 32;
        struct Body {
            id: usize,
            worker: usize,
            return_slot: *mut *mut u8,
            sp: *mut u8,
            saves: usize,
            sum: u64,
            visits: Vec<usize>,
            gate: Arc<Barrier>,
            freed: Arc<AtomicUsize>,
        }
        impl Drop for Body {
            fn drop(&mut self) {
                self.freed.fetch_add(1, Ordering::Relaxed);
            }
        }
        struct Image {
            _stack: Stack,
            body: *mut Body,
        }
        impl Drop for Image {
            fn drop(&mut self) {
                // SAFETY: dropping execution ownership happens only on the
                // idle stack, after the native fiber's last save.
                unsafe { drop(Box::from_raw(self.body)) };
            }
        }
        // SAFETY: only Running owns the image and its native stack. A queue
        // token is produced after switch saved sp; its next owner overwrites
        // return_slot before resuming. No pointer is used by a notifier.
        unsafe impl Send for Image {}

        unsafe extern "C" fn moving(body: *mut u8, _: *mut u8) -> ! {
            let body = body.cast::<Body>();
            // SAFETY: this fiber runs only while its worker holds Running.
            unsafe {
                let id = (*body).id;
                let mut kept = std::hint::black_box([id as u64; 32]);
                // All eight fibers must enter before any can save its first
                // frame. The test carries no serial runtime run token.
                (*body).gate.wait();
                for step in 1..=SAVES {
                    (*body).visits.push((*body).worker);
                    for value in &mut kept {
                        *value += 1;
                    }
                    (*body).sum = kept.iter().sum();
                    (*body).saves = step;
                    switch(&raw mut (*body).sp, *(*body).return_slot);
                    assert_eq!(std::hint::black_box(kept), [id as u64 + step as u64; 32]);
                }
            }
            unreachable!("a completed native image was resumed")
        }

        let gate = Arc::new(Barrier::new(WIDTH));
        let freed = Arc::new(AtomicUsize::new(0));
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        let mut notifications = Vec::new();
        for id in 0..WIDTH {
            let (send, receive) = mpsc::channel();
            let stack = Stack::new();
            let body = Box::into_raw(Box::new(Body {
                id,
                worker: id,
                return_slot: std::ptr::null_mut(),
                sp: std::ptr::null_mut(),
                saves: 0,
                sum: 0,
                visits: Vec::new(),
                gate: gate.clone(),
                freed: freed.clone(),
            }));
            // SAFETY: admission owns the image, before any worker can start it.
            unsafe { (*body).sp = stack.prepare(moving, body.cast(), std::ptr::null_mut()) };
            let (wake, queued) = frame::prepared(Image {
                _stack: stack,
                body,
            });
            notifications.push(wake);
            assert!(send.send(queued).is_ok());
            senders.push(send);
            receivers.push(receive);
        }
        std::thread::scope(|threads| {
            for (index, receive) in receivers.into_iter().enumerate() {
                let send = senders[(index + 1) % WIDTH].clone();
                threads.spawn(move || {
                    let mut idle = std::ptr::null_mut();
                    for step in 1..=SAVES {
                        let queued = receive.recv_timeout(Duration::from_secs(10)).unwrap();
                        let mut running = queued.start();
                        let body = running.value().body;
                        // SAFETY: Running owns this saved stack, and no image
                        // borrow survives switching into its native frames.
                        unsafe {
                            (*body).worker = index;
                            (*body).return_slot = &raw mut idle;
                            let sp = (*body).sp;
                            switch(&raw mut idle, sp);
                            assert_eq!((*body).saves, step);
                        }
                        if step == SAVES {
                            // SAFETY: switch returned to idle and saved this image.
                            let body = unsafe { &*running.value().body };
                            assert_eq!(body.sum, 32 * (body.id + SAVES) as u64);
                            assert_eq!(
                                body.visits,
                                (0..SAVES)
                                    .map(|n| (body.id + n) % WIDTH)
                                    .collect::<Vec<_>>()
                            );
                            drop(running);
                        } else {
                            assert!(send.send(running.save(true).unwrap()).is_ok());
                        }
                    }
                });
            }
        });
        assert_eq!(freed.load(Ordering::Relaxed), WIDTH);
        assert!(notifications.iter().all(|wake| wake.notify().is_none()));
    }
}
