//! Live controls and native affine identities. Control addresses may repeat;
//! process generation identities never do, so an old native handle cannot bind
//! a new allocation at the same address. No opaque native bits are dereferenced.
use crate::fault;
use std::collections::BTreeMap;

struct Entry {
    generation: u64,
    native: bool,
}
#[derive(Default)]
pub(crate) struct Owners {
    controls: BTreeMap<usize, Entry>,
    handles: BTreeMap<usize, usize>,
    metadata_run: Option<super::receipt::MetadataRun>,
    native_guards: BTreeMap<usize, usize>,
}
impl Owners {
    pub(super) fn metadata_run(&mut self) -> super::receipt::MetadataRun {
        *self
            .metadata_run
            .get_or_insert_with(super::receipt::MetadataRun::new)
    }

    pub(super) fn enter_guard(&mut self, guard: usize, ctx: usize) {
        if self.native_guards.contains_key(&guard) {
            fault("demanded arena guard entered twice");
        }
        self.native_guards.insert(guard, ctx);
    }
    pub(super) fn check_guard(&self, guard: usize, ctx: usize) {
        if self.native_guards.get(&guard) != Some(&ctx) {
            fault("foreign or closed demanded arena guard");
        }
    }
    pub(super) fn exit_guard(&mut self, guard: usize, ctx: usize) {
        self.check_guard(guard, ctx);
        self.native_guards.remove(&guard);
    }

    pub(super) fn register(&mut self, address: usize, generation: u64) {
        let handle = usize::try_from(generation)
            .unwrap_or_else(|_| fault("sealed native identity exhausted"));
        if handle == 0 || self.controls.contains_key(&address) || self.handles.contains_key(&handle)
        {
            fault("sealed control or identity registered twice");
        }
        self.controls.insert(
            address,
            Entry {
                generation,
                native: false,
            },
        );
        // Reserve both indexes while preparation may still allocate. Native
        // publication only flips the existing row, and retirement only removes.
        self.handles.insert(handle, address);
    }
    pub(super) fn control(&self, address: usize) -> u64 {
        let entry = self
            .controls
            .get(&address)
            .unwrap_or_else(|| fault("foreign or dead sealed owner"));
        if entry.native {
            fault("sealed Rust owner was already transferred");
        }
        entry.generation
    }
    pub(super) fn publish(&mut self, address: usize) -> usize {
        let generation = self.control(address);
        self.controls
            .get_mut(&address)
            .expect("checked live control")
            .native = true;
        generation as usize
    }
    pub(super) fn native(&self, handle: usize) -> (usize, u64) {
        self.lookup(handle)
            .unwrap_or_else(|| fault("foreign or dead sealed owner"))
    }
    fn lookup(&self, handle: usize) -> Option<(usize, u64)> {
        let address = *self.handles.get(&handle)?;
        let entry = self.controls.get(&address)?;
        (entry.native && entry.generation as usize == handle).then_some((address, entry.generation))
    }
    pub(super) fn take_control(&mut self, address: usize) -> u64 {
        let generation = self.control(address);
        self.remove(address, generation);
        generation
    }
    pub(super) fn take_native(&mut self, handle: usize) -> (usize, u64) {
        let (address, generation) = self.native(handle);
        self.remove(address, generation);
        (address, generation)
    }
    fn remove(&mut self, address: usize, generation: u64) {
        self.controls.remove(&address);
        self.handles.remove(&(generation as usize));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_guard_entry_33_metadata_is_lazy_and_shared_only_by_its_actual_run() {
        let mut owners = Owners::default();
        assert!(owners.metadata_run.is_none());
        assert!(owners.native_guards.is_empty());
        let run = owners.metadata_run();
        assert_eq!(owners.metadata_run(), run);
        assert_ne!(Owners::default().metadata_run(), run);
    }
    #[test]
    fn native_sealed_access_33_same_address_replacement_cannot_revive_a_stale_identity() {
        let mut owners = Owners::default();
        let address = 0x1000;
        owners.register(address, 41);
        let old = owners.publish(address);
        assert_eq!(owners.take_native(old), (address, 41));
        owners.register(address, 42);
        let fresh = owners.publish(address);
        assert_ne!(old, fresh);
        assert_eq!(
            owners.lookup(old),
            None,
            "same-address read cannot revive the old identity"
        );
        // Native read and native release both use this exact lookup before
        // touching a control; the new owner survives either stale lookup.
        assert_eq!(
            owners.lookup(old),
            None,
            "same-address release cannot retire the new owner"
        );
        assert_eq!(owners.native(fresh), (address, 42));
        assert_eq!(owners.take_native(fresh), (address, 42));
        assert!(owners.controls.is_empty() && owners.handles.is_empty());
    }
}
