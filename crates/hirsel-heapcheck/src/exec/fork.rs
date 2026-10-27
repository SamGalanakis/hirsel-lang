//! Checkpoints precede executor steps, so suspended frames and drains are intact.
//! The classifier only avoids needless clones; executed site deltas verify it.
use super::*;
use crate::{Audit, Run};
thread_local! { static SCANNED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
pub(super) fn scanned(n: usize) {
    SCANNED.with(|v| v.set(v.get() + n as u64));
}

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
            seal_inventory: self.seal_inventory.clone(),
            seal_allocations: self.seal_allocations.clone(),
            seal_sources_checked: self.seal_sources_checked,
            stack: self.stack.clone(),
            invocation_events: self.invocation_events.clone(),
            entry: self.entry,
            output: self.output.clone(),
            failures: self.failures.clone(),
            unanchored_failures: self.unanchored_failures.clone(),
            sites: self.sites,
            choices: self.choices,
            steps: self.steps,
            signaled: self.signaled,
            runtime_iterators: self.runtime_iterators.clone(),
            lazy_iterators: self.lazy_iterators.clone(),
            version_slots: self.version_slots.clone(),
            map_storage: self.map_storage.clone(),
            statics: self.statics.clone(),
            constants: self.constants.clone(),
            roots: self.roots.clone(),
            forgotten: self.forgotten.clone(),
            finished: self.finished.clone(),
            ended: self.ended,
            terminal: self.terminal.clone(),
            word: self.word,
            resolving: self.resolving,
            holders: self.holders.clone(),
            physical: self.physical.clone(),
            arena_no_drop: self.arena_no_drop.clone(),
            open_holes: self.open_holes.clone(),
            open_elements: self.open_elements.clone(),
            tasks: self.tasks.clone(),
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
                    | Inst::SliceView { .. }
                    | Inst::UniqueCopy { .. }
                    | Inst::Hash { .. }
                    | Inst::FrameEnter { .. } => true,
                    _ => false,
                }
            }
            None => false,
        }
    }

    fn report(&self, result: Checked<(Observation, crate::History<Exemption>)>) -> Run {
        Run {
            output: self.output.clone(),
            sites: self.sites,
            choices: self.choices,
            steps: self.steps,
            trace: self.ledger.trace.clone(),
            counters: self.ledger.counters,
            result: crate::blocked(
                result
                    .map(|(observation, exemptions)| Audit {
                        observation,
                        exemptions,
                    })
                    .map_err(|v| *v),
                &self.output,
                self.signaled,
            ),
        }
    }

    fn campaign_steps(&mut self, injected: &mut dyn FnMut(u64, Run)) -> Checked<()> {
        while !self.stack.is_empty() || self.in_child() {
            let site = self.sites;
            let checkpoint = self.checkpoint_site();
            if checkpoint {
                // A fork inherits a proof of the complete prefix, including
                // changes since the last geometric check. Refresh it once in
                // the base, rather than once per injected suffix.
                self.holders.sync(&mut self.ledger);
                // Retain discrepancies as well as successful proofs. A
                // prefix refresh must not move a finding from its original
                // audit boundary into the clean run before injection.
                self.refresh_holder_audit();
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
                    injected(site, fork.report(result));
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
    injected: &mut dyn FnMut(u64, Run),
) -> Run {
    SCANNED.with(|v| v.set(0));
    let started = std::time::Instant::now();
    let mut machine = Machine::new(program, config, ops);
    let result = (|| {
        program.check()?;
        machine.start(program.entry(entry)?, args)?;
        machine.campaign_steps(injected)?;
        machine.prepare_finish()?;
        machine.campaign_steps(injected)?;
        machine.finish()
    })();
    if std::env::var_os("HIRSEL_HEAPCHECK_PROFILE_CAMPAIGN").is_some() {
        eprintln!(
            "campaign-work steps={} sites={} scanned={} seconds={:.6}",
            machine.steps,
            machine.sites,
            SCANNED.with(|v| v.get()),
            started.elapsed().as_secs_f64()
        );
    }
    machine.report(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoOperations;
    use crate::build::{Layouts, fixture_unit, path};
    use hirsel_lir::layout_types::TargetSpec;
    use hirsel_lir::stage::Rc;
    use hirsel_lir::testing::UnitBuilder;

    #[derive(Clone, Copy, Debug)]
    enum Fault {
        LeakedHolder,
        DoubleRelease,
        StaleFailureCount,
        PrefixStaleFailureCount,
    }

    fn fault_control(fault: Fault, fixed: bool, reference: bool) -> (u64, u64, Checked<()>) {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let payload = layouts.scalar(ScalarKind::U64);
        let (target, layouts) = layouts.finish();
        let program = Program::<Rc> {
            target,
            layouts,
            units: vec![],
            statics: vec![],
            invocation_bodies: vec![],
            pretransfer_adapters: vec![],
            invocations: Default::default(),
        };
        let config = Config::default();
        let mut ops = NoOperations;
        let mut base = Machine::new(&program, &config, &mut ops);
        let p = base
            .ledger
            .alloc(Owner::Heap, Some(payload), 8, 8, 8, Some(Count::Finite(1)));
        let storage = base.ledger.alloc(Owner::Frame, None, 8, 8, 8, None);
        base.ledger.get_mut(storage.alloc).extent.put_cell(
            0,
            8,
            Cell::Data {
                ptr: p,
                counted: true,
                borrow: None,
            },
        );
        base.steps = 2;
        base.check_counts().unwrap();
        if matches!(fault, Fault::PrefixStaleFailureCount) {
            base.ledger
                .get_mut(storage.alloc)
                .extent
                .take(0, 8, FREE_POISON, None);
            if fixed {
                base.count_down(p).unwrap();
            } else {
                base.ledger
                    .get_mut(storage.alloc)
                    .extent
                    .holder_edits
                    .as_mut()
                    .unwrap()
                    .clear();
            }
            base.steps = 3;
            base.check_counts().unwrap();
            if !reference {
                base.holders.sync(&mut base.ledger);
                base.refresh_holder_audit();
            }
        }
        let base_count = base.ledger.get(p.alloc).count;
        let base_has_cell = base.ledger.get(storage.alloc).extent.cells.contains_key(&0);
        // The injected instruction does not touch the inherited holder or its
        // target. Failure cleanup must still audit the block it releases.
        let config = Config {
            inject: Some(0),
            full_recount_every_step: reference,
            ..config.clone()
        };
        let mut ops = NoOperations;
        let mut fork = base.fork(&config, &mut ops);
        fork.steps = base.steps + 1;
        assert!(fork.injection_site());
        assert!(std::sync::Arc::ptr_eq(
            &base.ledger.allocations[p.alloc.0 as usize],
            &fork.ledger.allocations[p.alloc.0 as usize]
        ));
        let result = (|| {
            fork.ledger
                .get_mut(storage.alloc)
                .extent
                .take(0, 8, FREE_POISON, None);
            match fault {
                Fault::LeakedHolder => {
                    if fixed {
                        fork.count_down(p)?;
                    }
                }
                Fault::DoubleRelease => {
                    fork.count_down(p)?;
                    if !fixed {
                        fork.count_down(p)?;
                    }
                }
                Fault::PrefixStaleFailureCount => {}
                Fault::StaleFailureCount => {
                    if fixed {
                        fork.count_down(p)?;
                    } else {
                        // Plant a missed cleanup write as well as its missing
                        // decrement. Fast tally and stale count still agree.
                        fork.ledger
                            .get_mut(storage.alloc)
                            .extent
                            .holder_edits
                            .as_mut()
                            .unwrap()
                            .clear();
                        assert!(std::sync::Arc::ptr_eq(
                            &base.ledger.allocations[p.alloc.0 as usize],
                            &fork.ledger.allocations[p.alloc.0 as usize]
                        ));
                    }
                }
            }
            fork.check_counts()
        })();
        // Fork-local physical writes and audit cache updates cannot affect the
        // clean prefix, including after a rejected fork.
        assert_eq!(base.ledger.get(p.alloc).count, base_count);
        assert_eq!(
            base.ledger.get(storage.alloc).extent.cells.contains_key(&0),
            base_has_cell
        );
        (fork.sites - 1, fork.steps, result)
    }

    fn check_fault(fault: Fault) {
        for fixed in [false, true] {
            let actual = fault_control(fault, fixed, false);
            let reference = fault_control(fault, fixed, true);
            assert_eq!(actual, reference, "{fault:?}, fixed={fixed}");
            assert_eq!(
                (actual.0, actual.1),
                (
                    0,
                    if matches!(fault, Fault::PrefixStaleFailureCount) {
                        4
                    } else {
                        3
                    }
                )
            );
            if fixed {
                actual.2.unwrap();
            } else {
                let expected = match fault {
                    Fault::LeakedHolder => V::CountMismatch,
                    Fault::DoubleRelease => V::TypeConfusion,
                    Fault::StaleFailureCount | Fault::PrefixStaleFailureCount => V::InternalAudit,
                };
                assert_eq!(actual.2.unwrap_err().kind, expected, "{fault:?}");
            }
        }
    }
    #[test]
    fn a_leaked_holder_keeps_its_injected_site() {
        check_fault(Fault::LeakedHolder);
    }
    #[test]
    fn a_double_release_keeps_its_injected_site() {
        check_fault(Fault::DoubleRelease);
    }
    #[test]
    fn a_stale_failure_count_in_an_inherited_object_keeps_its_site() {
        check_fault(Fault::StaleFailureCount);
    }

    #[test]
    fn a_cached_prefix_fault_keeps_its_original_injected_boundary() {
        check_fault(Fault::PrefixStaleFailureCount);
    }

    #[test]
    fn unchanged_dead_history_does_not_multiply_fork_audit_work() {
        let mut work = Vec::new();
        for n in [128, 8192] {
            let layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
            let (target, layouts) = layouts.finish();
            let program = Program::<Rc> {
                target,
                layouts,
                units: vec![],
                statics: vec![],
                invocation_bodies: vec![],
                pretransfer_adapters: vec![],
                invocations: Default::default(),
            };
            let config = Config::default();
            let mut ops = NoOperations;
            let mut base = Machine::new(&program, &config, &mut ops);
            for _ in 0..n {
                let p = base
                    .ledger
                    .alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(0)));
                base.ledger.free(p.alloc).unwrap();
            }
            base.steps = 2;
            base.check_counts().unwrap();
            SCANNED.with(|v| v.set(0));
            for _ in 0..16 {
                let config = Config {
                    inject: Some(0),
                    ..Config::default()
                };
                let mut ops = NoOperations;
                let mut fork = base.fork(&config, &mut ops);
                let p = fork
                    .ledger
                    .alloc(Owner::Heap, None, 8, 8, 8, Some(Count::Finite(1)));
                fork.set_root(
                    0,
                    Some(Held {
                        val: Val::Ptr(p),
                        credits: 1,
                        borrow: None,
                    }),
                );
                fork.steps = 3;
                assert!(fork.injection_site());
                fork.check_counts().unwrap();
                fork.set_root(0, None);
                fork.count_down(p).unwrap();
                fork.ledger.free(p.alloc).unwrap();
                fork.steps = 4;
                fork.check_counts().unwrap();
                fork.audit().unwrap();
            }
            work.push(SCANNED.with(|v| v.get()));
        }
        assert_eq!(
            work[0], work[1],
            "historical dead allocations must share their audit proof"
        );
        assert!(work[0] <= 64, "{work:?}");
    }

    #[test]
    fn scalar_site_classification_matches_runtime_operands_even_with_bad_metadata() {
        let mut layouts = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
        let float = layouts.scalar(ScalarKind::F64);
        let (target, table) = layouts.finish();
        let mut b = UnitBuilder::<Rc>::new(fixture_unit(path("main")));
        let block = b.block(BlockKind::Body, &[]);
        let fail_block = b.block(BlockKind::FailureCleanup, &[]);
        b.term(fail_block, Terminator::Propagate);
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
        b.term(block, Terminator::Return(None));
        let program = Program {
            invocations: Default::default(),
            invocation_bodies: Vec::new(),
            pretransfer_adapters: Vec::new(),
            target,
            layouts: table.clone(),
            units: vec![b.finish_with_layouts(&table)],
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
