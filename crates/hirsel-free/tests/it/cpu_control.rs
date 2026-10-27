use hirsel_free::phase::Phase;
use hirsel_free::projection::Observed;
use hirsel_free::session::{Recording, Session};

#[test]
fn cpu_receipts_replay_all_request_fields_and_preserve_typed_projection() {
    let mut live = Session::record(b"cpu", Recording::default());
    live.start(&[]);
    for (width, action, count, outcome) in [(8, 0, 0, 0), (8, 2, 17, 6), (8, 1, 0, 0)] {
        live.begin(&[]);
        assert_eq!(
            live.cpu_control(&[], width, action, count, outcome),
            Ok(outcome)
        );
        live.publish(&[], Phase::Done).unwrap();
    }
    live.stop("returned", None);
    let report = live.report();
    let tape = hirsel_free::tape::parse(&report.tape).unwrap();
    let records = hirsel_free::record::decode(&tape.logs[&vec![]]).unwrap();
    assert_eq!(
        records[0],
        hirsel_free::record::Record::Observation {
            op: 1,
            bytes: vec![8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        }
    );
    assert_eq!(
        records[2],
        hirsel_free::record::Record::Observation {
            op: 2,
            bytes: vec![8, 0, 0, 0, 2, 0, 0, 0, 17, 0, 0, 0, 0, 0, 0, 0],
        }
    );
    assert_eq!(
        records[3],
        hirsel_free::record::Record::Load { op: 2, bits: 6 }
    );
    let expected = &report.cuts.last().unwrap().1;
    assert_eq!(
        expected.observed[&(vec![], 2)],
        vec![Observed::CpuControl {
            width: 8,
            action: 2,
            count: 17,
            outcome: 6
        }]
    );
    let mut replay = Session::replay(&report.tape).unwrap();
    replay.start(&[]);
    for (width, action, count, expected) in [(8, 0, 0, 0), (8, 2, 17, 6), (8, 1, 0, 0)] {
        replay.begin(&[]);
        assert_eq!(
            replay.cpu_control(&[], width, action, count, u32::MAX),
            Ok(expected)
        );
        replay.publish(&[], Phase::Done).unwrap();
    }
    assert_eq!(&replay.finish().unwrap(), expected);
    for (width, action, count) in [(4, 0, 0), (8, 4, 0), (8, 0, 1)] {
        let mut replay = Session::replay(&report.tape).unwrap();
        replay.start(&[]);
        replay.begin(&[]);
        assert!(
            replay
                .cpu_control(&[], width, action, count, 0)
                .unwrap_err()
                .0
                .contains("CPU control request differs")
        );
    }
}
