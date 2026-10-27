//! ABI records borrowed from a checked CPU mapper's saved frame.
use super::native::Physical;
use crate::{Header, region::Region};

pub const VERSION: u32 = 1;
pub const ACCEPTED: u8 = 0;
pub const NOT_CERTIFIED: u8 = 1;
pub const SUMMARY_PENDING: u32 = 0;
pub const SUMMARY_PUBLISHED: u32 = 1;
pub const SUMMARY_RETIRED: u32 = 2;
pub const SUMMARY_WITHDRAWN: u32 = 3;
pub const GROW_BLOCK: u32 = 0;
pub const GROW_SEQUENCE: u32 = 1;
pub const ALLOC_REGION: u32 = 2;

/// The value and retained-block table remain owned by the saved mapper until
/// retirement. Publication borrows them; it never adds counted holders.
#[repr(C)]
pub struct Summary {
    pub version: u32,
    pub phase: u32,
    pub ordinal: u64,
    pub value: *const u8,
    pub value_bytes: u64,
    pub descriptor: u32,
    pub retained_count: u32,
    pub retained: *const *mut Header,
}

/// A saved storage call. `block` is both the old block and the successful
/// result. All input fields survive refusal unchanged. No Rust frame is saved.
#[repr(C)]
pub struct Growth {
    pub version: u32,
    pub kind: u32,
    pub ordinal: u64,
    pub block: *mut Header,
    pub region: *mut Region,
    pub index: u32,
    pub flags: u32,
    pub extent: u64,
    pub align: u64,
    pub keep: u64,
    pub element_size: u64,
    pub element_align: u64,
    pub capacity: u64,
}

const _: () = {
    assert!(size_of::<Summary>() == 48);
    assert!(std::mem::offset_of!(Summary, ordinal) == 8);
    assert!(std::mem::offset_of!(Summary, retained) == 40);
    assert!(size_of::<Growth>() == 88);
    assert!(std::mem::offset_of!(Growth, block) == 16);
    assert!(std::mem::offset_of!(Growth, capacity) == 80);
};

/// # Safety
/// Called only by a checked CPU alternate. Once supported, the summary and
/// its pointed-to value/table must remain in that mapper's owned saved frame
/// until this call returns. This declaration slice refuses without reading it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_publish(
    _state: *mut Physical,
    _summary: *mut Summary,
) -> u8 {
    NOT_CERTIFIED
}

/// # Safety
/// Called only by a checked CPU alternate. The request and any input block or
/// region belong exclusively to its ordinal and remain live across save.
/// This declaration slice refuses without reading or changing the request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_cpu_ahead_grow(
    _state: *mut Physical,
    _request: *mut Growth,
) -> u8 {
    NOT_CERTIFIED
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertified_handoffs_refuse_before_reading_the_saved_frame() {
        // SAFETY: these declaration-only entries never dereference arguments.
        unsafe {
            assert_eq!(
                hirsel_rt_v2_cpu_ahead_publish(std::ptr::null_mut(), std::ptr::null_mut()),
                NOT_CERTIFIED
            );
            assert_eq!(
                hirsel_rt_v2_cpu_ahead_grow(std::ptr::null_mut(), std::ptr::null_mut()),
                NOT_CERTIFIED
            );
        }
    }
}
