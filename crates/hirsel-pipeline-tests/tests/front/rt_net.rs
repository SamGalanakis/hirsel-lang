//! hirsel-rt's network test whose loopback recording the interpreter also replays
//! (`rt_replay`), beside the rest of them in `crates/hirsel-rt/tests/it/net.rs`.
use crate::rt_support::loopback::*;
use hirsel_rt::io::*;
use hirsel_rt::launch::render_tape;
#[test]
fn loopback_recording_replays_without_sockets() {
    let (rules, port, tape, chunks) = record_exchange();
    // The server is gone: replay must not touch the network at all.
    let text = render_tape(&tape).unwrap();
    assert!(text.starts_with("hirsel-io-tape 7\n"));
    let parsed = crate::rt_support::tape::events(&tape);
    assert_eq!(parsed, tape);
    let mut sim = replay(&rules, parsed);
    assert_eq!(exchange(&mut sim, port, b"ping"), chunks);
    sim.finish().unwrap();
    let initial = Tree::new(Default::default()).unwrap();
    crate::rt_replay::replay_interpreter(
        &tape,
        &initial,
        sim.tree(),
        &sim.handles().observed(),
        b"",
    );
    let mut altered = tape.clone();
    let bytes = altered
        .iter_mut()
        .find_map(|event| match &mut event.completion {
            Completion::NetBytes(Ok(bytes)) if !bytes.is_empty() => Some(bytes),
            _ => None,
        })
        .unwrap();
    bytes[0] ^= 1;
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::rt_replay::replay_interpreter_with_fixture(
                &altered,
                crate::rt_replay::ReplayFixture {
                    events: &tape,
                    pem_roots: &[],
                    tls_identities: &[],
                },
                &initial,
                sim.tree(),
                &sim.handles().observed(),
                b"",
                &std::collections::BTreeMap::from([(
                    String::new(),
                    hirsel_run_schema::Access::ReadWrite,
                )]),
            );
        }))
        .is_err(),
        "altered read bytes passed the independent interpreter comparison"
    );
}
