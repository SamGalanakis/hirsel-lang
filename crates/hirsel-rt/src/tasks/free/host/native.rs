//! Explicit native clock, file, output and random admission. Replay derives resource positions from
//! checked creator metadata, without consulting a live adapter or host clock.
use super::*;
use std::sync::atomic::Ordering;

pub(crate) fn namespace_policy(keys: KeySet, coupled: bool) -> KeySet {
    if coupled
        && keys
            .keys()
            .iter()
            .any(|(key, _)| *key == crate::io_keys::NAMESPACE)
    {
        let mut keys = keys.keys().to_vec();
        keys.push((crate::io_keys::COUPLED, Mode::Exclusive));
        KeySet::new(keys)
    } else {
        keys
    }
}

/// # Safety
/// Sole-root admission, before native Io entries and child dispatch.
pub(crate) unsafe fn enable(ctx: *mut TaskCtx) {
    assert!(
        super::super::record::on(ctx)
            || super::super::replay::on(ctx)
            || unsafe { (*ctx).run_ref().free_io.get().is_some() }
    );
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
    validate_span(ctx, &request)?;
    let result = if super::super::replay::on(ctx) {
        let keys = super::super::replay::token(ctx)
            .unwrap()
            .host_keys(&request)?;
        unsafe { replay_request(ctx, request, keys) }?
    } else if super::super::record::on(ctx) {
        let (keys, _) = unsafe { concurrent::native_keys(ctx, &request) }?;
        unsafe { concurrent::request_with_keys(ctx, request, keys) }?
    } else {
        let (keys, blocking) = unsafe { concurrent::native_keys(ctx, &request) }?;
        unsafe { super::live::request(ctx, request, keys, blocking) }?
    };
    // The existing native caller performs its bounded quantum check after
    // delivery. Waiting attempts already released their key positions.
    Ok((result, false))
}

#[cfg(test)]
pub(in crate::tasks::free) fn replay_keys(
    handles: &handles::Handles,
    request: &Request,
) -> Stopped<KeySet> {
    request_keys_with_domains(
        request,
        |word| handles.identity(word),
        |word| handles.process_keys(word),
        |word| handles.net_ports(word),
        false,
    )
}

/// One allow-list and footprint rule for live admission and replay. Native
/// files require fully described File identities. The admitted namespace policy
/// adds its conservative domain; undeclared extras refuse before any effect.
#[cfg(test)]
pub(super) fn request_keys(
    request: &Request,
    identity: impl FnMut(u64) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)>,
) -> Stopped<KeySet> {
    request_keys_with_process(request, identity, |_| {
        Err(Stop::Diverged("missing process issue domains".into()))
    })
}

#[cfg(test)]
pub(super) fn request_keys_with_process(
    request: &Request,
    identity: impl FnMut(u64) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)>,
    process_keys: impl FnMut(u64) -> Stopped<KeySet>,
) -> Stopped<KeySet> {
    request_keys_with_domains(
        request,
        identity,
        process_keys,
        |_| Err(Stop::Diverged("missing Net port domain".into())),
        false,
    )
}

pub(in crate::tasks::free) fn request_keys_with_domains(
    request: &Request,
    mut identity: impl FnMut(u64) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)>,
    mut process_keys: impl FnMut(u64) -> Stopped<KeySet>,
    mut net_ports: impl FnMut(u64) -> Stopped<crate::io_keys::Key>,
    initial_env: bool,
) -> Stopped<KeySet> {
    use crate::io_keys::{Kind, NAMESPACE, ROOT};
    use Request as R;
    let mut identity = |word| {
        let (key, kind) = identity(word)?;
        if let Kind::Half { stream } = kind
            && (key.0 < 4 || stream.0 < 4 || key == stream)
        {
            return Err(Stop::Diverged("invalid native half parent domain".into()));
        }
        Ok((key, kind))
    };
    let mut file = |word| {
        let (key, kind) = identity(word)?;
        if kind != Kind::File {
            return Err(Stop::Diverged("native file resource kind differs".into()));
        }
        Ok(key)
    };
    let keys = match request {
        R::RunnerCall(_)
        | R::RunnerPoll { .. }
        | R::RunnerCancel { .. }
        | R::CallReturn(_)
        | R::Stopped => {
            vec![(crate::io_keys::PLACE, Mode::Exclusive)]
        }
        R::MonotonicClock { io: 1 }
        | R::WallClock { io: 1 }
        | R::Process { io: 1 }
        | R::Telemetry { io: 1 }
        | R::Signals { io: 1 }
        | R::Stdin { io: 1 }
        | R::Random { io: 1 }
        | R::Tasks { io: 1 }
        | R::Net { io: 1 }
        | R::Stdout { io: 1 }
        | R::Stderr { io: 1 } => vec![(ROOT, Mode::Shared)],
        R::EnvGet { env, .. } | R::EnvNarrow { env, .. } => {
            if *env == crate::io::ENV_HANDLE {
                if !initial_env {
                    return Err(Stop::Diverged(
                        "initial Env authority was not admitted".into(),
                    ));
                }
            } else if identity(*env)?.1 != Kind::Other {
                return Err(Stop::Diverged("native Env authority differs".into()));
            }
            Vec::new()
        }
        R::NetNarrow { net, .. } | R::Resolve { net, .. } => {
            net_ports(*net)?;
            let (key, kind) = identity(*net)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native Net resource kind differs".into()));
            }
            vec![(key, Mode::Shared)]
        }
        R::Connect { net, .. }
        | R::ConnectAddr { net, .. }
        | R::Listen { net, .. }
        | R::UdpBind { net, .. }
        | R::UnixConnect { net, .. }
        | R::UnixListen { net, .. } => {
            let mut keys = vec![(net_ports(*net)?, Mode::Exclusive)];
            match request {
                R::UnixConnect { .. } => keys.push((NAMESPACE, Mode::Shared)),
                R::UnixListen { .. } => keys.push((NAMESPACE, Mode::Exclusive)),
                _ => {}
            }
            keys
        }
        R::NetRead {
            net, stream: word, ..
        }
        | R::NetWrite {
            net, stream: word, ..
        }
        | R::NetWriteAll {
            net, stream: word, ..
        }
        | R::Shutdown {
            net, stream: word, ..
        }
        | R::SetTimeout {
            net, stream: word, ..
        }
        | R::PeerAddr { net, stream: word }
        | R::TlsConnect {
            net, stream: word, ..
        }
        | R::TlsAccept {
            net, stream: word, ..
        }
        | R::TlsPeer { net, stream: word }
        | R::TlsProtocol { net, stream: word }
        | R::TlsClose { net, stream: word }
        | R::NetSplit { net, stream: word }
        | R::UdpSend {
            net, socket: word, ..
        }
        | R::UdpRecv {
            net, socket: word, ..
        }
        | R::Accept {
            net,
            listener: word,
        }
        | R::LocalAddr {
            net,
            listener: word,
        } => {
            net_ports(*net)?;
            let (key, kind) = identity(*word)?;
            let mut keys = vec![(key, Mode::Exclusive)];
            match kind {
                Kind::Other => {}
                Kind::Half { stream } => keys.push((stream, Mode::Shared)),
                _ => {
                    return Err(Stop::Diverged(
                        "native network resource kind differs".into(),
                    ));
                }
            }
            keys
        }
        R::NetJoin { net, read, write } => {
            net_ports(*net)?;
            let mut keys = Vec::new();
            for word in [*read, *write] {
                let (key, kind) = identity(word)?;
                keys.push((key, Mode::Exclusive));
                match kind {
                    Kind::Half { stream } => keys.push((stream, Mode::Exclusive)),
                    Kind::Other => {} // TLS split endpoints alias their parent.
                    _ => return Err(Stop::Diverged("native join resource kind differs".into())),
                }
            }
            keys
        }
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
        | R::RandomBytes { random: clock, .. }
        | R::SignalWait { signals: clock, .. } => {
            let (key, kind) = identity(*clock)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native scalar resource kind differs".into()));
            }
            vec![(key, Mode::Exclusive)]
        }
        R::TelemetryBegin { telemetry, .. }
        | R::TelemetryLog { telemetry, .. }
        | R::TelemetryMeasure { telemetry, .. } => {
            let (key, kind) = identity(*telemetry)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native telemetry kind differs".into()));
            }
            vec![(key, Mode::Exclusive)]
        }
        R::TelemetryEvent {
            telemetry, span, ..
        }
        | R::TelemetryFinish {
            telemetry, span, ..
        } => {
            let provider = identity(*telemetry)?;
            if provider.1 != Kind::Other || identity(*span)? != provider {
                return Err(Stop::Diverged("native span resource differs".into()));
            }
            vec![(provider.0, Mode::Exclusive)]
        }
        R::ProcessNarrow { process, .. } => {
            let (key, kind) = identity(*process)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native process kind differs".into()));
            }
            process_keys(*process)?;
            vec![(key, Mode::Shared)]
        }
        R::ProcessRun {
            process,
            command,
            deadline,
            ..
        } => {
            if let Some(directory) = command.directory {
                file(directory)?;
            }
            if let Some(deadline) = deadline {
                identity(deadline.clock)?;
            }
            return process_keys(*process);
        }
        R::ProcessSpawn { process, command } => {
            if let Some(directory) = command.directory {
                file(directory)?;
            }
            return process_keys(*process);
        }
        R::ProcessPipeline {
            process,
            stages,
            deadline,
            ..
        } => {
            for command in stages {
                if let Some(directory) = command.directory {
                    file(directory)?;
                }
            }
            if let Some(deadline) = deadline {
                identity(deadline.clock)?;
            }
            return process_keys(*process);
        }
        R::ProcessSpawnPipeline {
            process, stages, ..
        } => {
            for command in stages {
                if let Some(directory) = command.directory {
                    file(directory)?;
                }
            }
            return process_keys(*process);
        }
        R::ChildWrite { process, child, .. }
        | R::ChildRead { process, child, .. }
        | R::ChildCloseStdin { process, child }
        | R::ChildWait { process, child, .. }
        | R::ChildKill { process, child, .. } => {
            process_keys(*process)?;
            let (key, kind) = identity(*child)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native child kind differs".into()));
            }
            vec![(key, Mode::Exclusive)]
        }
        R::GroupWait { process, group, .. } => {
            process_keys(*process)?;
            let (key, kind) = identity(*group)?;
            if kind != Kind::Other {
                return Err(Stop::Diverged("native group kind differs".into()));
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
        | R::ReadDirPage { directory, .. }
        | R::Metadata { directory, .. } => {
            file(*directory)?;
            vec![(NAMESPACE, Mode::Shared)]
        }
        R::OpenAppend { directory, .. }
        | R::Create { directory, .. }
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
        R::Close { handle } => {
            let (key, kind) = identity(*handle)?;
            match kind {
                Kind::File => vec![(key, Mode::Exclusive), (NAMESPACE, Mode::Shared)],
                Kind::Other => vec![(key, Mode::Exclusive)],
                Kind::Half { stream } => vec![(key, Mode::Exclusive), (stream, Mode::Shared)],
                _ => return Err(Stop::Diverged("unadmitted native close resource".into())),
            }
        }
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

#[derive(Default)]
pub(crate) struct Spans(std::sync::Mutex<SpanState>);
#[derive(Default)]
pub(crate) struct SpanState {
    issued: crate::telemetry::Issued,
    rows: std::collections::BTreeMap<u64, (u64, Vec<u8>)>,
}

fn validate_span(ctx: *mut TaskCtx, request: &Request) -> Stopped<()> {
    if let Request::TelemetryEvent {
        telemetry, span, ..
    }
    | Request::TelemetryFinish {
        telemetry, span, ..
    } = request
        && !span_valid(ctx, *telemetry, *span)
    {
        return Err(Stop::Diverged("unknown native telemetry span owner".into()));
    }
    Ok(())
}

pub(crate) fn span_valid(ctx: *mut TaskCtx, telemetry: u64, span: u64) -> bool {
    unsafe { (*ctx).run_ref() }
        .free_native_spans
        .0
        .lock()
        .unwrap()
        .valid(telemetry, span)
}
pub(crate) fn span_context(ctx: *mut TaskCtx, telemetry: u64, span: u64) -> Option<Vec<u8>> {
    unsafe { (*ctx).run_ref() }
        .free_native_spans
        .0
        .lock()
        .unwrap()
        .context(telemetry, span)
}
impl SpanState {
    pub(super) fn valid(&self, telemetry: u64, span: u64) -> bool {
        self.rows
            .get(&span)
            .is_some_and(|(owner, _)| *owner == telemetry)
    }
    pub(super) fn context(&self, telemetry: u64, span: u64) -> Option<Vec<u8>> {
        self.rows
            .get(&span)
            .filter(|(owner, _)| *owner == telemetry)
            .map(|(_, bytes)| bytes.clone())
    }
    pub(super) fn observe(
        &mut self,
        request: &Request,
        completion: &Result<Completion, AdapterError>,
    ) -> Stopped<()> {
        match crate::runtime_roots::telemetry() {
            Some(provider) => (provider.observe_span)(self, request, completion),
            None => Ok(()),
        }
    }

    pub(crate) fn observe_linked(
        &mut self,
        request: &Request,
        completion: &Result<Completion, AdapterError>,
    ) -> Stopped<()> {
        match (request, completion) {
            (
                Request::TelemetryBegin {
                    telemetry, parent, ..
                },
                Ok(Completion::Span {
                    handle,
                    trace,
                    span,
                }),
            ) => {
                let parent_trace = crate::telemetry::kernel::parent(parent)
                    .map_err(|_| Stop::Diverged("invalid telemetry parent".into()))?;
                if self.rows.contains_key(handle) || !self.issued.admits(parent_trace, trace, span)
                {
                    return Err(Stop::Diverged(
                        "invalid or repeated native span identity".into(),
                    ));
                }
                self.issued.accept(parent_trace, *trace, *span);
                self.rows.insert(
                    *handle,
                    (
                        *telemetry,
                        crate::telemetry::kernel::context(trace, span, parent),
                    ),
                );
            }
            (
                Request::TelemetryFinish { span, .. } | Request::Close { handle: span },
                Ok(Completion::Unit(Ok(()))),
            ) => {
                self.rows.remove(span);
            }
            _ => {}
        }
        Ok(())
    }
}
/// Apply derived span state once per Ended, using the same initialized
/// completion and parent bytes live and in replay. No host query or phase.
pub(super) fn observe_span(
    ctx: *mut TaskCtx,
    request: &Request,
    completion: &Result<Completion, AdapterError>,
) -> Stopped<()> {
    if !enabled(ctx) {
        return Ok(());
    }
    unsafe { (*ctx).run_ref() }
        .free_native_spans
        .0
        .lock()
        .unwrap()
        .observe(request, completion)
}

#[cfg(test)]
pub(crate) fn test_net_creations(bytes: &[u8]) -> Vec<(u32, u32)> {
    let envelope: handles::Envelope = crate::tape_wire::decode(bytes).unwrap();
    envelope
        .created
        .iter()
        .filter_map(|c| c.net_ports.map(|ports| (c.resource, ports)))
        .collect()
}
