//! SEM-3 text leaves at the declaration's Hirsel ABI v1.
use crate::views::Slice;
use crate::{
    DOMAIN_HEAP, Header, LAYOUT_DYNAMIC, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, RunCtx,
    STATUS_FAILURE, STATUS_OK,
    alloc::{hirsel_rt_v2_alloc, payload},
    count::hirsel_rt_v2_dup,
    fault, kernels,
};
use std::ffi::c_void;

/// A static, pointer-free Sequence[u8] descriptor. Its representation is a C Layout.
#[repr(transparent)]
pub struct BytesDescriptor(pub Layout);
// SAFETY: this immutable descriptor contains no pointers to steps or mutable state.
unsafe impl Sync for BytesDescriptor {}

#[allow(non_upper_case_globals)]
#[unsafe(no_mangle)]
pub static hirsel_rt_v2_bytes_descriptor: BytesDescriptor = BytesDescriptor(Layout {
    magic: LAYOUT_MAGIC,
    version: LAYOUT_VERSION,
    size: 8,
    align: 8,
    finalizer: None,
    steps: std::ptr::null(),
    step_count: 0,
    field_count: 0,
    flags: LAYOUT_DYNAMIC,
});

/// Allocate the shared runtime byte storage layout, including its u64 count.
/// # Safety
/// `ctx` is a live run context.
pub unsafe fn allocate_bytes(ctx: *mut RunCtx, bytes: &[u8]) -> Slice {
    let length = u64::try_from(bytes.len()).unwrap_or_else(|_| fault("byte length overflow"));
    let extent = length
        .checked_add(8)
        .unwrap_or_else(|| fault("byte extent overflow"));
    let owner = unsafe { hirsel_rt_v2_alloc(ctx, 0, extent, 8, DOMAIN_HEAP, 0) };
    unsafe {
        payload(owner).cast::<u64>().write(length);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), payload(owner).add(8), bytes.len());
    }
    Slice {
        owner,
        start: 0,
        length,
    }
}

/// Read a slice without assuming UTF-8 or a Sequence owner (flat views are valid).
/// # Safety
/// The slice owner is live and its range contains initialized bytes.
unsafe fn bytes<'a>(slice: Slice) -> &'a [u8] {
    let start = usize::try_from(slice.start).unwrap_or_else(|_| fault("slice start overflow"));
    let length = usize::try_from(slice.length).unwrap_or_else(|_| fault("slice length overflow"));
    slice
        .start
        .checked_add(slice.length)
        .and_then(|end| end.checked_add(8))
        .filter(|&end| end <= isize::MAX as u64)
        .unwrap_or_else(|| fault("slice end overflow"));
    unsafe { std::slice::from_raw_parts(payload(slice.owner).add(8).add(start), length) }
}

/// `Result[String, Bytes]`: both arms hold the same Slice layout.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StringResult {
    pub tag: u32,
    pub padding: u32,
    pub value: Slice,
}

/// Consume one owner credit into either result arm, with no copy on either outcome.
/// # Safety
/// The context and output are live; the input transfers one Slice owner credit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_from_bytes(
    _ctx: *mut RunCtx,
    _destination: *mut c_void,
    out: *mut StringResult,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    let value = Slice {
        owner,
        start,
        length,
    };
    let tag = u32::from(std::str::from_utf8(unsafe { bytes(value) }).is_err());
    unsafe {
        out.write(StringResult {
            tag,
            padding: 0,
            value,
        })
    };
    STATUS_OK
}

/// StrPos has one u64 of storage but, as a nominal, travels by read-only address.
/// A failing export leaves output untouched; the caller records SEM-3 at its call site.
/// # Safety
/// Context/output and borrowed slice and position addresses are live and correctly typed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_slice(
    ctx: *mut RunCtx,
    _destination: *mut c_void,
    out: *mut Slice,
    owner: *mut Header,
    start: u64,
    length: u64,
    from: *const u64,
    to: *const u64,
) -> u8 {
    let source = Slice {
        owner,
        start,
        length,
    };
    let (from, to) = unsafe { (from.read(), to.read()) };
    let text = unsafe { std::str::from_utf8_unchecked(bytes(source)) };
    let Some(part) = kernels::string_slice(text, from, to) else {
        return STATUS_FAILURE;
    };
    let value = Slice {
        owner,
        start: start
            .checked_add(from)
            .unwrap_or_else(|| fault("slice origin overflow")),
        length: part.len() as u64,
    };
    unsafe {
        hirsel_rt_v2_dup(ctx, owner);
        out.write(value);
    }
    STATUS_OK
}

/// Share the incoming String interval as immutable Bytes.
/// # Safety
/// The borrowed source is live and out names writable Slice storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_as_bytes(
    ctx: *mut RunCtx,
    _destination: *mut c_void,
    out: *mut Slice,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    unsafe {
        hirsel_rt_v2_dup(ctx, owner);
        out.write(Slice {
            owner,
            start,
            length,
        });
    }
    STATUS_OK
}

/// A byte subinterval is checked completely before publication or sharing.
/// # Safety
/// The borrowed source is live and out names writable Slice storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_slice(
    ctx: *mut RunCtx,
    _destination: *mut c_void,
    out: *mut Slice,
    owner: *mut Header,
    start: u64,
    length: u64,
    from: u64,
    to: u64,
) -> u8 {
    if from > to || to > length {
        return STATUS_FAILURE;
    }
    let start = start
        .checked_add(from)
        .unwrap_or_else(|| fault("slice origin overflow"));
    unsafe {
        hirsel_rt_v2_dup(ctx, owner);
        out.write(Slice {
            owner,
            start,
            length: to - from,
        });
    }
    STATUS_OK
}

/// The concrete Option[StrPos] ABI. None initializes its padding/payload too.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PositionOption {
    pub tag: u32,
    pub padding: u32,
    pub offset: u64,
}

/// Mint a position only at a boundary of the selected source interval.
/// # Safety
/// The borrowed source is valid UTF-8; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_position(
    _ctx: *mut RunCtx,
    _destination: *mut c_void,
    out: *mut PositionOption,
    owner: *mut Header,
    start: u64,
    length: u64,
    offset: u64,
) -> u8 {
    let text = unsafe {
        std::str::from_utf8_unchecked(bytes(Slice {
            owner,
            start,
            length,
        }))
    };
    let valid = usize::try_from(offset)
        .ok()
        .is_some_and(|i| text.is_char_boundary(i));
    unsafe {
        out.write(PositionOption {
            tag: u32::from(valid),
            padding: 0,
            offset: if valid { offset } else { 0 },
        });
    }
    STATUS_OK
}

/// Read the relative position without creating a new branded value.
/// # Safety
/// The declared matching StrPos is live and out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_byte_offset(
    _ctx: *mut RunCtx,
    out: *mut u64,
    _owner: *mut Header,
    _start: u64,
    _length: u64,
    position: *const u64,
) -> u8 {
    unsafe {
        out.write(position.read());
    }
    STATUS_OK
}
