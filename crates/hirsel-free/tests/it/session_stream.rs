use hirsel_free::{
    phase::Phase,
    record::Record,
    recorder::TapeSink,
    session::{Claim, Recording, Session},
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Written {
    bytes: Vec<u8>,
    barriers: usize,
    fail: Option<usize>,
}
struct Stream(Arc<Mutex<Written>>);
impl TapeSink for Stream {
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn barrier(&mut self) -> std::io::Result<()> {
        let mut written = self.0.lock().unwrap();
        written.barriers += 1;
        if written.fail == Some(written.barriers) {
            return Err(std::io::Error::other("barrier failed"));
        }
        Ok(())
    }
}
fn start(fail: Option<usize>) -> (Session, Arc<Mutex<Written>>) {
    let written = Arc::new(Mutex::new(Written {
        fail,
        ..Default::default()
    }));
    let mut session = Session::record_to(
        b"stream",
        Recording {
            durable: true,
            ..Default::default()
        },
        Box::new(Stream(written.clone())),
    )
    .unwrap();
    assert!(session.start(&[]));
    session.begin(&[]);
    (session, written)
}

#[test]
fn streaming_durable_phases_replay_with_the_complete_live_projection() {
    let (mut live, written) = start(None);
    let keys = [("stdout", false)];
    let mut call = live.record_host_prepare(&[], &keys).unwrap();
    live.record_host_observation(&[], b"request").unwrap();
    live.record_host_entered(&[]).unwrap();
    {
        let written = written.lock().unwrap();
        assert_eq!(written.barriers, 2);
        let tape = hirsel_free::tape::parse(&written.bytes).unwrap();
        assert!(
            hirsel_free::record::decode(&tape.logs[&vec![]])
                .unwrap()
                .iter()
                .any(|record| matches!(record, Record::Prepared { .. }))
        );
    }
    assert_eq!(live.host_claim(&[], &mut call).unwrap(), Claim::Call);
    live.record_host_ended(&[], &call, 0, b"result", Some(("stdout", b"hi")), false)
        .unwrap();
    live.record_host_complete(&[], &call, b"result").unwrap();
    assert_eq!(written.lock().unwrap().barriers, 3);
    let expected = live
        .finish_recording(|sweep| {
            assert!(sweep.withdrawn.is_empty() && sweep.in_doubt.is_empty());
            "returned".into()
        })
        .unwrap();
    let mut replay = Session::replay(&written.lock().unwrap().bytes).unwrap();
    replay.start(&[]);
    replay.begin(&[]);
    let positions = replay.replay_host_positions(&[], 0, &keys).unwrap();
    replay.replay_host_request(&[], b"request").unwrap();
    replay.publish(&[], Phase::Entered).unwrap();
    replay.publish(&[], Phase::Claimed).unwrap();
    assert_eq!(
        replay.replay_host_observation(&[], &positions).unwrap(),
        b"result"
    );
    replay
        .replay_host_output(&[], &positions, "stdout", b"hi")
        .unwrap();
    replay.publish(&[], Phase::Ended(0)).unwrap();
    assert_eq!(replay.replay_host_completion(&[]).unwrap(), b"result");
    replay.publish(&[], Phase::Done).unwrap();
    assert_eq!(replay.finish().unwrap(), expected);
}

#[test]
fn failed_prepared_barrier_prevents_claim_and_failed_completion_prevents_delivery() {
    for fail in [2, 3] {
        let (mut live, _) = start(Some(fail));
        let mut call = live.record_host_prepare(&[], &[]).unwrap();
        let entered = live.record_host_entered(&[]);
        if fail == 2 {
            assert!(entered.unwrap_err().0.contains("barrier failed"));
            assert_eq!(live.host_claim(&[], &mut call).unwrap(), Claim::Final);
        } else {
            entered.unwrap();
            assert_eq!(live.host_claim(&[], &mut call).unwrap(), Claim::Call);
            live.record_host_ended(&[], &call, 0, b"result", None, false)
                .unwrap();
            assert!(
                live.record_host_complete(&[], &call, b"result")
                    .unwrap_err()
                    .0
                    .contains("barrier failed")
            );
            assert!(live.stopped());
        }
    }
}
