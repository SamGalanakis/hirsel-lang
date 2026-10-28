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
    region: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Observed {
    address: usize,
    generation: u64,
}

#[derive(Debug, Default)]
pub(crate) struct Blocks {
    generation: u64,
    live: BTreeMap<usize, Block>,
    regions: BTreeMap<usize, Owner>,
    observed: BTreeMap<usize, Observed>,
}

impl Blocks {
    /// A saved ordinal binds every allocation still in its
    /// custody, including locals in parent helper frames. No payload is read:
    /// an allocation handoff may have returned an uninitialized block.
    pub(crate) fn owned(&self, owner: Owner) -> Vec<Block> {
        self.live
            .values()
            .filter(|block| block.owner == owner)
            .copied()
            .collect()
    }

    /// Observe an input allocation without claiming its existing credits.
    /// Several saved ordinals can observe the same borrowed input generation.
    pub(crate) fn observe(&mut self, pointer: *mut Header) -> Observed {
        *self.observed.entry(pointer.addr()).or_insert_with(|| {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("CPU block generation");
            Observed {
                address: pointer.addr(),
                generation: self.generation,
            }
        })
    }

    /// A mapper can consume an input's container and move its children into
    /// helper locals. Filter by allocator generations, never by rereading the
    /// original container after its credit has moved.
    pub(crate) fn surviving(&self, input: &[Observed]) -> Vec<Observed> {
        input
            .iter()
            .filter(|block| self.observed.get(&block.address) == Some(block))
            .copied()
            .collect()
    }

    pub(crate) fn valid_observed(&self, input: &[Observed]) -> bool {
        input
            .iter()
            .all(|block| self.observed.get(&block.address) == Some(block))
    }

    pub(crate) fn valid_roots(
        &self,
        allocations: &[Block],
        inputs: &[Observed],
        roots: &[*mut Header],
    ) -> bool {
        allocations
            .iter()
            .filter(|block| roots.iter().any(|root| root.addr() == block.address))
            .all(|block| self.live.get(&block.address) == Some(block))
            && inputs
                .iter()
                .filter(|block| roots.iter().any(|root| root.addr() == block.address))
                .all(|block| self.observed.get(&block.address) == Some(block))
    }

    /// The checked mapper has exclusive custody of this fresh allocation.
    pub(crate) fn adopt(&mut self, owner: Owner, block: *mut Header) -> Block {
        self.adopt_in(owner, block, 0)
    }

    pub(crate) fn region_owned(&self, owner: Owner, region: usize) -> bool {
        self.regions
            .get(&region)
            .is_none_or(|registered| *registered == owner)
    }

    pub(crate) fn adopt_in(&mut self, owner: Owner, block: *mut Header, region: usize) -> Block {
        assert!(!block.is_null());
        if region != 0 {
            assert!(self.region_owned(owner, region), "an ordinal-local region");
            self.regions.insert(region, owner);
        }
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
            region,
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
        self.observed.remove(&pointer.addr());
    }

    pub(crate) fn release_region(&mut self, region: usize) {
        self.live.retain(|_, block| block.region != region);
        self.regions.remove(&region);
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
    fn regional_generations_end_at_exit_and_an_ordinal_cannot_borrow_another_region() {
        let mut blocks = Blocks::default();
        let pointer = std::ptr::dangling_mut::<Header>();
        let owner = Owner {
            task: (7 << 32) | 3,
            ordinal: 1,
        };
        let region = 128;
        let old = blocks.adopt_in(owner, pointer, region);
        assert!(!blocks.region_owned(
            Owner {
                ordinal: 2,
                ..owner
            },
            region
        ));
        assert!(!blocks.region_owned(
            Owner {
                task: owner.task + (1 << 32),
                ..owner
            },
            region
        ));
        blocks.release_region(region);
        assert!(!blocks.valid(&[old]));
        let next = Owner {
            ordinal: 2,
            ..owner
        };
        assert!(blocks.region_owned(next, region));
        let new = blocks.adopt_in(next, pointer, region);
        assert_ne!(new, old);
        assert!(!blocks.valid(&[old]));
        assert!(blocks.valid(&[new]));
        blocks.release_region(region);
        blocks.release_region(region);
        assert!(!blocks.valid(&[new]));
    }

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
