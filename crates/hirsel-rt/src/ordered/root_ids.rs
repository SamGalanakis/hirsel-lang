//! Shared roots have a task-local freeze ordinal, not an operation ordinal.
//! Freeze remains Local: only this trace-only concurrent registry is shared.
//! Like root_trace.rs:62-68, addresses are live lookup keys and are replaced on
//! storage reuse. They never enter the canonical identity or event value.
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SharedRootId {
    pub(crate) creator: Vec<u32>,
    pub(crate) freeze: u64,
}

#[derive(Default)]
pub(crate) struct Creator {
    pub(crate) path: Vec<u32>,
    next: u64,
}

#[derive(Default)]
pub(crate) struct Registry(Mutex<BTreeMap<usize, SharedRootId>>);
impl Registry {
    pub(crate) fn get(&self, block: *mut crate::Header) -> Option<SharedRootId> {
        self.0.lock().unwrap().get(&block.addr()).cloned()
    }
    pub(crate) fn retire(&self, block: *mut crate::Header) {
        self.0.lock().unwrap().remove(&block.addr());
    }
    fn insert(&self, block: *mut crate::Header, creator: &mut Creator) {
        let freeze = creator.next;
        creator.next = freeze.checked_add(1).expect("task freeze ordinal overflow");
        self.0.lock().unwrap().insert(
            block.addr(),
            SharedRootId {
                creator: creator.path.clone(),
                freeze,
            },
        );
    }
}

/// No scheduler/worker borrow or operation is introduced by this observer.
/// # Safety
/// The task exclusively owns this fresh frozen root and its Creator.
pub(crate) unsafe fn frozen(ctx: *mut crate::TaskCtx, block: *mut crate::Header) {
    let run = unsafe { &*(*ctx).run_ptr() };
    if let Some(registry) = run.ordered_roots.get() {
        registry.insert(block, unsafe { &mut (*ctx).roots.trace_creator });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_addresses_never_reuse_a_previous_root_identity() {
        let registry = Registry::default();
        let address = std::ptr::without_provenance_mut(0x1000);
        let mut first = Creator {
            path: vec![2],
            ..Default::default()
        };
        let mut second = Creator {
            path: vec![7],
            ..Default::default()
        };
        registry.insert(address, &mut first);
        assert_eq!(
            registry.get(address),
            Some(SharedRootId {
                creator: vec![2],
                freeze: 0
            })
        );
        registry.retire(address);
        assert_eq!(registry.get(address), None);
        registry.insert(address, &mut second);
        assert_eq!(
            registry.get(address),
            Some(SharedRootId {
                creator: vec![7],
                freeze: 0
            })
        );
        registry.retire(address);
        registry.insert(address, &mut first);
        assert_eq!(
            registry.get(address),
            Some(SharedRootId {
                creator: vec![2],
                freeze: 1
            })
        );
    }
}
