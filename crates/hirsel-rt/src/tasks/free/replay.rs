//! Native replay admission, shared with the F0 Session decoder and projection.
//! A mutex serializes session mutations. No session borrow or lock
//! crosses a physical save; atomic carriers are released before cut parking.
use super::super::*;
use hirsel_free::projection::Projection;
use hirsel_free::replay::Diverged;
use hirsel_free::session::Session;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) struct Replay {
    session: Mutex<Option<Session>>,
    published: AtomicBool,
    handles: Mutex<super::host::handles::Handles>,
    output: Mutex<(Vec<u8>, Vec<u8>)>,
    authorities: super::host::requests::Authorities,
    root_env: AtomicBool,
    namespace_coupled: AtomicBool,
}

impl Replay {
    pub(crate) fn new(tape: &[u8]) -> Result<Arc<Self>, Diverged> {
        let mut session = Session::replay(tape)?;
        session.start(&[]);
        Ok(Arc::new(Self {
            session: Mutex::new(Some(session)),
            published: AtomicBool::new(false),
            handles: Mutex::default(),
            output: Mutex::default(),
            authorities: Default::default(),
            root_env: AtomicBool::new(false),
            namespace_coupled: AtomicBool::new(false),
        }))
    }

    /// # Safety
    /// Admission only, before any task runs. This initial replay slice admits
    /// scalar atomics, administrative task-tree operations, pending Shared
    /// root tails, thaw outcomes, explicit task.canceled checks and deadline
    /// scopes whose children use these admitted operations, task-set selection, serial gates and
    /// scalar mutex operations including kill/cause and scalar channels with
    /// endpoint aliases/closes, and explicit bounded host requests whose
    /// results use admitted scalar/byte forms or creator-qualified Handle
    /// creations; general host dispatch and other handle shapes remain gated. Deterministic yielding object finalizers
    /// must use only these admitted operations and task-owned detached images.
    pub(crate) unsafe fn root(self: &Arc<Self>, ctx: *mut TaskCtx) {
        assert!(unsafe { (*ctx).recording.is_none() && (*ctx).run_ref().free.is_none() });
        if unsafe { (*ctx).run_ref().free_production } {
            let env = unsafe { (*ctx).run_ref().free_root_env };
            self.root_env.store(env, Ordering::Relaxed);
            self.namespace_coupled.store(
                unsafe {
                    (*ctx)
                        .run_ref()
                        .free_namespace_coupled
                        .load(Ordering::Relaxed)
                },
                Ordering::Relaxed,
            );
            self.authorities.root(env);
        }
        unsafe { (*ctx).run().free_replay = true };
        scheduler(ctx).logical = Logical::Free(super::Driver {
            replay: Some(self.clone()),
        });
    }

    pub(crate) fn finish(&self) -> Result<Projection, Diverged> {
        self.session
            .lock()
            .unwrap()
            .take()
            .expect("one replay finish")
            .finish()
    }

    fn with<T>(&self, action: impl FnOnce(&mut Session) -> T) -> T {
        let mut session = self.session.lock().unwrap();
        action(session.as_mut().expect("an unfinished replay"))
    }

    pub(crate) fn output(&self) -> (Vec<u8>, Vec<u8>) {
        self.output.lock().unwrap().clone()
    }
}

fn state(ctx: *mut TaskCtx) -> Option<Arc<Replay>> {
    if !on(ctx) {
        return None;
    }
    // Capture state before taking an independent cell access. Raw lookup
    // avoids a stop-aware accessor abandoning another caller-owned carrier;
    // the runtime owner observes stops after its temporary values drop.
    unsafe { role::enter(ctx) };
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    match &s.logical {
        Logical::Free(driver) => driver.replay.clone(),
        _ => None,
    }
}

fn path(ctx: *mut TaskCtx) -> Vec<u32> {
    // state() has acquired the role; no stop-aware accessor is used.
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref().unwrap() };
    s.tasks[unsafe { (*ctx).task_id } as usize]
        .as_ref()
        .unwrap()
        .path
        .clone()
}

pub(super) fn on(ctx: *mut TaskCtx) -> bool {
    // Admission fixes this selector before any task runs.
    unsafe { (*ctx).run_ref().free_replay }
}

pub(super) fn at_cut(ctx: *mut TaskCtx) -> bool {
    let Some(state) = state(ctx) else {
        return false;
    };
    let path = path(ctx);
    state.with(|session| session.at_cut(&path))
}

/// Before any user segment, a task whose cut is START stays saved forever.
pub(crate) fn start(ctx: *mut TaskCtx) {
    if at_cut(ctx) {
        park_final(ctx);
    }
}

/// Register in the parent's admitted spawn phase. False leaves the child
/// queued forever, including cuts that contain only its START registration.
pub(super) fn child(ctx: *mut TaskCtx, path: &[u32]) -> bool {
    let Some(state) = state(ctx) else { return true };
    state.with(|session| session.start(path))
}

/// An unadmitted continuation returns Requested only to unwind temporary
/// runtime carriers. At their outer check it becomes a permanent saved wait,
/// while a real external stop still ends the run normally.
pub(crate) fn cut_rejection(ctx: *mut TaskCtx, stop: &Stop) -> bool {
    matches!(stop, Stop::Requested)
        && on(ctx)
        && unsafe { (*ctx).run_ref().control.stopped().is_none() }
        && at_cut(ctx)
}

pub(super) fn begin(ctx: *mut TaskCtx) {
    let Some(state) = state(ctx) else { return };
    let path = path(ctx);
    state.with(|session| session.begin(&path));
}

/// A publication can enable replay waits. Drain the notification only under
/// scheduler ownership, outside independent cell accesses and session locks.
pub(super) fn wake(s: &mut Scheduler) {
    let Logical::Free(driver) = &s.logical else {
        return;
    };
    let Some(replay) = &driver.replay else { return };
    if !replay.published.swap(false, Ordering::AcqRel) {
        return;
    }
    for task in std::mem::take(&mut s.retrying) {
        if let Some(Some(row)) = s.tasks.get_mut(task as usize)
            && matches!(row.wait, Some(Wait::Retry { .. }))
        {
            row.retried = true;
            s.wake(task);
        }
    }
}

/// # Safety
/// Thaw's ordinal has begun, the role is held, root is live, out writable.
pub(super) unsafe fn thaw(
    ctx: *mut TaskCtx,
    root: *mut crate::Header,
    out: *mut u8,
) -> Stopped<()> {
    let mut entered = false;
    loop {
        // All owners of session state leave this scope before the save.
        let complete = {
            let token = token(ctx).expect("an admitted native replay");
            token.entry(ctx, Phase::Done)?;
            let last = unsafe { crate::roots::sole(root) };
            if token
                .shared
                .with(|session| session.thaw_ready(&token.path, last))
            {
                token.publish(ctx, Phase::Done, || {
                    let last = token
                        .shared
                        .with(|session| session.thaw(&token.path, last))
                        .map_err(|error| Stop::Diverged(error.0))?;
                    unsafe { out.write(u8::from(last)) };
                    Ok(())
                })??;
                true
            } else {
                if !entered {
                    token.publish(ctx, Phase::Entered, || ())?;
                    entered = true;
                }
                false
            }
        };
        if complete {
            return unsafe { super::try_yield(&mut *ctx) };
        }
        if at_cut(ctx) {
            return Err(Stop::Requested);
        }
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

/// An owner-local replay reservation captured under the scheduler role,
/// before taking any independent atomic/object access. No scheduler lookup
/// or role acquisition occurs while this token publishes or substitutes.
pub(super) struct Token {
    shared: Arc<Replay>,
    path: Vec<u32>,
}

pub(super) fn token(ctx: *mut TaskCtx) -> Option<Token> {
    let shared = state(ctx)?;
    Some(Token {
        shared,
        path: path(ctx),
    })
}

/// An object binding captured under the role before independent cell access.
pub(super) struct Object {
    token: Token,
    handle: hirsel_free::session::Handle,
}

pub(super) fn object(ctx: *mut TaskCtx, handle: hirsel_free::session::Handle) -> Option<Object> {
    token(ctx).map(|token| Object { token, handle })
}

/// Version waits own no cell, replay token or session lock across a save.
pub(super) unsafe fn wait_object(
    ctx: *mut TaskCtx,
    handle: hirsel_free::session::Handle,
) -> Stopped<()> {
    if !on(ctx) {
        return Ok(());
    }
    let mut entered = false;
    loop {
        {
            let object = object(ctx, handle).expect("an admitted object replay");
            if object.ready(ctx)? {
                return Ok(());
            }
            if !entered {
                object.token.publish(ctx, Phase::Entered, || ())?;
                entered = true;
            }
        }
        let s = scheduler(ctx);
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

impl Object {
    pub(super) fn token(&self) -> &Token {
        &self.token
    }

    pub(super) fn create(&self) -> Stopped<()> {
        self.token
            .shared
            .with(|session| session.create(&self.token.path, self.handle))
            .map_err(|error| Stop::Diverged(error.0))
    }

    fn ready(&self, ctx: *mut TaskCtx) -> Stopped<bool> {
        self.token.entry(ctx, Phase::Done)?;
        if !self
            .token
            .shared
            .with(|session| session.admits(&self.token.path, Phase::Done))
        {
            self.token.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        self.token
            .shared
            .with(|session| session.enabled(&self.token.path, 0, self.handle))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn touch(&self, outcome: &str) -> Stopped<()> {
        self.token
            .shared
            .with(|session| session.touch(&self.token.path, 0, self.handle, outcome))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn receive(&self, outcome: &str) -> Stopped<()> {
        self.token
            .shared
            .with(|session| session.receive(&self.token.path, self.handle, outcome))
            .map_err(|error| Stop::Diverged(error.0))
    }
}

impl Token {
    pub(super) fn admit_done(&self, ctx: *mut TaskCtx) -> Stopped<()> {
        self.entry(ctx, Phase::Done)?;
        if !self
            .shared
            .with(|session| session.admits(&self.path, Phase::Done))
        {
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        Ok(())
    }

    pub(super) fn cancel_peek(&self, live: Option<u32>) -> Option<u32> {
        self.shared
            .with(|session| session.cancel_peek(&self.path, live))
    }

    /// No action runs past its admitted phase. Rejection returns through its
    /// runtime owner so temporary cell accesses drop normally before stopping.
    pub(super) fn publish<T>(
        &self,
        ctx: *mut TaskCtx,
        phase: Phase,
        action: impl FnOnce() -> T,
    ) -> Stopped<T> {
        self.entry(ctx, phase)?;
        self.publish_phase(ctx, phase, action)
    }

    fn entry(&self, ctx: *mut TaskCtx, phase: Phase) -> Stopped<()> {
        // Immediate ops still have an entry when roots are pending. Admit
        // its deterministic nested tail even if this cut rejects Done, just
        // as record::publish_with_replay does on the recording path.
        if phase == Phase::Done && unsafe { (*ctx).roots.has_pending() } {
            self.publish_phase(ctx, Phase::Entered, || ())?;
        }
        Ok(())
    }

    fn publish_phase<T>(
        &self,
        ctx: *mut TaskCtx,
        phase: Phase,
        action: impl FnOnce() -> T,
    ) -> Stopped<T> {
        if !self
            .shared
            .with(|session| session.admits(&self.path, phase))
        {
            return Err(crate::stop::Stop::Requested);
        }
        let mut roots = crate::roots::PhaseRoots::prepare(ctx);
        while !roots.apply(ctx) {
            crate::roots::wait_phase(ctx);
        }
        let result = action();
        let publication = self
            .shared
            .with(|session| session.publish(&self.path, phase));
        roots.finish(ctx);
        if publication.is_ok() {
            self.shared.published.store(true, Ordering::Release);
        }
        publication
            .map(|()| result)
            .map_err(|error| crate::stop::Stop::Diverged(error.0))
    }

    pub(super) fn host_result(
        &self,
        request: &crate::io::Request,
        mut envelope: super::host::handles::Envelope,
    ) -> Stopped<Result<crate::io::Completion, crate::io::AdapterError>> {
        if super::host::handles::restore_refusal(&mut envelope, &self.shared.authorities)? {
            return Ok(envelope.result);
        }
        if envelope
            .created
            .iter()
            .any(|created| created.path != self.path)
        {
            return Err(Stop::Diverged(
                "host creation belongs to a different task".into(),
            ));
        }
        let result = self
            .shared
            .handles
            .lock()
            .unwrap()
            .substitute_for(request, envelope)?;
        let op = self.shared.with(|session| session.op(&self.path));
        self.shared.authorities.observe(&self.path, op, &result)?;
        Ok(result)
    }
    pub(super) fn host_request(&self, request: &crate::io::Request) -> Stopped<()> {
        let actual = self.shared.authorities.project(request)?;
        self.observation(&actual)
    }
    pub(super) fn observation(&self, bytes: &[u8]) -> Stopped<()> {
        self.shared
            .with(|session| session.replay_host_request(&self.path, bytes))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn host_keys(
        &self,
        request: &crate::io::Request,
    ) -> Stopped<crate::io_keys::KeySet> {
        let handles = self.shared.handles.lock().unwrap();
        let keys = super::host::native::request_keys_with_domains(
            request,
            |word| handles.identity(word),
            |word| handles.process_keys(word),
            |word| handles.net_ports(word),
            self.shared.root_env.load(Ordering::Relaxed),
        )?;
        Ok(super::host::native::namespace_policy(
            keys,
            self.shared.namespace_coupled.load(Ordering::Relaxed),
        ))
    }

    pub(super) fn host_identity(
        &self,
        word: u64,
    ) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.shared.handles.lock().unwrap().identity(word)
    }

    pub(super) fn host_resource(&self, word: u64) -> Stopped<crate::io_keys::Key> {
        self.shared.handles.lock().unwrap().resource(word)
    }

    pub(super) fn host_readiness(&self, ready: bool) -> Stopped<()> {
        self.shared
            .with(|session| session.replay_host_readiness(&self.path, ready))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn host_positions(
        &self,
        keys: &[(String, bool)],
        index: u32,
    ) -> Stopped<Vec<hirsel_free::recorder::Position>> {
        let keys: Vec<_> = keys
            .iter()
            .map(|(key, shared)| (key.as_str(), *shared))
            .collect();
        self.shared
            .with(|session| session.replay_host_positions(&self.path, index, &keys))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn host_ready(&self, positions: &[hirsel_free::recorder::Position]) -> bool {
        self.shared
            .with(|session| session.replay_host_ready(positions))
    }

    pub(super) fn host_observation(
        &self,
        positions: &[hirsel_free::recorder::Position],
    ) -> Stopped<Vec<u8>> {
        self.shared
            .with(|session| session.replay_host_observation(&self.path, positions))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn host_output(
        &self,
        positions: &[hirsel_free::recorder::Position],
        sink: &str,
        bytes: &[u8],
        stderr: bool,
    ) -> Stopped<()> {
        self.shared
            .with(|session| session.replay_host_output(&self.path, positions, sink, bytes))
            .map_err(|error| Stop::Diverged(error.0))?;
        let mut output = self.shared.output.lock().unwrap();
        if stderr {
            output.1.extend_from_slice(bytes);
        } else {
            output.0.extend_from_slice(bytes);
        }
        Ok(())
    }

    pub(super) fn host_completion(&self) -> Stopped<Vec<u8>> {
        self.shared
            .with(|session| session.replay_host_completion(&self.path))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn load(&self, live: u128) -> Stopped<u128> {
        self.shared
            .with(|session| session.load(&self.path, live))
            .map_err(|error| crate::stop::Stop::Diverged(error.0))
    }

    pub(super) fn cancel(&self, live: Option<u32>) -> Stopped<Option<u32>> {
        self.shared
            .with(|session| session.cancel_check(&self.path, live))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn expired(&self, live: bool) -> Stopped<bool> {
        self.shared
            .with(|session| session.expired(&self.path, live))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn await_member(&self, ctx: *mut TaskCtx) -> Stopped<u64> {
        if !self
            .shared
            .with(|session| session.admits(&self.path, Phase::Done))
        {
            // Entered-only cuts carry no selected member yet. Replay applies
            // entry (including its root tail) and parks before inspecting it.
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        self.shared
            .with(|session| session.await_any_member(&self.path))
            .ok_or_else(|| {
                Stop::Diverged("a nonempty native await_any has no recorded member".into())
            })
    }

    pub(super) fn await_any(&self, index: u64) -> Stopped<()> {
        let selected = self
            .shared
            .with(|session| {
                session.await_any(&self.path, Some(index), |candidate| candidate == index)
            })
            .map_err(|error| Stop::Diverged(error.0))?;
        assert_eq!(selected, Some(index), "the recorded member was ready");
        Ok(())
    }

    pub(super) fn gate_ready(&self, ctx: *mut TaskCtx, gate: u32) -> Stopped<bool> {
        self.entry(ctx, Phase::Done)?;
        if !self
            .shared
            .with(|session| session.admits(&self.path, Phase::Done))
        {
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        self.shared
            .with(|session| {
                session.gate((3, u64::from(gate)), &format!("package:{gate}"));
                session.enabled(&self.path, 0, (3, u64::from(gate)))
            })
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn gate_touch(&self, gate: u32, outcome: &str) -> Stopped<()> {
        self.shared
            .with(|session| session.touch(&self.path, 0, (3, u64::from(gate)), outcome))
            .map_err(|error| Stop::Diverged(error.0))
    }
}

#[cfg(test)]
mod tests;
