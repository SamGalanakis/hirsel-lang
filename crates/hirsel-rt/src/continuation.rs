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
//! A resumable task body runs through [`hirsel_rt_v2_resume_run`]; stackful
//! fibers keep their own switch until the dispatcher's saved-fiber cutover
//! (S2), so only tests save a `Fiber` here.
#![cfg_attr(not(test), allow(dead_code))]

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

/// What adoption of a private segment took over, exactly once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrivateSave {
    pub(crate) pc: SavedFiber,
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

/// Whether resumable frames save at every resume point (the published form),
/// under `HIRSEL_RT_RESUME_SAVE` or `HIRSEL_RT_MIGRATE`; otherwise each
/// boundary proceeds in place (the local form).
fn saving() -> bool {
    static SAVING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SAVING.get_or_init(|| {
        std::env::var_os("HIRSEL_RT_RESUME_SAVE").is_some()
            || std::env::var_os("HIRSEL_RT_MIGRATE").is_some()
    })
}

/// A resumable frame reached a boundary with its state materialized. The
/// decision is physical: it adds no operation, charge or observation.
///
/// # Safety
/// `frame` is the running task's live resumable frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_resume_point(
    _ctx: *mut crate::TaskCtx,
    frame: *mut u8,
    state: u32,
) -> u8 {
    if unsafe { frame.cast::<u32>().read() } != state {
        crate::fault("a resume point before its state was recorded");
    }
    u8::from(saving())
}

/// A resumable body's dispatcher: its task entry's arguments and its frame.
pub type ResumeDispatcher =
    unsafe extern "C" fn(*mut crate::TaskCtx, *mut crate::Header, u64, u64, *mut u8) -> u8;

/// A resumable task body. Its dispatcher starts at state 0 of a fresh owned
/// frame; each `STATUS_SUSPENDED` saves the frame, whose header names the next
/// state, and yields, so the task may resume on another worker. Activation
/// takes the saved frame back and the dispatcher continues at that state.
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
    let mut frame = OwnedResumeFrame::new(bytes);
    let mut owner = Continuation::Fresh;
    owner.start();
    loop {
        // SAFETY: caller contract; the frame is exclusively this body's.
        let status =
            unsafe { dispatcher(ctx, environment, caller_site, operation_site, frame.as_mut_ptr()) };
        if status != crate::STATUS_SUSPENDED {
            owner.complete();
            return status;
        }
        if frame.state() == 0 {
            crate::fault("a resumable frame suspended at its entry");
        }
        owner.save(SavedRepr::Resumable(frame));
        // SAFETY: the context is live and no borrow of it is held.
        unsafe { crate::tasks::yield_now(&mut *ctx) };
        let SavedRepr::Resumable(saved) = owner.activate() else {
            crate::fault("a resumable frame changed representation while saved");
        };
        frame = saved;
    }
}

#[cfg(test)]
mod tests;
