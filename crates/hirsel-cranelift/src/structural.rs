use crate::function::FnEmitter;
use crate::module::Emitted;
use cranelift_codegen::ir::Value;
use hirsel_lir::ir::{FailureId, ValueId};
use hirsel_lir::layout_types::LayoutId;
use hirsel_lir::runtime_abi::RuntimeEntry;

impl FnEmitter<'_, '_, '_> {
    pub(crate) fn needs_value_program(&self, layout: LayoutId) -> bool {
        self.unit
            .tables
            .structural
            .iter()
            .find(|p| p.root_layout() == layout)
            .is_some_and(|p| p.needs_value_program(self.em.inputs.layouts))
    }

    fn value_program(&mut self, layout: LayoutId) -> Emitted<(Value, Value)> {
        let Some(plan) = self
            .unit
            .tables
            .structural
            .iter()
            .find(|p| p.root_layout() == layout)
        else {
            return self.invalid("missing structural value program");
        };
        let words = plan
            .program(self.em.inputs.layouts, &|_| Some(0))
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let program = self.em.constant_data(8, &bytes)?;
        let program = self.data_addr(program, 0);
        let length = self.u64c(words.len() as u64);
        Ok((program, length))
    }

    pub(crate) fn structural_order_at(
        &mut self,
        layout: LayoutId,
        left: Value,
        right: Value,
    ) -> Emitted<Value> {
        let (program, length) = self.value_program(layout)?;
        let context = self.context;
        self.rt1(
            RuntimeEntry::StructuralOrder,
            &[context, program, length, left, right],
        )
    }

    pub(crate) fn structural_hash_at(
        &mut self,
        layout: LayoutId,
        at: Value,
        seed: Value,
    ) -> Emitted<Value> {
        let (program, length) = self.value_program(layout)?;
        let context = self.context;
        self.rt1(
            RuntimeEntry::StructuralHash,
            &[context, program, length, at, seed],
        )
    }

    pub(crate) fn structural_preflight_at(
        &mut self,
        layout: LayoutId,
        at: Value,
        failure: FailureId,
    ) -> Emitted {
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
        let words = plan
            .program(self.em.inputs.layouts, &|_| Some(0))
            .map_err(|what| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what,
            })?;
        let edges = plan.edges.clone();
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let program = self.em.constant_data(8, &bytes)?;
        let edge_data = self.em.constant_data(1, &edges)?;
        let program = self.data_addr(program, 0);
        let edge_data = self.data_addr(edge_data, 0);
        let program_len = self.u64c(words.len() as u64);
        let edge_count = self.u64c(edges.len() as u64);
        let context = self.context;
        let status = self.rt1(
            RuntimeEntry::DepthPreflight,
            &[context, program, program_len, edge_data, edge_count, at],
        )?;
        self.status_guard(status, failure)
    }

    pub(crate) fn structural_preflight_value(
        &mut self,
        layout: LayoutId,
        value: ValueId,
        failure: FailureId,
    ) -> Emitted {
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
        self.store(self.layout_shape(layout)?, operand, at)?;
        self.structural_preflight_at(layout, at.ptr, failure)
    }
}
