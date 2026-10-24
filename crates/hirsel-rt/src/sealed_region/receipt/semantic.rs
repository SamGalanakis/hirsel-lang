//! Bottom-up semantic obligations, separate from physical storage and credits.
//! The compiler must independently prove the original type, origin and logical
//! allocation size used by each construction. These records carry no authority
//! to dereference their descriptive image or release a language holder.
//!
//! Inventory arithmetic follows the existing MEM-4 interpreter inventory's
//! per-entry u128 total plus separate overflowing-entry count. H remains an
//! independent implementation over its own cells. No structural graph is added.
use super::{MetadataRun, Receipt};
use crate::sealed_region::Charge;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;

static GRANTS: AtomicU64 = AtomicU64::new(1);

/// Runtime semantic identity, independent of addresses, frames and metadata
/// node generation. Only demanded scope metadata creates one of these tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct GrantId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct AllocationId {
    grant: GrantId,
    ordinal: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Origin {
    Inline,
    Heap,
    /// An ordinary immutable static block still has tree-copy cost when its
    /// contents are evacuated. Global handles use their own boundary contract.
    Static,
    Arena(GrantId),
    Shared,
}

/// One foreign arena and several foreign arenas are both incompatible with a
/// consuming grant. Keeping this finite summary does not reject an ordinary
/// intermediate graph: only actual seal preflight requires compatibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Grants {
    None,
    One(GrantId),
    Mixed,
}
impl Grants {
    fn plus(self, other: Self) -> Self {
        match (self, other) {
            (Self::None, b) | (b, Self::None) => b,
            (Self::One(a), Self::One(b)) if a == b => Self::One(a),
            _ => Self::Mixed,
        }
    }
    fn accepts(self, grant: GrantId) -> bool {
        self == Self::None || self == Self::One(grant)
    }
}

/// Immutable summaries for a source value. Child links retain every original
/// occurrence, including repeated equal nodes; no pointer dedup changes charge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record {
    origin: Origin,
    logical: Charge,
    heap: Charge,
    outgoing: Charge,
    grants: Grants,
}
impl Record {
    pub fn origin(&self) -> Origin {
        self.origin
    }
    pub fn logical_charge(&self) -> Charge {
        self.logical
    }
    pub fn heap_charge(&self) -> Charge {
        self.heap
    }
    pub fn outgoing_heap_charge(&self) -> Charge {
        self.outgoing
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Metadata(super::Error),
    InvalidBoundary,
    WrongAllocationOrigin,
    UnknownAllocation,
    ForeignArena,
}
impl From<super::Error> for Error {
    fn from(error: super::Error) -> Self {
        Self::Metadata(error)
    }
}

/// Construct from the original logical allocation and ordered child snapshots.
/// `logical` is the block's own logical bytes, excluding padding/capacity; it
/// is zero for Inline/Shared. Original Heap is never inferred from a header.
/// Shared is a boundary: its frozen graph is not an owned child of this record.
/// Bytes are initialized descriptive data, not a storage-credit acquisition.
pub fn snapshot(
    run: MetadataRun,
    origin: Origin,
    logical: Charge,
    bytes: &[u8],
    alignment: usize,
    children: &[Receipt<Record>],
) -> Result<Receipt<Record>, Error> {
    if children.iter().any(|child| child.run() != run) {
        return Err(Error::Metadata(super::Error::DifferentRun));
    }
    if (matches!(origin, Origin::Inline | Origin::Shared) && logical != Charge::Bytes(0))
        || (origin == Origin::Shared && !children.is_empty())
    {
        return Err(Error::InvalidBoundary);
    }
    let outgoing = children
        .iter()
        .fold(Charge::Bytes(0), |sum, child| sum.plus(child.record().heap));
    let children_grants = children
        .iter()
        .fold(Grants::None, |sum, child| sum.plus(child.record().grants));
    let own_grant = match origin {
        Origin::Arena(grant) => Grants::One(grant),
        _ => Grants::None,
    };
    let heap = match origin {
        Origin::Heap | Origin::Static => logical.plus(outgoing),
        Origin::Inline => outgoing,
        Origin::Arena(_) | Origin::Shared => Charge::Bytes(0),
    };
    Ok(run.node_with_bytes(
        Record {
            origin,
            logical,
            heap,
            outgoing,
            grants: own_grant.plus(children_grants),
        },
        bytes,
        alignment,
        children,
    )?)
}

/// Metadata retained by an explicit arena. This owns only private snapshots.
/// Actual ordinary/Shared source credits remain a SEPARATE guard/journal
/// obligation until compiler custody transfer is implemented and checked.
/// No last-use removal exists; growth replaces, semantic COW allocates anew.
/// Allocation identities include the grant, so another inventory's local
/// ordinal cannot replace an entry here.
pub struct Inventory {
    run: MetadataRun,
    grant: GrantId,
    next: u64,
    entries: BTreeMap<AllocationId, Receipt<Record>>,
    bytes: u128,
    overflowing: usize,
    foreign: usize,
}
impl Inventory {
    pub fn new(run: MetadataRun) -> Self {
        Self {
            run,
            grant: GrantId(super::fresh(&GRANTS)),
            next: 0,
            entries: BTreeMap::new(),
            bytes: 0,
            overflowing: 0,
            foreign: 0,
        }
    }
    pub fn grant(&self) -> GrantId {
        self.grant
    }
    pub fn entries(&self) -> impl ExactSizeIterator<Item = (AllocationId, &Receipt<Record>)> {
        self.entries.iter().map(|(&id, value)| (id, value))
    }
    fn check(&self, value: &Receipt<Record>) -> Result<(), Error> {
        if value.run() != self.run {
            return Err(Error::Metadata(super::Error::DifferentRun));
        }
        if value.record().origin != Origin::Arena(self.grant) {
            return Err(Error::WrongAllocationOrigin);
        }
        Ok(())
    }
    fn add(&mut self, record: &Record) {
        match record.outgoing {
            Charge::Bytes(bytes) => self.bytes += u128::from(bytes),
            Charge::Overflow => self.overflowing += 1,
        }
        self.foreign += usize::from(!record.grants.accepts(self.grant));
    }
    fn subtract(&mut self, record: &Record) {
        match record.outgoing {
            Charge::Bytes(bytes) => self.bytes -= u128::from(bytes),
            Charge::Overflow => self.overflowing -= 1,
        }
        self.foreign -= usize::from(!record.grants.accepts(self.grant));
    }
    /// Record only after successful source construction. A snapshot clone does
    /// not create a semantic allocation, nor acquire an ordinary/Shared credit.
    pub fn allocate(&mut self, value: Receipt<Record>) -> Result<AllocationId, Error> {
        self.check(&value)?;
        let id = AllocationId {
            grant: self.grant,
            ordinal: self.next,
        };
        self.next = self
            .next
            .checked_add(1)
            .unwrap_or_else(|| crate::fault("semantic allocation identity exhausted"));
        self.add(value.record());
        self.entries.insert(id, value);
        Ok(id)
    }
    /// Preserve a unique builder's semantic identity. Return its old metadata,
    /// acquiring or retiring no language credits. COW must call allocate instead.
    pub fn replace(
        &mut self,
        id: AllocationId,
        value: Receipt<Record>,
    ) -> Result<Receipt<Record>, Error> {
        self.check(&value)?;
        let old = self.entries.get(&id).ok_or(Error::UnknownAllocation)?;
        let record = *old.record();
        self.subtract(&record);
        self.add(value.record());
        Ok(self
            .entries
            .insert(id, value)
            .expect("existing semantic allocation"))
    }
    /// O(1) additional seal charge from the maintained inventory and root.
    /// An arena root contributes zero, including its already retained edges.
    /// Every ordinary heap occurrence contributes, even if relocation dedups.
    pub fn seal_charge(&self, root: &Receipt<Record>) -> Result<Charge, Error> {
        if root.run() != self.run {
            return Err(Error::Metadata(super::Error::DifferentRun));
        }
        if self.foreign != 0 || !root.record().grants.accepts(self.grant) {
            return Err(Error::ForeignArena);
        }
        let retained = if self.overflowing == 0 {
            u64::try_from(self.bytes).map_or(Charge::Overflow, Charge::Bytes)
        } else {
            Charge::Overflow
        };
        Ok(retained.plus(root.record().heap))
    }
}

#[cfg(test)]
mod tests;
