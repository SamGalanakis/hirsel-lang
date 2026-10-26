//! Logical invocation admission before compiled calls and primitive effects.
use crate::{STATUS_FAILURE, STATUS_OK, context::TaskCtx};
pub(crate) mod storage;

/// Validated LANG-10 bound, including the independent cleanup allowance.
/// Allocate this storage before the task starts; admission never allocates.
pub(crate) const INVOCATION_CAPACITY: usize = hirsel_core_contract::depth::FOREGROUND_LIMIT
    as usize
    + hirsel_core_contract::depth::CLEANUP_LIMIT as usize;

#[cfg(target_arch = "x86_64")]
mod native;
#[cfg(target_arch = "x86_64")]
pub use native::{
    hirsel_rt_v2_depth_enter, hirsel_rt_v2_depth_enter_at_site, hirsel_rt_v2_depth_leave,
};

/// Admit one invocation on this task. Failure records belong to the compiler's
/// LANG-10 edge, so rejection changes no arguments, effects or failure state.
///
/// # Safety
/// `ctx` is the live running task, exclusively borrowed for this entry.
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
#[inline(always)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_enter(ctx: *mut TaskCtx) -> u8 {
    // SAFETY: same task and admission contract; no source pair at this boundary.
    unsafe { hirsel_rt_v2_depth_enter_at_site(ctx, 0, 0) }
}

/// Fixed-extent leaf admission, retaining the invoking source pair. Both paths
/// contain only task-owned bounded loads/stores and counter comparisons.
/// Failure is recorded only by the compiler's existing LANG-10 failure edge.
///
/// # Safety
/// `ctx` is this thread's live running task, exclusively borrowed for entry.
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
#[inline(always)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_enter_at_site(
    ctx: *mut TaskCtx,
    anchor: u64,
    operation_site: u64,
) -> u8 {
    // SAFETY: caller owns this live context and its task-created fixed arrays.
    let ctx = unsafe { &mut *ctx };
    let index = ctx.invocation_depth;
    if index >= INVOCATION_CAPACITY {
        return STATUS_FAILURE;
    }
    let admitted = if ctx.cleanup_depth != 0 {
        ctx.activations.admit_finalizer()
    } else {
        ctx.activations.admit()
    };
    let Some(activation) = admitted else {
        return STATUS_FAILURE;
    };
    // SAFETY: the bound above proves both preallocated slots exist. These slots
    // are outside the initialized prefix; no previous linear token is replaced.
    unsafe {
        ctx.active_invocations
            .as_mut_ptr()
            .wrapping_add(index)
            .write(std::mem::MaybeUninit::new(activation));
        ctx.invocation_sites
            .as_mut_ptr()
            .wrapping_add(index)
            .write(std::mem::MaybeUninit::new([anchor, operation_site]));
    }
    ctx.invocation_depth = index.wrapping_add(1);
    STATUS_OK
}

/// The innermost admitted invocation's site, outside the certified leaf span.
fn invocation_site(ctx: &TaskCtx) -> Option<(u64, u64)> {
    if ctx.invocation_depth == 0 {
        None
    } else {
        // SAFETY: entry initialized the complete live prefix before publishing
        // its length; sites are Copy and remain live until matching retirement.
        let [anchor, operation_site] = unsafe {
            ctx.invocation_sites
                .as_ptr()
                .wrapping_add(ctx.invocation_depth - 1)
                .read()
                .assume_init()
        };
        Some((anchor, operation_site))
    }
}

/// Runtime-only callback boundaries inherit the invoking language operation.
/// They have no LIR failure edge, so rejection records LANG-10 here.
/// # Safety
/// `ctx` is the exclusively running task.
pub(crate) unsafe fn enter_callback(ctx: *mut TaskCtx) -> u8 {
    // SAFETY: the task and its admission stack belong to the running caller.
    let (anchor, operation_site) = unsafe { invocation_site(&*ctx).unwrap_or((0, 0)) };
    // SAFETY: same task, no operand has been consumed.
    let status = unsafe { hirsel_rt_v2_depth_enter_at_site(ctx, anchor, operation_site) };
    if status != STATUS_OK {
        crate::failure::record_runtime(ctx, "LANG-10", anchor, operation_site);
    }
    status
}

/// Fixed-extent leaf retirement after all invocation cleanup.
///
/// # Safety
/// `ctx` is the live running task and the matching entry succeeded. Validated
/// native control flow retires each admitted token once, in nesting order.
#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
#[inline(always)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_leave(ctx: *mut TaskCtx) {
    // SAFETY: the matched-entry contract proves a nonempty initialized prefix.
    let ctx = unsafe { &mut *ctx };
    let index = ctx.invocation_depth.wrapping_sub(1);
    // SAFETY: index names the uniquely owned last live admission token.
    let activation = unsafe {
        ctx.active_invocations
            .as_ptr()
            .wrapping_add(index)
            .read()
            .assume_init()
    };
    ctx.invocation_depth = index;
    // SAFETY: this token is consumed once by the validated matching retirement.
    unsafe { ctx.activations.retire_unchecked(activation) };
}

/// Complete structural depth admission without argument transfer or effects.
///
/// # Safety
/// The program and edges are emitted from a validated AccessPlan; value is a
/// live borrowed operand of that plan's root layout. Both arrays remain live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_depth_preflight(
    _ctx: *mut TaskCtx,
    program: *const std::ffi::c_void,
    program_length: u64,
    edges: *const std::ffi::c_void,
    edge_count: u64,
    value: *const std::ffi::c_void,
) -> u8 {
    // SAFETY: caller owns these static arrays and the live operand.
    let admitted = unsafe {
        let words = std::slice::from_raw_parts(program.cast::<u64>(), program_length as usize);
        let edges = std::slice::from_raw_parts(edges.cast::<u8>(), edge_count as usize);
        crate::codec::structural::preflight(words, edges, value.cast())
    };
    if admitted { STATUS_OK } else { STATUS_FAILURE }
}

/// Compare two already admitted operands using a validated physical program.
/// # Safety
/// The static program matches both live borrowed operand addresses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_structural_order(
    ctx: *mut TaskCtx,
    program: *const std::ffi::c_void,
    program_length: u64,
    left: *const std::ffi::c_void,
    right: *const std::ffi::c_void,
) -> u8 {
    // SAFETY: the compiler validates the program and borrows both operands.
    unsafe {
        let words = std::slice::from_raw_parts(program.cast::<u64>(), program_length as usize);
        crate::codec::value_access::compare(words, ctx, left.cast(), right.cast())
    }
}

/// Hash an already admitted operand without changing it or its owner.
/// # Safety
/// The static program matches the live borrowed operand address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_structural_hash(
    ctx: *mut TaskCtx,
    program: *const std::ffi::c_void,
    program_length: u64,
    value: *const std::ffi::c_void,
    seed: u64,
) -> u64 {
    // SAFETY: the compiler validates the program and borrows the operand.
    unsafe {
        let words = std::slice::from_raw_parts(program.cast::<u64>(), program_length as usize);
        crate::codec::value_access::hash(words, ctx, value.cast(), seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_calls_borrow_program_words_without_allocating() {
        use crate::io_real::borrowed_net_tests::allocation_calls;
        let words = [
            crate::codec::CODEC_VERSION,
            1,
            0,
            0,
            0,
            0,
            u64::MAX,
            8,
            crate::codec::NODE_SCALAR,
            4,
        ];
        let value = 17u64;
        let before = allocation_calls();
        // SAFETY: one initialized scalar and its exact static program; no
        // operation uses task state or changes the borrowed value.
        unsafe {
            for _ in 0..16 {
                assert!(crate::codec::structural::preflight(
                    &words,
                    &[0],
                    std::ptr::from_ref(&value).cast()
                ));
                assert_eq!(
                    crate::codec::value_access::compare(
                        &words,
                        std::ptr::null_mut(),
                        std::ptr::from_ref(&value).cast(),
                        std::ptr::from_ref(&value).cast()
                    ),
                    0
                );
                let _ = crate::codec::value_access::hash(
                    &words,
                    std::ptr::null_mut(),
                    std::ptr::from_ref(&value).cast(),
                    3,
                );
            }
        }
        assert_eq!(
            allocation_calls(),
            before,
            "structural calls allocate runtime metadata or scratch"
        );
    }

    #[test]
    fn fixed_storage_preserves_both_allowances_tokens_and_sites_at_the_bound() {
        use hirsel_core_contract::depth::{CLEANUP_LIMIT, FOREGROUND_LIMIT};
        let config = crate::Config::new();
        let mut ctx = std::ptr::null_mut();
        // SAFETY: the test owns one live task; every successful entry is retired
        // in nesting order before destruction, including during cleanup mode.
        unsafe {
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                0
            );
            let tokens = (*ctx).active_invocations.as_ptr();
            let sites = (*ctx).invocation_sites.as_ptr();
            assert_eq!((&(*ctx).active_invocations).len(), INVOCATION_CAPACITY);
            assert_eq!((&(*ctx).invocation_sites).len(), INVOCATION_CAPACITY);
            for n in 0..FOREGROUND_LIMIT {
                assert_eq!(
                    hirsel_rt_v2_depth_enter_at_site(ctx, 1, u64::from(n)),
                    STATUS_OK
                );
            }
            // Counter exhaustion while the storage still has cleanup slots.
            assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 2, 99), STATUS_FAILURE);
            assert_eq!((*ctx).activations.live(), (FOREGROUND_LIMIT, 0));
            assert_eq!(
                invocation_site(&*ctx),
                Some((1, u64::from(FOREGROUND_LIMIT - 1)))
            );
            (*ctx).cleanup_depth = 1;
            for n in 0..CLEANUP_LIMIT {
                assert_eq!(
                    hirsel_rt_v2_depth_enter_at_site(ctx, 3, u64::from(n)),
                    STATUS_OK
                );
            }
            assert_eq!((*ctx).invocation_depth, INVOCATION_CAPACITY);
            // Full storage rejects before touching the last token or source pair.
            assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 4, 99), STATUS_FAILURE);
            assert_eq!((*ctx).activations.live(), (FOREGROUND_LIMIT, CLEANUP_LIMIT));
            assert_eq!(
                invocation_site(&*ctx),
                Some((3, u64::from(CLEANUP_LIMIT - 1)))
            );
            assert!(!(*ctx).failure.active());
            (*ctx).cleanup_depth = 0;
            hirsel_rt_v2_depth_leave(ctx);
            // Retained cleanup tokens, rather than the dynamic cleanup marker,
            // determine the allowance for a nested callback and its retirement.
            assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 5, 6), STATUS_OK);
            assert_eq!((*ctx).activations.live(), (FOREGROUND_LIMIT, CLEANUP_LIMIT));
            hirsel_rt_v2_depth_leave(ctx);
            for _ in 1..CLEANUP_LIMIT {
                hirsel_rt_v2_depth_leave(ctx);
            }
            assert_eq!((*ctx).activations.live(), (FOREGROUND_LIMIT, 0));
            assert_eq!(
                invocation_site(&*ctx),
                Some((1, u64::from(FOREGROUND_LIMIT - 1)))
            );
            assert_eq!(hirsel_rt_v2_depth_enter(ctx), STATUS_FAILURE);
            for _ in 0..FOREGROUND_LIMIT {
                hirsel_rt_v2_depth_leave(ctx);
            }
            assert_eq!((*ctx).activations.live(), (0, 0));
            assert_eq!((*ctx).invocation_depth, 0);
            assert_eq!(invocation_site(&*ctx), None);
            assert_eq!((*ctx).active_invocations.as_ptr(), tokens);
            assert_eq!((*ctx).invocation_sites.as_ptr(), sites);
            // A later invocation reuses the initialized storage without a reset.
            assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 7, 8), STATUS_OK);
            assert_eq!(invocation_site(&*ctx), Some((7, 8)));
            hirsel_rt_v2_depth_leave(ctx);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }

    #[test]
    fn moving_a_context_preserves_depth_aliases_and_reuses_only_live_sites() {
        use crate::io_real::borrowed_net_tests::{allocation_calls, count_deallocation};
        let mut boxed = Box::new(TaskCtx::new(std::ptr::null_mut(), std::ptr::null_mut()));
        // SAFETY: this test exclusively owns scratch; admission never uses worker/run.
        unsafe {
            assert_eq!(
                hirsel_rt_v2_depth_enter_at_site(&mut *boxed, 11, 12),
                STATUS_OK
            );
            boxed.cleanup_depth = 1;
            assert_eq!(
                hirsel_rt_v2_depth_enter_at_site(&mut *boxed, 21, 22),
                STATUS_OK
            );
            let token = boxed.activations.admit_finalizer().unwrap();
            boxed.cleanup_invocations.push(token);
            let tokens = boxed.invocation_tokens;
            let sites = boxed.invocation_site_slots;
            let allocation = boxed.invocation_sites.as_ptr().addr();
            // Same move as spawn::flush: the context leaves its box; aliases survive.
            let mut moved = *boxed;
            assert_eq!(moved.invocation_tokens, tokens);
            assert_eq!(moved.invocation_site_slots, sites);
            assert_eq!(invocation_site(&moved), Some((21, 22)));
            let before = allocation_calls();
            let token = moved.cleanup_invocations.pop().unwrap();
            moved.activations.retire(token);
            moved.cleanup_depth = 0;
            hirsel_rt_v2_depth_leave(&mut moved);
            assert_eq!(invocation_site(&moved), Some((11, 12)));
            hirsel_rt_v2_depth_leave(&mut moved);
            assert_eq!(invocation_site(&moved), None);
            assert_eq!(
                hirsel_rt_v2_depth_enter_at_site(&mut moved, 31, 32),
                STATUS_OK
            );
            assert_eq!(invocation_site(&moved), Some((31, 32)));
            hirsel_rt_v2_depth_leave(&mut moved);
            assert_eq!(moved.activations.live(), (0, 0));
            assert_eq!(
                allocation_calls(),
                before,
                "admission and retirement never allocate"
            );
            assert_eq!(count_deallocation(allocation, || drop(moved)), 1);
        }
        let fresh = TaskCtx::new(std::ptr::null_mut(), std::ptr::null_mut());
        assert_eq!(invocation_site(&fresh), None);
        assert_eq!(fresh.activations.live(), (0, 0));
        assert_eq!(fresh.cleanup_invocations.len(), 0);
    }

    #[test]
    fn depth_owner_is_destroyed_once_after_success_failure_or_abandoned_cleanup() {
        use crate::io_real::borrowed_net_tests::count_deallocation;
        for mode in 0..3 {
            let mut ctx = Box::new(TaskCtx::new(std::ptr::null_mut(), std::ptr::null_mut()));
            // SAFETY: the exclusively owned context has no worker-bound state.
            unsafe {
                assert_eq!(
                    hirsel_rt_v2_depth_enter_at_site(&mut *ctx, 41, 42),
                    STATUS_OK
                );
                if mode == 0 {
                    hirsel_rt_v2_depth_leave(&mut *ctx);
                } else if mode == 1 {
                    ctx.failure.record(crate::failure::FailureRecord {
                        kind: 1,
                        rule: 1,
                        site: 41,
                        operation_site: 42,
                    });
                    hirsel_rt_v2_depth_leave(&mut *ctx);
                } else {
                    ctx.cleanup_depth = 1;
                    assert_eq!(
                        hirsel_rt_v2_depth_enter_at_site(&mut *ctx, 51, 52),
                        STATUS_OK
                    );
                    let token = ctx.activations.admit_finalizer().unwrap();
                    ctx.cleanup_invocations.push(token);
                    // A stop abandons the native cleanup frames and their tokens.
                }
            }
            let allocation = ctx.invocation_sites.as_ptr().addr();
            let address = Box::into_raw(ctx).addr();
            // Tokio task_abort.rs:48-110: ownership can be destroyed on another
            // thread after suspension. No thread retains a borrow or runtime binding.
            let frees = std::thread::spawn(move || {
                count_deallocation(allocation, || {
                    // SAFETY: the transferred box is uniquely owned by this thread.
                    unsafe { drop(Box::from_raw(address as *mut TaskCtx)) };
                })
            })
            .join()
            .unwrap();
            assert_eq!(frees, 1, "exit mode {mode}");
        }
    }

    #[test]
    fn runtime_callbacks_admit_at_the_invoker_and_retire_without_leaking() {
        let config = crate::Config::new();
        let mut ctx = std::ptr::null_mut();
        // SAFETY: every admitted invocation is retired before context destruction.
        unsafe {
            assert_eq!(
                crate::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
                0
            );
            for _ in 1..hirsel_core_contract::depth::FOREGROUND_LIMIT {
                assert_eq!(hirsel_rt_v2_depth_enter_at_site(ctx, 77, 88), STATUS_OK);
            }
            assert_eq!(enter_callback(ctx), STATUS_OK);
            assert_eq!(enter_callback(ctx), STATUS_FAILURE);
            let record = (*ctx).failure.chain(&[])[0].0;
            assert_eq!(
                record.rule,
                hirsel_core_contract::runtime_rule_code("LANG-10").unwrap()
            );
            assert_eq!((record.site, record.operation_site), (77, 88));
            hirsel_rt_v2_depth_leave(ctx);
            assert_eq!(invocation_site(&*ctx), Some((77, 88)));
            assert_eq!(enter_callback(ctx), STATUS_OK);
            hirsel_rt_v2_depth_leave(ctx);
            for _ in 1..hirsel_core_contract::depth::FOREGROUND_LIMIT {
                hirsel_rt_v2_depth_leave(ctx);
            }
            assert_eq!((*ctx).activations.live(), (0, 0));
            assert_eq!((*ctx).invocation_depth, 0);
            assert_eq!(invocation_site(&*ctx), None);
            crate::context::hirsel_rt_v2_ctx_destroy(ctx);
        }
    }
}
