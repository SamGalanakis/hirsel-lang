//! TLS Hirsel ABI v2. Layouts are the ordinary core TlsRoots/TlsIdentity/TlsServer/
//! TlsClient/Stream layouts; this adapter adds no TLS representation to the language.
use crate::io::{Binding, Completion, Request};
use crate::net::{ClientAuth, RootsDigest};
use crate::net_abi::{NetUnitResult, NetWordResult};
use crate::{Header, STATUS_FAILURE, STATUS_OK, TaskCtx};

/// TlsRoots = WebPki() | Pem(Bytes): the borrowed Pem slice, whose null owner word is
/// `WebPki` (its niche).
#[repr(C)]
pub struct TlsRootsAbi {
    pub pem: crate::views::Slice,
}

/// The roots digest of a `TlsRoots` argument, supplying Pem material outside the request.
/// WebPki's inactive payload need not be initialized.
unsafe fn roots(ctx: *mut TaskCtx, stream: u64, roots: *const TlsRootsAbi) -> RootsDigest {
    let pem = unsafe { &(*roots).pem };
    if pem.owner.is_null() {
        return RootsDigest::WebPki(crate::net_tls::WEBPKI_VERSION.into());
    }
    let bytes = unsafe { crate::io::slice_bytes(pem.owner, pem.start, pem.length) };
    unsafe { Binding::get(ctx) }.tls_roots(stream, bytes);
    crate::net_tls::pem_digest(bytes)
}

/// A client handshake request. The taken handle transfers on every outcome; State closes
/// it on data error.
unsafe fn connect(
    ctx: *mut TaskCtx,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    name: (*mut Header, u64, u64),
    roots: *const TlsRootsAbi,
    identity: Option<*const TlsIdentityAbi>,
    alpn: Vec<String>,
) -> u8 {
    unsafe { Binding::get(ctx) }.tls(crate::net_tls::engine());
    let stream = unsafe { s.read() };
    unsafe { s.write(0) };
    let name = unsafe { crate::io::slice_bytes(name.0, name.1, name.2) };
    let server_name = std::str::from_utf8(name)
        .unwrap_or_else(|_| crate::fault("invalid String in TLS ABI"))
        .to_owned();
    let roots = unsafe { self::roots(ctx, stream, roots) };
    let identity = identity.map(|identity| {
        let (chain, key) = unsafe { (*identity).bytes() };
        unsafe { Binding::get(ctx) }.tls_identity(stream, chain, key);
        crate::net_tls::chain_digest(chain)
    });
    let request = Request::TlsConnect {
        net: unsafe { n.read() },
        stream,
        server_name,
        roots,
        identity,
        alpn,
    };
    let done = match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::Protocol(done)) => done.map(|_| stream),
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { out.write(NetWordResult::of(done)) };
    STATUS_OK
}

/// `tls.connect(n, take s, server_name, roots) -> Result[Stream of n, NetError]`.
/// # Safety
/// ctx bound; n/s/roots readable; s writable and transferred; name and PEM slices
/// live for the call; out writable for NetWordResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_connect(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    name_owner: *mut Header,
    name_start: u64,
    name_len: u64,
    roots: *const TlsRootsAbi,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe {
        connect(
            ctx,
            out,
            n,
            s,
            (name_owner, name_start, name_len),
            roots,
            None,
            Vec::new(),
        )
    }
}

/// `tls.connect_as(n, take s, server_name, roots, identity) -> Result[Stream of n,
/// NetError]`: `tls.connect` presenting a client identity, whose material reaches the
/// adapter outside the recorded request.
/// # Safety
/// As `hirsel_rt_v2_tls_connect`; identity readable and its slices live for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_connect_as(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    name_owner: *mut Header,
    name_start: u64,
    name_len: u64,
    roots: *const TlsRootsAbi,
    identity: *const TlsIdentityAbi,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    unsafe {
        connect(
            ctx,
            out,
            n,
            s,
            (name_owner, name_start, name_len),
            roots,
            Some(identity),
            Vec::new(),
        )
    }
}

/// TlsClient = { roots: TlsRoots, identity: Option[TlsIdentity], alpn: List[String] }: the
/// roots at 0, the TlsIdentity at 24, whose null chain owner word is `None` (its niche), and
/// the ALPN list block at 72; 80 bytes.
#[repr(C)]
pub struct TlsClientAbi {
    pub roots: TlsRootsAbi,
    pub identity: TlsIdentityAbi,
    pub alpn: *mut Header,
}

/// `tls.connect_with(n, take s, server_name, client) -> Result[Stream of n, NetError]`:
/// `tls.connect` or `tls.connect_as` by the client's identity, offering its ALPN list.
/// # Safety
/// As `hirsel_rt_v2_tls_connect`; client readable and its list and slices live for the
/// call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_connect_with(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    name_owner: *mut Header,
    name_start: u64,
    name_len: u64,
    client: *const TlsClientAbi,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let identity = (!unsafe { (*client).identity.chain.owner }.is_null())
        .then(|| unsafe { &raw const (*client).identity });
    let alpn = unsafe { strings((*client).alpn) };
    unsafe {
        connect(
            ctx,
            out,
            n,
            s,
            (name_owner, name_start, name_len),
            &raw const (*client).roots,
            identity,
            alpn,
        )
    }
}

/// A borrowed `List[String]` block's elements, in order.
/// # Safety
/// The list and every element's slice are live.
unsafe fn strings(list: *mut Header) -> Vec<String> {
    let payload = crate::alloc::payload(list);
    let count = usize::try_from(unsafe { payload.cast::<u64>().read() })
        .unwrap_or_else(|_| crate::fault("String list count overflow"));
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    let size = std::mem::size_of::<crate::views::Slice>();
    (0..count)
        .map(|index| {
            let slice = unsafe {
                &*payload
                    .add(first + index * size)
                    .cast::<crate::views::Slice>()
            };
            std::str::from_utf8(unsafe {
                crate::io::slice_bytes(slice.owner, slice.start, slice.length)
            })
            .unwrap_or_else(|_| crate::fault("invalid String in TLS ABI"))
            .to_owned()
        })
        .collect()
}

/// TlsIdentity = { chain: Bytes, key: Bytes }: two borrowed slices.
#[repr(C)]
pub struct TlsIdentityAbi {
    pub chain: crate::views::Slice,
    pub key: crate::views::Slice,
}
impl TlsIdentityAbi {
    /// # Safety
    /// Both slices are live.
    unsafe fn bytes(&self) -> (&[u8], &[u8]) {
        let (chain, key) = (&self.chain, &self.key);
        unsafe {
            (
                crate::io::slice_bytes(chain.owner, chain.start, chain.length),
                crate::io::slice_bytes(key.owner, key.start, key.length),
            )
        }
    }
}

/// TlsClientAuth = Anonymous() | Request(Bytes) | Require(Bytes): u32 tag, padding, then the
/// trust set's slice.
#[repr(C)]
pub struct TlsClientAuthAbi {
    pub tag: u32,
    pub padding: u32,
    pub pem: crate::views::Slice,
}

/// TlsServer = { identities: List[TlsIdentity], clients: TlsClientAuth, alpn: List[String] }:
/// the list block, the client policy at 8 and the ALPN list block at 40; 48 bytes.
#[repr(C)]
pub struct TlsServerAbi {
    pub identities: *mut Header,
    pub clients: TlsClientAuthAbi,
    pub alpn: *mut Header,
}
/// A borrowed `TlsServer`: its identities' `(chain, key)` PEM, its client policy's trust
/// set and its ALPN list.
pub(crate) type ServerParts<'a> = (Vec<(&'a [u8], &'a [u8])>, ClientAuth<&'a [u8]>, Vec<String>);
impl TlsServerAbi {
    /// The server's parts, its PEM borrowed.
    /// # Safety
    /// The lists and every slice are live for `'a`.
    pub(crate) unsafe fn parts<'a>(&'a self) -> ServerParts<'a> {
        let payload = crate::alloc::payload(self.identities);
        let count = usize::try_from(unsafe { payload.cast::<u64>().read() })
            .unwrap_or_else(|_| crate::fault("identity count overflow"));
        let first = crate::alloc::sequence_elements_offset(8) as usize;
        let size = std::mem::size_of::<TlsIdentityAbi>();
        let identities = (0..count)
            .map(|index| unsafe {
                (*payload.add(first + index * size).cast::<TlsIdentityAbi>()).bytes()
            })
            .collect();
        let pem = &self.clients.pem;
        let trust = || unsafe { crate::io::slice_bytes(pem.owner, pem.start, pem.length) };
        let clients = match self.clients.tag {
            0 => ClientAuth::Anonymous,
            1 => ClientAuth::Request(trust()),
            2 => ClientAuth::Require(trust()),
            _ => crate::fault("TlsClientAuth tag out of range"),
        };
        (identities, clients, unsafe { strings(self.alpn) })
    }
}

/// `tls.accept(n, take s, server) -> Result[Stream of n, NetError]`. The taken handle
/// transfers on every outcome; State closes it on data error. The server's material reaches
/// the adapter outside the recorded request, which names only digests.
/// # Safety
/// ctx bound; n/s/server readable; s writable and transferred; the server's list and slices
/// live for the call; out writable for NetWordResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_accept(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    server: *const TlsServerAbi,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let stream = unsafe { s.read() };
    unsafe { s.write(0) };
    let (identities, clients, alpn) = unsafe { (*server).parts() };
    let binding = unsafe { Binding::get(ctx) };
    binding.tls(crate::net_tls::engine());
    binding.tls_server(stream, &identities, clients, &alpn);
    let server = crate::net::TlsServerDigest::of(&identities, clients, &alpn);
    let request = Request::TlsAccept {
        net: unsafe { n.read() },
        stream,
        server,
    };
    let done = match unsafe { crate::net_abi::call_recording(ctx, request) } {
        Some(Completion::Protocol(done)) => done.map(|_| stream),
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { out.write(NetWordResult::of(done)) };
    STATUS_OK
}

/// `Result[Option[Bytes], NetError]`: the tag, then at 8 either the Option, a Bytes slice
/// whose null owner word is `None` (its niche), or the NetError words; 32 bytes, align 8.
#[repr(C)]
pub struct NetChainResult {
    pub tag: u32,
    pub padding: u32,
    pub value: crate::views::Slice,
}

/// `tls.peer(n, s) -> Result[Option[Bytes], NetError]`: the verified client chain.
/// # Safety
/// ctx bound; n/s readable; out writable for NetChainResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_peer(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetChainResult,
    n: *const u64,
    s: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::TlsPeer {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
    };
    let value = match unsafe { crate::net_abi::call(ctx, request) } {
        Some(Completion::Certificates(value)) => value,
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { write_optional_bytes(ctx, out, value) };
    STATUS_OK
}

/// `tls.negotiated_protocol(n, s) -> Result[Option[String], NetError]`: the protocol the
/// stream's handshake negotiated, in `tls.peer`'s result image.
/// # Safety
/// ctx bound; n/s readable; out writable for NetChainResult.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_negotiated_protocol(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetChainResult,
    n: *const u64,
    s: *const u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let request = Request::TlsProtocol {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
    };
    let value = match unsafe { crate::net_abi::call(ctx, request) } {
        Some(Completion::Protocol(value)) => value,
        Some(_) => crate::fault("TLS completion type mismatch"),
        None => return STATUS_FAILURE,
    };
    unsafe { write_optional_bytes(ctx, out, value.map(|p| p.map(String::into_bytes))) };
    STATUS_OK
}

/// Write a `Result[Option[Bytes], NetError]` (or its `String` twin) result.
/// # Safety
/// ctx bound; out writable for NetChainResult.
unsafe fn write_optional_bytes(
    ctx: *mut TaskCtx,
    out: *mut NetChainResult,
    value: Result<Option<Vec<u8>>, crate::net::NetError>,
) {
    // Initialize padding and inactive bytes without manufacturing a pointer from error bits.
    unsafe {
        out.cast::<u8>()
            .write_bytes(0, std::mem::size_of::<NetChainResult>())
    };
    match value {
        Ok(None) => {}
        Ok(Some(chain)) => unsafe {
            (*out).value = crate::text::allocate_bytes(ctx, &chain);
        },
        Err(error) => unsafe {
            (*out).tag = 1;
            std::ptr::addr_of_mut!((*out).value)
                .cast::<crate::net_abi::NetErrorAbi>()
                .write(crate::net_abi::NetErrorAbi::of(error));
        },
    }
}

/// `tls.close(n, take s) -> Result[(), NetError]`: send close_notify, then
/// close on every outcome. Implicit drop uses the ordinary nonblocking Close.
/// # Safety
/// ctx bound; n readable; s writable and transferred; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_tls_close(
    ctx: *mut TaskCtx,
    _destination: *mut u8,
    out: *mut NetUnitResult,
    n: *const u64,
    s: *mut u64,
) -> u8 {
    // SAFETY: this semantic entry owns its task until it suspends or returns.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
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
