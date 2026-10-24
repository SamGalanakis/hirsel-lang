use super::*;
use crate::{TimedChildRun, TimedChildStep as S};
fn timed(steps: Vec<S>) -> ScriptChild {
    ScriptChild::new_timed(
        TimedChildRun {
            program: "/bin/cat".into(),
            args: vec![],
            steps,
        },
        0,
    )
}
#[derive(Clone)]
enum Action {
    Poll(ScriptRead),
    At(u64),
    Write(&'static [u8]),
    Close,
    Kill,
    Status(Option<ExitStatus>),
}
struct Row {
    id: &'static str,
    steps: Vec<S>,
    actions: Vec<Action>,
    gate: Option<u64>,
}
#[test]
fn timed_declarative_machine_rows() {
    use Action::*;
    let rows = [
        Row {
            id: "T01",
            steps: vec![S::At(5), S::Print(b"x".to_vec())],
            actions: vec![Poll(ScriptRead::Waiting)],
            gate: Some(5),
        },
        Row {
            id: "T02",
            steps: vec![
                S::At(5),
                S::Print(b"x".to_vec()),
                S::PrintErr(b"E".to_vec()),
            ],
            actions: vec![At(5), Poll(ScriptRead::Bytes(b"x".to_vec()))],
            gate: None,
        },
        Row {
            id: "T03",
            steps: vec![S::At(5)],
            actions: vec![
                Poll(ScriptRead::Waiting),
                Poll(ScriptRead::Waiting),
                Poll(ScriptRead::Waiting),
            ],
            gate: Some(5),
        },
        Row {
            id: "T05",
            steps: vec![S::At(4), S::Exit(ExitStatus::Exited(7))],
            actions: vec![At(4), Status(Some(ExitStatus::Exited(7)))],
            gate: None,
        },
        Row {
            id: "T08-before",
            steps: vec![S::At(5), S::Exit(ExitStatus::Exited(7))],
            actions: vec![Kill, At(5), Status(Some(ExitStatus::Signaled(9)))],
            gate: None,
        },
        Row {
            id: "T08-after",
            steps: vec![S::At(5), S::Exit(ExitStatus::Exited(7))],
            actions: vec![At(5), Kill, Status(Some(ExitStatus::Exited(7)))],
            gate: None,
        },
        Row {
            id: "T10",
            steps: vec![S::Expect(b"ab".to_vec()), S::At(5), S::Print(b"x".to_vec())],
            actions: vec![
                Write(b"a"),
                At(7),
                Write(b"b"),
                Poll(ScriptRead::Bytes(b"x".to_vec())),
            ],
            gate: None,
        },
        Row {
            id: "T11",
            steps: vec![S::At(5), S::Expect(b"ab".to_vec()), S::Print(b"x".to_vec())],
            actions: vec![
                Write(b"a"),
                Write(b"bextra"),
                Poll(ScriptRead::Waiting),
                At(5),
                Poll(ScriptRead::Bytes(b"x".to_vec())),
            ],
            gate: None,
        },
        Row {
            id: "T14",
            steps: vec![S::At(5)],
            actions: vec![Kill, Write(b""), Status(Some(ExitStatus::Signaled(9)))],
            gate: None,
        },
        Row {
            id: "T15",
            steps: vec![
                S::Print(b"ab".to_vec()),
                S::At(5),
                S::Print(b"later".to_vec()),
            ],
            actions: vec![
                Kill,
                Poll(ScriptRead::Bytes(b"ab".to_vec())),
                Poll(ScriptRead::Bytes(vec![])),
            ],
            gate: None,
        },
        Row {
            id: "T17",
            steps: vec![S::At(5), S::Expect(b"ab".to_vec())],
            actions: vec![Close, Status(None)],
            gate: Some(5),
        },
        Row {
            id: "T20",
            steps: vec![
                S::At(5),
                S::Print(b"a".to_vec()),
                S::At(5),
                S::Print(b"b".to_vec()),
                S::At(7),
            ],
            actions: vec![
                At(5),
                Poll(ScriptRead::Bytes(b"a".to_vec())),
                Poll(ScriptRead::Bytes(b"b".to_vec())),
            ],
            gate: Some(7),
        },
        Row {
            id: "T22",
            steps: vec![S::Expect(b"x".to_vec()), S::At(5)],
            actions: vec![Poll(ScriptRead::Waiting), Status(None)],
            gate: None,
        },
    ];
    for row in rows {
        let mut c = timed(row.steps);
        for action in row.actions {
            match action {
                Poll(expected) => {
                    assert_eq!(c.read(ChildStream::Stdout, 100), expected, "{}", row.id)
                }
                At(now) => assert_eq!(c.settle_at(now), None, "{}", row.id),
                Write(bytes) => assert_eq!(c.write(bytes), Ok(Ok(())), "{}", row.id),
                Close => assert_eq!(c.close_stdin(), None, "{}", row.id),
                Kill => c.kill(Signal::Kill),
                Status(status) => assert_eq!(c.status(), status, "{}", row.id),
            }
        }
        assert_eq!(c.next_at(), row.gate, "{}", row.id);
    }
}
#[test]
fn timed_delayed_close_and_short_contradiction() {
    let mut c = timed(vec![S::At(5), S::Expect(b"ab".to_vec())]);
    assert_eq!(c.write(b"a"), Ok(Ok(())));
    assert_eq!(c.close_stdin(), None);
    assert_eq!(c.settle_at(4), None);
    assert_eq!(c.settle_at(5),Some("test failure: child /bin/cat expected b\"ab\" but the program closed its input after writing b\"a\"\n".into()));
    assert_eq!(c.close_stdin(), None);
    let mut c = timed(vec![S::At(5), S::Expect(b"ab".to_vec())]);
    assert_eq!(c.write(b"z"), Ok(Ok(())));
    assert_eq!(c.settle_at(5), None);
    assert_eq!(
        c.write(b"q"),
        Err("test failure: child /bin/cat expected b\"ab\" but the program wrote b\"zq\"\n".into())
    );
}
#[test]
fn timed_pipeline_feed_precedes_close_and_later_close_reports() {
    let run = |program: &str, steps| TimedChildRun {
        program: program.into(),
        args: vec![],
        steps,
    };
    let (mut g, report) = ScriptGroup::new_timed(
        vec![
            run("/fixture/a", vec![S::At(5), S::Print(b"ab".to_vec())]),
            run(
                "/fixture/b",
                vec![
                    S::Expect(b"ab".to_vec()),
                    S::Print(b"cd".to_vec()),
                    S::Exit(ExitStatus::Exited(3)),
                ],
            ),
        ],
        2,
        0,
    );
    assert_eq!(report, None);
    assert_eq!(g.read(100), ScriptRead::Waiting);
    assert_eq!(g.settle_at(5), None);
    assert_eq!(g.read(100), ScriptRead::Bytes(b"cd".to_vec()));
    assert_eq!(
        g.output().unwrap().statuses,
        vec![ExitStatus::Exited(0), ExitStatus::Exited(3)]
    );
    let (mut g, report) = ScriptGroup::new_timed(
        vec![
            run("/fixture/a", vec![S::At(3), S::Print(b"a".to_vec())]),
            run("/fixture/b", vec![S::At(5), S::Expect(b"ab".to_vec())]),
        ],
        2,
        0,
    );
    assert_eq!(report, None);
    assert_eq!(g.settle_at(3), None);
    assert_eq!(g.next_at(), Some(5));
    assert_eq!(g.settle_at(5),Some("test failure: child /fixture/b expected b\"ab\" but its input stage /fixture/a closed its output after writing b\"a\"\n".into()));
}

#[test]
fn timed_pipeline_mismatch_keeps_writer_before_eof() {
    let run = |program: &str, steps| TimedChildRun {
        program: program.into(),
        args: vec![],
        steps,
    };
    let (mut group, report) = ScriptGroup::new_timed(
        vec![
            run(
                "/fixture/a",
                vec![
                    S::At(3),
                    S::Print(b"zq".to_vec()),
                    S::At(7),
                    S::Exit(ExitStatus::Exited(0)),
                ],
            ),
            run("/fixture/b", vec![S::At(5), S::Expect(b"ab".to_vec())]),
        ],
        2,
        0,
    );
    assert_eq!(report, None);
    assert_eq!(group.settle_at(3), None);
    assert_eq!(group.next_at(), Some(5));
    assert_eq!(group.settle_at(5), Some("test failure: child /fixture/b expected b\"ab\" but its input stage /fixture/a wrote b\"zq\"\n".into()));
}
#[test]
fn timed_p01_p02_declarative_poll_checkpoints() {
    // Adapted from Tokio time_pause.rs:82-96; the model supplies checkpoints.
    // Production progress still belongs exclusively to runtime quiescence.
    for (id, checkpoints) in [
        ("P01", vec![1_000_000, 1_000_000, 101_000_000]),
        ("P02", vec![300_999_999, 301_000_000]),
    ] {
        let steps = vec![
            S::At(301_000_000),
            S::Print(b"x".to_vec()),
            S::Exit(ExitStatus::Exited(0)),
        ];
        let mut child = ScriptChild::new_timed(
            TimedChildRun {
                program: "/bin/cat".into(),
                args: vec![],
                steps,
            },
            1_000_000,
        );
        for tick in checkpoints {
            assert_eq!(child.settle_at(tick), None, "{id}");
            if tick < 301_000_000 {
                for _ in 0..3 {
                    assert_eq!(
                        child.read(ChildStream::Stdout, 1),
                        ScriptRead::Waiting,
                        "{id}"
                    );
                    assert_eq!(child.next_at(), Some(301_000_000), "{id}");
                }
            } else {
                assert_eq!(
                    child.read(ChildStream::Stdout, 1),
                    ScriptRead::Bytes(b"x".to_vec()),
                    "{id}"
                );
                assert_eq!(child.next_at(), None);
            }
        }
    }
}
