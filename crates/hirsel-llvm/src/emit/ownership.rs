use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// Adds one credit to every counted holder of the value at `place` (a semantic Copy by
    /// the `Dup` and `Fields` recipes), reading its layout's scan program.
    pub(super) fn dup_holders(&mut self, place: Place<'m>) -> Emitted {
        let place = self.gather(place)?;
        self.invalidate_uniqueness();
        let record = self.layout(place.layout)?;
        if let LayoutKind::PayloadReceipt { .. } = record.kind {
            let receipt = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            let retain = self.runtime(RuntimeEntry::ReceiptRetain)?;
            self.builder.call(retain, &[receipt]);
            return Ok(());
        }
        if let LayoutKind::Block { .. } | LayoutKind::Box { .. } = record.kind {
            let block = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.dup_block(block);
        }
        if let LayoutKind::Shared { .. } = record.kind {
            let block = self.load(Shape::Scalar(ScalarKind::Pointer), place);
            return self.root_dup_block(block);
        }
        self.dup_scan(place, &record.scan)
    }

    /// Metadata holders have their own count and never use a language block or root header.
    pub(super) fn is_receipt_value(&self, value: ValueId) -> Emitted<bool> {
        let ty = self.value_type(value)?;
        let receipt = matches!(
            self.layout(ty.layout)?.kind,
            LayoutKind::PayloadReceipt { .. }
        );
        if receipt != (ty.repr == ValueRepr::PayloadReceipt) {
            return self.invalid("a receipt layout and representation disagree");
        }
        Ok(receipt)
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
                ScanStep::Root { offset } => {
                    let at = self.builder.offset(place.ptr, self.const_u64(*offset));
                    let align = place.align.min(low_bit(*offset)).min(8);
                    let block = self.builder.load(ptr_ty, at, align);
                    self.root_dup_block(block)?;
                }
                ScanStep::Uncounted { .. } | ScanStep::Handle { .. } => {}
                ScanStep::Inline { offset, layout } => {
                    let child = self.field_place(place, *offset, *layout, false)?;
                    self.dup_holders(child)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    for i in 0..*count {
                        let child =
                            self.field_place(place, offset + i * stride, *element, false)?;
                        self.dup_holders(child)?;
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
        let ownership_cleanup = self.ownership_cleanup_begin(value)?;
        if self.resume.is_none() && ownership_cleanup.is_none()
            && live == MaskRef::All
            && glue::covered(self.inputs.layouts, layout)
        {
            // Plain inline values need no finalization frame. In particular, erased
            // slice owners decrement shared counts before their last-credit fallback.
            // Programs with finalizers or partial masks still use the runtime below.
            let record = self.layout(layout)?;
            let scope = glue::Scope::of(layout);
            let code = self.const_u64(self.cleanup_code(anchor)?);
            let status = self.drop_glue(code, |walk| {
                walk.steps(&scope, place.ptr, place.align, &record.drop)
            })?;
            return self.status_guard(status, failure);
        }
        let descriptor = self.descriptor(layout)?;
        let mask = self.live_mask(live)?;
        let code = self.cleanup_code(anchor)?;
        let (anchor, operation_site) = self.failure_sites(code);
        let continuation = self
            .prologue
            .alloca(self.module.array_type(self.module.int_type(64), 4), 8);
        let finalize = self.runtime(RuntimeEntry::Finalize)?;
        let status = self.builder.call(
            finalize,
            &[
                self.context,
                descriptor,
                place.ptr,
                mask,
                continuation,
                anchor,
                operation_site,
            ],
        );
        self.ownership_cleanup_end(ownership_cleanup)?;
        if self.resume.is_some() { self.resume_propagate(status)?; }
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
        let (anchor, operation_site) = self.failure_site_values(anchor);
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
            root_release: self.runtime(RuntimeEntry::RootDrop)?,
            measure_release,
            tracking: self.tracking(),
        };
        let i8_ty = self.module.int_type(8);
        let status = match self.drop_status {
            Some(status) => status,
            None => {
                let status = self.prologue.alloca(i8_ty, 1);
                self.drop_status = Some(status);
                status
            }
        };
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
            operation_site,
            budget: self.const_u64(glue::GLUE_MAX_DEPTH),
            status,
            pending: &mut pending,
            looping: None,
        });
        let defined =
            emitted.and_then(|()| glue::define(self.module, self.inputs.layouts, runtime, pending));
        if let Err(glue::GlueError(what)) = defined {
            return self.invalid(what);
        }
        // Walk declares this only when a reached drop step needs it. Keep empty-demand
        // modules free of receipt imports, including ordinary compiled drop glue.
        for entry in [RuntimeEntry::ReceiptRelease, RuntimeEntry::SealedRelease] {
            if self.module.declares(entry.declaration().symbol) {
                self.runtime(entry)?;
            }
        }
        Ok(self.builder.load(i8_ty, status, 1))
    }
}

#[cfg(test)]
mod receipt_tests;
