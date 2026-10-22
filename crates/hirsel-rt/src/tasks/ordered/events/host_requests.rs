//! Initialized request projection. The field walk follows Request's variants
//! and the existing tape codec, including handles nested in process arguments.
use super::host_values::{HostHandleId, HostValue};
use crate::io::Request;
use crate::tape_wire::Wire;

pub(super) fn project(
    request: &Request,
    mut identity: impl FnMut(u64) -> Option<HostHandleId>,
) -> Result<HostValue, &'static str> {
    let mut request = request.clone();
    let mut handles = Vec::new();
    let mut on = |word: &mut u64| {
        handles.push(identity(*word).ok_or("unmapped host request")?);
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
        | R::Create { directory, .. }
        | R::Walk { directory }
        | R::ReadDir { directory, .. }
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
        R::RunnerCall(_)
        | R::RunnerCancel { .. }
        | R::CallReturn(_)
        | R::Stopped
        | R::Ready { .. }
        | R::Schedule { .. } => {
            return Err("request needs a dedicated scheduling or runner projection");
        }
    }
    let mut payload = Vec::new();
    request.encode(&mut payload);
    Ok(HostValue { payload, handles })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(word: u64) -> Option<HostHandleId> {
        Some(
            crate::ordered::footprint::ObjectId {
                creator: vec![1, 3],
                ordinal: word % 16,
            }
            .into(),
        )
    }

    fn command(directory: Option<u64>) -> crate::process::Command {
        crate::process::Command {
            program: "/usr/bin/test".into(),
            args: vec!["λ\0".into()],
            directory,
            env: Some(vec![("NAME".into(), "value".into())]),
            own_group: true,
        }
    }

    #[test]
    fn host_request_projection_covers_nested_command_directories_and_deadline_clocks() {
        let requests = |base: u64| {
            vec![
                Request::ProcessRun {
                    process: base + 1,
                    command: command(Some(base + 2)),
                    stdin: vec![0, 255, 1],
                    max_output: 99,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::ProcessPipeline {
                    process: base + 1,
                    stages: vec![
                        command(None),
                        command(Some(base + 2)),
                        command(Some(base + 4)),
                    ],
                    stdin: vec![0, 255],
                    max_output: 99,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::ProcessSpawn {
                    process: base + 1,
                    command: command(Some(base + 2)),
                },
                Request::ProcessSpawnPipeline {
                    process: base + 1,
                    stages: vec![command(Some(base + 2)), command(None)],
                    max_stderr: 99,
                },
                Request::ChildWait {
                    process: base + 1,
                    child: base + 2,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
                Request::GroupWait {
                    process: base + 1,
                    group: base + 2,
                    deadline: Some(crate::process::Deadline {
                        clock: base + 3,
                        ticks: 771,
                    }),
                },
            ]
        };
        for ((first, second), ordinals) in requests(0x1234_0000)
            .into_iter()
            .zip(requests(0x9876_0000))
            .zip([
                vec![1, 2, 3],
                vec![1, 2, 4, 3],
                vec![1, 2],
                vec![1, 2],
                vec![1, 2, 3],
                vec![1, 2, 3],
            ])
        {
            let original = first.clone();
            let a = project(&first, identity).unwrap();
            assert_eq!(a, project(&second, identity).unwrap());
            assert_eq!(
                a.handles
                    .iter()
                    .map(|handle| handle.operation.ordinal)
                    .collect::<Vec<_>>(),
                ordinals
            );
            assert!(project(&first, |_| None).is_err());
            let mut visited = 0;
            assert!(
                project(&first, |word| {
                    visited += 1;
                    (visited != a.handles.len()).then(|| identity(word).unwrap())
                })
                .is_err()
            );
            assert_eq!(visited, a.handles.len(), "all nested fields must resolve");
            let mut normalized = crate::tape_wire::decode::<Request>(&a.payload).unwrap();
            assert_eq!(
                project(&normalized, |_| identity(0)).unwrap().payload,
                a.payload
            );
            if let Request::ProcessRun {
                command,
                deadline,
                max_output,
                stdin,
                ..
            } = &mut normalized
            {
                assert_eq!(command.directory, Some(0));
                assert_eq!(deadline.unwrap().clock, 0);
                assert_eq!(deadline.unwrap().ticks, 771);
                assert_eq!(*max_output, 99);
                assert_eq!(stdin, &[0, 255, 1]);
                assert_eq!(command.args, ["λ\0"]);
            }
            assert_eq!(first, original);
        }
    }

    #[test]
    fn host_request_projection_preserves_payloads_while_resolving_all_authority_fields() {
        let requests = |base: u64| {
            vec![
                Request::Print {
                    stdout: base + 1,
                    bytes: vec![0, 255],
                },
                Request::TelemetryEvent {
                    telemetry: base + 1,
                    span: base + 2,
                    payload: vec![0, 255],
                },
                Request::TelemetryBegin {
                    telemetry: base + 1,
                    parent: vec![9; 24],
                    spec: vec![8; 16],
                },
                Request::FileRead {
                    directory: base + 1,
                    file: base + 2,
                    max: u64::MAX,
                },
                Request::FileWrite {
                    directory: base + 1,
                    file: base + 2,
                    bytes: vec![0, 255],
                },
                Request::FileSeek {
                    directory: base + 1,
                    file: base + 2,
                    offset: 771,
                },
                Request::MapView {
                    directory: base + 1,
                    mapped: base + 2,
                },
                Request::WatchNext {
                    directory: base + 1,
                    watcher: base + 2,
                },
                Request::TestCrash {
                    test: base + 1,
                    directory: base + 2,
                    seed: 771,
                },
                Request::TestCheckCase {
                    test: base + 1,
                    condition: true,
                    case: 771,
                },
                Request::NetJoin {
                    net: base + 1,
                    read: base + 2,
                    write: base + 3,
                },
                Request::NetRead {
                    net: base + 1,
                    stream: base + 2,
                    max: u64::MAX,
                },
                Request::Resolve {
                    net: base + 1,
                    host: "example.org".into(),
                    port: 771,
                },
                Request::Close { handle: base + 1 },
                Request::EnvGet {
                    env: base + 1,
                    name: "NAME".into(),
                },
                Request::SleepUntil {
                    clock: base + 1,
                    deadline: 771,
                },
            ]
        };
        for (a, b) in requests(0x1234_0000).iter().zip(requests(0x9876_0000)) {
            let observed = project(a, identity).unwrap();
            assert_eq!(observed, project(&b, identity).unwrap());
            assert!(!observed.handles.is_empty());
            assert!(project(a, |_| None).is_err());
            let decoded = crate::tape_wire::decode::<Request>(&observed.payload).unwrap();
            assert_eq!(
                project(&decoded, identity).unwrap().payload,
                observed.payload
            );
        }
        let a = Request::Print {
            stdout: 1,
            bytes: vec![0, 255],
        };
        let b = Request::Print {
            stdout: 1,
            bytes: vec![255, 0],
        };
        assert_ne!(
            project(&a, identity).unwrap(),
            project(&b, identity).unwrap()
        );
        let a = Request::FileSeek {
            directory: 1,
            file: 2,
            offset: 771,
        };
        let b = Request::FileSeek {
            directory: 1,
            file: 2,
            offset: 772,
        };
        assert_ne!(
            project(&a, identity).unwrap(),
            project(&b, identity).unwrap()
        );
        for request in [
            Request::Ready { wait: 1, points: 2 },
            Request::Schedule {
                count: 3,
                choice: 1,
            },
            Request::RunnerCancel {
                key: [1; 32],
                cause: 3,
            },
            Request::Stopped,
        ] {
            assert!(project(&request, identity).is_err());
        }
    }
}
