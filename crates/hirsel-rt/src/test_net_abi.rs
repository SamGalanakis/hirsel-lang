//! Fixed core@1 `test.net` ABI. The layouts are the ordinary repr-C core layouts:
//! NetPeer (64 bytes), IpAddr (20), NetConnection (16), NetStep (32), PeerTls (16:
//! u32 tag, payload at 8), TlsFailure (8: u32 tag, AlertReceived's u8 at 4),
//! SocketAddr (24: IpAddr, port at 20), NetListener (32: SocketAddr, clients at 24)
//! and NetClient (48: SocketAddr, PeerTls at 24, steps at 40).
//! Native campaign programs exercise these layouts.
use crate::{
    Header, RunCtx,
    io::{Completion, Request},
};
use hirsel_run_config::{
    NetClient, NetConnection, NetListener, NetPeer, NetStep, PeerTls, TlsFailure,
};

unsafe fn list<T>(
    owner: *mut Header,
    stride: usize,
    mut item: impl FnMut(*const u8) -> T,
) -> Vec<T> {
    let payload = crate::alloc::payload(owner);
    let count = usize::try_from(unsafe { payload.cast::<u64>().read() })
        .unwrap_or_else(|_| crate::fault("Test peer list overflow"));
    // Every element here is at most 8-aligned.
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let offset = index
            .checked_mul(stride)
            .and_then(|n| n.checked_add(first))
            .filter(|n| *n <= isize::MAX as usize)
            .unwrap_or_else(|| crate::fault("Test peer element overflow"));
        values.push(item(unsafe { payload.add(offset) }));
    }
    values
}
unsafe fn bytes(at: *const u8) -> Vec<u8> {
    let slice = unsafe { at.cast::<crate::views::Slice>().read() };
    unsafe { crate::io::slice_bytes(slice.owner, slice.start, slice.length) }.to_vec()
}
unsafe fn text(at: *const u8) -> String {
    String::from_utf8(unsafe { bytes(at) })
        .unwrap_or_else(|_| crate::fault("Test peer String is not UTF-8"))
}
unsafe fn pointer(at: *const u8) -> *mut Header {
    unsafe { at.cast::<*mut Header>().read() }
}
unsafe fn tag(at: *const u8) -> u32 {
    unsafe { at.cast::<u32>().read() }
}
unsafe fn step(at: *const u8) -> NetStep {
    match unsafe { tag(at) } {
        0 => NetStep::Send(unsafe { bytes(at.add(8)) }),
        1 => NetStep::Expect(unsafe { bytes(at.add(8)) }),
        2 => NetStep::Stall,
        3 => NetStep::Close,
        4 => NetStep::Reset,
        5 => NetStep::Truncate,
        _ => crate::fault("Test peer NetStep tag"),
    }
}
unsafe fn connection(at: *const u8) -> NetConnection {
    match unsafe { tag(at) } {
        0 => NetConnection::Refused,
        1 => NetConnection::Unanswered,
        2 => NetConnection::Open(unsafe { list(pointer(at.add(8)), 32, |at| step(at)) }),
        _ => crate::fault("Test peer NetConnection tag"),
    }
}
unsafe fn ip(at: *const u8) -> String {
    match unsafe { tag(at) } {
        0 => std::net::Ipv4Addr::from(unsafe { at.add(4).cast::<[u8; 4]>().read() }).to_string(),
        1 => std::net::Ipv6Addr::from(unsafe { at.add(4).cast::<[u8; 16]>().read() }).to_string(),
        _ => crate::fault("Test peer IpAddr tag"),
    }
}
unsafe fn failure(at: *const u8) -> TlsFailure {
    match unsafe { tag(at) } {
        0 => TlsFailure::UnknownIssuer,
        1 => TlsFailure::Expired,
        2 => TlsFailure::NotYetValid,
        3 => TlsFailure::NameMismatch,
        4 => TlsFailure::Revoked,
        5 => TlsFailure::BadCertificate,
        6 => TlsFailure::AlertReceived(unsafe { at.add(4).read() }),
        7 => TlsFailure::HandshakeFailure,
        8 => TlsFailure::PeerMisbehaved,
        9 => TlsFailure::Truncated,
        _ => crate::fault("Test peer TlsFailure tag"),
    }
}
unsafe fn tls(at: *const u8) -> PeerTls {
    match unsafe { tag(at) } {
        0 => PeerTls::Plain,
        1 => PeerTls::Certificate(unsafe { list(pointer(at.add(8)), 24, |at| text(at)) }),
        2 => PeerTls::Fails(unsafe { failure(at.add(8)) }),
        _ => crate::fault("Test peer PeerTls tag"),
    }
}
unsafe fn peer(at: *const u8) -> NetPeer {
    NetPeer {
        address: unsafe { text(at) },
        port: unsafe { at.add(24).cast::<u16>().read() },
        resolves_to: unsafe { list(pointer(at.add(32)), 20, |at| ip(at)) },
        tls: unsafe { tls(at.add(40)) },
        connections: unsafe { list(pointer(at.add(56)), 16, |at| connection(at)) },
    }
}
unsafe fn socket_addr(at: *const u8) -> (String, u16) {
    (unsafe { ip(at) }, unsafe {
        at.add(20).cast::<u16>().read()
    })
}
unsafe fn listener(at: *const u8) -> NetListener {
    let (address, port) = unsafe { socket_addr(at) };
    NetListener {
        address,
        port,
        clients: unsafe {
            list(pointer(at.add(24)), 48, |at| {
                let (address, port) = socket_addr(at);
                NetClient {
                    address,
                    port,
                    tls: tls(at.add(24)),
                    steps: list(pointer(at.add(40)), 32, |at| step(at)),
                }
            })
        },
    }
}

/// # Safety
/// Context is bound; Test and the complete nested core peer lists are borrowed live
/// values; out is writable. No borrowed input is retained by the adapter.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_net(
    ctx: *mut RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
    peers: *mut Header,
    listeners: *mut Header,
) -> u8 {
    let request = Request::TestNet {
        test: unsafe { test.read() },
        peers: unsafe { list(peers, 64, |at| peer(at)) },
        listeners: unsafe { list(listeners, 32, |at| listener(at)) },
    };
    let completion = match unsafe { crate::io::capability_call(ctx, request) } {
        Ok(completion) => completion,
        Err(()) => return crate::STATUS_FAILURE,
    };
    let Completion::Handle(Ok(net)) = completion else {
        crate::fault("Test Net completion mismatch")
    };
    unsafe { out.write(net) };
    crate::STATUS_OK
}
