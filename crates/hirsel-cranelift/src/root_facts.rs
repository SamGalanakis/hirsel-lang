//! Private root companions carry only proved header uniqueness. The checked
//! ordinary library body still supplies every write and copy-on-write edge.
use crate::{function::FnEmitter, module::Emitted};
use cranelift_codegen::ir::{InstBuilder, Value, types};
use hirsel_lir::{ir::*, layout_types::LayoutKind};

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn root_facts_init(&mut self) -> Emitted {
        if !self.em.inputs.unit().alternate
            || self.em.inputs.unit().callbacks
            || !self.unit.tables.closures.is_empty()
            || self
                .em
                .inputs
                .unit()
                .ahead
                .and_then(|p| {
                    p.cpu_ordinal(self.unit.header.instance, self.em.inputs.unit().layouts)
                })
                .is_none()
        {
            return Ok(());
        }
        for (i, slot) in self.unit.slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Capture(_)) {
                continue;
            }
            let ty = &self.unit.types[slot.ty.index()];
            if ty.repr != ValueRepr::Block {
                continue;
            }
            let Some(record) = self.em.inputs.unit().layouts.layout(ty.layout) else {
                continue;
            };
            let LayoutKind::Block { payload } = record.kind else {
                continue;
            };
            let Some(payload) = self.em.inputs.unit().layouts.layout(payload) else {
                continue;
            };
            let LayoutKind::Sequence { element } = payload.kind else {
                continue;
            };
            if !hirsel_lir::ahead::scalar_image(self.em.inputs.unit().layouts, element) {
                continue;
            }
            let escaped = self.unit.tables.calls.iter().any(|call| {
                let private =
                    match call.callee {
                        Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
                            let symbol = &self.unit.tables.symbols[symbol.index()];
                            symbol.instance.is_some_and(|id| {
                                self.em.inputs.unit().ahead.is_some_and(|p| {
                                    p.has_private_edge(self.unit.header.instance, id)
                                })
                            }) && (symbol.definition.is_core("list.push")
                                || symbol.definition.is_core("list.reserve")
                                || symbol.definition.is_core("$native.list_push_spare"))
                        }
                        _ => false,
                    };
                !private
                    && call
                        .arguments
                        .get(&self.unit.pools.call_arguments)
                        .is_some_and(|args| {
                            args.iter().any(|arg| {
                                matches!(arg.value, ArgumentValue::Address { address, .. }
                        if self.whole_slot(address) == Some(SlotId(i as u32)))
                            })
                        })
            });
            if escaped {
                continue;
            }
            if matches!(slot.kind, SlotKind::Parameter(_))
                && !(slot.kind == SlotKind::Parameter(0) && self.sig.unique.is_some())
            {
                continue;
            }
            let var = self.b.declare_var(types::I8);
            let initial = if slot.kind == SlotKind::Parameter(0) {
                self.sig
                    .unique
                    .map(|(input, _)| self.entry_params[input])
                    .unwrap_or_else(|| self.b.ins().iconst(types::I8, 0))
            } else {
                self.b.ins().iconst(types::I8, 0)
            };
            self.b.def_var(var, initial);
            self.root_facts[i] = Some(var);
            self.root_facts_active = true;
        }
        for inst in &self.unit.instructions {
            if let Inst::Read {
                result, address, ..
            } = *inst
                && self
                    .whole_slot(address)
                    .and_then(|slot| self.root_facts[slot.index()])
                    .is_some()
            {
                let var = self.b.declare_var(types::I8);
                let no = self.b.ins().iconst(types::I8, 0);
                self.b.def_var(var, no);
                self.read_roots[result.index()] = Some(var);
            }
        }
        Ok(())
    }

    pub(crate) fn read_facts_invalidate(&mut self) {
        for var in self.read_roots.iter().flatten().copied() {
            let no = self.b.ins().iconst(types::I8, 0);
            self.b.def_var(var, no);
        }
    }

    pub(crate) fn root_facts_invalidate(&mut self) {
        self.read_facts_invalidate();
        for var in self.root_facts.iter().flatten().copied() {
            let no = self.b.ins().iconst(types::I8, 0);
            self.b.def_var(var, no);
        }
    }

    pub(crate) fn root_facts_before(&mut self, inst: &Inst) -> Emitted {
        if !self.root_facts_active {
            return Ok(());
        }
        let invalidates = match *inst {
            Inst::Dup { .. }
            | Inst::Copy { .. }
            | Inst::Share { .. }
            | Inst::RegionExit { .. }
            | Inst::Freeze { .. }
            | Inst::Thaw { .. }
            | Inst::Replace { .. } => true,
            Inst::UniqueCopy { value, .. } => {
                let layout = self.value_type(value)?.layout;
                let pointer_free = match self.layout(layout)?.kind {
                    LayoutKind::Block { payload } => match self.layout(payload)?.kind {
                        LayoutKind::Sequence { element } => {
                            hirsel_lir::ahead::scalar_image(self.em.inputs.unit().layouts, element)
                        }
                        _ => false,
                    },
                    _ => false,
                };
                !pointer_free
            }
            Inst::Release { layout, .. } | Inst::Reset { layout, .. } => {
                let record = self.layout(layout)?;
                record.finalizer.is_some() || !record.drop.is_empty()
            }
            _ => false,
        };
        if invalidates {
            self.root_facts_invalidate();
        }
        Ok(())
    }

    pub(crate) fn root_unique(&mut self, value: ValueId, block: Value, writable: bool) -> Value {
        let Some(var) = self.read_roots[value.index()] else {
            return if writable {
                self.is_writable(block)
            } else {
                self.is_unique(block)
            };
        };
        let known = self.b.use_var(var);
        let check = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I8);
        self.b.ins().brif(known, done, &[known.into()], check, &[]);
        self.b.switch_to_block(check);
        let tested = if writable {
            self.is_writable(block)
        } else {
            self.is_unique(block)
        };
        self.b.ins().jump(done, &[tested.into()]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    pub(crate) fn root_store(&mut self, address: AddressId) {
        if let Some(var) = self
            .whole_slot(address)
            .and_then(|slot| self.root_facts[slot.index()])
        {
            let unique = self.b.ins().iconst(types::I8, 0);
            self.b.def_var(var, unique);
        }
    }
}
