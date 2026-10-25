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
            self.b.ins().call(
                event,
                &[self.context, frame, id, kind, source, owner, site, layout],
            );
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
