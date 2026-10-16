//! Checkpoints precede executor steps, so suspended frames and drains are intact.
//! The classifier only avoids needless clones; executed site deltas verify it.
use super::*;
use crate::{Audit, Campaign, Run};

impl<'p, S: Stage> Machine<'p, S> {
    fn fork<'q>(&self, config: &'q Config, ops: &'q mut dyn Operations) -> Machine<'q, S>
    where
        'p: 'q,
    {
        Machine {
            program: self.program,
            config,
            ops,
            ledger: self.ledger.clone(),
            stack: self.stack.clone(),
            output: self.output.clone(),
            failures: self.failures.clone(),
            unanchored_failures: self.unanchored_failures.clone(),
            sites: self.sites,
            steps: self.steps,
            runtime_iterators: self.runtime_iterators.clone(),
            version_slots: self.version_slots.clone(),
            statics: self.statics.clone(),
            constants: self.constants.clone(),
            roots: self.roots.clone(),
            forgotten: self.forgotten.clone(),
            finished: self.finished.clone(),
            terminal: self.terminal.clone(),
            word: self.word,
            resolving: self.resolving,
            holders: self.holders.clone(),
            arena_no_drop: self.arena_no_drop.clone(),
            open_elements: self.open_elements.clone(),
            full_recount_due: self.full_recount_due,
        }
    }

    fn checkpoint_site(&self) -> bool {
        match self.stack.last() {
            Some(Activity::Drain(d)) => matches!(
                d.work.last(),
                Some(Work::Step {
                    step: DropStep::Finalize,
                    ..
                })
            ),
            Some(Activity::Frame(f)) => {
                let u = self.unit(f.unit);
                let Some(block) = u.blocks.get(f.block.index()) else {
                    return false;
                };
                let Some(inst) = block
                    .instructions
                    .get(&u.instructions)
                    .and_then(|is| is.get(f.statement as usize))
                else {
                    return false;
                };
                match *inst {
                    // Use the actual operand kind, as exec does. Malformed LIR
                    // may disagree with its declared type; classification must
                    // not change its site domain or its original diagnostic.
                    Inst::Unary { op, operand, .. } => match self.get(f, operand).map(|h| &h.val) {
                        Ok(Val::Bits { scalar: k, .. }) => scalar::unary_may_fail(op, *k),
                        _ => true,
                    },
                    Inst::Binary { op, left, .. } => match self.get(f, left).map(|h| &h.val) {
                        Ok(Val::Bits { scalar: k, .. }) => scalar::binary_may_fail(op, *k),
                        _ => true,
                    },
                    Inst::Address { address, .. } => {
                        self.checked_steps(f.unit, address).unwrap_or(true)
                    }
                    Inst::Compare { .. }
                    | Inst::Check { .. }
                    | Inst::Copy { .. }
                    | Inst::FlatCopy { .. }
                    | Inst::UniqueCopy { .. }
                    | Inst::Hash { .. }
                    | Inst::FrameEnter { .. } => true,
                    _ => false,
                }
            }
            None => false,
        }
    }

    fn report(&self, result: Checked<(Observation, Vec<Exemption>)>) -> Run {
        Run {
            output: self.output.clone(),
            sites: self.sites,
            steps: self.steps,
            trace: self.ledger.trace.clone(),
            counters: self.ledger.counters,
            result: result
                .map(|(observation, exemptions)| Audit {
                    observation,
                    exemptions,
                })
                .map_err(|v| *v),
        }
    }

    fn campaign_steps(&mut self, injected: &mut Vec<(u64, Run)>) -> Checked<()> {
        while !self.stack.is_empty() {
            let site = self.sites;
            let checkpoint = self.checkpoint_site();
            if checkpoint {
                assert_eq!(
                    self.ops.domain(),
                    crate::OperationDomain::Simulation,
                    "host operations cannot be cloned"
                );
                let mut ops = self
                    .ops
                    .fork_simulation()
                    .expect("simulation clone capability must remain stable");
                assert_eq!(
                    ops.domain(),
                    crate::OperationDomain::Simulation,
                    "a simulation clone must remain simulated"
                );
                let config = Config {
                    inject: Some(site),
                    ..self.config.clone()
                };
                let mut fork = self.fork(&config, &mut *ops);
                let first = fork.step();
                if fork.sites == site + 1 {
                    let result = first.and_then(|_| fork.run()).and_then(|_| fork.finish());
                    injected.push((site, fork.report(result)));
                } else if fork.sites != site {
                    return fail(
                        V::InternalAudit,
                        "a checkpoint step executed multiple failure sites",
                    );
                }
            }
            self.step()?;
            if self.sites != site && (!checkpoint || self.sites != site + 1) {
                return fail(
                    V::InternalAudit,
                    "failure-site classifier missed an executed site",
                );
            }
        }
        Ok(())
    }
}

pub(crate) fn campaign<S: Stage>(
    program: &Program<S>,
    entry: &hirsel_syntax::DefPath,
    args: &[Arg],
    ops: &mut dyn Operations,
    config: &Config,
) -> Campaign {
    let mut machine = Machine::new(program, config, ops);
    let mut injected = Vec::new();
    let result = (|| {
        program.check()?;
        machine.start(program.entry(entry)?, args)?;
        machine.campaign_steps(&mut injected)?;
        machine.prepare_finish()?;
        machine.campaign_steps(&mut injected)?;
        machine.finish()
    })();
    Campaign {
        clean: machine.report(result),
        injected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoOperations;
    use crate::build::{Layouts, UnitBuilder, path};
    use hirsel_lir::layout_types::TargetSpec;
    use hirsel_lir::stage::Rc;

    #[test]
    fn scalar_site_classification_matches_runtime_operands_even_with_bad_metadata() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let float = layouts.scalar(ScalarKind::F64);
        let (target, table) = layouts.finish();
        let mut b = UnitBuilder::<Rc>::new(path("main"));
        let block = b.block(BlockKind::Body, &[]);
        let fail_block = b.block(BlockKind::FailureCleanup, &[]);
        b.end(fail_block, Terminator::Propagate);
        let edge = b.edge(fail_block, &[]);
        let failure = b.failure(edge, None, &[]);
        let operand = b.plain(ValueRepr::Scalar(ScalarKind::F64), float);
        let result = b.plain(ValueRepr::Scalar(ScalarKind::F64), float);
        b.push(
            block,
            Inst::Binary {
                result,
                op: BinaryOp::Add,
                left: operand,
                right: operand,
                failure,
            },
        );
        b.end(block, Terminator::Return(None));
        let program = Program {
            target,
            layouts: table.clone(),
            units: vec![b.finish(&table)],
            statics: vec![],
        };
        let config = Config::default();
        let mut ops = NoOperations;
        let mut machine = Machine::new(&program, &config, &mut ops);
        for (scalar, expected) in [(ScalarKind::U64, true), (ScalarKind::F64, false)] {
            let mut frame = machine.frame(0, Role::Entry, None);
            frame.values[operand.index()] = Some(Held {
                val: Val::Bits { scalar, bits: 0 },
                credits: 0,
                borrow: None,
            });
            machine.stack.push(Activity::Frame(Box::new(frame)));
            assert_eq!(machine.checkpoint_site(), expected);
            machine.stack.clear();
        }
    }
}
