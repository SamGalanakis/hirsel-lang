//! Byte fields built in place (LIR schema revision 24). A local byte list
//! that is only reserved and pushed, then frozen whole into a Bytes field
//! of an element a flat list builder appends, is never read as a list: only
//! the element's encoding of it is. Its bytes are written where the element
//! will lie, past the builder's used bytes, so the list has no block, its
//! freeze copies nothing and nothing releases it.
//!
//! ```text
//! let mut payload: List[u8] = []             // FlatField Open of `payload`
//! list.reserve(mut payload, length)          // FlatField Reserve
//! for j in 0u64..length {
//!   list.push(mut payload, byte(j))          // FlatField Push
//! }
//! list.push(mut items, Message(id: i, payload: bytes.from_list(take payload)))
//!                                            // FlatClose at the construction
//! ```
//!
//! The builder holds one open element at a time, so from the list's
//! initialization to the construction no path may write the builder
//! otherwise, initialize the list again or leave the body normally; a
//! failure exit releases the builder with its open element. The field must
//! follow only fixed fields of its record, so it lies at a fixed offset
//! while the bytes are written.
use super::*;
use hirsel_lir::ir::FlatFieldOp;
use hirsel_lir::layout_types::ScalarKind;
use std::collections::HashSet;

/// The byte fields built in place of one body.
#[derive(Default)]
pub(super) struct Fields {
    /// Every local of a byte list built in place: none has a slot.
    pub(super) locals: HashSet<tir::LocalId>,
    /// Each list's initialization, its builder's place and its field.
    pub(super) opens: HashMap<tir::LocalId, (tir::PlaceId, u32)>,
    /// Blocks whose call writes an open field: its builder, its field and
    /// whether it pushes (else it reserves).
    pub(super) writes: HashMap<tir::BlockId, (tir::PlaceId, u32, bool)>,
    /// Blocks whose call, the list's freeze, is removed.
    pub(super) freezes: HashSet<tir::BlockId>,
    /// Each element construction completing an open element, and its field.
    pub(super) closes: HashMap<tir::LocalId, u32>,
    /// The removed calls' mutable loans.
    pub(super) loans: HashSet<tir::LoanId>,
}

/// One candidate list while its uses are checked.
#[derive(Default)]
struct Web {
    bad: bool,
    /// Initializations: block, instruction and local.
    inits: Vec<(usize, usize, tir::LocalId)>,
    reserves: Vec<usize>,
    pushes: Vec<usize>,
    freezes: Vec<usize>,
    /// Element constructions taking it: block, instruction, head, field.
    consumers: Vec<(usize, usize, tir::LocalId, u32)>,
    loans: Vec<tir::LoanId>,
}

/// The places an instruction names.
fn instruction_places(body: &tir::Body, kind: &tir::InstructionKind) -> Vec<tir::PlaceId> {
    use tir::InstructionKind as K;
    let mut out = Vec::new();
    match kind {
        K::Defuse { place } | K::EndOrigin(place) | K::StorageDead { place, .. } => {
            out.push(*place)
        }
        K::Drop { place, .. } => out.push(*place),
        K::BindOrigin { source, place, .. } => out.extend([*source, *place]),
        K::BindFields { source, fields } => {
            out.push(*source);
            out.extend(fields.iter().copied());
        }
        K::Assign { destination, value } => {
            out.push(*destination);
            out.extend(rvalue_places(&body.rvalues[value.0 as usize]));
        }
        K::Replace {
            destination,
            value,
            displaced,
            ..
        } => out.extend([*destination, value.place, *displaced]),
        K::BeginLoan { place, .. } => out.push(*place),
        K::Check { condition, .. } => out.push(condition.place),
        K::Snapshot { destination, value } => out.extend([*destination, value.place]),
        K::StorageLive(_) | K::EndScope(_) | K::EndLoan { .. } | K::RunCleanup(_) => {}
    }
    out
}

fn rvalue_places(rvalue: &tir::Rvalue) -> Vec<tir::PlaceId> {
    match rvalue {
        tir::Rvalue::Use(op) | tir::Rvalue::Unary { value: op, .. } => vec![op.place],
        tir::Rvalue::Binary { left, right, .. } => vec![left.place, right.place],
        tir::Rvalue::Construct {
            constructor: tir::Constructor::SortedMap { by },
            fields,
            ..
        } => std::iter::once(by.place)
            .chain(fields.iter().map(|f| f.value.place))
            .collect(),
        tir::Rvalue::Construct { fields, .. } => fields.iter().map(|f| f.value.place).collect(),
        tir::Rvalue::Closure { captures, .. } => captures.iter().map(|c| c.place).collect(),
        tir::Rvalue::Constant(_)
        | tir::Rvalue::ReadConstant { .. }
        | tir::Rvalue::FunctionReference { .. }
        | tir::Rvalue::Size(_)
        | tir::Rvalue::Shape(_) => Vec::new(),
    }
}

/// The places a terminator names.
fn terminator_places(body: &tir::Body, terminator: &tir::Terminator) -> Vec<tir::PlaceId> {
    use tir::Terminator as T;
    match terminator {
        T::Call {
            callee,
            arguments,
            returns,
            ..
        } => {
            let mut out: Vec<_> = arguments.iter().map(|a| a.value.place).collect();
            if let tir::Callee::Indirect { value, .. } = callee {
                out.push(value.place);
            }
            out.extend(returns.iter().map(|r| r.destination));
            out
        }
        T::Branch { condition: v, .. }
        | T::Switch { value: v, .. }
        | T::Return { value: v, .. } => {
            vec![v.place]
        }
        T::Match { table, .. } => {
            let m = &body.matches[table.0 as usize];
            std::iter::once(m.scrutinee)
                .chain(m.patterns.iter().map(|p| p.place))
                .collect()
        }
        T::Goto(_) | T::Fail { .. } | T::Unreachable => Vec::new(),
    }
}

impl Importer<'_> {
    /// Whether a local of this type is a byte list (`Some(true)`) or the
    /// Bytes it freezes to (`Some(false)`).
    fn byte_kind(&mut self, ty: TypeId) -> Option<bool> {
        let layout = self.lir_type(ty).ok().map(|t| self.layout_of(t))?;
        let byte = |this: &Self, e: LayoutId| {
            this.layouts.record(e).kind == LayoutKind::Scalar(ScalarKind::U8)
        };
        match self.layouts.record(layout).kind {
            LayoutKind::Block { payload } => match self.layouts.record(payload).kind {
                LayoutKind::Sequence { element } if byte(self, element) => Some(true),
                _ => None,
            },
            LayoutKind::Slice { element } if byte(self, element) => Some(false),
            _ => None,
        }
    }

    /// Whether the field `field` of the flat record `record` can be built in
    /// place: a byte sequence after only fixed fields.
    fn open_field(&self, record: LayoutId, field: u32) -> bool {
        let r = self.layouts.record(record);
        r.kind == LayoutKind::Flat
            && r.fields
                .iter()
                .filter(|f| f.ordinal < field)
                .all(|f| flat_fixed(self.layouts, f.layout))
            && r.fields.iter().any(|f| {
                f.ordinal == field
                    && flat_prefixed(self.layouts, f.layout).is_some_and(|(o, e)| {
                        o == 8 && self.layouts.record(e).kind == LayoutKind::Scalar(ScalarKind::U8)
                    })
            })
    }

    /// The content record of the builder holding `list`.
    fn builder_record(&self, list: tir::PlaceId) -> Option<LayoutId> {
        let root = self.body.places[list.0 as usize].root;
        let builder = *self.builders.locals.get(&root)?;
        let LayoutKind::Block { payload } = self.layouts.record(builder).kind else {
            return None;
        };
        let LayoutKind::Sequential { element } = self.layouts.record(payload).kind else {
            return None;
        };
        match flat_prefixed(self.layouts, element) {
            Some((8, content)) => Some(content),
            _ => None,
        }
    }

    /// The byte fields of this body built in place; a list used outside the
    /// shapes above stays an ordinary list.
    pub(super) fn field_webs(&mut self) -> R<Fields> {
        use tir::InstructionKind as K;
        let body = self.body;
        if self.builders.heads.is_empty() {
            return Ok(Fields::default());
        }
        let n = body.locals.len();
        let elided = self.instances.elided.get(&self.id);
        let mut kind = vec![None; n];
        for (i, local) in body.locals.iter().enumerate() {
            let id = tir::LocalId(i as u32);
            if matches!(local.storage, tir::Storage::Local | tir::Storage::Temporary)
                && !elided.is_some_and(|e| e.contains(&id))
                && !self.builders.locals.contains_key(&id)
                && !self.builders.elements.contains(&id)
            {
                kind[i] = self.byte_kind(local.ty);
            }
        }
        if kind.iter().all(Option::is_none) {
            return Ok(Fields::default());
        }
        let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
        let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
        let path_of = |callee: &tir::Callee| match callee {
            tir::Callee::Definition { path, .. } => Some(path.clone()),
            tir::Callee::Indirect { .. } => None,
        };
        let (push, reserve, freeze) = (
            hirsel_syntax::DefPath::core("list.push"),
            hirsel_syntax::DefPath::core("list.reserve"),
            hirsel_syntax::DefPath::core("bytes.from_list"),
        );
        // Webs: lists and their Bytes joined by whole moves and the freeze.
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        let mut union = |parent: &mut Vec<usize>, a: usize, b: usize| {
            let (a, b) = (find(parent, a), find(parent, b));
            parent[a] = b;
        };
        for block in &body.blocks {
            for i in &block.instructions {
                if let K::Assign { destination, value } = &i.kind
                    && let tir::Rvalue::Use(op) = &body.rvalues[value.0 as usize]
                    && op.usage == UseKind::Take
                    && whole(*destination)
                    && whole(op.place)
                {
                    let (d, s) = (root(*destination), root(op.place));
                    if kind[d].is_some() && kind[d] == kind[s] {
                        union(&mut parent, d, s);
                    }
                }
            }
            if let tir::Terminator::Call {
                callee,
                arguments,
                returns: Some(r),
                ..
            } = &block.terminator
                && path_of(callee).as_ref() == Some(&freeze)
                && let [a] = arguments.as_slice()
            {
                let (l, b) = (root(a.value.place), root(r.destination));
                if kind[l] == Some(true) && kind[b] == Some(false) {
                    union(&mut parent, l, b);
                }
            }
        }
        let web_of: Vec<Option<usize>> = (0..n)
            .map(|l| kind[l].map(|_| find(&mut parent, l)))
            .collect();
        let on = |p: tir::PlaceId| web_of[root(p)];
        let mut webs: HashMap<usize, Web> = HashMap::new();
        for w in web_of.iter().flatten() {
            webs.entry(*w).or_default();
        }
        let bad = |webs: &mut HashMap<usize, Web>, w: Option<usize>| {
            if let Some(w) = w {
                webs.get_mut(&w).expect("web").bad = true;
            }
        };
        // A web local is only ever named whole, and defined once.
        for p in 0..body.places.len() {
            let p = tir::PlaceId(p as u32);
            if !whole(p) {
                bad(&mut webs, on(p));
            }
        }
        let mut definitions = vec![0usize; n];
        for (b, block) in body.blocks.iter().enumerate() {
            for (at, i) in block.instructions.iter().enumerate() {
                match &i.kind {
                    K::Assign { destination, value } => {
                        definitions[root(*destination)] += 1;
                        let d = on(*destination);
                        match &body.rvalues[value.0 as usize] {
                            tir::Rvalue::Use(op) => {
                                let s = on(op.place);
                                if !(op.usage == UseKind::Take && d.is_some() && d == s) {
                                    bad(&mut webs, s);
                                    bad(&mut webs, d);
                                }
                            }
                            tir::Rvalue::Construct {
                                constructor: tir::Constructor::Sequence,
                                fields,
                                ..
                            } if fields.is_empty() && d.is_some() => {
                                let local = tir::LocalId(root(*destination) as u32);
                                let w = d.expect("a web");
                                webs.get_mut(&w).expect("web").inits.push((b, at, local));
                            }
                            tir::Rvalue::Construct { fields, .. } => {
                                bad(&mut webs, d);
                                let head = tir::LocalId(root(*destination) as u32);
                                for f in fields {
                                    let Some(w) = on(f.value.place) else {
                                        continue;
                                    };
                                    if self.builders.heads.contains_key(&head)
                                        && f.value.usage == UseKind::Take
                                    {
                                        webs.get_mut(&w)
                                            .expect("web")
                                            .consumers
                                            .push((b, at, head, f.field));
                                    } else {
                                        bad(&mut webs, Some(w));
                                    }
                                }
                            }
                            other => {
                                bad(&mut webs, d);
                                for p in rvalue_places(other) {
                                    bad(&mut webs, on(p));
                                }
                            }
                        }
                    }
                    K::BeginLoan {
                        id, place, mutable, ..
                    } => match on(*place) {
                        Some(w) if *mutable => webs.get_mut(&w).expect("web").loans.push(*id),
                        w => bad(&mut webs, w),
                    },
                    K::BindOrigin { .. }
                    | K::EndOrigin(_)
                    | K::StorageLive(_)
                    | K::StorageDead { .. }
                    | K::EndScope(_)
                    | K::EndLoan { .. }
                    | K::Drop { .. }
                    | K::RunCleanup(_) => {}
                    other => {
                        for p in instruction_places(body, other) {
                            bad(&mut webs, on(p));
                        }
                    }
                }
            }
            if let tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } = &block.terminator
            {
                if let Some(r) = returns {
                    definitions[root(r.destination)] += 1;
                }
                let path = path_of(callee);
                let argument = |k: u32| arguments.iter().find(|a| a.parameter.0 == k);
                let written = argument(0)
                    .filter(|a| a.value.usage == UseKind::Mut && returns.is_some())
                    .and_then(|a| on(a.value.place));
                let frozen = match (arguments.as_slice(), returns) {
                    ([a], Some(r))
                        if path.as_ref() == Some(&freeze) && a.value.usage == UseKind::Take =>
                    {
                        on(a.value.place).filter(|&w| on(r.destination) == Some(w))
                    }
                    _ => None,
                };
                match (written, argument(1), frozen) {
                    (Some(w), Some(v), _)
                        if arguments.len() == 2
                            && on(v.value.place).is_none()
                            && (path.as_ref() == Some(&push)
                                || path.as_ref() == Some(&reserve)) =>
                    {
                        let web = webs.get_mut(&w).expect("web");
                        if path.as_ref() == Some(&push) {
                            web.pushes.push(b);
                        } else {
                            web.reserves.push(b);
                        }
                    }
                    (_, _, Some(w)) => webs.get_mut(&w).expect("web").freezes.push(b),
                    _ => {
                        for p in terminator_places(body, &block.terminator) {
                            bad(&mut webs, on(p));
                        }
                    }
                }
            } else {
                for p in terminator_places(body, &block.terminator) {
                    bad(&mut webs, on(p));
                }
            }
        }
        for node in &body.cleanup_nodes {
            match &node.action {
                tir::CleanupAction::CloseArena(p)
                | tir::CleanupAction::JoinTasks { scope: p, .. } => bad(&mut webs, on(*p)),
                tir::CleanupAction::Drop { .. }
                | tir::CleanupAction::EndLoan { .. }
                | tir::CleanupAction::ReleaseRegion(_) => {}
            }
        }
        for site in &body.tail_sites {
            for (_, p) in &site.owned_loop_arguments {
                bad(&mut webs, on(*p));
            }
        }
        // The builder locals each list of a builder web reaches.
        let list_of = |p: tir::PlaceId| self.builders.lists.get(&body.places[p.0 as usize].root);
        let mut out = Fields::default();
        let mut keys: Vec<usize> = webs.keys().copied().collect();
        keys.sort_unstable();
        let mut taken: HashSet<tir::LocalId> = HashSet::new();
        for w in keys {
            let web = &webs[&w];
            let ([init], [_freeze], [consumer]) = (
                web.inits.as_slice(),
                web.freezes.as_slice(),
                web.consumers.as_slice(),
            ) else {
                continue;
            };
            let members: Vec<usize> = (0..n).filter(|&l| web_of[l] == Some(w)).collect();
            if web.bad
                || members.iter().any(|&l| definitions[l] != 1)
                || taken.contains(&consumer.2)
            {
                continue;
            }
            let (bc, ic, head, field) = *consumer;
            let list = self.builders.heads[&head];
            if !self
                .builder_record(list)
                .is_some_and(|record| self.open_field(record, field))
            {
                continue;
            }
            let builder = list_of(list).copied();
            // Every path from the initialization reaches the construction
            // without writing the builder, initializing a list for it again
            // or returning.
            let foreign = |b: usize, i: usize| -> bool {
                let kind = &body.blocks[b].instructions[i].kind;
                if let K::Assign { destination, .. } = kind
                    && let Some(other) = self
                        .builders
                        .heads
                        .get(&body.places[destination.0 as usize].root)
                    && list_of(*other).copied() == builder
                {
                    return true;
                }
                if let K::Assign { destination, .. } = kind
                    && let Some((other, _)) =
                        out.opens.get(&body.places[destination.0 as usize].root)
                    && list_of(*other).copied() == builder
                {
                    return true;
                }
                instruction_places(body, kind)
                    .into_iter()
                    .any(|p| list_of(p).copied() == builder)
            };
            let (bi, ii, _) = *init;
            let mut work = vec![(bi, ii + 1)];
            let mut entered: HashSet<usize> = HashSet::new();
            // Blocks whose terminator a path from the initialization reaches.
            let mut region: HashSet<usize> = HashSet::new();
            let mut closed = false;
            let mut ok = true;
            'walk: while let Some((b, from)) = work.pop() {
                let block = &body.blocks[b];
                for i in from..block.instructions.len() {
                    if (b, i) == (bc, ic) {
                        closed = true;
                        continue 'walk;
                    }
                    if (b, i) == (bi, ii) || foreign(b, i) {
                        ok = false;
                        break 'walk;
                    }
                }
                if self
                    .plan
                    .is_some_and(|plan| plan.site(tir::BlockId(b as u32)).is_some())
                    || matches!(block.terminator, tir::Terminator::Return { .. })
                    || terminator_places(body, &block.terminator)
                        .into_iter()
                        .any(|p| list_of(p).copied() == builder)
                {
                    ok = false;
                    break;
                }
                region.insert(b);
                for s in body.successors(&block.terminator) {
                    if entered.insert(s.0 as usize) {
                        work.push((s.0 as usize, 0));
                    }
                }
            }
            let inside = |b: &usize| region.contains(b);
            if !ok
                || !closed
                || !web
                    .pushes
                    .iter()
                    .chain(&web.reserves)
                    .chain(&web.freezes)
                    .all(inside)
            {
                continue;
            }
            taken.insert(head);
            out.locals
                .extend(members.iter().map(|&l| tir::LocalId(l as u32)));
            out.opens.insert(init.2, (list, field));
            for &b in &web.pushes {
                out.writes
                    .insert(tir::BlockId(b as u32), (list, field, true));
            }
            for &b in &web.reserves {
                out.writes
                    .insert(tir::BlockId(b as u32), (list, field, false));
            }
            out.freezes
                .extend(web.freezes.iter().map(|&b| tir::BlockId(b as u32)));
            out.closes.insert(head, field);
            out.loans.extend(web.loans.iter().copied());
        }
        Ok(out)
    }

    /// The open element's beginning at a list's initialization.
    pub(super) fn field_open(&mut self, list: tir::PlaceId, field: u32) -> R<()> {
        let place = self.place(list)?;
        let address = self.address(&place)?;
        let version = self.value(self.out.slots[place.slot.index()].ty, Holding::Plain);
        self.push(Inst::FlatField {
            address,
            field,
            op: FlatFieldOp::Open,
            version,
        });
        self.state.write(place.slot, &place.steps);
        self.versions[place.slot.index()] = Some(version);
        Ok(())
    }

    /// A list's reservation or push, written to its open field.
    pub(super) fn field_write(
        &mut self,
        (list, field, push): (tir::PlaceId, u32, bool),
        arguments: &[tir::CallArgument],
        returns: Option<tir::CallReturn>,
    ) -> R<()> {
        let [_, operand] = arguments else {
            return malformed("an open field write's arguments");
        };
        let value = self.operand(&operand.value)?;
        let place = self.place(list)?;
        let address = self.address(&place)?;
        let version = self.value(self.out.slots[place.slot.index()].ty, Holding::Plain);
        let op = if push {
            FlatFieldOp::Push {
                value,
                spare: false,
            }
        } else {
            FlatFieldOp::Reserve { capacity: value }
        };
        self.push(Inst::FlatField {
            address,
            field,
            op,
            version,
        });
        self.state.write(place.slot, &place.steps);
        self.versions[place.slot.index()] = Some(version);
        self.removed_call(returns)
    }

    /// A list's removed freeze: its Bytes is the open field, so the call
    /// only continues.
    pub(super) fn field_freeze(&mut self, returns: Option<tir::CallReturn>) -> R<()> {
        let Some(tir::CallReturn { next, .. }) = returns else {
            return malformed("a removed freeze without a continuation");
        };
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }
}
