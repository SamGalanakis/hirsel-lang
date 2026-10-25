//! Constructor-tail destination passing (LANG-10): the instance's root box,
//! its carried hole, and the fills at sites and returns. `crate::normalize`
//! describes the protocol; `hirsel_lir::validate` checks it independently.
use super::*;
use crate::normalize::{HoleSite, HoleStep};

/// The storage of an instance with constructor-tail sites.
#[derive(Clone, Copy, Debug)]
pub(super) struct Holes {
    /// The root box: its payload is the result while the loop builds it.
    pub root: SlotId,
    /// The hole the next fill writes.
    pub hole: SlotId,
    /// A site's new hole while the chain around it is built.
    pub next: SlotId,
    /// The box's stand-in while its payload is open.
    pub open: LayoutId,
    /// The payload layout: the result's.
    pub payload: LayoutId,
}

impl Importer<'_> {
    /// Make the root and hole slots of an instance with constructor-tail sites.
    pub(super) fn hole_slots(&mut self) -> R<()> {
        let Some(boxed) = self.plan.and_then(|p| p.boxed()) else {
            return Ok(());
        };
        let ty = self.lir_type(boxed)?;
        let layout = self.layout_of(ty);
        let LayoutKind::Box { payload } = self.layouts.record(layout).kind else {
            return malformed("a constructor-tail box that is not a box");
        };
        let open = self
            .layouts
            .open_box(layout)
            .map_err(|e| PassError::Layout(self.def.path.path().clone(), e))?;
        let hole_ty = self.intern_repr(ValueRepr::Hole, layout);
        let scope = ScopeId(0);
        let root = self.new_slot(ty, SlotKind::Local, scope);
        let hole = self.new_slot(hole_ty, SlotKind::Local, scope);
        let next = self.new_slot(hole_ty, SlotKind::Local, scope);
        self.holes = Some(Holes {
            root,
            hole,
            next,
            open,
            payload,
        });
        Ok(())
    }

    /// After `mark_versioned`: the carried slots join at the loop header.
    pub(super) fn version_holes(&mut self) {
        if let Some(h) = self.holes {
            for s in [h.root, h.hole, h.next] {
                self.versioned[s.index()] = true;
            }
        }
    }

    /// Open a hole of the instance's box: its block and its hole.
    fn open_hole(&mut self) -> R<(ValueId, ValueId)> {
        let h = self.holes.expect("hole slots");
        let anchor = self.anchor_here();
        let construct = hirsel_lir::ir::ConstructId(self.out.tables.constructs.len() as u32);
        self.out.tables.constructs.push(ConstructSite {
            arena_no_drop: false,
            tracked: false,
            constructor: ConstructorKind::Box,
            layout: h.payload,
            fields: PoolRange::EMPTY,
            destination: DestinationClass::Heap,
            placement: None,
            anchor,
        });
        let boxed = self.out.slots[h.root.index()].ty;
        let hole_ty = self.out.slots[h.hole.index()].ty;
        let result = self.fresh(boxed);
        let hole = self.fresh(hole_ty);
        let failure = self.fail(None)?;
        self.push(Inst::HoleOpen {
            result,
            hole,
            construct,
            open: h.open,
            failure,
        });
        Ok((result, hole))
    }

    /// Put an owned value into a dead slot.
    fn hold(&mut self, slot: SlotId, value: ValueId) -> R<()> {
        self.push(Inst::StorageLive { slot });
        self.state.live(slot);
        let p = self.whole(slot);
        let address = self.fixed_address(&p)?;
        self.store(&p, address, value)
    }

    /// Take the whole content of a slot, which then ends.
    fn take_held(&mut self, slot: SlotId) -> R<ValueId> {
        let p = self.whole(slot);
        let address = self.fixed_address(&p)?;
        let v = self.operand_at(UseKind::Take, &p, address, None)?;
        self.push(Inst::StorageDead {
            slot,
            if_live: false,
        });
        self.state.dead(slot);
        self.versions[slot.index()] = None;
        Ok(v)
    }

    /// The prologue: the root box opens and its hole waits for the result.
    pub(super) fn enter_holes(&mut self) -> R<()> {
        let Some(h) = self.holes else {
            return Ok(());
        };
        let (root, hole) = self.open_hole()?;
        self.hold(h.root, root)?;
        self.hold(h.hole, hole)
    }

    /// A constructor-tail site: open the new hole at the chain's `box`, build
    /// the rest of the chain around it, fill the previous hole and carry the
    /// new one. The back edge follows.
    pub(super) fn fill_site(&mut self, site: &HoleSite) -> R<()> {
        let h = self.holes.expect("hole slots");
        for &(b, i, step) in &site.steps {
            let instruction = &self.body.blocks[b.0 as usize].instructions[i];
            self.statement = (b.0, i as u32);
            self.span = instruction.span;
            self.failure = FailureContext {
                cleanup: Some(instruction.on_failure),
                before: Vec::new(),
                pending: Vec::new(),
            };
            match step {
                HoleStep::Import => self.instruction(&instruction.kind)?,
                HoleStep::Open => {
                    let tir::InstructionKind::Assign { destination, .. } = instruction.kind else {
                        return malformed("a constructor-tail box that is no assignment");
                    };
                    let (block, hole) = self.open_hole()?;
                    self.hold(h.next, hole)?;
                    let p = self.place(destination)?;
                    let address = self.address(&p)?;
                    self.store(&p, address, block)?;
                }
            }
        }
        let p = self.place(site.value)?;
        let address = self.address(&p)?;
        let value = self.operand_at(UseKind::Take, &p, address, None)?;
        let hole = self.take_held(h.hole)?;
        self.push(Inst::HoleFill { hole, value });
        self.consumed(hole);
        self.consumed(value);
        let next = self.take_held(h.next)?;
        self.hold(h.hole, next)
    }

    /// A return: the result fills the last hole, and the root's payload, now
    /// complete, is the result. Copying it shares its parts with the root,
    /// whose release then frees the root's shell alone.
    pub(super) fn fill_return(&mut self, value: ValueId) -> R<ValueId> {
        let h = self.holes.expect("hole slots");
        let hole = self.take_held(h.hole)?;
        self.push(Inst::HoleFill { hole, value });
        self.consumed(hole);
        self.consumed(value);
        let root = self.whole(h.root);
        let mut payload = root.clone();
        payload.steps.push(Step::Payload);
        payload.projection.push(ProjectionStep::Payload);
        payload.ty = self.intern(h.payload);
        payload.fixed = false;
        payload.crosses = true;
        let address = self.address(&payload)?;
        self.operand_at(UseKind::Copy, &payload, address, None)
    }

    /// Whether a hole may be held here.
    pub(super) fn holes_open(&self) -> bool {
        self.holes.is_some_and(|h| {
            [h.next, h.hole]
                .iter()
                .any(|s| self.state.slots[s.index()].present().may())
        })
    }

    /// A failure while holes are open: abandon each. The hole of the
    /// structure's tip releases the root, by its blocks' headers, so no open
    /// payload is read; a site's new hole, whose block the chain consumed,
    /// only ends. Runs at the head of every failure edge of the instance.
    pub(super) fn abandon_holes(&mut self) -> R<()> {
        let Some(h) = self.holes else {
            return Ok(());
        };
        for slot in [h.next, h.hole] {
            if !self.state.slots[slot.index()].present().may() {
                continue;
            }
            let hole = self.take_held(slot)?;
            let root = if slot == h.hole {
                Some(self.take_held(h.root)?)
            } else {
                None
            };
            self.push(Inst::HoleAbandon { hole, root });
            self.consumed(hole);
            if let Some(root) = root {
                self.consumed(root);
            }
        }
        Ok(())
    }
}
