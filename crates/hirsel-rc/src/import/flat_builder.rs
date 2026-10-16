//! Flat list builders (LIR schema revision 22). A local list whose only
//! elements are variable flat records constructed directly for its pushes, and
//! which is taken whole into one variable flat list field, is never read as a
//! list: only that field's encoding of it is. Such a list is built in that
//! encoding from the start. Each element's construction appends its encoding
//! to the list (`Inst::FlatAppend`) where the record would have been built.
//! The push then has nothing left to do, a reservation is only a capacity hint,
//! and the construction that takes the list copies its count and bytes whole.
//!
//! ```text
//! let mut items: List[Message] = []        // Block{Sequential{prefixed Message}}
//! list.reserve(mut items, n)               // removed
//! for i in 0u64..n {
//!   list.push(mut items, Message(..))      // FlatAppend at the construction
//! }
//! Messages(items: take items)              // count, then the used bytes
//! ```
//!
//! The construction is emitted where the record's construction ran, earlier
//! than the push. Both lie in one TIR block, which the push's call ends, and
//! the element temporaries between them are only moved: an exit between them
//! is a fail-stop failure, after which nothing reads the list. The list's
//! other uses are storage events, drops, moves among temporaries of its type,
//! and its final take, so no read sees an element earlier than the push.
use super::*;
use std::collections::HashSet;

/// The builder webs of one body.
#[derive(Default)]
pub(super) struct Builders {
    /// Each local of a web, and its builder layout.
    pub(super) locals: HashMap<tir::LocalId, LayoutId>,
    /// Each element temporary a construction initializes, and its list's whole place.
    pub(super) heads: HashMap<tir::LocalId, tir::PlaceId>,
    /// Element temporaries: never stored, as their construction appends.
    pub(super) elements: HashSet<tir::LocalId>,
    /// Blocks whose call, a builder's push, is removed.
    pub(super) calls: HashSet<tir::BlockId>,
    /// Blocks whose call, a builder's reservation, reserves its least encoding.
    pub(super) reserves: HashSet<tir::BlockId>,
    /// The removed calls' mutable loans.
    pub(super) loans: HashSet<tir::LoanId>,
}

/// One candidate web while its uses are checked.
#[derive(Default)]
struct Web {
    bad: bool,
    /// The record its elements' content is.
    content: Option<LayoutId>,
    /// The local each push and reservation mutates.
    list: Option<tir::LocalId>,
    pushes: Vec<(tir::BlockId, tir::LocalId)>,
    reserves: Vec<tir::BlockId>,
    /// Mutable loans of its places, which only its pushes and reservations use.
    loans: Vec<tir::LoanId>,
    /// The construction taking the list: its flat layout and field.
    consumers: Vec<(LayoutId, u32)>,
}

impl Importer<'_, '_> {
    /// The variable flat record a list local's elements hold, when its
    /// ordinary layout is a counted block of blocks of one.
    fn element_content(&mut self, ty: TypeId) -> Option<LayoutId> {
        let layout = self.lir_type(ty).ok().map(|t| self.layout_of(t))?;
        let LayoutKind::Block { payload } = self.layouts.record(layout).kind else {
            return None;
        };
        let LayoutKind::Sequence { element } = self.layouts.record(payload).kind else {
            return None;
        };
        let LayoutKind::Block { payload: content } = self.layouts.record(element).kind else {
            return None;
        };
        (self.layouts.record(content).kind == LayoutKind::Flat
            && !flat_fixed(self.layouts, content))
        .then_some(content)
    }

    /// The builder webs of this body; any use outside the shapes above
    /// leaves its web an ordinary list.
    pub(super) fn builder_webs(&mut self) -> R<Builders> {
        use tir::InstructionKind as K;
        let body = self.body;
        let n = body.locals.len();
        let elided = self.instances.elided.get(&self.id);
        let elided: Vec<bool> = (0..n)
            .map(|l| elided.is_some_and(|locals| locals.contains(&tir::LocalId(l as u32))))
            .collect();
        let mut content = vec![None; n];
        for (i, local) in body.locals.iter().enumerate() {
            if matches!(local.storage, tir::Storage::Local | tir::Storage::Temporary) && !elided[i]
            {
                content[i] = self.element_content(local.ty);
            }
        }
        if content.iter().all(Option::is_none) {
            return Ok(Builders::default());
        }
        let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
        let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
        // Webs: candidates joined by whole moves.
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(parent: &mut [usize], mut x: usize) -> usize {
            while parent[x] != x {
                parent[x] = parent[parent[x]];
                x = parent[x];
            }
            x
        }
        for block in &body.blocks {
            for i in &block.instructions {
                if let K::Assign { destination, value } = &i.kind
                    && let tir::Rvalue::Use(op) = &body.rvalues[value.0 as usize]
                    && op.usage == UseKind::Take
                    && whole(*destination)
                    && whole(op.place)
                {
                    let (d, s) = (root(*destination), root(op.place));
                    if content[d].is_some() && content[d] == content[s] {
                        let (a, b) = (find(&mut parent, d), find(&mut parent, s));
                        parent[a] = b;
                    }
                }
            }
        }
        let web_of: Vec<Option<usize>> = (0..n)
            .map(|l| content[l].map(|_| find(&mut parent, l)))
            .collect();
        let mut webs: HashMap<usize, Web> = HashMap::new();
        for (l, w) in web_of.iter().enumerate() {
            if let Some(w) = w {
                webs.entry(*w).or_default().content = content[l];
            }
        }
        let on = |p: tir::PlaceId| web_of[root(p)];
        let bad = |webs: &mut HashMap<usize, Web>, w: Option<usize>| {
            if let Some(w) = w {
                webs.get_mut(&w).expect("web").bad = true;
            }
        };
        // A web local is only ever named whole.
        for p in 0..body.places.len() {
            let p = tir::PlaceId(p as u32);
            if !whole(p) {
                bad(&mut webs, on(p));
            }
        }
        // Reads of each local other than storage, origin and drop events.
        let mut reads = vec![0usize; n];
        let operands = |op: &tir::Operand, reads: &mut Vec<usize>| reads[root(op.place)] += 1;
        for (b, block) in body.blocks.iter().enumerate() {
            for i in &block.instructions {
                match &i.kind {
                    K::Assign { destination, value } => {
                        let d = on(*destination);
                        match &body.rvalues[value.0 as usize] {
                            tir::Rvalue::Use(op) => {
                                operands(op, &mut reads);
                                let s = on(op.place);
                                let moved = op.usage == UseKind::Take && d.is_some() && d == s;
                                if !moved {
                                    bad(&mut webs, s);
                                    bad(&mut webs, d);
                                }
                            }
                            tir::Rvalue::Construct {
                                constructor: tir::Constructor::Sequence,
                                fields,
                                ..
                            } if fields.is_empty() && d.is_some() => {}
                            tir::Rvalue::Construct {
                                constructor,
                                fields,
                                ..
                            } => {
                                bad(&mut webs, d);
                                for f in fields {
                                    operands(&f.value, &mut reads);
                                    let Some(w) = on(f.value.place) else {
                                        continue;
                                    };
                                    let flat = matches!(
                                        constructor,
                                        tir::Constructor::Record(_) | tir::Constructor::Tuple
                                    );
                                    let ty = body.places[destination.0 as usize].ty;
                                    let layout = self.lir_type(ty).ok().map(|t| {
                                        let l = self.layout_of(t);
                                        match self.layouts.record(l).kind {
                                            LayoutKind::Block { payload } => payload,
                                            _ => l,
                                        }
                                    });
                                    match layout {
                                        Some(layout) if flat && f.value.usage == UseKind::Take => {
                                            webs.get_mut(&w)
                                                .expect("web")
                                                .consumers
                                                .push((layout, f.field));
                                        }
                                        _ => bad(&mut webs, Some(w)),
                                    }
                                }
                            }
                            tir::Rvalue::Unary { value, .. } => {
                                operands(value, &mut reads);
                                bad(&mut webs, on(value.place));
                                bad(&mut webs, d);
                            }
                            tir::Rvalue::Binary { left, right, .. } => {
                                operands(left, &mut reads);
                                operands(right, &mut reads);
                                bad(&mut webs, on(left.place));
                                bad(&mut webs, on(right.place));
                                bad(&mut webs, d);
                            }
                            tir::Rvalue::Closure { captures, .. } => {
                                for c in captures {
                                    operands(c, &mut reads);
                                    bad(&mut webs, on(c.place));
                                }
                                bad(&mut webs, d);
                            }
                            _ => bad(&mut webs, d),
                        }
                    }
                    K::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        operands(value, &mut reads);
                        reads[root(*destination)] += 1;
                        for p in [*destination, value.place, *displaced] {
                            bad(&mut webs, on(p));
                        }
                    }
                    K::Snapshot { destination, value } => {
                        operands(value, &mut reads);
                        bad(&mut webs, on(*destination));
                        bad(&mut webs, on(value.place));
                    }
                    K::Check { condition, .. } => {
                        operands(condition, &mut reads);
                        bad(&mut webs, on(condition.place));
                    }
                    K::Defuse { place } => {
                        reads[root(*place)] += 1;
                        bad(&mut webs, on(*place));
                    }
                    K::BindFields { source, fields } => {
                        reads[root(*source)] += 1;
                        for p in std::iter::once(source).chain(fields) {
                            bad(&mut webs, on(*p));
                        }
                    }
                    K::BeginLoan {
                        id, place, mutable, ..
                    } => {
                        reads[root(*place)] += 1;
                        if *mutable && let Some(w) = on(*place) {
                            webs.get_mut(&w).expect("web").loans.push(*id);
                        }
                    }
                    K::BindOrigin { .. }
                    | K::EndOrigin(_)
                    | K::StorageLive(_)
                    | K::StorageDead { .. }
                    | K::EndScope(_)
                    | K::EndLoan { .. }
                    | K::Drop { .. }
                    | K::RunCleanup(_) => {}
                }
            }
            let b = tir::BlockId(b as u32);
            match &block.terminator {
                tir::Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    for a in arguments {
                        operands(&a.value, &mut reads);
                    }
                    if let Some(r) = returns {
                        bad(&mut webs, on(r.destination));
                    }
                    let argument = |k: u32| {
                        arguments
                            .iter()
                            .find(|a| a.parameter.0 == k)
                            .map(|a| &a.value)
                    };
                    let path = match callee {
                        tir::Callee::Definition { path, .. } => Some(path),
                        tir::Callee::Indirect { value, .. } => {
                            operands(value, &mut reads);
                            bad(&mut webs, on(value.place));
                            None
                        }
                    };
                    let list = argument(0)
                        .filter(|a| a.usage == UseKind::Mut && returns.is_some())
                        .and_then(|a| on(a.place).map(|w| (w, a)));
                    let push = path.is_some_and(|p| *p == crate::library::core_path("list.push"));
                    let reserve =
                        path.is_some_and(|p| *p == crate::library::core_path("list.reserve"));
                    match (list, argument(1)) {
                        (Some((w, l)), Some(e))
                            if push
                                && arguments.len() == 2
                                && e.usage == UseKind::Take
                                && on(e.place).is_none() =>
                        {
                            let web = webs.get_mut(&w).expect("web");
                            web.pushes.push((b, tir::LocalId(root(e.place) as u32)));
                            if web
                                .list
                                .replace(tir::LocalId(root(l.place) as u32))
                                .is_some_and(|x| x.0 as usize != root(l.place))
                            {
                                web.bad = true;
                            }
                        }
                        (Some((w, l)), Some(c))
                            if reserve && arguments.len() == 2 && on(c.place).is_none() =>
                        {
                            let web = webs.get_mut(&w).expect("web");
                            web.reserves.push(b);
                            if web
                                .list
                                .replace(tir::LocalId(root(l.place) as u32))
                                .is_some_and(|x| x.0 as usize != root(l.place))
                            {
                                web.bad = true;
                            }
                        }
                        _ => {
                            for a in arguments {
                                bad(&mut webs, on(a.value.place));
                            }
                        }
                    }
                }
                tir::Terminator::Branch { condition: v, .. }
                | tir::Terminator::Switch { value: v, .. }
                | tir::Terminator::Return { value: v, .. } => {
                    operands(v, &mut reads);
                    bad(&mut webs, on(v.place));
                }
                tir::Terminator::Match { table, .. } => {
                    let m = &body.matches[table.0 as usize];
                    reads[root(m.scrutinee)] += 1;
                    bad(&mut webs, on(m.scrutinee));
                    for p in &m.patterns {
                        bad(&mut webs, on(p.place));
                    }
                }
                tir::Terminator::Goto(_)
                | tir::Terminator::Fail { .. }
                | tir::Terminator::Unreachable => {}
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
        // The single definition of each local, by block.
        let mut defs: Vec<Vec<(usize, tir::RvalueId)>> = vec![Vec::new(); n];
        for (b, block) in body.blocks.iter().enumerate() {
            for i in &block.instructions {
                if let K::Assign { destination, value } = &i.kind {
                    defs[root(*destination)].push((b, *value));
                }
            }
        }
        let mut out = Builders::default();
        let mut keys: Vec<usize> = webs.keys().copied().collect();
        keys.sort_unstable();
        'webs: for w in keys {
            let web = &webs[&w];
            let (Some(content), Some(list)) = (web.content, web.list) else {
                continue;
            };
            if web.bad
                || web.pushes.is_empty()
                || body.locals[list.0 as usize].storage != tir::Storage::Local
            {
                continue;
            }
            // The one construction taking the list, into a field encoding a list
            // of exactly these records.
            let [(layout, field)] = web.consumers.as_slice() else {
                continue;
            };
            let record = self.layouts.record(*layout);
            if record.kind != LayoutKind::Flat {
                continue;
            }
            let Some(encoding) = record
                .fields
                .iter()
                .find(|f| f.ordinal == *field)
                .map(|f| f.layout)
            else {
                continue;
            };
            let Some((16, element)) = flat_prefixed(self.layouts, encoding) else {
                continue;
            };
            if flat_prefixed(self.layouts, element) != Some((8, content)) {
                continue;
            }
            // Each pushed element is constructed in the push's block and only
            // moved between temporaries of its type on the way.
            let element_layout = |this: &mut Self, l: usize| {
                this.lir_type(body.locals[l].ty)
                    .ok()
                    .map(|t| this.layout_of(t))
                    .is_some_and(|t| {
                        matches!(this.layouts.record(t).kind, LayoutKind::Block { payload } if payload == content)
                    })
            };
            let mut heads = Vec::new();
            let mut elements = Vec::new();
            for &(b, pushed) in &web.pushes {
                let mut at = pushed.0 as usize;
                loop {
                    if body.locals[at].storage != tir::Storage::Temporary
                        || reads[at] != 1
                        || !element_layout(self, at)
                        || elements.contains(&at)
                    {
                        continue 'webs;
                    }
                    elements.push(at);
                    let [(db, value)] = defs[at].as_slice() else {
                        continue 'webs;
                    };
                    if *db != b.0 as usize {
                        continue 'webs;
                    }
                    match &body.rvalues[value.0 as usize] {
                        tir::Rvalue::Use(op) if op.usage == UseKind::Take && whole(op.place) => {
                            at = root(op.place);
                        }
                        tir::Rvalue::Construct {
                            constructor: tir::Constructor::Record(_) | tir::Constructor::Tuple,
                            ..
                        } => {
                            heads.push(at);
                            break;
                        }
                        _ => continue 'webs,
                    }
                }
            }
            let Some(list_place) =
                (0..body.places.len())
                    .map(|p| tir::PlaceId(p as u32))
                    .find(|&p| {
                        root(p) == list.0 as usize
                            && whole(p)
                            && body.places[p.0 as usize].loan.is_none()
                    })
            else {
                continue;
            };
            let builder = self
                .layouts
                .flat_builder(element)
                .map_err(|e| ImportError::Layout(self.def.path.clone(), e))?;
            for (l, x) in web_of.iter().enumerate() {
                if *x == Some(w) {
                    out.locals.insert(tir::LocalId(l as u32), builder);
                }
            }
            for h in heads {
                out.heads.insert(tir::LocalId(h as u32), list_place);
            }
            out.elements
                .extend(elements.into_iter().map(|l| tir::LocalId(l as u32)));
            out.calls.extend(web.pushes.iter().map(|&(b, _)| b));
            out.reserves.extend(web.reserves.iter().copied());
            out.loans.extend(web.loans.iter().copied());
        }
        Ok(out)
    }

    /// An element construction of a builder web: its encoding appended to the
    /// list at `list`, its encoded sources only read.
    pub(super) fn flat_append(&mut self, list: tir::PlaceId, value: tir::RvalueId) -> R<()> {
        let tir::Rvalue::Construct {
            constructor,
            fields,
            ..
        } = &self.body.rvalues[value.0 as usize]
        else {
            return malformed("a builder element that is not a construction");
        };
        let place = self.place(list)?;
        let builder = self.layout_of(place.ty);
        let element = match self.layouts.record(builder).kind {
            LayoutKind::Block { payload } => match self.layouts.record(payload).kind {
                LayoutKind::Sequential { element } => Some(element),
                _ => None,
            },
            _ => None,
        };
        let Some((8, layout)) = element.and_then(|e| flat_prefixed(self.layouts, e)) else {
            return malformed("a builder of no prefixed flat element");
        };
        let encoded: Vec<u32> = self
            .layouts
            .record(layout)
            .fields
            .iter()
            .filter(|d| !flat_fixed(self.layouts, d.layout))
            .map(|d| d.ordinal)
            .collect();
        let (values, loans, taken) = self.construct_fields(&encoded, fields)?;
        let constructor = match constructor {
            tir::Constructor::Record(path) => {
                ConstructorKind::Record(self.symbol(SymbolKind::Type, path, None))
            }
            tir::Constructor::Tuple => ConstructorKind::Tuple,
            other => return malformed(format!("a builder element {other:?} construction")),
        };
        self.reading(loans, taken, |this| {
            let anchor = this.anchor_here();
            let pooled = PoolRange::push(&mut this.out.pools.fields, values.iter().copied());
            let construct = hirsel_lir::ir::ConstructId(this.out.tables.constructs.len() as u32);
            this.out.tables.constructs.push(ConstructSite {
                arena_no_drop: false,
                constructor,
                layout,
                fields: pooled,
                destination: DestinationClass::Heap,
                placement: None,
                anchor,
            });
            for f in &values {
                this.consumed(f.value);
            }
            let address = this.address(&place)?;
            let ty = this.out.slots[place.slot.index()].ty;
            let version = this.value(ty, Holding::Plain);
            this.push(Inst::FlatAppend {
                address,
                construct,
                version,
            });
            this.state.write(place.slot, &place.steps);
            this.versions[place.slot.index()] = Some(version);
            Ok(())
        })
    }

    /// A builder's reservation of `n` elements, which C, H and LLVM read as
    /// room for `n` of its least encoded element.
    pub(super) fn builder_reserve(
        &mut self,
        arguments: &[tir::CallArgument],
        returns: Option<tir::CallReturn>,
    ) -> R<()> {
        let [list, count] = arguments else {
            return malformed("a builder reservation's arguments");
        };
        let capacity = self.operand(&count.value)?;
        let place = self.place(list.value.place)?;
        let address = self.address(&place)?;
        let version = self.value(self.out.slots[place.slot.index()].ty, Holding::Plain);
        self.push(Inst::SeqReserve {
            address,
            capacity,
            version,
        });
        self.state.write(place.slot, &place.steps);
        self.versions[place.slot.index()] = Some(version);
        self.removed_call(returns)
    }

    /// A builder's removed push: its unit result, then its continuation.
    pub(super) fn removed_call(&mut self, returns: Option<tir::CallReturn>) -> R<()> {
        let Some(tir::CallReturn { destination, next }) = returns else {
            return malformed("a removed builder call without a continuation");
        };
        let dest = self.place(destination)?;
        let constant = self.constant(dest.ty, ConstantValue::Unit);
        let unit = self.fresh(dest.ty);
        self.push(Inst::Const {
            result: unit,
            constant,
        });
        let address = self.address(&dest)?;
        self.store(&dest, address, unit)?;
        let e = self.enter(next);
        self.terminate(Terminator::Jump(e));
        Ok(())
    }
}
