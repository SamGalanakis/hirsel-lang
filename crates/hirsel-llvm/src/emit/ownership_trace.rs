use super::*;
use hirsel_lir::ownership_trace::{
    Moment, Point,
    native::{NativeLocation, NativeMarker},
};

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn ownership_function(
        &mut self,
        name: &str,
        result: Type<'m>,
        parameters: &[Type<'m>],
    ) -> Emitted<Function<'m>> {
        let function = self
            .module
            .function(name, self.module.function_type(result, parameters))
            .or_else(|what| self.invalid(what))?;
        self.manifested
            .imports
            .entry(name.into())
            .or_insert(ObjectSymbol {
                name: name.into(),
                kind: ObjectSymbolKind::Runtime,
                definition: None,
                abi: None,
            });
        Ok(function)
    }

    pub(super) fn ownership_init(&mut self) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let hash = plan.plan().unit;
        let name = format!(
            "hirsel.ownership.unit.{}",
            hash.0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        let bytes = self.module.const_bytes(&hash.0);
        let global = self.module.private_constant(&name, bytes.ty(), 1);
        self.module.set_initializer(global, bytes);
        let ptr = self.module.ptr_type();
        let enter = self.ownership_function(
            "hirsel_rt_v2_ownership_enter",
            self.module.int_type(64),
            &[ptr, self.module.int_type(32), ptr],
        )?;
        self.builder.position(self.prologue_block);
        let version = self.module.const_int(
            self.module.int_type(32),
            u128::from(hirsel_lir::ownership_trace::native::ABI_VERSION),
        );
        self.ownership_frame = Some(self.builder.call(enter, &[self.context, version, global]));
        self.ownership_point(Point::Entry)
    }

    pub(super) fn ownership_events(
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
    pub(super) fn ownership_point(&mut self, point: Point) -> Emitted {
        let Some(plan) = self.ownership else {
            return Ok(());
        };
        let markers = plan.at_point(point).collect();
        self.ownership_markers(markers)
    }
    pub(super) fn ownership_call(&mut self, call: CallId, returned: bool) -> Emitted {
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
        let frame = self.ownership_frame.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "ownership frame was not initialized".into(),
        })?;
        let (ptr, i32, i64) = (
            self.module.ptr_type(),
            self.module.int_type(32),
            self.module.int_type(64),
        );
        let event = self.ownership_function(
            "hirsel_rt_v2_ownership_event",
            self.module.void_type(),
            &[ptr, i64, i32, i32, i64, i64, i64, i32],
        )?;
        let location = |at: NativeLocation| (u64::from(at.kind) << 32) | u64::from(at.index);
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
            if marker.kind == 3 {
                let alias = self.ownership_function(
                    "hirsel_rt_v2_ownership_alias",
                    self.module.void_type(),
                    &[ptr, i64, i32, i64, i64, i64, i32],
                )?;
                self.builder.call(
                    alias,
                    &[
                        self.context,
                        frame,
                        self.module.const_int(i32, u128::from(marker.id.0)),
                        self.const_u64(location(marker.token)),
                        self.const_u64(location(marker.owner)),
                        self.const_u64(site),
                        self.module.const_int(i32, u128::from(marker.layout.0)),
                    ],
                );
                continue;
            }
            self.builder.call(
                event,
                &[
                    self.context,
                    frame,
                    self.module.const_int(i32, u128::from(marker.id.0)),
                    self.module.const_int(i32, u128::from(marker.kind)),
                    self.const_u64(location(marker.token)),
                    self.const_u64(location(marker.owner)),
                    self.const_u64(site),
                    self.module.const_int(i32, u128::from(marker.layout.0)),
                ],
            );
        }
        Ok(())
    }

    pub(super) fn ownership_cleanup_begin(&mut self, value: ValueId) -> Emitted<Option<Value<'m>>> {
        let Some(marker) = self.ownership_cleanup.take() else {
            return Ok(None);
        };
        let plan = self.ownership.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "ownership cleanup has no plan".into(),
        })?;
        let (released, fields) = plan
            .cleanup_captures(self.unit, self.inputs.layouts, marker.id)
            .or_else(|error| self.invalid(error))?;
        if released != value {
            return self.invalid("ownership cleanup names another physical release");
        }
        let (_, environment) = self.callable_parts(value)?;
        let i64 = self.module.int_type(64);
        let scope = self.prologue.alloca(self.module.array_type(i64, 6), 8);
        let bindings = self.prologue.alloca(
            self.module.array_type(i64, fields.len().max(1) as u64 * 4),
            8,
        );
        for (i, field) in fields.iter().enumerate() {
            let descriptor = self.descriptor(field.layout)?;
            let descriptor = self.builder.cast(Cast::ZExt, descriptor, i64);
            let words = [
                self.const_u64(field.offset),
                descriptor,
                self.const_u64(u64::from(field.field)),
                self.const_u64(u64::from(field.layout.0)),
            ];
            for (j, word) in words.into_iter().enumerate() {
                self.builder.store(
                    word,
                    self.builder
                        .offset(bindings, self.const_u64((i * 4 + j) as u64 * 8)),
                    8,
                );
            }
        }
        let ptr = self.module.ptr_type();
        let begin = self.ownership_function(
            "hirsel_rt_v2_ownership_cleanup_begin",
            self.module.void_type(),
            &[ptr, ptr, i64, self.module.int_type(32), ptr, ptr, i64],
        )?;
        let frame = self.ownership_frame.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "ownership cleanup has no frame".into(),
        })?;
        self.builder.call(
            begin,
            &[
                self.context,
                scope,
                frame,
                self.module
                    .const_int(self.module.int_type(32), u128::from(marker.id.0)),
                environment,
                bindings,
                self.const_u64(fields.len() as u64),
            ],
        );
        Ok(Some(scope))
    }
    pub(super) fn ownership_cleanup_end(&mut self, scope: Option<Value<'m>>) -> Emitted {
        if let Some(scope) = scope {
            let ptr = self.module.ptr_type();
            let end = self.ownership_function(
                "hirsel_rt_v2_ownership_cleanup_end",
                self.module.void_type(),
                &[ptr, ptr],
            )?;
            self.builder.call(end, &[self.context, scope]);
        }
        Ok(())
    }

    pub(super) fn ownership_leave(&mut self) -> Emitted {
        let Some(frame) = self.ownership_frame else {
            return Ok(());
        };
        let leave = self.ownership_function(
            "hirsel_rt_v2_ownership_leave",
            self.module.void_type(),
            &[self.module.ptr_type(), self.module.int_type(64)],
        )?;
        self.builder.call(leave, &[self.context, frame]);
        Ok(())
    }
}
