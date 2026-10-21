//! Native replay admission, shared with the F0 Session decoder and projection.
//! A mutex serializes session mutations. No session borrow or lock
//! crosses a physical save; atomic carriers are released before cut parking.
use super::super::*;
#[cfg(test)]
use hirsel_free::projection::Projection;
use hirsel_free::replay::Diverged;
use hirsel_free::session::Session;
use std::sync::{Arc, Mutex};

pub(crate) struct Replay {
    session: Mutex<Option<Session>>,
}

impl Replay {
    #[cfg(test)]
    pub(crate) fn new(tape: &[u8]) -> Result<Arc<Self>, Diverged> {
        let mut session = Session::replay(tape)?;
        session.start(&[]);
        Ok(Arc::new(Self {
            session: Mutex::new(Some(session)),
        }))
    }

    /// # Safety
    /// Admission only, before any task runs. This initial replay slice admits
    /// root scalar atomic operations only; no host/object/task-tree effects.
    #[cfg(test)]
    pub(crate) unsafe fn root(self: &Arc<Self>, ctx: *mut TaskCtx) {
        assert!(unsafe { (*ctx).recording.is_none() && (*ctx).run_ref().free.is_none() });
        unsafe { (*ctx).run().free_replay = true };
        scheduler(ctx).logical = Logical::Free(super::Driver {
            replay: Some(self.clone()),
        });
    }

    #[cfg(test)]
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

pub(super) fn begin(ctx: *mut TaskCtx) {
    let Some(state) = state(ctx) else { return };
    let path = path(ctx);
    state.with(|session| session.begin(&path));
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

impl Token {
    /// No action runs past its admitted phase. Rejection returns through its
    /// runtime owner so temporary cell accesses drop normally before stopping.
    pub(super) fn publish<T>(
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
        publication
            .map(|()| result)
            .map_err(|error| crate::stop::Stop::Diverged(error.0))
    }

    pub(super) fn load(&self, live: u128) -> Stopped<u128> {
        self.shared
            .with(|session| session.load(&self.path, live))
            .map_err(|error| crate::stop::Stop::Diverged(error.0))
    }
}

#[cfg(test)]
mod tests;
