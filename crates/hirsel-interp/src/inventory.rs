//! Semantic retained storage (MEM-4, decision 649442).
//!
//! Entries describe language allocations, never backing buffers or live bindings.
//! Growth preserves an entry, COW creates another, and lexical last use removes
//! neither. The payload is a snapshot supplied by the interpreter, not an owned
//! copy: recording it must not acquire Shared roots. Ownership is transferred
//! separately when a scope closes or a seal commits.
use std::collections::BTreeMap;

mod typed;
pub(crate) use typed::value_receipt;
pub use typed::{BlockPlacement, ChargeRoot, PlacementReceipt, heap_charge, heap_charge_with};

/// Identity within one arena's inventory; independent of host addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct LogicalBlockId(pub u64);

/// An exact logical charge, including sums greater than the largest capacity.
/// Saturating to u64::MAX would incorrectly admit overflow at that capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Charge {
    Bytes(u64),
    Overflow,
}

impl Charge {
    pub const ZERO: Self = Self::Bytes(0);

    pub fn plus(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bytes(a), Self::Bytes(b)) => {
                a.checked_add(b).map_or(Self::Overflow, Self::Bytes)
            }
            _ => Self::Overflow,
        }
    }

    pub fn times(self, occurrences: u64) -> Self {
        match (self, occurrences) {
            (_, 0) => Self::ZERO,
            (Self::Bytes(bytes), n) => bytes.checked_mul(n).map_or(Self::Overflow, Self::Bytes),
            _ => Self::Overflow,
        }
    }

    pub fn fits(self, remaining: u64) -> bool {
        matches!(self, Self::Bytes(bytes) if bytes <= remaining)
    }
}

/// One retained allocation and the tree charge of its outgoing heap occurrences.
/// Arena child references stop at that child's separate inventory entry; Shared
/// roots are boundaries and contribute zero. Repeated heap occurrences count
/// repeatedly, independently of physical relocation deduplication.
#[derive(Debug)]
pub struct RetainedAllocation<T> {
    pub payload: T,
    pub outgoing_heap: Charge,
}

/// Bottom-up summary of the arena's retained semantic allocations.
#[derive(Debug)]
pub struct ArenaInventory<T> {
    next: u64,
    entries: BTreeMap<LogicalBlockId, RetainedAllocation<T>>,
    // At most usize::MAX entries, each at most u64::MAX, fit u128 on
    // supported hosts. Keep overflow entries separately so replacing an
    // overflowing payload can restore a finite total without a recount.
    bytes: u128,
    overflowing: usize,
}

impl<T> Default for ArenaInventory<T> {
    fn default() -> Self {
        Self {
            next: 0,
            entries: BTreeMap::new(),
            bytes: 0,
            overflowing: 0,
        }
    }
}

impl<T> ArenaInventory<T> {
    /// Record a construction or semantic COW copy. Caller has already charged
    /// construction; this records only the additional evacuation obligation.
    pub fn allocate(&mut self, payload: T, outgoing_heap: Charge) -> LogicalBlockId {
        let id = LogicalBlockId(self.next);
        self.next = self
            .next
            .checked_add(1)
            .expect("logical allocation identity exhausted");
        self.add(outgoing_heap);
        self.entries.insert(
            id,
            RetainedAllocation {
                payload,
                outgoing_heap,
            },
        );
        id
    }

    /// Replace a unique builder's active payload. Physical growth uses the same
    /// id; callers must allocate a fresh entry for a COW copy. Return the old
    /// snapshot, without releasing or acquiring any language ownership.
    pub fn replace(
        &mut self,
        id: LogicalBlockId,
        payload: T,
        outgoing_heap: Charge,
    ) -> Result<T, T> {
        let Some(previous) = self.entries.get(&id) else {
            return Err(payload);
        };
        let previous_charge = previous.outgoing_heap;
        self.subtract(previous_charge);
        self.add(outgoing_heap);
        let previous = self.entries.insert(
            id,
            RetainedAllocation {
                payload,
                outgoing_heap,
            },
        );
        Ok(previous.expect("existing allocation").payload)
    }

    /// O(1) seal preflight. `root_extra` is the heap root's charge or the heap
    /// fields of an inline root. An arena root contributes ZERO: its payload is
    /// already present, including when the root binding itself is dead.
    pub fn seal_charge(&self, root_extra: Charge) -> Charge {
        let retained = if self.overflowing != 0 {
            Charge::Overflow
        } else {
            u64::try_from(self.bytes).map_or(Charge::Overflow, Charge::Bytes)
        };
        retained.plus(root_extra)
    }

    pub fn entries(&self) -> &BTreeMap<LogicalBlockId, RetainedAllocation<T>> {
        &self.entries
    }

    /// Move the inventory at commit/close. No snapshot clone is an ownership
    /// transfer, and there is deliberately no remove-on-last-use operation.
    pub fn into_entries(self) -> BTreeMap<LogicalBlockId, RetainedAllocation<T>> {
        self.entries
    }

    fn add(&mut self, charge: Charge) {
        match charge {
            Charge::Bytes(bytes) => self.bytes += u128::from(bytes),
            Charge::Overflow => self.overflowing += 1,
        }
    }

    fn subtract(&mut self, charge: Charge) {
        match charge {
            Charge::Bytes(bytes) => self.bytes -= u128::from(bytes),
            Charge::Overflow => self.overflowing -= 1,
        }
    }
}
