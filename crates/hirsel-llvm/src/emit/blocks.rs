use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Blocks and edges.

    pub(super) fn emit_block(&mut self, b: BlockId) -> Emitted {
        let unit = self.unit;
        self.builder.position(self.blocks[b.index()]);
        if self.fuel_points.get(b.index()).copied().unwrap_or(false) {
            self.charge_fuel()?;
        }
        self.emit_block_suffix(b, 0, false)
    }

    pub(super) fn emit_block_suffix(
        &mut self,
        b: BlockId,
        start: usize,
        resuming: bool,
    ) -> Emitted {
        let unit = self.unit;
        for (i, inst) in self
            .pool(unit.blocks[b.index()].instructions, &unit.instructions)?
            .iter()
            .enumerate()
            .skip(start)
        {
            self.at = (b, i);
            self.profile_location(
                hirsel_lir::profile::instruction_site(unit, b, inst),
                &format!("{:?}", inst.opcode()),
            )?;
            if self.host_instruction(inst)? {
                continue;
            }
            if let Inst::Release { layout, .. } = inst
                && !(self
                    .inputs
                    .ahead
                    .and_then(|p| p.host_spine(unit.header.instance))
                    .is_some()
                    && hirsel_lir::ahead::scalar_image(self.inputs.layouts, *layout))
            {
                self.ahead_barrier()?;
            }
            if !(resuming && i == start) {
                self.ownership_events(b, i as u32, hirsel_lir::ownership_trace::Moment::Before)?;
            }
            self.resume_cleanup_bind(b, i)?;
            self.instruction(inst)?;
            self.ownership_events(b, i as u32, hirsel_lir::ownership_trace::Moment::After)?;
        }
        let terminator = unit.terminators[b.index()].clone();
        self.profile_location(
            hirsel_lir::profile::terminator_site(unit, b),
            &format!("{:?}", terminator.opcode()),
        )?;
        if matches!(terminator, Terminator::Return(_)) {
            self.ownership_point(hirsel_lir::ownership_trace::Point::Return(b))?;
        }
        self.terminator(&terminator)
    }

    /// One unit of the run's fuel (`EmitInputs::fuel`): the context's fuel word, the running
    /// task's chunk, loses one, and a negative result calls `hirsel_rt_v2_fuel_refill`, which
    /// stores the next chunk (or stops the run) and returns to the code after the charge.
    fn charge_fuel(&mut self) -> Emitted {
        if self.inputs.alternate {
            return self.ahead_point(true);
        }
        use crate::context::{BinOp, IntPredicate};
        let refill = self.runtime(RuntimeEntry::FuelRefill)?;
        let i64_ty = self.module.int_type(64);
        let at = self.builder.offset(
            self.context,
            self.module
                .const_int(i64_ty, hirsel_lir::runtime_abi::CTX_FUEL_OFFSET.into()),
        );
        let fuel = self.builder.load(i64_ty, at, 8);
        let left = self
            .builder
            .binary(BinOp::Sub, fuel, self.module.const_int(i64_ty, 1));
        self.builder.store(left, at, 8);
        let (spent, go) = (
            self.module.append_block(self.function),
            self.module.append_block(self.function),
        );
        self.builder.cold_cond_br(
            self.builder
                .icmp(IntPredicate::Slt, left, self.module.const_zero(i64_ty)),
            spent,
            go,
        );
        self.builder.position(spent);
        self.builder.call(refill, &[self.context]);
        self.builder.br(go);
        self.builder.position(go);
        Ok(())
    }

    pub(super) fn profile_location(&mut self, site: SiteRange, operation: &str) -> Emitted {
        if let Some((debug, scope, item)) = self.debug {
            debug.location(
                &self.builder,
                scope,
                self.debug_sites
                    .as_ref()
                    .and_then(|sites| sites.code(site))
                    .unwrap_or(0),
                item,
            );
        }
        let Some(source) = self.measured else {
            return Ok(());
        };
        // Explicit source-less glue inherits the surrounding call's context.
        // A mapped selector/range outside this source revision remains an error.
        if site.source == u32::MAX {
            self.builder.profile_site(None);
            return Ok(());
        }
        let Some(position) = source.site_location(site) else {
            return self.invalid("profile anchor is outside its source revision");
        };
        let (line, column) = (position.line, position.column);
        let (file, _) = source.source(site.source).expect("resolved source");
        let digest = ContentHash::of_parts(
            "hirsel.profile-site.v2",
            [
                self.unit.header.definition.fingerprint().0.as_slice(),
                self.unit.header.implementation_revision.0.as_slice(),
                &site.source.to_le_bytes(),
                &site.start.to_le_bytes(),
                &site.end.to_le_bytes(),
                operation.as_bytes(),
            ],
        );
        let id = u64::from_le_bytes(digest.0[..8].try_into().expect("eight bytes")).max(1);
        let name = format!("hirsel.profile.site.{id}");
        let global = if let Some(global) = self.module.named_global(&name) {
            global
        } else {
            let mut strings = Vec::new();
            for (index, text) in [
                self.unit.header.definition.0.package.name.as_str(),
                &file.logical_path().to_string_lossy(),
                &self.unit.header.definition.to_string(),
                operation,
            ]
            .into_iter()
            .enumerate()
            {
                let bytes = self.module.const_bytes(&[text.as_bytes(), &[0]].concat());
                let string =
                    self.module
                        .private_constant(&format!("{name}.{index}"), bytes.ty(), 1);
                self.module.set_initializer(string, bytes);
                strings.push(string);
            }
            let initializer = self.module.const_struct(&[
                self.const_u64(id),
                strings[0],
                strings[1],
                strings[2],
                strings[3],
                self.module.const_int(self.module.int_type(32), line.into()),
                self.module
                    .const_int(self.module.int_type(32), column.into()),
            ]);
            let global = self.module.private_constant(&name, initializer.ty(), 8);
            self.module.set_initializer(global, initializer);
            global
        };
        let enter_name = "hirsel_rt_v2_profile_enter";
        let leave_name = "hirsel_rt_v2_profile_leave";
        let ptr = self.module.ptr_type();
        let enter = self
            .module
            .function(
                enter_name,
                self.module
                    .function_type(self.module.int_type(64), &[ptr, ptr]),
            )
            .or_else(|what| self.invalid(what))?;
        let leave = self
            .module
            .function(
                leave_name,
                self.module
                    .function_type(self.module.void_type(), &[ptr, self.module.int_type(64)]),
            )
            .or_else(|what| self.invalid(what))?;
        for name in [enter_name, leave_name] {
            self.manifested
                .imports
                .entry(name.into())
                .or_insert(ObjectSymbol {
                    name: name.into(),
                    kind: ObjectSymbolKind::Runtime,
                    definition: None,
                    abi: None,
                });
        }
        self.builder
            .profile_site(Some((enter, leave, self.context, global)));
        Ok(())
    }

    /// Adds `edge`'s arguments to its target's phis as coming from `from`.
    pub(super) fn incoming(&self, edge: &Edge, from: BasicBlock<'m>) -> Emitted {
        let arguments = self.pool(edge.arguments, &self.unit.pools.values)?;
        let phis = &self.phis[edge.target.index()];
        if arguments.len() != phis.len() {
            return self.invalid("edge arguments differ from the target's parameters");
        }
        for (&argument, phi) in arguments.iter().zip(phis) {
            if let Some(phi) = phi {
                self.builder.add_incoming(*phi, self.get(argument)?, from);
            }
        }
        Ok(())
    }

    /// Branches from the current block along `edge`.
    pub(super) fn jump(&mut self, edge: EdgeId) -> Emitted {
        self.ownership_point(hirsel_lir::ownership_trace::Point::Edge(edge))?;
        let edge = self.edge(edge)?;
        self.incoming(edge, self.builder.current())?;
        self.builder.br(self.blocks[edge.target.index()]);
        Ok(())
    }

    /// A block that takes `edge` from a multi-way branch: the target itself when the edge
    /// passes no values, else a fresh block carrying the arguments.
    pub(super) fn edge_block(&mut self, edge: EdgeId) -> Emitted<BasicBlock<'m>> {
        let e = self.edge(edge)?;
        let carries = self.phis[e.target.index()].iter().any(Option::is_some);
        let observed = self.ownership.is_some_and(|plan| {
            plan.at_point(hirsel_lir::ownership_trace::Point::Edge(edge))
                .next()
                .is_some()
        });
        if !carries && !observed {
            return Ok(self.blocks[e.target.index()]);
        }
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        self.jump(edge)?;
        self.builder.position(here);
        Ok(block)
    }

    /// The block a failure edge starts in: it begins the failure (when the edge names a
    /// rule), disposes of the listed tokens and enters the cleanup edge.
    pub(super) fn failure_block(&mut self, failure: FailureId) -> Emitted<BasicBlock<'m>> {
        if let Some(&block) = self.failures.get(&failure) {
            return Ok(block);
        }
        let edge = self.failure_edge(failure)?;
        let here = self.builder.current();
        let block = self.module.append_block(self.function);
        self.builder.position(block);
        self.ahead_barrier()?;
        if edge.rule.as_deref() == Some("MEM-9") {
            let stop = self.runtime(RuntimeEntry::UncheckedAddOverflow)?;
            let site = self.anchor_code(edge.anchor)?;
            let site = self
                .module
                .const_int(self.module.int_type(64), u128::from(site));
            self.builder.call(stop, &[self.context, site]);
            self.builder.unreachable();
            self.builder.position(here);
            self.failures.insert(failure, block);
            return Ok(block);
        }
        if let Some(rule) = &edge.rule {
            let (kind, rule) = self.failure_code(rule);
            let (site, operation) = self.failure_sites(self.anchor_code(edge.anchor)?);
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            self.builder
                .call(begin, &[self.context, kind, rule, site, operation]);
        }
        if edge.rule.is_none() {
            let (site, operation) = self.failure_sites(self.anchor_code(edge.anchor)?);
            let reanchor = self.runtime(RuntimeEntry::FailureReanchor)?;
            self.builder
                .call(reanchor, &[self.context, site, operation]);
        }
        let dispose = self.pool(edge.dispose, &self.unit.pools.values)?;
        if !dispose.is_empty() {
            let entry = self.runtime(RuntimeEntry::Dispose)?;
            for &token in dispose {
                self.builder.call(entry, &[self.context, self.get(token)?]);
            }
        }
        self.jump(edge.cleanup)?;
        self.builder.position(here);
        self.failures.insert(failure, block);
        Ok(block)
    }

    /// Branches to `failure` when `failed` holds and continues in a fresh block.
    /// The index in `LirUnit::instructions` of the instruction being emitted.
    pub(super) fn instruction_index(&self) -> usize {
        self.unit.blocks[self.at.0.index()].instructions.start as usize + self.at.1
    }

    /// A depth-leaf callee's admission (`invocations::room`): whether the task has no
    /// room for one more activation, as `hirsel_rt_v2_depth_enter_at_site` decides. In
    /// cleanup mode (nesting or cleanup count nonzero) the cleanup count is tested,
    /// otherwise the foreground count; the invocation stack's capacity is their sum.
    ///
    /// Like Go's stack-split prologue (cmd/internal/obj/x86/obj6.go `stacksplit`), the
    /// counters are read once, in the prologue: every admission a callee, finalizer or
    /// callback makes is retired before control returns to this frame, so outside its
    /// own admission a frame always sees its entry values. One test then serves every
    /// such call, and a loop's is invariant.
    pub(super) fn no_room(&mut self) -> Value<'m> {
        if let Some(full) = self.no_room {
            return full;
        }
        use crate::context::{Access, BinOp, Cast, IntPredicate};
        use hirsel_core_contract::depth::{CLEANUP_LIMIT, FOREGROUND_LIMIT};
        use hirsel_lir::runtime_abi::{
            CTX_CLEANUP_COUNT_OFFSET, CTX_CLEANUP_MODE_OFFSET, CTX_FOREGROUND_COUNT_OFFSET,
        };
        let b = &self.prologue;
        let (i16_ty, i32_ty, i64_ty) = (
            self.module.int_type(16),
            self.module.int_type(32),
            self.module.int_type(64),
        );
        let word = |offset: u64, ty, align| {
            let at = b.offset(self.context, self.module.const_int(i64_ty, offset.into()));
            b.load_access(ty, at, align, Access::Depth)
        };
        let foreground = word(CTX_FOREGROUND_COUNT_OFFSET, i16_ty, 2);
        let cleanup = word(CTX_CLEANUP_COUNT_OFFSET, i16_ty, 2);
        let mode = word(CTX_CLEANUP_MODE_OFFSET, i32_ty, 4);
        let either = b.binary(BinOp::Or, mode, b.cast(Cast::ZExt, cleanup, i32_ty));
        let cleaning = b.icmp(IntPredicate::Ne, either, self.module.const_zero(i32_ty));
        let limit = |count, limit: u16| {
            b.icmp(
                IntPredicate::Uge,
                count,
                self.module.const_int(i16_ty, limit.into()),
            )
        };
        let (cleanup_full, foreground_full) = (
            limit(cleanup, CLEANUP_LIMIT),
            limit(foreground, FOREGROUND_LIMIT),
        );
        let full = b.select(cleaning, cleanup_full, foreground_full);
        self.no_room = Some(full);
        full
    }

    pub(super) fn guard(&mut self, failed: Value<'m>, failure: FailureId) -> Emitted {
        let fail = self.failure_block(failure)?;
        let next = self.module.append_block(self.function);
        self.builder.cold_cond_br(failed, fail, next);
        self.builder.position(next);
        Ok(())
    }

    /// A guard closure for the scalar module, bound to one failure edge.
    pub(super) fn guard_for(
        &mut self,
        failure: FailureId,
    ) -> Emitted<impl FnMut(&Builder<'m>, Value<'m>) + use<'m>> {
        let fail = self.failure_block(failure)?;
        let module = self.module;
        let function = self.function;
        Ok(move |b: &Builder<'m>, failed: Value<'m>| {
            let next = module.append_block(function);
            b.cold_cond_br(failed, fail, next);
            b.position(next);
        })
    }

    /// Branches to `failure` when a runtime entry's status is nonzero. The runtime has
    /// already recorded the failure.
    pub(super) fn status_guard(&mut self, status: Value<'m>, failure: FailureId) -> Emitted {
        let zero = self.module.const_int(self.module.int_type(8), 0);
        let failed = self.builder.icmp(IntPredicate::Ne, status, zero);
        self.guard(failed, failure)
    }

    /// Raw inherited attribution remains zero when the entry has no language caller.
    /// A library's actual operation comes from its source invocation, never its synthetic span.
    pub(super) fn caller_sites(&self, code: u64, forward: bool) -> (Value<'m>, Value<'m>) {
        self.caller_site_values(self.const_u64(code), forward)
    }
    fn caller_site_values(&self, code: Value<'m>, forward: bool) -> (Value<'m>, Value<'m>) {
        let incoming = |hidden| {
            self.signature
                .hidden_index(hidden)
                .map(|i| self.module.parameter(self.function, i))
                .unwrap_or_else(|| self.const_u64(0))
        };
        let operation = if self.unit.header.origin == UnitOrigin::Library {
            incoming(|h| matches!(h, Hidden::OperationSite))
        } else {
            code
        };
        let caller = if forward
            && (self.unit.header.track_caller || self.unit.header.origin == UnitOrigin::Library)
        {
            incoming(|h| matches!(h, Hidden::CallerSite))
        } else {
            operation
        };
        (caller, operation)
    }
    pub(super) fn failure_sites(&self, code: u64) -> (Value<'m>, Value<'m>) {
        self.failure_sites_for(code, true)
    }
    pub(super) fn failure_sites_for(&self, code: u64, forward: bool) -> (Value<'m>, Value<'m>) {
        self.failure_site_values_for(self.const_u64(code), forward)
    }
    pub(super) fn failure_site_values(&self, code: Value<'m>) -> (Value<'m>, Value<'m>) {
        self.failure_site_values_for(code, true)
    }
    fn failure_site_values_for(&self, code: Value<'m>, forward: bool) -> (Value<'m>, Value<'m>) {
        let (caller, operation) = self.caller_site_values(code, forward);
        let present = self
            .builder
            .icmp(IntPredicate::Ne, caller, self.const_u64(0));
        (self.builder.select(present, caller, operation), operation)
    }

    /// The site code of a source anchor.
    pub(super) fn anchor_code(&self, anchor: SourceAnchorId) -> Emitted<u64> {
        if self.unit.header.origin == UnitOrigin::Library {
            return Ok(0);
        }
        let Some(anchor) = self.unit.tables.source_anchors.get(anchor.index()) else {
            return self.invalid("an anchor outside the anchor table");
        };
        Ok(site_code(
            &self.unit.header.definition,
            &self.unit.header.implementation_revision,
            anchor.site,
        ))
    }

    /// The site code of a cleanup anchor: the `anchor` a release or finalizer reports.
    pub(super) fn cleanup_code(&self, anchor: CleanupAnchorId) -> Emitted<u64> {
        let Some(anchor) = self.unit.tables.cleanup_anchors.get(anchor.index()) else {
            return self.invalid("a cleanup anchor outside the anchor table");
        };
        Ok(site_code(
            &self.unit.header.definition,
            &self.unit.header.implementation_revision,
            anchor.site,
        ))
    }
}

impl<'m, 'a> FnEmitter<'m, 'a> {
    /// This frame's allowance cannot change outside its own admissions: the
    /// selector refuses FinalizeBegin/End and every callee retires its counters.
    fn counter_cell(&mut self) -> (Value<'m>, Value<'m>) {
        if let Some(cell) = self.counter_cell {
            return cell;
        }
        use crate::context::{Access, BinOp, Cast, IntPredicate};
        use hirsel_core_contract::depth::{CLEANUP_LIMIT, FOREGROUND_LIMIT};
        use hirsel_lir::runtime_abi::{
            CTX_CLEANUP_COUNT_OFFSET, CTX_CLEANUP_MODE_OFFSET, CTX_FOREGROUND_COUNT_OFFSET,
        };
        let b = &self.prologue;
        let i16_ty = self.module.int_type(16);
        let i32_ty = self.module.int_type(32);
        let offset = |n| b.offset(self.context, self.const_u64(n));
        let foreground_at = offset(CTX_FOREGROUND_COUNT_OFFSET);
        let cleanup_at = offset(CTX_CLEANUP_COUNT_OFFSET);
        let cleanup = b.load_access(i16_ty, cleanup_at, 2, Access::Depth);
        let mode = b.load_access(i32_ty, offset(CTX_CLEANUP_MODE_OFFSET), 4, Access::Depth);
        let either = b.binary(BinOp::Or, mode, b.cast(Cast::ZExt, cleanup, i32_ty));
        let cleaning = b.icmp(IntPredicate::Ne, either, self.module.const_zero(i32_ty));
        let cell = (
            b.select(cleaning, cleanup_at, foreground_at),
            b.select(
                cleaning,
                self.module.const_int(i16_ty, CLEANUP_LIMIT.into()),
                self.module.const_int(i16_ty, FOREGROUND_LIMIT.into()),
            ),
        );
        self.counter_cell = Some(cell);
        cell
    }
    pub(super) fn counter_enter(&mut self, failure: FailureId) -> Emitted {
        use crate::context::{Access, BinOp, IntPredicate};
        let (at, limit) = self.counter_cell();
        let ty = self.module.int_type(16);
        let count = self.builder.load_access(ty, at, 2, Access::Depth);
        let full = self.builder.icmp(IntPredicate::Uge, count, limit);
        self.guard(full, failure)?;
        let next = self
            .builder
            .binary(BinOp::Add, count, self.module.const_int(ty, 1));
        self.builder.store_access(next, at, 2, Access::Depth);
        Ok(())
    }
    pub(super) fn counter_leave(&mut self) {
        use crate::context::{Access, BinOp};
        let (at, _) = self.counter_cell();
        let ty = self.module.int_type(16);
        let count = self.builder.load_access(ty, at, 2, Access::Depth);
        let next = self
            .builder
            .binary(BinOp::Sub, count, self.module.const_int(ty, 1));
        self.builder.store_access(next, at, 2, Access::Depth);
    }
}
