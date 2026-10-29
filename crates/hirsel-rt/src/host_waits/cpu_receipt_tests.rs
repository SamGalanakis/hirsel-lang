use super::super::*;
use crate::io_concurrent::ConcurrentAdapter;
use std::sync::Arc;

#[test]
fn cpu_receipts_drain_pending_ready_points_and_replace_the_deadline_even_on_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let mut real = crate::io_real::RealIo::grant(temp.path()).unwrap();
    real.virtual_clock(0);
    let io = Arc::new(
        real.into_concurrent()
            .unwrap_or_else(|_| panic!("Send streams")),
    );
    let receipts = crate::io_real::CpuReceipts::admit(&io).unwrap();
    let clock = match io.call(Request::MonotonicClock { io: 1 }).unwrap() {
        Completion::ClockGrant(Ok(clock)) => clock,
        other => panic!("{other:?}"),
    };
    let token = 7;
    assert_eq!(
        io.call_at(
            token,
            Request::SleepUntil {
                clock,
                deadline: 100
            }
        ),
        Err(AdapterError::Blocked)
    );
    io.parked(token);
    let outcome = crate::host::BodyRun::new(1, false).run(
        |ctx| unsafe {
            let hint = &(*ctx).run_ref().timer_hint;
            hint.scope_only(true);
            for points in [3, 4] {
                hint.publish(Some(
                    std::time::Instant::now() + std::time::Duration::from_secs(60),
                ));
                for _ in 0..points {
                    assert!(hint.empty_point());
                }
                if points == 3 {
                    assert_eq!(receipts.control(ctx, 4, 2, 0, Some(0)), Ok(Some(0)));
                } else {
                    let invalid = crate::io::CpuSummary {
                        version: 0,
                        retirement: 0,
                        ordinal: 0,
                        value: Vec::new(),
                        stop: None,
                        native: None,
                    };
                    assert_eq!(
                        receipts.summary(ctx, &invalid),
                        Err(crate::stop::Stop::refusal(AdapterError::InvalidCompletion))
                    );
                }
                assert_eq!(
                    hint.take_points(),
                    0,
                    "the receipt drained every pending point"
                );
                assert!(
                    !hint.empty_point(),
                    "the shared core declines the old exclusive timer hint"
                );
            }
        },
        |_| crate::STATUS_OK,
    );
    assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
    assert!(io.wait().unwrap());
    assert_eq!(io.ready_for(false).unwrap(), Ready::Tokens(vec![token]));
    assert!(
        io.tape()
            .iter()
            .any(|event| event.request == Request::Ready { wait: 0, points: 7 })
    );
}
