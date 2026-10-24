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
        let expired = self
            .builder
            .icmp(IntPredicate::Sle, left, self.module.const_zero(i64t));
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
