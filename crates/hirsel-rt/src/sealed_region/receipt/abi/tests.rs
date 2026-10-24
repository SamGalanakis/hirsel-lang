use super::*;
use crate::sealed_region::{
    Charge,
    receipt::{
        MetadataRun,
        semantic::{Origin, Record, snapshot},
    },
};
use std::sync::atomic::Ordering;

fn node(run: MetadataRun, children: &[Receipt<Record>]) -> Receipt<Record> {
    snapshot(
        run,
        Origin::Inline,
        Charge::Bytes(0),
        b"image",
        16,
        children,
    )
    .unwrap()
}
fn holders(receipt: &Receipt<Record>) -> usize {
    // SAFETY: this holder keeps the immutable metadata header live.
    unsafe { receipt.pointer.as_ref() }
        .holders
        .load(Ordering::Relaxed)
}
#[test]
fn opaque_abi_holder_exchange_preserves_identity_and_child_multiplicity_without_allocation() {
    let run = MetadataRun::new();
    let leaf = node(run, &[]);
    let parent = node(run, &[leaf.clone(), leaf.clone()]);
    let generation = parent.generation();
    assert_eq!(holders(&leaf), 3);
    let raw = parent.as_raw();
    let before = crate::io_real::borrowed_net_tests::allocation_calls();
    // SAFETY: parent keeps this semantic Record pointer live; the new holder
    // is consumed by the matching release below.
    unsafe {
        hirsel_rt_v2_receipt_retain(raw);
    }
    assert_eq!(holders(&parent), 2);
    assert_eq!(holders(&leaf), 3);
    // SAFETY: consumes the newly acquired native holder, not parent's holder.
    unsafe {
        hirsel_rt_v2_receipt_release(raw);
    }
    assert_eq!(holders(&parent), 1);
    let raw = parent.into_raw();
    // SAFETY: into_raw transferred exactly this one live semantic holder.
    let adopted = unsafe { Receipt::<Record>::from_raw(raw) };
    assert_eq!(adopted.generation(), generation);
    assert_eq!(adopted.run(), run);
    assert_eq!(adopted.bytes(), Some(b"image".as_slice()));
    drop(adopted);
    assert_eq!(holders(&leaf), 1);
    assert_eq!(
        before,
        crate::io_real::borrowed_net_tests::allocation_calls()
    );
}
#[test]
fn opaque_abi_metadata_survives_its_source_thread_and_releases_on_another_thread() {
    let (raw, run, generation) = std::thread::spawn(|| {
        let run = MetadataRun::new();
        let receipt = node(run, &[]);
        let generation = receipt.generation();
        (receipt.into_raw() as usize, run, generation)
    })
    .join()
    .unwrap();
    std::thread::spawn(move || {
        let raw = raw as *mut c_void;
        // SAFETY: the source thread transferred one live Record holder.
        unsafe {
            hirsel_rt_v2_receipt_retain(raw);
        }
        // SAFETY: consumes exactly one of the two native holders.
        unsafe {
            hirsel_rt_v2_receipt_release(raw);
        }
        // SAFETY: the other native holder remains live and is adopted once.
        let receipt = unsafe { Receipt::<Record>::from_raw(raw) };
        assert_eq!(receipt.run(), run);
        assert_eq!(receipt.generation(), generation);
        assert_eq!(receipt.bytes(), Some(b"image".as_slice()));
        // SAFETY: transfers the adopted holder back and releases it once.
        unsafe {
            hirsel_rt_v2_receipt_release(receipt.into_raw());
        }
    })
    .join()
    .unwrap();
}
#[test]
fn opaque_abi_last_release_drains_a_chain_on_a_small_stack_without_scratch_allocation() {
    let run = MetadataRun::new();
    let mut receipt = node(run, &[]);
    for _ in 0..512 {
        receipt = node(run, &[receipt]);
    }
    let raw = receipt.into_raw() as usize;
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            let before = crate::io_real::borrowed_net_tests::allocation_calls();
            // SAFETY: the chain's single live root holder was transferred above;
            // its immutable child edges own every remaining metadata holder.
            unsafe {
                hirsel_rt_v2_receipt_release(raw as *mut c_void);
            }
            assert_eq!(
                before,
                crate::io_real::borrowed_net_tests::allocation_calls()
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

fn metadata_step(offset: u64) -> crate::release::Step {
    crate::release::Step {
        kind: crate::release::STEP_RELEASE_RECEIPT,
        field: crate::release::WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset,
        stride: 0,
        count: 0,
        count_offset: crate::release::NO_OFFSET,
        layout: std::ptr::null(),
    }
}

fn with_metadata_program(
    steps: &[crate::release::Step],
    size: u64,
    fields: u32,
    body: impl FnOnce(*mut crate::TaskCtx),
) {
    use crate::context::{CONFIG_DIAGNOSTIC, hirsel_rt_v2_ctx_create, hirsel_rt_v2_ctx_destroy};
    use crate::release::{LAYOUT_MAGIC, LAYOUT_VERSION, LOGICAL_UNSUPPORTED, Layout};
    let layout = Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size,
        align: 8,
        finalizer: None,
        steps: steps.as_ptr(),
        step_count: steps.len() as u64,
        field_count: fields,
        flags: if steps
            .iter()
            .any(|step| step.kind == crate::release::STEP_LOOP)
        {
            crate::release::LAYOUT_WIDE_CURSOR
        } else {
            0
        },
        logical: LOGICAL_UNSUPPORTED,
        logical_count: std::ptr::null(),
        scan: std::ptr::null(),
        scan_words: u64::MAX,
    };
    let descriptors = [std::ptr::from_ref(&layout)];
    let config = crate::Config {
        flags: CONFIG_DIAGNOSTIC,
        layouts: descriptors.as_ptr(),
        layout_count: 1,
        ..crate::Config::new()
    };
    let mut ctx = std::ptr::null_mut();
    // SAFETY: config and its descriptors remain live until context destruction.
    assert_eq!(unsafe { hirsel_rt_v2_ctx_create(&config, &mut ctx) }, 0);
    body(ctx);
    // SAFETY: body has retired every context-owned block and invocation.
    unsafe { hirsel_rt_v2_ctx_destroy(ctx) };
}

unsafe fn finalize_metadata(ctx: *mut crate::TaskCtx, value: *mut u8, mask: *const u64) {
    let mut continuation = std::mem::MaybeUninit::<crate::Cleanup>::uninit();
    // SAFETY: caller owns the program's live inline fields. The continuation is
    // initialized and completely drained by finalize before this frame returns.
    assert_eq!(
        unsafe {
            crate::finalize::hirsel_rt_v2_finalize(
                ctx,
                2,
                value,
                mask,
                continuation.as_mut_ptr(),
                0,
                0,
            )
        },
        crate::STATUS_OK
    );
}

#[test]
fn receipt_drop_program_consumes_only_the_live_field_metadata_holder() {
    let run = MetadataRun::new();
    let receipt = node(run, &[]);
    let mut fields = [receipt.clone().into_raw(), receipt.clone().into_raw()];
    assert_eq!(holders(&receipt), 3);
    let steps = [
        crate::release::Step {
            field: 0,
            ..metadata_step(0)
        },
        crate::release::Step {
            field: 1,
            ..metadata_step(8)
        },
    ];
    with_metadata_program(&steps, 16, 2, |ctx| {
        let mask = 1u64;
        let before = crate::io_real::borrowed_net_tests::allocation_calls();
        // SAFETY: the mask publishes custody only of field zero to this cleanup.
        unsafe { finalize_metadata(ctx, fields.as_mut_ptr().cast(), &mask) };
        assert_eq!(holders(&receipt), 2);
        assert_eq!(
            before,
            crate::io_real::borrowed_net_tests::allocation_calls()
        );
    });
    // SAFETY: the skipped field still owns exactly one native metadata holder.
    unsafe { hirsel_rt_v2_receipt_release(fields[1]) };
    assert_eq!(holders(&receipt), 1);
}

#[test]
fn receipt_drop_program_loops_release_each_metadata_occurrence_without_deduplication() {
    let run = MetadataRun::new();
    let receipt = node(run, &[]);
    let mut fields = [receipt.clone().into_raw(), receipt.clone().into_raw()];
    let steps = [
        crate::release::Step {
            kind: crate::release::STEP_LOOP,
            span: 1,
            stride: 8,
            count: 2,
            ..metadata_step(0)
        },
        metadata_step(0),
    ];
    with_metadata_program(&steps, 16, 0, |ctx| {
        // SAFETY: both loop elements own one native holder, even at equal pointers.
        unsafe { finalize_metadata(ctx, fields.as_mut_ptr().cast(), std::ptr::null()) };
    });
    assert_eq!(holders(&receipt), 1);
}

#[test]
fn receipt_drop_program_skips_inactive_variants_and_releases_active_metadata() {
    #[repr(C)]
    struct Tagged {
        tag: u64,
        receipt: *mut c_void,
    }
    let run = MetadataRun::new();
    let receipt = node(run, &[]);
    let mut value = Tagged {
        tag: 0,
        receipt: receipt.clone().into_raw(),
    };
    let steps = [
        crate::release::Step {
            kind: crate::release::STEP_TAG,
            span: 1,
            variant: 1,
            ..metadata_step(0)
        },
        metadata_step(8),
    ];
    with_metadata_program(&steps, 16, 0, |ctx| {
        // SAFETY: variant zero has no live receipt contribution.
        unsafe { finalize_metadata(ctx, std::ptr::from_mut(&mut value).cast(), std::ptr::null()) };
        assert_eq!(holders(&receipt), 2);
        value.tag = 1;
        // SAFETY: variant one now publishes the previously retained field holder.
        unsafe { finalize_metadata(ctx, std::ptr::from_mut(&mut value).cast(), std::ptr::null()) };
        assert_eq!(holders(&receipt), 1);
    });
}
