//! Opaque semantic-receipt custody across the native boundary. No task/frame,
//! source allocation or language Shared root participates in these helpers.
use super::{Node, Receipt, retain, semantic};
use crate::fault;
use std::ffi::c_void;
use std::ptr::NonNull;

fn pointer(raw: *mut c_void) -> NonNull<Node<semantic::Record>> {
    NonNull::new(raw.cast()).unwrap_or_else(|| fault("null semantic receipt handle"))
}

impl Receipt<semantic::Record> {
    /// Borrow this opaque handle. This does not transfer or acquire a holder;
    /// the returned pointer may only be used while an owning holder stays live.
    pub fn as_raw(&self) -> *mut c_void {
        self.pointer.as_ptr().cast()
    }

    /// Transfer exactly one holder to the native boundary, like Rust Arc's
    /// into_raw. The caller must adopt it or release it exactly once.
    pub fn into_raw(self) -> *mut c_void {
        let raw = self.as_raw();
        std::mem::forget(self);
        raw
    }

    /// Adopt exactly one native holder without changing its count.
    ///
    /// # Safety
    /// `raw` names a live semantic Record receipt from into_raw, or an owning
    /// holder acquired by receipt_retain. It must not merely be a borrowed
    /// pointer or have already been adopted/released. No generic Receipt<T>
    /// with a different T may cross this semantic-receipt ABI.
    pub unsafe fn from_raw(raw: *mut c_void) -> Self {
        Self {
            pointer: pointer(raw),
        }
    }
}

/// Acquire one metadata holder. No source storage/Shared credit is acquired.
///
/// # Safety
/// `receipt` is a non-null live semantic Record handle, kept live across this
/// call by an owning holder or a bounded loan from one. The caller owns the
/// newly acquired holder and must adopt or release it exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_retain(receipt: *mut c_void) {
    retain(pointer(receipt));
}

/// Consume one metadata holder with iterative, nonallocating cleanup. This
/// cannot invoke user code or fail in the language and needs no live TaskCtx.
///
/// # Safety
/// `receipt` is a non-null live semantic Record handle and this call consumes
/// exactly one owning native holder which has not been adopted or released.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_receipt_release(receipt: *mut c_void) {
    // SAFETY: the ABI caller transfers precisely the holder this wrapper drops.
    drop(unsafe { Receipt::<semantic::Record>::from_raw(receipt) });
}

#[cfg(test)]
mod tests;
