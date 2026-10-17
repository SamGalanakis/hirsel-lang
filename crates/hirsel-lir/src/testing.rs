//! Raw LIR assembly shared by hand-built validator and backend fixtures.
//! Suite-specific layouts, identities, cleanup recipes and validation stay with the tests.
use crate::ir::*;
use crate::stage::Stage;

/// Buffers each block's instructions and terminator until the unit is finished.
/// The caller initializes the header and side tables and declares the entry block.
pub struct UnitBuilder<S> {
    pub unit: LirUnit<S>,
    insts: Vec<Vec<Inst>>,
    terms: Vec<Option<Terminator>>,
    propagate: Option<BlockId>,
}

impl<S: Stage> UnitBuilder<S> {
    pub fn new(unit: LirUnit<S>) -> Self {
        Self {
            unit,
            insts: Vec::new(),
            terms: Vec::new(),
            propagate: None,
        }
    }
    pub fn block(&mut self, kind: BlockKind, params: &[ValueId]) -> BlockId {
        let parameters = PoolRange::push(&mut self.unit.pools.values, params.iter().copied());
        self.unit.blocks.push(Block {
            kind,
            parameters,
            instructions: PoolRange::EMPTY,
            anchor: SourceAnchorId(0),
        });
        self.insts.push(Vec::new());
        self.terms.push(None);
        BlockId(self.unit.blocks.len() as u32 - 1)
    }

    pub fn push(&mut self, b: BlockId, inst: Inst) {
        self.insts[b.index()].push(inst);
    }

    pub fn term(&mut self, b: BlockId, term: Terminator) {
        self.terms[b.index()] = Some(term);
    }

    pub fn edge(&mut self, target: BlockId, args: &[ValueId]) -> EdgeId {
        let arguments = PoolRange::push(&mut self.unit.pools.values, args.iter().copied());
        self.unit.tables.edges.push(Edge { target, arguments });
        EdgeId(self.unit.tables.edges.len() as u32 - 1)
    }

    pub fn jump(&mut self, from: BlockId, to: BlockId, args: &[ValueId]) {
        let e = self.edge(to, args);
        self.term(from, Terminator::Jump(e));
    }

    pub fn propagate(&mut self) -> BlockId {
        if let Some(b) = self.propagate {
            return b;
        }
        let b = self.block(BlockKind::FailureCleanup, &[]);
        self.term(b, Terminator::Propagate);
        self.propagate = Some(b);
        b
    }

    pub fn ret(&mut self, b: BlockId, value: Option<ValueId>) {
        self.term(b, Terminator::Return(value));
    }

    pub fn finish(mut self) -> LirUnit<S> {
        for (b, insts) in self.insts.iter().enumerate() {
            let range = PoolRange::push(&mut self.unit.instructions, insts.iter().copied());
            self.unit.blocks[b].instructions = range;
        }
        self.unit.terminators = self
            .terms
            .iter()
            .enumerate()
            .map(|(b, t)| {
                t.clone()
                    .unwrap_or_else(|| panic!("block {b} has no terminator"))
            })
            .collect();
        self.unit
    }
}
