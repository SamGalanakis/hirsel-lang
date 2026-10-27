//! Physical CPU allocations retain their exact task and allocation generation.
//! This table owns no counted holder. Saved mapper frames own their contents.
use crate::Header;
use std::collections::{BTreeMap, btree_map::Entry};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Owner {
    pub(crate) task: u64,
    pub(crate) ordinal: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Block {
    address: usize,
    generation: u64,
    owner: Owner,
}

#[derive(Debug, Default)]
pub(crate) struct Blocks {
    generation: u64,
    live: BTreeMap<usize, Block>,
}

impl Blocks {
    /// The checked mapper has exclusive custody of this fresh allocation.
    pub(crate) fn adopt(&mut self, owner: Owner, block: *mut Header) -> Block {
        assert!(!block.is_null());
        let Entry::Vacant(slot) = self.live.entry(block.addr()) else {
            panic!("a CPU block already has an owner");
        };
        self.generation = self
            .generation
            .checked_add(1)
            .expect("CPU block generation");
        let block = Block {
            address: block.addr(),
            generation: self.generation,
            owner,
        };
        slot.insert(block);
        block
    }

    /// Once mapper cleanup finishes, ordinary values own escaped results.
    pub(crate) fn disown(&mut self, owner: Owner) {
        self.live.retain(|_, block| block.owner != owner);
    }

    pub(crate) fn release(&mut self, pointer: *mut Header) {
        self.live.remove(&pointer.addr());
    }

    /// Unknown roots are borrowed inputs. A registered mapper block must
    /// belong to this exact task generation and ordinal before it is retained.
    pub(crate) fn retain(&self, owner: Owner, pointers: &[*mut Header]) -> Option<Vec<Block>> {
        pointers
            .iter()
            .filter_map(|pointer| self.live.get(&pointer.addr()))
            .map(|block| (block.owner == owner).then_some(*block))
            .collect()
    }

    pub(crate) fn valid(&self, retained: &[Block]) -> bool {
        retained
            .iter()
            .all(|block| self.live.get(&block.address) == Some(block))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recycled_addresses_and_task_slots_do_not_adopt_an_earlier_generation() {
        let mut blocks = Blocks::default();
        // Opaque keys are never dereferenced by the ownership table.
        let pointer = std::ptr::dangling_mut::<Header>();
        let owner = Owner {
            task: (7 << 32) | 3,
            ordinal: 1,
        };
        let old = blocks.adopt(owner, pointer);
        assert_eq!(blocks.retain(owner, &[pointer]), Some(vec![old]));
        assert!(
            blocks
                .retain(
                    Owner {
                        task: owner.task + (1 << 32),
                        ..owner
                    },
                    &[pointer]
                )
                .is_none()
        );
        assert!(
            blocks
                .retain(
                    Owner {
                        ordinal: 0,
                        ..owner
                    },
                    &[pointer]
                )
                .is_none()
        );
        blocks.release(pointer);
        assert!(!blocks.valid(&[old]));
        let new = blocks.adopt(owner, pointer);
        assert_ne!(new, old);
        assert!(!blocks.valid(&[old]));
        assert!(blocks.valid(&[new]));
        blocks.disown(Owner {
            task: owner.task + (1 << 32),
            ..owner
        });
        assert!(blocks.valid(&[new]));
        blocks.disown(owner);
        assert!(!blocks.valid(&[new]));
        assert_eq!(
            blocks.retain(
                Owner {
                    task: owner.task + (1 << 32),
                    ..owner
                },
                &[pointer]
            ),
            Some(vec![])
        );
    }
}
