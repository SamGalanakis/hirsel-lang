use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// Adds one credit to every counted holder of the value at `place` (a semantic Copy by
    /// the `Dup` and `Fields` recipes), reading its layout's scan program.
    pub(super) fn dup_holders(&mut self, place: Place<'m>) -> Emitted {
        let place = self.gather(place)?;
        self.invalidate_uniqueness();
        let record = self.layout(place.layout)?;
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = record.kind {
            let block = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.dup_block(block);
        }
        self.dup_scan(place, &record.scan)
    }

    pub(super) fn dup_scan(&mut self, place: Place<'m>, scan: &'a [ScanStep]) -> Emitted {
        let ptr_ty = self.module.ptr_type();
        for step in scan {
            match step {
                ScanStep::Counted { offset }
                | ScanStep::SliceOwner { offset }
                | ScanStep::Environment { offset } => {
                    let at = self.builder.offset(place.ptr, self.const_u64(*offset));
                    let align = place.align.min(low_bit(*offset)).min(8);
                    let block = self.builder.load(ptr_ty, at, align);
                    self.dup_block(block)?;
                }
                ScanStep::Uncounted { .. } | ScanStep::Handle { .. } => {}
                ScanStep::Inline { offset, layout } => {
                    let child = self.field_place(place, *offset, *layout, false)?;
                    let scan = &self.layout(*layout)?.scan;
                    self.dup_scan(child, scan)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let scan = &self.layout(*element)?.scan;
                    for i in 0..*count {
                        let child =
                            self.field_place(place, offset + i * stride, *element, false)?;
                        self.dup_scan(child, scan)?;
                    }
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = self.tag_at(place, *tag_offset, *form, false);
                    let done = self.module.append_block(self.function);
                    let mut cases = Vec::new();
                    let here = self.builder.current();
                    for (variant, steps) in variants {
                        let arm = self.module.append_block(self.function);
                        self.builder.position(arm);
                        self.dup_scan(place, steps)?;
                        self.builder.br(done);
                        cases.push((
                            self.module
                                .const_int(self.module.int_type(32), u128::from(*variant)),
                            arm,
                        ));
                    }
                    self.builder.position(here);
                    self.builder.switch(active, done, &cases);
                    self.builder.position(done);
                }
                ScanStep::Sequence { .. } | ScanStep::Columns(_) => {
                    return self.unsupported("a copy of an inline sequence");
                }
            }
        }
        Ok(())
    }

    /// `Release` of an owning inline value: its drop program walked in drop
    /// order with the counts decremented, each counted leaf released through the runtime
    /// with the descriptor its own header names. A failing leaf does not stop the walk; the
    /// failure edge is taken after it, as a block release accumulates. `live` gates
    /// top-level fields.
    pub(super) fn release_inline(
        &mut self,
        value: ValueId,
        layout: LayoutId,
        live: MaskRef,
        anchor: CleanupAnchorId,
        failure: FailureId,
    ) -> Emitted {
        let ty = self.value_type(value)?;
        if ty.layout != layout
            || !matches!(
                ty.repr,
                ValueRepr::Aggregate | ValueRepr::Callable | ValueRepr::Handle
            )
        {
            return self.invalid("an inline release at another layout or representation");
        }
        let place = self.stack(layout)?;
        self.store(self.value_shape(value)?, self.get(value)?, place);
        let record = self.layout(layout)?;
        // Byte ranges of the top-level fields the mask marks dead.
        let dead: Vec<(u64, u64)> = match live {
            MaskRef::All => Vec::new(),
            MaskRef::Static(mask) => {
                let Some(mask) = self.unit.tables.masks.get(mask.index()) else {
                    return self.invalid("a mask outside the mask table");
                };
                let words = self.pool(mask.words, &self.unit.pools.mask_words)?;
                if record.tag.is_some() {
                    return self.unsupported("a live mask on an inline variant release");
                }
                let mut dead = Vec::new();
                for field in &record.fields {
                    let i = field.ordinal as usize;
                    let set = words.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1);
                    if !set {
                        let size = self.fixed_size(field.layout)?;
                        dead.push((field.offset, field.offset + size));
                    }
                }
                dead
            }
            MaskRef::Dynamic(_) => {
                return self.unsupported("a dynamic live mask on an inline release");
            }
        };
        let anchor = self.const_u64(self.cleanup_code(anchor)?);
        let steps: Vec<&'a DropStep> = record
            .drop
            .iter()
            .filter(|step| {
                let offset = match step {
                    DropStep::Release { offset }
                    | DropStep::Environment { offset }
                    | DropStep::CloseHandle { offset }
                    | DropStep::Child { offset, .. } => Some(*offset),
                    DropStep::RepeatReverse { offset, .. } => Some(*offset),
                    _ => None,
                };
                offset.is_none_or(|o| !dead.iter().any(|&(a, b)| a <= o && o < b))
            })
            .collect();
        let scope = glue::Scope::of(layout);
        let status = self.drop_glue(anchor, |walk| {
            steps
                .into_iter()
                .try_for_each(|step| walk.step(&scope, place.ptr, place.align, step))
        })?;
        self.status_guard(status, failure)
    }

    /// Emits drop code through `emit` at the builder's position, then the body of every
    /// release function it called. Returns the accumulated `i8` status: a failing leaf does
    /// not stop a drop.
    pub(super) fn drop_glue(
        &mut self,
        anchor: Value<'m>,
        emit: impl FnOnce(&mut glue::Walk<'m, '_>) -> Result<(), glue::GlueError>,
    ) -> Emitted<Value<'m>> {
        let measure_release = if self.inputs.measurement {
            let name = "hirsel_rt_v2_measure_release";
            let ptr = self.module.ptr_type();
            let ty = self
                .module
                .function_type(self.module.void_type(), &[ptr, ptr]);
            let function =
                self.module
                    .function(name, ty)
                    .map_err(|what| CodegenError::Invalid {
                        definition: self.unit.header.definition.clone(),
                        what,
                    })?;
            self.manifested
                .imports
                .entry(name.to_owned())
                .or_insert_with(|| ObjectSymbol {
                    name: name.to_owned(),
                    kind: ObjectSymbolKind::Runtime,
                    definition: None,
                    abi: None,
                });
            Some(function)
        } else {
            None
        };
        let runtime = glue::Runtime {
            release: self.runtime(RuntimeEntry::Release)?,
            free: self.runtime(RuntimeEntry::Free)?,
            close_handle: self.runtime(RuntimeEntry::CloseHandle)?,
            measure_release,
            tracking: self.tracking(),
        };
        let i8_ty = self.module.int_type(8);
        let status = self.prologue.alloca(i8_ty, 1);
        self.builder
            .store(self.module.const_int(i8_ty, 0), status, 1);
        let mut pending = Vec::new();
        let emitted = emit(&mut glue::Walk {
            module: self.module,
            layouts: self.inputs.layouts,
            runtime,
            function: self.function,
            builder: &self.builder,
            context: self.context,
            anchor,
            budget: self.const_u64(glue::GLUE_MAX_DEPTH),
            status,
            pending: &mut pending,
        });
        let defined =
            emitted.and_then(|()| glue::define(self.module, self.inputs.layouts, runtime, pending));
        if let Err(glue::GlueError(what)) = defined {
            return self.invalid(what);
        }
        Ok(self.builder.load(i8_ty, status, 1))
    }
}
