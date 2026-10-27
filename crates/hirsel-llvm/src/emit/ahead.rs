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
        let Some(state) = self.ahead_state else {
            return Ok(());
        };
        self.ahead_flush();
        let helper = self.ahead_helper(true)?;
        self.builder.call(
            helper,
            &[state, self.module.const_int(self.module.int_type(32), 4)],
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
        if !self.inputs.alternate {
            return Ok(());
        }
        let Some(summary) = self
            .inputs
            .ahead
            .and_then(|plan| plan.cpu_summary(self.unit.header.instance, call))
            .cloned()
        else {
            return Ok(());
        };
        let state = self.ahead_state.ok_or_else(|| CodegenError::Invalid {
            definition: self.definition().clone(),
            what: "CPU summary without its physical state".into(),
        })?;
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
                self.module.const_int(i32t, u128::from(summary.retained)),
                4,
            ),
            // A retained Block/Box is a single pointer image. Its payload has
            // no child references, as independently checked by the CPU plan.
            (
                40,
                if summary.retained {
                    image.ptr
                } else {
                    self.module.const_zero(self.module.ptr_type())
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
        let publish = self.runtime(RuntimeEntry::CpuAheadPublish)?;
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
