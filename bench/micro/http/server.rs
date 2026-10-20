//! Single thread, one persistent connection, 1024 fixed HTTP/1.1 requests.
//! Based on std/net/tcp.rs:904-915 (TcpListener::accept example).
//! The fixed request recognizer is a TCP cost floor, not a general HTTP parser.
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let listener = TcpListener::bind("127.0.0.1:18476").unwrap();
    let (mut stream, _) = listener.accept().unwrap();
    stream.set_nodelay(true).unwrap();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 16384];
    for _ in 0..1024 {
        loop {
            if let Some(end) = pending.windows(4).position(|s| s == b"\r\n\r\n") {
                assert!(pending.starts_with(b"GET / HTTP/1.1\r\n"));
                pending.drain(..end + 4);
                break;
            }
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            pending.extend_from_slice(&buffer[..n]);
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 12\r\n\r\nHello, world").unwrap();
    }
    assert_eq!(stream.read(&mut buffer).unwrap(), 0);
    println!("1024");
}
