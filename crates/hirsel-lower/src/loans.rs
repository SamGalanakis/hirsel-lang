//! Transport permission presence through feedback without duplicating loop IR.
//! These are emitted loan operations, not reconstructed source mode judgments.
use super::builder::Builder;
use hirsel_tir::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

#[derive(Clone, Default, PartialEq, Eq)]
struct Presence {
    may: BTreeSet<u32>,
    must: BTreeSet<u32>,
}
impl Presence {
    fn join(&mut self, other: &Self) -> bool {
        let before = self.clone();
        self.may.extend(&other.may);
        self.must.retain(|loan| other.must.contains(loan));
        *self != before
    }
    fn end(&mut self, loan: LoanId) {
        self.may.remove(&loan.0);
        self.must.remove(&loan.0);
    }
    fn cleanup(&mut self, body: &Body, cleanup: CleanupId) {
        for action in body.cleanup_actions(cleanup) {
            if let CleanupAction::EndLoan { loan, .. } = action {
                self.end(*loan);
            }
        }
    }
    fn instruction(&mut self, body: &Body, instruction: &InstructionKind) {
        match instruction {
            InstructionKind::BeginLoan { id, .. } => {
                self.may.insert(id.0);
                self.must.insert(id.0);
            }
            InstructionKind::EndLoan { loan, .. } => self.end(*loan),
            InstructionKind::RunCleanup(cleanup) => self.cleanup(body, *cleanup),
            _ => {}
        }
    }
}
pub(super) fn successors(body: &Body, terminator: &Terminator) -> Vec<BlockId> {
    match terminator {
        Terminator::Goto(next)
        | Terminator::Call {
            returns: Some(CallReturn { next, .. }),
            ..
        } => vec![*next],
        Terminator::Branch { yes, no, .. } => vec![*yes, *no],
        Terminator::Switch {
            arms, otherwise, ..
        } => arms
            .iter()
            .map(|(_, target)| *target)
            .chain([*otherwise])
            .collect(),
        Terminator::Match { table, from } => body.matches[table.0 as usize]
            .arms
            .iter()
            .skip(*from as usize)
            .map(|arm| arm.guard.unwrap_or(arm.target))
            .collect(),
        _ => vec![],
    }
}
#[derive(Default)]
struct CleanupTransport {
    // Each original chain node is rewritten once. Non-loan suffixes and the
    // immutable loan selection share their parent's result, as do the roots of
    // destroyed holders the chain drops.
    nodes: HashMap<
        CleanupNodeId,
        (
            Option<CleanupNodeId>,
            Arc<BTreeSet<u32>>,
            Arc<BTreeSet<u32>>,
        ),
    >,
}
impl Builder<'_, '_> {
    fn loan_cleanup(
        &mut self,
        transport: &mut CleanupTransport,
        original: CleanupId,
        state: &Presence,
        complete: bool,
    ) -> CleanupId {
        let original_head = self.body.cleanups[original.0 as usize].head;
        let mut pending = vec![];
        let mut cursor = original_head;
        while let Some(id) = cursor {
            if transport.nodes.contains_key(&id) {
                break;
            }
            pending.push(id);
            cursor = self.body.cleanup_nodes[id.0 as usize].parent;
        }
        let (mut head, mut loans, mut holders) = cursor
            .and_then(|id| transport.nodes.get(&id).cloned())
            .unwrap_or_default();
        while let Some(id) = pending.pop() {
            let action = self.body.cleanup_nodes[id.0 as usize].action.clone();
            match action {
                CleanupAction::EndLoan { loan, .. } => {
                    Arc::make_mut(&mut loans).insert(loan.0);
                }
                action => {
                    if let CleanupAction::Drop { place, .. } = &action {
                        let root = self.root(*place).0;
                        if self.destroyed_holders.contains_key(&root) {
                            Arc::make_mut(&mut holders).insert(root);
                        }
                    }
                    head = Some(self.cleanup_node(action, head))
                }
            }
            transport
                .nodes
                .insert(id, (head, loans.clone(), holders.clone()));
        }
        let selected: Vec<u32> = if complete { &state.may } else { &loans }
            .iter()
            .copied()
            .filter(|loan| state.may.contains(loan))
            .collect();
        let end = |loan: u32| CleanupAction::EndLoan {
            loan: LoanId(loan),
            if_active: !state.must.contains(&loan),
        };
        // A holder whose destruction is user-visible uses its loans when it is
        // dropped (§04), so those loans end after its drop. Others end first.
        let held_until: BTreeMap<u32, BTreeSet<u32>> = selected
            .iter()
            .map(|loan| {
                let roots: BTreeSet<u32> = holders
                    .iter()
                    .copied()
                    .filter(|root| self.destroyed_holders[root].contains(loan))
                    .collect();
                (*loan, roots)
            })
            .filter(|(_, roots)| !roots.is_empty())
            .collect();
        if !held_until.is_empty() {
            let mut actions = vec![];
            let mut cursor = head;
            while let Some(id) = cursor {
                let node = &self.body.cleanup_nodes[id.0 as usize];
                actions.push(node.action.clone());
                cursor = node.parent;
            }
            let mut ordered = vec![];
            // Child loans have later IDs, so they end first.
            for &loan in selected.iter().rev() {
                if !held_until.contains_key(&loan) {
                    ordered.push(end(loan));
                }
            }
            for (index, action) in actions.iter().enumerate() {
                ordered.push(action.clone());
                if let CleanupAction::Drop { place, .. } = action {
                    let root = self.root(*place).0;
                    for (&loan, roots) in held_until.iter().rev() {
                        let last = actions[index + 1..].iter().all(|later| {
                            !matches!(later, CleanupAction::Drop { place, .. }
                                if roots.contains(&self.root(*place).0))
                        });
                        if roots.contains(&root) && last {
                            ordered.push(end(loan));
                        }
                    }
                }
            }
            let mut head = None;
            for action in ordered.into_iter().rev() {
                head = Some(self.cleanup_node(action, head));
            }
            return self.cleanup_root(head);
        }
        // Child loans have later IDs, so prepend in creation order.
        for loan in selected {
            head = Some(self.cleanup_node(end(loan), head));
        }
        self.cleanup_root(head)
    }
    pub(super) fn complete_loan_flow(&mut self) {
        if self.loan_count == 0 {
            return;
        }
        let mut incoming = vec![None::<Presence>; self.body.blocks.len()];
        incoming[self.body.entry.0 as usize] = Some(Presence::default());
        let mut pending = VecDeque::from([self.body.entry]);
        let mut queued = vec![false; self.body.blocks.len()];
        queued[self.body.entry.0 as usize] = true;
        while let Some(block) = pending.pop_front() {
            let index = block.0 as usize;
            queued[index] = false;
            let mut state = incoming[index].clone().unwrap();
            for instruction in &self.body.blocks[index].instructions {
                state.instruction(&self.body, &instruction.kind);
            }
            for target in successors(&self.body, &self.body.blocks[index].terminator) {
                let changed = if let Some(previous) = &mut incoming[target.0 as usize] {
                    previous.join(&state)
                } else {
                    incoming[target.0 as usize] = Some(state.clone());
                    true
                };
                if changed && !queued[target.0 as usize] {
                    queued[target.0 as usize] = true;
                    pending.push_back(target);
                }
            }
        }
        let mut cleanups = CleanupTransport::default();
        for (index, incoming) in incoming.into_iter().enumerate() {
            let Some(mut state) = incoming else { continue };
            let instructions = std::mem::take(&mut self.body.blocks[index].instructions);
            let mut output = vec![];
            for mut instruction in instructions {
                let keep = match &mut instruction.kind {
                    InstructionKind::BeginLoan {
                        id,
                        mutable,
                        if_inactive,
                        ..
                    } => {
                        if !*mutable && state.must.contains(&id.0) {
                            false
                        } else {
                            *if_inactive = !*mutable && state.may.contains(&id.0);
                            true
                        }
                    }
                    InstructionKind::EndLoan { loan, if_active } => {
                        *if_active = !state.must.contains(&loan.0);
                        state.may.contains(&loan.0)
                    }
                    InstructionKind::RunCleanup(cleanup) => {
                        *cleanup = self.loan_cleanup(&mut cleanups, *cleanup, &state, false);
                        true
                    }
                    _ => true,
                };
                if keep {
                    state.instruction(&self.body, &instruction.kind);
                    instruction.on_failure =
                        self.loan_cleanup(&mut cleanups, instruction.on_failure, &state, true);
                    output.push(instruction);
                }
            }
            self.body.blocks[index].instructions = output;
            let cleanup = match self.body.blocks[index].terminator {
                Terminator::Call { on_failure, .. } => Some(on_failure),
                Terminator::Return { cleanup, .. } | Terminator::Fail { cleanup, .. } => {
                    Some(cleanup)
                }
                _ => None,
            };
            if let Some(cleanup) = cleanup {
                let complete = self.loan_cleanup(&mut cleanups, cleanup, &state, true);
                match &mut self.body.blocks[index].terminator {
                    Terminator::Call { on_failure, .. } => *on_failure = complete,
                    Terminator::Return { cleanup, .. } | Terminator::Fail { cleanup, .. } => {
                        *cleanup = complete
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
}
