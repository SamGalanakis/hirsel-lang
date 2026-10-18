//! The host process provider (docs/spec/core-library.md#processes): one-shot runs, and
//! streaming children spawned with three nonblocking pipes. Without a cooperative
//! scheduler a child operation that would block polls its one descriptor. Under one it
//! records that descriptor as the entered token's interest and parks, and `child.wait`
//! runs on a [`HostJob`](crate::process::HostJob), so the task parks instead of blocking
//! the thread.
use crate::io::{AdapterError, Completion};
use crate::process::{ChildStream, Ended, ExitStatus, HostCall, PipelineOutcome, ProcessFailure};
use crate::readiness::Readiness;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};

/// One running child and the program's ends of its pipes.
struct HostChild {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
}
/// Dropping a Child closes its input, sends SIGKILL, reaps it, then closes its outputs
/// (the fields drop after this body).
impl Drop for HostChild {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The host children of one adapter, by Child handle.
#[derive(Default)]
pub(crate) struct HostChildren(BTreeMap<u64, HostChild>);

fn nonblocking(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: fd is a live pipe descriptor owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Park the entered token on `fd`, or wait for it without a scheduler.
fn would_block(
    readiness: Option<&Readiness>,
    fd: RawFd,
    events: libc::c_short,
) -> Result<(), AdapterError> {
    if let Some(readiness) = readiness {
        readiness.pending().interest = Some((fd, events));
        return Err(AdapterError::Blocked);
    }
    let mut row = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    loop {
        // SAFETY: row holds a live descriptor owned by the caller.
        if unsafe { libc::poll(&mut row, 1, -1) } >= 0 {
            return Ok(());
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(AdapterError::Unsupported("child pipe poll".into()));
        }
    }
}

fn exit_status(status: std::process::ExitStatus) -> Option<ExitStatus> {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => Some(ExitStatus::Exited(code)),
        (None, Some(signal)) => Some(ExitStatus::Signaled(signal)),
        (None, None) => None,
    }
}

impl HostChildren {
    /// Perform one checked call. `cwd` resolves a Dir handle to its pinned directory
    /// descriptor; `readiness` is the cooperative scheduler's, if any.
    pub(crate) fn call(
        &mut self,
        call: HostCall<'_>,
        cwd: &dyn Fn(u64) -> Option<RawFd>,
        readiness: Option<&Readiness>,
    ) -> Result<Completion, AdapterError> {
        let directory = |d: Option<u64>| match d {
            None => Ok(None),
            Some(d) => cwd(d).map(Some).ok_or(ProcessFailure::InvalidInput),
        };
        Ok(match call {
            HostCall::Run(run) => {
                Completion::Process(Self::run(run, &directory, readiness)?.into_run())
            }
            HostCall::Pipeline(run) => Completion::Pipeline(Self::run(run, &directory, readiness)?),
            HostCall::Spawn { child, spec } => Completion::Spawned(
                directory(spec.directory)
                    .and_then(|cwd| self.spawn(child, spec, cwd))
                    .map(|()| child),
            ),
            HostCall::Write { child, data } => {
                Completion::ChildUnit(self.write(child, data, readiness)?)
            }
            HostCall::CloseStdin { child } => {
                self.child(child)?.stdin = None;
                Completion::ChildUnit(Ok(()))
            }
            HostCall::Read { child, stream, max } => {
                Completion::ChildBytes(self.read(child, stream, max, readiness)?)
            }
            HostCall::Wait { child, deadline } => {
                Completion::ChildExit(self.wait(child, deadline, readiness)?)
            }
            // The child is unreaped until its wait or drop, so its pid still names it.
            HostCall::Kill { child, signal } => {
                let pid = self.child(child)?.child.id() as libc::pid_t;
                // SAFETY: kill(2) on a live child's pid with a valid signal number.
                unsafe { libc::kill(pid, signal.number()) };
                Completion::ChildUnit(Ok(()))
            }
            // A child a cancelled wait took was dropped with its job already.
            HostCall::Close { child } => {
                self.0.remove(&child);
                Completion::Unit(Ok(()))
            }
        })
    }
    /// A run or pipeline: under cooperative admission it parks its task instead of
    /// blocking the thread, and a cancellation kills its children.
    fn run(
        run: &crate::process::HostRun<'_>,
        directory: &dyn Fn(Option<u64>) -> Result<Option<RawFd>, ProcessFailure>,
        readiness: Option<&Readiness>,
    ) -> Result<PipelineOutcome, AdapterError> {
        let cwds = match run
            .stages
            .iter()
            .map(|s| directory(s.directory))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(cwds) => cwds,
            Err(failure) => return Ok(PipelineOutcome::Error(failure)),
        };
        Ok(match readiness {
            Some(readiness) => crate::process::run_cooperative(readiness, run, &cwds)?,
            None => crate::process::run_host(run, &cwds, None),
        })
    }
    fn child(&mut self, id: u64) -> Result<&mut HostChild, AdapterError> {
        self.0.get_mut(&id).ok_or(AdapterError::InvalidHandle(id))
    }
    /// Spawn one checked program as Child `id`, starting in `cwd` or `/`.
    fn spawn(
        &mut self,
        id: u64,
        spec: &crate::process::ProcessSpec<'_>,
        cwd: Option<RawFd>,
    ) -> Result<(), ProcessFailure> {
        let mut child = crate::process::host_command(spec, cwd)
            .spawn()
            .map_err(|e| crate::process::spawn_failure(&e))?;
        let child = HostChild {
            stdin: child.stdin.take(),
            stdout: child.stdout.take().expect("piped stdout"),
            stderr: child.stderr.take().expect("piped stderr"),
            child,
        };
        let stdin = child.stdin.as_ref().expect("piped stdin").as_raw_fd();
        for fd in [stdin, child.stdout.as_raw_fd(), child.stderr.as_raw_fd()] {
            if nonblocking(fd).is_err() {
                return Err(ProcessFailure::Fault(1));
            }
        }
        self.0.insert(id, child);
        Ok(())
    }
    /// Write all of nonempty `data`, from the offset a parked attempt reached. Any
    /// accepted byte commits the write, which is then no cancellation point.
    fn write(
        &mut self,
        id: u64,
        data: &[u8],
        readiness: Option<&Readiness>,
    ) -> Result<Result<(), ProcessFailure>, AdapterError> {
        let child = self.child(id)?;
        let Some(stdin) = &mut child.stdin else {
            return Err(AdapterError::InvalidCompletion);
        };
        let mut written = readiness.map_or(0, |r| r.pending().offset);
        let result = crate::process::without_sigpipe(|| {
            loop {
                if written == data.len() {
                    return Ok(Ok(()));
                }
                match stdin.write(&data[written..]) {
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if let Some(r) = readiness {
                            let mut pending = r.pending();
                            pending.offset = written;
                            pending.committed |= written > 0;
                        }
                        would_block(readiness, stdin.as_raw_fd(), libc::POLLOUT)?;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                        return Ok(Err(ProcessFailure::InputClosed));
                    }
                    Err(_) => return Ok(Err(ProcessFailure::Fault(1))),
                }
            }
        });
        if let (Some(r), Ok(_)) = (readiness, &result) {
            r.pending().offset = 0;
        }
        result
    }
    /// Up to `max` (positive) bytes of `stream` as soon as any are available; none at end
    /// of stream.
    fn read(
        &mut self,
        id: u64,
        stream: ChildStream,
        max: u64,
        readiness: Option<&Readiness>,
    ) -> Result<Result<Vec<u8>, ProcessFailure>, AdapterError> {
        let child = self.child(id)?;
        let (fd, pipe): (_, &mut dyn Read) = match stream {
            ChildStream::Stdout => (child.stdout.as_raw_fd(), &mut child.stdout),
            ChildStream::Stderr => (child.stderr.as_raw_fd(), &mut child.stderr),
        };
        let mut bytes = vec![0; usize::try_from(max).unwrap_or(usize::MAX).min(64 * 1024)];
        loop {
            match pipe.read(&mut bytes) {
                Ok(n) => {
                    bytes.truncate(n);
                    return Ok(Ok(bytes));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    would_block(readiness, fd, libc::POLLIN)?;
                }
                Err(_) => return Ok(Err(ProcessFailure::Fault(1))),
            }
        }
    }
    /// `child.wait`: under a scheduler it runs as the entered token's job, which the child
    /// moves into.
    fn wait(
        &mut self,
        id: u64,
        deadline: Option<u64>,
        readiness: Option<&Readiness>,
    ) -> Result<Result<ExitStatus, ProcessFailure>, AdapterError> {
        let outcome = match readiness {
            None => finish(
                self.0.remove(&id).ok_or(AdapterError::InvalidHandle(id))?,
                None,
                deadline,
            ),
            Some(readiness) => {
                let children = &mut self.0;
                crate::process::cooperative_job(readiness, || {
                    let child = children.remove(&id)?;
                    Some(Box::new(move |cancel| {
                        finish(child, Some(cancel), deadline)
                    }))
                })?
            }
        };
        Ok(match outcome {
            PipelineOutcome::Output { statuses, .. } if statuses.len() == 1 => Ok(statuses[0]),
            PipelineOutcome::Output { .. } => Err(ProcessFailure::Fault(1)),
            PipelineOutcome::Error(failure) => Err(failure),
        })
    }
}

/// Close input, read and discard both outputs to end of file, then reap. A readable
/// `cancel` drops the child instead, which kills it. At `deadline` the child gets
/// SIGTERM, then SIGKILL after the grace, and is reaped: `TimedOut`.
fn finish(mut child: HostChild, cancel: Option<RawFd>, deadline: Option<u64>) -> PipelineOutcome {
    let timed_out = |child: &mut HostChild| {
        crate::process::terminate(std::slice::from_mut(&mut child.child), cancel);
        PipelineOutcome::Error(ProcessFailure::TimedOut)
    };
    child.stdin = None;
    let mut open = [true, true];
    let mut chunk = vec![0u8; 64 * 1024];
    while open[0] || open[1] {
        let mut fds = [
            libc::pollfd {
                fd: if open[0] {
                    child.stdout.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if open[1] {
                    child.stderr.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: cancel.unwrap_or(-1),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        if deadline.is_some_and(|d| crate::process::monotonic_now() >= d) {
            return timed_out(&mut child);
        }
        let timeout = crate::process::poll_timeout(deadline);
        // SAFETY: fds is a live array of three pollfd records.
        if unsafe { libc::poll(fds.as_mut_ptr(), 3, timeout) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return PipelineOutcome::Error(ProcessFailure::Fault(1));
        }
        if fds[2].revents != 0 {
            return PipelineOutcome::Error(ProcessFailure::Canceled(0));
        }
        let HostChild { stdout, stderr, .. } = &mut child;
        for (i, pipe) in [stdout as &mut dyn Read, stderr as &mut dyn Read]
            .into_iter()
            .enumerate()
        {
            if fds[i].revents == 0 {
                continue;
            }
            match pipe.read(&mut chunk) {
                Ok(0) => open[i] = false,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return PipelineOutcome::Error(ProcessFailure::Fault(1)),
            }
        }
    }
    // A reaped child's Drop signals nothing: std keeps its status.
    match crate::process::wait_child(&mut child.child, deadline, cancel) {
        Ok(Ended::Exited(status)) => match exit_status(status) {
            Some(status) => PipelineOutcome::Output {
                statuses: vec![status],
                stdout: vec![],
                stderr: vec![],
            },
            None => PipelineOutcome::Error(ProcessFailure::Fault(1)),
        },
        Ok(Ended::Canceled) => PipelineOutcome::Error(ProcessFailure::Canceled(0)),
        Ok(Ended::Expired) => timed_out(&mut child),
        Err(_) => PipelineOutcome::Error(ProcessFailure::Fault(1)),
    }
}
