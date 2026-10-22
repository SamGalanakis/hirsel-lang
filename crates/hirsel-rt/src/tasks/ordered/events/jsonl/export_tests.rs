use super::*;
use crate::host_paths::{PinnedDirectory, RecorderDestination};

fn completed() -> Log {
    let mut log = Log::default();
    let mut end = super::tests::event(CanonicalOutcome::RootEnd {
        status: 0,
        result: None,
        failures: vec![],
    });
    end.item = CanonicalItem::Root;
    end.transition = Transition::End;
    log.canonical.push(end);
    log
}

#[test]
fn canonical_export_requires_terminal_and_complete_evidence_before_opening_destination() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing.jsonl");
    let missing = dir.path().join("missing.jsonl");
    std::fs::write(&existing, b"sentinel").unwrap();
    for path in [&existing, &missing] {
        for cause in 0..3 {
            let mut log = completed();
            match cause {
                0 => log.canonical.clear(),
                1 => log.canonical[0].outcome = CanonicalOutcome::Unit,
                2 => log.unsupported.push(Unsupported {
                    task: vec![],
                    ordinal: 0,
                    op: None,
                }),
                _ => unreachable!(),
            }
            let destination = RecorderDestination::pin(path).unwrap();
            assert!(
                Trace::completed(&log)
                    .map_err(str::to_owned)
                    .and_then(|trace| trace.publish(destination, &[], &[]))
                    .is_err()
            );
            assert_eq!(std::fs::read(&existing).unwrap(), b"sentinel");
            assert!(!missing.exists());
        }
    }
    let trace = Trace::completed(&completed()).unwrap();
    trace
        .publish(RecorderDestination::pin(&existing).unwrap(), &[], &[])
        .unwrap();
    assert_eq!(std::fs::read(&existing).unwrap(), trace.bytes());
    trace
        .publish(RecorderDestination::pin(&missing).unwrap(), &[], &[])
        .unwrap();
    assert_eq!(std::fs::read(&missing).unwrap(), trace.bytes());
}

#[test]
fn canonical_export_uses_physical_grant_and_sink_checks_before_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let grant = PinnedDirectory::open(dir.path()).unwrap();
    let file = dir.path().join("sink");
    let alias = dir.path().join("alias");
    std::fs::write(&file, b"sentinel").unwrap();
    std::fs::hard_link(&file, &alias).unwrap();
    let sink = std::fs::File::open(&file).unwrap();
    let trace = Trace::completed(&completed()).unwrap();
    assert!(
        trace
            .publish(
                RecorderDestination::pin(&alias).unwrap(),
                &[],
                &[("stdout", &sink)]
            )
            .is_err()
    );
    assert!(
        trace
            .publish(
                RecorderDestination::pin(&file).unwrap(),
                &[("root", &grant)],
                &[]
            )
            .is_err()
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"sentinel");
    let missing = dir.path().join("new");
    assert!(
        trace
            .publish(
                RecorderDestination::pin(&missing).unwrap(),
                &[("root", &grant)],
                &[]
            )
            .is_err()
    );
    assert!(!missing.exists());
}

#[test]
fn canonical_export_propagates_open_short_write_and_flush_failures() {
    struct Failing(bool);
    impl std::io::Write for Failing {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 { Ok(bytes.len()) } else { Ok(0) }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("flush"))
        }
    }
    let trace = Trace::completed(&completed()).unwrap();
    assert_eq!(
        trace.write::<Vec<u8>>(|| Err("open failed".into())),
        Err("open failed".into())
    );
    assert!(
        trace
            .write(|| Ok(Failing(false)))
            .unwrap_err()
            .contains("canonical trace:")
    );
    assert!(
        trace
            .write(|| Ok(Failing(true)))
            .unwrap_err()
            .contains("flush")
    );
}

#[test]
fn canonical_export_captures_final_root_boundary_before_scheduler_destruction() {
    let mut reference = None;
    for width in [1, 2, 4, 8] {
        for reorder in [
            hirsel_run_config::Reorder::Off,
            hirsel_run_config::Reorder::Auto,
        ] {
            let mut run = crate::host::BodyRun::new(width, true);
            unsafe { run.use_physical_dispatch() };
            let (outcome, trace) = run.run_traced(
                move |ctx| unsafe {
                    let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                    policy.reorder = reorder;
                    super::super::super::engine::enable(ctx, policy);
                    super::super::enable(ctx);
                },
                |_| crate::STATUS_OK,
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            assert_eq!(outcome.pending, 0);
            let trace = trace.unwrap();
            let rows: Vec<Value> = std::str::from_utf8(trace.bytes())
                .unwrap()
                .lines()
                .map(|row| serde_json::from_str(row).unwrap())
                .collect();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["outcome"]["kind"], "root_end");
            assert_eq!(rows[0]["transition"]["kind"], "end");
            if let Some(reference) = &reference {
                assert_eq!(trace.bytes(), reference);
            } else {
                reference = Some(trace.bytes().to_vec());
            }
        }
    }
}

#[test]
fn canonical_export_refuses_running_workers_and_disabled_collectors() {
    // Pending workers are rejected without even reading the run pointer.
    assert_eq!(
        unsafe { Trace::capture(std::ptr::null_mut(), 1) }.unwrap_err(),
        "canonical trace has running workers"
    );
    for ordered in [false, true] {
        let run = crate::host::BodyRun::new(1, false);
        let (outcome, trace) = run.run_traced(
            move |ctx| unsafe {
                if ordered {
                    super::super::super::engine::enable(
                        ctx,
                        crate::ordered::Policy::admit(&Default::default(), None),
                    );
                }
            },
            |_| crate::STATUS_OK,
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(
            trace.unwrap_err(),
            if ordered {
                "canonical evidence was not enabled"
            } else {
                "no ordered scheduler"
            }
        );
    }
}

#[test]
fn canonical_export_captures_owned_terminal_stop_before_abandonment() {
    let run = crate::host::BodyRun::new(1, false);
    let (outcome, trace) = run.run_traced(
        |ctx| unsafe {
            super::super::super::engine::enable(
                ctx,
                crate::ordered::Policy::admit(&Default::default(), None),
            );
            super::super::enable(ctx);
        },
        |ctx| unsafe {
            crate::stop::raise(ctx, crate::stop::Stop::Refused("trace terminal λ".into()))
        },
    );
    assert_eq!(
        outcome.ended,
        Err(crate::stop::Stop::Refused("trace terminal λ".into()))
    );
    assert_eq!(outcome.pending, 0);
    let trace = trace.unwrap();
    let rows: Vec<Value> = std::str::from_utf8(trace.bytes())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["item"]["kind"], "terminal");
    assert_eq!(rows[0]["outcome"]["kind"], "stopped");
    assert_eq!(rows[0]["transition"]["kind"], "stop");
}
