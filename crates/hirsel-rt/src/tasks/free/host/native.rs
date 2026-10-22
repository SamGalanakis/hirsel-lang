//! Explicit native clock, file, output and random admission. Replay derives resource positions from
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

/// # Safety
/// An admitted native entry holds the role. Its owned request moves into
/// Pending before any phase, wait or adapter call can abandon this frame.
pub(crate) unsafe fn issue(
    ctx: *mut TaskCtx,
    request: Request,
) -> Stopped<(Result<Completion, AdapterError>, bool)> {
    let result = if super::super::replay::on(ctx) {
        let keys = super::super::replay::token(ctx)
            .unwrap()
            .host_keys(&request)?;
        unsafe { replay_request(ctx, request, keys) }?
    } else {
        let keys = unsafe { concurrent::native_keys(ctx, &request) }?;
        unsafe { concurrent::request_with_keys(ctx, request, keys) }?
    };
    // The existing native caller performs its bounded quantum check after
    // delivery. Waiting attempts already released their key positions.
    Ok((result, false))
}

pub(in crate::tasks::free) fn replay_keys(
    handles: &handles::Handles,
    request: &Request,
) -> Stopped<KeySet> {
    request_keys(request, |word| handles.identity(word))
}

/// One allow-list and footprint rule for live admission and replay. Native
/// files currently require fully described File identities and the namespace
/// domain alone; extra adapter domains refuse before preparation or effect.
pub(super) fn request_keys(
    request: &Request,
    mut identity: impl FnMut(u64) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)>,
) -> Stopped<KeySet> {
    use crate::io_keys::{Kind, NAMESPACE, ROOT};
    use Request as R;
    let mut file = |word| {
        let (key, kind) = identity(word)?;
        if kind != Kind::File {
            return Err(Stop::Diverged("native file resource kind differs".into()));
        }
        Ok(key)
    };
    let keys = match request {
        R::MonotonicClock { io: 1 }
        | R::WallClock { io: 1 }
        | R::Stdin { io: 1 }
        | R::Random { io: 1 }
        | R::Stdout { io: 1 }
        | R::Stderr { io: 1 } => vec![(ROOT, Mode::Shared)],
        R::Print { stdout: word, .. }
        | R::PrintStderr { stderr: word, .. }
        | R::ReadStdin { stdin: word, .. } => {
            let (key, kind) = identity(*word)?;
            let Kind::Sink { file_backed } = kind else {
                return Err(Stop::Diverged("native stream resource kind differs".into()));
            };
            let mut keys = vec![(key, Mode::Exclusive)];
            if file_backed {
                keys.push((NAMESPACE, Mode::Shared));
            }
            keys
        }
        R::Monotonic { clock }
        | R::Wall { clock }
        | R::SleepUntil { clock, .. }
        | R::RandomBytes { random: clock, .. } => {
            let (key, kind) = identity(*clock)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native scalar resource kind differs".into()));
            }
            vec![(key, Mode::Exclusive)]
        }
        R::Directory { io: 1, .. } => vec![(NAMESPACE, Mode::Shared)],
        R::NarrowDir { directory, .. }
        | R::ReadOnlyDir { directory }
        | R::Open { directory, .. }
        | R::Size { directory, .. }
        | R::Read { directory, .. }
        | R::Map { directory, .. }
        | R::Watch { directory, .. }
        | R::ReadDir { directory, .. }
        | R::Metadata { directory, .. } => {
            file(*directory)?;
            vec![(NAMESPACE, Mode::Shared)]
        }
        R::Create { directory, .. }
        | R::CreateAtomic { directory, .. }
        | R::Write { directory, .. }
        | R::PutNew { directory, .. }
        | R::Remove { directory, .. }
        | R::RemoveAll { directory, .. }
        | R::Mkdir { directory, .. }
        | R::RemoveDir { directory, .. }
        | R::Rename { directory, .. }
        | R::SetPermissions { directory, .. } => {
            file(*directory)?;
            vec![(NAMESPACE, Mode::Exclusive)]
        }
        R::FileRead {
            directory,
            file: word,
            ..
        }
        | R::FileSeek {
            directory,
            file: word,
            ..
        }
        | R::FileWrite {
            directory,
            file: word,
            ..
        }
        | R::FileAppend {
            directory,
            file: word,
            ..
        }
        | R::NarrowFile {
            directory,
            file: word,
        }
        | R::WriteAll {
            directory,
            file: word,
            ..
        }
        | R::Sync {
            directory,
            file: word,
        }
        | R::MapView {
            directory,
            mapped: word,
        }
        | R::MapCheck {
            directory,
            mapped: word,
        }
        | R::WatchNext {
            directory,
            watcher: word,
        } => {
            file(*directory)?;
            vec![(file(*word)?, Mode::Exclusive), (NAMESPACE, Mode::Shared)]
        }
        R::Commit {
            directory,
            file: word,
        } => {
            file(*directory)?;
            vec![
                (file(*word)?, Mode::Exclusive),
                (NAMESPACE, Mode::Exclusive),
            ]
        }
        R::Close { handle } => vec![(file(*handle)?, Mode::Exclusive), (NAMESPACE, Mode::Shared)],
        _ => return Err(Stop::Diverged("unadmitted native host request".into())),
    };
    Ok(KeySet::new(keys))
}

/// Keep returned buffers task-owned across the native caller's scheduling
/// check. Terminal cuts abandon saved Rust continuations without Drop.
/// # Safety
/// The owner holds no table/adapter borrow; no earlier result is outstanding.
pub(crate) unsafe fn finish_result(
    ctx: *mut TaskCtx,
    result: Result<Completion, ()>,
    operation: bool,
    parked: bool,
) -> Result<Completion, ()> {
    assert!(unsafe { (*ctx).native_host_result.is_none() });
    unsafe { (*ctx).native_host_result = Some(Box::new(result)) };
    if operation && !parked {
        unsafe { crate::tasks::yield_now(&mut *ctx) };
    } else if !operation {
        unsafe { crate::tasks::leave_home(ctx) };
    }
    *unsafe { (*ctx).native_host_result.take().expect("a native delivery") }
}

/// Keep host-owned payloads outside abandonable Rust frames while an ABI
/// conversion allocates Hirsel storage. The closure borrows the stable result;
/// it must not issue another host request or move its buffers onto the stack.
/// # Safety
/// The task holds its role, has no pending native delivery, and owns completion.
pub(crate) unsafe fn convert_result<T>(
    ctx: *mut TaskCtx,
    completion: Completion,
    convert: impl FnOnce(&Completion) -> T,
) -> T {
    if !enabled(ctx) {
        return convert(&completion);
    }
    assert!(unsafe { (*ctx).native_host_result.is_none() });
    unsafe { (*ctx).native_host_result = Some(Box::new(Ok(completion))) };
    let value = unsafe {
        (*ctx)
            .native_host_result
            .as_deref()
            .unwrap()
            .as_ref()
            .unwrap()
    } as *const Completion;
    let converted = convert(unsafe { &*value });
    drop(unsafe { (*ctx).native_host_result.take().unwrap() });
    converted
}
