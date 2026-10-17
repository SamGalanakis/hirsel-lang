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
//! thread's: environmental termination, never a language failure. Valgrind is told about each
//! stack (`STACK_REGISTER`) so a switch is not mistaken for a huge frame.

use crate::environmental;

/// Usable bytes of a fiber stack: the default main-thread limit, reserved lazily.
pub(crate) const STACK_BYTES: usize = 8 << 20;
/// Inaccessible bytes below each stack, so a frame larger than a page still lands in them.
pub(crate) const GUARD_BYTES: usize = 64 << 10;

/// The entry a fresh fiber starts in, with the two words [`Stack::prepare`] stored.
pub(crate) type Entry = unsafe extern "C" fn(*mut u8, *mut u8) -> !;

/// One guarded stack mapping.
pub(crate) struct Stack {
    base: *mut u8,
    valgrind: u64,
}

impl Stack {
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
        Stack { base, valgrind }
    }

    /// One past the highest usable byte, 16-aligned.
    fn top(&self) -> *mut u8 {
        self.base.wrapping_add(GUARD_BYTES + STACK_BYTES)
    }

    /// The stack pointer of a fiber that, when switched to, calls `entry(first, second)`.
    pub(crate) fn prepare(&self, entry: Entry, first: *mut u8, second: *mut u8) -> *mut u8 {
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
    // SAFETY: caller contract.
    unsafe { fiber_switch(save, to) }
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

#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
unsafe extern "C" fn fiber_switch(_save: *mut *mut u8, _to: *mut u8) {
    core::arch::naked_asm!(
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
}
