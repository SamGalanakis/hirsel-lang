//! Program-lifetime data a `ReadStatic` names (CT-2): a materialized
//! constant as a graph of immortal static blocks.
//!
//! Block 0 holds the constant itself. A `ReadStatic` whose result is a block
//! reference to block 0's payload layout yields that block (an uncounted
//! reference yields its payload); any other result reads the value of the
//! result's layout from block 0's payload, which then has that layout. Every
//! other block is a payload the value reaches through a counted reference
//! cell: a sequence behind a String, Bytes or List, a Box's content. A cell
//! names its target's block, as a block reference names its header; the
//! count is immortal, so a static is consumed without a credit and never
//! released. The bytes hold no pointer: each reference is a `(offset,
//! block)` cell, which the heap checker installs as a handle and emission as
//! a relocation, so the data is independent of where the blocks land.
//!
//! A *constant slot* ([`constant_slots`]) holds pointer-free static data for
//! its whole life: its one write stores a `ReadStatic` value and nothing else
//! writes it. Backends address such a slot in place, in a private read-only
//! copy of the bytes named by their content ([`constant_data_name`]), so an
//! indexed constant table is never copied to the stack.
use crate::ir::{AddressBase, AddressId, ArgumentValue, Inst, LirUnit, Ref, Symbol, ValueRepr};
use crate::layout_types::LayoutId;
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer};
use hirsel_syntax::ContentHash;

/// One constant's static blocks; block 0 is the constant's own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticData {
    pub symbol: Symbol,
    pub blocks: Vec<StaticBlock>,
}

impl StaticData {
    /// A constant of one pointer-free block of `layout`.
    pub fn plain(symbol: Symbol, layout: LayoutId, bytes: Vec<u8>) -> Self {
        Self {
            symbol,
            blocks: vec![StaticBlock {
                layout,
                bytes,
                references: Vec::new(),
            }],
        }
    }
}

/// An immortal block's payload: `bytes` of `layout`, with a counted block
/// reference to `blocks[target]` in the pointer-sized cell at each offset of
/// `references` (whose bytes are zero).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaticBlock {
    pub layout: LayoutId,
    pub bytes: Vec<u8>,
    pub references: Vec<StaticReference>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaticReference {
    pub offset: u64,
    pub target: u32,
}

impl Snapshot for StaticReference {
    fn write(&self, w: &mut Writer) {
        w.open("reference");
        w.uint(u128::from(self.offset));
        w.uint(u128::from(self.target));
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("reference")?;
        let offset = u64::try_from(r.uint()?).map_err(|_| r.error("reference offset"))?;
        let target = u32::try_from(r.uint()?).map_err(|_| r.error("reference target"))?;
        r.close()?;
        Ok(Self { offset, target })
    }
}

impl Snapshot for StaticBlock {
    fn write(&self, w: &mut Writer) {
        w.open("block");
        self.layout.write(w);
        w.bytes(&self.bytes);
        self.references.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("block")?;
        let block = Self {
            layout: LayoutId::read(r)?,
            bytes: r.bytes()?,
            references: Vec::read(r)?,
        };
        r.close()?;
        Ok(block)
    }
}

impl Snapshot for StaticData {
    fn write(&self, w: &mut Writer) {
        w.open("static");
        self.symbol.write(w);
        self.blocks.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.open("static")?;
        let data = Self {
            symbol: Symbol::read(r)?,
            blocks: Vec::read(r)?,
        };
        r.close()?;
        Ok(data)
    }
}

/// A slot whose content is always the pointer-free bytes of one static (see the module).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConstantSlot<'a> {
    /// The `ReadStatic` value its one store writes; the store writes nothing.
    pub stored: crate::ir::ValueId,
    /// The static's bytes at the slot's layout.
    pub bytes: &'a [u8],
}

/// The private read-only symbol holding constant `bytes` aligned to `align`: named by its
/// content, so units that index the same table share one name and no program numbering.
pub fn constant_data_name(align: u32, bytes: &[u8]) -> String {
    let digest = ContentHash::of_parts("hirsel.constant-data.v1", [&align.to_le_bytes(), bytes]);
    let hex: String = digest
        .0
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("hirsel.data.{hex}")
}

/// Per slot of `unit`, its static content when it is a constant slot: its only write is one
/// whole `Store` of an aggregate `ReadStatic` value whose static is one block without
/// references at the slot's layout, and every other operation on it, or on an address within it, only
/// prepares, reads, copies, borrows or ends its storage. A `mut` loan or argument, a
/// replace, a sequence or flat write, and any operation not listed rules a slot out.
pub fn constant_slots<'a, S>(
    unit: &LirUnit<S>,
    statics: &'a [StaticData],
) -> Vec<Option<ConstantSlot<'a>>> {
    let slots = unit.slots.len();
    let root = |address: AddressId| {
        let mut at = address;
        loop {
            match unit.addresses.get(at.index())?.base {
                AddressBase::Slot(slot) => return Some(slot.index()),
                AddressBase::Address(base) => at = base,
                AddressBase::Value(_) => return None,
            }
        }
    };
    let whole = |address: AddressId| {
        unit.addresses
            .get(address.index())
            .filter(|a| a.projection.len == 0)
            .and_then(|a| match a.base {
                AddressBase::Slot(slot) => Some(slot.index()),
                _ => None,
            })
    };
    // The static each value reads, if it is a `ReadStatic` result.
    let mut statics_of = vec![None; unit.values.len()];
    for inst in &unit.instructions {
        if let Inst::ReadStatic { result, symbol } = *inst
            && let Some(symbol) = unit.tables.symbols.get(symbol.index())
            && let Some(data) = statics.iter().find(|d| d.symbol == *symbol)
            && let [block] = data.blocks.as_slice()
            && block.references.is_empty()
            && let Some(slot) = statics_of.get_mut(result.index())
        {
            *slot = Some(block);
        }
    }
    let mut stores: Vec<Vec<crate::ir::ValueId>> = vec![Vec::new(); slots];
    let mut ruled_out = vec![false; slots];
    let mut rule_out = |address: AddressId, ruled_out: &mut Vec<bool>| {
        if let Some(slot) = root(address) {
            ruled_out[slot] = true;
        }
    };
    for inst in &unit.instructions {
        match *inst {
            Inst::Store { address, value, .. } => match whole(address) {
                Some(slot) => stores[slot].push(value),
                None => rule_out(address, &mut ruled_out),
            },
            Inst::Address { .. }
            | Inst::Read { .. }
            | Inst::Copy { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::EndOrigin { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::EndScope { .. } => {}
            _ => inst.visit(&mut |r| match r {
                Ref::Address(address) => rule_out(address, &mut ruled_out),
                Ref::Slot(slot) => ruled_out[slot.index()] = true,
                _ => {}
            }),
        }
    }
    for loan in &unit.tables.loans {
        if loan.mutable {
            rule_out(loan.address, &mut ruled_out);
        }
    }
    for argument in &unit.pools.call_arguments {
        if let ArgumentValue::Address { address, .. } = argument.value {
            rule_out(address, &mut ruled_out);
        }
    }
    for &address in &unit.pools.addresses {
        rule_out(address, &mut ruled_out);
    }
    for terminator in &unit.terminators {
        terminator.visit(&mut |r| match r {
            Ref::Address(address) => rule_out(address, &mut ruled_out),
            Ref::Slot(slot) => ruled_out[slot.index()] = true,
            _ => {}
        });
    }
    (0..slots)
        .map(|slot| {
            let [stored] = stores[slot].as_slice() else {
                return None;
            };
            let block = statics_of.get(stored.index()).copied().flatten()?;
            let slot_layout = unit.types.get(unit.slots[slot].ty.index())?.layout;
            let value = unit
                .types
                .get(unit.values.get(stored.index())?.ty.index())?;
            (!ruled_out[slot]
                && value.repr == ValueRepr::Aggregate
                && value.layout == slot_layout
                && block.layout == slot_layout)
                .then_some(ConstantSlot {
                    stored: *stored,
                    bytes: &block.bytes,
                })
        })
        .collect()
}
