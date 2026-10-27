//! Uninstrumented bridges. The logical TSan fiber stays unchanged while the
//! callback runs below the suspended native idle continuation. Explicit-PC
//! memory callbacks retain the emitted access location across the stack bridge.

core::arch::global_asm!(
    ".section .tbss,\"awT\",@nobits",
    ".p2align 3",
    ".globl hirsel_tsan_idle",
    ".hidden hirsel_tsan_idle",
    ".type hirsel_tsan_idle,@object",
    "hirsel_tsan_idle: .zero 8",
    ".size hirsel_tsan_idle,8",
    ".globl hirsel_tsan_private",
    ".hidden hirsel_tsan_private",
    ".type hirsel_tsan_private,@object",
    "hirsel_tsan_private: .zero 1",
    ".size hirsel_tsan_private,1",
    ".text",
);

/// # Safety
/// Idle only. `idle` is the current native dispatcher's live saved-SP word.
/// It remains valid until `clear`, after switching back to idle.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn bind(idle: *mut *mut u8) {
    core::arch::naked_asm!(
        "mov qword ptr fs:[hirsel_tsan_idle@TPOFF], rdi",
        "mov byte ptr fs:[hirsel_tsan_private@TPOFF], 0",
        "ret",
    );
}

/// # Safety
/// The dispatcher has returned to idle and will not enter its private image
/// again without rebinding on the executing OS thread.
#[unsafe(naked)]
pub(crate) unsafe extern "C" fn clear() {
    core::arch::naked_asm!("mov qword ptr fs:[hirsel_tsan_idle@TPOFF], 0", "ret",);
}

macro_rules! bridge {
    ($name:ident, $target:path, $pc:literal) => {
        /// # Safety
        /// Same integer-register ABI as the named compiler-rt entry. Private
        /// execution has a live idle binding installed by the dispatcher. The
        /// idle continuation is suspended and its stack below saved SP is free.
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                $pc,
                "cmp byte ptr fs:[hirsel_tsan_private@TPOFF], 0",
                "je 2f",
                "push r12",
                "mov r12, rsp",
                "mov r11, qword ptr fs:[hirsel_tsan_idle@TPOFF]",
                "mov rsp, [r11]",
                "and rsp, -16",
                "call {target}",
                "mov rsp, r12",
                "pop r12",
                "ret",
                "2: jmp {target}",
                target = sym $target,
            );
        }
    };
}
macro_rules! callbacks {
    ($(($name:ident, $raw:ident, $target:ident, $pc:literal),)*) => {
        $(mod $name {
            unsafe extern "C" {
                #[link_name = stringify!($target)]
                pub(super) fn callback();
            }
        }
        bridge!($name, $name::callback, $pc);)*
    };
}
hirsel_core_contract::tsan_callbacks!(callbacks);
bridge!(
    hirsel_tsan_switch_target,
    crate::fiber::tsan_switch_target,
    ""
);
bridge!(
    hirsel_tsan_switch_to_fiber,
    crate::fiber::tsan_switch_to_fiber,
    ""
);

#[cfg(test)]
pub(crate) mod tests {
    unsafe extern "C" fn target(out: *mut u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64 {
        let marker = 0u64;
        // SAFETY: the control owns seven output words across the synchronous call.
        unsafe {
            out.write((&raw const marker) as u64);
            for (i, value) in [a, b, c, d, e].into_iter().enumerate() {
                out.add(i + 1).write(value);
            }
        }
        0x123456789abcdef0
    }
    bridge!(hirsel_tsan_callback_probe, target, "");

    pub(crate) unsafe fn probe(out: *mut u64) -> u64 {
        unsafe extern "C" {
            #[link_name = "hirsel_tsan_callback_probe"]
            fn call(out: *mut u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> u64;
        }
        // SAFETY: the bridge preserves all six argument registers and RAX.
        unsafe { call(out, 11, 22, 33, 44, 55) }
    }
}
