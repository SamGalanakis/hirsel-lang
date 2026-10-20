//! Mode-aware host-key epochs (design §6 Io, Mode-aware epochs).
//!
//! Each logical host key's transitions form a sequence of epochs, numbered from 1. An
//! exclusive transition is an epoch by itself; a shared transition joins the latest epoch only
//! if it is shared, so a queued exclusive transition is never overtaken. A transition of epoch
//! e may make its host call once every transition of epoch e - 1 has ended: an exclusive one
//! thereby waits for the whole preceding (possibly shared) epoch, and a shared one only for the
//! preceding exclusive epoch, never for the other members of its own. A transition ends when
//! its bounded call returns or registers its wait, never at its request's completion, so no
//! key is held across a wait.

/// One epoch of a key.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Epoch {
    pub shared: bool,
    pub members: u32,
    pub ended: u32,
}

/// One key's epochs, live or as a replay computes them from the tape.
#[derive(Clone, Debug, Default)]
pub struct KeyEpochs {
    epochs: Vec<Epoch>,
}

impl KeyEpochs {
    /// Take a position: the epoch of a new transition in `shared` or exclusive mode.
    pub fn take(&mut self, shared: bool) -> u64 {
        match self.epochs.last_mut() {
            Some(last) if shared && last.shared => last.members += 1,
            _ => self.epochs.push(Epoch {
                shared,
                members: 1,
                ended: 0,
            }),
        }
        self.epochs.len() as u64
    }

    /// Add a member to `epoch` as a tape names it (replay).
    pub fn member(&mut self, epoch: u64, shared: bool) -> Result<(), String> {
        let index = usize::try_from(epoch)
            .ok()
            .filter(|&e| e >= 1)
            .ok_or("epoch 0")?
            - 1;
        if index >= self.epochs.len() {
            self.epochs.resize(index + 1, Epoch::default());
        }
        let slot = &mut self.epochs[index];
        if slot.members > 0 && (slot.shared != shared || !shared) {
            return Err(format!(
                "epoch {epoch} mixes modes or holds two exclusive transitions"
            ));
        }
        slot.shared = shared;
        slot.members += 1;
        Ok(())
    }

    /// Whether a transition of `epoch` may make its host call.
    pub fn ready(&self, epoch: u64) -> bool {
        epoch <= 1
            || self
                .epochs
                .get(epoch as usize - 2)
                .is_some_and(|e| e.ended == e.members)
    }

    pub fn end(&mut self, epoch: u64) {
        let e = &mut self.epochs[epoch as usize - 1];
        assert!(
            e.ended < e.members,
            "an epoch ended more often than it has members"
        );
        e.ended += 1;
    }

    pub fn epochs(&self) -> &[Epoch] {
        &self.epochs
    }
}
