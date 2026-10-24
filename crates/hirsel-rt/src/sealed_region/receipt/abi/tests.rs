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
