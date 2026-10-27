//! Initialized request field walk shared by execution modes. The existing
//! Ordered codec remains the source of this exhaustive projection; callers
//! supply logical authority identities and own the resulting tape format.
use crate::io::Request;
use crate::tape_wire::Wire;

pub(crate) fn project<A>(
    request: &Request,
    mut identity: impl FnMut(u64) -> Option<A>,
) -> Result<(Vec<u8>, Vec<A>), &'static str> {
    let mut request = request.clone();
    let mut authorities = Vec::new();
    let mut on = |word: &mut u64| {
        authorities.push(identity(*word).ok_or("unmapped host request")?);
        *word = 0;
        Ok::<_, &'static str>(())
    };
    use Request as R;
    match &mut request {
        R::Stdin { io }
        | R::Stderr { io }
        | R::Stdout { io }
        | R::MonotonicClock { io }
        | R::WallClock { io }
        | R::Random { io }
        | R::Tasks { io }
        | R::Net { io }
        | R::Process { io }
        | R::Signals { io }
        | R::Telemetry { io }
        | R::Directory { io, .. } => on(io)?,
        R::EnvGet { env, .. } | R::EnvNarrow { env, .. } => on(env)?,
        R::Print { stdout, .. } => on(stdout)?,
        R::PrintStderr { stderr, .. } => on(stderr)?,
        R::ReadStdin { stdin, .. } => on(stdin)?,
        R::Monotonic { clock } | R::Wall { clock } | R::SleepUntil { clock, .. } => on(clock)?,
        R::RandomBytes { random, .. } => on(random)?,
        R::SignalWait { signals, .. } => on(signals)?,
        R::TelemetryBegin { telemetry, .. }
        | R::TelemetryLog { telemetry, .. }
        | R::TelemetryMeasure { telemetry, .. } => on(telemetry)?,
        R::TelemetryEvent {
            telemetry, span, ..
        }
        | R::TelemetryFinish {
            telemetry, span, ..
        } => {
            on(telemetry)?;
            on(span)?;
        }
        R::TestProcess { test, .. }
        | R::TestCheck { test, .. }
        | R::TestCheckCase { test, .. }
        | R::TestFail { test }
        | R::TestClock { test }
        | R::TestSeed { test }
        | R::TestTasks { test }
        | R::TestLog { test, .. }
        | R::TestSnapshot { test, .. }
        | R::TestNet { test, .. }
        | R::TestDir { test, .. } => on(test)?,
        R::TestCrash {
            test, directory, ..
        }
        | R::TestDirEvent {
            test, directory, ..
        } => {
            on(test)?;
            on(directory)?;
        }
        R::ProcessNarrow { process, .. } => on(process)?,
        R::ProcessRun {
            process,
            command,
            deadline,
            ..
        } => {
            on(process)?;
            if let Some(directory) = &mut command.directory {
                on(directory)?;
            }
            if let Some(deadline) = deadline {
                on(&mut deadline.clock)?;
            }
        }
        R::ProcessSpawn { process, command } => {
            on(process)?;
            if let Some(directory) = &mut command.directory {
                on(directory)?;
            }
        }
        R::ProcessPipeline {
            process,
            stages,
            deadline,
            ..
        } => {
            on(process)?;
            for command in stages {
                if let Some(directory) = &mut command.directory {
                    on(directory)?;
                }
            }
            if let Some(deadline) = deadline {
                on(&mut deadline.clock)?;
            }
        }
        R::ProcessSpawnPipeline {
            process, stages, ..
        } => {
            on(process)?;
            for command in stages {
                if let Some(directory) = &mut command.directory {
                    on(directory)?;
                }
            }
        }
        R::ChildWrite { process, child, .. }
        | R::ChildCloseStdin { process, child }
        | R::ChildRead { process, child, .. }
        | R::ChildKill { process, child, .. } => {
            on(process)?;
            on(child)?;
        }
        R::ChildWait {
            process,
            child,
            deadline,
        } => {
            on(process)?;
            on(child)?;
            if let Some(deadline) = deadline {
                on(&mut deadline.clock)?;
            }
        }
        R::GroupWait {
            process,
            group,
            deadline,
        } => {
            on(process)?;
            on(group)?;
            if let Some(deadline) = deadline {
                on(&mut deadline.clock)?;
            }
        }
        R::NarrowDir { directory, .. }
        | R::ReadOnlyDir { directory }
        | R::Open { directory, .. }
        | R::OpenAppend { directory, .. }
        | R::Create { directory, .. }
        | R::ReadDirPage { directory, .. }
        | R::Metadata { directory, .. }
        | R::Mkdir { directory, .. }
        | R::Rename { directory, .. }
        | R::RemoveDir { directory, .. }
        | R::SetPermissions { directory, .. }
        | R::RemoveAll { directory, .. }
        | R::Size { directory, .. }
        | R::Read { directory, .. }
        | R::Write { directory, .. }
        | R::Remove { directory, .. }
        | R::CreateAtomic { directory, .. }
        | R::PutNew { directory, .. }
        | R::Map { directory, .. }
        | R::Watch { directory, .. } => on(directory)?,
        R::FileRead {
            directory, file, ..
        }
        | R::FileSeek {
            directory, file, ..
        }
        | R::FileWrite {
            directory, file, ..
        }
        | R::FileAppend {
            directory, file, ..
        }
        | R::NarrowFile { directory, file }
        | R::WriteAll {
            directory, file, ..
        }
        | R::Commit { directory, file }
        | R::Sync { directory, file } => {
            on(directory)?;
            on(file)?;
        }
        R::MapView { directory, mapped } | R::MapCheck { directory, mapped } => {
            on(directory)?;
            on(mapped)?;
        }
        R::WatchNext { directory, watcher } => {
            on(directory)?;
            on(watcher)?;
        }
        R::Close { handle } => on(handle)?,
        R::NetNarrow { net, .. }
        | R::Resolve { net, .. }
        | R::Connect { net, .. }
        | R::ConnectAddr { net, .. }
        | R::Listen { net, .. }
        | R::UdpBind { net, .. }
        | R::UnixConnect { net, .. }
        | R::UnixListen { net, .. } => on(net)?,
        R::NetRead { net, stream, .. }
        | R::NetWrite { net, stream, .. }
        | R::NetWriteAll { net, stream, .. }
        | R::Shutdown { net, stream, .. }
        | R::SetTimeout { net, stream, .. }
        | R::PeerAddr { net, stream }
        | R::TlsConnect { net, stream, .. }
        | R::TlsAccept { net, stream, .. }
        | R::TlsPeer { net, stream }
        | R::TlsProtocol { net, stream }
        | R::TlsClose { net, stream }
        | R::NetSplit { net, stream } => {
            on(net)?;
            on(stream)?;
        }
        R::NetJoin { net, read, write } => {
            on(net)?;
            on(read)?;
            on(write)?;
        }
        R::UdpSend { net, socket, .. } | R::UdpRecv { net, socket, .. } => {
            on(net)?;
            on(socket)?;
        }
        R::Accept { net, listener } | R::LocalAddr { net, listener } => {
            on(net)?;
            on(listener)?;
        }
        // Runner messages contain the canonical grant/argument/result bytes,
        // definition path and logical call key. No field is a physical handle
        // requiring authority substitution; preserve every initialized byte.
        R::RunnerCall(_)
        | R::RunnerPoll { .. }
        | R::RunnerCancel { .. }
        | R::CallReturn(_)
        | R::Stopped => {}
        R::Ready { .. }
        | R::Schedule { .. }
        | R::CpuControl { .. }
        | R::CpuSummary(_)
        | R::ProcessTime { .. }
        | R::ProcessSettle { .. } => {
            return Err("request needs a dedicated scheduling projection");
        }
    }
    let mut payload = Vec::new();
    request.encode(&mut payload);
    Ok((payload, authorities))
}
