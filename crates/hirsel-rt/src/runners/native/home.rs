//! The callee service has one owner: the launcher on its admitting thread.
//! Worker-visible Slot holds only transport state and an opaque home location.
use super::*;
use crate::io::Adapter;
use std::sync::atomic::{AtomicPtr, Ordering};

pub(crate) struct Slot {
    pub(super) transport: Arc<Transport>,
    home: AtomicPtr<Home>,
}
impl Slot {
    pub(crate) fn new(io: Arc<crate::io_real::ConcurrentIo>) -> Result<Arc<Self>, AdapterError> {
        Ok(Arc::new(Self {
            transport: Transport::new(io)?,
            home: AtomicPtr::new(std::ptr::null_mut()),
        }))
    }
}

struct Child {
    run: CalleeRun,
    token: u64,
    key: [u8; 32],
    lose_reply: bool,
    io: Rc<RefCell<RealIo>>,
    recording: bool,
}

pub(crate) struct Home {
    slot: Arc<Slot>,
    config: Config,
    children: Vec<Child>,
    tag: u64,
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
        });
        let pointer = &raw mut *home;
        assert!(home.slot.home.swap(pointer, Ordering::AcqRel).is_null());
        home
    }

    fn start(&mut self, token: u64, key: [u8; 32], spec: Spec) -> Stopped<()> {
        if spec.crash_after.is_some() {
            return Err(Stop::Refused(
                "native Free runner crash simulation is not yet admitted".into(),
            ));
        }
        let seed = |name: &str| hirsel_run_schema::callee_seed(&key, name);
        let grants = spec.grant.grants;
        self.tag += 1;
        let stdout = self
            .config
            .stdout
            .clone()
            .unwrap_or_else(|| crate::host::Sink::new(crate::launch::RawStdout));
        let mut io = RealIo::from_pinned_mounts([], stdout).expect("empty callee mounts");
        io.set_projections(projections(&grants));
        io.set_cooperative(self.slot.transport.driver.tagged(self.tag));
        io.clock_grants(
            grants.monotonic_clock.is_some(),
            grants.wall_clock.is_some(),
        );
        io.set_random(
            hirsel_run_schema::ProviderContract::RandomSplitMix64,
            seed("random"),
        );
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
        if let Some(tape) = &tape {
            let refused = |error: String| {
                Stop::Environmental(hirsel_run_schema::Environmental::Recording(format!(
                    "callee Io tape: {error}"
                )))
            };
            let file = io
                .prepare_recording_with_files(tape, &[])
                .map_err(|e| refused(e.to_string()))?;
            let tree = io.snapshot().map_err(|e| refused(format!("{e:?}")))?;
            let record = crate::launch::Recording {
                arguments: vec![],
                run_identity: [0; 32],
                hash_seed: seed("hash"),
                random_seed: seed("random"),
                test_seed: seed("test"),
                binding: None,
                tree,
                events: vec![],
            };
            let writer = crate::launch::RecordingWriter::new(file, &record)
                .map_err(|e| refused(e.to_string()))?;
            io.record_to(writer);
        }
        io.discard_history();
        let io = Rc::new(RefCell::new(io));
        let binding = Box::new(Binding::new(crate::launch::Shared(io.clone())));
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
            token,
            key,
            lose_reply: spec.lose_reply,
            io,
            recording: tape.is_some(),
        });
        Ok(())
    }

    fn poll(&mut self) -> Stopped<bool> {
        self.slot.transport.signal.drain();
        let mut changed = false;
        for (token, key, spec) in self.slot.transport.take() {
            self.start(token, key, spec)?;
            changed = true;
        }
        let mut index = 0;
        while index < self.children.len() {
            changed |= self.children[index].run.service();
            let Some(reply) = self.children[index].run.poll() else {
                index += 1;
                continue;
            };
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
            let outcome = if child.lose_reply {
                CallOutcome::InDoubt(LOST)
            } else {
                match reply.tag {
                    REPLY_OK => CallOutcome::Ok(reply.bytes),
                    REPLY_ERROR => CallOutcome::Failed(reply.bytes),
                    _ => CallOutcome::Faulted(reply.bytes),
                }
            };
            self.slot.transport.io.deliver(
                child.token,
                RunnerCompletion {
                    outcome,
                    callee: Some(hirsel_run_schema::callee_instance(&child.key)),
                },
            );
            changed = true;
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
    match unsafe { (*home).poll() } {
        Ok(changed) => changed,
        Err(stop) => {
            unsafe { (*run).control.request(stop) };
            true
        }
    }
}
