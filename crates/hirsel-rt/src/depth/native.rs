//! The exact x86 SysV leaf span admitted on E1 private stacks.
//! No instruction changes the stack or calls another function. The entire
//! call extent is the caller-pushed eight-byte return address.
use crate::context::TaskCtx;

/// Fixed call footprint, including the caller's return address.
pub const STACK_BYTES: u64 = 8;

/// # Safety
/// `ctx` owns its initialized fixed-capacity task storage exclusively.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_enter(_ctx: *mut TaskCtx) -> u8 {
    core::arch::naked_asm!(
        "xor esi, esi",
        "xor edx, edx",
        "jmp {at_site}",
        at_site = sym hirsel_rt_v2_depth_enter_at_site,
    );
}

/// # Safety
/// `ctx` owns its initialized fixed-capacity task storage exclusively.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_enter_at_site(
    _ctx: *mut TaskCtx,
    _anchor: u64,
    _operation_site: u64,
) -> u8 {
    core::arch::naked_asm!(
        "mov rax, [rdi + {depth}]",
        "cmp rax, {capacity}",
        "jae 4f",
        "mov ecx, [rdi + {cleanup_mode}]",
        "movzx r8d, word ptr [rdi + {cleanup}]",
        "test ecx, ecx",
        "jnz 2f",
        "test r8d, r8d",
        "jnz 2f",
        "cmp word ptr [rdi + {foreground}], {foreground_limit}",
        "jae 4f",
        "inc word ptr [rdi + {foreground}]",
        "xor r8d, r8d",
        "jmp 3f",
        "2:",
        "cmp r8w, {cleanup_limit}",
        "jae 4f",
        "inc word ptr [rdi + {cleanup}]",
        "mov r8d, 1",
        "3:",
        "mov rcx, [rdi + {tokens}]",
        "mov [rcx + rax], r8b",
        "mov rcx, [rdi + {sites}]",
        "mov r8, rax",
        "shl r8, 4",
        "mov [rcx + r8], rsi",
        "mov [rcx + r8 + 8], rdx",
        "inc rax",
        "mov [rdi + {depth}], rax",
        "xor eax, eax",
        "ret",
        "4:",
        "mov eax, 1",
        "ret",
        depth = const std::mem::offset_of!(TaskCtx, invocation_depth),
        capacity = const super::INVOCATION_CAPACITY,
        cleanup_mode = const std::mem::offset_of!(TaskCtx, cleanup_depth),
        foreground = const std::mem::offset_of!(TaskCtx, activations),
        cleanup = const std::mem::offset_of!(TaskCtx, activations) + 2,
        foreground_limit = const hirsel_core_contract::depth::FOREGROUND_LIMIT,
        cleanup_limit = const hirsel_core_contract::depth::CLEANUP_LIMIT,
        tokens = const std::mem::offset_of!(TaskCtx, invocation_tokens),
        sites = const std::mem::offset_of!(TaskCtx, invocation_site_slots),
    );
}

/// # Safety
/// The validated native CFG matches this exit to its live, successful entry.
/// The last initialized token belongs to this context and is retired once.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_leave(_ctx: *mut TaskCtx) {
    core::arch::naked_asm!(
        "mov rax, [rdi + {depth}]",
        "dec rax",
        "mov r8, [rdi + {tokens}]",
        "movzx ecx, byte ptr [r8 + rax]",
        "mov [rdi + {depth}], rax",
        "test ecx, ecx",
        "jnz 2f",
        "dec word ptr [rdi + {foreground}]",
        "ret",
        "2:",
        "dec word ptr [rdi + {cleanup}]",
        "ret",
        depth = const std::mem::offset_of!(TaskCtx, invocation_depth),
        foreground = const std::mem::offset_of!(TaskCtx, activations),
        cleanup = const std::mem::offset_of!(TaskCtx, activations) + 2,
        tokens = const std::mem::offset_of!(TaskCtx, invocation_tokens),
    );
}

const _: () = {
    // The shared contract declares these repr(C)/repr(transparent). These are
    // native byte accesses, so reject a width/representation drift at compile time.
    assert!(STACK_BYTES == crate::DEPTH_LEAF_STACK_BYTES);
    assert!(size_of::<hirsel_core_contract::depth::Activations>() == 4);
    assert!(align_of::<hirsel_core_contract::depth::Activations>() == 2);
    assert!(size_of::<hirsel_core_contract::depth::Activation>() == 1);
    assert!(size_of::<[u64; 2]>() == 16);
};
