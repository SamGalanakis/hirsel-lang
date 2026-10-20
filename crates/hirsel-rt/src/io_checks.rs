//! Effect-free program checks shared by live, replay and modeled cancellation (CONC-3).
use crate::handles::Grant;
use crate::io::{
    Adapter, AdapterError, Completion as C, InputError, IoError, Request as R, State, WaitError,
};
use crate::net::NetError;
use crate::process::{ProcessFailure, ProcessOutcome};
use hirsel_run_schema::Access;

pub(crate) fn net_completion(request: &R, error: NetError) -> C {
    match request {
        R::Resolve { .. } => C::Addresses(Err(error)),
        R::UdpBind { .. }
        | R::Connect { .. }
        | R::ConnectAddr { .. }
        | R::UnixConnect { .. }
        | R::NetJoin { .. } => C::NetHandle(Err(error)),
        R::NetSplit { .. } => C::Halves(Err(error)),
        R::UnixListen { .. } => C::Listened(Err(error)),
        R::Accept { .. } => C::Accepted(Err(error)),
        R::NetRead { .. } => C::NetBytes(Err(error)),
        R::UdpRecv { .. } => C::Datagram(Err(error)),
        R::UdpSend { .. } | R::NetWrite { .. } => C::Count(Err(error)),
        R::PeerAddr { .. } => C::Address(Err(error)),
        R::TlsPeer { .. } => C::Certificates(Err(error)),
        R::TlsConnect { .. } | R::TlsAccept { .. } | R::TlsProtocol { .. } => {
            C::Protocol(Err(error))
        }
        _ => C::NetUnit(Err(error)),
    }
}

impl State {
    /// Only argument and authority errors, without consuming a handle or provider input.
    pub(crate) fn preflight(&self, request: &R) -> Result<Option<C>, AdapterError> {
        if crate::net::is_net_request(request) {
            return Ok(self.net_error(request)?.map(|e| net_completion(request, e)));
        }
        if crate::process::is_process_request(request) {
            return self.process_error(request);
        }
        match request {
            R::ReadStdin { stdin, max } => {
                self.expect(*stdin, Grant::Stdin)?;
                Ok((*max == 0).then_some(C::Input(Err(InputError::InvalidInput))))
            }
            R::Print { stdout, .. } => {
                self.expect(*stdout, Grant::Stdout)?;
                Ok(None)
            }
            R::PrintStderr { stderr, .. } => {
                self.expect(*stderr, Grant::Stderr)?;
                Ok(None)
            }
            R::SleepUntil { clock, .. } => {
                self.expect(*clock, Grant::MonotonicClock)?;
                Ok(None)
            }
            _ => Ok(self
                .file_error(request)?
                .map(|e| file_completion(request, e))),
        }
    }

    fn file_error(&self, request: &R) -> Result<Option<IoError>, AdapterError> {
        let directory = match request {
            R::Open { directory, .. }
            | R::Create { directory, .. }
            | R::FileRead { directory, .. }
            | R::FileWrite { directory, .. }
            | R::FileAppend { directory, .. }
            | R::FileSeek { directory, .. }
            | R::Read { directory, .. }
            | R::Write { directory, .. }
            | R::Size { directory, .. }
            | R::Walk { directory }
            | R::ReadDir { directory, .. }
            | R::Metadata { directory, .. }
            | R::Remove { directory, .. }
            | R::Mkdir { directory, .. }
            | R::RemoveDir { directory, .. }
            | R::RemoveAll { directory, .. }
            | R::SetPermissions { directory, .. }
            | R::Rename { directory, .. }
            | R::CreateAtomic { directory, .. }
            | R::WriteAll { directory, .. }
            | R::Commit { directory, .. }
            | R::Sync { directory, .. }
            | R::PutNew { directory, .. } => *directory,
            R::Map { directory, .. }
            | R::MapView { directory, .. }
            | R::MapCheck { directory, .. }
            | R::Watch { directory, .. }
            | R::WatchNext { directory, .. } => *directory,
            _ => return Ok(None),
        };
        let root = self.directory(directory)?;
        // A forged or foreign stream is a tool error before any language result,
        // including denial. The interpreter checks this same boundary first.
        let stream_writable = match request {
            R::FileRead { file, .. }
            | R::FileWrite { file, .. }
            | R::FileAppend { file, .. }
            | R::FileSeek { file, .. }
            | R::Sync { file, .. } => {
                let Some(Grant::File {
                    directory: owner,
                    writable,
                    ..
                }) = self.handles.rows.get(file)
                else {
                    return Err(AdapterError::InvalidHandle(*file));
                };
                if *owner != directory {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                Some(*writable)
            }
            R::WriteAll { file, .. } | R::Commit { file, .. } => {
                if !matches!(self.handles.rows.get(file), Some(Grant::AtomicFile { directory: owner, .. }) if *owner == directory)
                {
                    return Err(AdapterError::InvalidHandle(*file));
                }
                None
            }
            R::MapView { mapped, .. } | R::MapCheck { mapped, .. } => {
                if !matches!(self.handles.rows.get(mapped), Some(Grant::Mapped { directory: owner, .. }) if *owner == directory)
                {
                    return Err(AdapterError::InvalidHandle(*mapped));
                }
                None
            }
            R::WatchNext { watcher, .. } => {
                if !matches!(self.handles.rows.get(watcher), Some(Grant::Watcher { directory: owner, .. }) if *owner == directory)
                {
                    return Err(AdapterError::InvalidHandle(*watcher));
                }
                None
            }
            _ => None,
        };
        if self.access(directory)? == Access::Read
            && matches!(
                request,
                R::Create { .. }
                    | R::Write { .. }
                    | R::FileWrite { .. }
                    | R::FileAppend { .. }
                    | R::Remove { .. }
                    | R::Mkdir { .. }
                    | R::RemoveDir { .. }
                    | R::RemoveAll { .. }
                    | R::SetPermissions { .. }
                    | R::Rename { .. }
                    | R::CreateAtomic { .. }
                    | R::WriteAll { .. }
                    | R::Commit { .. }
                    | R::PutNew { .. }
            )
        {
            return Ok(Some(IoError::NotGranted));
        }
        let invalid = match request {
            R::FileRead { .. }
            | R::FileWrite { .. }
            | R::FileAppend { .. }
            | R::FileSeek { .. }
            | R::Sync { .. } => {
                let writable = stream_writable.expect("stream ownership checked");
                matches!(request, R::FileRead { .. }) && writable
                    || matches!(request, R::FileWrite { .. } | R::FileAppend { .. }) && !writable
                    || matches!(request, R::FileSeek { offset, .. } if *offset > i64::MAX as u64)
            }
            R::SetPermissions {
                path, permissions, ..
            } => {
                *permissions > 0o777 || hirsel_run_schema::confined_relative(&root, path).is_none()
            }
            R::RemoveAll { path, .. } => {
                hirsel_run_schema::confined_relative(&root, path).is_none_or(|p| p == root)
            }
            R::Open { path, .. }
            | R::Create { path, .. }
            | R::Read { path, .. }
            | R::Write { path, .. }
            | R::Size { path, .. }
            | R::ReadDir { path, .. }
            | R::Metadata { path, .. }
            | R::Remove { path, .. }
            | R::Mkdir { path, .. }
            | R::RemoveDir { path, .. }
            | R::CreateAtomic { path, .. }
            | R::PutNew { path, .. }
            | R::Map { path, .. }
            | R::Watch { path, .. } => hirsel_run_schema::confined_relative(&root, path).is_none(),
            R::Rename { from, to, .. } => {
                hirsel_run_schema::confined_relative(&root, from).is_none()
                    || hirsel_run_schema::confined_relative(&root, to).is_none()
            }
            _ => false,
        };
        Ok(invalid.then_some(IoError::Fault(1)))
    }
}

fn file_completion(request: &R, error: IoError) -> C {
    match request {
        R::Open { .. }
        | R::Create { .. }
        | R::CreateAtomic { .. }
        | R::Map { .. }
        | R::Watch { .. } => C::Handle(Err(error)),
        R::WatchNext { .. } => C::Watch(Err(error)),
        R::FileRead { .. } | R::Read { .. } => C::Bytes(Err(error)),
        R::FileWrite { .. } | R::Size { .. } => C::Size(Err(error)),
        R::ReadDir { .. } => C::Entries(Err(error)),
        R::Metadata { .. } => C::Metadata(Err(error)),
        R::Walk { .. } => C::Paths(Err(error)),
        _ => C::Unit(Err(error)),
    }
}

/// Complete a cancellation point after program checks, without issuing its effect.
/// Consumed resources still close on every outcome, including a program error.
pub fn cancel(
    adapter: &mut dyn Adapter,
    request: &R,
    cause: u32,
) -> Result<Option<C>, AdapterError> {
    let canceled = match request {
        R::ReadStdin { .. } => C::Input(Err(InputError::Canceled(cause))),
        R::Print { .. } | R::PrintStderr { .. } => C::Unit(Err(IoError::Canceled(cause))),
        R::SleepUntil { .. } => C::Wait(Err(WaitError::Canceled(cause))),
        R::ProcessRun { .. } => C::Process(ProcessOutcome::Error(ProcessFailure::Canceled(cause))),
        R::ProcessPipeline { .. } => C::Pipeline(hirsel_run_schema::PipelineOutcome::Error(
            ProcessFailure::Canceled(cause),
        )),
        R::ProcessSpawn { .. } | R::ProcessSpawnPipeline { .. } => {
            C::Spawned(Err(ProcessFailure::Canceled(cause)))
        }
        R::ChildWrite { .. } => C::ChildUnit(Err(ProcessFailure::Canceled(cause))),
        R::ChildRead { .. } => C::ChildBytes(Err(ProcessFailure::Canceled(cause))),
        R::ChildWait { .. } => C::ChildExit(Err(ProcessFailure::Canceled(cause))),
        R::GroupWait { .. } => C::GroupExit(Err(ProcessFailure::Canceled(cause))),
        R::UdpBind { .. }
        | R::UdpSend { .. }
        | R::UdpRecv { .. }
        | R::Resolve { .. }
        | R::Connect { .. }
        | R::ConnectAddr { .. }
        | R::UnixConnect { .. }
        | R::Accept { .. }
        | R::NetRead { .. }
        | R::NetWrite { .. }
        | R::NetWriteAll { .. }
        | R::TlsConnect { .. }
        | R::TlsAccept { .. }
        | R::TlsClose { .. } => net_completion(request, NetError::Canceled(cause)),
        R::Open { .. }
        | R::Create { .. }
        | R::FileRead { .. }
        | R::FileWrite { .. }
        | R::FileAppend { .. }
        | R::FileSeek { .. }
        | R::Read { .. }
        | R::Write { .. }
        | R::Size { .. }
        | R::Walk { .. }
        | R::ReadDir { .. }
        | R::Metadata { .. }
        | R::Remove { .. }
        | R::Mkdir { .. }
        | R::RemoveDir { .. }
        | R::RemoveAll { .. }
        | R::SetPermissions { .. }
        | R::Rename { .. }
        | R::CreateAtomic { .. }
        | R::WriteAll { .. }
        | R::Commit { .. }
        | R::Sync { .. }
        | R::PutNew { .. }
        | R::Map { .. }
        | R::Watch { .. }
        | R::WatchNext { .. } => file_completion(request, IoError::Canceled(cause)),
        _ => return Ok(None),
    };
    let completion = adapter.preflight(request)?.unwrap_or(canceled);
    let taken = match request {
        R::TlsConnect { stream, .. } | R::TlsAccept { stream, .. } | R::TlsClose { stream, .. } => {
            Some(*stream)
        }
        R::ChildWait { child, .. } | R::GroupWait { group: child, .. } => Some(*child),
        R::Commit { file, .. } => Some(*file),
        _ => None,
    };
    if let Some(handle) = taken {
        match adapter.call(R::Close { handle })? {
            C::Unit(Ok(())) => {}
            C::Checked(false) => return Ok(Some(C::Checked(false))),
            _ => return Err(AdapterError::InvalidCompletion),
        }
    }
    Ok(Some(completion))
}
