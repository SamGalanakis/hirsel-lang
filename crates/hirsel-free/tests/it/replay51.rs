//! Replay storage (replay-storage-51): every stream is checked before any task runs, a cursor
//! holds decoded only the records of its active ops, and only an observed replay keeps events.
//! The vectors follow serde_json's raw values (validate the borrowed bytes, then own them;
//! owned subvalues of one array), adapted to task logs.

use hirsel_free::phase::{Phase, Progress};
use hirsel_free::projection::Observed;
use hirsel_free::record::{Encoder, Record, put_varint};
use hirsel_free::replay::Shared;
use hirsel_free::tape::{self, End, Tape};
use std::collections::BTreeMap;

fn done(op: u64) -> Progress {
    Progress::new(op, Phase::Done)
}

fn log(records: &[Record]) -> Vec<u8> {
    let mut encoder = Encoder::default();
    let mut out = Vec::new();
    for record in records {
        encoder.encode(record, &mut out);
    }
    out
}

fn loads(ops: std::ops::RangeInclusive<u64>) -> Vec<Record> {
    ops.map(|op| Record::Load {
        op,
        bits: u128::from(op) << 70,
    })
    .collect()
}

/// A terminal tape of `tasks` (path, log, cut) naming one host key.
fn tape_of(tasks: Vec<(Vec<u32>, Vec<u8>, Progress)>) -> Tape {
    let mut tape = Tape {
        keys: vec!["stdout".into()],
        end: Some(End {
            outcome: "returned".into(),
            task: None,
        }),
        ..Tape::default()
    };
    for (path, bytes, cut) in tasks {
        if !bytes.is_empty() {
            tape.logs.insert(path.clone(), bytes);
        }
        tape.cut.insert(path, cut);
    }
    tape
}

fn refusal(tape: &Tape) -> String {
    let refusals = [true, false].map(|observe| match Shared::open(tape, observe) {
        Ok(_) => panic!("a malformed tape is admitted (observe {observe})"),
        Err(e) => e.0,
    });
    assert_eq!(
        refusals[0], refusals[1],
        "observation does not change refusal"
    );
    refusals[0].clone()
}

#[test]
fn replay51_malformed_later_streams_refuse_before_any_task_runs() {
    let good = |path: Vec<u32>| (path, log(&loads(1..=3)), done(3));
    // A later task (in path order) whose last record is cut inside a varint, or has no kind:
    // the whole tape is refused, so no earlier task's cursor ever exists.
    let mut corrupt = log(&loads(1..=3));
    *corrupt.last_mut().unwrap() |= 0x80;
    let mut bad_kind = log(&loads(1..=2));
    bad_kind.extend([0x0c, 0x01]);
    for (bytes, expect) in [
        (corrupt, "truncated record"),
        (bad_kind, "bad record kind 12"),
    ] {
        let tape = tape_of(vec![good(vec![]), good(vec![0]), (vec![1], bytes, done(3))]);
        let error = refusal(&tape);
        assert!(
            error.starts_with("task [1]") && error.contains(expect),
            "{error}"
        );
    }
    // A record truncated inside its 128-bit field.
    let mut truncated = log(&loads(1..=1));
    truncated.pop();
    let error = refusal(&tape_of(vec![(vec![], truncated, done(1))]));
    assert!(error.contains("truncated record"), "{error}");
    // A key the tape does not name, and an epoch past the key's previous one.
    let key = |key, gap| Record::Key {
        op: 1,
        key,
        shared: false,
        gap,
    };
    let error = refusal(&tape_of(vec![(vec![], log(&[key(5, 1)]), done(1))]));
    assert_eq!(error, "unnamed key 5");
    // A record past its task's cut.
    let error = refusal(&tape_of(vec![(vec![], log(&loads(1..=4)), done(3))]));
    assert!(error.contains("records op 4 past its cut"), "{error}");
    // A stream that does not decode is refused as such, even after a record past its cut.
    let mut both = log(&loads(1..=4));
    both.push(0xff);
    let error = refusal(&tape_of(vec![(vec![], both, done(3))]));
    assert!(error.contains("truncated record"), "{error}");
    // Records of a task outside the cut.
    let mut orphan = tape_of(vec![good(vec![])]);
    orphan.logs.insert(vec![7], log(&loads(1..=1)));
    assert!(refusal(&orphan).contains("no progress in the cut"));

    // An invalid phase in a cut is refused by the tape reader.
    let mut payload = Vec::new();
    put_varint(&mut payload, 0);
    put_varint(&mut payload, 1);
    put_varint(&mut payload, 0);
    put_varint(&mut payload, u128::from(3u64 << 24 | 7 << 21));
    put_varint(&mut payload, 0);
    let mut body = vec![2u8];
    body.extend(payload);
    let mut bytes = tape::header(b"binding");
    bytes.extend((body.len() as u32).to_le_bytes());
    bytes.extend(tape::crc32c(&body).to_le_bytes());
    bytes.extend(body);
    assert_eq!(tape::parse(&bytes).unwrap_err(), "bad progress");
}

#[test]
fn replay51_unstarted_and_absent_children_hold_nothing_decoded() {
    let tape = tape_of(vec![
        (vec![], log(&loads(1..=2)), done(2)),
        (vec![0], Vec::new(), Progress::START),
    ]);
    for observe in [true, false] {
        let shared = Shared::open(&tape, observe).unwrap();
        for child in [&[0][..], &[1]] {
            assert!(!shared.starts(child));
            let cursor = shared.cursor(child);
            assert!(!cursor.starts() && !cursor.admits(done(1)));
            assert_eq!(cursor.cancel_outcome(1), None);
            assert_eq!(cursor.await_any_member(1), None);
            assert_eq!(cursor.decoded(), 0, "child {child:?}");
        }
        let mut root = shared.cursor(&[]);
        assert_eq!(
            root.decoded(),
            0,
            "a cursor decodes nothing before it is asked"
        );
        for op in 1..=2 {
            root.load(op).unwrap();
            root.publish(done(op)).unwrap();
        }
        let projection = shared.finish([(&root, done(2))]).unwrap();
        assert_eq!(projection.is_some(), observe);
    }
}

#[test]
fn replay51_windows_bound_decoded_records_at_1x_2x_4x() {
    for scale in [1u64, 2, 4] {
        let n = 512 * scale;
        let tape = tape_of(vec![(vec![], log(&loads(1..=n)), done(n))]);
        for observe in [true, false] {
            let shared = Shared::open(&tape, observe).unwrap();
            let mut cursor = shared.cursor(&[]);
            let mut widest = 0;
            for op in 1..=n {
                assert_eq!(cursor.load(op).unwrap(), u128::from(op) << 70);
                widest = widest.max(cursor.decoded());
                cursor.publish(done(op)).unwrap();
            }
            // The current op's record and at most one record of a later op.
            assert!(widest <= 2, "{n} records: {widest} decoded at once");
            assert_eq!(cursor.decoded(), 0);
            let projection = shared.finish([(&cursor, done(n))]).unwrap();
            match projection {
                Some(projection) => {
                    assert_eq!(projection.observed.len() as u64, n);
                    assert_eq!(
                        projection.observed[&(vec![], n)],
                        [Observed::Load(u128::from(n) << 70)]
                    );
                }
                None => assert!(!observe),
            }
        }
    }
}

#[test]
fn replay51_an_earlier_record_stays_decoded_until_consumed() {
    // Op 1's thaw is taken after later ops (as an engine applies a sub-event after its op
    // published): the window keeps it, and every record is still consumed once.
    let mut records = vec![Record::Thaw { op: 1, ok: true }];
    records.extend(loads(2..=8));
    let tape = tape_of(vec![(vec![], log(&records), done(8))]);
    let shared = Shared::open(&tape, false).unwrap();
    let mut cursor = shared.cursor(&[]);
    assert_eq!(cursor.thaw_outcome(1), Some(true));
    for op in 2..=8 {
        cursor.load(op).unwrap();
    }
    // The window runs from the unconsumed thaw through the last record decoded.
    assert_eq!(cursor.decoded(), 8);
    let error = shared.finish([(&cursor, done(8))]).unwrap_err();
    assert!(error.0.contains("Thaw { op: 1, ok: true }"), "{error}");
    assert!(cursor.thaw(1).unwrap());
    assert_eq!(cursor.decoded(), 0);
    // A second finish of the same cursors sees every record consumed.
    assert_eq!(shared.finish([(&cursor, done(8))]).unwrap(), None);
}

#[test]
fn replay51_streams_retain_checked_bytes_not_records() {
    // Many tasks with many records each: admission keeps their bytes, and cursors are built
    // without decoding until asked.
    let tasks: BTreeMap<Vec<u32>, u64> = (0..16u32).map(|i| (vec![i], 64)).collect();
    let mut all = vec![(vec![], log(&loads(1..=1)), done(1))];
    all.extend(
        tasks
            .iter()
            .map(|(path, &n)| (path.clone(), log(&loads(1..=n)), done(n))),
    );
    let tape = tape_of(all);
    let shared = Shared::open(&tape, false).unwrap();
    let cursors: Vec<_> = tasks.keys().map(|path| shared.cursor(path)).collect();
    assert!(cursors.iter().all(|cursor| cursor.decoded() == 0));
}
