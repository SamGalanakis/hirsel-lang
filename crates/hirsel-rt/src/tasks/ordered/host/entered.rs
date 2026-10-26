//! Retained callers drive the original owned host invocation. Like the
//! stackful entry, completion owns its bytes through finish; Pending retains
//! neither an adapter loan nor a pointer into the abandoned native stack.
use super::super::inop::{self, Drive, Operation};
use super::*;
use crate::continuation::binding::BoundSite;
use crate::io::{AdapterError, Completion, Request, UnitResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::tasks::ordered) struct Binding {
    site: BoundSite,
    offset: usize,
}

fn binding(ctx: *mut TaskCtx) -> Option<Binding> {
    inop::with_operation(ctx, |d, task| match d.in_operations.get(&task) {
        None => None,
        Some(Operation::Host(h)) => Some(h.binding.expect("unbound entered host")),
        _ => panic!("different entered operation at host entry"),
    })
}

impl Binding {
    fn result(self, ctx: *mut TaskCtx) -> *mut UnitResult {
        assert_eq!(self.site.task, crate::tasks::running_key(ctx));
        assert!(crate::tasks::compact::contains_frame(
            ctx,
            self.site.frame,
            self.site.bytes as usize
        ));
        let address = self
            .site
            .frame
            .checked_add(self.offset)
            .expect("host result address overflow");
        assert_eq!(
            inop::retained_offset(
                self.site.frame,
                self.site.bytes as usize,
                address,
                std::mem::size_of::<UnitResult>() as u64,
                std::mem::align_of::<UnitResult>() as u32,
            ),
            Some(self.offset)
        );
        address as *mut UnitResult
    }
}

/// Enter or continue one certified clock wait. The first entry captures the
/// clock/deadline and result offset. Re-entry uses only the original record.
/// # Safety
/// The independently checked wrapper owns the bound frame and its result
/// cell. Original inputs are readable on first entry; no input is reread
/// after Pending. Only certified callers accept STATUS_SUSPENDED.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ordered_sleep_until(
    ctx: *mut TaskCtx,
    caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut u8,
    clock: *const u64,
    deadline: *const u64,
) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let bound = unsafe { crate::continuation::binding::take_bound(ctx, 33) };
    let Some(site) = bound.filter(|_| engine::on(ctx)) else {
        let status = unsafe {
            crate::io::hirsel_rt_v2_clock_sleep_until_at_site(
                ctx,
                caller_site,
                operation_site,
                destination,
                out.cast(),
                clock,
                deadline,
            )
        };
        if let Some(site) = bound {
            unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        }
        return status;
    };
    assert_eq!(site.operation_site, operation_site, "host operation site");
    unsafe {
        enter(ctx, site, out, || Request::SleepUntil {
            clock: clock.read(),
            deadline: deadline.read(),
        })
    }
}

/// # Safety
/// The first entry's borrowed output and request operands are live. The
/// request factory is consumed before any possible Pending return.
unsafe fn enter(
    ctx: *mut TaskCtx,
    site: BoundSite,
    out: *mut u8,
    request: impl FnOnce() -> Request,
) -> u8 {
    let retained = if let Some(existing) = binding(ctx) {
        assert_eq!(existing.site, site, "stale entered host identity");
        existing
    } else {
        let offset = inop::retained_offset(
            site.frame,
            site.bytes as usize,
            out as usize,
            std::mem::size_of::<UnitResult>() as u64,
            std::mem::align_of::<UnitResult>() as u32,
        )
        .expect("host result outside retained wrapper");
        let retained = Binding { site, offset };
        retained.result(ctx);
        let request = request();
        if unsafe { (*ctx).run_ref().ordered_io.get().is_some() } {
            unsafe { concurrent::start_entered(ctx, request) };
        } else {
            unsafe { home::start_entered(ctx, request) };
        }
        inop::with_operation(ctx, |d, task| {
            let Operation::Host(host) = d.in_operations.get_mut(&task).unwrap() else {
                unreachable!()
            };
            assert!(host.binding.replace(retained).is_none());
        });
        retained
    };
    assert!(unsafe { (*ctx).ordered_inop_leave.is_none() });
    loop {
        let outcome = if unsafe { (*ctx).run_ref().ordered_io.get().is_some() } {
            unsafe { concurrent::drive_entered(ctx) }
        } else {
            unsafe { home::drive_entered(ctx) }
        };
        match unsafe { crate::stop::check(ctx, outcome) } {
            Drive::Pending { head_only } => {
                if unsafe { crate::continuation::binding::can_leave(ctx, site) } {
                    #[cfg(feature = "diagnostic")]
                    crate::compact_trace::runtime_wait(
                        site.task.slot,
                        site.state,
                        site.kind,
                        "host",
                        site.invocation,
                    );
                    unsafe { (*ctx).ordered_inop_leave = Some(head_only) };
                    return crate::STATUS_SUSPENDED;
                }
                engine::suspend(ctx, engine::current(ctx), head_only);
            }
            Drive::Complete(completion) => {
                inop::finish(ctx);
                unsafe { (*ctx).pending_ordered_host.take() };
                unsafe { crate::continuation::binding::complete_bound(ctx, site) };
                let value = match completion {
                    Ok(Completion::Wait(value)) if site.kind == 33 => crate::io::wait_result(value),
                    Ok(Completion::Unit(value)) if site.kind == 36 => UnitResult::of(value),
                    Err(AdapterError::InvalidHandle(_)) if site.kind == 33 => {
                        return crate::STATUS_FAILURE;
                    }
                    Err(error) => unsafe {
                        crate::stop::raise(ctx, crate::stop::Stop::refusal(error))
                    },
                    _ => crate::fault("Io entered host completion mismatch"),
                };
                // Conversion and initialization happen once, after the
                // original finish; no stop or scheduling point intervenes.
                unsafe { retained.result(ctx).write(value) };
                return crate::STATUS_OK;
            }
        }
    }
}

/// Enter/resume a reviewed Print with owned bytes before issuing a job.
/// Unobserved prints retain their existing borrowed, synchronous fast path.
/// # Safety
/// First entry owns readable stdout/String operands and the bound output
/// cell. Re-entry may pass null original operands; the original request owns
/// its bytes until the actual host return and canonical finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ordered_print(
    ctx: *mut TaskCtx,
    caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut u8,
    stdout: *const u64,
    owner: *mut crate::Header,
    start: u64,
    len: u64,
) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let bound = unsafe { crate::continuation::binding::take_bound(ctx, 36) };
    let Some(site) = bound.filter(|_| engine::on(ctx) && super::super::events::observing(ctx))
    else {
        let status = unsafe {
            crate::io::hirsel_rt_v2_out_print_at_site(
                ctx,
                caller_site,
                operation_site,
                destination,
                out.cast(),
                stdout,
                owner,
                start,
                len,
            )
        };
        if let Some(site) = bound {
            unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        }
        return status;
    };
    assert_eq!(site.operation_site, operation_site, "print operation site");
    unsafe {
        enter(ctx, site, out, || {
            let start = usize::try_from(start)
                .unwrap_or_else(|_| crate::fault("Io start exceeds target usize"));
            let data = crate::alloc::payload(owner)
                .wrapping_add(8)
                .wrapping_add(start);
            let bytes = std::slice::from_raw_parts(data, len as usize).to_vec();
            Request::Print {
                stdout: stdout.read(),
                bytes,
            }
        })
    }
}

#[cfg(test)]
mod print_tests;
