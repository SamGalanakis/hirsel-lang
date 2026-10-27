//! Scoped CPU receipts retire through the existing Ordered head and Yield.
use super::{engine, events};
use crate::TaskCtx;
use crate::ordered::Next;
use crate::stop::{Stop, Stopped};

fn receipt(
    ctx: *mut TaskCtx,
    width: u32,
    action: u32,
    count: u64,
    value: Option<u32>,
) -> Stopped<Option<u32>> {
    if unsafe { (*ctx).io_data() }.is_null() {
        return Ok(value);
    }
    let mut result = Ok(None);
    unsafe {
        crate::pool::call_home(ctx, &mut || {
            result = crate::io::Binding::get(ctx)
                .adapter
                .cpu_control(width, action, count, value);
        })
    }?;
    result.map_err(Stop::refusal)
}

pub(crate) fn control(
    ctx: *mut TaskCtx,
    width: u32,
    action: u32,
    count: u64,
    live: impl FnOnce() -> u32,
) -> Stopped<u32> {
    engine::at_head(ctx)?;
    // The physical lifecycle may migrate and save this same head. No frontier
    // borrow or adapter loan spans startup, migration or join.
    let value = match receipt(ctx, width, action, count, None)? {
        Some(value) => value,
        None => {
            let value = live();
            receipt(ctx, width, action, count, Some(value))?;
            value
        }
    };
    let (_, progress) = engine::head_canonical(
        ctx,
        "cpu.control",
        || ((), Next::Yield),
        |_| {
            Some(vec![(
                events::CanonicalItem::Root,
                events::CanonicalOutcome::CpuControl {
                    width,
                    action,
                    count,
                    outcome: value,
                },
            )])
        },
    )?;
    if action == 2 && count == 0 && value == 0 {
        // The claim receipt retired at head. Its CPU-only tail may overlap
        // later turns, just like an Ordered completed read's certified tail.
        engine::continue_after_read(ctx, progress)?;
    } else {
        engine::continue_at_head(ctx, progress)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{Adapter, AdapterError, Completion, Event, Request, Tree};
    use crate::io_sim::SimIo;

    fn event(width: u32, action: u32, count: u64, outcome: u32) -> Event {
        Event {
            request: Request::CpuControl {
                width,
                action,
                count,
                outcome,
            },
            completion: Completion::Ready,
            fault: None,
            resources: None,
        }
    }

    #[test]
    fn replay_checks_the_entire_receipt_before_consuming_it_and_refuses_a_cut() {
        for (width, action, count) in [(1, 0, 0), (4, 1, 0), (4, 0, 1)] {
            let mut replay = SimIo::new(Tree::default(), vec![event(4, 0, 0, 4)]);
            assert_eq!(
                replay.cpu_control(width, action, count, None),
                Err(AdapterError::ReplayMismatch(0))
            );
            assert_eq!(replay.cpu_control(4, 0, 0, None), Ok(Some(4)));
            assert_eq!(
                replay.cpu_control(4, 1, 0, None),
                Err(AdapterError::MissingInput(1))
            );
        }
    }

    #[test]
    fn replay_at_one_permit_uses_receipts_without_entering_the_physical_lifecycle() {
        let mut run = crate::host::BodyRun::new(1, false);
        run.adapter = Some(Box::new(SimIo::new(
            Tree::default(),
            vec![event(16, 0, 0, 0), event(16, 2, 32, 0), event(16, 1, 0, 0)],
        )));
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                engine::enable(
                    ctx,
                    crate::ordered::Policy::admit(&Default::default(), None),
                );
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                for (action, count) in [(0, 0), (2, 32), (1, 0)] {
                    assert_eq!(
                        control(ctx, 16, action, count, || panic!(
                            "replay queried physical admission"
                        )),
                        Ok(0)
                    );
                    assert!(!crate::tasks::cpu_scoped(ctx));
                }
                crate::stop::check(ctx, engine::root_end(ctx, 0));
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
    }
}
