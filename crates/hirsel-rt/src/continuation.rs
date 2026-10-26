//! One ownership interface for native continuations (r6 S2/S7).
//!
//! A continuation is `Fresh` until its body first executes; after that it is
//! exclusively `Active` or `Saved` until `Complete`. Only a `Fresh`
//! continuation may enter its body's entry, so no representation can restart
//! executed work. Saving records the exact resume point: a stackful fiber's
//! stack pointer or a compact frame's resume state and live image.
//!
//! Private ahead execution saves through the same record: adoption takes the
//! exact saved PC, the custody generation and the private fuel once, after the
//! physical owner acknowledged its save (Rayon join's stolen-job lifetime,
//! `rayon-core/src/join/mod.rs:132-185`: the owner never resumes storage it
//! already handed off).
//!
//! A resumable task body runs through [`hirsel_rt_v2_resume_run`]. It saves
//! only at a boundary where the task would wait (see
//! [`hirsel_rt_v2_resume_point`]). Under the
//! shared physical dispatcher a saved compact frame has exactly one owner, the
//! dispatcher, and no stack: its task's stack goes back to its worker after
//! the unwind, and any worker activates it on a stack of its own (Rayon join's
//! stolen job, `rayon-core/src/join/mod.rs:132-165`: whoever pops the job
//! runs it once, wherever). Stackful fibers keep their own switch until the
//! dispatcher's saved-fiber cutover (S2), so only tests save a `Fiber` here.
#![cfg_attr(not(test), allow(dead_code))]

use crate::{RESUME_EARLY, RESUME_LEAVES, RESUME_LOGGED};
pub(crate) mod binding;

/// The resume point of a stackful frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SavedFiber {
    pub(crate) sp: *mut u8,
}

/// A 16-byte frame unit: nested frames and frame storage sit at 16-byte offsets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C, align(16))]
pub(crate) struct FrameWord([u64; 2]);

/// The owned compact frame of a validated continuation plan. Compiled state
/// alternates materialize every live value into it before a suspension; its
/// 8-byte header (`hirsel_lir::continuation::FRAME_HEADER_BYTES`) holds the
/// state to resume, and state 0 is the body's entry.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct OwnedResumeFrame {
    image: Box<[FrameWord]>,
}

impl OwnedResumeFrame {
    pub(crate) fn new(bytes: u32) -> Self {
        Self {
            image: vec![FrameWord::default(); (bytes as usize).div_ceil(16).max(1)]
                .into_boxed_slice(),
        }
    }

    pub(crate) fn state(&self) -> u32 {
        self.image[0].0[0] as u32
    }

    pub(crate) fn bytes(&self) -> usize {
        self.image.len() * 16
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut u8 {
        self.image.as_mut_ptr().cast()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SavedRepr {
    Fiber(SavedFiber),
    Resumable(OwnedResumeFrame),
}

/// The linear Saved/Active protocol shared by every representation.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) enum Continuation {
    #[default]
    Fresh,
    Active,
    Saved(SavedRepr),
    Complete,
}

impl Continuation {
    /// Enter the body's entry: legal exactly once, before any execution.
    pub(crate) fn start(&mut self) {
        assert!(
            matches!(self, Self::Fresh),
            "continuation restart after execution"
        );
        *self = Self::Active;
    }

    pub(crate) fn save(&mut self, repr: SavedRepr) {
        assert!(matches!(self, Self::Active), "save without an Active owner");
        *self = Self::Saved(repr);
    }

    /// Take the exclusive saved image; only a Saved owner can be activated.
    pub(crate) fn activate(&mut self) -> SavedRepr {
        match std::mem::replace(self, Self::Active) {
            Self::Saved(repr) => repr,
            other => {
                *self = other;
                panic!("activate without an exclusive Saved owner")
            }
        }
    }

    pub(crate) fn complete(&mut self) {
        assert!(
            matches!(self, Self::Active),
            "complete without an Active owner"
        );
        *self = Self::Complete;
    }
}

/// Where an adopted private segment resumes: a stackful fiber's exact stack
/// pointer, or a compact frame's resume state, which its frame header names.
/// A compact save is never at the entry: adoption must not restart work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SavedPc {
    Fiber(SavedFiber),
    Compact { state: u32 },
}

impl SavedPc {
    /// The resume state of a compact frame saved at a boundary.
    pub(crate) fn compact(frame: &OwnedResumeFrame) -> Self {
        let state = frame.state();
        assert_ne!(state, 0, "a compact private save at its entry");
        Self::Compact { state }
    }

    /// The saved stack pointer of a stackful segment.
    pub(crate) fn sp(&self) -> Option<*mut u8> {
        match *self {
            Self::Fiber(fiber) => Some(fiber.sp),
            Self::Compact { .. } => None,
        }
    }
}

/// What adoption of a private segment took over, exactly once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrivateSave {
    pub(crate) pc: SavedPc,
    pub(crate) generation: u32,
    pub(crate) fuel: u64,
    /// The selected original result descriptor of a host-return segment.
    pub(crate) descriptor: Option<u32>,
}

/// Custody of a privately started body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Custody {
    /// Running or saved before its spawn's admission.
    Private {
        generation: u32,
    },
    /// Logical from the start (a certified host-return continuation).
    Logical,
    Adopted(PrivateSave),
    /// Withdrawn at quiescent teardown; its saved image is never resumed.
    Withdrawn,
}

impl Custody {
    fn generation(&self, generation: u32, what: &str) {
        if let Self::Private { generation: g } = *self {
            assert_eq!(generation, g, "stale ahead {what} generation");
        }
    }

    /// Adopt the saved private segment once, with its exact PC and fuel.
    pub(crate) fn adopt(&mut self, save: PrivateSave) -> PrivateSave {
        self.generation(save.generation, "custody");
        assert!(
            matches!(self, Self::Private { .. }),
            "ahead custody adopted twice"
        );
        *self = Self::Adopted(save);
        save
    }

    /// Whether this withdrawal is the first: the caller then restores the
    /// original ownership image. Adopted or logical custody needs nothing.
    pub(crate) fn withdraw(&mut self, generation: u32) -> bool {
        self.generation(generation, "discard");
        let first = matches!(self, Self::Private { .. });
        if first {
            *self = Self::Withdrawn;
        }
        first
    }

    pub(crate) fn private(&self) -> bool {
        matches!(self, Self::Private { .. })
    }
}

/// A resumable frame reached a boundary with its state materialized. It
/// saves exactly when the task would wait here before issuing the boundary's
/// operation, or is published to another worker (forced migration); otherwise
/// the operation proceeds in place on this stack. `state` carries
/// [`RESUME_LOGGED`] / [`RESUME_EARLY`] when a non-head is logged for the
/// operation or may commit it early, and [`RESUME_LEAVES`] when the
/// operation's first op may leave its stack in its wait. A private ahead segment proceeds in
/// place: its handoff stays stackful. The decision is
/// physical: it adds no operation, charge or observation (Tokio's harness goes
/// idle only on `Pending`, `runtime/task/harness.rs:193-231`; Go parks only at
/// `gopark`, `runtime/proc.go:457-492`).
///
/// # Safety
/// `ctx` is the running task's context and `frame` its live resumable frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_point(
    ctx: *mut crate::TaskCtx,
    frame: *mut u8,
    state: u32,
) -> u8 {
    let hints = RESUME_LOGGED | RESUME_EARLY | RESUME_LEAVES;
    if unsafe { frame.cast::<u32>().read() } != state & !hints {
        crate::fault("a resume point before its state was recorded");
    }
    let ahead = crate::tasks::compact::Ahead {
        logged: state & RESUME_LOGGED != 0,
        early: state & RESUME_EARLY != 0,
    };
    // SAFETY: compiled code calls this at a boundary of the running task.
    unsafe { (*ctx).point_ops = crate::tasks::compact::point(state & RESUME_LEAVES != 0) };
    let saves = !crate::tasks::ordered::ahead::private(ctx)
        && unsafe { crate::tasks::compact::parks(ctx, ahead) };
    #[cfg(feature = "diagnostic")]
    crate::compact_trace::point(unsafe { (*ctx).task_id }, state & !hints, saves);
    u8::from(saves)
}

/// A resumable body's dispatcher: its task entry's arguments and its frame.
pub type ResumeDispatcher =
    unsafe extern "C" fn(*mut crate::TaskCtx, *mut crate::Header, u64, u64, *mut u8) -> u8;

/// The task entry's arguments a resumable body's dispatcher continues with.
#[derive(Clone, Copy)]
struct Body {
    dispatcher: ResumeDispatcher,
    environment: *mut crate::Header,
    caller_site: u64,
    operation_site: u64,
}

/// A compact frame saved off every stack: its exclusive continuation, which
/// the shared dispatcher owns until one activation takes it, and its body.
pub(crate) struct Compact {
    owner: Continuation,
    body: Body,
}

impl Compact {
    /// The state the saved frame resumes at.
    #[cfg(feature = "diagnostic")]
    pub(crate) fn state(&self) -> u32 {
        match &self.owner {
            Continuation::Saved(SavedRepr::Resumable(frame)) => frame.state(),
            _ => crate::fault("a compact frame not saved"),
        }
    }
}

/// A resumable task body. Its dispatcher starts at state 0 of a fresh owned
/// frame; each `STATUS_SUSPENDED` unwinds the compiled stack to this owner,
/// which saves the frame (its header names the next state).
///
/// # Safety
/// As for the task entry it replaces; `dispatcher` is the body's compiled
/// dispatcher and `bytes` its plan's frame size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_run(
    ctx: *mut crate::TaskCtx,
    environment: *mut crate::Header,
    caller_site: u64,
    operation_site: u64,
    dispatcher: ResumeDispatcher,
    bytes: u32,
) -> u8 {
    // SAFETY: this physical entry owns its task between compiled segments.
    let _role = unsafe { crate::tasks::role::Guard::new(ctx) };
    let mut owner = Continuation::Fresh;
    owner.start();
    let body = Body {
        dispatcher,
        environment,
        caller_site,
        operation_site,
    };
    // SAFETY: caller contract; the task entry called this body.
    unsafe { drive(ctx, body, owner, OwnedResumeFrame::new(bytes)) }
}

/// Continue the compact frame the shared dispatcher activated on a stack it
/// lent this task: its exact saved state, never its entry. Returns the body's
/// status, environment and site for the task entry's suffix.
///
/// # Safety
/// The task's compact entry, holding the role, on its lent stack.
pub(crate) unsafe fn resume_compact(ctx: *mut crate::TaskCtx) -> (u8, *mut crate::Header, u64) {
    let Some(compact) = (unsafe { (*ctx).compact.take() }) else {
        crate::fault("a compact entry without its saved frame");
    };
    let Compact { mut owner, body } = *compact;
    let SavedRepr::Resumable(frame) = owner.activate() else {
        crate::fault("a compact entry activated a stackful frame");
    };
    // SAFETY: caller contract.
    let status = unsafe { drive(ctx, body, owner, frame) };
    (status, body.environment, body.operation_site)
}

/// Continue a resumable ahead body's frame, saved at a boundary by its ahead
/// code, in its ordinary `dispatcher`: the same save that `drive` performs,
/// then the same loop. The body never re-enters its entry.
///
/// # Safety
/// As for `drive`; the frame holds the state its ahead code recorded.
pub(crate) unsafe fn continue_saved(
    ctx: *mut crate::TaskCtx,
    dispatcher: ResumeDispatcher,
    environment: *mut crate::Header,
    caller_site: u64,
    operation_site: u64,
    frame: OwnedResumeFrame,
) -> u8 {
    let mut owner = Continuation::Fresh;
    owner.start();
    let body = Body {
        dispatcher,
        environment,
        caller_site,
        operation_site,
    };
    // SAFETY: caller contract.
    let frame = unsafe { park(ctx, body, &mut owner, frame) };
    unsafe { drive(ctx, body, owner, frame) }
}

/// Save the suspended frame. The shared dispatcher takes it off this stack
/// when the task can leave it, else it activates again here.
///
/// # Safety
/// As for `drive`, at the suspension.
unsafe fn park(
    ctx: *mut crate::TaskCtx,
    body: Body,
    owner: &mut Continuation,
    frame: OwnedResumeFrame,
) -> OwnedResumeFrame {
    if frame.state() == 0 {
        crate::fault("a resumable frame suspended at its entry");
    }
    owner.save(SavedRepr::Resumable(frame));
    // SAFETY: caller contract; at a resume point no runtime borrow, wait
    // or address into this stack is live, and its frames own no Drop value.
    if unsafe { crate::tasks::compact::leave(ctx) } {
        let owner = std::mem::take(owner);
        unsafe { crate::pool::free::leave_compact(ctx, Box::new(Compact { owner, body })) }
    }
    let SavedRepr::Resumable(saved) = owner.activate() else {
        crate::fault("a resumable frame changed representation while saved");
    };
    saved
}

/// Run `body`'s dispatcher over the Active owner's frame until it completes.
/// Each suspension saves the frame. The shared dispatcher then takes the
/// saved frame off this stack when the task can leave it (the stack goes back
/// to its worker), else the frame activates again here. Either way the
/// boundary it resumes at is the one suspension: a save is physical, adding no
/// operation or scheduling point.
///
/// # Safety
/// `ctx`'s task entry called this body, which holds the role at depth one.
unsafe fn drive(
    ctx: *mut crate::TaskCtx,
    body: Body,
    mut owner: Continuation,
    mut frame: OwnedResumeFrame,
) -> u8 {
    loop {
        crate::tasks::compact::bind_frame(ctx, Some((frame.as_mut_ptr() as usize, frame.bytes())));
        let status = {
            // SAFETY: no scheduler borrow survives the call into compiled code.
            let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
            // SAFETY: caller contract; the frame is exclusively this body's.
            unsafe {
                (body.dispatcher)(
                    ctx,
                    body.environment,
                    body.caller_site,
                    body.operation_site,
                    frame.as_mut_ptr(),
                )
            }
        };
        crate::tasks::compact::bind_frame(ctx, None);
        if status != crate::STATUS_SUSPENDED {
            owner.complete();
            return status;
        }
        // SAFETY: caller contract.
        frame = unsafe { park(ctx, body, &mut owner, frame) };
    }
}

#[cfg(test)]
mod tests;
