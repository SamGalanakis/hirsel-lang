use crate::function::FnEmitter;
use crate::module::{Emitted, PTR};
use cranelift_codegen::ir::{AbiParam, InstBuilder, Type, types};
use cranelift_module::{FuncId, Module};
use hirsel_lir::emission::site_code;
use hirsel_lir::ir::{BlockId, CallId};
use hirsel_lir::ownership_trace::{
    Moment, Point,
    native::{NativeLocation, NativeMarker},
};
use hirsel_lir::stage::ObjectSymbolKind;

impl FnEmitter<'_, '_, '_> {
    fn ownership_function(
        &mut self,
        name: &str,
        result: Option<Type>,
        parameters: &[Type],
    ) -> Emitted<FuncId> {
        let mut signature = self.em.module.make_signature();
        for &ty in parameters {
            let parameter = AbiParam::new(ty);
            signature.params.push(if ty == types::I32 {
                parameter.uext()
            } else {
                parameter
            });
        }
        if let Some(ty) = result {
            signature.returns.push(AbiParam::new(ty));
        }
        self.em
            .manifested
            .import(name, ObjectSymbolKind::Runtime, None);
        self.em.import_function(name, &signature)
    }

    pub(crate) fn ownership_init(&mut self) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let unit = self.em.constant_data(1, &plan.plan().unit.0)?;
        let pointer = self.data_addr(unit, 0);
        let enter = self.ownership_function(
            "hirsel_rt_v2_ownership_enter",
            Some(types::I64),
            &[PTR, types::I32, PTR],
        )?;
        let enter = self.func_ref(enter);
        let version = self.b.ins().iconst(
            types::I32,
            i64::from(hirsel_lir::ownership_trace::native::ABI_VERSION),
        );
        let call = self.b.ins().call(enter, &[self.context, version, pointer]);
        self.ownership_frame = Some(self.b.inst_results(call)[0]);
        self.ownership_point(Point::Entry)
    }

    pub(crate) fn ownership_events(
        &mut self,
        block: BlockId,
        index: u32,
        moment: Moment,
    ) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let markers: Vec<_> = plan.at(block, index, moment).collect();
        self.ownership_markers(markers)
    }
    pub(crate) fn ownership_empty_drop(&mut self, block: BlockId) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let markers = plan.at_empty_drop(block).collect();
        self.ownership_markers(markers)
    }
    pub(crate) fn ownership_point(&mut self, point: Point) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let markers = plan.at_point(point).collect();
        self.ownership_markers(markers)
    }
    pub(crate) fn ownership_call(&mut self, call: CallId, returned: bool) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let markers = plan
            .at_call(self.unit, call, returned)
            .or_else(|error| self.invalid(error))?;
        self.ownership_markers(markers)
    }
    fn ownership_markers(&mut self, markers: Vec<NativeMarker>) -> Emitted {
        if markers.is_empty() {
            return Ok(());
        }
        let Some(frame) = self.ownership_frame else {
            return self.invalid("ownership frame was not initialized");
        };
        let event = self.ownership_function(
            "hirsel_rt_v2_ownership_event",
            None,
            &[
                PTR,
                types::I64,
                types::I32,
                types::I32,
                types::I64,
                types::I64,
                types::I64,
                types::I32,
            ],
        )?;
        let event = self.func_ref(event);
        let location =
            |at: NativeLocation| ((u64::from(at.kind) << 32) | u64::from(at.index)) as i64;
        for marker in markers {
            if marker.kind == 4 {
                if self.ownership_cleanup.replace(marker).is_some() {
                    return self.invalid("overlapping ownership cleanup markers");
                }
                continue;
            }
            let site = site_code(
                self.definition(),
                &self.unit.header.implementation_revision,
                marker.site,
            );
            let id = self.b.ins().iconst(types::I32, i64::from(marker.id.0));
            let kind = self.b.ins().iconst(types::I32, i64::from(marker.kind));
            let source = self.b.ins().iconst(types::I64, location(marker.token));
            let owner = self.b.ins().iconst(types::I64, location(marker.owner));
            let site = self.b.ins().iconst(types::I64, site as i64);
            let layout = self.b.ins().iconst(types::I32, i64::from(marker.layout.0));
            if marker.kind == 3 {
                let alias = self.ownership_function(
                    "hirsel_rt_v2_ownership_alias",
                    None,
                    &[
                        PTR,
                        types::I64,
                        types::I32,
                        types::I64,
                        types::I64,
                        types::I64,
                        types::I32,
                    ],
                )?;
                let alias = self.func_ref(alias);
                self.b.ins().call(
                    alias,
                    &[self.context, frame, id, source, owner, site, layout],
                );
            } else {
                self.b.ins().call(
                    event,
                    &[self.context, frame, id, kind, source, owner, site, layout],
                );
            }
        }
        Ok(())
    }

    pub(crate) fn ownership_cleanup_begin(
        &mut self,
        value: hirsel_lir::ir::ValueId,
    ) -> Emitted<Option<cranelift_codegen::ir::Value>> {
        let Some(marker) = self.ownership_cleanup.take() else {
            return Ok(None);
        };
        let plan = self.ownership.ok_or_else(|| crate::CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "ownership cleanup has no plan".into(),
        })?;
        let (released, fields) = plan
            .cleanup_captures(self.unit, self.em.inputs.layouts, marker.id)
            .or_else(|error| self.invalid(error))?;
        if released != value {
            return self.invalid("ownership cleanup names another physical release");
        }
        let crate::function::V::Pair(_, environment) = self.get(value)? else {
            return self.invalid("ownership cleanup has no callable pair");
        };
        let (scope, _) = self.stack_bytes(48, 8);
        let (bindings, _) = self.stack_bytes(fields.len().max(1) as u64 * 40, 8);
        for (i, field) in fields.iter().enumerate() {
            let descriptor = self.descriptor_value(field.layout)?;
            let descriptor = self.b.ins().uextend(types::I64, descriptor);
            let offset = self.u64c(field.offset);
            let ordinal = self.u64c(u64::from(field.field));
            let layout = self.u64c(u64::from(field.layout.0));
            let declaration = self.u64c(u64::from(field.path[0]));
            for (j, word) in [offset, descriptor, ordinal, layout, declaration]
                .into_iter()
                .enumerate()
            {
                self.store_at(word, bindings, (i * 5 + j) as u64 * 8);
            }
        }
        let begin = self.ownership_function(
            "hirsel_rt_v2_ownership_cleanup_begin",
            None,
            &[PTR, PTR, types::I64, types::I32, PTR, PTR, types::I64],
        )?;
        let begin = self.func_ref(begin);
        let frame = self
            .ownership_frame
            .ok_or_else(|| crate::CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "ownership cleanup has no frame".into(),
            })?;
        let id = self.b.ins().iconst(types::I32, i64::from(marker.id.0));
        let count = self.u64c(fields.len() as u64);
        self.b.ins().call(
            begin,
            &[self.context, scope, frame, id, environment, bindings, count],
        );
        Ok(Some(scope))
    }
    pub(crate) fn ownership_cleanup_end(
        &mut self,
        scope: Option<cranelift_codegen::ir::Value>,
    ) -> Emitted {
        if let Some(scope) = scope {
            let end =
                self.ownership_function("hirsel_rt_v2_ownership_cleanup_end", None, &[PTR, PTR])?;
            let end = self.func_ref(end);
            self.b.ins().call(end, &[self.context, scope]);
        }
        Ok(())
    }

    pub(crate) fn ownership_leave(&mut self) -> Emitted {
        let Some(frame) = self.ownership_frame else {
            return Ok(());
        };
        let leave =
            self.ownership_function("hirsel_rt_v2_ownership_leave", None, &[PTR, types::I64])?;
        let leave = self.func_ref(leave);
        self.b.ins().call(leave, &[self.context, frame]);
        Ok(())
    }
}
