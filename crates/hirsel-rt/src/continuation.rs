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
//! Compact frames and the Saved/Active state are wired by the dispatcher's
//! saved-continuation cutover (S2); until then only tests construct them.
#![cfg_attr(not(test), allow(dead_code))]

/// The resume point of a stackful frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SavedFiber {
    pub(crate) sp: *mut u8,
}

/// A compact frame saved at a validated resume state. The image starts with
/// the plan's 8-byte header (`hirsel_lir::continuation::FRAME_HEADER_BYTES`).
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct OwnedResumeFrame {
    pub(crate) plan: [u8; 32],
    pub(crate) state: u32,
    pub(crate) image: Box<[u64]>,
}

/// The runtime half of a validated plan: each state's image bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResumeShape {
    pub(crate) plan: [u8; 32],
    pub(crate) bytes: Vec<u32>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SavedRepr {
    Fiber(SavedFiber),
    Resumable(OwnedResumeFrame),
}

impl OwnedResumeFrame {
    /// Materialize every live word before a suspension, wait or cut.
    pub(crate) fn save(shape: &ResumeShape, state: u32, live: &[u64]) -> Option<Self> {
        let bytes = *shape.bytes.get(state.checked_sub(1)? as usize)?;
        if live.len() * 8 + 8 != bytes as usize {
            return None;
        }
        let mut image = Vec::with_capacity(live.len() + 1);
        image.push(u64::from(state));
        image.extend_from_slice(live);
        Some(Self {
            plan: shape.plan,
            state,
            image: image.into_boxed_slice(),
        })
    }

    /// Admit an imported frame only against the shape of its own plan.
    pub(crate) fn check(&self, shape: &ResumeShape) -> bool {
        self.plan == shape.plan
            && self.image.first() == Some(&u64::from(self.state))
            && self
                .state
                .checked_sub(1)
                .and_then(|i| shape.bytes.get(i as usize))
                .is_some_and(|b| *b as usize == self.image.len() * 8)
    }
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

#[cfg(test)]
mod tests;
