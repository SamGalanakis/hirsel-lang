//! Network operations at the Hirsel ABI v1 (`HIRSEL_ABI_EXPORTS`).
//! Each export builds one typed `Request`; policy and recording happen in the adapter.
//! Expected network errors are `Error` results; a stream reached through a Net that does
//! not brand it is a CAP-2 refusal (status 1, recorded by the compiled caller).
//!
//! Layouts follow `hirsel_lir::layout` (tag word, payload at its alignment, declaration
//! order): `NetError` is a u32 tag and a 12-byte payload union at offset 4, `SocketAddr` is
//! `IpAddr` (u32 tag, octets at 4) then the port at 20, and the results below.
use crate::io::{Completion, Request};
use crate::net::{
    NetError, NetOperation, NetRule, NetTimeout, ShutdownHow, SocketAddr, TimeoutKind, TlsFailure,
};
use crate::{Header, RunCtx, STATUS_FAILURE, STATUS_OK};

/// `NetError` (docs/spec/core-library.md variant order): tag, then the payload words of Canceled(CancelCause),
/// Tls(TlsFailure) (its tag, then the alert byte) or Fault(u32).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetErrorAbi {
    pub tag: u32,
    pub word: u32,
    pub extra: u32,
}
impl NetErrorAbi {
    pub fn of(error: NetError) -> Self {
        let tls = |f: TlsFailure| match f {
            TlsFailure::UnknownIssuer => (0, 0),
            TlsFailure::Expired => (1, 0),
            TlsFailure::NotYetValid => (2, 0),
            TlsFailure::NameMismatch => (3, 0),
            TlsFailure::Revoked => (4, 0),
            TlsFailure::BadCertificate => (5, 0),
            TlsFailure::AlertReceived(alert) => (6, u32::from(alert)),
            TlsFailure::HandshakeFailure => (7, 0),
            TlsFailure::PeerMisbehaved => (8, 0),
            TlsFailure::Truncated => (9, 0),
        };
        let (tag, word, extra) = match error {
            NetError::NotGranted => (0, 0, 0),
            NetError::Canceled(cause) => (1, cause, 0),
            NetError::InvalidInput => (2, 0, 0),
            NetError::UnknownHost => (3, 0, 0),
            NetError::ResolverFailure => (4, 0, 0),
            NetError::ConnectionRefused => (5, 0, 0),
            NetError::ConnectionReset => (6, 0, 0),
            NetError::ConnectionAborted => (7, 0, 0),
            NetError::NotConnected => (8, 0, 0),
            NetError::AddrInUse => (9, 0, 0),
            NetError::AddrNotAvailable => (10, 0, 0),
            NetError::HostUnreachable => (11, 0, 0),
            NetError::NetworkUnreachable => (12, 0, 0),
            NetError::NetworkDown => (13, 0, 0),
            NetError::BrokenPipe => (14, 0, 0),
            NetError::TimedOut => (15, 0, 0),
            NetError::Tls(f) => {
                let (word, extra) = tls(f);
                (16, word, extra)
            }
            NetError::Fault(code) => (17, code, 0),
        };
        Self { tag, word, extra }
    }
}

/// `Result[T, NetError]` for an eight-byte `T` (a Net, Stream or u64): 24 bytes, align 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetWordResult {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
    pub rest: u64,
}
impl NetWordResult {
    pub fn of(value: Result<u64, NetError>) -> Self {
        match value {
            Ok(value) => Self {
                value,
                ..Self::default()
            },
            Err(error) => {
                let e = NetErrorAbi::of(error);
                Self {
                    tag: 1,
                    padding: 0,
                    value: u64::from(e.tag) | u64::from(e.word) << 32,
                    rest: u64::from(e.extra),
                }
            }
        }
    }
}

/// `Result[(), NetError]`: the tag, then the error at offset 4; 16 bytes, align 4.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetUnitResult {
    pub tag: u32,
    pub error: NetErrorAbi,
}
impl NetUnitResult {
    pub fn of(value: Result<(), NetError>) -> Self {
        match value {
            Ok(()) => Self::default(),
            Err(error) => Self {
                tag: 1,
                error: NetErrorAbi::of(error),
            },
        }
    }
}

/// `SocketAddr`: `IpAddr` (u32 tag, V4 octets or V6 octets at 4), then the port at 20.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SocketAddrAbi {
    pub ip_tag: u32,
    pub octets: [u8; 16],
    pub port: u16,
    pub padding: u16,
}
impl SocketAddrAbi {
    pub fn of(addr: SocketAddr) -> Self {
        let mut octets = [0; 16];
        let ip_tag = match addr.ip {
            std::net::IpAddr::V4(ip) => {
                octets[..4].copy_from_slice(&ip.octets());
                0
            }
            std::net::IpAddr::V6(ip) => {
                octets = ip.octets();
                1
            }
        };
        Self {
            ip_tag,
            octets,
            port: addr.port,
            padding: 0,
        }
    }
    pub fn get(&self) -> SocketAddr {
        let ip = match self.ip_tag {
            0 => std::net::IpAddr::from(<[u8; 4]>::try_from(&self.octets[..4]).unwrap()),
            1 => std::net::IpAddr::from(self.octets),
            _ => crate::fault("IpAddr tag out of range"),
        };
        SocketAddr {
            ip,
            port: self.port,
        }
    }
}

/// `Result[SocketAddr, NetError]`: the tag, then either payload at offset 4; 28 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetAddrResult {
    pub tag: u32,
    pub value: SocketAddrAbi,
}
impl NetAddrResult {
    pub fn of(value: Result<SocketAddr, NetError>) -> Self {
        match value {
            Ok(addr) => Self {
                tag: 0,
                value: SocketAddrAbi::of(addr),
            },
            Err(error) => {
                let e = NetErrorAbi::of(error);
                let mut result = Self {
                    tag: 1,
                    value: SocketAddrAbi::default(),
                };
                // SAFETY: the 12 error bytes fit in the 24-byte payload union.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (&raw const e).cast::<u8>(),
                        (&raw mut result.value).cast::<u8>(),
                        std::mem::size_of::<NetErrorAbi>(),
                    )
                };
                result
            }
        }
    }
}

/// `NetTimeout`: u32 tag, the u64 of `Nanos` at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetTimeoutAbi {
    pub tag: u32,
    pub padding: u32,
    pub nanos: u64,
}
impl NetTimeoutAbi {
    pub fn get(&self) -> NetTimeout {
        match self.tag {
            0 => NetTimeout::Unbounded,
            1 => NetTimeout::Nanos(self.nanos),
            _ => crate::fault("NetTimeout tag out of range"),
        }
    }
}

/// Run `request` through the bound adapter's `State`; `None` after a CAP-2 refusal.
/// Shared with the TLS exports (net-t4).
pub(crate) unsafe fn call(ctx: *mut RunCtx, request: Request) -> Option<Completion> {
    unsafe { crate::io::capability_call(ctx, request) }.ok()
}
pub(crate) unsafe fn text(owner: *mut Header, start: u64, len: u64) -> String {
    std::str::from_utf8(unsafe { crate::io::slice_bytes(owner, start, len) })
        .unwrap_or_else(|_| crate::fault("String operand is not UTF-8"))
        .to_owned()
}
pub(crate) fn mismatch() -> ! {
    crate::fault("network completion type mismatch")
}

/// `net.narrow(n: Net, rules: List[NetRule]) -> Result[Net, NetError]`. Each NetRule
/// element is 32 bytes: the address String slice, the port at 24, the operation tag at 28.
/// # Safety
/// Context bound; n readable; rules a live List[NetRule] block; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_narrow(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    rules: *mut Header,
) -> u8 {
    let payload = crate::alloc::payload(rules);
    let count = unsafe { payload.cast::<u64>().read() };
    let count = usize::try_from(count).unwrap_or_else(|_| crate::fault("rule count overflow"));
    let first = crate::alloc::sequence_elements_offset(8);
    let mut list = Vec::with_capacity(count);
    for index in 0..count {
        let offset = (index as u64)
            .checked_mul(32)
            .and_then(|n| n.checked_add(first))
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or_else(|| crate::fault("rule offset overflow"));
        let entry = unsafe { payload.add(offset) };
        let address = unsafe { entry.cast::<crate::views::Slice>().read() };
        let port = unsafe { entry.add(24).cast::<u16>().read() };
        let operation = match unsafe { entry.add(28).cast::<u32>().read() } {
            0 => NetOperation::Connect,
            1 => NetOperation::ConnectTls,
            2 => NetOperation::Listen,
            _ => crate::fault("NetOperation tag out of range"),
        };
        list.push(NetRule {
            address: unsafe { text(address.owner, address.start, address.length) },
            port,
            operation,
        });
    }
    let request = Request::NetNarrow {
        net: unsafe { n.read() },
        rules: list,
    };
    let Some(Completion::NetHandle(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.connect(n, host: String, port: u16, timeout: NetTimeout) -> Result[Stream of n, NetError]`.
/// # Safety
/// Context bound; n and timeout readable; the host slice borrowed; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_connect(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    owner: *mut Header,
    start: u64,
    len: u64,
    port: u16,
    timeout: *const NetTimeoutAbi,
) -> u8 {
    let request = Request::Connect {
        net: unsafe { n.read() },
        host: unsafe { text(owner, start, len) },
        port,
        timeout: unsafe { (*timeout).get() },
    };
    let Some(Completion::NetHandle(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.connect_addr(n, addr: SocketAddr, timeout: NetTimeout) -> Result[Stream of n, NetError]`.
/// # Safety
/// Context bound; n, addr and timeout readable; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_connect_addr(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    addr: *const SocketAddrAbi,
    timeout: *const NetTimeoutAbi,
) -> u8 {
    let request = Request::ConnectAddr {
        net: unsafe { n.read() },
        addr: unsafe { (*addr).get() },
        timeout: unsafe { (*timeout).get() },
    };
    let Some(Completion::NetHandle(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.resolve(n, host: String, port: u16) -> Result[List[SocketAddr], NetError]`. The list
/// is a plain sequence (u64 count, then 24-byte SocketAddr elements from offset 8) under the
/// runtime's pointer-free sequence descriptor 0, as `file.read_bytes` allocates Bytes.
/// # Safety
/// Context bound; n readable; the host slice borrowed; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_resolve(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    owner: *mut Header,
    start: u64,
    len: u64,
    port: u16,
) -> u8 {
    let request = Request::Resolve {
        net: unsafe { n.read() },
        host: unsafe { text(owner, start, len) },
        port,
    };
    let Some(Completion::Addresses(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    let value = value.map(|addresses| unsafe { address_list(ctx, &addresses) } as u64);
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// A fresh `List[SocketAddr]` block holding `addresses`.
/// # Safety
/// `ctx` is a live run context.
pub unsafe fn address_list(ctx: *mut RunCtx, addresses: &[SocketAddr]) -> *mut Header {
    let size = std::mem::size_of::<SocketAddrAbi>();
    // The pointer-free byte storage descriptor's extent fits any pointer-free list.
    let block =
        unsafe { crate::alloc::allocate_sequence(ctx, 0, addresses.len() as u64, size as u64, 8) };
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    unsafe {
        let payload = crate::alloc::payload(block);
        for (i, address) in addresses.iter().enumerate() {
            payload
                .add(first + i * size)
                .cast::<SocketAddrAbi>()
                .write_unaligned(SocketAddrAbi::of(*address));
        }
    }
    block
}

/// `net.read(n, mut s, mut buffer: List[u8], max: u64) -> Result[u64, NetError]`: append the
/// delivered bytes to the caller's list, growing it through `hirsel_rt_v2_seq_grow`.
/// # Safety
/// Context bound; n readable; s and buffer exclusive addresses of live values; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_read(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    buffer: *mut *mut Header,
    max: u64,
) -> u8 {
    let request = Request::NetRead {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
        max,
    };
    let Some(Completion::NetBytes(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    let value = value.map(|bytes| {
        if !bytes.is_empty() {
            unsafe {
                let block = buffer.read();
                let count = crate::alloc::payload(block).cast::<u64>().read();
                let total = count
                    .checked_add(bytes.len() as u64)
                    .unwrap_or_else(|| crate::fault("List[u8] length overflow"));
                let block = crate::alloc::hirsel_rt_v2_seq_grow(ctx, block, 1, 1, total);
                let payload = crate::alloc::payload(block);
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    payload
                        .add(crate::alloc::sequence_elements_offset(1) as usize + count as usize),
                    bytes.len(),
                );
                payload.cast::<u64>().write(total);
                buffer.write(block);
            }
        }
        bytes.len() as u64
    });
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.read_into(n, mut s, mut buffer: Inline[u8, k])` as its compiled body calls it: the
/// buffer's address and `capacity = k`. At most `capacity` bytes are written from the start
/// of the buffer; the rest keep their values.
/// # Safety
/// Context bound; n readable; s exclusive; buffer exclusive for `capacity` bytes; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_read_into(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    buffer: *mut u8,
    capacity: u64,
) -> u8 {
    let request = Request::NetRead {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
        max: capacity,
    };
    let Some(Completion::NetBytes(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    // `State` refuses a completion longer than `max`, so the copy stays inside the buffer.
    let value = value.map(|bytes| {
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        bytes.len() as u64
    });
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.write(n, mut s, data: Bytes) -> Result[u64, NetError]`; data is borrowed.
/// # Safety
/// Context bound; n readable; s exclusive; the slice borrowed; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_write(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    s: *mut u64,
    owner: *mut Header,
    start: u64,
    len: u64,
) -> u8 {
    let request = Request::NetWrite {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
        bytes: unsafe { crate::io::slice_bytes(owner, start, len) }.to_vec(),
    };
    let Some(Completion::Count(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}

/// `net.write_all(n, mut s, data: Bytes) -> Result[(), NetError]`; data is borrowed.
/// # Safety
/// As for `hirsel_rt_v2_net_write`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_write_all(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetUnitResult,
    n: *const u64,
    s: *mut u64,
    owner: *mut Header,
    start: u64,
    len: u64,
) -> u8 {
    let request = Request::NetWriteAll {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
        bytes: unsafe { crate::io::slice_bytes(owner, start, len) }.to_vec(),
    };
    let Some(Completion::NetUnit(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetUnitResult::of(value)) };
    STATUS_OK
}

/// `net.shutdown(n, mut s, how: Shutdown) -> Result[(), NetError]`.
/// # Safety
/// Context bound; n and how readable; s exclusive; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_shutdown(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetUnitResult,
    n: *const u64,
    s: *mut u64,
    how: *const u32,
) -> u8 {
    let how = match unsafe { how.read() } {
        0 => ShutdownHow::Read,
        1 => ShutdownHow::Write,
        2 => ShutdownHow::Both,
        _ => crate::fault("Shutdown tag out of range"),
    };
    let request = Request::Shutdown {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
        how,
    };
    let Some(Completion::NetUnit(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetUnitResult::of(value)) };
    STATUS_OK
}

macro_rules! timeout_export {
    ($name:ident, $which:expr) => {
        /// `net.set_*_timeout(n, mut s, timeout: NetTimeout) -> Result[(), NetError]`.
        /// # Safety
        /// Context bound; n and timeout readable; s exclusive; out writable.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut RunCtx,
            _destination: *mut u8,
            out: *mut NetUnitResult,
            n: *const u64,
            s: *mut u64,
            timeout: *const NetTimeoutAbi,
        ) -> u8 {
            let request = Request::SetTimeout {
                net: unsafe { n.read() },
                stream: unsafe { s.read() },
                which: $which,
                timeout: unsafe { (*timeout).get() },
            };
            let Some(Completion::NetUnit(value)) = (unsafe { call(ctx, request) }) else {
                return STATUS_FAILURE;
            };
            unsafe { out.write(NetUnitResult::of(value)) };
            STATUS_OK
        }
    };
}
timeout_export!(hirsel_rt_v2_net_set_read_timeout, TimeoutKind::Read);
timeout_export!(hirsel_rt_v2_net_set_write_timeout, TimeoutKind::Write);

/// `net.peer_addr(n, s) -> Result[SocketAddr, NetError]`.
/// # Safety
/// Context bound; n and s readable; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_peer_addr(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetAddrResult,
    n: *const u64,
    s: *const u64,
) -> u8 {
    let request = Request::PeerAddr {
        net: unsafe { n.read() },
        stream: unsafe { s.read() },
    };
    let Some(Completion::Address(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetAddrResult::of(value)) };
    STATUS_OK
}

/// `net.close(n, take s) -> ()`: the drop at a chosen point. The stream transfers at entry.
/// # Safety
/// Context bound; n readable; s the address of a live Stream ID the call consumes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_close(
    ctx: *mut RunCtx,
    n: *const u64,
    s: *mut u64,
) -> u8 {
    let _ = unsafe { n.read() };
    let handle = unsafe { s.read() };
    // Erasing the transferred word makes accidental reuse fail closed.
    unsafe { s.write(0) };
    match unsafe { call(ctx, Request::Close { handle }) } {
        Some(Completion::Unit(Ok(()))) => STATUS_OK,
        Some(_) => mismatch(),
        None => STATUS_FAILURE,
    }
}

/// `net.listen(n, addr) -> Result[Listener of n, NetError]`.
/// # Safety
/// Context bound; n and addr readable; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_listen(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetWordResult,
    n: *const u64,
    addr: *const SocketAddrAbi,
) -> u8 {
    let request = Request::Listen {
        net: unsafe { n.read() },
        addr: unsafe { (*addr).get() },
    };
    let Some(Completion::NetHandle(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetWordResult::of(value)) };
    STATUS_OK
}
/// `net.local_addr(n, listener) -> Result[SocketAddr, NetError]`.
/// # Safety
/// Context bound; n and listener readable; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_local_addr(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetAddrResult,
    n: *const u64,
    listener: *const u64,
) -> u8 {
    let request = Request::LocalAddr {
        net: unsafe { n.read() },
        listener: unsafe { listener.read() },
    };
    let Some(Completion::Address(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    unsafe { out.write(NetAddrResult::of(value)) };
    STATUS_OK
}
/// `Result[(Stream,SocketAddr),NetError]`: eight-byte-aligned payload at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NetAcceptedResult {
    pub tag: u32,
    pub padding: u32,
    pub stream: u64,
    pub address: SocketAddrAbi,
}
/// `net.accept(n, mut listener)`.
/// # Safety
/// Context bound; n and listener readable; out writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_net_accept(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut NetAcceptedResult,
    n: *const u64,
    listener: *const u64,
) -> u8 {
    let request = Request::Accept {
        net: unsafe { n.read() },
        listener: unsafe { listener.read() },
    };
    let Some(Completion::Accepted(value)) = (unsafe { call(ctx, request) }) else {
        return STATUS_FAILURE;
    };
    let mut result = NetAcceptedResult::default();
    match value {
        Ok((stream, addr)) => {
            result.stream = stream;
            result.address = SocketAddrAbi::of(addr);
        }
        Err(e) => {
            result.tag = 1;
            let error = NetErrorAbi::of(e);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    (&raw const error).cast::<u8>(),
                    (&raw mut result.stream).cast::<u8>(),
                    std::mem::size_of::<NetErrorAbi>(),
                )
            };
        }
    }
    unsafe { out.write(result) };
    STATUS_OK
}
