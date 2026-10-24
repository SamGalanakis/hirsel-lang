//! Borrowed SHA extents and caller-owned Inline results. All algorithms are RustCrypto.
use crate::{Header, STATUS_OK, TaskCtx, views::Slice};
use hirsel_kernels::sha::Algorithm;

unsafe fn hash(
    ctx: *mut TaskCtx,
    algorithm: Algorithm,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> crate::stop::Stopped<[u8; 64]> {
    let source = unsafe {
        crate::text::bytes(Slice {
            owner,
            start,
            length,
        })
    };
    hirsel_kernels::sha::digest(algorithm, source, |cost| unsafe {
        crate::budget::charge_primitive(ctx, cost)
    })
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 20 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha1(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    if length > u64::MAX / 8 {
        return crate::STATUS_FAILURE;
    }
    let result = unsafe { hash(ctx, Algorithm::Sha1, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 20) };
    STATUS_OK
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 28 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha224(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    if length > u64::MAX / 8 {
        return crate::STATUS_FAILURE;
    }
    let result = unsafe { hash(ctx, Algorithm::Sha224, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 28) };
    STATUS_OK
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 32 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha256(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    if length > u64::MAX / 8 {
        return crate::STATUS_FAILURE;
    }
    let result = unsafe { hash(ctx, Algorithm::Sha256, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 32) };
    STATUS_OK
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 48 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha384(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    let result = unsafe { hash(ctx, Algorithm::Sha384, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 48) };
    STATUS_OK
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 64 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha512(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    let result = unsafe { hash(ctx, Algorithm::Sha512, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 64) };
    STATUS_OK
}

/// Hash exactly the borrowed byte extent, without allocation or owner transfer.
/// # Safety
/// Context/input are live and initialized; output addresses 32 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_bytes_sha512_256(
    ctx: *mut TaskCtx,
    out: *mut u8,
    owner: *mut Header,
    start: u64,
    length: u64,
) -> u8 {
    let result = unsafe { hash(ctx, Algorithm::Sha512_256, owner, start, length) };
    let digest = unsafe { crate::stop::check(ctx, result) };
    unsafe { std::ptr::copy_nonoverlapping(digest.as_ptr(), out, 32) };
    STATUS_OK
}
