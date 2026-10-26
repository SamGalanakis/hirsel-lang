//! Typed no-yield read entries over the existing owned host phases.
use super::*;

/// # Safety
/// First entry borrows Stdin and an exclusive retained List[u8] slot, with a
/// disjoint retained HandleResult. Re-entry may pass null original pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ordered_stdin_read(
    ctx: *mut TaskCtx,
    _caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut u8,
    stdin: *const u64,
    buffer: *mut *mut crate::Header,
    max: u64,
) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let bound = unsafe { crate::continuation::binding::take_bound(ctx, 34) };
    let Some(site) = bound.filter(|_| engine::on(ctx)) else {
        let status = unsafe {
            crate::io::hirsel_rt_v2_stdin_read(ctx, destination, out.cast(), stdin, buffer, max)
        };
        if let Some(site) = bound {
            unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        }
        return status;
    };
    assert_eq!(site.operation_site, operation_site, "stdin operation site");
    unsafe {
        enter(ctx, site, out, Some(buffer), || Request::ReadStdin {
            stdin: stdin.read(),
            max,
        })
    }
}

/// # Safety
/// First entry borrows Directory and exclusive Watcher, and owns a retained
/// WatchResult. Re-entry may pass null original pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_ordered_watcher_next(
    ctx: *mut TaskCtx,
    _caller_site: u64,
    operation_site: u64,
    destination: *mut u8,
    out: *mut u8,
    directory: *const u64,
    watcher: *mut u64,
) -> u8 {
    let _role = unsafe { role::Guard::new(ctx) };
    let bound = unsafe { crate::continuation::binding::take_bound(ctx, 35) };
    let Some(site) = bound.filter(|_| engine::on(ctx)) else {
        let status = unsafe {
            crate::io::hirsel_rt_v2_watcher_next(ctx, destination, out.cast(), directory, watcher)
        };
        if let Some(site) = bound {
            unsafe { crate::continuation::binding::complete_bound(ctx, site) };
        }
        return status;
    };
    assert_eq!(site.operation_site, operation_site, "watch operation site");
    unsafe {
        enter(ctx, site, out, None, || Request::WatchNext {
            directory: directory.read(),
            watcher: watcher.read(),
        })
    }
}

#[cfg(test)]
mod tests;
