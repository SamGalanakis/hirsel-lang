//! Native replay admission over the backend-neutral decoder and projection.
//! Each task replays from its own cursor over its immutable stream; an ordinary
//! phase locks only that task's state and the object version cell it orders
//! against. Object handle lookup, key epochs and the terminal check are the
//! only shared administration. No cursor borrow or lock crosses a physical
//! save; atomic carriers are released before cut parking.
use super::super::*;
use hirsel_free::phase::Progress;
use hirsel_free::projection::Projection;
use hirsel_free::record::ObjectId;
use hirsel_free::replay::{Cursor, Diverged, Shared};
use hirsel_free::session::Handle;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

/// One task's replay state. Only its running owner locks it (and the terminal
/// check after every task stopped), so the lock is never contended.
pub(crate) struct TaskReplay {
    state: Mutex<TaskState>,
}

struct TaskState {
    cursor: Cursor,
    op: u64,
    progress: Progress,
    started: bool,
}

impl TaskState {
    fn at(&self, phase: Phase) -> Progress {
        Progress::new(self.op, phase)
    }
}

pub(crate) struct Replay {
    shared: Arc<Shared>,
    /// Every task the tape can start, built at admission and never changed.
    tasks: HashMap<Vec<u32>, Arc<TaskReplay>>,
    objects: RwLock<HashMap<Handle, ObjectId>>,
    finished: AtomicBool,
    published: AtomicBool,
    handles: Mutex<super::host::handles::Handles>,
    output: Mutex<(Vec<u8>, Vec<u8>)>,
    authorities: super::host::requests::Authorities,
    root_env: AtomicBool,
    namespace_coupled: AtomicBool,
}

impl Replay {
    pub(crate) fn new(tape: &[u8]) -> Result<Arc<Self>, Diverged> {
        let tape = hirsel_free::tape::parse(tape).map_err(Diverged)?;
        let shared = Shared::new(&tape)?;
        let mut paths: Vec<Vec<u32>> = shared.cut().keys().cloned().collect();
        paths.push(Vec::new());
        let tasks = paths
            .into_iter()
            .map(|path| {
                let cursor = shared.cursor(&path);
                let task = Arc::new(TaskReplay {
                    state: Mutex::new(TaskState {
                        cursor,
                        op: 0,
                        progress: Progress::START,
                        started: path.is_empty(),
                    }),
                });
                (path, task)
            })
            .collect();
        Ok(Arc::new(Self {
            shared,
            tasks,
            objects: RwLock::default(),
            finished: AtomicBool::new(false),
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
        role::with_role(ctx, |s| {
            s.logical = Logical::Free(super::Driver {
                replay: Some(self.clone()),
            })
        });
    }

    /// The terminal check: every started task consumed its stream and reached
    /// its cut. Called once, after every task stopped.
    pub(crate) fn finish(&self) -> Result<Projection, Diverged> {
        assert!(
            !self.finished.swap(true, Ordering::AcqRel),
            "one replay finish"
        );
        let mut paths: Vec<_> = self.tasks.keys().collect();
        paths.sort();
        let states: Vec<_> = paths
            .into_iter()
            .map(|path| self.tasks[path].state.lock().unwrap())
            .collect();
        self.shared.finish(
            states
                .iter()
                .filter(|state| state.started && state.cursor.starts())
                .map(|state| (&state.cursor, state.progress)),
        )
    }

    fn task(&self, path: &[u32]) -> &Arc<TaskReplay> {
        self.tasks
            .get(path)
            .unwrap_or_else(|| panic!("free-mode task {path:?} never starts in this replay"))
    }

    fn object(&self, handle: Handle) -> Result<ObjectId, Diverged> {
        self.objects
            .read()
            .unwrap()
            .get(&handle)
            .cloned()
            .ok_or_else(|| Diverged(format!("free-mode object {handle:?} was never created")))
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
    let Some(task) = state.tasks.get(&path) else {
        return true;
    };
    let task = task.state.lock().unwrap();
    !task.cursor.starts() || task.cursor.cut() == Some(task.progress)
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
    let Some(task) = state.tasks.get(path) else {
        return false;
    };
    let mut task = task.state.lock().unwrap();
    task.started = true;
    task.cursor.starts()
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
    let mut task = state.task(&path).state.lock().unwrap();
    task.op = task.op.checked_add(1).expect("native op ordinal overflow");
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
            if token.with(|task| task.cursor.thaw_outcome(task.op) != Some(true) || last) {
                token.publish(ctx, Phase::Done, || {
                    let last = token
                        .with(|task| {
                            let ok = task.cursor.thaw(task.op)?;
                            if ok && !last {
                                return Err(Diverged(format!(
                                    "{:?} op {}: a recorded Ok thaw of a root that is not the last",
                                    task.cursor.path(),
                                    task.op
                                )));
                            }
                            Ok(ok)
                        })
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
        retry_later(ctx);
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

/// An owner-local replay reservation captured under the scheduler role,
/// before taking any independent atomic/object access. No scheduler lookup
/// or role acquisition occurs while this token publishes or substitutes.
/// A task's token never changes, so its immediate ops keep one (objects.rs).
#[derive(Clone)]
pub(super) struct Token {
    shared: Arc<Replay>,
    task: Arc<TaskReplay>,
}

pub(super) fn token(ctx: *mut TaskCtx) -> Option<Token> {
    let shared = state(ctx)?;
    let task = shared.task(&path(ctx)).clone();
    Some(Token { shared, task })
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
        retry_later(ctx);
        unsafe { try_block(&mut *ctx, Wait::Retry { cancelable: false }, false, 0) }?;
    }
}

/// Queue the running task for the next replay publication.
pub(super) fn retry_later(ctx: *mut TaskCtx) {
    role::with_role(ctx, |s| {
        let me = s.current;
        if !s.retrying.contains(&me) {
            s.retrying.push(me);
        }
    });
}

impl Object {
    pub(super) fn token(&self) -> &Token {
        &self.token
    }

    pub(super) fn create(&self) -> Stopped<()> {
        self.token.with(|task| {
            let id = ObjectId::Made {
                path: task.cursor.path().to_vec(),
                op: task.op,
            };
            self.token
                .shared
                .objects
                .write()
                .unwrap()
                .insert(self.handle, id.clone());
            task.cursor.create(id);
        });
        Ok(())
    }

    fn ready(&self, ctx: *mut TaskCtx) -> Stopped<bool> {
        self.token.entry(ctx, Phase::Done)?;
        if !self.token.admits(Phase::Done) {
            self.token.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        let id = self.id()?;
        self.token
            .with(|task| task.cursor.enabled(task.op, 0, &id))
            .map_err(|error| Stop::Diverged(error.0))
    }

    fn id(&self) -> Stopped<ObjectId> {
        self.token
            .shared
            .object(self.handle)
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn touch(&self, outcome: &str) -> Stopped<()> {
        let id = self.id()?;
        self.token
            .with(|task| task.cursor.touch(task.op, 0, &id, outcome))
            .map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn receive(&self, outcome: &str) -> Stopped<()> {
        let id = self.id()?;
        self.token.with(|task| task.cursor.receive(&id, outcome));
        Ok(())
    }
}

impl Token {
    /// Lock this task's own replay state for one bounded step.
    fn with<T>(&self, action: impl FnOnce(&mut TaskState) -> T) -> T {
        action(&mut self.task.state.lock().unwrap())
    }

    /// The role-free begin's and boundary's cut check, as for [`at_cut`].
    pub(super) fn at_cut(&self) -> bool {
        self.with(|task| !task.cursor.starts() || task.cursor.cut() == Some(task.progress))
    }

    /// Whether this task's next op completes whole from here: the cut is not
    /// reached, admits its Done and, on an object, its version order enables
    /// it. Read-only, so a false leaves the op unbegun for the role path,
    /// which parks at the cut, waits for the version or reports divergence.
    pub(super) fn immediate(&self, handle: Option<Handle>) -> bool {
        let id = match handle.map(|handle| self.shared.object(handle)) {
            None => None,
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => return false,
        };
        self.with(|task| {
            let Some(op) = task.op.checked_add(1) else {
                return false;
            };
            task.cursor.starts()
                && task.cursor.cut() != Some(task.progress)
                && task.cursor.admits(Progress::new(op, Phase::Done))
                && id.is_none_or(|id| task.cursor.enabled(op, 0, &id).unwrap_or(false))
        })
    }

    /// The role-free [`begin`].
    pub(super) fn begin(&self) {
        self.with(|task| task.op = task.op.checked_add(1).expect("native op ordinal overflow"));
    }

    pub(super) fn bind(&self, handle: Handle) -> Object {
        Object {
            token: self.clone(),
            handle,
        }
    }

    fn admits(&self, phase: Phase) -> bool {
        self.with(|task| task.cursor.admits(task.at(phase)))
    }

    fn diverged<T>(
        &self,
        action: impl FnOnce(&mut TaskState) -> Result<T, Diverged>,
    ) -> Stopped<T> {
        self.with(action).map_err(|error| Stop::Diverged(error.0))
    }

    pub(super) fn admit_done(&self, ctx: *mut TaskCtx) -> Stopped<()> {
        self.entry(ctx, Phase::Done)?;
        if !self.admits(Phase::Done) {
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        Ok(())
    }

    pub(super) fn cancel_peek(&self, _live: Option<u32>) -> Option<u32> {
        self.with(|task| task.cursor.cancel_outcome(task.op))
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
        if !self.admits(phase) {
            return Err(crate::stop::Stop::Requested);
        }
        let mut roots = crate::roots::PhaseRoots::prepare(ctx);
        while !roots.apply(ctx) {
            crate::roots::wait_phase(ctx);
        }
        let result = action();
        let publication = self.with(|task| {
            let progress = task.at(phase);
            task.cursor.publish(progress)?;
            task.progress = progress;
            Ok::<_, Diverged>(())
        });
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
        envelope: super::host::handles::Envelope,
    ) -> Stopped<Result<crate::io::Completion, crate::io::AdapterError>> {
        let (path, op) = self.with(|task| (task.cursor.path().to_vec(), task.op));
        super::host::engine::restore(
            &path,
            op,
            request,
            envelope,
            &mut self.shared.handles.lock().unwrap(),
            &self.shared.authorities,
        )
    }

    pub(super) fn host_request(&self, request: &crate::io::Request) -> Stopped<()> {
        let actual = self.shared.authorities.project(request)?;
        self.observation(&actual)
    }
    pub(super) fn observation(&self, bytes: &[u8]) -> Stopped<()> {
        self.diverged(|task| {
            if task.cursor.observation(task.op)? != bytes {
                return Err(Diverged(format!(
                    "host request differs at {:?} operation {}",
                    task.cursor.path(),
                    task.op
                )));
            }
            Ok(())
        })
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
        self.diverged(|task| {
            if ready {
                task.cursor.ready(task.op)
            } else {
                task.cursor.blocked(task.op)
            }
        })
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
        self.diverged(|task| {
            if index == 0 {
                task.cursor.prepare(task.op)?;
            }
            task.cursor.positions(task.op, index, &keys)
        })
    }

    pub(super) fn host_ready(&self, positions: &[hirsel_free::recorder::Position]) -> bool {
        self.shared.shared.positions_ready(positions)
    }

    pub(super) fn host_observation(
        &self,
        positions: &[hirsel_free::recorder::Position],
    ) -> Stopped<Vec<u8>> {
        let bytes = self.diverged(|task| task.cursor.observation(task.op))?;
        self.shared.shared.end_positions(positions);
        Ok(bytes)
    }

    pub(super) fn host_output(
        &self,
        positions: &[hirsel_free::recorder::Position],
        sink: &str,
        bytes: &[u8],
        stderr: bool,
    ) -> Stopped<()> {
        self.diverged(|task| task.cursor.output_at_position(positions, sink, bytes))?;
        let mut output = self.shared.output.lock().unwrap();
        if stderr {
            output.1.extend_from_slice(bytes);
        } else {
            output.0.extend_from_slice(bytes);
        }
        Ok(())
    }

    pub(super) fn host_completion(&self) -> Stopped<Vec<u8>> {
        self.diverged(|task| task.cursor.completion(task.op))
    }

    pub(super) fn load(&self, _live: u128) -> Stopped<u128> {
        self.diverged(|task| task.cursor.load(task.op))
    }

    pub(super) fn cancel(&self, _live: Option<u32>) -> Stopped<Option<u32>> {
        Ok(self.with(|task| task.cursor.cancel_check(task.op)))
    }

    pub(super) fn expired(&self, _live: bool) -> Stopped<bool> {
        Ok(self.with(|task| task.cursor.expired(task.op)))
    }

    pub(super) fn await_member(&self, ctx: *mut TaskCtx) -> Stopped<u64> {
        if !self.admits(Phase::Done) {
            // Entered-only cuts carry no selected member yet. Replay applies
            // entry (including its root tail) and parks before inspecting it.
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        self.with(|task| task.cursor.await_any_member(task.op))
            .ok_or_else(|| {
                Stop::Diverged("a nonempty native await_any has no recorded member".into())
            })
    }

    pub(super) fn await_any(&self, index: u64) -> Stopped<()> {
        let selected = self.diverged(|task| task.cursor.await_any(task.op))?;
        assert_eq!(selected, index, "the recorded member was ready");
        Ok(())
    }

    pub(super) fn gate_ready(&self, ctx: *mut TaskCtx, gate: u32) -> Stopped<bool> {
        self.entry(ctx, Phase::Done)?;
        if !self.admits(Phase::Done) {
            self.publish(ctx, Phase::Entered, || ())?;
            return Err(Stop::Requested);
        }
        let id = self.gate_id(gate);
        self.diverged(|task| task.cursor.enabled(task.op, 0, &id))
    }

    /// A serial gate exists from admission at version 1, under its package identity.
    fn gate_id(&self, gate: u32) -> ObjectId {
        let package = format!("package:{gate}");
        self.shared.shared.gate(&package);
        ObjectId::Gate(package)
    }

    pub(super) fn gate_touch(&self, gate: u32, outcome: &str) -> Stopped<()> {
        let id = self.gate_id(gate);
        self.diverged(|task| task.cursor.touch(task.op, 0, &id, outcome))
    }
}

#[cfg(test)]
mod tests;
