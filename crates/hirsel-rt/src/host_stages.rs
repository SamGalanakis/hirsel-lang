//! The host children of one run, child or pipeline (docs/spec/core-library.md#processes),
//! in stage order, and the process group they share when their commands ask for one
//! (`own_group`). Every signal goes to the last stage first, so no stage can read an
//! earlier stage's death as end of input before its own signal arrives, then to the whole
//! group, so whatever a stage started gets it too. Every stage is reaped here, which
//! records its status: no signal goes to a reaped stage's pid, and the group's leader (the
//! first stage, whose pid is the group's id) is reaped after every other stage, so the id
//! names the group whenever it is signalled.
use std::os::fd::RawFd;

/// How a bounded wait for every stage ended.
pub(crate) enum Ended {
    /// Every stage's status, in stage order.
    Exited(Vec<std::process::ExitStatus>),
    Expired,
    Canceled,
}

struct Stage {
    child: std::process::Child,
    status: Option<std::process::ExitStatus>,
}

/// The stages of one run, child or pipeline. Dropping them kills and reaps every stage
/// not yet reaped.
pub(crate) struct Stages {
    stages: Vec<Stage>,
    own_group: bool,
}

/// What a signal reaches: the unreaped stages' pids in stage order and the group's id.
/// A thread that holds a copy signals only while no stage is reaped.
#[derive(Clone)]
pub(crate) struct Targets {
    pids: Vec<libc::pid_t>,
    group: Option<libc::pid_t>,
}
impl Targets {
    /// Send `signal` to every stage, the last first, then to the group.
    pub(crate) fn signal(&self, signal: libc::c_int) {
        for &pid in self.pids.iter().rev() {
            // SAFETY: kill(2) on an unreaped stage's pid with a valid signal number.
            unsafe { libc::kill(pid, signal) };
        }
        if let Some(group) = self.group {
            // SAFETY: killpg(2) on a group whose leader is unreaped, so its id names it.
            unsafe { libc::killpg(group, signal) };
        }
    }
}

impl Stages {
    pub(crate) fn new(own_group: bool) -> Self {
        Self {
            stages: vec![],
            own_group,
        }
    }
    /// Spawn the next stage: in a group the first stage leads a new process group and
    /// every later stage joins it.
    pub(crate) fn spawn(
        &mut self,
        command: &mut std::process::Command,
    ) -> std::io::Result<&mut std::process::Child> {
        use std::os::unix::process::CommandExt;
        if self.own_group {
            command.process_group(self.stages.first().map_or(0, |s| s.child.id() as i32));
        }
        self.stages.push(Stage {
            child: command.spawn()?,
            status: None,
        });
        Ok(&mut self.stages.last_mut().expect("pushed").child)
    }
    /// The unreaped stages and, while its leader is unreaped, the group.
    pub(crate) fn targets(&self) -> Targets {
        let unreaped = |s: &&Stage| s.status.is_none();
        Targets {
            pids: (self.stages.iter().filter(unreaped))
                .map(|s| s.child.id() as libc::pid_t)
                .collect(),
            group: (self.own_group)
                .then(|| self.stages.first().filter(unreaped))
                .flatten()
                .map(|leader| leader.child.id() as libc::pid_t),
        }
    }
    /// Send `signal` to every unreaped stage, the last first, then to the group.
    pub(crate) fn signal(&self, signal: libc::c_int) {
        self.targets().signal(signal);
    }
    /// Wait for every stage to exit and reap it, the last first, until `deadline` or until
    /// `cancel` is readable.
    pub(crate) fn wait(
        &mut self,
        deadline: Option<u64>,
        cancel: Option<RawFd>,
    ) -> std::io::Result<Ended> {
        for stage in self.stages.iter_mut().rev() {
            if stage.status.is_some() {
                continue;
            }
            match wait_child(&mut stage.child, deadline, cancel)? {
                Some(status) => stage.status = Some(status),
                None if crate::process::signaled(cancel) => return Ok(Ended::Canceled),
                None => return Ok(Ended::Expired),
            }
        }
        Ok(Ended::Exited(
            self.stages.iter().filter_map(|s| s.status).collect(),
        ))
    }
    /// Stages that are not trusted to stop: SIGKILL, then reap every one.
    pub(crate) fn kill(&mut self) {
        self.signal(libc::SIGKILL);
        let _ = self.wait(None, None);
    }
    /// Timed-out stages: SIGTERM, then SIGKILL once every stage has exited or
    /// [`TERMINATION_GRACE`](crate::process::TERMINATION_GRACE) has passed (at once when
    /// `cancel` is readable), then reap them all. In a group the SIGKILL reaches whatever
    /// the stages started that outlived them.
    pub(crate) fn terminate(&mut self, cancel: Option<RawFd>) {
        self.signal(libc::SIGTERM);
        let grace =
            crate::process::monotonic_now() + crate::process::TERMINATION_GRACE.as_nanos() as u64;
        while !self.exited()
            && !crate::process::signaled(cancel)
            && crate::process::monotonic_now() < grace
        {
            pause(Some(grace), cancel);
        }
        self.kill();
    }
    /// Whether every unreaped stage has exited, reaping none.
    fn exited(&self) -> bool {
        self.stages.iter().filter(|s| s.status.is_none()).all(|s| {
            // SAFETY: waitid(2) with WNOWAIT on an unreaped child fills `info` and reaps
            // nothing; an all-zero siginfo_t is a valid value.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                libc::waitid(
                    libc::P_PID,
                    s.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } == 0;
            // SAFETY: waitid filled `info` for a child state change, or left si_pid zero.
            !ok || unsafe { info.si_pid() } != 0
        })
    }
}
impl Drop for Stages {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Wait for `child` to exit and reap it, until `deadline` or until `cancel` is readable:
/// `None` when either came first.
fn wait_child(
    child: &mut std::process::Child,
    deadline: Option<u64>,
    cancel: Option<RawFd>,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    if deadline.is_none() && cancel.is_none() {
        loop {
            match child.wait() {
                Ok(status) => return Ok(Some(status)),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if crate::process::signaled(cancel)
            || deadline.is_some_and(|d| crate::process::monotonic_now() >= d)
        {
            return Ok(None);
        }
        pause(deadline, cancel);
    }
}

/// No descriptor reports a child's exit portably: sleep up to 10 ms, less before
/// `deadline`, waking early when `cancel` becomes readable.
fn pause(deadline: Option<u64>, cancel: Option<RawFd>) {
    let mut row = libc::pollfd {
        fd: cancel.unwrap_or(-1),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = match crate::process::poll_timeout(deadline) {
        -1 => 10,
        t => t.min(10),
    };
    // SAFETY: row holds a live descriptor or -1, which poll ignores.
    unsafe { libc::poll(&mut row, 1, timeout) };
}
