//! Explicit native-clock admission. Replay derives resource positions from
//! checked creator metadata, without consulting a live adapter or host clock.
use super::*;
use std::sync::atomic::Ordering;

/// # Safety
/// Sole-root admission, before native Io entries and child dispatch.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    assert!(super::super::record::on(ctx) || super::super::replay::on(ctx));
    unsafe {
        (*ctx)
            .run_ref()
            .free_native_io
            .store(true, Ordering::Release)
    };
}

pub(crate) fn enabled(ctx: *mut TaskCtx) -> bool {
    unsafe { (*ctx).run_ref().free_native_io.load(Ordering::Acquire) }
}

fn clock_request(request: &Request) -> bool {
    matches!(
        request,
        Request::MonotonicClock { io: 1 }
            | Request::WallClock { io: 1 }
            | Request::Monotonic { .. }
            | Request::Wall { .. }
            | Request::SleepUntil { .. }
    )
}

/// # Safety
/// An admitted native entry holds the role. Its owned request moves into
/// Pending before any phase, wait or adapter call can abandon this frame.
pub(crate) unsafe fn issue(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    if !clock_request(&request) {
        return Err(Stop::Refused(
            "native Free host request is not admitted yet".into(),
        ));
    }
    let result = if super::super::replay::on(ctx) {
        let keys = super::super::replay::token(ctx)
            .unwrap()
            .host_clock_keys(&request)?;
        unsafe { replay_request(ctx, request, keys) }?
    } else {
        unsafe { concurrent::request(ctx, request) }?
    };
    // The existing native caller performs its bounded quantum check after
    // delivery. Waiting attempts already released their key positions.
    Ok((result, false))
}

pub(in crate::tasks::free) fn replay_clock_keys(
    handles: &handles::Handles,
    request: &Request,
) -> Stopped<KeySet> {
    let keys = match request {
        Request::MonotonicClock { io: 1 } | Request::WallClock { io: 1 } => {
            vec![(crate::io_keys::ROOT, Mode::Shared)]
        }
        Request::Monotonic { clock: word }
        | Request::Wall { clock: word }
        | Request::SleepUntil { clock: word, .. } => {
            let (key, kind) = handles.identity(*word)?;
            if kind != crate::io_keys::Kind::Other {
                return Err(Stop::Diverged("native clock resource kind differs".into()));
            }
            vec![(key, Mode::Exclusive)]
        }
        _ => return Err(Stop::Diverged("unadmitted native clock request".into())),
    };
    Ok(KeySet::new(keys))
}
