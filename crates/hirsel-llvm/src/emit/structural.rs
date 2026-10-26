use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn structural_bytes(&self, bytes: &[u8], align: u32) -> Value<'m> {
        let name = hirsel_lir::statics::constant_data_name(align, bytes);
        if let Some(global) = self.module.named_global(&name) {
            return global;
        }
        let ty = self
            .module
            .array_type(self.module.int_type(8), bytes.len() as u64);
        let global = self.module.private_constant(&name, ty, align);
        self.module
            .set_initializer(global, self.module.const_bytes(bytes));
        global
    }

    pub(super) fn needs_value_program(&self, layout: LayoutId) -> bool {
        self.unit
            .tables
            .structural
            .iter()
            .find(|p| p.root_layout() == layout)
            .is_some_and(|p| p.needs_value_program(self.inputs.layouts))
    }

    fn value_program(&self, layout: LayoutId) -> Emitted<(Value<'m>, u64)> {
        let plan = self
            .unit
            .tables
            .structural
            .iter()
            .find(|p| p.root_layout() == layout)
            .ok_or_else(|| CodegenError::Invalid {
                definition: self.unit.header.definition.clone(),
                what: "missing structural value program".into(),
            })?;
        let words = plan
            .program(self.inputs.layouts, &|_| Some(0))
            .map_err(|what| CodegenError::Invalid {
                definition: self.unit.header.definition.clone(),
                what,
            })?;
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        Ok((self.structural_bytes(&bytes, 8), words.len() as u64))
    }

    pub(super) fn structural_order_at(
        &mut self,
        layout: LayoutId,
        left: Value<'m>,
        right: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let (program, length) = self.value_program(layout)?;
        let entry = self.runtime(RuntimeEntry::StructuralOrder)?;
        Ok(self.builder.call(
            entry,
            &[self.context, program, self.const_u64(length), left, right],
        ))
    }

    pub(super) fn structural_hash_at(
        &mut self,
        layout: LayoutId,
        at: Value<'m>,
        seed: Value<'m>,
    ) -> Emitted<Value<'m>> {
        let (program, length) = self.value_program(layout)?;
        let entry = self.runtime(RuntimeEntry::StructuralHash)?;
        Ok(self.builder.call(
            entry,
            &[self.context, program, self.const_u64(length), at, seed],
        ))
    }

    pub(super) fn structural_preflight_at(
        &mut self,
        layout: LayoutId,
        at: Value<'m>,
        failure: FailureId,
    ) -> Emitted<()> {
        let Some(plan) = self
            .unit
            .tables
            .structural
            .iter()
            .find(|p| p.root_layout() == layout)
        else {
            return self.invalid("structural operand has no validated logical access plan");
        };
        if plan
            .graph
            .static_depth_bound()
            .is_some_and(|bound| bound <= hirsel_core_contract::depth::STRUCTURAL_LIMIT)
        {
            return Ok(());
        }
        // Descriptor indices are unused by the read-only access walker.
        let words = plan
            .program(self.inputs.layouts, &|_| Some(0))
            .map_err(|what| CodegenError::Invalid {
                definition: self.unit.header.definition.clone(),
                what,
            })?;
        let edges = plan.edges.clone();
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let program = self.structural_bytes(&bytes, 8);
        let edge_data = self.structural_bytes(&edges, 1);
        let entry = self.runtime(RuntimeEntry::DepthPreflight)?;
        let status = self.builder.call(
            entry,
            &[
                self.context,
                program,
                self.const_u64(words.len() as u64),
                edge_data,
                self.const_u64(edges.len() as u64),
                at,
            ],
        );
        self.status_guard(status, failure)
    }

    pub(super) fn structural_preflight_value(
        &mut self,
        layout: LayoutId,
        value: ValueId,
        failure: FailureId,
    ) -> Emitted<()> {
        if self
            .unit
            .tables
            .structural
            .iter()
            .find(|plan| plan.root_layout() == layout)
            .and_then(|plan| plan.graph.static_depth_bound())
            .is_some_and(|bound| bound <= hirsel_core_contract::depth::STRUCTURAL_LIMIT)
        {
            return Ok(());
        }
        let at = self.stack(layout)?;
        let operand = self.counted_view(value)?;
        self.store(self.layout_shape(layout)?, operand, at);
        self.structural_preflight_at(layout, at.ptr, failure)
    }
}
