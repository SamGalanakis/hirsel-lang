//! A loan lasts until the last use of every value holding it, on every path
//! (LANG-3), not to the end of its holder's scope. Construction ends loans
//! lexically; this pass ends each one where it stops being needed, and the
//! presence pass (`loans.rs`) then retires the lexical ends that no longer
//! apply. A loan is needed while some path reaches an access through it, a
//! read of content holding it on that path, or the destruction of such content when its
//! type has user-visible destruction (DROP-1/2), including failure cleanups.
use super::builder::Builder;
use hirsel_tir::*;
use hirsel_types::{TypeId, UseKind};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// One step of a content path. All elements of a sequence, map or set are one
/// place (OWN-3), so an element step names no index, as the IR checker's does.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Field(u32),
    Variant(u32, u32),
    Element,
}
impl Part {
    fn path<'a>(projection: impl IntoIterator<Item = &'a Projection>) -> Vec<Self> {
        projection
            .into_iter()
            .map(|step| match step {
                Projection::Field(field) => Self::Field(*field),
                Projection::VariantField { variant, field } => Self::Variant(*variant, *field),
                Projection::Index(_) => Self::Element,
            })
            .collect()
    }
}

/// Loans held by installed content, by root and part path.
#[derive(Clone, Default, PartialEq, Eq)]
struct Content(BTreeMap<u32, Vec<(Vec<Part>, BTreeSet<u32>)>>);
impl Content {
    fn join(&mut self, other: &Self) -> bool {
        let before = self.clone();
        for (root, rows) in &other.0 {
            let joined = self.0.entry(*root).or_default();
            for (path, loans) in rows {
                if let Some((_, held)) = joined.iter_mut().find(|(p, _)| p == path) {
                    held.extend(loans);
                } else {
                    joined.push((path.clone(), loans.clone()));
                }
            }
        }
        *self != before
    }
    fn place(&self, place: &Place) -> Vec<(Vec<Part>, BTreeSet<u32>)> {
        let prefix = Part::path(place.projection.iter());
        let mut held = vec![];
        for (path, loans) in self.0.get(&place.root.0).into_iter().flatten() {
            if path.starts_with(&prefix) {
                held.push((path[prefix.len()..].to_vec(), loans.clone()));
            } else if prefix.starts_with(path) {
                held.push((vec![], loans.clone()));
            }
        }
        held
    }
    /// Every loan some installed content holds.
    fn loans(&self) -> BTreeSet<u32> {
        self.0
            .values()
            .flatten()
            .flat_map(|(_, loans)| loans.iter().copied())
            .collect()
    }
    fn clear(&mut self, place: &Place) {
        let prefix = Part::path(place.projection.iter());
        // One element's write or move leaves the others' loans held.
        if prefix.contains(&Part::Element) {
            return;
        }
        if let Some(rows) = self.0.get_mut(&place.root.0) {
            rows.retain(|(path, _)| !path.starts_with(&prefix));
        }
    }
    fn set(&mut self, place: &Place, held: Vec<(Vec<Part>, BTreeSet<u32>)>) {
        self.clear(place);
        let prefix = Part::path(place.projection.iter());
        let rows = self.0.entry(place.root.0).or_default();
        for (path, loans) in held {
            if loans.is_empty() {
                continue;
            }
            let path: Vec<_> = prefix.iter().cloned().chain(path).collect();
            if let Some((_, previous)) = rows.iter_mut().find(|(p, _)| *p == path) {
                previous.extend(loans);
            } else {
                rows.push((path, loans));
            }
        }
        if rows.is_empty() {
            self.0.remove(&place.root.0);
        }
    }
}

/// The loans one instruction or terminator needs active through its own
/// accesses and the content it installs, the places whose content it reads,
/// the places whose content it replaces, and the loan it begins.
#[derive(Default)]
struct Step {
    needs: BTreeSet<u32>,
    reads: Vec<PlaceId>,
    defines: Vec<PlaceId>,
    begins: Option<u32>,
    cursors: Vec<(PlaceId, BTreeSet<u32>)>,
}

/// What is needed at a point: loans needed through later accesses, and places
/// whose installed content a later step reads before replacing it.
#[derive(Clone, Default, PartialEq, Eq)]
struct Live {
    loans: BTreeSet<u32>,
    places: BTreeSet<u32>,
}
impl Live {
    fn of(step: &Step) -> Self {
        Self {
            loans: step.needs.clone(),
            places: step.reads.iter().map(|place| place.0).collect(),
        }
    }
    fn after(successors: &[usize], live_in: &[Live]) -> Self {
        let mut live = Self::default();
        for next in successors {
            live.loans.extend(&live_in[*next].loans);
            live.places.extend(&live_in[*next].places);
        }
        live
    }
}

impl Builder<'_, '_> {
    pub(super) fn end_loans_at_last_use(&mut self) {
        if self.loan_count == 0 {
            return;
        }
        let blocks = self.body.blocks.len();
        let successors: Vec<Vec<usize>> = self
            .body
            .blocks
            .iter()
            .map(|block| {
                self.body
                    .successors(&block.terminator)
                    .into_iter()
                    .map(|b| b.0 as usize)
                    .collect()
            })
            .collect();
        // Forward: the loans installed content holds at each block entry.
        let mut entry = vec![None::<Content>; blocks];
        entry[self.body.entry.0 as usize] = Some(Content::default());
        let mut pending = VecDeque::from([self.body.entry.0 as usize]);
        let mut queued = vec![false; blocks];
        queued[self.body.entry.0 as usize] = true;
        while let Some(block) = pending.pop_front() {
            queued[block] = false;
            let mut content = entry[block].clone().unwrap();
            for index in 0..self.body.blocks[block].instructions.len() {
                let kind = self.body.blocks[block].instructions[index].kind.clone();
                self.transfer_content(&mut content, &kind);
            }
            self.transfer_terminator(&mut content, block);
            for &next in &successors[block] {
                let changed = match &mut entry[next] {
                    Some(previous) => previous.join(&content),
                    slot => {
                        *slot = Some(content.clone());
                        true
                    }
                };
                if changed && !queued[next] {
                    queued[next] = true;
                    pending.push_back(next);
                }
            }
        }
        // What each step needs and reads, against the content before it.
        let mut steps: Vec<Vec<Step>> = Vec::with_capacity(blocks);
        for (block, state) in entry.iter().enumerate() {
            let Some(mut content) = state.clone() else {
                steps.push(vec![]);
                continue;
            };
            let mut row = vec![];
            for index in 0..self.body.blocks[block].instructions.len() {
                let instruction = self.body.blocks[block].instructions[index].clone();
                let mut step = self.instruction_step(&content, &instruction);
                let held = content.loans();
                self.transfer_content(&mut content, &instruction.kind);
                step.needs.extend(content.loans().difference(&held));
                row.push(step);
                self.record_destroyed_holders(&content);
            }
            let mut step = self.terminator_step(block);
            let held = content.loans();
            self.transfer_terminator(&mut content, block);
            step.needs.extend(content.loans().difference(&held));
            row.push(step);
            self.record_destroyed_holders(&content);
            steps.push(row);
        }
        // Backward: the loans and the places whose content is read after each
        // step. A place whose content is replaced is not read through its old
        // content, so a holder only one path fills needs its loans only there.
        let mut predecessors = vec![vec![]; blocks];
        for (block, next) in successors.iter().enumerate() {
            for &n in next {
                predecessors[n].push(block);
            }
        }
        let mut live_in = vec![Live::default(); blocks];
        let mut pending: VecDeque<_> = (0..blocks).rev().collect();
        let mut queued = vec![true; blocks];
        while let Some(block) = pending.pop_front() {
            queued[block] = false;
            let mut live = Live::after(&successors[block], &live_in);
            for step in steps[block].iter().rev() {
                self.live_before(&mut live, step);
            }
            if live != live_in[block] {
                live_in[block] = live;
                for &previous in &predecessors[block] {
                    if !queued[previous] {
                        queued[previous] = true;
                        pending.push_back(previous);
                    }
                }
            }
        }
        // The loans needed at each block's entry and exit, against the content
        // there: an exit reads its successors' places through its own content.
        let mut needed_in = vec![BTreeSet::new(); blocks];
        let mut exits = vec![BTreeSet::new(); blocks];
        for block in 0..blocks {
            let Some(mut content) = entry[block].clone() else {
                continue;
            };
            needed_in[block] = self.needed(&content, &live_in[block]);
            for index in 0..self.body.blocks[block].instructions.len() {
                let kind = self.body.blocks[block].instructions[index].kind.clone();
                self.transfer_content(&mut content, &kind);
            }
            let terminator = steps[block].last().unwrap();
            let mut exit = self.needed(&content, &Live::of(terminator));
            self.transfer_terminator(&mut content, block);
            exit.extend(self.needed(&content, &Live::after(&successors[block], &live_in)));
            exits[block] = exit;
        }
        // End each loan where it stops being needed. Ends of loans that are
        // inactive there are removed by the presence pass.
        for block in 0..blocks {
            // An exhaustive switch's impossible fallback stays empty.
            if entry[block].is_none()
                || self.body.blocks[block].terminator == Terminator::Unreachable
            {
                continue;
            }
            let mut after: Vec<Live> = vec![Live::default(); steps[block].len()];
            let mut live = Live::after(&successors[block], &live_in);
            for (index, step) in steps[block].iter().enumerate().rev() {
                after[index] = live.clone();
                self.live_before(&mut live, step);
            }
            let span = self.body.blocks[block].span;
            let instructions = std::mem::take(&mut self.body.blocks[block].instructions);
            let mut output = Vec::with_capacity(instructions.len());
            let entry_failure = instructions.first().map_or(CleanupId(0), |i| i.on_failure);
            // Loans a predecessor still needed on another of its edges.
            let incoming: BTreeSet<u32> = predecessors[block]
                .iter()
                .flat_map(|p| exits[*p].iter().copied())
                .collect();
            // Child loans have later IDs, so they end first.
            let ended: Vec<u32> = incoming.difference(&needed_in[block]).copied().collect();
            let mut content = entry[block].clone().unwrap();
            let cursors = self.cursor_holders(&content);
            // A loop body's iteration charge stays its first instruction (CT-4);
            // edge loan ends are mechanical and follow it.
            let charged = matches!(
                instructions.first().map(|i| &i.kind),
                Some(InstructionKind::Charge(_))
            );
            if charged {
                output.push(instructions[0].clone());
            }
            for &loan in ended.iter().rev() {
                self.retire_cursors(&mut output, &cursors, loan, span, entry_failure);
                output.push(Instruction {
                    kind: InstructionKind::EndLoan {
                        loan: LoanId(loan),
                        if_active: true,
                    },
                    span,
                    on_failure: entry_failure,
                });
            }
            let mut before = needed_in[block].clone();
            for (index, mut instruction) in instructions.into_iter().enumerate() {
                before.extend(steps[block][index].begins);
                self.transfer_content(&mut content, &instruction.kind);
                // The last instruction's successor is the terminator, which
                // reads against the content before it, as here.
                let needed = self.needed(&content, &after[index]);
                // Lexical cleanup can precede a holder's destruction: a
                // discarded call result ends evaluation loans before its
                // scoped callback temporary is dropped. Keep those loans
                // through every later use, including destruction and loop
                // feedback. Failure cleanup still ends all permissions.
                match &mut instruction.kind {
                    InstructionKind::EndLoan { loan, .. } if needed.contains(&loan.0) => {
                        continue;
                    }
                    InstructionKind::RunCleanup(cleanup) => {
                        *cleanup = self.defer_live_loans(*cleanup, &needed);
                    }
                    _ => {}
                }
                let ended: Vec<u32> = before.difference(&needed).copied().collect();
                let span = instruction.span;
                let on_failure = if ended.is_empty() {
                    instruction.on_failure
                } else {
                    self.retirement_cleanup(&instruction)
                };
                if !(charged && index == 0) {
                    output.push(instruction);
                }
                for loan in ended.iter().rev() {
                    self.retire_cursors(
                        &mut output,
                        &steps[block][index].cursors,
                        *loan,
                        span,
                        on_failure,
                    );
                    output.push(Instruction {
                        kind: InstructionKind::EndLoan {
                            loan: LoanId(*loan),
                            if_active: true,
                        },
                        span,
                        on_failure,
                    });
                }
                before = needed;
            }
            self.body.blocks[block].instructions = output;
        }
    }
    /// Retirement runs after the instruction succeeds. Its failure edge must
    /// include newly initialized owners, which the instruction's own failure
    /// edge deliberately excludes. Old taken/dropped roots remain conditional.
    fn retirement_cleanup(&mut self, instruction: &Instruction) -> CleanupId {
        let initialized = match instruction.kind {
            InstructionKind::Assign { destination, .. } => [Some(destination), None],
            InstructionKind::Replace {
                destination,
                displaced,
                ..
            } => [Some(destination), Some(displaced)],
            _ => return instruction.on_failure,
        };
        let mut slots: BTreeSet<_> = initialized
            .into_iter()
            .flatten()
            .filter(|place| {
                self.owned[self.root(*place).0 as usize] && !self.plain(self.ty(*place))
            })
            .map(|place| self.root(place).0)
            .collect();
        if slots.is_empty() {
            return instruction.on_failure;
        }
        let mut loans = vec![];
        let mut added = slots.clone();
        // Construction emits only whole-root drops and loan ends. Reuse its
        // DROP-2 ordering rather than appending an inner owner to an outer tail.
        for action in self.body.cleanup_actions(instruction.on_failure) {
            match *action {
                CleanupAction::Drop { place, .. } => {
                    let root = self.root(place).0;
                    added.remove(&root);
                    slots.insert(root);
                }
                CleanupAction::EndLoan { loan, .. } => loans.push((loan, ScopeId(0))),
                _ => unreachable!("lowering cleanup contains an unsupported action"),
            }
        }
        if added.is_empty() {
            return instruction.on_failure;
        }
        loans.reverse(); // cleanup_for prepends ends in creation order.
        self.cleanup_for(&slots, &loans)
    }
    fn defer_live_loans(&mut self, cleanup: CleanupId, needed: &BTreeSet<u32>) -> CleanupId {
        let actions: Vec<_> = self.body.cleanup_actions(cleanup).cloned().collect();
        if !actions.iter().any(|action| {
            matches!(action, CleanupAction::EndLoan { loan, .. } if needed.contains(&loan.0))
        }) {
            return cleanup;
        }
        let mut head = None;
        for action in actions.into_iter().rev() {
            if matches!(&action, CleanupAction::EndLoan { loan, .. } if needed.contains(&loan.0)) {
                continue;
            }
            head = Some(self.cleanup_node(action, head));
        }
        self.cleanup_root(head)
    }
    /// The places live before `step`, from those live after it.
    fn live_before(&self, live: &mut Live, step: &Step) {
        live.places.retain(|place| {
            !step
                .defines
                .iter()
                .any(|defined| self.replaces(*defined, PlaceId(*place)))
        });
        live.places.extend(step.reads.iter().map(|place| place.0));
        if let Some(loan) = step.begins {
            live.loans.remove(&loan);
        }
        live.loans.extend(&step.needs);
    }
    /// Whether writing `defined` replaces all the content a read of `place`
    /// uses. Only a whole root's write does: a part's write keeps the rows of
    /// its ancestors, which a read of any part below them still uses.
    fn replaces(&self, defined: PlaceId, place: PlaceId) -> bool {
        let defined = &self.body.places[defined.0 as usize];
        defined.projection.is_empty() && defined.root == self.body.places[place.0 as usize].root
    }
    /// The loans `live` needs against `content`: its own loans and those the
    /// content of its places holds, each with whatever its source's content
    /// holds.
    fn needed(&self, content: &Content, live: &Live) -> BTreeSet<u32> {
        let mut pending: Vec<u32> = live.loans.iter().copied().collect();
        for place in &live.places {
            for (_, loans) in content.place(&self.body.places[*place as usize]) {
                pending.extend(loans);
            }
        }
        let mut needs = BTreeSet::new();
        while let Some(loan) = pending.pop() {
            if !needs.insert(loan) {
                continue;
            }
            let source = &self.body.places[self.loan_source(LoanId(loan)).0 as usize];
            for (_, held) in content.place(source) {
                pending.extend(held.into_iter().filter(|l| !needs.contains(l)));
            }
        }
        needs
    }
    fn cursor_holders(&self, content: &Content) -> Vec<(PlaceId, BTreeSet<u32>)> {
        content
            .0
            .iter()
            .filter_map(|(&root, rows)| {
                self.borrowed_cursor_types
                    .contains(&self.body.locals[root as usize].ty)
                    .then(|| {
                        (
                            self.roots[root as usize],
                            rows.iter()
                                .flat_map(|(_, loans)| loans.iter().copied())
                                .collect(),
                        )
                    })
            })
            .collect()
    }
    fn retire_cursors(
        &self,
        output: &mut Vec<Instruction>,
        cursors: &[(PlaceId, BTreeSet<u32>)],
        ending: u32,
        span: hirsel_span::Span,
        on_failure: CleanupId,
    ) {
        for (place, loans) in cursors {
            if loans.contains(&ending) {
                output.push(Instruction {
                    kind: InstructionKind::Drop {
                        place: *place,
                        if_live: true,
                    },
                    span,
                    on_failure,
                });
            }
        }
    }
    fn record_destroyed_holders(&mut self, content: &Content) {
        for (root, rows) in &content.0 {
            let ty = self.body.locals[*root as usize].ty;
            // Failure cleanup still destroys the noncopyable cursor before
            // ending its permission. This does not make cleanup a normal-path
            // use for liveness: a silent cursor can retire before a source write.
            if self.destroyed(ty) || self.borrowed_cursor_types.contains(&ty) {
                let held = self.destroyed_holders.entry(*root).or_default();
                for (_, loans) in rows {
                    held.extend(loans);
                }
            }
        }
    }
    /// User-visible destruction (DROP-1/2), as the ownership pass judged it.
    fn destroyed(&self, ty: TypeId) -> bool {
        !self.borrowed_cursor_types.contains(&ty)
            && !self.input.ownership.silent_drop_types.contains(&ty)
    }
    /// The loans an access through this place requires: its loan and each
    /// loan that loan was taken through.
    fn access_chain(&self, place: PlaceId, needs: &mut BTreeSet<u32>) {
        let mut loan = self.body.places[place.0 as usize].loan;
        while let Some(id) = loan {
            if !needs.insert(id.0) {
                break;
            }
            loan = self.body.places[self.loan_source(id).0 as usize].loan;
        }
    }
    /// Reading a place uses its access chain and the content it holds; a loan
    /// is needed together with whatever its source's content holds (`needed`).
    fn read(&self, place: PlaceId, step: &mut Step) {
        self.access_chain(place, &mut step.needs);
        step.reads.push(place);
    }
    fn cleanup_uses(&self, cleanup: CleanupId, step: &mut Step) {
        for action in self.body.cleanup_actions(cleanup) {
            if let CleanupAction::Drop { place, .. } = action {
                if self.destroyed(self.ty(*place)) {
                    self.read(*place, step);
                }
            }
        }
    }
    fn instruction_step(&self, content: &Content, instruction: &Instruction) -> Step {
        let mut step = Step {
            cursors: self.cursor_holders(content),
            ..Step::default()
        };
        self.cleanup_uses(instruction.on_failure, &mut step);
        match &instruction.kind {
            InstructionKind::Defuse { place } => self.read(*place, &mut step),
            // Naming a value accesses its place without reading its content.
            InstructionKind::BindOrigin { source, place, .. } => {
                self.access_chain(*source, &mut step.needs);
                self.access_chain(*place, &mut step.needs);
            }
            InstructionKind::BindFields { source, fields } => {
                self.access_chain(*source, &mut step.needs);
                for field in fields {
                    self.access_chain(*field, &mut step.needs);
                }
            }
            InstructionKind::Assign { destination, value } => {
                self.access_chain(*destination, &mut step.needs);
                for operand in rvalue_operands(&self.body.rvalues[value.0 as usize]) {
                    self.read(operand.place, &mut step);
                }
                step.defines.push(*destination);
            }
            InstructionKind::Replace {
                destination,
                value,
                displaced,
                ..
            } => {
                self.access_chain(*destination, &mut step.needs);
                self.read(value.place, &mut step);
                if self.destroyed(self.ty(*destination)) {
                    self.read(*destination, &mut step);
                }
                self.access_chain(*displaced, &mut step.needs);
                step.defines.extend([*destination, *displaced]);
            }
            InstructionKind::BeginLoan { id, place, .. } => {
                self.read(*place, &mut step);
                step.begins = Some(id.0);
            }
            InstructionKind::Drop { place, .. } => {
                if self.destroyed(self.ty(*place)) {
                    self.read(*place, &mut step);
                }
            }
            InstructionKind::RunCleanup(cleanup) => self.cleanup_uses(*cleanup, &mut step),
            InstructionKind::Check { condition, .. } => self.read(condition.place, &mut step),
            InstructionKind::Snapshot { value, .. } => self.read(value.place, &mut step),
            InstructionKind::StorageLive(local) => {
                step.defines.push(self.roots[local.0 as usize]);
            }
            InstructionKind::EndOrigin(_)
            | InstructionKind::StorageDead { .. }
            | InstructionKind::EndScope(_)
            | InstructionKind::EndLoan { .. }
            | InstructionKind::Charge(_) => {}
        }
        step
    }
    fn terminator_step(&self, block: usize) -> Step {
        let mut step = Step::default();
        match &self.body.blocks[block].terminator {
            Terminator::Branch { condition, .. } => self.read(condition.place, &mut step),
            Terminator::Switch { value, .. } => self.read(value.place, &mut step),
            Terminator::Match { table, .. } => {
                let table = &self.body.matches[table.0 as usize];
                self.read(table.scrutinee, &mut step);
                for pattern in &table.patterns {
                    self.read(pattern.place, &mut step);
                }
            }
            Terminator::Call {
                callee,
                arguments,
                returns,
                on_failure,
            } => {
                if let Callee::Indirect { value, .. } = callee {
                    self.read(value.place, &mut step);
                }
                for argument in arguments {
                    self.read(argument.value.place, &mut step);
                }
                self.cleanup_uses(*on_failure, &mut step);
                step.defines.extend(returns.as_ref().map(|r| r.destination));
            }
            Terminator::Return { value, cleanup } => {
                self.read(value.place, &mut step);
                self.cleanup_uses(*cleanup, &mut step);
            }
            Terminator::Fail { cleanup, .. } => self.cleanup_uses(*cleanup, &mut step),
            Terminator::Goto(_) | Terminator::Unreachable => {}
        }
        step
    }
    /// The loans an operand hands to the value it initialises (construction's
    /// `operand_loans`, replayed over the content at this point).
    fn handed(&self, content: &Content, operand: &Operand) -> Vec<(Vec<Part>, BTreeSet<u32>)> {
        let place = &self.body.places[operand.place.0 as usize];
        if operand.usage == UseKind::Copy
            && !matches!(
                self.types.get(place.ty).unwrap().kind,
                hirsel_types::TypeKind::Function(_)
            )
        {
            return vec![];
        }
        let mut held = content.place(place);
        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
            let mut loans = BTreeSet::new();
            self.access_chain(operand.place, &mut loans);
            if !loans.is_empty() {
                held.push((vec![], loans));
            }
        }
        held
    }
    fn move_out(&self, content: &mut Content, operand: &Operand) {
        if operand.usage == UseKind::Take {
            content.clear(&self.body.places[operand.place.0 as usize]);
        }
    }
    fn transfer_content(&self, content: &mut Content, kind: &InstructionKind) {
        match kind {
            InstructionKind::StorageLive(local) => {
                content.0.remove(&local.0);
            }
            InstructionKind::Assign { destination, value } => {
                let rvalue = &self.body.rvalues[value.0 as usize];
                let mut held = vec![];
                match rvalue {
                    Rvalue::Use(value) => held = self.handed(content, value),
                    Rvalue::Unary {
                        op: hirsel_syntax::UnaryOp::Unique,
                        value,
                    } if matches!(
                        self.types.get(self.ty(*destination)).unwrap().kind,
                        hirsel_types::TypeKind::Function(_) | hirsel_types::TypeKind::Parameter(_)
                    ) =>
                    {
                        held = self.handed(content, value)
                    }
                    Rvalue::Construct {
                        constructor,
                        fields,
                        ..
                    } => {
                        for field in fields {
                            let prefix = match constructor {
                                Constructor::Variant { variant, .. } => {
                                    Part::Variant(*variant, field.field)
                                }
                                Constructor::Sequence | Constructor::Map | Constructor::Set => {
                                    Part::Element
                                }
                                Constructor::Tuple
                                | Constructor::Record(_)
                                | Constructor::SortedMap { .. }
                                | Constructor::Box => Part::Field(field.field),
                            };
                            held.extend(self.handed(content, &field.value).into_iter().map(
                                |(path, loans)| {
                                    (std::iter::once(prefix.clone()).chain(path).collect(), loans)
                                },
                            ));
                        }
                    }
                    Rvalue::Closure { captures, .. } => {
                        for capture in captures {
                            held.extend(self.handed(content, capture));
                        }
                    }
                    _ => {}
                }
                if self.owned[self.root(*destination).0 as usize]
                    && self.plain(self.ty(*destination))
                {
                    held.clear();
                }
                for operand in rvalue_operands(rvalue) {
                    self.move_out(content, operand);
                }
                content.set(&self.body.places[destination.0 as usize], held);
            }
            InstructionKind::Replace {
                destination,
                value,
                displaced,
                ..
            } => {
                let incoming = self.handed(content, value);
                let outgoing = content.place(&self.body.places[destination.0 as usize]);
                self.move_out(content, value);
                content.set(&self.body.places[destination.0 as usize], incoming);
                content.set(&self.body.places[displaced.0 as usize], outgoing);
            }
            _ => {}
        }
    }
    fn transfer_terminator(&self, content: &mut Content, block: usize) {
        // A diverging call has no normal successor for content to reach.
        let Terminator::Call {
            callee,
            arguments,
            returns: Some(CallReturn { destination, .. }),
            ..
        } = &self.body.blocks[block].terminator
        else {
            return;
        };
        let mut held = vec![];
        if let Some((parameters, environment, access)) = self.call_retention.get(&destination.0) {
            let fields = self.call_field_loans.get(&destination.0);
            let composed = self.call_composed_arguments.get(&destination.0);
            for argument in arguments {
                if parameters.contains(&argument.parameter.0) {
                    // A field-related result holds the field loan (#77).
                    let field = fields.and_then(|f| f.get(&argument.parameter.0));
                    let operand = field.map(|place| Operand {
                        place: *place,
                        usage: UseKind::Borrow,
                    });
                    // Relations compose through the pre-invocation view. The
                    // call's new carrier loan is only needed for the call.
                    let source = composed.and_then(|c| c.get(&argument.parameter.0));
                    held.extend(self.handed(
                        content,
                        operand.as_ref().or(source).unwrap_or(&argument.value),
                    ));
                }
            }
            if let (true, Callee::Indirect { value, .. }) = (environment, callee) {
                if *access {
                    held.extend(self.handed(content, value));
                } else {
                    held.extend(content.place(&self.body.places[value.place.0 as usize]));
                }
            }
        }
        if let Callee::Indirect { value, .. } = callee {
            self.move_out(content, value);
        }
        for argument in arguments {
            self.move_out(content, &argument.value);
        }
        content.set(&self.body.places[destination.0 as usize], held);
    }
}

fn rvalue_operands(rvalue: &Rvalue) -> Vec<&Operand> {
    match rvalue {
        Rvalue::Use(value) | Rvalue::Unary { value, .. } => vec![value],
        Rvalue::Binary { left, right, .. } => vec![left, right],
        Rvalue::Construct {
            constructor: Constructor::SortedMap { by },
            fields,
            ..
        } => std::iter::once(by)
            .chain(fields.iter().map(|field| &field.value))
            .collect(),
        Rvalue::Construct { fields, .. } => fields.iter().map(|field| &field.value).collect(),
        Rvalue::Closure { captures, .. } => captures.iter().collect(),
        _ => vec![],
    }
}
