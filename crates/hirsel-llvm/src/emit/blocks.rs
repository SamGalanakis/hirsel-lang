use super::*;

impl<'m, 'a> FnEmitter<'m, 'a> {
    // Blocks and edges.

    pub(super) fn emit_block(&mut self, b: BlockId) -> Emitted {
        let unit = self.unit;
        self.builder.position(self.blocks[b.index()]);
        for (i, inst) in self
            .pool(unit.blocks[b.index()].instructions, &unit.instructions)?
            .iter()
            .enumerate()
        {
            self.at = (b, i);
            self.profile_location(
                hirsel_lir::profile::instruction_site(unit, b, inst),
                &format!("{:?}", inst.opcode()),
            )?;
            self.instruction(inst)?;
        }
        let terminator = unit.terminators[b.index()].clone();
        self.profile_location(
            hirsel_lir::profile::terminator_site(unit, b),
            &format!("{:?}", terminator.opcode()),
        )?;
        self.terminator(&terminator)
    }

    pub(super) fn profile_location(&mut self, site: SiteRange, operation: &str) -> Emitted {
        let Some((debug, scope, source)) = self.profile else {
            return Ok(());
        };
        let Some(position) = source.location(site.start) else {
            return self.invalid("profile anchor is outside its source revision");
        };
        let (line, column) = (position.line, position.column);
        debug.location(&self.builder, scope, line, column);
        if !self.inputs.measurement {
            return Ok(());
        }
        let digest = ContentHash::of_parts(
            "hirsel.profile-site.v1",
            [
                self.unit.header.definition.fingerprint().0.as_slice(),
                self.unit.header.implementation_revision.0.as_slice(),
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
                &source.path.to_string_lossy(),
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
    pub(super) fn jump(&self, edge: EdgeId) -> Emitted {
        let edge = self.edge(edge)?;
        self.incoming(edge, self.builder.current())?;
        self.builder.br(self.blocks[edge.target.index()]);
        Ok(())
    }

    /// A block that takes `edge` from a multi-way branch: the target itself when the edge
    /// passes no values, else a fresh block carrying the arguments.
    pub(super) fn edge_block(&self, edge: EdgeId) -> Emitted<BasicBlock<'m>> {
        let e = self.edge(edge)?;
        let carries = self.phis[e.target.index()].iter().any(Option::is_some);
        if !carries {
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
        if let Some(rule) = &edge.rule {
            let Some(code) = self.inputs.failures.codes.get(rule).copied() else {
                return Err(CodegenError::UnknownRule {
                    definition: self.definition().clone(),
                    rule: rule.clone(),
                });
            };
            let (site, operation) = self.failure_sites(self.anchor_code(edge.anchor)?);
            let begin = self.runtime(RuntimeEntry::FailureBegin)?;
            let i32_ty = self.module.int_type(32);
            self.builder.call(
                begin,
                &[
                    self.context,
                    self.module.const_int(i32_ty, u128::from(code.kind)),
                    self.module.const_int(i32_ty, u128::from(code.rule)),
                    site,
                    operation,
                ],
            );
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
