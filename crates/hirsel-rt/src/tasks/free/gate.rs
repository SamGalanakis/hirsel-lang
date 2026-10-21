//! Free serial gates reuse the scheduler's role-owned owner/depth table.
//! As with native mutex Retry waits, readiness only notifies; the returning
//! owner performs acquisition and its versioned record in one Done section.
use super::super::*;
use hirsel_free::record::ObjectId;

/// # Safety
/// The task holds the role and no C frame of this outer call lies below it.
pub(crate) unsafe fn enter(ctx: *mut TaskCtx, gate: u32, site: u64) -> Stopped<bool> {
    let me = scheduler(ctx).current;
    if scheduler(ctx).gate(gate).owner == Some(me) {
        let row = scheduler(ctx).gate(gate);
        row.depth = row
            .depth
            .checked_add(1)
            .expect("serial gate depth overflow");
        return Ok(false);
    }
    let started = unsafe { (*ctx).run_ref() }
        .extern_metrics
        .as_ref()
        .map(|_| std::time::Instant::now());
    super::record::begin(ctx);
    let mut entered = false;
    loop {
        let s = scheduler(ctx) as *mut Scheduler;
        unsafe { (*s).task(me).retried = false };
        if unsafe { (*s).gate(gate).owner.is_none() } {
            let object = super::record::on(ctx).then(|| ObjectId::Gate(format!("package:{gate}")));
            let publication = super::record::publish(ctx, Phase::Done, |writer| unsafe {
                let row = (*s).gate(gate);
                row.owner = Some(me);
                row.depth = 1;
                row.waiters.retain(|&task| task != me);
                row.version = row
                    .version
                    .checked_add(1)
                    .expect("serial gate version overflow");
                if let Some(writer) = writer {
                    // The public counter starts at zero; compressed objects
                    // exist from admission at version one without a create op.
                    writer.touch(
                        object.as_ref().expect("a recorded gate identity"),
                        row.version.checked_add(1).unwrap(),
                        0,
                        "gate enter",
                    );
                }
            });
            drop(object);
            publication?;
            if let (Some(started), Some(metrics)) =
                (started, &unsafe { (*ctx).run_ref() }.extern_metrics)
            {
                metrics.gate_entry(started.elapsed());
            }
            return Ok(true);
        }
        let row = unsafe { (*s).gate(gate) };
        if !row.waiters.contains(&me) {
            row.waiters.reserve(1);
            row.waiters.push(me);
        }
        if !entered {
            super::record::publish(ctx, Phase::Entered, |_| ())?;
            entered = true;
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, site) }?;
    }
}

/// # Safety
/// The task holds the role and is leaving its own active gate interval.
pub(crate) unsafe fn leave(ctx: *mut TaskCtx, gate: u32, outermost: bool) -> Stopped<()> {
    let s = scheduler(ctx) as *mut Scheduler;
    let me = unsafe { (*s).current };
    let row = unsafe { (*s).gate(gate) };
    if row.owner != Some(me) || row.depth == 0 || (row.depth == 1) != outermost {
        fault("a serial gate left by a task that does not hold it");
    }
    if !outermost {
        row.depth -= 1;
        return Ok(());
    }
    let waiters = row.waiters.len();
    unsafe { (*s).woken.reserve(waiters) };
    super::record::begin(ctx);
    let object = super::record::on(ctx).then(|| ObjectId::Gate(format!("package:{gate}")));
    let publication = super::record::publish(ctx, Phase::Done, |writer| unsafe {
        let row = (*s).gate(gate);
        row.depth = 0;
        row.owner = None;
        row.version = row
            .version
            .checked_add(1)
            .expect("serial gate version overflow");
        if let Some(writer) = writer {
            writer.touch(
                object.as_ref().expect("a recorded gate identity"),
                row.version.checked_add(1).unwrap(),
                0,
                "gate leave",
            );
        }
        std::mem::take(&mut row.waiters)
    });
    drop(object);
    let waiters = publication?;
    for task in waiters {
        unsafe {
            (*s).task(task).retried = true;
            (*s).wake(task);
        }
    }
    unsafe {
        readiness::poll_free(&mut *s);
        super::publish_ready(&mut *s, (*ctx).run_ptr());
    }
    Ok(())
}
