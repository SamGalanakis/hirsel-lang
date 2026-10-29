//! The callee service has one owner: the launcher on its admitting thread.
//! Worker-visible Slot holds only transport state and an opaque home location.
use super::*;
use crate::io::Adapter;
use std::sync::atomic::{AtomicPtr, Ordering};

pub(crate) struct Slot {
    pub(super) transport: Arc<Transport>,
    home: AtomicPtr<Home>,
    poll: unsafe fn(*mut Home, *mut RunShared) -> Stopped<bool>,
}
impl Slot {
    pub(crate) fn new(io: Arc<crate::io_real::ConcurrentIo>) -> Result<Arc<Self>, AdapterError> {
        Ok(Arc::new(Self {
            transport: Transport::new(io)?,
            home: AtomicPtr::new(std::ptr::null_mut()),
            poll: poll_home,
        }))
    }
}

unsafe fn poll_home(home: *mut Home, run: *mut RunShared) -> Stopped<bool> {
    // SAFETY: only the admission owner calls the slot's poll hook.
    unsafe { (*home).poll(run) }
}

struct Child {
    run: CalleeRun,
    key: [u8; 32],
    lose_reply: bool,
    crashing: bool,
    io: Rc<RefCell<RealIo>>,
    recording: bool,
}

pub(crate) struct Home {
    slot: Arc<Slot>,
    config: Config,
    children: Vec<Child>,
    tag: u64,
    driver_installed: bool,
}
impl Home {
    /// # Safety
    /// Sole admission owner before root publication. The returned stable box
    /// stays home and outlives every call to service on this run.
    pub(crate) unsafe fn new(slot: Arc<Slot>, config: Config) -> Box<Self> {
        let mut home = Box::new(Self {
            slot,
            config,
            children: Vec::new(),
            tag: 0,
            driver_installed: false,
        });
        let pointer = &raw mut *home;
        assert!(home.slot.home.swap(pointer, Ordering::AcqRel).is_null());
        home
    }

    fn start_callee(&mut self, _token: u64, key: [u8; 32], spec: Spec) -> Stopped<()> {
        let seed = |name: &str| hirsel_run_schema::callee_seed(&key, name);
        let grants = spec.grant.grants;
        self.tag += 1;
        let stdout = self
            .config
            .stdout
            .clone()
            .unwrap_or_else(|| crate::host::Sink::new(crate::launch::RawStdout));
        // Follow the source host's host_callee admission: the same checked
        // callee facts, narrowed grants, cause and derived seeds bind its tape.
        let facts = self
            .config
            .callees
            .get(&spec.request.path)
            .ok_or_else(|| Stop::Refused("missing checked callee facts".into()))?;
        let cause = hirsel_run_schema::Cause {
            instance: spec.caller,
            ordinal: spec.ordinal,
            digest: spec.request.digest,
        };
        let mut record = hirsel_run_config::RunConfig::empty(
            facts.program,
            spec.request.path.clone(),
            hirsel_run_schema::LaunchKind::Call,
        );
        record.call = Some(hirsel_run_schema::CallRecord::new(
            spec.request.form,
            &cause,
            &spec.request.argument,
        ));
        record.inputs.hash_seed = seed("hash");
        record.inputs.random_seed = seed("random");
        record.inputs.test_seed = seed("test");
        record.io = (spec.request.form == CallForm::Call).then_some(grants);
        record.admission =
            spec.grant
                .entry
                .admission
                .as_ref()
                .map(|a| hirsel_run_schema::Admission {
                    not_before: None,
                    expires: None,
                    revocation: None,
                    code: a.code.clone(),
                });
        let refused = |error: hirsel_run_schema::RunRefusal| {
            Stop::Refused(format!("callee admission: {error}"))
        };
        let bound = record
            .validated()
            .map_err(refused)?
            .bind(&hirsel_run_config::ProgramFacts {
                program: facts.program,
                entry: spec.request.path.clone(),
                launch: hirsel_run_schema::LaunchKind::Call,
                roots: if spec.request.form == CallForm::Call {
                    vec![hirsel_run_config::Root::Io]
                } else {
                    vec![]
                },
                code: facts.reach.clone(),
                fueled: true,
            })
            .map_err(refused)?;
        let mut host = crate::host_launch::HostGrants::sharing(
            stdout,
            self.slot.transport.driver.tagged(self.tag),
        );
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| Stop::Refused(e.to_string()))?
            .as_nanos();
        let now = i64::try_from(now).map_err(|e| Stop::Refused(e.to_string()))?;
        let admitted = bound.admit(&mut host, now).map_err(refused)?;
        let mut io = host.into_io();
        let tape = self
            .config
            .tape
            .as_ref()
            .filter(|_| spec.grant.entry.recording.as_deref() != Some("none"))
            .map(|tape| {
                let mut name = tape.clone().into_os_string();
                name.push(format!(".call-{}", spec.ordinal));
                std::path::PathBuf::from(name)
            });
        crate::host_launch::journal(&mut io, &admitted, tape.as_deref(), &[]).map_err(|e| {
            Stop::Environmental(hirsel_run_schema::Environmental::Recording(format!(
                "callee Io tape: {e}"
            )))
        })?;
        io.discard_history();
        let io = Rc::new(RefCell::new(io));
        let binding = match spec.crash_after {
            Some(point) => Binding::new(super::callee_io::CalleeIo::new(io.clone(), point)),
            None => Binding::new(crate::launch::Shared(io.clone())),
        };
        let binding = Box::new(binding);
        let mut runtime = self.config.runtime;
        runtime.hash_seed = seed("hash");
        let run = unsafe {
            CalleeRun::start(
                runtime,
                self.config.failure_table,
                spec.start,
                binding,
                self.slot.transport.driver.clone(),
            )
        };
        unsafe { run.notify_on(self.slot.transport.signal.clone()) };
        self.children.push(Child {
            run,
            key,
            lose_reply: spec.lose_reply,
            crashing: spec.crash_after.is_some(),
            io,
            recording: tape.is_some(),
        });
        Ok(())
    }

    fn poll(&mut self, root: *mut RunShared) -> Stopped<bool> {
        if !self.driver_installed {
            unsafe { crate::pool::home_driver(root, self.slot.transport.driver.clone()) };
            self.driver_installed = true;
        }
        self.slot.transport.signal.drain();
        let mut changed = false;
        for (token, key, spec) in self.slot.transport.take() {
            self.start_callee(token, key, spec)?;
            changed = true;
        }
        for key in self.slot.transport.cancels() {
            if let Some(child) = self.children.iter().find(|child| child.key == key) {
                unsafe { crate::tasks::free::home::runner_cancel((*child.run.root).run_ptr()) };
                changed = true;
            }
        }
        let mut waiters = Vec::new();
        let mut index = 0;
        while index < self.children.len() {
            changed |= self.children[index].run.service();
            let Some(reply) = self.children[index].run.poll() else {
                index += 1;
                continue;
            };
            if matches!(
                &reply,
                Err(Stop::Environmental(
                    hirsel_run_schema::Environmental::Stopped
                ))
            ) && self.children[index].crashing
            {
                let child = self.children.swap_remove(index);
                if child.recording {
                    child
                        .io
                        .borrow_mut()
                        .finish_recording(&hirsel_run_schema::RunOutcome::Environmental(
                            hirsel_run_schema::Environmental::Stopped,
                        ))
                        .map_err(|e| {
                            Stop::Environmental(hirsel_run_schema::Environmental::Recording(
                                format!("callee Io trailer: {e}"),
                            ))
                        })?;
                }
                if let Some(waiter) = self.slot.transport.complete(
                    child.key,
                    RunnerCompletion {
                        outcome: CallOutcome::InDoubt(ABANDONED),
                        callee: Some(hirsel_run_schema::callee_instance(&child.key)),
                    },
                ) {
                    waiters.push(waiter);
                }
                changed = true;
                continue;
            }
            let reply = reply?;
            let child = self.children.swap_remove(index);
            let returned = match reply.tag {
                REPLY_OK => CalleeOutcome::Ok(reply.bytes.clone()),
                REPLY_ERROR => CalleeOutcome::Err(reply.bytes.clone()),
                _ => CalleeOutcome::Failed(reply.bytes.clone()),
            };
            if !matches!(
                child.io.borrow_mut().call(Request::CallReturn(returned)),
                Ok(Completion::Ready)
            ) {
                return Err(Stop::refusal(AdapterError::InvalidCompletion));
            }
            if child.recording {
                use hirsel_run_schema::{Returned, RunOutcome};
                let outcome = match reply.tag {
                    REPLY_OK => RunOutcome::Returned(Returned::Ok),
                    REPLY_ERROR => RunOutcome::Returned(Returned::Error),
                    _ => {
                        let table = self.config.failure_table.expect("callee failure table");
                        let mut failures =
                            reply.chain.iter().map(|(record, _)| table.failure(record));
                        RunOutcome::Failed {
                            primary: failures.next().expect("callee primary failure"),
                            causes: failures.collect(),
                        }
                    }
                };
                child
                    .io
                    .borrow_mut()
                    .finish_recording(&outcome)
                    .map_err(|e| {
                        Stop::Environmental(hirsel_run_schema::Environmental::Recording(format!(
                            "callee Io trailer: {e}"
                        )))
                    })?;
            }
            let outcome = if child.crashing {
                // A callee that ends before its selected crash point ran to
                // completion, but the simulated place still loses its outcome.
                CallOutcome::InDoubt(ABANDONED)
            } else if child.lose_reply {
                CallOutcome::InDoubt(LOST)
            } else {
                match reply.tag {
                    REPLY_OK => CallOutcome::Ok(reply.bytes),
                    REPLY_ERROR => CallOutcome::Failed(reply.bytes),
                    _ => CallOutcome::Faulted(reply.bytes),
                }
            };
            if let Some(waiter) = self.slot.transport.complete(
                child.key,
                RunnerCompletion {
                    outcome,
                    callee: Some(hirsel_run_schema::callee_instance(&child.key)),
                },
            ) {
                waiters.push(waiter);
            }
            changed = true;
        }
        unsafe { crate::tasks::free::home::runner_wakes(root, &waiters) };
        if !changed && self.slot.transport.pending() && !unsafe { crate::pool::home_posts(root) } {
            crate::readiness::Readiness::wait_many(&[
                &self.slot.transport.driver,
                &self.slot.transport.work,
            ])
            .map_err(Stop::refusal)?;
            return Ok(true);
        }
        Ok(changed)
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        self.slot
            .home
            .store(std::ptr::null_mut(), Ordering::Release);
        // Children are destroyed before config/Sink on this home thread;
        // CalleeRun closes its bridge before its Rc adapter is dropped.
        self.children.clear();
    }
}

/// # Safety
/// Admission's home loop, never from program code or a worker. It serializes
/// all access to Home with native runner metadata entries served on home.
pub(crate) unsafe fn service(run: *mut RunShared) -> bool {
    let Some(slot) = (unsafe { (*run).runner_home.as_ref() }) else {
        return false;
    };
    let home = slot.home.load(Ordering::Acquire);
    if home.is_null() {
        return false;
    }
    match unsafe { (slot.poll)(home, run) } {
        Ok(changed) => changed,
        Err(stop) => {
            unsafe { (*run).control.request(stop) };
            true
        }
    }
}
