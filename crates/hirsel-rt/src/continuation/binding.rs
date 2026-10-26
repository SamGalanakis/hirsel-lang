//! Invocation custody shared by independently checked entered runtime sites.
//! A binding disarms before the intrinsic runs. It survives Pending under the
//! task's existing Active/Saved owner, and completion retires its serial once.
use crate::tasks::registry::TaskKey;

#[derive(Default)]
pub(crate) struct Bindings {
    next: u64,
    stack: Vec<Binding>,
}

/// Identity of one checked entered invocation, stable across rebinds and
/// never reused when a loop reaches the same compiled state again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BoundSite {
    pub(crate) task: TaskKey,
    pub(crate) invocation: u64,
    pub(crate) frame: usize,
    pub(crate) state: u32,
    pub(crate) kind: u32,
    pub(crate) bytes: u32,
    pub(crate) anchor: u64,
    pub(crate) operation_site: u64,
}

struct Binding {
    site: BoundSite,
    armed: bool,
}

/// Bind only a site for which the independent continuation checker proved
/// the whole native callchain. This carries no scheduling permission.
/// Kinds 0..=2 are mutex put/kill/cause; scope operations use 16 and above.
/// # Safety
/// The Active dispatcher owns frame, with exactly this checked state/layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_tail(
    ctx: *mut crate::TaskCtx,
    frame: *mut u8,
    state: u32,
    kind: u32,
    bytes: u32,
    anchor: u64,
    operation_site: u64,
) {
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    if !unsafe { crate::pool::free::enabled((*ctx).run_ref()) } {
        return;
    }
    assert_eq!(unsafe { frame.cast::<u32>().read() }, state);
    assert!(kind <= 2 || kind >= 16, "unknown entered-tail kind");
    let task = crate::tasks::running_key(ctx);
    assert!(
        crate::tasks::compact::contains_frame(ctx, frame as usize, bytes as usize),
        "an entered tail outside the active compact frame"
    );
    unsafe {
        (*ctx).resume_bindings.bind(BoundSite {
            task,
            invocation: 0,
            frame: frame as usize,
            state,
            kind,
            bytes,
            anchor,
            operation_site,
        })
    };
}

impl Bindings {
    fn bind(&mut self, mut site: BoundSite) {
        if let Some(binding) = self
            .stack
            .last_mut()
            .filter(|b| b.site.frame == site.frame && b.site.state == site.state)
        {
            site.invocation = binding.site.invocation;
            assert_eq!(binding.site, site, "a different entered-tail invocation");
            assert!(
                !binding.armed,
                "an entered tail rebound before its intrinsic"
            );
            binding.armed = true;
        } else {
            assert!(
                self.stack.last().is_none_or(|b| !b.armed),
                "nested tail before intrinsic entry"
            );
            self.next = self
                .next
                .checked_add(1)
                .expect("entered-site identity overflow");
            site.invocation = self.next;
            self.stack.push(Binding { site, armed: true });
        }
    }
    fn take(&mut self, kind: u32) -> Option<BoundSite> {
        let binding = self.stack.last_mut()?;
        if !binding.armed {
            return None;
        }
        assert_eq!(
            binding.site.kind, kind,
            "a bound tail reached a different intrinsic"
        );
        binding.armed = false;
        Some(binding.site)
    }
    fn complete(&mut self, site: BoundSite) {
        let binding = self.stack.last().expect("an unbound tail completion");
        assert_eq!(binding.site, site, "a stale bound tail completion");
        assert!(
            !binding.armed,
            "a tail completed before entering its intrinsic"
        );
        self.stack.pop();
    }
}

/// Disarm before entering the intrinsic. Nested synchronous callbacks cannot
/// inherit this call site's permission, even for the same operation kind.
/// # Safety
/// The running task owns the context; the entry uses exactly `kind`'s ABI.
pub(crate) unsafe fn take_bound(ctx: *mut crate::TaskCtx, kind: u32) -> Option<BoundSite> {
    unsafe { (*ctx).resume_bindings.take(kind) }
}

/// Retire after the exact completed suffix. A pending rebind retains its key;
/// the next completed loop invocation gets a new serial instead.
/// # Safety
/// The Active task completed this site's original operation and its suffix.
pub(crate) unsafe fn complete_bound(ctx: *mut crate::TaskCtx, site: BoundSite) {
    unsafe { (*ctx).resume_bindings.complete(site) };
}

/// Only the exclusive Active task can leave, with this complete retained
/// callchain. No first-op heuristic or whole-call retry hint is involved.
/// # Safety
/// The site's checked entered operation has published its logical wait and
/// holds no transient guard, lease or borrowed runtime frame across return.
pub(crate) unsafe fn can_leave(ctx: *mut crate::TaskCtx, site: BoundSite) -> bool {
    let binding = unsafe { (*ctx).resume_bindings.stack.last() }.expect("an unbound tail leave");
    assert_eq!(binding.site, site, "a stale entered-site leave");
    assert!(!binding.armed, "leave before intrinsic entry");
    assert_eq!(
        crate::tasks::running_key(ctx),
        site.task,
        "a stale entered-site task generation"
    );
    let inside = crate::tasks::compact::contains_frame(ctx, site.frame, site.bytes as usize);
    inside && unsafe { crate::tasks::compact::movable(ctx) }
}

#[cfg(test)]
mod tests;
