//! Per-program runtime roots. A compiler-generated constant names each selected
//! provider directly. ELF section collection drops unselected providers.
//! Embedding hosts use these weak defaults and keep the complete runtime;
//! images share their host's runtime rather than replacing its roots.

core::arch::global_asm!(
    ".pushsection .data.rel.ro.hirsel_runtime_runners,\"aw\",%progbits",
    ".weak hirsel.runtime.runners",
    ".hidden hirsel.runtime.runners",
    ".type hirsel.runtime.runners, %object",
    ".p2align 3",
    "hirsel.runtime.runners:",
    ".quad hirsel.runtime.runners.provider",
    ".size hirsel.runtime.runners, 8",
    ".popsection",
    ".pushsection .rodata.hirsel_runtime_tasks,\"a\",%progbits",
    ".weak hirsel.runtime.tasks",
    ".hidden hirsel.runtime.tasks",
    ".type hirsel.runtime.tasks, %object",
    "hirsel.runtime.tasks:",
    ".byte 1",
    ".size hirsel.runtime.tasks, 1",
    ".popsection",
);

pub(crate) fn runners() -> Option<&'static crate::runners::Launch> {
    unsafe extern "C" {
        #[link_name = "hirsel.runtime.runners"]
        static ROOT: *const std::ffi::c_void;
    }
    // SAFETY: the linker binds the immutable root to null or the runtime's
    // runner provider. Both have process lifetime.
    unsafe { ROOT.cast::<crate::runners::Launch>().as_ref() }
}

pub(crate) fn tasks() -> bool {
    unsafe extern "C" {
        #[link_name = "hirsel.runtime.tasks"]
        static ROOT: u8;
    }
    // SAFETY: the compiler or embedding default defines one immutable byte.
    unsafe { ROOT != 0 }
}
