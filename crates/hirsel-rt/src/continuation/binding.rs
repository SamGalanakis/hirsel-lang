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
    finalizers: Option<FinalizerRows>,
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
        let existing = self
            .stack
            .iter()
            .rposition(|b| b.site.frame == site.frame && b.site.state == site.state);
        if let Some(index) = existing {
            let binding = &self.stack[index];
            site.invocation = binding.site.invocation;
            assert_eq!(binding.site, site, "a different entered-tail invocation");
            assert!(
                !binding.armed,
                "an entered tail rebound before its intrinsic"
            );
            assert!(
                self.stack.iter().all(|b| !b.armed),
                "nested tail before intrinsic entry"
            );
            // Reentry walks ancestors before their pending descendants. Keep
            // both records and arm only this immediate call's binding.
            self.stack[index].armed = true;
        } else {
            assert!(
                self.stack.iter().all(|b| !b.armed),
                "nested tail before intrinsic entry"
            );
            self.next = self
                .next
                .checked_add(1)
                .expect("entered-site identity overflow");
            site.invocation = self.next;
            self.stack.push(Binding {
                site,
                armed: true,
                finalizers: None,
            });
        }
    }
    fn take(&mut self, kind: u32) -> Option<BoundSite> {
        let binding = self.stack.iter_mut().rev().find(|b| b.armed)?;
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
    // Optional cleanup tracing retains a native scope pointer until its
    // synchronous cleanup returns. It cannot follow a discarded native stack.
    #[cfg(feature = "ownership-trace")]
    if !unsafe { (*ctx).ownership_cleanup.is_null() } {
        return false;
    }
    let inside = crate::tasks::compact::contains_frame(ctx, site.frame, site.bytes as usize);
    inside && unsafe { crate::tasks::compact::movable(ctx) }
}

#[cfg(test)]
mod tests;

/// Immutable native resolver row. Descriptor indices live in the linked
/// program's glue cells; the table and dispatchers outlive the entire run.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct NativeFinalizer {
    pub descriptor: *const u32,
    pub dispatch: crate::finalize::entered::Dispatch,
    pub offset: u32,
    pub bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FinalizerRows {
    address: usize,
    count: u32,
}

/// Bind a checked cleanup root and its complete immutable source resolver.
/// # Safety
/// As for resume_tail. Every row belongs to this root's independently checked
/// cleanup graph; nested extents are disjoint from its live image and from
/// one another. Rows and code are immutable program storage, never stack bytes.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_cleanup(
    ctx: *mut crate::TaskCtx,
    frame: *mut u8,
    state: u32,
    kind: u32,
    bytes: u32,
    anchor: u64,
    operation_site: u64,
    rows: *const u8,
    count: u32,
) {
    unsafe { hirsel_rt_v2_resume_tail(ctx, frame, state, kind, bytes, anchor, operation_site) };
    if !unsafe { crate::pool::free::enabled((*ctx).run_ref()) } {
        return;
    }
    assert!((48..=50).contains(&kind));
    assert!(!rows.is_null() && count > 0 && count <= 64);
    let claimed = FinalizerRows {
        address: rows as usize,
        count,
    };
    let binding = unsafe {
        (*ctx)
            .resume_bindings
            .stack
            .iter_mut()
            .rev()
            .find(|b| b.armed)
    }
    .expect("missing cleanup binding");
    if let Some(previous) = binding.finalizers {
        // Ordinary/ahead objects can own distinct immutable tables and adapter
        // addresses for the same checked source. Keep the original table and
        // any entered Driver; rebinding must never replace its saved PC.
        assert_eq!(previous.count, claimed.count);
        let previous = unsafe {
            std::slice::from_raw_parts(previous.address as *const NativeFinalizer, count as usize)
        };
        let claimed = unsafe {
            std::slice::from_raw_parts(claimed.address as *const NativeFinalizer, count as usize)
        };
        for (a, b) in previous.iter().zip(claimed) {
            assert!(!a.descriptor.is_null() && !b.descriptor.is_null());
            assert_eq!(unsafe { a.descriptor.read() }, unsafe {
                b.descriptor.read()
            });
            assert_eq!((a.offset, a.bytes), (b.offset, b.bytes));
        }
    } else {
        binding.finalizers = Some(claimed);
    }
}

/// The explicitly bound root alone opts into source finalizers. An opaque
/// nested release cannot inherit this resolver after the binding disarms.
pub(crate) unsafe fn cleanup_resolver(
    ctx: *mut crate::TaskCtx,
) -> Option<crate::tasks::free::tail::FinalizerResolver> {
    let binding = unsafe { (*ctx).resume_bindings.stack.last() }?;
    if binding.armed || !(48..=50).contains(&binding.site.kind) || binding.finalizers.is_none() {
        return None;
    }
    Some(crate::tasks::free::tail::FinalizerResolver {
        resolve: resolve_finalizer,
        retained: binding.site.frame,
    })
}

unsafe fn resolve_finalizer(
    ctx: *mut crate::TaskCtx,
    token: crate::release::Finalizer,
    retained: usize,
) -> Option<crate::tasks::free::tail::ResolvedFinalizer> {
    let binding = unsafe {
        (*ctx)
            .resume_bindings
            .stack
            .iter()
            .rev()
            .find(|b| b.site.frame == retained && b.finalizers.is_some())
    }?;
    let site = binding.site;
    let rows = binding.finalizers?;
    assert_eq!(crate::tasks::running_key(ctx), site.task);
    let rows = unsafe {
        std::slice::from_raw_parts(rows.address as *const NativeFinalizer, rows.count as usize)
    };
    let mut selected = None;
    for row in rows {
        assert!(!row.descriptor.is_null());
        let descriptor = unsafe { row.descriptor.read() };
        if unsafe { (*ctx).descriptor(descriptor) } != token.layout {
            continue;
        }
        assert!(selected.is_none(), "duplicate source finalizer resolver");
        assert!(row.offset >= 8 && row.offset.is_multiple_of(16));
        assert!((8..=4096).contains(&row.bytes));
        assert!(
            row.offset
                .checked_add(row.bytes)
                .is_some_and(|end| end <= site.bytes)
        );
        let frame = unsafe { (retained as *mut u8).add(row.offset as usize) };
        assert!(crate::tasks::compact::contains_frame(
            ctx,
            frame as usize,
            row.bytes as usize
        ));
        selected = Some(crate::tasks::free::tail::ResolvedFinalizer {
            dispatch: row.dispatch,
            frame,
            bytes: row.bytes,
        });
    }
    selected
}
