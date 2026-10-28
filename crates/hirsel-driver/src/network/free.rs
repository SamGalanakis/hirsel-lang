//! Typed native Free host phases in the evaluator. Requests and suspended
//! conversions are owned by the adapter; no OperationCall borrow escapes.
use super::*;
use hirsel_rt::free_replay::{Call, Codec, Poll};

pub(super) struct Wait {
    request: Call,
    operation: OwnedCall,
}
pub(super) struct Drop {
    request: Call,
    id: u64,
    kind: String,
}
pub(super) struct Check {
    request: Option<Call>,
    result: OperationResult,
}
pub(super) struct Runner {
    request: Option<Call>,
    key: [u8; 32],
    forwarded: bool,
    needs_check: bool,
    child: Option<u64>,
    immediate: Option<hirsel_run_schema::RunnerCompletion>,
    acknowledged: bool,
    finished: bool,
    site: hirsel_interp::observation::Anchor,
    sent: Result<u64, hirsel_run_schema::RunnerCompletion>,
    result: OperationResult,
}
struct OwnedCall {
    runtime: hirsel_interp::aggregate::RuntimeTypes,
    path: DefPath,
    generics: Vec<hirsel_types::GenericArgument>,
    arguments: Vec<Value>,
    result_type: hirsel_types::TypeId,
    site: hirsel_span::Span,
}
impl OwnedCall {
    fn new(call: OperationCall<'_>) -> Self {
        Self {
            runtime: call.runtime.clone(),
            path: call.path.clone(),
            generics: call.generics.to_vec(),
            arguments: call.arguments.to_vec(),
            result_type: call.result_type,
            site: call.site,
        }
    }
    fn call(&mut self) -> OperationCall<'_> {
        OperationCall {
            runtime: &self.runtime,
            path: &self.path,
            generics: &self.generics,
            arguments: &mut self.arguments,
            result_type: self.result_type,
            types: &self.runtime.types,
            site: self.site,
        }
    }
}
impl HostLibrary {
    pub(crate) fn enable_free_recording(
        &mut self,
        codec: Codec,
        session: hirsel_free::session::Session,
        report: Option<hirsel_rt::free_replay::Report>,
        seed: u64,
    ) {
        self.library.enable_native_free(session, seed);
        self.free = Some(codec);
        self.free_report = report;
    }

    /// The library supplies the checked request, placement decision and decode
    /// schemas. Replay advances the same one native host operation and never
    /// starts a callee or asks the legacy Io tape for an outcome.
    pub(super) fn free_runner(
        &mut self,
        result: OperationResult,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let mut sends = self.library.take_runner_sends();
        if sends.len() != 1 {
            return Err(invalid());
        }
        let send = sends.pop().unwrap();
        if !self.library.begin_native_host()? {
            return Ok(self.library.native_free_cut(cx));
        }
        let key = send.request.key;
        let request = Call::new(
            self.free.as_ref().unwrap(),
            R::RunnerCall(Box::new(send.request)),
        )
        .map_err(Error::InvalidTir)?;
        let token = self.next_wait;
        self.next_wait += 1;
        self.free_runners.insert(
            token,
            Runner {
                request: Some(request),
                key,
                forwarded: false,
                needs_check: true,
                child: if send.child {
                    let OperationResult::Complete {
                        value: Value::Handle { kind, id },
                        ..
                    } = &result
                    else {
                        return Err(invalid());
                    };
                    if kind != "Task" {
                        return Err(invalid());
                    }
                    Some(*id)
                } else {
                    None
                },
                immediate: None,
                acknowledged: false,
                finished: false,
                site: cx.anchor().cloned().ok_or_else(invalid)?,
                sent: send.sent,
                result,
            },
        );
        Ok(self
            .free_poll_runner(token, cx)?
            .unwrap_or(OperationResult::Park {
                token,
                wait: hirsel_interp::BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                    kind: "free.host.runner".into(),
                    targets: vec![],
                },
            }))
    }

    pub(super) fn free_poll_runner(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        loop {
            let mut runner = self.free_runners.remove(&token).unwrap();
            if runner.request.is_none() && runner.immediate.is_none() {
                let cause = if runner.needs_check {
                    let Some(cause) = self.library.native_runner_poll_cancellation(cx)? else {
                        self.free_runners.insert(token, runner);
                        return Ok(Some(self.library.native_free_cut(cx)));
                    };
                    runner.needs_check = false;
                    cause
                } else {
                    self.library.host_cancellation().flatten()
                };
                if !self.library.begin_native_host()? {
                    self.free_runners.insert(token, runner);
                    return Ok(Some(self.library.native_free_cut(cx)));
                }
                let cancel = cause.filter(|_| !runner.forwarded);
                runner.forwarded |= cancel.is_some();
                runner.request = Some(
                    Call::new(
                        self.free.as_ref().unwrap(),
                        R::RunnerPoll {
                            key: runner.key,
                            cancel,
                        },
                    )
                    .map_err(Error::InvalidTir)?,
                );
            }
            let poll = if let Some(completion) = runner.immediate.take() {
                Poll::Complete(Ok(C::Runner(completion)))
            } else {
                self.free_advance(runner.request.as_mut().unwrap())?
            };
            return match poll {
                Poll::Pending => {
                    self.free_runners.insert(token, runner);
                    Ok(None)
                }
                Poll::Cut => {
                    self.free_runners.insert(token, runner);
                    Ok(Some(self.library.native_free_cut(cx)))
                }
                Poll::Complete(result) => {
                    if let Some(child) = runner.child.filter(|_| !runner.acknowledged) {
                        match &result {
                            Ok(C::Ready) if runner.sent.is_ok() => {}
                            Ok(C::Runner(completion)) => {
                                if let Err(expected) = &runner.sent {
                                    if expected != completion {
                                        return Err(invalid());
                                    }
                                }
                                runner.immediate = Some(completion.clone());
                            }
                            _ => return Err(invalid()),
                        }
                        runner.request = None;
                        runner.needs_check = true;
                        runner.acknowledged = true;
                        let result =
                            self.library
                                .native_runner_spawn(child, runner.result.clone(), cx)?;
                        self.free_runners.insert(token, runner);
                        return Ok(Some(result));
                    }
                    if matches!(result, Ok(C::Ready)) {
                        runner.request = None;
                        runner.needs_check = true;
                        self.free_runners.insert(token, runner);
                        continue;
                    }
                    let completion = match result {
                        Ok(C::Runner(completion)) => completion,
                        result => {
                            return Err(Error::Unavailable(format!(
                                "Free runner result: {result:?}"
                            )));
                        }
                    };
                    let result = match &runner.sent {
                        Err(expected) if expected == &completion => {
                            if let Some(child) = runner.child {
                                self.library.native_runner_refused(child, &completion, cx)?;
                                OperationResult::returned(Value::Unit)
                            } else {
                                runner.result
                            }
                        }
                        Err(_) => return Err(invalid()),
                        Ok(target) => {
                            self.library.runner_completed(*target, completion, cx)?;
                            if runner.child.is_some() {
                                OperationResult::returned(Value::Unit)
                            } else {
                                self.library.poll(*target, cx)?.ok_or_else(invalid)?
                            }
                        }
                    };
                    runner.finished = true;
                    let path = self.library.task_path();
                    if self
                        .library
                        .with_free_session(|session| Ok(session.at_cut(&path)))?
                    {
                        // Keep decoded owners alive until the terminal cut ends
                        // the execution, as native saved result storage does.
                        runner.result = result;
                        self.free_runners.insert(token, runner);
                        Ok(Some(self.library.native_free_cut(cx)))
                    } else {
                        Ok(Some(result))
                    }
                }
            };
        }
    }
    /// Spawned runner waiters have no source frames in the reference engine.
    /// Consume their own registered phase logs at scheduling points, never
    /// the current parent's log, and leave key-dependent waits pending.
    pub(super) fn free_runner_children(
        &mut self,
        cx: &mut OperationContext<'_>,
    ) -> Result<(), Error> {
        loop {
            let children: Vec<_> = self
                .free_runners
                .iter()
                .filter(|(_, runner)| {
                    runner.child.is_some() && runner.acknowledged && !runner.finished
                })
                .map(|(&token, runner)| (token, runner.child.unwrap(), runner.site.clone()))
                .collect();
            let mut progressed = false;
            for (token, child, site) in children {
                let previous = self.library.native_runner_enter_child(child)?;
                let path = self.library.task_path();
                let before = self.library.with_free_session(|s| Ok(s.progress(&path)))?;
                let result = self.free_poll_runner(
                    token,
                    &mut OperationContext {
                        configuration: cx.configuration,
                        sources: cx.sources,
                        stdout: cx.stdout,
                        trace: cx.trace,
                        trace_mode: cx.trace_mode,
                        anchor: Some(&site),
                    },
                );
                let after = self.library.with_free_session(|s| Ok(s.progress(&path)))?;
                self.library.native_runner_restore_task(previous);
                result?;
                progressed |= before != after;
            }
            // Siblings can enable each other's next key epoch. A pass with
            // no publication leaves the genuinely pending dependency alone.
            if !progressed {
                return Ok(());
            }
        }
    }

    pub(super) fn free_request(&self, name: &str, a: &[Value]) -> Result<Option<R>, Error> {
        Ok(Some(match name {
            "io.stdout" => R::Stdout { io: 1 },
            "io.stderr" => R::Stderr { io: 1 },
            "io.stdin" => R::Stdin { io: 1 },
            "io.tasks" => R::Tasks { io: 1 },
            "io.random" => R::Random { io: 1 },
            "io.signals" => R::Signals { io: 1 },
            "env.get" | "env.narrow" | "stdin.read" => self.input_request(name, a)?,
            "signal.wait" => self.signal_request(a)?,
            "watcher.next" => self.watch_host_request(a)?,
            "mapped.with" => R::MapView {
                directory: self.id(&a[0])?,
                mapped: self.id(&a[1])?,
            },
            "random.bytes" if word(&a[1])? != 0 => R::RandomBytes {
                random: self.id(&a[0])?,
                count: word(&a[1])?,
            },
            "clock.sleep_until" => {
                let Value::Handle { kind, id } = &a[1] else {
                    return Err(invalid());
                };
                if kind != "Instant" {
                    return Err(invalid());
                }
                R::SleepUntil {
                    clock: self.id(&a[0])?,
                    deadline: *id,
                }
            }
            "out.print" | "out.write" => R::Print {
                stdout: self.id(&a[0])?,
                bytes: bytes(&a[1])?,
            },
            "err.print" | "err.write" => R::PrintStderr {
                stderr: self.id(&a[0])?,
                bytes: bytes(&a[1])?,
            },
            _ if matches!(
                host_route(name),
                HostRoute::Network | HostRoute::NetProjection
            ) =>
            {
                return match self.network_request(name, a)? {
                    Some(request) => Ok(Some(request)),
                    None if matches!(
                        name,
                        "tls.connect" | "tls.connect_as" | "tls.connect_with" | "tls.accept"
                    ) =>
                    {
                        self.tls_request(name, a)
                            .map(|(request, _setup)| Some(request))
                    }
                    None => Ok(None),
                };
            }
            _ if matches!(host_route(name), HostRoute::Telemetry) => {
                return self.telemetry_request(name, a);
            }
            _ if matches!(host_route(name), HostRoute::Process) => {
                if matches!(name, "io.process" | "proc.narrow") {
                    self.process_projection_request(name, a)?
                } else {
                    return if matches!(name, "proc.run" | "proc.pipeline") {
                        self.process_run_request(name, a)
                    } else {
                        self.child_host_request(name, a)
                    };
                }
            }
            _ if matches!(host_route(name), HostRoute::Storage | HostRoute::Projection) => {
                self.storage_request(name, a)?
            }
            _ => return Ok(None),
        }))
    }
    pub(super) fn free_advance(&mut self, request: &mut Call) -> Result<Poll, Error> {
        let path = self.library.task_path();
        let cancellation = self.library.host_cancellation();
        let codec = self.free.as_mut().unwrap();
        let io = &mut self.io;
        let (result, progressed) = self.library.with_free_session(|session| {
            let before = session.progress(&path);
            let result = match io {
                HostIo::Live(io) => {
                    request.advance_live(codec, session, &path, cancellation, io)?
                }
                HostIo::Replay(_) => request.advance(codec, session, &path, cancellation)?,
            };
            Ok((result, before != session.progress(&path)))
        })?;
        self.free_progressed |= progressed;
        Ok(result)
    }
    fn free_convert(
        &mut self,
        request: R,
        completion: Result<C, hirsel_rt::io::AdapterError>,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if call.path == &DefPath::core("iter.next") {
            return self.walk_next(call, cx, Some((request, completion)));
        }
        let operation =
            hirsel_interp::OperationsAdapter::resolve(self, call.path).ok_or_else(invalid)?;
        self.free_ready = Some((request, completion));
        let result = hirsel_interp::OperationsAdapter::call_resolved(self, operation, call, cx)?
            .ok_or_else(invalid)?;
        if self.free_ready.is_some() {
            return Err(invalid());
        }
        self.echo(cx.stdout)?;
        let path = self.library.task_path();
        if self
            .library
            .with_free_session(|session| Ok(session.at_cut(&path)))?
        {
            return Ok(self.library.native_free_cut(cx));
        }
        Ok(result)
    }
    pub(super) fn free_call(
        &mut self,
        request: R,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if !self.library.begin_native_host()? {
            return Ok(self.library.native_free_cut(cx));
        }
        let mut request =
            Call::new(self.free.as_ref().unwrap(), request).map_err(|e| Error::InvalidTir(e))?;
        match self.free_advance(&mut request)? {
            Poll::Complete(result) => {
                self.free_convert(request.request().clone(), result, call, cx)
            }
            Poll::Cut => Ok(self.library.native_free_cut(cx)),
            Poll::Pending => {
                let token = self.next_wait;
                self.next_wait += 1;
                let wait = hirsel_interp::BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                    kind: "free.host".into(),
                    targets: vec![],
                };
                self.free_waits.insert(
                    token,
                    Wait {
                        request,
                        operation: OwnedCall::new(call),
                    },
                );
                Ok(OperationResult::Park { token, wait })
            }
        }
    }
    /// The existing typed wait owns the walk cursor as its iter.next argument.
    pub(super) fn park_walk(
        &mut self,
        request: Call,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let token = self.next_wait;
        self.next_wait += 1;
        let wait = hirsel_interp::BlockedWait {
            spawn_path: self.library.task_path(),
            anchor: cx.anchor().cloned().ok_or_else(invalid)?,
            kind: "free.host.walk".into(),
            targets: vec![],
        };
        self.free_waits.insert(
            token,
            Wait {
                request,
                operation: OwnedCall::new(call),
            },
        );
        Ok(OperationResult::Park { token, wait })
    }
    pub(super) fn free_poll(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let mut wait = self.free_waits.remove(&token).unwrap();
        match self.free_advance(&mut wait.request)? {
            Poll::Pending => {
                self.free_waits.insert(token, wait);
                Ok(None)
            }
            Poll::Cut => Ok(Some(self.library.native_free_cut(cx))),
            Poll::Complete(result) => self
                .free_convert(
                    wait.request.request().clone(),
                    result,
                    wait.operation.call(),
                    cx,
                )
                .map(Some),
        }
    }
    /// The callback result remains owned while the final map check is pending.
    pub(super) fn free_mapped_check(
        &mut self,
        directory: u64,
        mapped: u64,
        result: OperationResult,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        let entered = self.library.begin_native_host()?;
        let request = if entered {
            Some(
                Call::new(
                    self.free.as_ref().unwrap(),
                    R::MapCheck { directory, mapped },
                )
                .map_err(Error::InvalidTir)?,
            )
        } else {
            None
        };
        let token = self.next_wait;
        self.next_wait += 1;
        self.free_checks.insert(token, Check { request, result });
        if !entered {
            return Ok(self.library.native_free_cut(cx));
        }
        Ok(self
            .free_poll_check(token, cx)?
            .unwrap_or(OperationResult::Park {
                token,
                wait: hirsel_interp::BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                    kind: "free.host.map_check".into(),
                    targets: vec![],
                },
            }))
    }
    pub(super) fn free_poll_check(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let mut check = self.free_checks.remove(&token).unwrap();
        let Some(request) = &mut check.request else {
            self.free_checks.insert(token, check);
            return Ok(Some(self.library.native_free_cut(cx)));
        };
        match self.free_advance(request)? {
            Poll::Pending => {
                self.free_checks.insert(token, check);
                Ok(None)
            }
            Poll::Cut => {
                self.free_checks.insert(token, check);
                Ok(Some(self.library.native_free_cut(cx)))
            }
            Poll::Complete(completion) => {
                let result = mapped_checked(
                    check.result,
                    completion.map_err(|e| Error::Unavailable(format!("Free map check: {e:?}")))?,
                )?;
                let path = self.library.task_path();
                if self
                    .library
                    .with_free_session(|session| Ok(session.at_cut(&path)))?
                {
                    // Retain the result until the cut ends the run, as native frames do.
                    self.free_checks.insert(
                        token,
                        Check {
                            request: None,
                            result,
                        },
                    );
                    Ok(Some(self.library.native_free_cut(cx)))
                } else {
                    Ok(Some(result))
                }
            }
        }
    }
    pub(super) fn free_drop(
        &mut self,
        kind: &str,
        id: u64,
        handle: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
        if !self.library.begin_native_host()? {
            return Ok(self.library.native_free_cut(cx));
        }
        let request = Call::new(self.free.as_ref().unwrap(), R::Close { handle })
            .map_err(Error::InvalidTir)?;
        let token = self.next_wait;
        self.next_wait += 1;
        self.free_drops.insert(
            token,
            Drop {
                request,
                id,
                kind: kind.into(),
            },
        );
        Ok(self
            .free_poll_drop(token, cx)?
            .unwrap_or(OperationResult::Park {
                token,
                wait: hirsel_interp::BlockedWait {
                    spawn_path: self.library.task_path(),
                    anchor: cx.anchor().cloned().ok_or_else(invalid)?,
                    kind: "free.host.close".into(),
                    targets: vec![],
                },
            }))
    }
    pub(super) fn free_poll_drop(
        &mut self,
        token: u64,
        cx: &mut OperationContext<'_>,
    ) -> Result<Option<OperationResult>, Error> {
        let mut drop = self.free_drops.remove(&token).unwrap();
        match self.free_advance(&mut drop.request)? {
            Poll::Pending => {
                self.free_drops.insert(token, drop);
                Ok(None)
            }
            Poll::Cut => Ok(Some(self.library.native_free_cut(cx))),
            Poll::Complete(completion) => {
                match completion.map_err(|e| Error::Unavailable(format!("Free close: {e:?}")))? {
                    C::Unit(Ok(())) => {}
                    C::Checked(false) if matches!(drop.kind.as_str(), "Child" | "ChildGroup") => {
                        self.library.fail_drop("scripted child unmet");
                    }
                    _ => return Err(invalid()),
                }
                self.handles.remove(&drop.id);
                let path = self.library.task_path();
                Ok(Some(
                    if self
                        .library
                        .with_free_session(|session| Ok(session.at_cut(&path)))?
                    {
                        self.library.native_free_cut(cx)
                    } else {
                        OperationResult::returned(Value::Unit)
                    },
                ))
            }
        }
    }
    pub fn finish_free(
        &mut self,
        outcome: &hirsel_run_schema::RunOutcome,
    ) -> Result<Option<hirsel_run_schema::RunOutcome>, String> {
        if self.free.is_none() {
            return Ok(None);
        }
        if let Some(expected) = self.library.take_native_root_observation() {
            Codec::check_outcome(&expected, outcome)?;
        }
        let root_pending = self.library.native_root_pending();
        let session = self
            .library
            .take_free()
            .ok_or("missing Free phase session")?;
        let projection = if session.is_replay() {
            session.finish().map_err(|e| e.to_string())?
        } else {
            Codec::finish_recording(session, outcome, root_pending)?
        };
        if let Some(report) = &mut self.free_report {
            report.write(&projection)?;
        }
        Codec::finished_outcome(&projection)
    }
}
