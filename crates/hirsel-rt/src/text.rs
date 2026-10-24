//! SEM-3 text leaves at the declaration's Hirsel ABI v2.
use crate::views::Slice;
use crate::{
    DOMAIN_HEAP, Header, LAYOUT_DYNAMIC, LAYOUT_MAGIC, LAYOUT_VERSION, Layout, STATUS_FAILURE,
    STATUS_OK, TaskCtx,
    alloc::{
        allocate_sequence, hirsel_rt_v2_alloc, payload, sequence_elements_offset,
        sequence_slice_start,
    },
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
    size: 16,
    align: 8,
    finalizer: None,
    steps: std::ptr::null(),
    step_count: 0,
    field_count: 0,
    flags: LAYOUT_DYNAMIC | crate::LAYOUT_LOGICAL_PER_ELEMENT | crate::LAYOUT_SEQUENCE,
    logical: 1,
    logical_count: std::ptr::null(),
    scan: std::ptr::null(),
    scan_words: 0,
});

/// The `List[Bytes]` payload descriptor the runtime allocates its byte-string lists under:
/// a sequence of 24-byte slices whose drop program releases each element's owner, highest
/// index first. Its steps are the derivation's (`hirsel_lir::rt_program`).
#[repr(transparent)]
pub struct BytesListDescriptor(pub Layout);
// SAFETY: this immutable descriptor points only at the immutable steps below.
unsafe impl Sync for BytesListDescriptor {}
#[repr(transparent)]
struct BytesListSteps([crate::Step; 2]);
// SAFETY: immutable steps with no layout pointers.
unsafe impl Sync for BytesListSteps {}
static BYTES_LIST_STEPS: BytesListSteps = BytesListSteps([
    crate::Step {
        kind: crate::STEP_LOOP,
        field: crate::WHOLE_VALUE,
        variant: 0,
        span: 1,
        offset: 16,
        stride: 24,
        count: 0,
        count_offset: 0,
        layout: std::ptr::null(),
    },
    crate::Step {
        kind: crate::STEP_RELEASE,
        field: crate::WHOLE_VALUE,
        variant: 0,
        span: 0,
        offset: 0,
        stride: 24,
        count: 0,
        count_offset: crate::NO_OFFSET,
        layout: std::ptr::null(),
    },
]);
pub static BYTES_LIST_DESCRIPTOR: BytesListDescriptor = BytesListDescriptor(Layout {
    magic: LAYOUT_MAGIC,
    version: LAYOUT_VERSION,
    size: 16,
    align: 8,
    finalizer: None,
    steps: BYTES_LIST_STEPS.0.as_ptr(),
    step_count: 2,
    field_count: 0,
    flags: LAYOUT_DYNAMIC
        | crate::LAYOUT_LOGICAL_PER_ELEMENT
        | crate::LAYOUT_SEQUENCE
        | crate::release::LAYOUT_WIDE_CURSOR,
    logical: 8,
    logical_count: std::ptr::null(),
    scan: std::ptr::null(),
    scan_words: 0,
});

/// The registry index of [`BYTES_LIST_DESCRIPTOR`]: the run context places it after the
/// linked program's descriptors.
/// # Safety
/// `ctx` is a live run context.
pub unsafe fn bytes_list_index(ctx: *mut TaskCtx) -> u32 {
    unsafe { (*ctx).heap().layout_count() - 1 }
}

/// A fresh `List[Bytes]` holding a fresh Bytes of each of `items`, in order.
/// # Safety
/// `ctx` is a live run context.
pub unsafe fn allocate_bytes_list(ctx: *mut TaskCtx, items: &[Vec<u8>]) -> *mut Header {
    let count = u64::try_from(items.len()).unwrap_or_else(|_| fault("list length overflow"));
    unsafe {
        let block = allocate_sequence(ctx, bytes_list_index(ctx), count, 24, 8);
        let first = payload(block).add(sequence_elements_offset(8) as usize);
        for (index, item) in items.iter().enumerate() {
            first
                .add(index * 24)
                .cast::<Slice>()
                .write(allocate_bytes(ctx, item));
        }
        block
    }
}

/// Allocate the shared runtime byte storage layout, including its u64 count and capacity,
/// as a whole slice of it.
/// # Safety
/// `ctx` is a live run context.
pub unsafe fn allocate_bytes(ctx: *mut TaskCtx, bytes: &[u8]) -> Slice {
    let length = u64::try_from(bytes.len()).unwrap_or_else(|_| fault("byte length overflow"));
    let owner = unsafe { allocate_sequence(ctx, 0, length, 1, 1) };
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            payload(owner).add(sequence_elements_offset(1) as usize),
            bytes.len(),
        );
    }
    Slice {
        owner,
        start: sequence_slice_start(1),
        length,
    }
}

/// Read a slice without assuming UTF-8 or a Sequence owner (flat views are valid).
/// # Safety
/// The slice owner is live and its range contains initialized bytes.
pub(crate) unsafe fn bytes<'a>(slice: Slice) -> &'a [u8] {
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
    _ctx: *mut TaskCtx,
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
    let tag = u32::from(simdutf8::basic::from_utf8(unsafe { bytes(value) }).is_err());
    unsafe {
        out.write(StringResult {
            tag,
            padding: 0,
            value,
        })
    };
    STATUS_OK
}

/// Fresh initialized byte storage, owned solely by the adapter until returned.
/// # Safety
/// `ctx` is a live context; allocation checks the complete sequence extent.
unsafe fn zeroed_bytes(ctx: *mut TaskCtx, length: usize) -> Slice {
    let length = u64::try_from(length).unwrap_or_else(|_| fault("byte length overflow"));
    let owner = unsafe { allocate_sequence(ctx, 0, length, 1, 1) };
    // SAFETY: the fresh byte sequence has exactly this writable extent and no
    // aliases. Initializing it permits a safe &mut [u8] for the safe SIMD API.
    unsafe {
        payload(owner)
            .add(sequence_elements_offset(1) as usize)
            .write_bytes(0, length as usize)
    };
    Slice {
        owner,
        start: sequence_slice_start(1),
        length,
    }
}

/// # Safety
/// `value` is fresh byte storage exclusively owned by this adapter.
unsafe fn fresh_bytes_mut<'a>(value: Slice) -> &'a mut [u8] {
    unsafe {
        std::slice::from_raw_parts_mut(
            payload(value.owner).add(8 + value.start as usize),
            value.length as usize,
        )
    }
}

/// Read well-typed named configuration values. Invalid enum bits are an ABI fault.
/// # Safety
/// Both pointers address initialized u32 enum tags.
unsafe fn base64_engine(alphabet: *const u32, padding: *const u32) -> base64_simd::Base64 {
    let url_safe = match unsafe { alphabet.read() } {
        0 => false,
        1 => true,
        _ => fault("Base64Alphabet tag"),
    };
    let padded = match unsafe { padding.read() } {
        0 => true,
        1 => false,
        _ => fault("Base64Padding tag"),
    };
    crate::base64::engine(url_safe, padded)
}

/// Encode a borrowed extent directly into fresh byte storage. Configuration
/// values are std's existing Standard/UrlSafe and Required/Omitted variants.
/// # Safety
/// Context/output and enum addresses are live; the borrowed extent is initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_base64_encode(
    ctx: *mut TaskCtx,
    _destination: *mut c_void,
    out: *mut Slice,
    owner: *mut Header,
    start: u64,
    length: u64,
    alphabet: *const u32,
    padding: *const u32,
) -> u8 {
    use base64_simd::AsOut;
    let engine = unsafe { base64_engine(alphabet, padding) };
    let source = unsafe {
        bytes(Slice {
            owner,
            start,
            length,
        })
    };
    let result = unsafe { zeroed_bytes(ctx, engine.encoded_length(source.len())) };
    let encoded = engine.encode(source, unsafe { fresh_bytes_mut(result) }.as_out());
    debug_assert_eq!(encoded.len(), result.length as usize);
    unsafe { out.write(result) };
    STATUS_OK
}

/// Try a strict decode. Option[Bytes] uses the Slice owner niche: null is None.
/// Rejection exposes no partial output; std retries its unchanged parser from
/// input zero to recover its exact DecodeError and first-error precedence.
/// # Safety
/// Context/output and enum addresses are live; the borrowed extent is initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_base64_decode(
    ctx: *mut TaskCtx,
    _destination: *mut c_void,
    out: *mut Slice,
    owner: *mut Header,
    start: u64,
    length: u64,
    alphabet: *const u32,
    padding: *const u32,
) -> u8 {
    use base64_simd::AsOut;
    let engine = unsafe { base64_engine(alphabet, padding) };
    let source = unsafe {
        bytes(Slice {
            owner,
            start,
            length,
        })
    };
    let decoded = engine.decoded_length(source).ok().and_then(|length| {
        let result = unsafe { zeroed_bytes(ctx, length) };
        match engine.decode(source, unsafe { fresh_bytes_mut(result) }.as_out()) {
            Ok(decoded) => {
                debug_assert_eq!(decoded.len(), result.length as usize);
                Some(result)
            }
            Err(_) => {
                // SAFETY: the fresh byte storage is task-local, uniquely owned,
                // unpublished and has no drop work. Free it locally rather than
                // entering the scheduler through semantic release.
                unsafe { (*ctx).heap().free(result.owner) };
                None
            }
        }
    });
    unsafe {
        out.write(decoded.unwrap_or(Slice {
            owner: std::ptr::null_mut(),
            start: 0,
            length: 0,
        }))
    };
    STATUS_OK
}

/// StrPos has one u64 of storage but, as a nominal, travels by read-only address.
/// A failing export leaves output untouched; the caller records SEM-3 at its call site.
/// # Safety
/// Context/output and borrowed slice and position addresses are live and correctly typed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_string_slice(
    ctx: *mut TaskCtx,
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

/// Append a borrowed interval to the caller's byte List. Import prepares and
/// charges tracked arena room before entering this leaf.
/// # Safety
/// Context, exclusive builder slot and borrowed source interval are live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_append(
    ctx: *mut TaskCtx,
    builder: *mut *mut Header,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    let bytes = unsafe {
        std::slice::from_raw_parts(payload(owner).add(8 + start as usize), length as usize)
    };
    unsafe { crate::io::append_byte_list(ctx, builder, bytes) };
    STATUS_OK
}

/// Replace initialized bytes without growth or length publication. The wrapper
/// validates before making the destination writable; this repeats ABI checks.
/// Based on Rust slice::copy_within (core/src/slice/mod.rs:4372-4388,
/// f6c1107f8be1), under MIT/Apache-2.0; see third-party/NOTICES.md.
/// # Safety
/// Context and list slot are live. For nonempty input the destination backing
/// is writable and the borrowed source interval stays live across wrapper COW.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_replace_range(
    _ctx: *mut TaskCtx,
    builder: *mut *mut Header,
    start: u64,
    owner: *mut Header,
    origin: u64,
    length: u64,
) -> u8 {
    unsafe {
        let block = builder.read();
        let len = payload(block).cast::<u64>().read();
        if !replacement_fits(len, start, length) {
            return STATUS_FAILURE;
        }
        if length == 0 {
            return STATUS_OK;
        }
        std::ptr::copy(
            payload(owner).add(8 + origin as usize),
            payload(block).add(sequence_elements_offset(1) as usize + start as usize),
            length as usize,
        );
    }
    STATUS_OK
}

/// Ordered subtraction avoids overflow and host-index narrowing.
pub fn replacement_fits(len: u64, start: u64, count: u64) -> bool {
    start <= len && count <= len - start
}

/// Append a forward LZ match without exposing a mutable alias to source code.
/// The compiled wrapper validates and charges arena room before this leaf.
/// Validation is repeated at the ABI boundary. Zero count never detaches.
/// # Safety
/// Context and exclusive byte-list slot are live; tracked arena room has been
/// prepared and charged by the wrapper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_append_from_history(
    ctx: *mut TaskCtx,
    out: *mut *mut Header,
    distance: u64,
    count: u64,
) -> u8 {
    unsafe {
        let len = payload(out.read()).cast::<u64>().read();
        if distance == 0
            || distance > len
            || count > (crate::alloc::EXTENT_MAX - 16).saturating_sub(len)
        {
            return STATUS_FAILURE;
        }
        if count == 0 {
            return STATUS_OK;
        }
        let (block, len) = crate::io::reserve_byte_list(ctx, out, count);
        // Reacquire all addresses after COW/reallocation. Each copy is disjoint:
        // the first covers at most distance bytes; subsequent copies double
        // the initialized prefix, including the short-distance pattern case.
        let data = payload(block).add(sequence_elements_offset(1) as usize);
        let source = data.add((len - distance) as usize);
        if distance == 1 {
            std::ptr::write_bytes(data.add(len as usize), source.read(), count as usize);
            payload(block).cast::<u64>().write(len + count);
            return STATUS_OK;
        }
        let mut copied = distance.min(count);
        std::ptr::copy_nonoverlapping(source, data.add(len as usize), copied as usize);
        while copied < count {
            let chunk = copied.min(count - copied);
            std::ptr::copy_nonoverlapping(
                data.add(len as usize),
                data.add((len + copied) as usize),
                chunk as usize,
            );
            copied += chunk;
        }
        payload(block).cast::<u64>().write(len + count);
    }
    STATUS_OK
}

/// A byte subinterval is checked completely before publication or sharing.
/// # Safety
/// The borrowed source is live and out names writable Slice storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_slice(
    ctx: *mut TaskCtx,
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
    _ctx: *mut TaskCtx,
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
    _ctx: *mut TaskCtx,
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

/// Search exactly the borrowed extents without retaining either owner.
/// # Safety
/// Both extents are initialized byte slices; `out` is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_find_any(
    _ctx: *mut TaskCtx,
    out: *mut u64,
    owner: *mut Header,
    start: u64,
    length: u64,
    needles_owner: *mut Header,
    needles_start: u64,
    needles_length: u64,
    offset: u64,
) -> u8 {
    let source = unsafe {
        bytes(Slice {
            owner,
            start,
            length,
        })
    };
    let needles = unsafe {
        bytes(Slice {
            owner: needles_owner,
            start: needles_start,
            length: needles_length,
        })
    };
    unsafe { out.write(hirsel_kernels::byte_scan::find_any(source, needles, offset)) };
    STATUS_OK
}

/// Strict JSON grammar over a borrowed extent; no lexeme allocation/conversion.
/// # Safety
/// The extent is live and initialized; `out` holds two writable u64 words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_json_number_end(
    ctx: *mut TaskCtx,
    out: *mut [u64; 2],
    owner: *mut Header,
    start: u64,
    length: u64,
    offset: u64,
) -> u8 {
    let source = unsafe {
        bytes(Slice {
            owner,
            start,
            length,
        })
    };
    let (result, cost) = hirsel_kernels::numeric::json_number(source, offset);
    let charged = unsafe { crate::budget::charge_primitive(ctx, cost) };
    unsafe { crate::stop::check(ctx, charged) };
    unsafe { out.write(result) };
    STATUS_OK
}

/// Exact bounded integer parsing; result words are high, low, status.
/// # Safety
/// The source is a live byte extent; `out` holds three writable u64 words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_integer(
    ctx: *mut TaskCtx,
    out: *mut [u64; 3],
    owner: *mut Header,
    start: u64,
    length: u64,
    radix: u8,
    signed: u8,
    positive_hi: u64,
    positive_lo: u64,
    negative_hi: u64,
    negative_lo: u64,
) -> u8 {
    let charged = unsafe { crate::budget::charge_primitive(ctx, length.saturating_add(1)) };
    unsafe { crate::stop::check(ctx, charged) };
    let source = unsafe {
        bytes(Slice {
            owner,
            start,
            length,
        })
    };
    let result = hirsel_kernels::numeric::integer(
        source,
        radix,
        signed != 0,
        u128::from(positive_hi) << 64 | u128::from(positive_lo),
        u128::from(negative_hi) << 64 | u128::from(negative_lo),
    );
    unsafe { out.write(result) };
    STATUS_OK
}

/// Decimal u64 emission into a caller-owned fixed scratch; tail bytes untouched.
/// # Safety
/// `out` and the exclusive initialized twenty-byte `buffer` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_decimal_word(
    ctx: *mut TaskCtx,
    out: *mut u64,
    value: u64,
    buffer: *mut [u8; 20],
) -> u8 {
    let charged = unsafe { crate::budget::charge_primitive(ctx, 9) };
    unsafe { crate::stop::check(ctx, charged) };
    let count = hirsel_kernels::numeric::decimal_word(value, unsafe { &mut *buffer });
    unsafe { out.write(count) };
    STATUS_OK
}
