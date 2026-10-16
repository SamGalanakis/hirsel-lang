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
use crate::ir::Symbol;
use crate::layout_types::LayoutId;
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer};

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
