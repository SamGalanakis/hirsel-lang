//! Rust std/src/net/tcp/tests.rs::timeouts, test_timeout_zero_duration,
//! test_read_with_timeout and shutdown_smoke, adapted to frozen scripted peers.
use super::*;

fn stream(steps: Vec<NetStep>) -> ScriptStream {
    ScriptStream::new(1, "example.com".into(), 80, PeerTls::Plain, false, steps)
}
#[test]
fn rust_timeout_defaults_zero_and_reset_vectors() {
    let mut s = stream(vec![NetStep::Stall]);
    assert_eq!(s.read_timeout, None);
    assert_eq!(s.write_timeout, None);
    assert_eq!(
        timeout(&variant(1, vec![u64_value(0)])).unwrap(),
        Err(NetFault::InvalidInput)
    );
    s.read_timeout = Some(15410);
    s.write_timeout = Some(15410);
    assert_eq!(s.read(10), Err(NetFault::TimedOut));
    s.read_timeout = None;
    assert_eq!(s.read(10), Err(NetFault::Blocked));
}
#[test]
fn rust_read_with_timeout_delivers_available_bytes_first() {
    let mut s = stream(vec![NetStep::Send(b"hello world".to_vec()), NetStep::Stall]);
    s.read_timeout = Some(1000);
    assert_eq!(s.read(11), Ok(b"hello world".to_vec()));
    assert_eq!(s.read(11), Err(NetFault::TimedOut));
    assert_eq!(s.read(11), Err(NetFault::TimedOut));
}
#[test]
fn rust_shutdown_smoke_half_close_preserves_reads() {
    let mut s = stream(vec![NetStep::Send(b"reply".to_vec())]);
    s.write_closed = true;
    assert_eq!(s.write(b"request"), Err(NetFault::BrokenPipe));
    assert_eq!(s.read(10), Ok(b"reply".to_vec()));
    s.read_closed = true;
    assert_eq!(s.read(10), Ok(vec![]));
}
#[test]
fn script_chunks_empty_sends_and_implicit_fin() {
    let mut s = stream(vec![
        NetStep::Send(vec![]),
        NetStep::Send(b"abc".to_vec()),
        NetStep::Send(b"de".to_vec()),
    ]);
    assert_eq!(s.read(2), Ok(b"ab".to_vec()));
    assert_eq!(s.read(10), Ok(b"c".to_vec()));
    assert_eq!(s.read(10), Ok(b"de".to_vec()));
    assert_eq!(s.read(10), Ok(vec![]));
    assert_eq!(s.read(10), Ok(vec![]));
}
#[test]
fn expectations_accumulate_across_writes_and_before_reads() {
    let mut s = stream(vec![
        NetStep::Expect(b"GET".to_vec()),
        NetStep::Send(b"ok".to_vec()),
        NetStep::Expect(b"end".to_vec()),
        NetStep::Send(b"bye".to_vec()),
    ]);
    assert_eq!(s.write(b"GE"), Ok(2));
    assert_eq!(s.read(10), Err(NetFault::Blocked));
    assert_eq!(s.write(b"Tend"), Ok(4));
    assert_eq!(s.read(10), Ok(b"ok".to_vec()));
    assert_eq!(s.read(10), Ok(b"bye".to_vec()));
}
#[test]
fn mismatch_and_explicit_reset_persist() {
    let mut s = stream(vec![
        NetStep::Expect(b"yes".to_vec()),
        NetStep::Send(b"ok".to_vec()),
    ]);
    assert_eq!(s.write(b"no!"), Ok(3));
    assert_eq!(s.read(10), Err(NetFault::ConnectionReset));
    assert_eq!(s.write(b"yes"), Err(NetFault::ConnectionReset));
    let mut s = stream(vec![NetStep::Reset]);
    assert_eq!(s.read(10), Err(NetFault::ConnectionReset));
    assert_eq!(s.write(b""), Err(NetFault::ConnectionReset));
}
