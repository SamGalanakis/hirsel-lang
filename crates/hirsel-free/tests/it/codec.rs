//! Records, tapes, progress, key epochs and pending releases.

use hirsel_free::keys::KeyEpochs;
use hirsel_free::pending::Pending;
use hirsel_free::phase::{Phase, Progress};
use hirsel_free::record::{Encoder, ObjectId, Record, decode};
use hirsel_free::recorder::Recorder;
use hirsel_free::tape::{self, End, TaskChunk};

#[test]
fn records_round_trip_and_contended_events_stay_small() {
    let records = vec![
        Record::Gap {
            op: 1,
            sub: 0,
            gap: 1,
        },
        Record::Gap {
            op: 3,
            sub: 2,
            gap: 14,
        },
        Record::Gap {
            op: 3,
            sub: 0,
            gap: 15,
        },
        Record::Gap {
            op: 900,
            sub: 0,
            gap: u64::MAX,
        },
        Record::Load {
            op: 901,
            bits: u128::MAX,
        },
        Record::AwaitAny { op: 901, index: 3 },
        Record::Cancel {
            op: 902,
            check: 0,
            cause: None,
        },
        Record::Cancel {
            op: 903,
            check: u64::MAX,
            cause: Some(2),
        },
        Record::Expired { op: 904 },
        Record::Thaw { op: 905, ok: true },
        Record::Thaw { op: 905, ok: false },
        Record::Prepared { op: 906, id: 77 },
        Record::Key {
            op: 906,
            key: 3,
            shared: true,
            gap: 0,
        },
        Record::Key {
            op: 906,
            key: 4,
            shared: false,
            gap: 99,
        },
        Record::Observation {
            op: 907,
            bytes: b"exit 0".to_vec(),
        },
        Record::Completion {
            op: 908,
            bytes: vec![],
        },
        Record::Blocked { op: 909 },
        Record::Ready { op: 909 },
    ];
    let mut encoder = Encoder::default();
    let mut bytes = Vec::new();
    for record in &records {
        encoder.encode(record, &mut bytes);
    }
    assert_eq!(decode(&bytes).unwrap(), records);
    // A contended object op with a small gap, one op after the task's previous record.
    let mut one = Vec::new();
    Encoder::default().encode(
        &Record::Gap {
            op: 1,
            sub: 0,
            gap: 3,
        },
        &mut one,
    );
    assert_eq!(one.len(), 3);
    assert!(
        decode(&[0x0f]).is_err() && decode(&[0x00, 0x01, 0x00]).is_err(),
        "gap 0 never decodes"
    );
}

#[test]
fn uncontended_object_ops_record_nothing_and_fresh_objects_start_at_version_one() {
    let mut recorder = Recorder::new(Vec::new(), b"", false).unwrap();
    let (a, b) = (vec![], vec![0]);
    recorder.start(a.clone()).unwrap();
    recorder.start(b.clone()).unwrap();
    let object = ObjectId::Made {
        path: a.clone(),
        op: 1,
    };
    recorder.create(&a, object.clone());
    recorder.publish(&a, Progress::new(1, Phase::Done)).unwrap();
    // The creator's first touch: p = 1, version 1, gap 0.
    assert_eq!(recorder.touch(&a, 2, 0, &object, "send"), 0);
    // Another task's first touch after it: p = 1 by default, version 2, gap 1.
    assert_eq!(recorder.touch(&b, 1, 0, &object, "send"), 1);
    assert_eq!(recorder.touch(&a, 3, 0, &object, "send"), 1);
    assert_eq!(recorder.touch(&a, 4, 0, &object, "send"), 0);
    // A fresh object whose first touch is a non-creator's: gap 0 again.
    let fresh = ObjectId::Made {
        path: a.clone(),
        op: 5,
    };
    recorder.create(&a, fresh.clone());
    assert_eq!(recorder.touch(&b, 2, 0, &fresh, "send"), 0);
    for _ in 0..1000 {
        recorder.touch(&b, 3, 0, &fresh, "send");
    }
    recorder.publish(&a, Progress::new(5, Phase::Done)).unwrap();
    recorder.publish(&b, Progress::new(3, Phase::Done)).unwrap();
    recorder.stop("returned", None).unwrap();
    let tape = tape::parse(&recorder.into_sink()).unwrap();
    // Only the two nonzero gaps are on the tape.
    assert_eq!(
        decode(&tape.logs[&a]).unwrap(),
        vec![Record::Gap {
            op: 3,
            sub: 0,
            gap: 1
        }]
    );
    assert_eq!(
        decode(&tape.logs[&b]).unwrap(),
        vec![Record::Gap {
            op: 1,
            sub: 0,
            gap: 1
        }]
    );
}

#[test]
fn progress_orders_phases_and_packs_into_one_word() {
    let op = |phase| Progress::new(7, phase);
    let request = [
        Phase::Entered,
        Phase::Claimed,
        Phase::Ended(0),
        Phase::Attempt(1),
        Phase::Ended(1),
        Phase::Attempt(2),
        Phase::Done,
    ];
    for pair in request.windows(2) {
        assert!(
            op(pair[0]) < op(pair[1]),
            "{} < {}",
            op(pair[0]),
            op(pair[1])
        );
    }
    let end = [
        Phase::Entered,
        Phase::Cleanup(1),
        Phase::Cleanup(2),
        Phase::Done,
    ];
    for pair in end.windows(2) {
        assert!(op(pair[0]) < op(pair[1]));
    }
    assert!(Progress::new(6, Phase::Done) < op(Phase::Entered));
    assert!(Progress::START < Progress::new(1, Phase::Entered));
    for phase in request.into_iter().chain(end) {
        let p = Progress::new(hirsel_free::phase::MAX_OP, phase);
        assert_eq!(Progress::unpack(p.pack()), Some(p));
    }
    assert!(
        op(Phase::Done).admits(op(Phase::Entered)) && !op(Phase::Entered).admits(op(Phase::Done))
    );
}

#[test]
fn tapes_keep_whole_cuts_and_refuse_damage_before_the_end() {
    let chunk = |op| TaskChunk {
        path: vec![1, 2],
        progress: Progress::new(op, Phase::Done),
        bytes: vec![op as u8],
    };
    let mut bytes = tape::header(b"binding");
    let header = bytes.len();
    bytes.extend(tape::flush(&["out".into()], &[chunk(1)]));
    let flushed = bytes.len();
    bytes.extend(tape::stop(
        &[],
        &[chunk(2)],
        &End {
            outcome: "returned".into(),
            task: Some(vec![1]),
        },
    ));
    let whole = tape::parse(&bytes).unwrap();
    assert_eq!(whole.binding, b"binding");
    assert_eq!(whole.keys, vec!["out".to_string()]);
    assert_eq!(whole.logs[&vec![1, 2]], vec![1, 2]);
    assert_eq!(whole.cut[&vec![1, 2]], Progress::new(2, Phase::Done));
    assert_eq!(whole.end.as_ref().unwrap().task, Some(vec![1]));
    for torn in [flushed + 3, bytes.len() - 1] {
        let tape = tape::parse(&bytes[..torn]).unwrap();
        assert!(tape.torn && tape.end.is_none());
        assert_eq!(tape.cut[&vec![1, 2]], Progress::new(1, Phase::Done));
    }
    assert!(tape::parse(&bytes[..header]).unwrap().cut.is_empty());
    let mut damaged = bytes.clone();
    damaged[header + 9] ^= 1;
    assert!(
        tape::parse(&damaged).is_err(),
        "a damaged flush chunk followed by the stop chunk"
    );
    let mut tail = bytes.clone();
    *tail.last_mut().unwrap() ^= 1;
    assert!(tape::parse(&tail).unwrap().torn);
}

#[test]
fn key_epochs_never_let_a_shared_transition_overtake_an_exclusive_one() {
    let mut key = KeyEpochs::default();
    let (r1, r2) = (key.take(true), key.take(true));
    assert_eq!(
        (r1, r2),
        (1, 1),
        "consecutive shared transitions share an epoch"
    );
    let w = key.take(false);
    let r3 = key.take(true);
    assert_eq!(
        (w, r3),
        (2, 3),
        "a shared transition after a queued exclusive one waits for it"
    );
    assert!(key.ready(r1) && key.ready(r2) && !key.ready(w));
    key.end(r1);
    assert!(
        !key.ready(w),
        "an exclusive transition waits for the whole shared epoch"
    );
    key.end(r2);
    assert!(key.ready(w) && !key.ready(r3));
    key.end(w);
    assert!(key.ready(r3));
    let mut replayed = KeyEpochs::default();
    replayed.member(1, true).unwrap();
    replayed.member(1, true).unwrap();
    replayed.member(2, false).unwrap();
    assert!(replayed.member(2, false).is_err() && replayed.member(1, false).is_err());
}

#[test]
fn pending_releases_coalesce_and_dups_cancel_them() {
    let mut pending = Pending::default();
    pending.defer("s");
    pending.defer("t");
    pending.defer("s");
    assert!(pending.dup(&"t") && !pending.dup(&"u"));
    assert_eq!(pending.take(), vec![("s", 2)]);
    assert!(pending.is_empty());
}

#[test]
fn cancellation_changes_keep_their_check_across_host_retries_and_partial_tapes() {
    use hirsel_free::projection::{Event, Observed, Projection, Stamped};
    use hirsel_free::replay::Replayer;
    let checks = [
        (Progress::new(1, Phase::Entered), None),
        (Progress::new(1, Phase::Attempt(1)), None),
        (Progress::new(1, Phase::Attempt(2)), Some(3)),
        (Progress::new(1, Phase::Attempt(3)), Some(3)),
        (Progress::new(1, Phase::Attempt(4)), None),
        (Progress::new(1, Phase::Done), Some(5)),
        (Progress::new(2, Phase::Entered), Some(5)),
        (Progress::new(2, Phase::Done), None),
    ];
    let trace: Vec<_> = checks
        .iter()
        .map(|&(progress, cause)| Stamped {
            task: vec![],
            progress,
            event: Event::Observed(Observed::Cancel(cause)),
        })
        .collect();
    for concurrent in [false, true] {
        let mut recorder = Recorder::new(Vec::new(), b"cancel checks", false).unwrap();
        recorder.start(vec![]).unwrap();
        let parallel = hirsel_free::concurrent::Recorder::<Vec<u8>>::new(
            Vec::new(),
            b"cancel checks",
            1,
            false,
            true,
            None,
        )
        .unwrap();
        let mut worker = parallel.executor(0).unwrap();
        let mut task = parallel.root(&mut worker).unwrap();
        for &(progress, cause) in &checks {
            if concurrent {
                parallel
                    .phase(
                        &mut worker,
                        &mut task,
                        progress,
                        hirsel_free::record::RecordBound::records(1),
                        |writer| writer.cancel(cause),
                    )
                    .unwrap();
                parallel.flush().unwrap().unwrap();
            } else {
                recorder.cancel_check(&[], progress.op, cause);
                recorder.publish(&[], progress).unwrap();
                recorder.flush().unwrap();
            }
        }
        let bytes = if concurrent {
            parallel.stop("returned", None).unwrap().unwrap();
            drop(worker);
            parallel.into_sink()
        } else {
            recorder.stop("returned", None).unwrap().unwrap();
            recorder.into_sink()
        };
        // Every flush is a valid prefix, including immediately before and after
        // a changed retry. The same unchanged checks must remain byte-free.
        for length in 0..=bytes.len() {
            let Ok(tape) = tape::parse(&bytes[..length]) else {
                continue;
            };
            let Some(&cut) = tape.cut.get(&vec![]) else {
                continue;
            };
            let mut replay = Replayer::new(&tape).unwrap();
            for &(progress, cause) in &checks {
                if !cut.admits(progress) {
                    break;
                }
                assert_eq!(replay.cancel_outcome(&[], progress.op), cause);
                assert_eq!(
                    replay.cancel_outcome(&[], progress.op),
                    cause,
                    "peek is inert"
                );
                assert_eq!(replay.cancel_check(&[], progress.op).unwrap(), cause);
                replay.publish(&[], progress).unwrap();
            }
            let outcome = if tape.end.is_some() {
                "returned"
            } else {
                "truncated"
            };
            let expected = Projection::new(outcome, &tape.cut, &trace);
            assert_eq!(replay.finish(&tape.cut).unwrap(), expected);
        }
        let tape = tape::parse(&bytes).unwrap();
        let records = decode(&tape.logs[&vec![]]).unwrap();
        assert_eq!(records.len(), 4, "unchanged checks record nothing");
        assert!(matches!(
            records[0],
            Record::Cancel {
                op: 1,
                check: 2,
                cause: Some(3)
            }
        ));
        assert!(matches!(
            records[3],
            Record::Cancel {
                op: 2,
                check: 1,
                cause: None
            }
        ));
    }
}
