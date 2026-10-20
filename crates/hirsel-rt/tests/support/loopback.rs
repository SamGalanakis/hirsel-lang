//! Loopback recording of the network tests and of their interpreter-replay twin in
//! hirsel-pipeline-tests (`rt_net`): an in-process server, the exchange against it and
//! replay with no sockets.
use hirsel_rt::io::*;
use hirsel_rt::io_real::RealIo;
use hirsel_rt::io_sim::SimIo;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
pub const IO: u64 = 1;

pub fn rule(address: &str, port: u16, operation: NetOperation) -> NetRule {
    NetRule {
        address: address.into(),
        port,
        operation,
    }
}

pub fn loopback(port: u16) -> SocketAddr {
    SocketAddr {
        ip: std::net::Ipv4Addr::LOCALHOST.into(),
        port,
    }
}

pub fn real(rules: impl IntoIterator<Item = NetRule>) -> RealIo {
    let mut io = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    super::host(&mut io);
    io.set_net_rules(rules.into_iter().collect());
    io
}

pub fn net(io: &mut impl Adapter) -> u64 {
    match io.call(Request::Net { io: IO }) {
        Ok(Completion::Handle(Ok(id))) => id,
        other => panic!("io.net: {other:?}"),
    }
}

pub fn handle(completion: Result<Completion, AdapterError>) -> u64 {
    match completion {
        Ok(Completion::NetHandle(Ok(id)) | Completion::Listened(Ok((id, _)))) => id,
        other => panic!("expected a handle: {other:?}"),
    }
}

pub fn read_to_end(io: &mut impl Adapter, net: u64, stream: u64, max: u64) -> Vec<Vec<u8>> {
    let mut chunks = vec![];
    loop {
        match io.call(Request::NetRead { net, stream, max }) {
            Ok(Completion::NetBytes(Ok(bytes))) if bytes.is_empty() => return chunks,
            Ok(Completion::NetBytes(Ok(bytes))) => {
                assert!(bytes.len() as u64 <= max);
                chunks.push(bytes);
            }
            other => panic!("read: {other:?}"),
        }
    }
}

/// A one-connection server: read the request until the client half-closes, then send
/// the response and close.
pub fn serve(response: &'static [u8]) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = vec![];
        stream.read_to_end(&mut request).unwrap();
        stream.write_all(response).unwrap();
        request
    });
    (port, thread)
}

/// The recorded program: connect, send, half-close, read to EOF in small reads, close.
pub fn exchange(io: &mut impl Adapter, port: u16, message: &[u8]) -> Vec<Vec<u8>> {
    let n = net(io);
    let s = handle(io.call(Request::ConnectAddr {
        net: n,
        addr: loopback(port),
        timeout: NetTimeout::Nanos(5_000_000_000),
    }));
    assert_eq!(
        io.call(Request::NetWriteAll {
            net: n,
            stream: s,
            bytes: message.to_vec(),
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    assert_eq!(
        io.call(Request::Shutdown {
            net: n,
            stream: s,
            how: ShutdownHow::Write,
        }),
        Ok(Completion::NetUnit(Ok(())))
    );
    let chunks = read_to_end(io, n, s, 4);
    assert_eq!(
        io.call(Request::Close { handle: s }),
        Ok(Completion::Unit(Ok(())))
    );
    chunks
}

pub fn record_exchange() -> (BTreeSet<NetRule>, u16, Vec<Event>, Vec<Vec<u8>>) {
    let (port, server) = serve(b"pong and more");
    let rules = BTreeSet::from([rule("127.0.0.1", port, NetOperation::Connect)]);
    let mut io = real(rules.clone());
    let chunks = exchange(&mut io, port, b"ping");
    assert_eq!(server.join().unwrap(), b"ping");
    assert_eq!(chunks.concat(), b"pong and more");
    (rules, port, io.tape().to_vec(), chunks)
}

pub fn replay(rules: &BTreeSet<NetRule>, tape: Vec<Event>) -> SimIo {
    let mut sim = SimIo::new(Tree::new(Default::default()).unwrap(), tape);
    super::host(&mut sim);
    sim.set_net_rules(rules.clone());
    sim
}
