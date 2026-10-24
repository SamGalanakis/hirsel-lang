//! Reuse RealIo, the existing process HostJob and epoll readiness. The shell
//! waits for a mounted file that a different Hirsel fiber writes, while both
//! requests take the same run-wide namespace key for each bounded transition.
use super::*;
use crate::io::{Adapter, Ready};
use hirsel_run_schema::{Command, ExitStatus, ProcessOutcome};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

struct Probe {
    run: AtomicPtr<crate::context::RunShared>,
    started: AtomicBool,
    written: AtomicBool,
    process_live: AtomicBool,
    directory: u64,
    replay: bool,
}
struct Home {
    real: crate::io_real::RealIo,
    thread: std::thread::ThreadId,
    probe: Arc<Probe>,
}
impl Adapter for Home {
    fn preflight(&self, r: &Request) -> Result<Option<Completion>, AdapterError> {
        self.real.preflight(r)
    }
    fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
        assert_eq!(std::thread::current().id(), self.thread);
        let process = matches!(request, Request::ProcessRun { .. });
        if matches!(request, Request::Write { .. }) {
            assert!(self.probe.started.load(Ordering::Acquire));
            assert!(
                self.probe.process_live.load(Ordering::Acquire),
                "writer must finish while the process is still in flight"
            );
        }
        let result = self.real.call(request);
        if process {
            self.probe.process_live.store(
                matches!(result, Err(AdapterError::Blocked)),
                Ordering::Release,
            );
            if matches!(result, Err(AdapterError::Blocked)) {
                self.probe.started.store(true, Ordering::Release);
                // Only scheduling metadata; neither the adapter nor any
                // parked context is borrowed while the role is held.
                let run = self.probe.run.load(Ordering::Acquire);
                let _guard = unsafe { role::IdleGuard::new(run) };
                let s = unsafe { (*(*run).tasks.get()).as_deref_mut().unwrap() };
                for id in std::mem::take(&mut s.retrying) {
                    s.task(id).retried = true;
                    s.wake(id);
                }
                unsafe {
                    crate::tasks::readiness::poll_free(s);
                    super::super::publish_ready(s, run);
                }
            }
        }
        result
    }
    fn enter(&mut self, token: u64) {
        self.real.enter(token);
    }
    fn parked(&mut self, token: u64) {
        self.real.parked(token);
    }
    fn finish(&mut self, token: u64) {
        self.real.finish(token);
    }
    fn ready(&mut self) -> Result<Ready, AdapterError> {
        let ready = self.real.ready();
        ready
    }
    fn wait(&mut self) -> Result<bool, AdapterError> {
        self.real.wait()
    }
    fn wake_on(&mut self, fd: std::os::fd::RawFd) -> bool {
        self.real.wake_on(fd)
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        assert_eq!(std::thread::current().id(), self.thread);
    }
}

fn namespace(key: u32) -> KeySet {
    KeySet::new(vec![
        (crate::io_keys::Key(key), Mode::Exclusive),
        (crate::io_keys::NAMESPACE, Mode::Exclusive),
    ])
}
unsafe fn wait_flag(ctx: *mut TaskCtx, flag: &AtomicBool) {
    while !flag.load(Ordering::Acquire) {
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe {
            crate::stop::check(
                ctx,
                try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0),
            )
        };
    }
}
unsafe extern "C" fn writer(ctx: *mut u8, data: *mut u8) -> ! {
    let ctx = ctx.cast::<TaskCtx>();
    let probe = unsafe { &*data.cast::<Probe>() };
    let role = unsafe { role::Guard::new(ctx) };
    unsafe {
        if !probe.replay {
            wait_flag(ctx, &probe.started);
        }
        let request = Request::Write {
            directory: probe.directory,
            path: "trigger".into(),
            bytes: b"ready from other task\n".to_vec(),
        };
        let result = if probe.replay {
            replay_request(ctx, request, namespace(18))
        } else {
            request_home(ctx, request, namespace(18))
        };
        assert_eq!(
            crate::stop::check(ctx, result),
            Ok(Completion::Unit(Ok(())))
        );
        probe.written.store(true, Ordering::Release);
        wake_retrying(ctx);
        if probe.replay {
            super::super::replay::start(ctx);
            unreachable!("writer past cut");
        }
        // This raw dispatcher fixture has no language TaskFinish wrapper.
        // Request logical completion in the saved-frame callback before
        // relinquishing the image. An unset finishing flag leaves Ready and
        // prevents the home loop from waiting for Io.
        let s = scheduler(ctx);
        let me = s.current;
        s.task(me).finishing = true;
        drop(role);
        role::enter(ctx);
        crate::pool::free::finish_current(ctx);
    }
}
unsafe fn spawn_writer(ctx: *mut TaskCtx, probe: &Arc<Probe>) {
    unsafe {
        let stack = Stack::new();
        let child_ctx = stack.context();
        child_ctx.write(TaskCtx::new((*ctx).worker_ptr(), (*ctx).run_ptr()));
        let path = if probe.replay {
            let path = vec![0];
            super::super::record::begin(ctx);
            assert!(crate::stop::check(
                ctx,
                super::super::record::publish(ctx, Phase::Done, |_| super::super::replay::child(
                    ctx, &path
                ))
            ));
            path
        } else {
            let child = crate::stop::check(ctx, super::super::record::child(ctx));
            let path = child.path().to_vec();
            (*child_ctx).recording = Some(Box::new(child));
            path
        };
        let s = scheduler(ctx);
        let id = s.tasks.len() as u32;
        (*child_ctx).task_id = id;
        (*child_ctx).task_sp = stack.prepare(
            writer,
            child_ctx.cast(),
            Arc::as_ptr(probe).cast_mut().cast(),
        );
        let mut row = crate::tasks::Task::new(1, child_ctx, None, path, None);
        row.stack = Some(stack);
        s.tasks.push(Some(row));
        crate::pool::free::schedule_saved((*ctx).run_ptr(), id, Owner::Any);
    }
}
fn process_request(process: u64, directory: u64) -> Request {
    Request::ProcessRun {
        process, command: Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "i=0; while [ ! -f trigger ]; do i=$((i + 1)); [ \"$i\" -lt 1000 ] || exit 42; /bin/sleep 0.001; done; /bin/cat trigger; printf observed >&2".into()],
            directory: Some(directory), env: Some(vec![]), own_group: true,
        }, stdin: vec![], max_output: 4096, deadline: None,
    }
}
fn expected() -> Completion {
    Completion::Process(ProcessOutcome::Output {
        status: ExitStatus::Exited(0),
        stdout: b"ready from other task\n".to_vec(),
        stderr: b"observed".to_vec(),
    })
}

#[test]
fn native_process_wait_releases_namespace_for_other_task_file_write_and_replays_without_mount() {
    const CHILD: &str = "HIRSEL_F1_PROCESS_FILE";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tasks::free::host::process_tests::native_process_wait_releases_namespace_for_other_task_file_write_and_replays_without_mount", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    crate::pool::set_compute_grant(1);
    for width in [1, 8] {
        for durable in [false, true] {
            let directory = std::env::temp_dir().join(format!(
                "hirsel-free-process-file-{}-{width}-{durable}",
                std::process::id()
            ));
            std::fs::create_dir(&directory).unwrap();
            let mut real = crate::io_real::RealIo::grant(&directory).unwrap();
            real.set_cooperative(Default::default());
            real.process(crate::process::host);
            real.set_process_grant(&hirsel_run_schema::ProcessGrant {
                timed: None,
                provider: "process.spawn@1".into(),
                rules: vec![hirsel_run_schema::ProcessRule {
                    program: "/bin/sh".into(),
                    env: vec![],
                }],
                runs: None,
                children: None,
            });
            real.discard_history();
            let Ok(Completion::Handle(Ok(process))) = real.call(Request::Process { io: 1 }) else {
                panic!("process grant")
            };
            let Ok(Completion::Handle(Ok(dir))) = real.call(Request::Directory {
                io: 1,
                path: String::new(),
            }) else {
                panic!("directory grant")
            };
            let recording = super::super::record::Recording::new(b"native process file", durable);
            let install = recording.clone();
            let probe = Arc::new(Probe {
                run: AtomicPtr::default(),
                started: false.into(),
                written: false.into(),
                process_live: false.into(),
                directory: dir,
                replay: false,
            });
            let observed = probe.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            run.adapter = Some(Box::new(Home {
                real,
                thread: std::thread::current().id(),
                probe: probe.clone(),
            }));
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    observed.run.store((*ctx).run_ptr(), Ordering::Release);
                    spawn_writer(ctx, &observed);
                    let result = crate::stop::check(
                        ctx,
                        request_home(ctx, process_request(process, dir), namespace(17)),
                    );
                    assert_eq!(result, Ok(expected()));
                    wait_flag(ctx, &observed.written);
                    STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(STATUS_OK));
            assert_eq!(outcome.pending, 0);
            assert!(probe.written.load(Ordering::Acquire));
            assert_eq!(
                std::fs::read(directory.join("trigger")).unwrap(),
                b"ready from other task\n"
            );
            std::fs::remove_dir_all(&directory).unwrap();
            drop(probe);
            let cut = recording
                .recorder
                .stop("returned", None)
                .unwrap()
                .unwrap()
                .0;
            let expected_projection = hirsel_free::projection::Projection::new(
                "returned",
                &cut.progress,
                &recording.trace(),
            );
            let bytes = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            for width in [1, 8] {
                let replay = super::super::replay::Replay::new(&bytes).unwrap();
                let install = replay.clone();
                let probe = Arc::new(Probe {
                    run: AtomicPtr::default(),
                    started: false.into(),
                    written: false.into(),
                    process_live: false.into(),
                    directory: dir,
                    replay: true,
                });
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let _role = role::Guard::new(ctx);
                        spawn_writer(ctx, &probe);
                        let result = crate::stop::check(
                            ctx,
                            replay_request(ctx, process_request(process, dir), namespace(17)),
                        );
                        assert_eq!(result, Ok(expected()));
                        wait_flag(ctx, &probe.written);
                        super::super::replay::start(ctx);
                        unreachable!("process root past cut");
                    },
                );
                assert!(
                    matches!(outcome.ended, Err(Stop::Blocked(_))),
                    "{outcome:?}"
                );
                assert_eq!(replay.finish().unwrap(), expected_projection);
                assert!(!directory.exists());
            }
        }
    }
}

#[test]
fn owned_process_results_preserve_all_payloads_and_reject_creation_metadata() {
    use hirsel_run_schema::{GroupOutput, PipelineOutcome, ProcessFailure};
    let values = [
        expected(),
        Completion::Process(ProcessOutcome::Error(ProcessFailure::Fault(37))),
        Completion::Pipeline(PipelineOutcome::Output {
            statuses: vec![ExitStatus::Exited(7), ExitStatus::Signaled(15)],
            stdout: vec![0, 255, 10],
            stderr: vec![128, 1],
        }),
        Completion::Pipeline(PipelineOutcome::Error(ProcessFailure::TimedOut)),
        Completion::ChildUnit(Ok(())),
        Completion::ChildUnit(Err(ProcessFailure::InputClosed)),
        Completion::ChildBytes(Ok(vec![0, 255, 42])),
        Completion::ChildBytes(Err(ProcessFailure::Canceled(3))),
        Completion::ChildExit(Ok(ExitStatus::Signaled(9))),
        Completion::ChildExit(Err(ProcessFailure::NotFound)),
        Completion::GroupExit(Ok(GroupOutput {
            statuses: vec![ExitStatus::Exited(0), ExitStatus::Exited(42)],
            stderr: vec![vec![0], vec![255]],
        })),
        Completion::GroupExit(Err(ProcessFailure::OutputLimit)),
    ];
    for value in values {
        let mut result = Ok(value);
        assert!(
            handles::fields(&mut result)
                .unwrap()
                .iter()
                .all(Option::is_none)
        );
        let mut bytes = Vec::new();
        Envelope {
            refusal: None,
            output_count: None,
            result: result.clone(),
            created: vec![],
        }
        .encode(&mut bytes);
        let envelope = crate::tape_wire::decode(&bytes).unwrap();
        let mut mapping = handles::Handles::default();
        assert_eq!(mapping.substitute(envelope).unwrap(), result);
        assert!(
            mapping
                .substitute(Envelope {
                    refusal: None,
                    output_count: None,
                    result,
                    created: vec![Created {
                        process_keys: None,
                        net_ports: None,
                        path: vec![],
                        ordinal: 1,
                        resource: 17,
                        kind: crate::io_keys::Kind::Other,
                    }]
                })
                .is_err()
        );
    }
}
