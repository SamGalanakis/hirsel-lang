//! Owned adapters use exactly the synchronous mutex mutation functions.
//! No entry here changes admission of existing native callers.
use super::*;
use crate::TaskCtx;
use crate::tasks::free::{mutex, record, replay};
use crate::tasks::role;

/// Move a certified Some carrier into a stable image before beginning put.
/// Kill/cause retain the task failure and the wrapper's checked frame borrow.
/// # Safety
/// The running Active task holds the role. The compiler proves all active
/// callers resumable, the payload carrier is in retained frame storage with
/// no live aliases, and the callback's exclusive loan has ended. The caller
/// keeps the wrapper owner, callback result and failure cleanup in its frame.
/// `payload` is nonnull only for Put and holds its unique Some image.
pub(in crate::tasks::free) unsafe fn prepare_mutex(
    ctx: *mut TaskCtx,
    site: Site,
    payload: *mut u8,
) -> Ticket {
    assert!(matches!(site.kind, Kind::Put | Kind::Kill | Kind::Cause));
    assert_eq!(site.object.0, crate::free::MUTEX);
    let task = role::with_role(ctx, |s| s.key(s.current));
    let mut owners = Owners::default();
    if site.kind == Kind::Put {
        let cell = role::with_role(ctx, |s| s.sync.mutex(site.object.1));
        let layout = unsafe { &*(*ctx).descriptor(cell.element.descriptor) };
        let inside = role::with_role(ctx, |s| {
            let me = s.current;
            s.task(me).compact_frame.is_some_and(|(base, bytes)| {
                let start = payload as usize;
                start >= base
                    && start
                        .checked_add(cell.element.size)
                        .is_some_and(|end| end <= base + bytes)
            })
        });
        assert!(
            inside,
            "an owned put carrier outside retained frame storage"
        );
        let mut image = Ring::new(cell.element.size, layout.align as usize);
        image.push(payload, cell.element.size);
        // The image is now the sole owner. Clear with the actual Option None,
        // which can have a nonzero tag or nontrivial padding representation.
        unsafe {
            std::ptr::copy_nonoverlapping(cell.element.none.as_ptr(), payload, cell.element.size)
        };
        owners.image = Some(image);
        owners.descriptor = Some(cell.element.descriptor);
    } else {
        assert!(payload.is_null());
    }
    unsafe { (*ctx).free_ops.tails.prepare(task, site, owners) }
}

/// Poll the entered intrinsic, never its `mutex.with` wrapper. The caller
/// handles Pending through its certified return action and polls this ticket
/// again after activation. A completed call drains only its original suffix.
/// # Safety
/// As for prepare_mutex, with its same task generation and live ticket. The
/// caller holds no record borrow; the task's Active continuation is exclusive.
pub(in crate::tasks::free) unsafe fn poll_mutex(
    ctx: *mut TaskCtx,
    ticket: Ticket,
    leave: bool,
) -> Stopped<Poll<u8>> {
    let rest = std::cell::Cell::new(false);
    let result = unsafe {
        poll_mutex_with(ctx, ticket, leave, |site, entered| {
            replay::poll_object_entered(ctx, site.object, entered, &rest)
        })
    };
    if matches!(result, Ok(Poll::Pending)) && rest.get() {
        unsafe { (*ctx).free_ops.tails.active(crate::tasks::running_key(ctx), ticket).rest = true };
    }
    result
}

unsafe fn poll_mutex_with(
    ctx: *mut TaskCtx,
    ticket: Ticket,
    leave: bool,
    ready: impl FnOnce(Site, &mut bool) -> Stopped<bool>,
) -> Stopped<Poll<u8>> {
    let task = role::with_role(ctx, |s| s.key(s.current));
    let record = unsafe { (*ctx).free_ops.tails.active(task, ticket) as *mut Record };
    let site = unsafe { (*record).site };
    // Consume only the physical leave marker; ordinal/Entered live in record.
    crate::tasks::compact::waited(ctx);
    assert!(matches!(site.kind, Kind::Put | Kind::Kill | Kind::Cause));
    let result = unsafe { &mut *record }.poll_entered(
        || {
            if !record::begin_entered(ctx) { return Err(crate::stop::Stop::Requested); }
            Ok(record::on(ctx).then(|| record::position(ctx).1.op))
        },
        |entered| ready(site, entered),
        |owners| {
            match site.kind {
                Kind::Put => {
                    let image = owners.image.as_ref().expect("an owned put image");
                    let Some(next) = (unsafe { mutex::put_action_entered(ctx, site.object.1, image.at(0), true) })? else { return Ok(None) };
                    owners.wakes.extend(next);
                    // cell.put installed its exact None; all language credits
                    // belong to the cell now, so this only reclaims image bytes.
                    owners.image.take();
                }
                Kind::Kill => {
                    let Some(wakes) = (unsafe { mutex::kill_action_entered(ctx, site.object.1, true) })? else { return Ok(None) };
                    owners.wakes = wakes;
                }
                Kind::Cause => {
                    let Some(chain) = (unsafe { mutex::cause_action_entered(ctx, site.object.1, true) })? else { return Ok(None) };
                    owners.chain = Some(chain);
                }
                _ => unreachable!("a non-mutex tail"),
            }
            Ok(Some(STATUS_OK))
        },
    )?;
    let Poll::Ready(status) = result else {
        return Ok(Poll::Pending);
    };
    // No child op starts in this bounded suffix, and no saved pointer names
    // a Rust wake list. Role::with_held never observes a stop partway through.
    unsafe {
      if !(*record).suffix_done {
        role::with_held(ctx, |s| {
            while let Some(task) = (*record).owners.wakes.pop_front() {
                s.task(task).retried = true;
                s.wake(task);
            }
        });
        if let Some(chain) = (*record).owners.chain.take() {
            (*ctx).failure.append_chain(chain.as_ref().clone());
        }
        (*record).suffix_done = true;
      }
      if !(*record).yielded {
        (*record).yielded = true;
        if site.kind == Kind::Cause {
            if replay::at_cut(ctx) { return Err(crate::stop::Stop::Requested); }
        } else if crate::tasks::free::try_yield_entered(&mut *ctx, leave)? {
            return Ok(Poll::Pending);
        }
      }
      (*record).finish();
      (*ctx).free_ops.tails.retire(task, ticket);
    }
    Ok(Poll::Ready(status))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cut_tests;
