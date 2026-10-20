//! The host process provider (docs/spec/core-library.md#processes): one-shot runs,
//! streaming children spawned with three nonblocking pipes, and spawned pipelines whose
//! stages' standard error a [`Capture`] thread drains. Without a cooperative scheduler a
//! child operation that would block polls its one descriptor. Under one it records that
//! descriptor as the entered token's interest and parks, and a wait runs on a
//! [`HostJob`](crate::process::HostJob), so the task parks instead of blocking the thread.
use crate::host_stages::{Ended, Stages, Targets};
use crate::io::{AdapterError, Completion};
use crate::process::{
    ChildStream, ExitStatus, GroupOutput, HostCall, JobOutcome, PipelineOutcome, ProcessFailure,
};
use crate::readiness::Readiness;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};

/// One running child, or the stages of one spawned pipeline, and the program's ends of
/// their pipes: the first stage's input and the last stage's output.
struct HostChild {
    /// Every stage in stage order; a spawned child is one. No stage is reaped before its
    /// wait or drop.
    children: Stages,
    stdin: Option<std::process::ChildStdin>,
    stdout: std::process::ChildStdout,
    stderr: Errors,
}
/// A child's standard error, which the program reads, or a pipeline's, captured per stage.
enum Errors {
    Pipe(std::process::ChildStderr),
    Captured(Capture),
}
/// Dropping a Child or ChildGroup closes its input and stops its capture; then its
/// stages drop, which SIGKILLs and reaps them, then its outputs close (the fields drop
/// after this body, in order).
impl Drop for HostChild {
    fn drop(&mut self) {
        self.stdin = None;
        if let Errors::Captured(capture) = &mut self.stderr {
            capture.stop();
        }
    }
}

/// The standard error of every stage of a spawned pipeline, drained on a thread of its own
/// from the spawn until each stage closes it, so no stage waits on it. A stage past the
/// bound makes the thread SIGKILL every stage and the group. A stage is reaped only after
/// the thread ended (its `done` end reads end of file), so every pid it signals still
/// names its stage.
struct Capture {
    stop: std::os::unix::net::UnixStream,
    done: std::os::unix::net::UnixStream,
    thread: Option<std::thread::JoinHandle<(Vec<Vec<u8>>, bool)>>,
}
impl Capture {
    /// Drain `pipes`, one per stage, each bounded by `max` bytes; past it, SIGKILL
    /// `targets`.
    fn start(
        pipes: Vec<std::process::ChildStderr>,
        targets: Targets,
        max: u64,
    ) -> std::io::Result<Self> {
        use std::os::unix::net::UnixStream;
        let (stop, stopped) = UnixStream::pair()?;
        let (done, ended) = UnixStream::pair()?;
        for pipe in &pipes {
            nonblocking(pipe.as_raw_fd())?;
        }
        let thread = std::thread::Builder::new()
            .name("hirsel-stderr".into())
            .spawn(move || {
                let captured = drain_stages(pipes, &targets, max, stopped.as_raw_fd());
                drop(ended);
                captured
            })?;
        Ok(Self {
            stop,
            done,
            thread: Some(thread),
        })
    }
    /// Stop draining and join the thread.
    fn stop(&mut self) {
        let _ = (&self.stop).write_all(&[1]);
        self.join();
    }
    /// Each stage's captured bytes and whether one passed the bound, once the thread ended.
    fn join(&mut self) -> (Vec<Vec<u8>>, bool) {
        self.thread
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default()
    }
}

/// Read every pipe to end of file, or until `stop` is readable, each into its own buffer;
/// once one passes `max` bytes every stage gets SIGKILL and draining stops, since a
/// process a stage started may hold its pipe open.
fn drain_stages(
    mut pipes: Vec<std::process::ChildStderr>,
    targets: &Targets,
    max: u64,
    stop: RawFd,
) -> (Vec<Vec<u8>>, bool) {
    let mut buffers = vec![vec![]; pipes.len()];
    let mut open = vec![true; pipes.len()];
    let mut chunk = vec![0u8; 64 * 1024];
    while open.iter().any(|o| *o) {
        let mut fds: Vec<libc::pollfd> = pipes
            .iter()
            .zip(&open)
            .map(|(pipe, open)| libc::pollfd {
                fd: if *open { pipe.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        fds.push(libc::pollfd {
            fd: stop,
            events: libc::POLLIN,
            revents: 0,
        });
        // SAFETY: fds is a live array of pollfd records.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if fds[pipes.len()].revents != 0 {
            break;
        }
        for (i, pipe) in pipes.iter_mut().enumerate() {
            if fds[i].revents == 0 {
                continue;
            }
            match pipe.read(&mut chunk) {
                Ok(0) => open[i] = false,
                Ok(n) => {
                    buffers[i].extend_from_slice(&chunk[..n]);
                    if buffers[i].len() as u64 > max {
                        // No stage is reaped before this thread ends.
                        targets.signal(libc::SIGKILL);
                        return (buffers, true);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => open[i] = false,
            }
        }
    }
    (buffers, false)
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

impl crate::process::ProcessHost for HostChildren {
    fn call(
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
            HostCall::SpawnPipeline {
                group,
                stages,
                max_stderr,
            } => Completion::Spawned(
                stages
                    .iter()
                    .map(|s| directory(s.directory))
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|cwds| self.spawn_pipeline(group, stages, &cwds, max_stderr))
                    .map(|()| group),
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
                Completion::ChildExit(match self.wait(child, deadline, readiness)? {
                    JobOutcome::Run(PipelineOutcome::Output { statuses, .. })
                        if statuses.len() == 1 =>
                    {
                        Ok(statuses[0])
                    }
                    JobOutcome::Run(PipelineOutcome::Error(failure)) => Err(failure),
                    _ => Err(ProcessFailure::Fault(1)),
                })
            }
            HostCall::WaitGroup { group, deadline } => {
                Completion::GroupExit(match self.wait(group, deadline, readiness)? {
                    JobOutcome::Group(outcome) => outcome,
                    JobOutcome::Run(PipelineOutcome::Error(failure)) => Err(failure),
                    JobOutcome::Run(_) => Err(ProcessFailure::Fault(1)),
                })
            }
            HostCall::Kill { child, signal } => {
                self.child(child)?.children.signal(signal.number());
                Completion::ChildUnit(Ok(()))
            }
            // A child a cancelled wait took was dropped with its job already.
            HostCall::Close { child } => {
                self.0.remove(&child);
                Completion::Unit(Ok(()))
            }
        })
    }
}

impl HostChildren {
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
        let mut children = Stages::new(spec.own_group);
        let child = children
            .spawn(&mut crate::process::host_command(spec, cwd))
            .map_err(|e| crate::process::spawn_failure(&e))?;
        let (stdin, stdout) = (
            child.stdin.take(),
            child.stdout.take().expect("piped stdout"),
        );
        let stderr = child.stderr.take().expect("piped stderr");
        if nonblocking(stderr.as_raw_fd()).is_err() {
            return Err(ProcessFailure::Fault(1));
        }
        let child = HostChild {
            children,
            stdin,
            stdout,
            stderr: Errors::Pipe(stderr),
        };
        self.insert(id, child)
    }
    /// Make the program's ends nonblocking and keep the child as handle `id`.
    fn insert(&mut self, id: u64, child: HostChild) -> Result<(), ProcessFailure> {
        let stdin = child.stdin.as_ref().expect("piped stdin").as_raw_fd();
        for fd in [stdin, child.stdout.as_raw_fd()] {
            if nonblocking(fd).is_err() {
                return Err(ProcessFailure::Fault(1));
            }
        }
        self.0.insert(id, child);
        Ok(())
    }
    /// Spawn checked stages as ChildGroup `id`, stage `i` starting in `cwds[i]` or `/`:
    /// each stage's standard output feeds the next through a pipe the program never sees,
    /// and each stage's standard error goes to its own pipe, which a [`Capture`] drains. A
    /// stage that cannot start kills and reaps the stages before it (they drop).
    fn spawn_pipeline(
        &mut self,
        id: u64,
        stages: &[crate::process::ProcessSpec<'_>],
        cwds: &[Option<RawFd>],
        max_stderr: u64,
    ) -> Result<(), ProcessFailure> {
        let mut children = Stages::new(stages[0].own_group);
        let mut stdin = None;
        let mut pipes = vec![];
        let mut previous: Option<std::process::ChildStdout> = None;
        for (spec, cwd) in stages.iter().zip(cwds) {
            let mut command = crate::process::host_command(spec, *cwd);
            if let Some(output) = previous.take() {
                command.stdin(std::process::Stdio::from(output));
            }
            let child = children
                .spawn(&mut command)
                .map_err(|e| crate::process::spawn_failure(&e))?;
            if stdin.is_none() {
                stdin = child.stdin.take();
            }
            previous = child.stdout.take();
            pipes.push(child.stderr.take().expect("piped stderr"));
        }
        let Ok(capture) = Capture::start(pipes, children.targets(), max_stderr) else {
            return Err(ProcessFailure::Fault(1));
        };
        self.insert(
            id,
            HostChild {
                children,
                stdin,
                stdout: previous.expect("piped stdout"),
                stderr: Errors::Captured(capture),
            },
        )
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
        let (fd, pipe): (_, &mut dyn Read) = match (stream, &mut child.stderr) {
            (ChildStream::Stdout, _) => (child.stdout.as_raw_fd(), &mut child.stdout),
            (ChildStream::Stderr, Errors::Pipe(stderr)) => (stderr.as_raw_fd(), stderr),
            (ChildStream::Stderr, Errors::Captured(_)) => {
                return Err(AdapterError::InvalidCompletion);
            }
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
    /// `child.wait` or `proc.wait_group`: under a scheduler it runs as the entered token's
    /// job, which the child moves into.
    fn wait(
        &mut self,
        id: u64,
        deadline: Option<u64>,
        readiness: Option<&Readiness>,
    ) -> Result<JobOutcome, AdapterError> {
        let finish = move |child: HostChild, cancel| match child.stderr {
            Errors::Pipe(_) => JobOutcome::Run(finish(child, cancel, deadline)),
            Errors::Captured(_) => JobOutcome::Group(finish_group(child, cancel, deadline)),
        };
        Ok(match readiness {
            None => finish(
                self.0.remove(&id).ok_or(AdapterError::InvalidHandle(id))?,
                None,
            ),
            Some(readiness) => {
                let children = &mut self.0;
                crate::process::cooperative_job(readiness, || {
                    let child = children.remove(&id)?;
                    Some(Box::new(move |cancel| finish(child, Some(cancel))))
                })?
            }
        })
    }
}

/// Close input, read and discard both outputs to end of file, then reap. A readable
/// `cancel` drops the child instead, which kills it. At `deadline` the child gets
/// SIGTERM, then SIGKILL after the grace, and is reaped: `TimedOut`.
fn finish(mut child: HostChild, cancel: Option<RawFd>, deadline: Option<u64>) -> PipelineOutcome {
    let timed_out = |child: &mut HostChild| {
        child.children.terminate(cancel);
        PipelineOutcome::Error(ProcessFailure::TimedOut)
    };
    let Errors::Pipe(stderr) = &child.stderr else {
        return PipelineOutcome::Error(ProcessFailure::Fault(1));
    };
    let stderr = stderr.as_raw_fd();
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
                fd: if open[1] { stderr } else { -1 },
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
        let HostChild {
            stdout,
            stderr: Errors::Pipe(stderr),
            ..
        } = &mut child
        else {
            unreachable!("checked above")
        };
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
    // Reaped stages' Drop signals nothing.
    match child.children.wait(deadline, cancel) {
        Ok(Ended::Exited(statuses)) => match exit_status(statuses[0]) {
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

/// `proc.wait_group`: close input, read and discard the last stage's output to end of file
/// while the capture drains every standard error, then reap every stage. A readable
/// `cancel` drops the group instead, which kills it. At `deadline` every stage gets
/// SIGTERM, then SIGKILL after the grace, and is reaped: `TimedOut`.
fn finish_group(
    mut child: HostChild,
    cancel: Option<RawFd>,
    deadline: Option<u64>,
) -> Result<GroupOutput, ProcessFailure> {
    let Errors::Captured(capture) = &mut child.stderr else {
        return Err(ProcessFailure::Fault(1));
    };
    let done = capture.done.as_raw_fd();
    let timed_out = |child: &mut HostChild| {
        if let Errors::Captured(capture) = &mut child.stderr {
            capture.stop();
        }
        child.children.terminate(cancel);
        Err(ProcessFailure::TimedOut)
    };
    child.stdin = None;
    let mut captured = None;
    let mut output_open = true;
    let mut chunk = vec![0u8; 64 * 1024];
    while output_open || captured.is_none() {
        let mut fds = [
            libc::pollfd {
                fd: if output_open {
                    child.stdout.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if captured.is_none() { done } else { -1 },
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
        // SAFETY: fds is a live array of three pollfd records.
        if unsafe { libc::poll(fds.as_mut_ptr(), 3, crate::process::poll_timeout(deadline)) } < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(ProcessFailure::Fault(1));
        }
        if fds[2].revents != 0 {
            return Err(ProcessFailure::Canceled(0));
        }
        // The capture's thread ended once its end of `done` closed. Over the bound it
        // killed every stage, whose output need not end: a process a stage started may
        // hold it open.
        if fds[1].revents != 0 {
            let Errors::Captured(capture) = &mut child.stderr else {
                unreachable!("checked above")
            };
            let (stderr, over) = capture.join();
            if over {
                drop(child);
                return Err(ProcessFailure::OutputLimit);
            }
            captured = Some(stderr);
        }
        if fds[0].revents != 0 {
            match child.stdout.read(&mut chunk) {
                Ok(0) => output_open = false,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(ProcessFailure::Fault(1)),
            }
        }
    }
    let stderr = captured.unwrap_or_default();
    match child.children.wait(deadline, cancel) {
        Ok(Ended::Exited(statuses)) => Ok(GroupOutput {
            statuses: (statuses.into_iter().map(exit_status))
                .collect::<Option<_>>()
                .ok_or(ProcessFailure::Fault(1))?,
            stderr,
        }),
        Ok(Ended::Canceled) => Err(ProcessFailure::Canceled(0)),
        Ok(Ended::Expired) => timed_out(&mut child),
        Err(_) => Err(ProcessFailure::Fault(1)),
    }
}
