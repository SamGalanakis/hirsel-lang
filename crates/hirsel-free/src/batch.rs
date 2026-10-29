//! Immutable CPU claim identities over the production saved-frame owner.
//!
//! The lifecycle owner constructs the whole directory before dispatch. A slot's
//! ordinal names storage only. Its caller must derive retirement from the real
//! cooperative head, including holding rests and polls. Retained notifications
//! own the old allocation and never look up a replacement by ordinal.
use crate::frame::{self, Notification, Queued, Running};
use crate::sync::{Arc, AtomicBool, AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Key {
    pub slot: u32,
    pub generation: u32,
}

impl Key {
    /// Exhaustion retires this storage position permanently; it never wraps.
    pub fn successor(self) -> Option<Self> {
        Some(Self {
            generation: self.generation.checked_add(1)?,
            ..self
        })
    }
}

/// `lease` retains the runtime task generation and its physical storage. It
/// must cover every pointer in `image`; a key alone grants no pointer access.
pub struct Binding<L, T> {
    pub image: T,
    pub key: Key,
    pub path: Box<[u32]>,
    pub lease: L,
}

/// Canonical binding shared by a batch slot and its physical owner. The
/// lifecycle owner supplies the lease before any work is published.
pub struct Identity<L> {
    active: AtomicBool,
    key: Key,
    ordinal: usize,
    path: Box<[u32]>,
    lease: L,
}

impl<L> Identity<L> {
    pub fn bind(key: Key, ordinal: usize, path: Box<[u32]>, lease: L) -> Self {
        Self {
            active: AtomicBool::new(true),
            key,
            ordinal,
            path,
            lease,
        }
    }
    pub fn key(&self) -> Key {
        self.key
    }
    pub fn ordinal(&self) -> usize {
        self.ordinal
    }
    pub fn path(&self) -> &[u32] {
        &self.path
    }
    pub fn lease(&self) -> &L {
        &self.lease
    }
    /// Physical owners may retain this allocation after logical retirement.
    pub fn retire(&self) {
        self.active.store(false, Ordering::Release);
    }
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
}

struct Claim<L, T> {
    notification: Notification<T>,
    identity: Arc<Identity<L>>,
}

/// Complete, immutable membership. A new batch allocates a new directory.
pub struct Directory<L, T> {
    claims: Box<[Arc<Claim<L, T>>]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    DuplicateKey,
    DuplicatePath,
}

/// A generation-qualified retained wake, with no access to its frame image.
pub struct Notice<L, T>(Arc<Claim<L, T>>);

impl<L, T> Clone for Notice<L, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// Exclusive saved ownership retains both the physical frame and its claim.
pub struct Saved<L, T> {
    queued: Queued<T>,
    claim: Arc<Claim<L, T>>,
}

/// Private work owns the exact frame until idle acknowledges its save.
pub struct Active<L, T> {
    running: Running<T>,
    claim: Arc<Claim<L, T>>,
}

impl<L, T> Directory<L, T> {
    pub fn admit(bindings: Vec<Binding<L, T>>) -> Result<(Self, Vec<Saved<L, T>>), AdmissionError> {
        let mut keys = std::collections::BTreeSet::new();
        let mut paths = std::collections::BTreeSet::new();
        for binding in &bindings {
            if !keys.insert(binding.key) {
                return Err(AdmissionError::DuplicateKey);
            }
            if !paths.insert(&binding.path) {
                return Err(AdmissionError::DuplicatePath);
            }
        }
        drop(paths);
        let mut claims = Vec::with_capacity(bindings.len());
        let mut saved = Vec::with_capacity(bindings.len());
        for (ordinal, binding) in bindings.into_iter().enumerate() {
            let (notification, queued) = frame::prepared(binding.image);
            let claim = Arc::new(Claim {
                notification,
                identity: Arc::new(Identity::bind(
                    binding.key,
                    ordinal,
                    binding.path,
                    binding.lease,
                )),
            });
            claims.push(claim.clone());
            saved.push(Saved { claim, queued });
        }
        Ok((
            Self {
                claims: claims.into_boxed_slice(),
            },
            saved,
        ))
    }

    pub fn notice(&self, ordinal: usize, key: Key) -> Option<Notice<L, T>> {
        let claim = self.claims.get(ordinal)?;
        (claim.identity.key == key).then(|| Notice(claim.clone()))
    }

    pub fn len(&self) -> usize {
        self.claims.len()
    }
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }
}

impl<L, T> Notice<L, T> {
    pub fn key(&self) -> Key {
        self.0.identity.key
    }
    pub fn ordinal(&self) -> usize {
        self.0.identity.ordinal
    }
    pub fn path(&self) -> &[u32] {
        &self.0.identity.path
    }

    /// A running wake latches in the frame. Only idle's later save can publish
    /// another owner, and a completed frame can never be reactivated.
    pub fn notify(&self) -> Option<Saved<L, T>> {
        self.0.notification.notify().map(|queued| Saved {
            claim: self.0.clone(),
            queued,
        })
    }
}

impl<L, T> Saved<L, T> {
    pub fn start(self) -> Active<L, T> {
        Active {
            claim: self.claim,
            running: self.queued.start(),
        }
    }
}

impl<L, T> Active<L, T> {
    pub fn key(&self) -> Key {
        self.claim.identity.key
    }
    pub fn ordinal(&self) -> usize {
        self.claim.identity.ordinal
    }
    pub fn path(&self) -> &[u32] {
        &self.claim.identity.path
    }
    pub fn image(&self) -> &T {
        self.running.value()
    }
    pub fn image_mut(&mut self) -> &mut T {
        self.running.value_mut()
    }

    /// Called by idle after the exact stack and all initialized roots are
    /// saved. This ports Running::save, without a second publication word.
    pub fn save(self, yielded: bool) -> Option<Saved<L, T>> {
        let Self { claim, running } = self;
        running.save(yielded).map(|queued| Saved { claim, queued })
    }
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;

/// Cold startup has one aggregate outcome. Count every required destination
/// before publishing any worker, so an early acknowledgement cannot look final.
pub struct Startup {
    pending: AtomicU32,
    failed: AtomicBool,
}
impl Startup {
    pub fn new(count: u32) -> Self {
        Self {
            pending: AtomicU32::new(count),
            failed: AtomicBool::new(false),
        }
    }
    /// Returns true only for the last acknowledgement, which wakes admission.
    pub fn acknowledge(&self, ready: bool) -> bool {
        if !ready {
            self.failed.store(true, Ordering::Relaxed);
        }
        let previous = self.pending.fetch_sub(1, Ordering::AcqRel);
        assert_ne!(previous, 0, "one startup acknowledgement per worker");
        previous == 1
    }
    pub fn outcome(&self) -> Option<bool> {
        (self.pending.load(Ordering::Acquire) == 0).then(|| !self.failed.load(Ordering::Relaxed))
    }
}
