//! Finalizers: shielding, failure accumulation with the primary preserved, field continuation,
//! inline continuations and FAIL-3 termination for unsafe-ffi drops.

use crate::support;

use std::cell::Cell;
use std::mem::MaybeUninit;
use std::ptr;

use hirsel_rt::alloc::hirsel_rt_v2_alloc;
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::failure::{
    hirsel_rt_v2_failure_append, hirsel_rt_v2_failure_begin, hirsel_rt_v2_failure_cause,
    hirsel_rt_v2_failure_cause_count, hirsel_rt_v2_failure_clear, hirsel_rt_v2_failure_primary,
};
use hirsel_rt::finalize::{
    hirsel_rt_v2_close_handle, hirsel_rt_v2_finalize, hirsel_rt_v2_finalize_begin,
    hirsel_rt_v2_finalize_end, hirsel_rt_v2_shielded,
};
use hirsel_rt::release::hirsel_rt_v2_release;
use hirsel_rt::{Cleanup, DOMAIN_HEAP, FailureRecord, Header, LAYOUT_UNSAFE_FFI, Layout, TaskCtx};
use support::*;

fn node(ctx: *mut TaskCtx, layout: &Layout, id: u64) -> *mut Header {
    let block =
        unsafe { hirsel_rt_v2_alloc(ctx, index(ctx, layout), layout.size, 8, DOMAIN_HEAP, 0) };
    write_u64(block, 0, id);
    block
}

fn release(ctx: *mut TaskCtx, block: *mut Header) -> u8 {
    unsafe { hirsel_rt_v2_release(ctx, block, (*block).index(), ptr::null(), 0, 0) }
}

fn primary(ctx: &Ctx) -> Option<FailureRecord> {
    let mut record = MaybeUninit::uninit();
    (unsafe { hirsel_rt_v2_failure_primary(ctx.0, record.as_mut_ptr()) } == 1)
        .then(|| unsafe { record.assume_init() })
}

fn cause_rules(ctx: &Ctx) -> Vec<u32> {
    let count = unsafe { hirsel_rt_v2_failure_cause_count(ctx.0) };
    (0..count)
        .map(|index| {
            let mut record = MaybeUninit::uninit();
            assert_eq!(
                unsafe { hirsel_rt_v2_failure_cause(ctx.0, index, record.as_mut_ptr()) },
                1
            );
            unsafe { record.assume_init() }.rule
        })
        .collect()
}

fn ids(events: Vec<(u64, u64)>) -> Vec<u64> {
    events.into_iter().map(|(id, _)| id).collect()
}

/// `{ id, a: ref, b: ref }` with the given finalizer; fields drop b then a.
fn pair(finalizer: hirsel_rt::DropThunk) -> &'static Layout {
    cached_layout("finalizer-pair", finalizer as usize as u64, || {
        layout(
            24,
            3,
            Some(finalizer),
            vec![release_step(2, 16), release_step(1, 8)],
        )
    })
}
fn resource(finalizer: hirsel_rt::DropThunk) -> &'static Layout {
    cached_layout("finalizer-resource", finalizer as usize as u64, || {
        layout(8, 1, Some(finalizer), Vec::new())
    })
}
fn context() -> Ctx {
    pair(log_drop);
    pair(failing_drop);
    resource(log_drop);
    resource(failing_drop);
    Ctx::diagnostic()
}

#[test]
fn finalizer_failures_keep_the_first_as_primary_and_release_everything_once() {
    let ctx = context();
    // root(1, fails) { a: A(2, fails) { a: x(4), b: y(5, fails) }, b: B(3) }
    let a = node(ctx.0, pair(failing_drop), 2);
    write_ref(a, 8, node(ctx.0, resource(log_drop), 4));
    write_ref(a, 16, node(ctx.0, resource(failing_drop), 5));
    let b = node(ctx.0, resource(log_drop), 3);
    let root = node(ctx.0, pair(failing_drop), 1);
    write_ref(root, 8, a);
    write_ref(root, 16, b);
    take_events();
    assert_eq!(
        release(ctx.0, root),
        1,
        "the failure is reported after cleanup finishes"
    );
    assert_eq!(
        ids(take_events()),
        [1, 3, 2, 5, 4],
        "every finalizer ran exactly once, in drop order"
    );
    assert_eq!(primary(&ctx).unwrap().rule, 1);
    assert_eq!(cause_rules(&ctx), [2, 5]);
    assert_eq!(
        ctx.stats().live_blocks,
        0,
        "fields and siblings of failed finalizers were released"
    );
    unsafe { hirsel_rt_v2_failure_clear(ctx.0) };
    assert!(primary(&ctx).is_none());
}

#[test]
fn an_earlier_language_failure_stays_primary_during_cleanup() {
    let ctx = context();
    index(ctx.0, resource(failing_drop));
    unsafe { hirsel_rt_v2_failure_begin(ctx.0, 3, 900, 42, 42) };
    let block = node(ctx.0, resource(failing_drop), 7);
    assert_eq!(release(ctx.0, block), 1);
    let first = primary(&ctx).unwrap();
    assert_eq!((first.kind, first.rule, first.site), (3, 900, 42));
    assert_eq!(cause_rules(&ctx), [7]);
    let external = FailureRecord {
        kind: 1,
        rule: 8,
        site: 0,
        operation_site: 0,
    };
    unsafe { hirsel_rt_v2_failure_append(ctx.0, &external) };
    assert_eq!(cause_rules(&ctx), [7, 8]);
}

#[test]
fn a_field_shared_with_another_holder_is_released_exactly_once_after_a_failure() {
    // Resources are noncopyable, so the shared child is a plain block.
    let ctx = context();
    let one = layout(16, 2, Some(failing_drop), vec![release_step(1, 8)]);
    let child = node(ctx.0, leaf(8), 9);
    unsafe { hirsel_rt_v2_dup(ctx.0, child) };
    let parent = node(ctx.0, one, 1);
    write_ref(parent, 8, child);
    take_events();
    assert_eq!(release(ctx.0, parent), 1);
    assert_eq!(ids(take_events()), [1]);
    assert_eq!(
        count(child),
        1,
        "the failed parent released its credit once"
    );
    assert_eq!(ctx.stats().live_blocks, 1);
    assert_eq!(release(ctx.0, child), 0);
    assert_eq!(ctx.stats().live_blocks, 0);
}

thread_local! {
    static SEEN_SHIELD: Cell<u8> = const { Cell::new(9) };
}

unsafe extern "C" fn shield_probe(
    ctx: *mut TaskCtx,
    _value: *mut u8,
    _anchor: u64,
    _operation_site: u64,
) -> u8 {
    SEEN_SHIELD.with(|seen| seen.set(unsafe { hirsel_rt_v2_shielded(ctx) }));
    0
}

#[test]
fn finalizers_run_shielded() {
    let ctx = context();
    assert_eq!(unsafe { hirsel_rt_v2_shielded(ctx.0) }, 0);
    release(ctx.0, node(ctx.0, resource(shield_probe), 1));
    assert_eq!(SEEN_SHIELD.with(Cell::get), 1);
    assert_eq!(unsafe { hirsel_rt_v2_shielded(ctx.0) }, 0);
}

/// Compiled code brackets a drop body it calls itself with finalize_begin and finalize_end,
/// which nest; an end without its begin is an invariant fault.
#[test]
fn compiled_finalization_shields_until_its_end() {
    if in_child("compiled_finalization_shields_until_its_end") {
        let ctx = context();
        unsafe { hirsel_rt_v2_finalize_end(ctx.0, 7) };
        return;
    }
    assert_child_aborts(
        "compiled_finalization_shields_until_its_end",
        "a finalizer end without its begin",
    );
    // Accepted twin: balanced and nested brackets.
    let ctx = context();
    unsafe {
        hirsel_rt_v2_finalize_begin(ctx.0, 1);
        assert_eq!(hirsel_rt_v2_shielded(ctx.0), 1);
        hirsel_rt_v2_finalize_begin(ctx.0, 2);
        hirsel_rt_v2_finalize_end(ctx.0, 2);
        assert_eq!(hirsel_rt_v2_shielded(ctx.0), 1);
        hirsel_rt_v2_finalize_end(ctx.0, 1);
        assert_eq!(hirsel_rt_v2_shielded(ctx.0), 0);
    }
}

thread_local! {
    static CLOSED: Cell<(u64, u64, u8)> = const { Cell::new((0, 0, 9)) };
}

/// An Io adapter's close thunk: handle 13 fails, recording rule 77 at the anchor; any other
/// handle closes. It notes the handle, the anchor and whether it ran shielded.
unsafe extern "C" fn close_probe(
    ctx: *mut TaskCtx,
    handle: u64,
    anchor: u64,
    _operation_site: u64,
) -> u8 {
    CLOSED.with(|seen| seen.set((handle, anchor, unsafe { hirsel_rt_v2_shielded(ctx) })));
    if handle == 13 {
        unsafe { hirsel_rt_v2_failure_begin(ctx, 1, 77, anchor, _operation_site) };
        return 1;
    }
    0
}

/// Compiled code closes an inline handle through close_handle: shielded, a failing close
/// returns 1 with its failure recorded, and a context without an Io adapter faults.
#[test]
fn compiled_handle_close_runs_shielded_and_reports_a_failing_close() {
    if in_child("compiled_handle_close_runs_shielded_and_reports_a_failing_close") {
        let ctx = context();
        unsafe { hirsel_rt_v2_close_handle(ctx.0, 12, 3, 3) };
        return;
    }
    assert_child_aborts(
        "compiled_handle_close_runs_shielded_and_reports_a_failing_close",
        "without an Io adapter's close_handle",
    );
    let ctx = Ctx::with(hirsel_rt::Config {
        close_handle: Some(close_probe),
        ..hirsel_rt::Config::new()
    });
    assert_eq!(unsafe { hirsel_rt_v2_close_handle(ctx.0, 13, 5, 5) }, 1);
    assert_eq!(CLOSED.with(Cell::get), (13, 5, 1));
    let failure = primary(&ctx).unwrap();
    assert_eq!((failure.rule, failure.site), (77, 5));
    assert_eq!(unsafe { hirsel_rt_v2_shielded(ctx.0) }, 0);
    // Accepted twin: a close that succeeds records nothing.
    unsafe { hirsel_rt_v2_failure_clear(ctx.0) };
    assert_eq!(unsafe { hirsel_rt_v2_close_handle(ctx.0, 12, 6, 6) }, 0);
    assert_eq!(CLOSED.with(Cell::get), (12, 6, 1));
    assert!(primary(&ctx).is_none());
}

thread_local! {
    static OWNED: Cell<*mut Header> = const { Cell::new(ptr::null_mut()) };
}

/// A finalizer whose body releases a block it owns, as user drop code may.
unsafe extern "C" fn releasing_drop(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    _operation_site: u64,
) -> u8 {
    unsafe { log_drop(ctx, value, anchor, _operation_site) };
    let owned = OWNED.with(|owned| owned.replace(ptr::null_mut()));
    unsafe {
        hirsel_rt_v2_release(
            ctx,
            owned,
            (*owned).index(),
            ptr::null(),
            anchor,
            _operation_site,
        )
    }
}

#[test]
fn a_release_inside_a_finalizer_drains_without_disturbing_pending_work() {
    let ctx = context();
    pair(releasing_drop);
    let owned = node(ctx.0, pair(log_drop), 50);
    write_ref(owned, 8, node(ctx.0, resource(log_drop), 51));
    write_ref(owned, 16, node(ctx.0, resource(failing_drop), 52));
    OWNED.with(|slot| slot.set(owned));
    let root = node(ctx.0, pair(releasing_drop), 1);
    write_ref(root, 8, node(ctx.0, resource(log_drop), 2));
    write_ref(root, 16, node(ctx.0, resource(log_drop), 3));
    take_events();
    assert_eq!(
        release(ctx.0, root),
        1,
        "the nested failure fails the finalizer that observed it"
    );
    assert_eq!(ids(take_events()), [1, 50, 52, 51, 3, 2]);
    assert_eq!(primary(&ctx).unwrap().rule, 52);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn an_inline_resource_finalizes_through_its_continuation_and_releases_its_fields() {
    let ctx = context();
    // A frame value `{ id, a: ref, b: ref }` whose finalizer fails.
    let frame = pair(failing_drop);
    let mut value = [0u64; 3];
    value[0] = 1;
    value[1] = node(ctx.0, resource(log_drop), 2) as u64;
    value[2] = node(ctx.0, resource(log_drop), 3) as u64;
    let mut continuation = MaybeUninit::<Cleanup>::uninit();
    take_events();
    let status = unsafe {
        hirsel_rt_v2_finalize(
            ctx.0,
            index(ctx.0, frame),
            value.as_mut_ptr().cast(),
            ptr::null(),
            continuation.as_mut_ptr(),
            4,
            4,
        )
    };
    assert_eq!(status, 1);
    assert_eq!(take_events(), [(1, 4), (3, 4), (2, 4)]);
    assert_eq!(unsafe { continuation.assume_init_ref() }.phase, 0);
    assert_eq!(ctx.stats().live_blocks, 0);
    assert_eq!(primary(&ctx).unwrap().rule, 1);
}

#[test]
fn a_partially_built_inline_value_releases_only_its_live_fields() {
    let ctx = context();
    let frame = pair(log_drop);
    let kept = node(ctx.0, resource(log_drop), 3);
    let mut value = [1u64, node(ctx.0, resource(log_drop), 2) as u64, 0x1];
    let mut continuation = MaybeUninit::<Cleanup>::uninit();
    take_events();
    let mask = [0b011u64];
    let status = unsafe {
        hirsel_rt_v2_finalize(
            ctx.0,
            index(ctx.0, frame),
            value.as_mut_ptr().cast(),
            mask.as_ptr(),
            continuation.as_mut_ptr(),
            0,
            0,
        )
    };
    assert_eq!(status, 0);
    assert_eq!(ids(take_events()), [2]);
    release(ctx.0, kept);
    assert_eq!(ctx.stats().live_blocks, 0);
}

#[test]
fn a_failing_unsafe_ffi_finalizer_ends_the_process() {
    if in_child("a_failing_unsafe_ffi_finalizer_ends_the_process") {
        let ctx = context();
        let ffi = with_flags(resource(failing_drop), LAYOUT_UNSAFE_FFI);
        release(ctx.0, node(ctx.0, ffi, 1));
        return;
    }
    assert_child_aborts("a_failing_unsafe_ffi_finalizer_ends_the_process", "FAIL-3");
    // Accepted twin: a succeeding unsafe-ffi finalizer is an ordinary drop.
    let ctx = context();
    let ffi = with_flags(resource(log_drop), LAYOUT_UNSAFE_FFI);
    take_events();
    assert_eq!(release(ctx.0, node(ctx.0, ffi, 1)), 0);
    assert_eq!(ids(take_events()), [1]);
}

#[test]
fn a_failing_unsafe_ffi_field_ends_the_process_even_inside_a_safe_value() {
    if in_child("a_failing_unsafe_ffi_field_ends_the_process_even_inside_a_safe_value") {
        let ctx = context();
        let ffi = with_flags(
            layout(8, 1, Some(failing_drop), Vec::new()),
            LAYOUT_UNSAFE_FFI,
        );
        let outer = layout(16, 2, Some(log_drop), vec![finalize_step(1, 8, ffi)]);
        release(ctx.0, node(ctx.0, outer, 1));
        return;
    }
    assert_child_aborts(
        "a_failing_unsafe_ffi_field_ends_the_process_even_inside_a_safe_value",
        "FAIL-3",
    );
}

unsafe extern "C" fn bad_status_drop(
    _ctx: *mut TaskCtx,
    _value: *mut u8,
    _anchor: u64,
    _operation_site: u64,
) -> u8 {
    7
}

#[test]
fn a_finalizer_status_other_than_zero_or_one_is_an_invariant_fault() {
    if in_child("a_finalizer_status_other_than_zero_or_one_is_an_invariant_fault") {
        let ctx = Ctx::production();
        release(ctx.0, node(ctx.0, resource(bad_status_drop), 1));
        return;
    }
    assert_child_aborts(
        "a_finalizer_status_other_than_zero_or_one_is_an_invariant_fault",
        "other than 0 or 1",
    );
}

unsafe extern "C" fn silent_failure_drop(
    _ctx: *mut TaskCtx,
    _value: *mut u8,
    _anchor: u64,
    _operation_site: u64,
) -> u8 {
    1
}

#[test]
fn a_finalizer_that_fails_without_recording_is_rejected_in_diagnostic_mode() {
    if in_child("a_finalizer_that_fails_without_recording_is_rejected_in_diagnostic_mode") {
        let ctx = context();
        release(ctx.0, node(ctx.0, resource(silent_failure_drop), 1));
        return;
    }
    assert_child_aborts(
        "a_finalizer_that_fails_without_recording_is_rejected_in_diagnostic_mode",
        "without recording a failure",
    );
}

// ----- the final schemas: layouts derived by hirsel-lir --------------------------------------

#[test]
fn a_failing_derived_unsafe_ffi_finalizer_ends_the_process() {
    use support::schema::*;
    if in_child("a_failing_derived_unsafe_ffi_finalizer_ends_the_process") {
        let Resources {
            derived: d, boxed, ..
        } = resources(true);
        assert_ne!(d.descriptor(boxed).flags & LAYOUT_UNSAFE_FFI, 0);
        let ctx = context();
        let block = d.build(&ctx, boxed, 0, &mut 5);
        fail_at(vec![support::payload(block) as u64]);
        release(ctx.0, block);
        return;
    }
    assert_child_aborts(
        "a_failing_derived_unsafe_ffi_finalizer_ends_the_process",
        "FAIL-3",
    );
}

#[test]
fn library_failure_sites_rise_once_and_source_finalizer_sites_stay_put() {
    use hirsel_rt::failure::hirsel_rt_v2_failure_reanchor;
    use support::schema::context;
    let ctx = context();
    unsafe {
        hirsel_rt_v2_failure_begin(ctx.0, 1, 3, 0, 0);
        hirsel_rt_v2_failure_begin(ctx.0, 2, 7, 77, 77);
        hirsel_rt_v2_failure_begin(ctx.0, 3, 9, 0, 0);
        hirsel_rt_v2_failure_reanchor(ctx.0, 0, 0);
    }
    assert_eq!(primary(&ctx).unwrap().site, 0);
    unsafe { hirsel_rt_v2_failure_reanchor(ctx.0, 41, 41) };
    assert_eq!(primary(&ctx).unwrap().site, 41);
    let mut first = MaybeUninit::uninit();
    let mut second = MaybeUninit::uninit();
    unsafe {
        assert_eq!(hirsel_rt_v2_failure_cause(ctx.0, 0, first.as_mut_ptr()), 1);
        assert_eq!(hirsel_rt_v2_failure_cause(ctx.0, 1, second.as_mut_ptr()), 1);
        assert_eq!(first.assume_init().site, 77);
        assert_eq!(second.assume_init().site, 41);
        hirsel_rt_v2_failure_reanchor(ctx.0, 99, 99);
    }
    assert_eq!(primary(&ctx).unwrap().site, 41);
}

#[test]
fn caller_attribution_keeps_operation_sites_and_ordered_causes() {
    use hirsel_rt::failure::hirsel_rt_v2_failure_reanchor;
    let ctx = support::schema::context();
    unsafe {
        hirsel_rt_v2_failure_begin(ctx.0, 1, 3, 0, 0);
        hirsel_rt_v2_failure_begin(ctx.0, 2, 7, 77, 88);
        hirsel_rt_v2_failure_begin(ctx.0, 3, 9, 31, 41);
        hirsel_rt_v2_failure_reanchor(ctx.0, 31, 41);
    }
    let primary_record = primary(&ctx).unwrap();
    assert_eq!(
        (primary_record.site, primary_record.operation_site),
        (31, 41)
    );
    let cause = |index| unsafe {
        let mut record = MaybeUninit::uninit();
        assert_eq!(
            hirsel_rt_v2_failure_cause(ctx.0, index, record.as_mut_ptr()),
            1
        );
        record.assume_init()
    };
    let first = cause(0);
    let second = cause(1);
    assert_eq!((first.rule, first.site, first.operation_site), (7, 77, 88));
    assert_eq!(
        (second.rule, second.site, second.operation_site),
        (9, 31, 41)
    );
    // An enclosing call cannot overwrite an already attributed child failure.
    unsafe { hirsel_rt_v2_failure_reanchor(ctx.0, 99, 99) };
    assert_eq!(primary(&ctx), Some(primary_record));
    assert_eq!(cause(0), first);
    assert_eq!(cause(1), second);
}

/// The public cleanup ABI carries both codes through an independent host descriptor,
/// including every later field failure after an already anchored primary failure.
#[test]
fn cleanup_abi_preserves_distinct_sites_and_anchored_causes() {
    for inline in [false, true] {
        let ctx = context();
        // Bind the fixture descriptors before recording state in the context.
        index(ctx.0, pair(failing_drop));
        unsafe { hirsel_rt_v2_failure_begin(ctx.0, 1, 99, 11, 12) };
        let first = node(ctx.0, resource(failing_drop), 2);
        let second = node(ctx.0, resource(failing_drop), 3);
        let status = if inline {
            let mut value = [1u64, first as u64, second as u64];
            let mut continuation = MaybeUninit::<Cleanup>::uninit();
            unsafe {
                hirsel_rt_v2_finalize(
                    ctx.0,
                    index(ctx.0, pair(failing_drop)),
                    value.as_mut_ptr().cast(),
                    ptr::null(),
                    continuation.as_mut_ptr(),
                    101,
                    202,
                )
            }
        } else {
            let root = node(ctx.0, pair(failing_drop), 1);
            write_ref(root, 8, first);
            write_ref(root, 16, second);
            unsafe { hirsel_rt_v2_release(ctx.0, root, (*root).index(), ptr::null(), 101, 202) }
        };
        assert_eq!(status, 1);
        let first = primary(&ctx).unwrap();
        assert_eq!((first.rule, first.site, first.operation_site), (99, 11, 12));
        assert_eq!(cause_rules(&ctx), [1, 3, 2]);
        for ordinal in 0..3 {
            let mut record = MaybeUninit::uninit();
            assert_eq!(
                unsafe { hirsel_rt_v2_failure_cause(ctx.0, ordinal, record.as_mut_ptr()) },
                1
            );
            let record = unsafe { record.assume_init() };
            assert_eq!((record.site, record.operation_site), (101, 202));
        }
        assert_eq!(ctx.stats().live_blocks, 0);
    }
    let ctx = Ctx::with(hirsel_rt::Config {
        close_handle: Some(close_probe),
        ..hirsel_rt::Config::new()
    });
    assert_eq!(unsafe { hirsel_rt_v2_close_handle(ctx.0, 13, 303, 404) }, 1);
    let failure = primary(&ctx).unwrap();
    assert_eq!((failure.site, failure.operation_site), (303, 404));
    unsafe { hirsel_rt_v2_failure_clear(ctx.0) };
    assert_eq!(unsafe { hirsel_rt_v2_close_handle(ctx.0, 12, 303, 404) }, 0);
    assert!(primary(&ctx).is_none());
}
