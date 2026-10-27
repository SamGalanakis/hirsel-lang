//! Cooperative segment trace; compiled out of every non-measurement archive.
//!
//! Commit ids are one-based; zero is entry. `after` is the commit that made a
//! segment eligible, not the (later) moment FIFO actually dispatched its task.
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;

use crate::TaskCtx;

#[derive(Default)]
pub(crate) struct Profile {
    active: Option<Instant>,
    after: BTreeMap<Vec<u32>, usize>,
    rows: Vec<Segment>,
}

struct Segment {
    task: Vec<u32>,
    ns: u64,
    operation: &'static str,
    after: usize,
}

impl Profile {
    fn end(&mut self, path: Vec<u32>, operation: &'static str, end: Instant) -> bool {
        let Some(start) = self.active.take() else {
            return false;
        };
        let ns = end
            .duration_since(start)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        let after = self.after.get(&path).copied().unwrap_or(0);
        self.rows.push(Segment {
            task: path.clone(),
            ns,
            operation,
            after,
        });
        self.after.insert(path, self.rows.len());
        true
    }
    pub(crate) fn ready(&mut self, path: Vec<u32>) {
        self.after.insert(path, self.rows.len());
    }
    pub(crate) fn write(&self, mut out: impl Write) -> std::io::Result<()> {
        // Reuse the host-admitted measurement descriptor, never program Io or tape.
        writeln!(out, "mc-profile 1")?;
        for row in &self.rows {
            write!(out, "mc ")?;
            serde_json::to_writer(
                &mut out,
                &serde_json::json!({
                    "task": row.task, "ns": row.ns, "operation": row.operation, "after": row.after,
                }),
            )?;
            writeln!(out)?;
        }
        out.flush()
    }
}

fn path(ctx: &TaskCtx) -> Vec<u32> {
    ctx.run_ref().tasks.as_ref().map_or_else(Vec::new, |s| {
        s.tasks
            .get(s.current as usize)
            .and_then(Option::as_ref)
            .map_or_else(Vec::new, |task| task.path.clone())
    })
}

/// Begin the next pure segment after a runtime operation, or at task entry.
pub(crate) fn resume(ctx: *mut TaskCtx) {
    // SAFETY: callers have a live context and retain no borrow across a switch.
    if let Some(profile) = unsafe { &mut (*(*(*ctx).run_ptr()).mc_profile.get()) }.as_mut() {
        profile.active = Some(Instant::now());
    }
}

/// A runtime operation may suspend its fiber; this guard owns no runtime borrow.
/// Nested helpers do not introduce additional commits or restart the segment.
pub(crate) struct Operation {
    ctx: *mut TaskCtx,
    resume: bool,
}
impl Operation {
    pub(crate) fn new(ctx: *mut TaskCtx, operation: &'static str) -> Self {
        // SAFETY: every caller is a runtime entry with a live context.
        let context = unsafe { &mut *ctx };
        let resume = if context.run_ref().mc_profile.is_some() {
            let end = Instant::now();
            let path = path(context);
            unsafe { &mut (*(*context.run_ptr()).mc_profile.get()) }
                .as_mut()
                .unwrap()
                .end(path, operation, end)
        } else {
            false
        };
        Self { ctx, resume }
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if self.resume {
            resume(self.ctx);
        }
    }
}

pub(crate) fn finish(ctx: &mut TaskCtx) {
    if ctx.run_ref().mc_profile.is_some() {
        let end = Instant::now();
        let path = path(ctx);
        unsafe { &mut (*(*ctx.run_ptr()).mc_profile.get()) }
            .as_mut()
            .unwrap()
            .end(path, "run.end", end);
    }
}

/// Operation names without formatting request operands or copying their data.
pub(crate) fn request_kind(request: &crate::io::Request) -> &'static str {
    use crate::io::Request;
    match request {
        Request::Stdin { .. } => "Stdin",
        Request::Stderr { .. } => "Stderr",
        Request::ReadStdin { .. } => "ReadStdin",
        Request::PrintStderr { .. } => "PrintStderr",
        Request::EnvGet { .. } => "EnvGet",
        Request::EnvNarrow { .. } => "EnvNarrow",
        Request::MonotonicClock { .. } => "MonotonicClock",
        Request::WallClock { .. } => "WallClock",
        Request::Monotonic { .. } => "Monotonic",
        Request::Wall { .. } => "Wall",
        Request::Random { .. } => "Random",
        Request::RandomBytes { .. } => "RandomBytes",
        Request::Telemetry { .. } => "Telemetry",
        Request::TelemetryBegin { .. } => "TelemetryBegin",
        Request::TelemetryEvent { .. } => "TelemetryEvent",
        Request::TelemetryFinish { .. } => "TelemetryFinish",
        Request::TelemetryLog { .. } => "TelemetryLog",
        Request::TelemetryMeasure { .. } => "TelemetryMeasure",
        Request::SleepUntil { .. } => "SleepUntil",
        Request::Tasks { .. } => "Tasks",
        Request::Net { .. } => "Net",
        Request::Process { .. } => "Process",
        Request::Signals { .. } => "Signals",
        Request::SignalWait { .. } => "SignalWait",
        Request::TestProcess { .. } => "TestProcess",
        Request::ProcessNarrow { .. } => "ProcessNarrow",
        Request::ProcessRun { .. } => "ProcessRun",
        Request::ProcessPipeline { .. } => "ProcessPipeline",
        Request::ProcessSpawn { .. } => "ProcessSpawn",
        Request::ProcessSpawnPipeline { .. } => "ProcessSpawnPipeline",
        Request::ChildWrite { .. } => "ChildWrite",
        Request::ChildCloseStdin { .. } => "ChildCloseStdin",
        Request::ChildRead { .. } => "ChildRead",
        Request::ChildWait { .. } => "ChildWait",
        Request::ChildKill { .. } => "ChildKill",
        Request::GroupWait { .. } => "GroupWait",
        Request::TestCheck { .. } => "TestCheck",
        Request::TestCheckCase { .. } => "TestCheckCase",
        Request::TestFail { .. } => "TestFail",
        Request::TestClock { .. } => "TestClock",
        Request::TestSeed { .. } => "TestSeed",
        Request::TestTasks { .. } => "TestTasks",
        Request::TestLog { .. } => "TestLog",
        Request::TestSnapshot { .. } => "TestSnapshot",
        Request::TestNet { .. } => "TestNet",
        Request::TestDir { .. } => "TestDir",
        Request::TestCrash { .. } => "TestCrash",
        Request::RunnerCall(_) => "RunnerCall",
        Request::RunnerPoll { .. } => "RunnerPoll",
        Request::RunnerCancel { .. } => "RunnerCancel",
        Request::CallReturn(_) => "CallReturn",
        Request::Stopped => "Stopped",
        Request::Stdout { .. } => "Stdout",
        Request::Directory { .. } => "Directory",
        Request::NarrowDir { .. } => "NarrowDir",
        Request::ReadOnlyDir { .. } => "ReadOnlyDir",
        Request::Print { .. } => "Print",
        Request::Open { .. } => "Open",
        Request::Create { .. } => "Create",
        Request::OpenAppend { .. } => "OpenAppend",
        Request::FileRead { .. } => "FileRead",
        Request::FileSeek { .. } => "FileSeek",
        Request::FileWrite { .. } => "FileWrite",
        Request::FileAppend { .. } => "FileAppend",
        Request::NarrowFile { .. } => "NarrowFile",
        Request::ReadDirPage { .. } => "ReadDirPage",
        Request::Metadata { .. } => "Metadata",
        Request::Mkdir { .. } => "Mkdir",
        Request::Rename { .. } => "Rename",
        Request::RemoveDir { .. } => "RemoveDir",
        Request::SetPermissions { .. } => "SetPermissions",
        Request::RemoveAll { .. } => "RemoveAll",
        Request::Size { .. } => "Size",
        Request::Read { .. } => "Read",
        Request::Write { .. } => "Write",
        Request::Remove { .. } => "Remove",
        Request::CreateAtomic { .. } => "CreateAtomic",
        Request::WriteAll { .. } => "WriteAll",
        Request::Commit { .. } => "Commit",
        Request::Sync { .. } => "Sync",
        Request::PutNew { .. } => "PutNew",
        Request::Map { .. } => "Map",
        Request::MapView { .. } => "MapView",
        Request::MapCheck { .. } => "MapCheck",
        Request::Watch { .. } => "Watch",
        Request::WatchNext { .. } => "WatchNext",
        Request::TestDirEvent { .. } => "TestDirEvent",
        Request::Close { .. } => "Close",
        Request::NetNarrow { .. } => "NetNarrow",
        Request::Resolve { .. } => "Resolve",
        Request::Connect { .. } => "Connect",
        Request::UdpBind { .. } => "UdpBind",
        Request::UdpSend { .. } => "UdpSend",
        Request::UdpRecv { .. } => "UdpRecv",
        Request::ConnectAddr { .. } => "ConnectAddr",
        Request::NetRead { .. } => "NetRead",
        Request::NetWrite { .. } => "NetWrite",
        Request::NetWriteAll { .. } => "NetWriteAll",
        Request::Shutdown { .. } => "Shutdown",
        Request::SetTimeout { .. } => "SetTimeout",
        Request::PeerAddr { .. } => "PeerAddr",
        Request::TlsConnect { .. } => "TlsConnect",
        Request::TlsAccept { .. } => "TlsAccept",
        Request::TlsPeer { .. } => "TlsPeer",
        Request::TlsProtocol { .. } => "TlsProtocol",
        Request::NetSplit { .. } => "NetSplit",
        Request::NetJoin { .. } => "NetJoin",
        Request::TlsClose { .. } => "TlsClose",
        Request::Listen { .. } => "Listen",
        Request::Accept { .. } => "Accept",
        Request::LocalAddr { .. } => "LocalAddr",
        Request::UnixConnect { .. } => "UnixConnect",
        Request::UnixListen { .. } => "UnixListen",
        Request::Ready { .. } => "Ready",
        Request::Schedule { .. } => "Schedule",
        Request::CpuControl { .. } => "CpuControl",
        Request::CpuSummary(_) => "CpuSummary",
        Request::ProcessTime { .. } => "ProcessTime",
        Request::ProcessSettle { .. } => "ProcessSettle",
    }
}
