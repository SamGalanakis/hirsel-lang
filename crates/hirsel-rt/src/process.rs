//! The Process capability (docs/spec/core-library.md#processes): exact program rules
//! checked before any effect, scripted worlds, and the host provider.
//!
//! The host provider is Rust std's `Command` with an empty environment and an explicit
//! working directory. Its stream loop adapts std's unix `read_output`
//! (library/std/src/sys/process/unix/common.rs, f6c1107f; third-party/NOTICES.md):
//! one `poll` over nonblocking pipes, here also feeding stdin and bounding each
//! stream, so neither pipe can stall the other.
use crate::handles::Grant;
use crate::io::{AdapterError, Completion, Request, State};
pub use hirsel_run_config::{
    ExitStatus, ProcessFailure, ProcessOutcome, ProcessRule, ProcessRun, bounded,
    canonical_program, check,
};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

/// Which provider serves a Proc: the host, or a scripted world.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProcessWorld {
    #[default]
    Host,
    Script(u64),
}

/// One run after every check passed: the host provider's whole input.
pub struct ProcessSpec<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub stdin: &'a [u8],
    /// Pass-through variable names from the granting rule.
    pub env: &'a [String],
    /// The working directory's Dir handle, always host-backed here; `None` is `/`.
    pub directory: Option<u64>,
    pub max_output: u64,
}

impl State {
    /// The admitted `io.process` grant: its rules, and for process.script@1 the root
    /// scripted world. Scripted runs never widen the rules.
    pub(crate) fn set_process_grant(&mut self, grant: &hirsel_run_config::ProcessGrant) {
        self.process_rules = grant.rules.clone();
        self.process_world = match &grant.runs {
            Some(runs) => {
                self.process_scripts
                    .insert(0, runs.iter().cloned().map(Some).collect());
                ProcessWorld::Script(0)
            }
            None => ProcessWorld::Host,
        };
    }
    fn process_grant(&self, id: u64) -> Result<(Vec<ProcessRule>, ProcessWorld), AdapterError> {
        match self.handles.rows.get(&id) {
            Some(Grant::Process { rules, world }) => Ok((rules.clone(), *world)),
            _ => Err(AdapterError::InvalidHandle(id)),
        }
    }
    /// Check the grant and inputs before any effect, serve a scripted world itself, and
    /// ask `host` only for a checked host run. `host` is the recorded completion in replay.
    pub(crate) fn execute_process(
        &mut self,
        host: &mut dyn FnMut(&ProcessSpec<'_>) -> Result<ProcessOutcome, AdapterError>,
        request: &Request,
    ) -> Result<Completion, AdapterError> {
        match request {
            Request::Process { io } => {
                self.expect(*io, Grant::Io)?;
                if !self.projections.contains("process") {
                    return Ok(Completion::Handle(Err(crate::io::IoError::NotGranted)));
                }
                let grant = Grant::Process {
                    rules: self.process_rules.clone(),
                    world: self.process_world,
                };
                Ok(Completion::Handle(Ok(self.handles.insert(grant))))
            }
            Request::TestProcess { test, runs } => {
                if !matches!(self.handles.rows.get(test), Some(Grant::Test { .. })) {
                    return Err(AdapterError::InvalidHandle(*test));
                }
                if runs.iter().any(|run| !canonical_program(&run.program)) {
                    return Err(AdapterError::InvalidCompletion);
                }
                let mut programs: Vec<_> = runs.iter().map(|r| r.program.clone()).collect();
                programs.sort();
                programs.dedup();
                let id = self.handles.next();
                self.handles.insert(Grant::Process {
                    rules: programs
                        .into_iter()
                        .map(|program| ProcessRule {
                            program,
                            env: vec![],
                        })
                        .collect(),
                    world: ProcessWorld::Script(id),
                });
                self.process_scripts
                    .insert(id, runs.iter().cloned().map(Some).collect());
                Ok(Completion::Handle(Ok(id)))
            }
            Request::ProcessRun {
                process,
                program,
                args,
                stdin,
                directory,
                max_output,
            } => {
                let (rules, world) = self.process_grant(*process)?;
                if let Some(d) = directory
                    && !matches!(self.handles.rows.get(d), Some(Grant::Directory { .. }))
                {
                    return Err(AdapterError::InvalidHandle(*d));
                }
                if let Err(failure) = check(program, args, &rules) {
                    return Ok(Completion::Process(ProcessOutcome::Error(failure)));
                }
                let outcome = match world {
                    ProcessWorld::Script(id) => {
                        let runs = self
                            .process_scripts
                            .get_mut(&id)
                            .ok_or(AdapterError::InvalidCompletion)?;
                        match runs.iter_mut().find(|r| {
                            r.as_ref()
                                .is_some_and(|r| &r.program == program && &r.args == args)
                        }) {
                            Some(entry) => bounded(entry.take().unwrap().outcome, *max_output),
                            None => ProcessOutcome::Error(ProcessFailure::NotFound),
                        }
                    }
                    // A fake test.dir has no host directory to start in.
                    ProcessWorld::Host
                        if directory.is_some_and(|d| self.fake_directories.contains_key(&d)) =>
                    {
                        ProcessOutcome::Error(ProcessFailure::InvalidInput)
                    }
                    ProcessWorld::Host => {
                        let rule = rules.iter().find(|r| &r.program == program).unwrap();
                        let outcome = host(&ProcessSpec {
                            program,
                            args,
                            stdin,
                            env: &rule.env,
                            directory: *directory,
                            max_output: *max_output,
                        })?;
                        // A provider can fail or exceed the bound, never contradict the checks.
                        let valid = match &outcome {
                            ProcessOutcome::Error(ProcessFailure::NotGranted) => false,
                            ProcessOutcome::Error(ProcessFailure::InvalidInput) => {
                                directory.is_some()
                            }
                            outcome => bounded(outcome.clone(), *max_output) == *outcome,
                        };
                        if !valid {
                            return Err(AdapterError::InvalidCompletion);
                        }
                        outcome
                    }
                };
                Ok(Completion::Process(outcome))
            }
            _ => Err(AdapterError::InvalidCompletion),
        }
    }
}

pub(crate) fn is_process_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Process { .. } | Request::TestProcess { .. } | Request::ProcessRun { .. }
    )
}

fn spawn_failure(error: &std::io::Error) -> ProcessFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => ProcessFailure::NotFound,
        std::io::ErrorKind::PermissionDenied => ProcessFailure::PermissionDenied,
        _ => ProcessFailure::Fault(1),
    }
}

/// Run one checked program on the host to completion, starting in `cwd` (a pinned
/// directory descriptor) or `/`.
pub fn run_host(spec: &ProcessSpec<'_>, cwd: Option<std::os::fd::RawFd>) -> ProcessOutcome {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    let mut command = Command::new(spec.program);
    command
        .args(spec.args)
        .env_clear()
        .current_dir(cwd.map_or_else(|| "/".to_owned(), |fd| format!("/proc/self/fd/{fd}")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in spec.env {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return ProcessOutcome::Error(spawn_failure(&error)),
    };
    let streams = without_sigpipe(|| {
        drain(
            child.stdin.take().expect("piped stdin"),
            spec.stdin,
            child.stdout.take().expect("piped stdout"),
            child.stderr.take().expect("piped stderr"),
            spec.max_output,
        )
    });
    let (stdout, stderr) = match streams {
        Ok(Some(streams)) => streams,
        Ok(None) => {
            // Over the bound: the child is not trusted to stop writing.
            let _ = child.kill();
            let _ = child.wait();
            return ProcessOutcome::Error(ProcessFailure::OutputLimit);
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return ProcessOutcome::Error(ProcessFailure::Fault(1));
        }
    };
    let status = loop {
        match child.wait() {
            Ok(status) => break status,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return ProcessOutcome::Error(ProcessFailure::Fault(1)),
        }
    };
    let status = match (status.code(), status.signal()) {
        (Some(code), _) => ExitStatus::Exited(code),
        (None, Some(signal)) => ExitStatus::Signaled(signal),
        (None, None) => return ProcessOutcome::Error(ProcessFailure::Fault(1)),
    };
    ProcessOutcome::Output {
        status,
        stdout,
        stderr,
    }
}

/// Run `body` with SIGPIPE blocked on this thread, then discard a SIGPIPE it raised:
/// a child that closes its stdin early must not end the program, whose disposition of
/// SIGPIPE is not the runtime's to change.
fn without_sigpipe<T>(body: impl FnOnce() -> T) -> T {
    // SAFETY: sigset_t values are initialized by sigemptyset before use, and the
    // thread mask is restored before returning.
    unsafe {
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut previous: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGPIPE);
        let mut pending: libc::sigset_t = std::mem::zeroed();
        libc::sigpending(&mut pending);
        let already = libc::sigismember(&pending, libc::SIGPIPE) == 1;
        libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut previous);
        let result = body();
        if !already {
            let zero = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            while libc::sigtimedwait(&block, std::ptr::null_mut(), &zero) == libc::SIGPIPE {}
        }
        libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
        result
    }
}

/// Write all of `input` and read both streams to end of file, concurrently. `None` when a
/// stream passes `max` bytes. A child that closes its stdin early only ends the writing.
fn drain(
    stdin: std::process::ChildStdin,
    input: &[u8],
    mut stdout: std::process::ChildStdout,
    mut stderr: std::process::ChildStderr,
    max: u64,
) -> std::io::Result<Option<(Vec<u8>, Vec<u8>)>> {
    fn nonblocking(fd: std::os::fd::RawFd) -> std::io::Result<()> {
        // SAFETY: fd is a live pipe descriptor owned by the caller.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    let mut stdin = Some(stdin);
    let mut written = 0;
    if input.is_empty() {
        stdin = None;
    }
    for fd in [
        stdout.as_raw_fd(),
        stderr.as_raw_fd(),
        stdin.as_ref().map_or(-1, AsRawFd::as_raw_fd),
    ] {
        if fd >= 0 {
            nonblocking(fd)?;
        }
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_open, mut err_open) = (true, true);
    let mut chunk = vec![0u8; 64 * 1024];
    while out_open || err_open {
        let mut fds = [
            libc::pollfd {
                fd: if out_open { stdout.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if err_open { stderr.as_raw_fd() } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stdin.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLOUT,
                revents: 0,
            },
        ];
        // SAFETY: fds is a live array of three pollfd records.
        if unsafe { libc::poll(fds.as_mut_ptr(), 3, -1) } < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[2].revents != 0
            && let Some(pipe) = &mut stdin
        {
            match pipe.write(&input[written..]) {
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                // The child closed its input; it simply did not read the rest.
                Err(_) => written = input.len(),
            }
            if written == input.len() {
                stdin = None;
            }
        }
        for (revents, open, pipe, buffer) in [
            (
                fds[0].revents,
                &mut out_open,
                &mut stdout as &mut dyn Read,
                &mut out,
            ),
            (
                fds[1].revents,
                &mut err_open,
                &mut stderr as &mut dyn Read,
                &mut err,
            ),
        ] {
            if revents == 0 {
                continue;
            }
            match pipe.read(&mut chunk) {
                Ok(0) => *open = false,
                Ok(n) => {
                    buffer.extend_from_slice(&chunk[..n]);
                    if buffer.len() as u64 > max {
                        return Ok(None);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(Some((out, err)))
}

/// `Result[ProcessOutput, ProcessError]`. Ok: ExitStatus (u32 tag, i32) at 8, stdout at
/// 16, stderr at 40. Error: ProcessError (u32 tag, Fault's u32) at 8.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ProcessResult {
    pub tag: u32,
    pub padding: u32,
    pub kind: u32,
    pub code: u32,
    pub stdout: crate::views::Slice,
    pub stderr: crate::views::Slice,
}

unsafe fn list<T>(
    owner: *mut crate::Header,
    stride: usize,
    mut item: impl FnMut(*const u8) -> T,
) -> Vec<T> {
    let payload = crate::alloc::payload(owner);
    let count = usize::try_from(unsafe { payload.cast::<u64>().read() })
        .unwrap_or_else(|_| crate::fault("process list overflow"));
    let first = crate::alloc::sequence_elements_offset(8) as usize;
    (0..count)
        .map(|index| {
            let offset = index
                .checked_mul(stride)
                .and_then(|n| n.checked_add(first))
                .filter(|n| *n <= isize::MAX as usize)
                .unwrap_or_else(|| crate::fault("process list element overflow"));
            item(unsafe { payload.add(offset) })
        })
        .collect()
}
unsafe fn slice(at: *const u8) -> Vec<u8> {
    let slice = unsafe { at.cast::<crate::views::Slice>().read() };
    unsafe { crate::io::slice_bytes(slice.owner, slice.start, slice.length) }.to_vec()
}
unsafe fn text(at: *const u8) -> String {
    String::from_utf8(unsafe { slice(at) })
        .unwrap_or_else(|_| crate::fault("process String is not UTF-8"))
}
fn failure_parts(failure: ProcessFailure) -> (u32, u32) {
    match failure {
        ProcessFailure::NotGranted => (0, 0),
        ProcessFailure::InvalidInput => (1, 0),
        ProcessFailure::NotFound => (2, 0),
        ProcessFailure::PermissionDenied => (3, 0),
        ProcessFailure::OutputLimit => (4, 0),
        ProcessFailure::Fault(code) => (5, code),
    }
}
/// One `ProcessScript` (96 bytes): program String at 0, args List at 24, outcome
/// `Result[ProcessOutput, ProcessError]` at 32.
unsafe fn script(at: *const u8) -> ProcessRun {
    let outcome = unsafe { at.add(32) };
    let kind = unsafe { outcome.add(8).cast::<u32>().read() };
    let code = unsafe { outcome.add(12).cast::<u32>().read() };
    ProcessRun {
        program: unsafe { text(at) },
        args: unsafe {
            list(at.add(24).cast::<*mut crate::Header>().read(), 24, |a| {
                text(a)
            })
        },
        outcome: match unsafe { outcome.cast::<u32>().read() } {
            0 => ProcessOutcome::Output {
                status: match kind {
                    0 => ExitStatus::Exited(code as i32),
                    1 => ExitStatus::Signaled(code as i32),
                    _ => crate::fault("ExitStatus tag"),
                },
                stdout: unsafe { slice(outcome.add(16)) },
                stderr: unsafe { slice(outcome.add(40)) },
            },
            1 => ProcessOutcome::Error(match (kind, code) {
                (0, _) => ProcessFailure::NotGranted,
                (1, _) => ProcessFailure::InvalidInput,
                (2, _) => ProcessFailure::NotFound,
                (3, _) => ProcessFailure::PermissionDenied,
                (4, _) => ProcessFailure::OutputLimit,
                (5, code) => ProcessFailure::Fault(code),
                _ => crate::fault("ProcessError tag"),
            }),
            _ => crate::fault("ProcessScript outcome tag"),
        },
    }
}

/// # Safety
/// Context is bound; Proc, Path, the List[String] block, the Bytes slice and the
/// `Option[Dir]` (u32 tag, Dir at 8) are borrowed live values; out is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_proc_run(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut ProcessResult,
    process: *const u64,
    program: *const u64,
    args: *mut crate::Header,
    stdin_owner: *mut crate::Header,
    stdin_start: u64,
    stdin_length: u64,
    directory: *const u8,
    max_output: u64,
) -> u8 {
    let request = Request::ProcessRun {
        process: unsafe { process.read() },
        program: unsafe { crate::io::Binding::get(ctx) }.path(unsafe { program.read() }),
        args: unsafe { list(args, 24, |a| text(a)) },
        stdin: unsafe { crate::io::slice_bytes(stdin_owner, stdin_start, stdin_length) }.to_vec(),
        directory: match unsafe { directory.cast::<u32>().read() } {
            0 => None,
            _ => Some(unsafe { directory.add(8).cast::<u64>().read() }),
        },
        max_output,
    };
    let Ok(completion) = (unsafe { crate::io::capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Process(outcome) = completion else {
        crate::fault("process completion mismatch")
    };
    // Zero padding and the inactive arm; no pointer is made from error bits.
    let mut result: ProcessResult = unsafe { std::mem::zeroed() };
    match outcome {
        ProcessOutcome::Output {
            status,
            stdout,
            stderr,
        } => {
            (result.kind, result.code) = match status {
                ExitStatus::Exited(code) => (0, code as u32),
                ExitStatus::Signaled(signal) => (1, signal as u32),
            };
            result.stdout = unsafe { crate::text::allocate_bytes(ctx, &stdout) };
            result.stderr = unsafe { crate::text::allocate_bytes(ctx, &stderr) };
        }
        ProcessOutcome::Error(failure) => {
            result.tag = 1;
            (result.kind, result.code) = failure_parts(failure);
        }
    }
    unsafe { out.write(result) };
    crate::STATUS_OK
}

/// # Safety
/// Context is bound; Test and the List[ProcessScript] block are borrowed live values;
/// out is writable. No borrowed input is retained.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_test_process(
    ctx: *mut crate::RunCtx,
    _destination: *mut u8,
    out: *mut u64,
    test: *const u64,
    runs: *mut crate::Header,
) -> u8 {
    let request = Request::TestProcess {
        test: unsafe { test.read() },
        runs: unsafe { list(runs, 96, |at| script(at)) },
    };
    let Ok(completion) = (unsafe { crate::io::capability_call(ctx, request) }) else {
        return crate::STATUS_FAILURE;
    };
    let Completion::Handle(Ok(process)) = completion else {
        crate::fault("Test Proc completion mismatch")
    };
    unsafe { out.write(process) };
    crate::STATUS_OK
}
