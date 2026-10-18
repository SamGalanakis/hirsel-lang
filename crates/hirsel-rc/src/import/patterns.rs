use super::*;

impl Importer<'_, '_> {
    /// `Match from k`: arms k.. tested in order, each arm's failure continuing
    /// with the next; the remaining arms are exhaustive (TIR match coverage),
    /// so the last is entered without a test.
    pub(super) fn match_from(&mut self, table: tir::MatchTableId, from: u32) -> R<()> {
        let m = &self.body.matches[table.0 as usize];
        let arms = &m.arms[from as usize..];
        let Some((last, tested)) = arms.split_last() else {
            return malformed("a match continuation with no arms");
        };
        for arm in tested {
            let proven = if irrefutable(m, arm.pattern) {
                true
            } else if let tir::MatchPatternKind::Variant {
                variant, fields, ..
            } = &m.patterns[arm.pattern.0 as usize].kind
                && fields.iter().all(|(_, sub)| irrefutable(m, *sub))
            {
                let p = self.place(m.patterns[arm.pattern.0 as usize].place)?;
                self.proven_variant(&p) == Some(*variant)
            } else {
                false
            };
            if proven {
                // A failed guard resumes through its own TIR continuation.
                // No pattern-failure block is reachable from this arm.
                let edge = self.enter(arm.guard.unwrap_or(arm.target));
                self.terminate(Terminator::Jump(edge));
                return Ok(());
            }
            let next = self.block(BlockKind::Body);
            // `next` holds what held before the test, and when only the tag
            // test of a variant pattern can fail, that it holds another variant.
            let mut after = self.facts.clone();
            if let tir::MatchPatternKind::Variant {
                variant, fields, ..
            } = &m.patterns[arm.pattern.0 as usize].kind
                && fields.iter().all(|(_, sub)| irrefutable(m, *sub))
            {
                let p = self.place(m.patterns[arm.pattern.0 as usize].place)?;
                if let Some(version) = self.fact_version(&p)
                    && let Some(tag) = &self.record(p.ty).tag
                {
                    let others: Vec<u32> = tag
                        .variants
                        .iter()
                        .map(|v| v.ordinal)
                        .filter(|v| v != variant)
                        .collect();
                    narrow(&mut after, p.slot, &p.projection, version, &others);
                }
            }
            self.pattern(m, arm.pattern, next)?;
            let e = self.enter(arm.guard.unwrap_or(arm.target));
            self.terminate(Terminator::Jump(e));
            self.current = next;
            self.facts = after;
        }
        let e = self.enter(last.guard.unwrap_or(last.target));
        self.terminate(Terminator::Jump(e));
        Ok(())
    }

    /// Test one pattern; success continues in the current block, failure
    /// jumps to `fail`.
    pub(super) fn pattern(
        &mut self,
        m: &tir::MatchTable,
        id: tir::MatchPatternId,
        fail: BlockId,
    ) -> R<()> {
        use tir::MatchPatternKind as P;
        let pattern = &m.patterns[id.0 as usize];
        match &pattern.kind {
            P::Wildcard | P::Binding { .. } | P::Literal(tir::Constant::Unit) => {}
            P::Literal(tir::Constant::Bool(b)) => {
                let v = self.pattern_bits(pattern.place)?;
                let cont = self.block(BlockKind::Body);
                let hit = self.edge(cont, Vec::new());
                let miss = self.edge(fail, Vec::new());
                let (yes, no) = if *b { (hit, miss) } else { (miss, hit) };
                self.terminate(Terminator::Branch {
                    condition: v,
                    yes,
                    no,
                });
                self.current = cont;
            }
            P::Literal(tir::Constant::Integer(n)) => {
                let v = self.pattern_bits(pattern.place)?;
                self.compare_constant(v, BinaryOp::Eq, *n, fail)?;
            }
            P::Range {
                start: tir::Constant::Integer(start),
                end: tir::Constant::Integer(end),
            } => {
                // The value type's signedness orders both comparisons.
                let v = self.pattern_bits(pattern.place)?;
                self.compare_constant(v, BinaryOp::Ge, *start, fail)?;
                self.compare_constant(v, BinaryOp::Le, *end, fail)?;
            }
            P::Range { .. } => return malformed("a range pattern with non-integer bounds"),
            P::Literal(other) => {
                return unsupported(format!("a {other:?} literal pattern"));
            }
            P::Tuple(fields) | P::Record { fields, .. } => {
                for (_, sub) in fields {
                    self.pattern(m, *sub, fail)?;
                }
            }
            P::Variant {
                variant, fields, ..
            } => {
                self.tag(pattern.place, *variant, fail)?;
                for (_, sub) in fields {
                    self.pattern(m, *sub, fail)?;
                }
            }
        }
        Ok(())
    }

    /// Continue when `v op n` holds; otherwise jump to `fail`.
    fn compare_constant(&mut self, v: ValueId, op: BinaryOp, n: u128, fail: BlockId) -> R<()> {
        let ty = self.value_type(v);
        let constant = self.constant(ty, ConstantValue::Integer(n));
        let k = self.fresh(ty);
        self.push(Inst::Const {
            result: k,
            constant,
        });
        let bty = self.bool_type()?;
        let c = self.value(bty, Holding::Plain);
        let failure = self.fail(None)?;
        self.push(Inst::Binary {
            result: c,
            op,
            left: v,
            right: k,
            failure,
        });
        let cont = self.block(BlockKind::Body);
        let yes = self.edge(cont, Vec::new());
        let no = self.edge(fail, Vec::new());
        self.terminate(Terminator::Branch {
            condition: c,
            yes,
            no,
        });
        self.current = cont;
        Ok(())
    }

    pub(super) fn unsigned(&self, ty: LirTypeId) -> bool {
        use hirsel_lir::layout_types::ScalarKind as K;
        matches!(
            self.out.types[ty.index()].repr,
            ValueRepr::Scalar(K::U8 | K::U16 | K::U32 | K::U64 | K::U128)
        )
    }

    /// The LIR type of `bool`, for comparison results.
    pub(super) fn bool_type(&mut self) -> R<LirTypeId> {
        let ty = self.store.push(hirsel_types::Type::boolean());
        let layout = self.layout(ty)?;
        Ok(self.intern(layout))
    }

    /// A plain read of a pattern place.
    pub(super) fn pattern_bits(&mut self, place: tir::PlaceId) -> R<ValueId> {
        let p = self.place(place)?;
        if !self.bits(p.ty) {
            return unsupported("a literal pattern on a counted place");
        }
        let address = self.address(&p)?;
        let reaching = self.reaching(p.slot)?;
        let result = self.value(p.ty, Holding::Plain);
        self.push(Inst::Read {
            result,
            address,
            reaching,
            mode: ReadMode::Bits,
        });
        Ok(result)
    }

    /// Switch on a variant's tag: every variant is an arm (`variant` continues,
    /// the others fail), so the otherwise edge is unreachable. A counted
    /// variant is read through a synthetic loan ended on every arm.
    pub(super) fn tag(&mut self, place: tir::PlaceId, variant: u32, fail: BlockId) -> R<()> {
        let p = self.place(place)?;
        // Earlier arms may already have proved this tag at the current
        // version. Retesting it would introduce infeasible variant edges.
        if self.proven_variant(&p) == Some(variant) {
            return Ok(());
        }
        let address = self.address(&p)?;
        let reaching = self.reaching(p.slot)?;
        let variants: Vec<u32> = match &self.record(p.ty).tag {
            Some(tag) => tag.variants.iter().map(|v| v.ordinal).collect(),
            None => return malformed("a variant pattern on a layout without a tag"),
        };
        let loan = self.owns(p.ty).then(|| self.synthetic_loan(address));
        let (mode, holding) = match loan {
            Some(l) => (ReadMode::Borrow, Holding::Borrowed(l)),
            None if self.bits(p.ty) => (ReadMode::Bits, Holding::Plain),
            // Uncounted but not plain bits (a fixed flat record's arm): a
            // plain view, as a borrowed read of uncounted storage elsewhere.
            None => (ReadMode::Borrow, Holding::Plain),
        };
        let value = self.value(p.ty, holding);
        self.push(Inst::Read {
            result: value,
            address,
            reaching,
            mode,
        });
        let cont = self.block(BlockKind::Body);
        let switch = self.current;
        let mut arms = Vec::new();
        for v in variants {
            let target = if v == variant { cont } else { fail };
            let edge = match loan {
                None => self.edge(target, Vec::new()),
                Some(l) => {
                    let trampoline = self.block(BlockKind::Body);
                    let e = self.edge(trampoline, Vec::new());
                    self.current = trampoline;
                    self.push(Inst::EndLoan {
                        loan: l,
                        if_active: false,
                    });
                    let out = self.edge(target, Vec::new());
                    self.terminate(Terminator::Jump(out));
                    self.current = switch;
                    e
                }
            };
            arms.push(SwitchArm { value: v, edge });
        }
        let unreachable = match self.unreachable {
            Some(b) => b,
            None => {
                let b = self.block(BlockKind::Body);
                self.current = b;
                self.terminate(Terminator::Unreachable);
                self.current = switch;
                self.unreachable = Some(b);
                b
            }
        };
        let otherwise = self.edge(unreachable, Vec::new());
        let arms = PoolRange::push(&mut self.out.pools.switch_arms, arms);
        self.terminate(Terminator::Switch {
            value,
            arms,
            otherwise,
        });
        if let Some(l) = loan {
            self.state.loans[l.index()] = Tri::No;
        }
        self.current = cont;
        if self.fact_version(&p) == Some(reaching) {
            narrow(&mut self.facts, p.slot, &p.projection, reaching, &[variant]);
        }
        Ok(())
    }
}
