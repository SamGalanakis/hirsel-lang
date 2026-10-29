//! Independent pattern-matrix coverage and guard-entry validation.
//! Specialization follows the definition of usefulness: split finite constructors,
//! expand wildcard rows, and use the default matrix for an open scalar domain.
//! This module consumes only IR and declarations, never H's coverage judgment.
use super::*;
use hirsel_types::{BuiltinType, GenericArgument, Primitive, SignatureKind};

/// Deterministic work budget, independent of machine speed (spec #33).
pub const MAX_MATCH_WORK: u64 = 2_000_000;
/// Same deterministic operational tooling boundary as parser nesting exhaustion.
pub fn tooling_limit_error(report: &Validation) -> Option<hirsel_types::QueryError> {
    report.violations.iter().find_map(|v| match v.kind {
        ViolationKind::TypeProofToolingLimit { .. } => Some(hirsel_types::type_proof_limit()),
        ViolationKind::MatchToolingLimit { limit } => Some(hirsel_types::QueryError::Operational(
            hirsel_types::OperationalFailure {
                operation: "match-tooling-limit".into(),
                message: format!(
                    "independent pattern coverage exceeded {limit} work units (spec #33)"
                ),
            },
        )),
        _ => None,
    })
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tag {
    Bool(bool),
    Tuple,
    Record,
    Variant(u32),
}
/// The order key of an integer or char constant of a range-pattern type.
fn range_key(c: &Constant, kind: &TypeKind) -> Option<u128> {
    match (c, kind) {
        (Constant::Integer(bits), TypeKind::Primitive(p)) => p.pattern_key(*bits),
        _ => None,
    }
}
/// The key interval a literal or range pattern covers on a scalar place.
fn interval(pattern: &MatchPatternKind, kind: &TypeKind) -> Option<(u128, u128)> {
    match pattern {
        MatchPatternKind::Literal(c) => range_key(c, kind).map(|k| (k, k)),
        MatchPatternKind::Range { start, end } => {
            Some((range_key(start, kind)?, range_key(end, kind)?))
        }
        _ => None,
    }
}
/// The domain of a scalar column split at every row interval's bounds; each
/// piece lies wholly inside or outside each interval.
fn scalar_pieces(domain: Vec<(u128, u128)>, intervals: &[(u128, u128)]) -> Vec<(u128, u128)> {
    let mut cuts: Vec<u128> = intervals
        .iter()
        .flat_map(|(low, high)| [Some(*low), high.checked_add(1)])
        .flatten()
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut pieces = vec![];
    for (from, to) in domain {
        let mut low = from;
        for cut in cuts.iter().filter(|c| **c > from && **c <= to) {
            pieces.push((low, cut - 1));
            low = *cut;
        }
        pieces.push((low, to));
    }
    pieces
}
fn tag(pattern: &MatchPatternKind) -> Option<Tag> {
    match pattern {
        MatchPatternKind::Literal(Constant::Bool(b)) => Some(Tag::Bool(*b)),
        MatchPatternKind::Literal(Constant::Unit) => Some(Tag::Tuple),
        MatchPatternKind::Tuple(_) => Some(Tag::Tuple),
        MatchPatternKind::Record { .. } => Some(Tag::Record),
        MatchPatternKind::Variant { variant, .. } => Some(Tag::Variant(*variant)),
        _ => None,
    }
}
fn wildcard(table: &ReadMatch<'_>, cell: Option<MatchPatternId>) -> bool {
    cell.is_none_or(|id| {
        matches!(
            table.patterns[id.0 as usize].kind,
            MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }
        )
    })
}
impl Checker<'_, '_> {
    pub(super) fn match_tables(&mut self) {
        for table in &self.body.matches {
            let errors_before = self.errors.len();
            let store = self.t;
            let mut guarded = BTreeSet::new();
            let mut pending: Vec<_> = table
                .arms
                .iter()
                .filter(|a| a.guard.is_some())
                .map(|a| a.pattern)
                .collect();
            while let Some(id) = pending.pop() {
                if guarded.insert(id.0) {
                    pending.extend(
                        table.patterns[id.0 as usize]
                            .kind
                            .fields()
                            .iter()
                            .map(|(_, child)| *child),
                    );
                }
            }
            for (index, pattern) in table.patterns.iter().enumerate() {
                let ty = self.place_type(pattern.place);
                let kind = if matches!(
                    pattern.kind,
                    MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }
                ) {
                    None
                } else {
                    self.layout(store, ty)
                };
                let valid = match (&pattern.kind, &kind) {
                    (MatchPatternKind::Wildcard, _) => true,
                    (MatchPatternKind::Binding { usage, guard_usage }, _) => {
                        (!matches!(guard_usage, UseKind::Take | UseKind::Mut)
                            || !guarded.contains(&(index as u32)))
                            && (*usage != UseKind::Copy || self.plain_or_copyable(ty))
                    }
                    (MatchPatternKind::Literal(c), Some(k)) => crate::typing::constant_valid(c, k),
                    (MatchPatternKind::Range { start, end }, Some(k)) => {
                        crate::typing::constant_valid(start, k)
                            && crate::typing::constant_valid(end, k)
                            && matches!(
                                (range_key(start, k), range_key(end, k)),
                                (Some(low), Some(high)) if low <= high
                            )
                    }
                    (MatchPatternKind::Tuple(fields), Some(TypeKind::Tuple(ts))) => {
                        fields.len() == ts.len()
                            && fields.iter().all(|(f, _)| (*f as usize) < ts.len())
                    }
                    (
                        MatchPatternKind::Record { definition, fields },
                        Some(TypeKind::Record(fs)),
                    ) => {
                        self.pattern_owner_matches(store, ty, definition)
                            && fields.iter().all(|(f, _)| (*f as usize) < fs.len())
                    }
                    (
                        MatchPatternKind::Variant {
                            definition,
                            variant,
                            fields,
                        },
                        Some(TypeKind::Variant(vs)),
                    ) => {
                        self.pattern_owner_matches(store, ty, definition)
                            && vs.get(*variant as usize).is_some_and(|v| {
                                fields.iter().all(|(f, _)| (*f as usize) < v.fields.len())
                            })
                    }
                    _ => false,
                };
                if !valid {
                    self.error(ViolationKind::TypeMismatch {
                        entity: "match pattern",
                    });
                }
                for (ordinal, child) in pattern.kind.fields() {
                    let parent = &self.body.places[pattern.place.0 as usize];
                    let child_pattern = &table.patterns[child.0 as usize];
                    let child_place = child_pattern.place;
                    let child = &self.body.places[child_place.0 as usize];
                    let expected = match pattern.kind {
                        MatchPatternKind::Variant { variant, .. } => Projection::VariantField {
                            variant,
                            field: *ordinal,
                        },
                        _ => Projection::Field(*ordinal),
                    };
                    let mut expected_path = parent.projection.clone();
                    expected_path.push(expected);
                    let probe = Place {
                        projection: expected_path.clone(),
                        ..parent.clone()
                    };
                    let mut ty = self.projection_type(store, &probe, |_, _| None);
                    let inspect = !matches!(
                        child_pattern.kind,
                        MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }
                    );
                    if inspect {
                        while let Some(TypeKind::Box(payload)) =
                            ty.and_then(|ty| self.layout(store, ty))
                        {
                            expected_path.push(Projection::Field(0));
                            ty = Some(payload);
                        }
                    }
                    if parent.root != child.root
                        || child.projection != expected_path
                        || parent.loan != child.loan
                    {
                        self.error(ViolationKind::InvalidProjection(child_place));
                    }
                }
            }
            for arm in &table.arms {
                let mut pending = vec![arm.pattern];
                while let Some(id) = pending.pop() {
                    let node = &table.patterns[id.0 as usize];
                    if let MatchPatternKind::Binding { usage, guard_usage } = node.kind {
                        if !self.binding_phase(arm.target, node.place, usage, false)
                            || arm.guard.is_some_and(|guard| {
                                !self.binding_phase(guard, node.place, guard_usage, true)
                            })
                        {
                            self.error(ViolationKind::InvalidUse(node.place));
                        }
                    }
                    pending.extend(node.kind.fields().iter().map(|(_, child)| *child));
                }
                let root =
                    &self.body.places[table.patterns[arm.pattern.0 as usize].place.0 as usize];
                let scrutinee = &self.body.places[table.scrutinee.0 as usize];
                let node = &table.patterns[arm.pattern.0 as usize];
                let mut expected_path = scrutinee.projection.clone();
                let mut ty = self.place_type(table.scrutinee);
                if !matches!(
                    node.kind,
                    MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }
                ) {
                    while let Some(TypeKind::Box(payload)) = self.layout(store, ty) {
                        expected_path.push(Projection::Field(0));
                        ty = payload;
                    }
                }
                if root.root != scrutinee.root
                    || root.projection != expected_path
                    || root.loan != scrutinee.loan
                {
                    self.error(ViolationKind::InvalidProjection(
                        table.patterns[arm.pattern.0 as usize].place,
                    ));
                }
            }
            if self.errors.len() != errors_before {
                continue;
            }
            match self.coverage(table, store) {
                Ok(true) => (),
                Ok(false) => self.error(ViolationKind::MatchCoverage),
                Err(()) => self.error(ViolationKind::MatchToolingLimit {
                    limit: MAX_MATCH_WORK,
                }),
            }
        }
    }
    fn binding_phase(&self, entry: BlockId, place: PlaceId, usage: UseKind, guard: bool) -> bool {
        let same = |id: PlaceId| {
            let a = &self.body.places[place.0 as usize];
            let b = &self.body.places[id.0 as usize];
            a.root == b.root && a.projection == b.projection
        };
        let mut mutable = false;
        for instruction in &self.body.blocks[entry.0 as usize].instructions {
            match instruction.kind {
                InstructionKind::BeginLoan {
                    place,
                    mutable: is_mut,
                    ..
                } if same(place) => mutable |= is_mut,
                InstructionKind::Assign { value, .. } => {
                    if let Rvalue::Use(operand) = &self.body.rvalues[value.0 as usize] {
                        if same(operand.place) {
                            return if guard {
                                usage == UseKind::Copy && operand.usage == usage
                            } else {
                                operand.usage == usage
                                    || (usage == UseKind::Copy
                                        && operand.usage == UseKind::Take
                                        && self.plain_or_copyable(self.place_type(place)))
                            };
                        }
                    }
                }
                InstructionKind::BindOrigin {
                    source,
                    place: alias,
                    ..
                } if same(source) && same(alias) => {
                    return if guard {
                        !mutable && matches!(usage, UseKind::Borrow | UseKind::Copy)
                    } else {
                        usage == UseKind::Borrow || (usage == UseKind::Mut && mutable)
                    };
                }
                _ => (),
            }
        }
        false
    }
    fn pattern_owner_matches(&self, store: &TypeInterner, ty: TypeId, owner: &DefPath) -> bool {
        if let Some(found) = self.pattern_owner(store, ty) {
            return &found == owner;
        }
        // Independently manufactured structural fixtures retain an exact public
        // representation. A resource can never be reconstructed by erasing identity.
        let Some(signature) = self.signatures.get(owner) else {
            return false;
        };
        let SignatureKind::Type(td) = &signature.kind else {
            return false;
        };
        let ty = match &store.get(ty).unwrap().kind {
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(payload)] => *payload,
                _ => return false,
            },
            _ => ty,
        };
        !td.resource
            && td.drop.is_none()
            && td
                .representation
                .is_some_and(|repr| store.shape_eq(ty, repr).unwrap_or(false))
    }
    fn pattern_owner(&self, store: &TypeInterner, ty: TypeId) -> Option<DefPath> {
        match &store.get(ty)?.kind {
            TypeKind::Named { definition, .. } => Some(definition.path().clone()),
            TypeKind::Builtin {
                kind: BuiltinType::Packed,
                arguments,
            } => match arguments.as_slice() {
                [GenericArgument::Type(ty)] => self.pattern_owner(store, *ty),
                _ => None,
            },
            _ => None,
        }
    }
    fn plain_or_copyable(&self, ty: TypeId) -> bool {
        hirsel_types::structural_property(
            self.t,
            ty,
            hirsel_types::StructuralProperty::Copyable,
            &self.generic_declarations(),
            &mut |p| {
                Ok::<_, hirsel_types::QueryError>(
                    self.signatures.get(p).cloned().map(std::sync::Arc::new),
                )
            },
        )
        .unwrap_or(false)
    }
    /// A nonzero restart must follow the false edge of this arm's own guard.
    /// The guard dominates that branch; only cleanup/Goto blocks lie between
    /// the branch and commit/restart. Thus earlier unguarded matches were already
    /// excluded, and whole-table coverage proves the remaining domain covered.
    pub(super) fn match_start(&mut self, id: MatchTableId, from: u32) {
        if from == 0 {
            return;
        }
        let arm = &self.body.matches[id.0 as usize].arms[from as usize - 1];
        let restart = self.site.block.unwrap();
        let Some(guard) = arm.guard else {
            self.error(ViolationKind::MatchCoverage);
            return;
        };
        let adjacent: Vec<Vec<usize>> = self
            .body
            .blocks
            .iter()
            .map(|b| {
                self.body
                    .successors(&b.terminator)
                    .iter()
                    .map(|b| b.0 as usize)
                    .collect()
            })
            .collect();
        let dom = hirsel_graph::Dominators::new(&adjacent, self.body.entry.0 as usize);
        // Dominance of a dead target is vacuous in this guard proof.
        let dominates = |a: BlockId, b: BlockId| {
            !dom.reachable(b.0 as usize) || dom.dominates(a.0 as usize, b.0 as usize)
        };
        let valid = self.body.blocks.iter().enumerate().any(|(index, b)| {
            let Terminator::Branch { yes, no, .. } = b.terminator else { return false; };
            yes != no && self.cleanup_reaches(yes, arm.target) && self.failed_guard_reaches(BlockId(index as u32), no, restart)
                && dominates(guard, BlockId(index as u32))
                && self.guard_preserves_scrutinee(guard, BlockId(index as u32), self.body.matches[id.0 as usize].scrutinee)
                && dominates(BlockId(index as u32), restart)
                && guard != self.body.entry
                && self.body.blocks.iter().enumerate().filter(|(_, b)| self.body.successors(&b.terminator).contains(&guard)).all(|(_, b)| matches!(b.terminator, Terminator::Match { table, from: start } if table == id && start < from))
        });
        // No unrelated predecessor may bypass the failed-guard branch.
        if !valid {
            self.error(ViolationKind::MatchCoverage);
        }
    }
    /// Coverage of a suffix assumes the selected value has not changed. Only
    /// paths continuing to the guard decision matter; failure/return paths exit.
    fn guard_preserves_scrutinee(
        &self,
        guard: BlockId,
        decision: BlockId,
        scrutinee: PlaceId,
    ) -> bool {
        let root = self.body.places[scrutinee.0 as usize].root;
        let touches = |place: PlaceId| self.body.places[place.0 as usize].root == root;
        let consumes = |operand: &Operand| {
            touches(operand.place) && matches!(operand.usage, UseKind::Take | UseKind::Mut)
        };
        let mut pending = vec![decision];
        let mut seen = BTreeSet::new();
        while let Some(block) = pending.pop() {
            if !seen.insert(block.0) {
                continue;
            }
            let block_data = &self.body.blocks[block.0 as usize];
            for instruction in &block_data.instructions {
                let changes = match &instruction.kind {
                    InstructionKind::StorageLive(local) => *local == root,
                    InstructionKind::StorageDead { place, .. }
                    | InstructionKind::Drop { place, .. } => touches(*place),
                    InstructionKind::BeginLoan { place, mutable, .. } => {
                        *mutable && touches(*place)
                    }
                    InstructionKind::Assign { destination, value } => {
                        touches(*destination)
                            || typing::operands(&self.body.rvalues[value.0 as usize])
                                .into_iter()
                                .any(consumes)
                    }
                    InstructionKind::Snapshot { destination, value } => {
                        touches(*destination) || consumes(value)
                    }
                    InstructionKind::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => touches(*destination) || touches(*displaced) || consumes(value),
                    InstructionKind::Check { condition, .. } => consumes(condition),
                    InstructionKind::EndScope(scope) => {
                        self.body.locals[root.0 as usize].scope == *scope
                    }
                    InstructionKind::RunCleanup(cleanup) => self
                        .body
                        .cleanup_actions(*cleanup)
                        .any(|a| matches!(a,CleanupAction::Drop {place,..} if touches(*place))),
                    _ => false,
                };
                if changes {
                    return false;
                }
            }
            let changes = match &block_data.terminator {
                Terminator::Branch { condition, .. } => consumes(condition),
                Terminator::Switch { value, .. } => consumes(value),
                Terminator::Call {
                    arguments, returns, ..
                } => {
                    returns.is_some_and(|r| touches(r.destination))
                        || arguments.iter().any(|a| consumes(&a.value))
                }
                _ => false,
            };
            if changes {
                return false;
            }
            if block != guard {
                pending.extend(
                    self.body
                        .blocks
                        .iter()
                        .enumerate()
                        .filter_map(|(index, b)| {
                            self.body
                                .successors(&b.terminator)
                                .contains(&block)
                                .then_some(BlockId(index as u32))
                        }),
                );
            }
        }
        true
    }
    fn failed_guard_reaches(&self, branch: BlockId, mut block: BlockId, target: BlockId) -> bool {
        let mut previous = branch;
        let mut seen = BTreeSet::new();
        while seen.insert(block.0) {
            // Every entry to the continuation must cross this false edge.
            if self.body.blocks.iter().enumerate().any(|(index, b)| {
                BlockId(index as u32) != previous
                    && self.body.successors(&b.terminator).contains(&block)
            }) {
                return false;
            }
            let b = &self.body.blocks[block.0 as usize];
            if b.instructions.iter().any(|i| {
                !matches!(
                    i.kind,
                    InstructionKind::RunCleanup(_)
                        | InstructionKind::EndLoan { .. }
                        | InstructionKind::EndOrigin(_)
                        | InstructionKind::EndScope(_)
                        | InstructionKind::StorageDead { .. }
                )
            }) {
                return false;
            }
            if block == target {
                return true;
            }
            let Terminator::Goto(next) = b.terminator else {
                return false;
            };
            previous = block;
            block = next;
        }
        false
    }
    fn cleanup_reaches(&self, mut block: BlockId, target: BlockId) -> bool {
        let mut seen = BTreeSet::new();
        while block != target && seen.insert(block.0) {
            let b = &self.body.blocks[block.0 as usize];
            if b.instructions.iter().any(|i| {
                !matches!(
                    i.kind,
                    InstructionKind::RunCleanup(_)
                        | InstructionKind::EndLoan { .. }
                        | InstructionKind::EndOrigin(_)
                        | InstructionKind::EndScope(_)
                        | InstructionKind::StorageDead { .. }
                )
            }) {
                return false;
            }
            let Terminator::Goto(next) = b.terminator else {
                return false;
            };
            block = next;
        }
        block == target
    }
    fn coverage(&mut self, table: &ReadMatch<'_>, store: &TypeInterner) -> Result<bool, ()> {
        let rows: Vec<Vec<Option<MatchPatternId>>> = table
            .arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| vec![Some(a.pattern)])
            .collect();
        let mut pending = vec![(vec![self.place_type(table.scrutinee)], rows)];
        let mut work = 0u64;
        while let Some((types, matrix)) = pending.pop() {
            work += 1 + matrix.iter().map(|r| r.len() as u64).sum::<u64>();
            if work > MAX_MATCH_WORK {
                work::count_by(Counter::ValidateMatchWork, work);
                return Err(());
            }
            if matrix
                .iter()
                .any(|row| row.iter().all(|cell| wildcard(table, *cell)))
            {
                continue;
            }
            if matrix.is_empty() {
                work::count_by(Counter::ValidateMatchWork, work);
                return Ok(false);
            }
            let Some((head, tail)) = types.split_first() else {
                continue;
            };
            if matrix.iter().all(|row| wildcard(table, row[0])) {
                pending.push((
                    tail.to_vec(),
                    matrix.into_iter().map(|r| r[1..].to_vec()).collect(),
                ));
                continue;
            }
            let mut inspected = *head;
            while let Some(TypeKind::Box(payload)) = self.layout(store, inspected) {
                inspected = payload;
            }
            let layout = self.layout(store, inspected);
            if let Some(kind @ TypeKind::Primitive(p)) = &layout
                && let Some(domain) = p.pattern_domain()
            {
                let intervals: Vec<_> = matrix
                    .iter()
                    .filter_map(|row| {
                        row[0].and_then(|id| interval(&table.patterns[id.0 as usize].kind, kind))
                    })
                    .collect();
                if !intervals.is_empty() {
                    // Pieces kept by the same rows specialize identically.
                    let mut seen = BTreeSet::new();
                    for (low, high) in scalar_pieces(domain, &intervals) {
                        let kept: Vec<usize> = matrix
                            .iter()
                            .enumerate()
                            .filter(|(_, row)| {
                                wildcard(table, row[0])
                                    || interval(
                                        &table.patterns[row[0].unwrap().0 as usize].kind,
                                        kind,
                                    )
                                    .is_some_and(|(a, b)| a <= low && high <= b)
                            })
                            .map(|(index, _)| index)
                            .collect();
                        if seen.insert(kept.clone()) {
                            let specialized = kept
                                .iter()
                                .map(|index| matrix[*index][1..].to_vec())
                                .collect();
                            pending.push((tail.to_vec(), specialized));
                        }
                    }
                    continue;
                }
            }
            let domain = match layout {
                Some(TypeKind::Primitive(Primitive::Bool)) => {
                    Some(vec![(Tag::Bool(false), vec![]), (Tag::Bool(true), vec![])])
                }
                Some(TypeKind::Tuple(ts)) => Some(vec![(Tag::Tuple, ts.to_vec())]),
                Some(TypeKind::Record(fs)) => {
                    Some(vec![(Tag::Record, fs.into_iter().map(|f| f.ty).collect())])
                }
                Some(TypeKind::Variant(vs)) => Some(
                    vs.into_iter()
                        .enumerate()
                        .map(|(v, variant)| {
                            (
                                Tag::Variant(v as u32),
                                variant.fields.into_iter().map(|(_, ty)| ty).collect(),
                            )
                        })
                        .collect(),
                ),
                _ => None,
            };
            match domain {
                None => pending.push((
                    tail.to_vec(),
                    matrix
                        .into_iter()
                        .filter(|r| wildcard(table, r[0]))
                        .map(|r| r[1..].to_vec())
                        .collect(),
                )),
                Some(constructors) => {
                    for (constructor, arguments) in constructors {
                        let mut specialized = vec![];
                        for row in &matrix {
                            let mut cells = vec![None; arguments.len()];
                            if !wildcard(table, row[0]) {
                                let p = &table.patterns[row[0].unwrap().0 as usize];
                                if tag(&p.kind) != Some(constructor) {
                                    continue;
                                }
                                for (field, child) in p.kind.fields() {
                                    cells[*field as usize] = Some(*child);
                                }
                            }
                            cells.extend_from_slice(&row[1..]);
                            specialized.push(cells);
                        }
                        let mut next = arguments;
                        next.extend_from_slice(tail);
                        pending.push((next, specialized));
                    }
                }
            }
        }
        work::count_by(Counter::ValidateMatchWork, work);
        Ok(true)
    }
}
