//! TLS Hirsel ABI v2. Layouts are the ordinary core TlsRoots/TlsIdentity/Stream layouts;
//! this adapter adds no TLS representation to the language.
use crate::io::{Binding, Completion, Request};
use crate::net::RootsDigest;
use crate::net_abi::{NetUnitResult, NetWordResult};
use crate::{Header, RunCtx, STATUS_FAILURE, STATUS_OK};

/// TlsRoots = WebPki() | Pem(Bytes): u32 tag, padding, then a borrowed slice.
#[repr(C)]
pub struct TlsRootsAbi {
    pub tag: u32,
    pub padding: u32,
    pub pem: crate::views::Slice,
}

/// `tls.connect(n, take s, server_name, roots) -> Result[Stream of n, NetError]`.
/// The taken handle transfers on every outcome; State closes it on data error.
/// # Safety
/// ctx bound; n/s/roots readable; s writable and transferred; name and PEM slices
/// live for the call; out writable for NetWordResult. WebPki's inactive payload
/// need not be initialized.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_connect(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    name_owner: *mut Header,
    name_start: u64,
    name_len: u64,
    roots: *const TlsRootsAbi,
) -> u8 {
    let stream = unsafe { s.read() };
    unsafe { s.write(0) };
    let name = unsafe { crate::io::slice_bytes(name_owner, name_start, name_len) };
    let server_name = std::str::from_utf8(name)
        .unwrap_or_else(|_| crate::fault("invalid String in TLS ABI"))
        .to_owned();
    let roots = match unsafe { (*roots).tag } {
        0 => RootsDigest::WebPki(crate::net_tls::WEBPKI_VERSION.into()),
        1 => {
            // Read only the active Pem payload.
            let pem = unsafe { &(*roots).pem };
            let bytes = unsafe { crate::io::slice_bytes(pem.owner, pem.start, pem.length) };
            unsafe { Binding::get(ctx) }.tls_roots(stream, bytes);
            crate::net_tls::pem_digest(bytes)
        }
        _ => crate::fault("TlsRoots tag out of range"),
    };
    let request = Request::TlsConnect {
        net: unsafe { n.read() },
        stream,
        server_name,
        roots,
    };
    let done = match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::NetUnit(done)) => done.map(|()| stream),
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { out.write(NetWordResult::of(done)) };
    STATUS_OK
}

/// TlsIdentity = { chain: Bytes, key: Bytes }: two borrowed slices.
#[repr(C)]
pub struct TlsIdentityAbi {
    pub chain: crate::views::Slice,
    pub key: crate::views::Slice,
}

/// `tls.accept(n, take s, identity) -> Result[Stream of n, NetError]`. The taken handle
/// transfers on every outcome; State closes it on data error. The identity material reaches
/// the adapter outside the recorded request, which names only the chain digest.
/// # Safety
/// ctx bound; n/s/identity readable; s writable and transferred; the identity's slices live
/// for the call; out writable for NetWordResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_accept(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    identity: *const TlsIdentityAbi,
) -> u8 {
    let stream = unsafe { s.read() };
    unsafe { s.write(0) };
    let (chain, key) = unsafe { (&(*identity).chain, &(*identity).key) };
    let chain = unsafe { crate::io::slice_bytes(chain.owner, chain.start, chain.length) };
    let key = unsafe { crate::io::slice_bytes(key.owner, key.start, key.length) };
    unsafe { Binding::get(ctx) }.tls_identity(stream, chain, key);
    let request = Request::TlsAccept {
        net: unsafe { n.read() },
        stream,
        identity: crate::net_tls::chain_digest(chain),
    };
    let done = match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::NetUnit(done)) => done.map(|()| stream),
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { out.write(NetWordResult::of(done)) };
    STATUS_OK
}

/// `tls.close(n, take s) -> Result[(), NetError]`: send close_notify, then
/// close on every outcome. Implicit drop uses the ordinary nonblocking Close.
/// # Safety
/// ctx bound; n readable; s writable and transferred; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_close(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetUnitResult,
    n: *const u64,
    s: *mut u64,
) -> u8 {
    let stream = unsafe { s.read() };
    unsafe { s.write(0) };
    let request = Request::TlsClose {
        net: unsafe { n.read() },
        stream,
    };
    let done = match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::NetUnit(done)) => done,
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { out.write(NetUnitResult::of(done)) };
    STATUS_OK
}
