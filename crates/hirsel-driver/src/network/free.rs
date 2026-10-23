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
    fn free_advance(&mut self, request: &mut Call) -> Result<Poll, Error> {
        let path = self.library.task_path();
        let cancellation = self.library.host_cancellation();
        let codec = self.free.as_mut().unwrap();
        self.library
            .with_free_session(|session| request.advance(codec, session, &path, cancellation))
    }
    fn free_convert(
        &mut self,
        request: R,
        completion: Result<C, hirsel_rt::io::AdapterError>,
        call: OperationCall<'_>,
        cx: &mut OperationContext<'_>,
    ) -> Result<OperationResult, Error> {
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
        let projection = self
            .library
            .take_free()
            .ok_or("missing Free replay session")?
            .finish()
            .map_err(|e| e.to_string())?;
        if let Some(report) = &mut self.free_report {
            report.write(&projection)?;
        }
        Codec::finished_outcome(&projection)
    }
}
