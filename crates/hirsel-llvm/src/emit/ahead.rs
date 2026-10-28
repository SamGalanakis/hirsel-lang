use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn ahead_helper(&mut self, handoff: bool) -> Emitted<Function<'m>> {
        self.runtime(if handoff {
            RuntimeEntry::AheadHandoff
        } else {
            RuntimeEntry::AheadState
        })
    }
    pub(super) fn ahead_init(&mut self) -> Emitted {
        if !self.inputs.alternate {
            return Ok(());
        }
        self.builder.position(self.prologue_block);
        let getter = self.ahead_helper(false)?;
        let state = self.builder.call(getter, &[self.context]);
        let counter = self.prologue.alloca(self.module.int_type(64), 8);
        self.ahead_state = Some(state);
        self.ahead_counter = Some(counter);
        self.ahead_reload();
        Ok(())
    }
    pub(super) fn ahead_flush(&self) {
        if let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) {
            let value = self.builder.load(self.module.int_type(64), counter, 8);
            self.builder.store(value, state, 8);
        }
    }
    pub(super) fn ahead_reload(&self) {
        if let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) {
            let value = self.builder.load(self.module.int_type(64), state, 8);
            self.builder.store(value, counter, 8);
        }
    }
    pub(super) fn ahead_barrier(&mut self) -> Emitted {
        self.ahead_barrier_reason(4)
    }
    pub(super) fn ahead_failure_barrier(&mut self) -> Emitted {
        self.ahead_barrier_reason(hirsel_lir::runtime_abi::AHEAD_FAILURE_BARRIER)
    }
    fn ahead_barrier_reason(&mut self, reason: u32) -> Emitted {
        let Some(state) = self.ahead_state else {
            return Ok(());
        };
        self.ahead_flush();
        let helper = self.ahead_helper(true)?;
        self.builder.call(
            helper,
            &[
                state,
                self.module
                    .const_int(self.module.int_type(32), reason.into()),
            ],
        );
        self.ahead_reload();
        Ok(())
    }
    pub(super) fn ahead_point(&mut self, logical: bool) -> Emitted {
        let (Some(state), Some(counter)) = (self.ahead_state, self.ahead_counter) else {
            return Ok(());
        };
        let i64t = self.module.int_type(64);
        let i32t = self.module.int_type(32);
        let left = self.builder.binary(
            BinOp::Sub,
            self.builder.load(i64t, counter, 8),
            self.module.const_int(i64t, 1),
        );
        self.builder.store(left, counter, 8);
        // Initialization/refill supply 1024; Saved adoption supplies i64::MAX.
        // Each point hands off at zero and reloads a positive word before the
        // next subtraction, so expiry cannot be skipped or wrap through zero.
        let expired = self
            .builder
            .icmp(IntPredicate::Eq, left, self.module.const_zero(i64t));
        let mut reason = self.builder.select(
            expired,
            self.module.const_int(i32t, 1),
            self.module.const_zero(i32t),
        );
        if logical && self.inputs.fuel {
            let at = self.builder.offset(
                self.context,
                self.module
                    .const_int(i64t, hirsel_lir::runtime_abi::CTX_FUEL_OFFSET.into()),
            );
            let fuel = self.builder.binary(
                BinOp::Sub,
                self.builder.load(i64t, at, 8),
                self.module.const_int(i64t, 1),
            );
            self.builder.store(fuel, at, 8);
            let spent = self
                .builder
                .icmp(IntPredicate::Slt, fuel, self.module.const_zero(i64t));
            reason = self.builder.binary(
                BinOp::Or,
                reason,
                self.builder.select(
                    spent,
                    self.module.const_int(i32t, 2),
                    self.module.const_zero(i32t),
                ),
            );
        }
        let cold = self.module.append_block(self.function);
        let next = self.module.append_block(self.function);
        let spent = self
            .builder
            .icmp(IntPredicate::Ne, reason, self.module.const_zero(i32t));
        self.builder.cold_cond_br(spent, cold, next);
        self.builder.position(cold);
        self.ahead_flush();
        let helper = self.ahead_helper(true)?;
        self.builder.call(helper, &[state, reason]);
        self.ahead_reload();
        self.builder.br(next);
        self.builder.position(next);
        Ok(())
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn host_read_boundary(&mut self, call: CallId, enter: bool) -> Emitted {
        let Some(state) = self.ahead_state else {
            return Ok(());
        };
        if self
            .inputs
            .ahead
            .is_some_and(|plan| plan.cpu_claim(self.unit.header.instance, call))
        {
            let helper = self.runtime(if enter {
                RuntimeEntry::CpuAheadEnter
            } else {
                RuntimeEntry::CpuAheadArm
            })?;
            self.builder
                .call(helper, &[if enter { state } else { self.context }]);
            if enter {
                self.ahead_reload();
            }
            return Ok(());
        }
        let Some((buffer, name)) = self.inputs.ahead.and_then(|p| {
            p.host_read(self.unit.header.instance, call).map(|r| {
                (
                    r.buffer,
                    hirsel_lir::ahead::read_metadata_name(
                        self.unit.header.instance,
                        call,
                        p.identity,
                    ),
                )
            })
        }) else {
            return Ok(());
        };
        let recipe = self.glue_cell(&name, self.module.int_type(8));
        let helper = self.runtime(if enter {
            RuntimeEntry::HostEnter
        } else {
            RuntimeEntry::HostArm
        })?;
        if enter {
            self.builder.call(helper, &[state, recipe]);
            self.ahead_reload();
        } else {
            let place = self.slots[buffer.index()].ok_or_else(|| CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "missing selected buffer slot".into(),
            })?;
            self.builder
                .call(helper, &[self.context, recipe, place.ptr]);
        }
        Ok(())
    }

    /// Retain the selected inert buffer without a count mutation or runtime
    /// allocator on the private stack. Its old fuel position is journaled on idle.
    pub(super) fn host_instruction(&mut self, inst: &Inst) -> Emitted<bool> {
        let Some(state) = self.ahead_state else {
            return Ok(false);
        };
        let i = self.unit.blocks[self.at.0.index()].instructions.start as usize + self.at.1;
        let Some(spine) = self
            .inputs
            .ahead
            .and_then(|p| p.host_spine(self.unit.header.instance))
        else {
            return Ok(false);
        };
        if spine.barriers.contains(&i) {
            self.ahead_barrier()?;
        }
        let Some(&slot) = spine.deferred_releases.get(&i) else {
            return Ok(false);
        };
        let Inst::Release { value, .. } = *inst else {
            return self.invalid("deferred cleanup is not a release");
        };
        let active = self.runtime(RuntimeEntry::HostActive)?;
        let active = self.builder.call(active, &[state]);
        let active = self.builder.icmp(
            IntPredicate::Ne,
            active,
            self.module.const_zero(self.module.int_type(8)),
        );
        let held = self.module.append_block(self.function);
        let ordinary = self.module.append_block(self.function);
        let next = self.module.append_block(self.function);
        self.builder.cond_br(active, held, ordinary);
        self.builder.position(held);
        let block = self.get(value)?;
        let at = self.builder.offset(
            self.context,
            self.module.const_int(
                self.module.int_type(64),
                hirsel_lir::runtime_abi::CTX_FUEL_OFFSET.into(),
            ),
        );
        let fuel = self.builder.load(self.module.int_type(64), at, 8);
        let helper = self.runtime(RuntimeEntry::HostDefer)?;
        self.builder.call(
            helper,
            &[
                state,
                self.module.const_int(self.module.int_type(32), slot.into()),
                block,
                fuel,
            ],
        );
        self.builder.br(next);
        self.builder.position(ordinary);
        self.ahead_barrier()?;
        self.instruction(inst)?;
        self.builder.br(next);
        self.builder.position(next);
        Ok(true)
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// Publish only the successful mapper result, before any instruction can
    /// move or release it. The image and retained table borrow that SSA owner.
    pub(super) fn cpu_publish(&mut self, call: CallId) -> Emitted {
        let Some(summary) = self
            .inputs
            .ahead
            .and_then(|plan| plan.cpu_summary(self.unit.header.instance, call))
            .cloned()
        else {
            return Ok(());
        };
        let (entry, state) = match self.ahead_state {
            Some(state) => (RuntimeEntry::CpuAheadPublish, state),
            None => (RuntimeEntry::CpuPublish, self.context),
        };
        let image = self.stack(summary.layout)?;
        self.store(
            self.value_shape(summary.value)?,
            self.get(summary.value)?,
            image,
        );
        let ordinal = self.slots[summary.ordinal.index()].ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "CPU summary without its ordinal capture".into(),
        })?;
        let ordinal = self.load(Shape::Scalar(ScalarKind::U64), ordinal);
        let record = self
            .prologue
            .alloca(self.module.array_type(self.module.int_type(8), 48), 8);
        let retained = if summary.retained_count == 0 {
            self.module.const_zero(self.module.ptr_type())
        } else {
            let table = self.prologue.alloca(
                self.module
                    .array_type(self.module.ptr_type(), u64::from(summary.retained_count)),
                8,
            );
            for i in 0..summary.retained_count {
                self.builder.store(
                    self.module.const_zero(self.module.ptr_type()),
                    self.builder.offset(table, self.const_u64(u64::from(i) * 8)),
                    8,
                );
            }
            self.cpu_retained(image, &summary.owners, table, &mut 0)?;
            table
        };
        let i32t = self.module.int_type(32);
        for (offset, value, align) in [
            (0, self.module.const_int(i32t, 1), 4),
            (4, self.module.const_zero(i32t), 4),
            (8, ordinal, 8),
            (16, image.ptr, 8),
            (24, self.const_u64(summary.bytes), 8),
            (32, self.descriptor(summary.layout)?, 4),
            (
                36,
                self.module
                    .const_int(i32t, u128::from(summary.retained_count)),
                4,
            ),
            (40, retained, 8),
        ] {
            self.builder.store(
                value,
                self.builder.offset(record, self.const_u64(offset)),
                align,
            );
        }
        self.ahead_flush();
        let publish = self.runtime(entry)?;
        let status = self.builder.call(publish, &[state, record]);
        self.ahead_reload();
        let accepted = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_zero(self.module.int_type(8)),
        );
        let next = self.module.append_block(self.function);
        let invalid = self.module.append_block(self.function);
        self.builder.cond_br(accepted, next, invalid);
        self.builder.position(invalid);
        let trap = self
            .module
            .intrinsic("llvm.trap", &[])
            .or_else(|error| self.invalid(error))?;
        self.builder.call(trap, &[]);
        self.builder.unreachable();
        self.builder.position(next);
        Ok(())
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn cpu_reserve(&mut self) -> Emitted<bool> {
        if !self.inputs.alternate {
            return Ok(false);
        }
        let index = self.unit.blocks[self.at.0.index()].instructions.start as usize + self.at.1;
        let Some((recipe, ordinal_offset)) = self
            .inputs
            .ahead
            .and_then(|p| p.cpu_growth(self.unit.header.instance, index, self.inputs.layouts))
            .map(|(recipe, offset)| (recipe.clone(), offset))
        else {
            return Ok(false);
        };
        let state = self.ahead_state.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "CPU growth without its physical state".into(),
        })?;
        use hirsel_lir::ahead::CpuGrowthPlace;
        let place = match recipe.place {
            CpuGrowthPlace::Address(address) => Some(self.place(address)?),
            CpuGrowthPlace::Copy { .. } => None,
        };
        let old = match recipe.place {
            CpuGrowthPlace::Copy { value, .. } => self.get(value)?,
            CpuGrowthPlace::Address(_) => {
                self.load(Shape::Scalar(ScalarKind::Pointer), place.unwrap())
            }
        };
        let kept = recipe.capacity.and_then(|_| {
            self.sequence_state_at(place.unwrap())
                .map(|state| self.grown_facts(state, old))
        });
        let kind = match recipe.place {
            CpuGrowthPlace::Copy { .. } => 5,
            CpuGrowthPlace::Address(_) if recipe.capacity.is_some() => 1,
            CpuGrowthPlace::Address(_) => 4,
        };
        let done = if kind == 4 {
            let slow = self.module.append_block(self.function);
            let done = self.module.append_block(self.function);
            let tracked = crate::runtime::is_tracked(&self.builder, old);
            self.builder.cold_cond_br(tracked, slow, done);
            self.builder.position(slow);
            Some(done)
        } else {
            None
        };
        let environment = self.builder.load(
            self.module.ptr_type(),
            self.builder.offset(
                state,
                self.const_u64(hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET),
            ),
            8,
        );
        let ordinal = self.load_u64(
            self.builder
                .offset(environment, self.const_u64(ordinal_offset)),
        );
        let record = self
            .prologue
            .alloca(self.module.array_type(self.module.int_type(8), 88), 8);
        let i32t = self.module.int_type(32);
        let LayoutKind::Block { payload } = self.layout(recipe.layout)?.kind else {
            return self.invalid("CPU sequence handoff without a block payload");
        };
        let descriptor = self.descriptor(payload)?;
        for (offset, value, align) in [
            (0, self.module.const_int(i32t, 1), 4),
            (4, self.module.const_int(i32t, kind), 4),
            (8, ordinal, 8),
            (16, old, 8),
            (24, self.module.const_zero(self.module.ptr_type()), 8),
            (32, descriptor, 4),
            (36, self.module.const_zero(i32t), 4),
            (40, self.const_u64(0), 8),
            (48, self.const_u64(0), 8),
            (56, self.const_u64(0), 8),
            (64, self.const_u64(recipe.size), 8),
            (72, self.const_u64(u64::from(recipe.align)), 8),
            (
                80,
                match recipe.capacity {
                    Some(capacity) => self.get(capacity)?,
                    None => self.const_u64(0),
                },
                8,
            ),
        ] {
            self.builder.store(
                value,
                self.builder.offset(record, self.const_u64(offset)),
                align,
            );
        }
        self.ahead_flush();
        let helper = self.runtime(RuntimeEntry::CpuAheadGrow)?;
        let status = self.builder.call(helper, &[state, record]);
        self.ahead_reload();
        let accepted = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_zero(self.module.int_type(8)),
        );
        let next = self.module.append_block(self.function);
        let invalid = self.module.append_block(self.function);
        self.builder.cond_br(accepted, next, invalid);
        self.builder.position(invalid);
        let trap = self
            .module
            .intrinsic("llvm.trap", &[])
            .or_else(|error| self.invalid(error))?;
        self.builder.call(trap, &[]);
        self.builder.unreachable();
        self.builder.position(next);
        let grown = self.builder.load(
            self.module.ptr_type(),
            self.builder.offset(record, self.const_u64(16)),
            8,
        );
        if let CpuGrowthPlace::Copy { result, .. } = recipe.place {
            return self.define(result, grown).map(|()| true);
        }
        let place = place.unwrap();
        self.store(Shape::Scalar(ScalarKind::Pointer), grown, place);
        if let (Some(state), Some((unique, untracked))) = (self.sequence_state_at(place), kept) {
            self.builder.store(unique, state.unique, 1);
            self.builder.store(untracked, state.untracked, 1);
        }
        if let Some(done) = done {
            self.builder.br(done);
            self.builder.position(done);
        }
        Ok(true)
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    fn cpu_retained(
        &mut self,
        place: Place<'m>,
        scan: &[ScanStep],
        table: Value<'m>,
        cursor: &mut u64,
    ) -> Emitted {
        for step in scan {
            match step {
                ScanStep::Counted { offset } | ScanStep::SliceOwner { offset } => {
                    let root = self.builder.load(
                        self.module.ptr_type(),
                        self.builder.offset(place.ptr, self.const_u64(*offset)),
                        place.align.min(8).min(low_bit(*offset)),
                    );
                    self.builder.store(
                        root,
                        self.builder.offset(table, self.const_u64(*cursor * 8)),
                        8,
                    );
                    *cursor += 1;
                }
                ScanStep::Inline { offset, layout } => {
                    let child = self.field_place(place, *offset, *layout, false)?;
                    let scan = self.layout(*layout)?.scan.clone();
                    self.cpu_retained(child, &scan, table, cursor)?;
                }
                ScanStep::Repeat {
                    offset,
                    count,
                    stride,
                    element,
                } => {
                    let scan = self.layout(*element)?.scan.clone();
                    for i in 0..*count {
                        let child =
                            self.field_place(place, offset + i * stride, *element, false)?;
                        self.cpu_retained(child, &scan, table, cursor)?;
                    }
                }
                ScanStep::ActiveVariant {
                    tag_offset,
                    form,
                    variants,
                } => {
                    let active = self.tag_at(place, *tag_offset, *form, false);
                    let done = self.module.append_block(self.function);
                    let here = self.builder.current();
                    let mut cases = Vec::new();
                    for (variant, steps) in variants {
                        let arm = self.module.append_block(self.function);
                        self.builder.position(arm);
                        self.cpu_retained(place, steps, table, cursor)?;
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
                _ => return self.invalid("uncertified CPU retained root"),
            }
        }
        Ok(())
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    pub(super) fn cpu_region(
        &mut self,
        region: Value<'m>,
        layout: LayoutId,
        extent: Value<'m>,
        align: u32,
    ) -> Emitted<Option<Value<'m>>> {
        if !self.inputs.alternate {
            return Ok(None);
        }
        let index = self.unit.blocks[self.at.0.index()].instructions.start as usize + self.at.1;
        let Some(plan) = self.inputs.ahead else {
            return Ok(None);
        };
        let Some(recipe) = plan.cpu_regional(self.unit.header.instance, index) else {
            return Ok(None);
        };
        if recipe.layout != layout || recipe.align != align {
            return self.invalid("CPU regional layout mismatch");
        }
        let ordinal_offset = plan
            .cpu_ordinal(self.unit.header.instance, self.inputs.layouts)
            .ok_or_else(|| CodegenError::Invalid {
                definition: self.definition().clone(),
                what: "ambiguous CPU regional ordinal".into(),
            })?;
        let state = self.ahead_state.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "CPU region without physical state".into(),
        })?;
        let environment = self.builder.load(
            self.module.ptr_type(),
            self.builder.offset(
                state,
                self.const_u64(hirsel_lir::runtime_abi::AHEAD_WORKER_ENVIRONMENT_OFFSET),
            ),
            8,
        );
        let ordinal = self.load_u64(
            self.builder
                .offset(environment, self.const_u64(ordinal_offset)),
        );
        let record = self
            .prologue
            .alloca(self.module.array_type(self.module.int_type(8), 88), 8);
        let i32t = self.module.int_type(32);
        for (offset, value, alignment) in [
            (0, self.module.const_int(i32t, 1), 4),
            (4, self.module.const_int(i32t, 3), 4),
            (8, ordinal, 8),
            (16, self.module.const_zero(self.module.ptr_type()), 8),
            (24, region, 8),
            (32, self.descriptor(layout)?, 4),
            (36, self.module.const_zero(i32t), 4),
            (40, extent, 8),
            (48, self.const_u64(u64::from(align)), 8),
            (56, self.const_u64(0), 8),
            (64, self.const_u64(0), 8),
            (72, self.const_u64(0), 8),
            (80, self.const_u64(0), 8),
        ] {
            self.builder.store(
                value,
                self.builder.offset(record, self.const_u64(offset)),
                alignment,
            );
        }
        self.ahead_flush();
        let helper = self.runtime(RuntimeEntry::CpuAheadGrow)?;
        let status = self.builder.call(helper, &[state, record]);
        self.ahead_reload();
        let accepted = self.builder.icmp(
            IntPredicate::Eq,
            status,
            self.module.const_zero(self.module.int_type(8)),
        );
        let next = self.module.append_block(self.function);
        let invalid = self.module.append_block(self.function);
        self.builder.cond_br(accepted, next, invalid);
        self.builder.position(invalid);
        let trap = self
            .module
            .intrinsic("llvm.trap", &[])
            .or_else(|error| self.invalid(error))?;
        self.builder.call(trap, &[]);
        self.builder.unreachable();
        self.builder.position(next);
        Ok(Some(self.builder.load(
            self.module.ptr_type(),
            self.builder.offset(record, self.const_u64(16)),
            8,
        )))
    }
}
