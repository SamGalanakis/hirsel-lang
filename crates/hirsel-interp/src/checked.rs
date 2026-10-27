//! Checked execution: the memory model enforced at run time.
//!
//! Every value carries a bound, the set of lifetime instances it is local to
//! (MODE-1, MEM-10, PROV-4). Instances are scope entries, parameters, closure
//! environments and arenas; each is poisoned when it ends, and any later access
//! to a value bounded by it traps. Bounds live in a shadow beside every slot.
//! Only TIR, run-time values and declared signatures are consulted.
use crate::{Error, Value};
use hirsel_span::Span;
use hirsel_syntax::DefPath;
use hirsel_tir::{Body, LocalId, ScopeId, Storage};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::{Rc, Weak},
};

/// Declared facts the driver supplies for checked execution only.
#[derive(Clone, Debug, Default)]
pub struct CheckedRuntime {
    /// Definitions and library operations whose signature is `@noalloc` (MEM-5).
    pub noalloc: BTreeSet<DefPath>,
    /// Declared callable headers and nominal protocol associations, including Source.
    pub operations: BTreeMap<DefPath, hirsel_types::SignatureRef>,
    /// Nominal destruction signatures explicitly promising no allocation.
    pub noalloc_drop: BTreeSet<DefPath>,
    /// `arena.with`, executed by the checker with a fresh poisonable arena (MEM-4).
    pub arena_with: Option<DefPath>,
    /// `flat type` declarations: a copy of one owns independent bytes and
    /// leaves its source's blocks unshared (MEM-12).
    pub flat: BTreeSet<DefPath>,
}

/// The memory-model rule a checked execution found broken.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum TrapRule {
    RegionUseAfterScopeExit,
    ArenaUseAfterEnd,
    FinalizerCount,
    FinalizerOrder,
    AllocationInNoalloc,
    AllocatingCallInNoalloc,
    UniqueShared,
    TaskShared,
}
impl TrapRule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RegionUseAfterScopeExit => "region use after scope exit",
            Self::ArenaUseAfterEnd => "arena use after arena end",
            Self::FinalizerCount => "resource not finalised exactly once",
            Self::FinalizerOrder => "finaliser order not reverse declaration",
            Self::AllocationInNoalloc => "allocation inside @noalloc",
            Self::AllocatingCallInNoalloc => "allocating call inside @noalloc",
            Self::UniqueShared => "unique value another holder shares",
            Self::TaskShared => "counted block another task owns",
        }
    }
    /// The spec rules whose run-time consequence this trap witnesses.
    pub fn spec(self) -> &'static str {
        match self {
            Self::RegionUseAfterScopeExit => "MODE-1/MEM-10",
            Self::ArenaUseAfterEnd => "PROV-4/MEM-4",
            Self::FinalizerCount | Self::FinalizerOrder => "DROP-2",
            Self::AllocationInNoalloc | Self::AllocatingCallInNoalloc => "MEM-5",
            Self::UniqueShared => "MODE-2/MEM-11",
            Self::TaskShared => "OWN-6/CONC-4",
        }
    }
}
impl std::fmt::Display for TrapRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A checked execution stopped at a memory-model violation. Like undefined
/// behaviour, no further cleanup runs and no semantic observation exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trap {
    pub rule: TrapRule,
    pub definition: DefPath,
    pub site: Span,
    pub line: u32,
    pub detail: String,
}
impl std::fmt::Display for Trap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = self
            .definition
            .0
            .modules
            .iter()
            .chain(std::iter::once(&self.definition.0.name))
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        write!(
            f,
            "checked trap: {} ({}) in {path} at line {}: {}",
            self.rule,
            self.rule.spec(),
            self.line,
            self.detail
        )
    }
}

/// Sorted, deduplicated lifetime instances.
pub(crate) type Bound = Vec<u32>;
fn union(into: &mut Bound, other: &[u32]) {
    if other.is_empty() {
        return;
    }
    into.extend_from_slice(other);
    into.sort_unstable();
    into.dedup();
}

/// One holder of a counted block: the run-time count a value-based
/// interpreter has no blocks for (MODE-2, MEM-11). Every holder of a block
/// shares its `Block`; a semantic copy is a new holder, administrative Take
/// keeps the holder, and a holder lives while some shadow keeps it, so a
/// block's live holders are its count as far as shadows reach. A copy of an
/// element of a non-inline block is not tracked: such a block has one shadow.
#[derive(Clone)]
pub(crate) struct Holder {
    block: Rc<Block>,
    token: Rc<Token>,
}
struct Block {
    /// Some copy may share the block, or its row did not promise it unique;
    /// this never resets.
    shared: Cell<bool>,
    holders: RefCell<Vec<Weak<Token>>>,
    /// The task whose count operations reach this block (OWN-6): none until
    /// one does, and none again once a task capture hands the block over.
    owner: Cell<Option<u64>>,
}
/// A holder's identity, with the holders its storage keeps alive besides
/// its own block: those a native `mut` operation took into it.
#[derive(Default)]
struct Token {
    retained: RefCell<Vec<Rc<Token>>>,
}
impl Holder {
    fn new(unique: bool) -> Self {
        let token = Rc::new(Token::default());
        Self {
            block: Rc::new(Block {
                shared: Cell::new(!unique),
                holders: RefCell::new(vec![Rc::downgrade(&token)]),
                owner: Cell::new(None),
            }),
            token,
        }
    }
    /// A new holder of the same block, keeping what this one retains.
    fn copy(&self) -> Self {
        let token = Rc::new(Token {
            retained: RefCell::new(self.token.retained.borrow().clone()),
        });
        let mut holders = self.block.holders.borrow_mut();
        holders.retain(|holder| holder.strong_count() > 0);
        holders.push(Rc::downgrade(&token));
        Self {
            block: self.block.clone(),
            token,
        }
    }
    /// Whether another holder of this block is still live.
    fn shared_by_another(&self) -> bool {
        self.block
            .holders
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|holder| !Rc::ptr_eq(&holder, &self.token))
    }
}
impl std::fmt::Debug for Holder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Holder")
            .field("shared", &self.block.shared.get())
            .finish()
    }
}
/// Holders compare by their sharing flag, as the witness compared before it
/// counted holders.
impl PartialEq for Holder {
    fn eq(&self, other: &Self) -> bool {
        self.block.shared.get() == other.block.shared.get()
    }
}
impl Eq for Holder {}

/// Bounds per value node: one per field of an inline aggregate, one for a block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Shadow {
    Uniform(Bound),
    /// A run-time deep ownership witness. Semantic copies invalidate every
    /// surviving shadow sharing this block and add a holder to it;
    /// administrative Take preserves it.
    Ownership {
        content: Box<Shadow>,
        holder: Holder,
    },
    Parts(Vec<Shadow>),
    /// Content reached through storage bounded by `home`: a borrowed capture
    /// (the captured value's bound and the captured slot's home), or a closure
    /// whose environment is region-placed. A call over a global value is
    /// global; the view itself is not.
    View {
        content: Box<Shadow>,
        home: Bound,
    },
}
impl Default for Shadow {
    fn default() -> Self {
        Self::Uniform(vec![])
    }
}
impl Shadow {
    pub fn of(bound: Bound) -> Self {
        Self::Uniform(bound)
    }
    pub fn total(&self) -> Bound {
        let mut out = vec![];
        let mut work = vec![self];
        while let Some(s) = work.pop() {
            match s {
                Self::Uniform(b) => union(&mut out, b),
                Self::Parts(p) => work.extend(p),
                Self::Ownership { content, .. } => work.push(content),
                Self::View { content, home } => {
                    union(&mut out, home);
                    work.push(content);
                }
            }
        }
        out
    }
    fn instances(&self) -> impl Iterator<Item = u32> + '_ {
        let mut work = vec![self];
        let mut current: &[u32] = &[];
        std::iter::from_fn(move || {
            loop {
                if let Some((first, rest)) = current.split_first() {
                    current = rest;
                    return Some(*first);
                }
                match work.pop()? {
                    Self::Uniform(b) => current = b,
                    Self::Parts(p) => work.extend(p),
                    Self::Ownership { content, .. } => work.push(content),
                    Self::View { content, home } => {
                        current = home;
                        work.push(content);
                    }
                }
            }
        })
    }
    /// Bounds of the owned content, excluding borrowed-view access homes.
    /// Copying an item out of a native cursor retains its source storage, but
    /// does not borrow the cursor's temporary access binding (OWN-4).
    pub fn content_bound(&self) -> Bound {
        match self {
            Self::Uniform(bound) => bound.clone(),
            Self::Parts(parts) => {
                let mut bound = vec![];
                for part in parts {
                    union(&mut bound, &part.content_bound());
                }
                bound
            }
            Self::View { content, .. } | Self::Ownership { content, .. } => content.content_bound(),
        }
    }
    /// A borrowed protocol transports the source value, excluding the outer
    /// access chain. Nested capture/view homes remain real content bounds.
    pub(crate) fn source_bound(&self) -> Bound {
        let mut content = self;
        while let Self::View { content: inner, .. } | Self::Ownership { content: inner, .. } =
            content
        {
            content = inner;
        }
        content.total()
    }
    pub fn add_bound(&mut self, bound: &[u32]) {
        match self {
            Self::Uniform(b) => union(b, bound),
            Self::Parts(parts) => {
                for part in parts {
                    part.add_bound(bound);
                }
            }
            Self::View { content, .. } | Self::Ownership { content, .. } => {
                content.add_bound(bound)
            }
        }
    }
    /// Add one instance to every node (`x: T in a` places all of x's blocks in a).
    pub fn add_all(&mut self, instance: u32) {
        let mut work = vec![self];
        while let Some(s) = work.pop() {
            match s {
                Self::Uniform(b) => union(b, &[instance]),
                Self::Parts(p) => work.extend(p.iter_mut()),
                Self::View { content, .. } | Self::Ownership { content, .. } => work.push(content),
            }
        }
    }
    /// The subtree for one field of an inline aggregate.
    pub fn part(&self, index: usize) -> Shadow {
        match self {
            Self::Uniform(_) => Self::Uniform(self.total()),
            Self::Ownership { content, .. } => {
                let mut part = content.part(index);
                part.add_bound(&self.total());
                part
            }
            Self::View { content, home } => Self::View {
                content: Box::new(content.part(index)),
                home: home.clone(),
            },
            Self::Parts(p) => p.get(index).cloned().unwrap_or_default(),
        }
    }
    /// Project borrowed subtrees, deferring copies and leaf-bound additions until
    /// the result escapes. A missing Parts child discards earlier additions;
    /// Ownership and Index add their bounds *after* selecting that child.
    pub(crate) fn project(&self, steps: impl IntoIterator<Item = (usize, bool)>) -> Self {
        let mut steps = steps.into_iter();
        let Some(first) = steps.next() else {
            #[cfg(test)]
            checked_shadow_cost_7::record_clone(self);
            return self.clone();
        };
        let Some(second) = steps.next() else {
            // One projection already materializes only its escaping result.
            // Retain that shallow path without deferred-bound bookkeeping.
            let mut part = self.part(first.0);
            if first.1 {
                part.add_bound(&self.total());
            }
            return part;
        };
        let empty = Self::default();
        let mut node = self;
        let mut inherited = Bound::new();
        let mut views: Vec<&Bound> = vec![];
        for (index, indexed) in [first, second].into_iter().chain(steps) {
            let mut added = indexed.then(|| {
                let mut total = node.projection_total(&inherited);
                for home in &views {
                    union(&mut total, home);
                }
                total
            });
            loop {
                match node {
                    Self::Ownership { content, .. } => {
                        // An index or outer ownership already covers all nested
                        // ownership totals, including the deferred leaf bounds.
                        if added.is_none() {
                            added = Some(node.projection_total(&inherited));
                        }
                        node = content;
                    }
                    Self::View { content, home } => {
                        views.push(home);
                        node = content;
                    }
                    Self::Parts(parts) => {
                        node = parts.get(index).unwrap_or_else(|| {
                            inherited.clear();
                            &empty
                        });
                        break;
                    }
                    Self::Uniform(_) => break,
                }
            }
            if let Some(added) = added {
                union(&mut inherited, &added);
            }
        }
        #[cfg(test)]
        checked_shadow_cost_7::record_clone(node);
        let mut result = node.clone();
        if !inherited.is_empty() {
            result.add_bound(&inherited);
        }
        for home in views.into_iter().rev() {
            result = Self::View {
                content: Box::new(result),
                home: home.clone(),
            };
        }
        result
    }
    /// Total of a borrowed subtree after deferred add_bound calls. Empty Parts
    /// (even behind views) have no leaf to receive those additions.
    fn projection_total(&self, inherited: &[u32]) -> Bound {
        if inherited.is_empty() {
            return self.total();
        }
        let mut total = vec![];
        let mut leaf = false;
        let mut work = vec![self];
        while let Some(node) = work.pop() {
            match node {
                Self::Uniform(bound) => {
                    leaf = true;
                    union(&mut total, bound);
                }
                Self::Parts(parts) => work.extend(parts),
                Self::Ownership { content, .. } => work.push(content),
                Self::View { content, home } => {
                    union(&mut total, home);
                    work.push(content);
                }
            }
        }
        if leaf {
            union(&mut total, inherited);
        }
        total
    }
    /// Invocation consumes the callable access view before reading captures.
    /// The capture fields retain their own storage bounds and borrowed homes.
    pub fn capture(&self, index: usize) -> Self {
        match self {
            Self::View { content, .. } | Self::Ownership { content, .. } => content.capture(index),
            other => other.part(index),
        }
    }
    /// Replace instances by bounds; others are kept.
    pub fn substitute(&mut self, map: &[(u32, Bound)]) {
        if map.is_empty() {
            return;
        }
        let mut work = vec![self];
        while let Some(s) = work.pop() {
            match s {
                Self::Uniform(b) => {
                    if b.iter().any(|i| map.iter().any(|(k, _)| k == i)) {
                        let mut out = vec![];
                        for i in b.iter() {
                            match map.iter().find(|(k, _)| k == i) {
                                Some((_, to)) => union(&mut out, to),
                                None => union(&mut out, &[*i]),
                            }
                        }
                        *b = out;
                    }
                }
                Self::Parts(p) => work.extend(p.iter_mut()),
                Self::Ownership { content, .. } => work.push(content),
                Self::View { content, home } => {
                    let mut mapped = Shadow::Uniform(std::mem::take(home));
                    mapped.substitute(map);
                    *home = mapped.total();
                    work.push(content);
                }
            }
        }
    }
    /// The lifetime a call or capture over this value lends: empty for a
    /// global value, otherwise its bound plus the home of a borrowed view.
    pub fn lifetime(&self) -> Bound {
        match self {
            Self::Ownership { content, .. } => content.lifetime(),
            Self::View { content, home } => {
                let mut bound = content.total();
                if !bound.is_empty() {
                    union(&mut bound, home);
                }
                bound
            }
            other => other.total(),
        }
    }
    pub fn ownership(self, value: &Value, unique: bool) -> Self {
        if !holds_storage(value) {
            return Self::default();
        }
        if matches!(self, Self::Ownership { .. }) {
            return self;
        }
        if let Self::View { content, home } = self {
            return Self::View {
                content: Box::new(content.ownership(value, unique)),
                home,
            };
        }
        if inline_fields(value).is_some() {
            return Self::Parts(
                value
                    .fields()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        field
                            .as_ref()
                            .map(|value| self.part(i).ownership(value, unique))
                            .unwrap_or_default()
                    })
                    .collect(),
            );
        }
        Self::Ownership {
            content: Box::new(self),
            holder: Holder::new(unique),
        }
    }
    pub fn share(&self) {
        match self {
            Self::Ownership { content, holder } => {
                holder.block.shared.set(true);
                content.share();
            }
            Self::Parts(parts) => {
                for part in parts {
                    part.share();
                }
            }
            Self::View { content, .. } => content.share(),
            Self::Uniform(_) => (),
        }
    }
    pub fn is_unique(&self) -> bool {
        match self {
            Self::Ownership { holder, .. } => !holder.block.shared.get(),
            Self::Parts(parts) => parts.iter().all(Self::is_unique),
            Self::View { content, .. } => content.is_unique(),
            Self::Uniform(_) => true,
        }
    }
    /// The shadow a semantic copy of `value` holds, after marking what it
    /// shares: a new holder of each block the witness tracks, except that a
    /// `flat type` part is copied into fresh blocks (MEM-12).
    pub(crate) fn copy_out(&self, value: &Value, state: &State) -> Self {
        if let Value::Record { definition, .. } = value
            && state.config.flat.contains(definition.path())
        {
            return self.fresh_copy(state);
        }
        match (self, value.fields()) {
            (Self::Parts(parts), Some(fields)) if inline_fields(value).is_some() => Self::Parts(
                parts
                    .iter()
                    .zip(fields)
                    .map(|(part, field)| match field {
                        Some(field) => part.copy_out(field, state),
                        None => part.clone(),
                    })
                    .collect(),
            ),
            (Self::View { content, home }, _) => Self::View {
                content: Box::new(content.copy_out(value, state)),
                home: home.clone(),
            },
            _ => {
                self.share();
                self.copied()
            }
        }
    }
    /// The shadow a semantic copy of this value holds: a new holder of each
    /// block the witness tracks.
    pub fn copied(&self) -> Self {
        match self {
            Self::Ownership { content, holder } => Self::Ownership {
                content: Box::new(content.copied()),
                holder: holder.copy(),
            },
            Self::Parts(parts) => Self::Parts(parts.iter().map(Self::copied).collect()),
            Self::View { content, home } => Self::View {
                content: Box::new(content.copied()),
                home: home.clone(),
            },
            Self::Uniform(bound) => Self::Uniform(bound.clone()),
        }
    }
    /// OWN-6: tasks never share counted blocks, so a count operation (a copy,
    /// take or `mut` access) claims each tracked block for `task`. False when
    /// another task already owns one.
    pub(crate) fn claim(&self, task: u64) -> bool {
        match self {
            Self::Ownership { content, holder } => {
                let owner = holder.block.owner.get();
                if owner.is_some_and(|owner| owner != task) {
                    return false;
                }
                holder.block.owner.set(Some(task));
                content.claim(task)
            }
            Self::Parts(parts) => parts.iter().all(|part| part.claim(task)),
            Self::View { content, .. } => content.claim(task),
            Self::Uniform(_) => true,
        }
    }
    /// A task capture moves its blocks to the task that next uses them.
    pub(crate) fn release(&self) {
        match self {
            Self::Ownership { content, holder } => {
                holder.block.owner.set(None);
                content.release();
            }
            Self::Parts(parts) => parts.iter().for_each(Self::release),
            Self::View { content, .. } => content.release(),
            Self::Uniform(_) => (),
        }
    }
    /// MODE-2: a value proved unique is the sole owner of every tracked block.
    pub fn sole_owner(&self) -> bool {
        match self {
            Self::Ownership { content, holder } => {
                !holder.shared_by_another() && content.sole_owner()
            }
            Self::Parts(parts) => parts.iter().all(Self::sole_owner),
            Self::View { content, .. } => content.sole_owner(),
            Self::Uniform(_) => true,
        }
    }
    fn holders(&self, out: &mut Vec<Rc<Token>>) {
        match self {
            Self::Ownership { content, holder } => {
                out.push(holder.token.clone());
                content.holders(out);
            }
            Self::Parts(parts) => parts.iter().for_each(|part| part.holders(out)),
            Self::View { content, .. } => content.holders(out),
            Self::Uniform(_) => (),
        }
    }
    /// A native `mut` operation may keep what it took in this storage, so
    /// each of its blocks keeps the taken holders alive.
    pub fn retain(&self, taken: &Self) {
        let mut tokens = vec![];
        taken.holders(&mut tokens);
        if tokens.is_empty() {
            return;
        }
        let mut work = vec![self];
        while let Some(shadow) = work.pop() {
            match shadow {
                Self::Ownership { content, holder } => {
                    holder
                        .token
                        .retained
                        .borrow_mut()
                        .extend(tokens.iter().cloned());
                    work.push(content);
                }
                Self::Parts(parts) => work.extend(parts),
                Self::View { content, .. } => work.push(content),
                Self::Uniform(_) => (),
            }
        }
    }
    /// Replace the local lifetime inside a callee without discarding the
    /// actual incoming sharing witness, including a forged Unique row.
    pub fn rebound(&self, bound: &Bound) -> Self {
        match self {
            Self::Ownership { content, holder } => Self::Ownership {
                content: Box::new(content.rebound(bound)),
                holder: holder.clone(),
            },
            Self::Parts(parts) => Self::Parts(parts.iter().map(|p| p.rebound(bound)).collect()),
            _ => Self::of(bound.clone()),
        }
    }
    /// Borrowing a callable keeps its environment fields: invoking it reads
    /// those fields, rather than content stored in the borrowed callable slot.
    /// Its access view still expires with the parameter or capture instance.
    pub fn rebound_value(&self, value: &Value, bound: &Bound) -> Self {
        if matches!(value, Value::Closure { .. }) {
            let content = match self {
                Self::View { content, .. } => (**content).clone(),
                _ => self.clone(),
            };
            Self::View {
                content: Box::new(content),
                home: bound.clone(),
            }
        } else {
            self.rebound(bound)
        }
    }
    /// MODE-7: deep copying removes binding/own-root loans, retaining only
    /// the regions and arenas in which the source content lives.
    pub fn fresh_copy(&self, state: &State) -> Self {
        match self {
            Self::Ownership { content, .. } => Self::Ownership {
                content: Box::new(content.fresh_copy(state)),
                holder: Holder::new(true),
            },
            Self::Parts(parts) => {
                Self::Parts(parts.iter().map(|part| part.fresh_copy(state)).collect())
            }
            Self::View { content, home } => Self::View {
                content: Box::new(content.fresh_copy(state)),
                home: state.deep_bound(home),
            },
            Self::Uniform(bound) => Self::of(state.deep_bound(bound)),
        }
    }
    /// Plain values are never local (MODE-1): their bound is dropped.
    pub fn for_value(self, value: &Value) -> Shadow {
        if holds_storage(value) {
            self
        } else {
            Shadow::default()
        }
    }
}

/// True when the value can hold region, arena or heap storage.
pub(crate) fn holds_storage(value: &Value) -> bool {
    let mut work = vec![value];
    while let Some(v) = work.pop() {
        match v {
            Value::String(_)
            | Value::ArenaString { .. }
            | Value::Bytes(_)
            | Value::ArenaBytes { .. }
            | Value::Map { .. }
            | Value::Set { .. }
            | Value::Box(_)
            | Value::ArenaBox { .. } => {
                return true;
            }
            Value::Handle { kind, .. } => {
                if kind == ARENA_HANDLE {
                    return true;
                }
            }
            Value::Sequence { kind, elements, .. } => {
                if !matches!(
                    kind,
                    hirsel_types::BuiltinType::Inline | hirsel_types::BuiltinType::Simd
                ) {
                    return true;
                }
                work.extend(elements.iter().filter_map(Option::as_ref));
            }
            Value::Closure { captures, .. } => {
                if !captures.is_empty() {
                    return true;
                }
            }
            Value::Record { definition, .. }
                if crate::library::simulation::plain_evidence(definition.path()) => {}
            Value::Tuple(f)
            | Value::Record { fields: f, .. }
            | Value::Distinct { fields: f, .. }
            | Value::Variant { fields: f, .. }
            | Value::Resource { fields: f, .. }
            | Value::ResourceVariant { fields: f, .. } => {
                work.extend(f.iter().filter_map(Option::as_ref))
            }
            Value::Unit
            | Value::Bool(_)
            | Value::Integer(_)
            | Value::Char(_)
            | Value::F32(_)
            | Value::F64(_)
            | Value::FunctionReference(_) => (),
            // A root is global storage: no local bound reaches its graph.
            Value::Shared(_) | Value::Mutex(_) => (),
        }
    }
    false
}
/// Inline aggregates keep a shadow per field; everything else is one block.
pub(crate) fn inline_fields(value: &Value) -> Option<usize> {
    match value {
        Value::Tuple(f)
        | Value::Record { fields: f, .. }
        | Value::Distinct { fields: f, .. }
        | Value::Variant { fields: f, .. }
        | Value::Resource { fields: f, .. }
        | Value::ResourceVariant { fields: f, .. }
        | Value::Closure { captures: f, .. } => Some(f.len()),
        Value::Sequence {
            kind: hirsel_types::BuiltinType::Inline | hirsel_types::BuiltinType::Simd,
            elements,
            ..
        } => Some(elements.len()),
        _ => None,
    }
}
pub(crate) fn resources_in(value: &Value) -> u64 {
    let mut count = 0;
    let mut work = vec![value];
    while let Some(v) = work.pop() {
        if matches!(v, Value::Resource { .. } | Value::ResourceVariant { .. })
            || matches!(v, Value::Handle { kind, .. } if kind == "Region")
        {
            count += 1;
        }
        if let Some(fields) = v.fields() {
            work.extend(fields.iter().filter_map(Option::as_ref));
        }
    }
    count
}

pub(crate) const ARENA_HANDLE: &str = "Arena";

#[derive(Clone, Debug, Eq, PartialEq)]
enum Kind {
    /// A binding/own root, with the actual content's deep placement bounds.
    /// Ordinary reads retain this instance; deep unique extracts these bounds.
    Root(Bound),
    /// Mutable access to caller-owned content, with its original incoming bounds.
    MutableRoot(Bound),
    Region,
    Arena,
}
/// Execution-wide checker state, shared by every frame.
pub(crate) struct State {
    pub config: CheckedRuntime,
    /// The task now running, whose count operations claim blocks (OWN-6).
    pub task: u64,
    open: Vec<bool>,
    kinds: Vec<Kind>,
    pub site: Span,
    pub definition: Option<DefPath>,
    pub constructed: u64,
    pub released: u64,
    pub forgotten: u64,
}
pub(crate) type Shared = Rc<RefCell<State>>;
pub(crate) fn state(config: CheckedRuntime) -> Shared {
    Rc::new(RefCell::new(State {
        config,
        task: 0,
        open: vec![],
        kinds: vec![],
        site: Span::default(),
        definition: None,
        constructed: 0,
        released: 0,
        forgotten: 0,
    }))
}
impl State {
    fn instance(&mut self, kind: Kind) -> u32 {
        self.open.push(true);
        self.kinds.push(kind);
        (self.open.len() - 1) as u32
    }
    fn root(&mut self, content: &Shadow) -> u32 {
        self.instance(Kind::Root(self.deep_bound(&content.total())))
    }
    fn deep_bound(&self, bound: &[u32]) -> Bound {
        let mut out = vec![];
        let mut pending = bound.to_vec();
        let mut seen = BTreeSet::new();
        while let Some(instance) = pending.pop() {
            if !seen.insert(instance) {
                continue;
            }
            match &self.kinds[instance as usize] {
                Kind::Root(content) | Kind::MutableRoot(content) => pending.extend(content),
                Kind::Region | Kind::Arena => union(&mut out, &[instance]),
            }
        }
        out
    }
    /// Moving caller-owned content removes only mutable access roots. Ordinary
    /// binding/capture roots and actual placement bounds remain observable.
    pub(crate) fn caller_content(&self, shadow: &mut Shadow) {
        let map = shadow
            .instances()
            .filter_map(|instance| {
                let Kind::MutableRoot(content) = &self.kinds[instance as usize] else {
                    return None;
                };
                let mut pending = content.clone();
                let mut seen = BTreeSet::new();
                let mut bound = vec![];
                while let Some(input) = pending.pop() {
                    if !seen.insert(input) {
                        continue;
                    }
                    match &self.kinds[input as usize] {
                        Kind::MutableRoot(original) => pending.extend(original),
                        _ => union(&mut bound, &[input]),
                    }
                }
                Some((instance, bound))
            })
            .collect::<Vec<_>>();
        shadow.substitute(&map);
    }
    pub fn sealed_view(&mut self) -> u32 {
        self.instance(Kind::Region)
    }
    pub fn arena(&mut self) -> u32 {
        self.instance(Kind::Arena)
    }
    pub fn close(&mut self, instance: u32) {
        if let Some(open) = self.open.get_mut(instance as usize) {
            *open = false;
        }
    }
    pub fn trap(&self, rule: TrapRule, detail: impl Into<String>) -> Error {
        Error::Trapped(Box::new(Trap {
            rule,
            definition: self
                .definition
                .clone()
                .unwrap_or_else(|| hirsel_syntax::DefPath::core("checked.execution")),
            site: self.site,
            line: 0,
            detail: detail.into(),
        }))
    }
    /// Trap if any instance in the shadow has ended.
    pub fn access(&self, shadow: &Shadow, what: &str) -> Result<(), Error> {
        for instance in shadow.instances() {
            if !self.open.get(instance as usize).copied().unwrap_or(true) {
                let rule = match self.kinds[instance as usize] {
                    Kind::Root(_) | Kind::MutableRoot(_) | Kind::Region => {
                        TrapRule::RegionUseAfterScopeExit
                    }
                    Kind::Arena => TrapRule::ArenaUseAfterEnd,
                };
                return Err(self.trap(rule, format!("{what} reaches ended instance {instance}")));
            }
        }
        Ok(())
    }
}

/// Lifetime transport at an opaque native boundary. Only declarations and
/// incoming run-time shadows contribute; callbacks publish their actual returns.
#[derive(Clone, Debug)]
pub(crate) struct OperationCheck {
    pub result: Shadow,
    pub updates: Vec<(usize, Shadow)>,
    pub inputs: Vec<Argument>,
    pub callbacks: Bound,
    /// Checked preparation result retained as a tooling snapshot, without credits.
    pub task_arguments: Option<(Value, Shadow)>,
}
impl OperationCheck {
    /// Native list edits move the corresponding ownership subtrees as well as
    /// their values. Keeping only the outer holder loses pushed children's
    /// witnesses; keeping stale indices after remove/swap can invent uniqueness.
    /// An element moved out of `mut` storage is the caller's, as for Replace:
    /// it keeps where it lives, not this frame's access root.
    pub fn list_operation(
        &mut self,
        state: &State,
        path: &DefPath,
        arguments: &[Value],
        result: &crate::OperationResult,
    ) {
        if ![
            "list.push",
            "list.pop",
            "list.remove",
            "list.swap",
            "list.truncate",
        ]
        .iter()
        .any(|name| path.is_core(name))
        {
            return;
        }
        let mut result = result;
        while let crate::OperationResult::ArenaCharge { result: inner, .. } = result {
            result = inner;
        }
        let crate::OperationResult::Complete {
            value,
            arguments: updated,
            ..
        } = result
        else {
            return;
        };
        let list = updated
            .iter()
            .find(|(i, _)| *i == 0)
            .map(|(_, v)| v)
            .or_else(|| arguments.first());
        let Some(Value::Sequence {
            kind: hirsel_types::BuiltinType::List,
            elements,
            ..
        }) = list
        else {
            return;
        };
        let index = |ordinal: usize| match arguments.get(ordinal) {
            Some(Value::Integer(i)) => usize::try_from(i.bits()).ok(),
            _ => None,
        };
        let Some((_, node)) = self.updates.iter_mut().find(|(i, _)| *i == 0) else {
            return;
        };
        let mut node = node;
        while let Shadow::Ownership { content, .. } | Shadow::View { content, .. } = node {
            node = content;
        }
        let Shadow::Parts(parts) = node else {
            // No structural witness: retain the conservative existing bound.
            return;
        };
        let removed = if path.is_core("list.push") && parts.len() + 1 == elements.len() {
            let Some(input) = self.inputs.get(1) else {
                return;
            };
            parts.push(input.shadow.clone());
            None
        } else if path.is_core("list.pop") && parts.len() == elements.len() + 1 {
            parts.pop().map(|part| Shadow::Parts(vec![part]))
        } else if path.is_core("list.remove") && parts.len() == elements.len() + 1 {
            index(1)
                .filter(|&i| i < parts.len())
                .map(|i| parts.remove(i))
        } else if path.is_core("list.swap") && parts.len() == elements.len() {
            if let (Some(i), Some(j)) = (index(1), index(2))
                && i < parts.len()
                && j < parts.len()
            {
                parts.swap(i, j);
            }
            None
        } else if path.is_core("list.truncate") {
            parts.truncate(elements.len());
            None
        } else {
            None
        };
        if let Some(mut removed) = removed {
            state.caller_content(&mut removed);
            removed.add_bound(&self.result.total());
            self.result = removed.for_value(value);
        }
    }
    pub fn input_bound(&self) -> Bound {
        let mut bound = self.callbacks.clone();
        for input in &self.inputs {
            union(&mut bound, &input.shadow.total());
        }
        bound
    }
    pub fn returned_shadow(&self, value: &Value, arguments: &[Value]) -> Shadow {
        if let Value::Closure {
            captures,
            environment,
            ..
        } = value
        {
            let bound = self.result.total();
            Shadow::Parts(
                captures
                    .iter()
                    .zip(&environment.owned_captures)
                    .map(|(v, owned)| {
                        let Some(v) = v else {
                            return Shadow::default();
                        };
                        if *owned {
                            self.result.clone().for_value(v)
                        } else {
                            let content = native_shadow(v, arguments, self).for_value(v);
                            Shadow::View {
                                content: Box::new(content),
                                home: bound.clone(),
                            }
                        }
                    })
                    .collect(),
            )
        } else {
            self.result.clone().for_value(value)
        }
    }
    pub fn accept_callback(&mut self, returned: &Shadow, written: &[Shadow]) {
        union(&mut self.callbacks, &returned.total());
        for shadow in written {
            union(&mut self.callbacks, &shadow.total());
        }
        // Native continuations may retain the callback return in their state.
        // Its bounds persist through subsequent resumes and mutable writebacks.
        self.result.add_bound(&self.callbacks);
        for (_, update) in &mut self.updates {
            update.add_bound(&self.callbacks);
        }
    }
}
/// Derive every result position from the declared type, preserving inline
/// partitions. Type arguments of blocks share one lifetime; callback headers
/// describe future invocations and are never result payloads.
pub(crate) fn operation_result(
    types: &hirsel_types::TypeInterner,
    root: hirsel_types::TypeId,
    inputs: &[Argument],
    inherited: &Bound,
) -> Shadow {
    use hirsel_types::{BinderRef, TypeKind};
    fn walk(
        types: &hirsel_types::TypeInterner,
        ty: hirsel_types::TypeId,
        inputs: &[Argument],
        inherited: &Bound,
        active: &mut std::collections::HashSet<hirsel_types::TypeId>,
    ) -> Shadow {
        let mut bound = inherited.clone();
        let Some(row) = types.get(ty) else {
            return Shadow::of(bound);
        };
        if let Some(listed) = row.modes.relation.named_parameters() {
            for source in listed.iter() {
                if let BinderRef::Bound {
                    depth: 0,
                    parameter,
                } = source
                {
                    if let Some(input) = inputs.get(parameter.0 as usize) {
                        union(&mut bound, &input.lifetime);
                    }
                }
            }
        }
        if !active.insert(ty) {
            return Shadow::of(bound);
        }
        let parts = match &row.kind {
            TypeKind::Tuple(parts) => Some(parts.iter().copied().collect::<Vec<_>>()),
            TypeKind::Record(fields) => Some(fields.iter().map(|f| f.ty).collect()),
            _ => None,
        };
        let shadow = if let Some(parts) = parts {
            Shadow::Parts(
                parts
                    .into_iter()
                    .map(|part| walk(types, part, inputs, &bound, active))
                    .collect(),
            )
        } else {
            // A container stores all its elements in the same lifetime.
            if !matches!(row.kind, TypeKind::Function(_)) {
                for edge in row.kind.graph_edges() {
                    union(
                        &mut bound,
                        &walk(types, edge.ty, inputs, &vec![], active).total(),
                    );
                }
            }
            Shadow::of(bound)
        };
        active.remove(&ty);
        shadow
    }
    walk(types, root, inputs, inherited, &mut Default::default())
}
/// Recover the shadow of an argument/callee selected from native saved state.
/// Operations can unwrap containers, but cannot create caller lifetime facts.
pub(crate) fn native_shadow(target: &Value, values: &[Value], check: &OperationCheck) -> Shadow {
    let mut bound = vec![];
    let mut found: Option<Shadow> = None;
    let mut matches = 0;
    let mut work: Vec<_> = values
        .iter()
        .zip(&check.inputs)
        .map(|(v, c)| (v, c.shadow.clone()))
        .collect();
    while let Some((value, shadow)) = work.pop() {
        if value == target {
            matches += 1;
            union(&mut bound, &shadow.total());
            if found.is_none() {
                found = Some(shadow.clone());
            }
        }
        if let Some(fields) = value.fields() {
            for (i, field) in fields.iter().enumerate() {
                if let Some(field) = field {
                    work.push((field, shadow.part(i)));
                }
            }
        }
    }
    if let Some(mut shadow) = found {
        // One recovered source retains its independent fields: an owned capture
        // must not acquire a sibling borrow's lifetime. Equal values from several
        // sources are ambiguous, so retain every possible bound conservatively.
        if matches > 1 {
            shadow.add_bound(&bound);
        }
        shadow
    } else {
        Shadow::of(check.input_bound())
    }
}

/// Select an owned mapper input by its preparation ordinal. Equal data at
/// another ordinal cannot substitute another allocation's ownership witness.
pub(crate) fn task_argument_shadow(
    check: &OperationCheck,
    path: &[usize],
    argument: &Value,
) -> Result<Shadow, Error> {
    let (mut value, mut shadow) = check
        .task_arguments
        .as_ref()
        .map(|(value, shadow)| (value, shadow.clone()))
        .ok_or_else(|| crate::invalid("task argument without its checked callback return"))?;
    for &field in path {
        value = value
            .fields()
            .and_then(|fields| fields.get(field))
            .and_then(Option::as_ref)
            .ok_or_else(|| crate::invalid("task argument projection outside callback return"))?;
        shadow = shadow.part(field);
    }
    if value != argument {
        return Err(crate::invalid(
            "task argument differs from its checked callback projection",
        ));
    }
    Ok(shadow.for_value(argument))
}

/// A frame's lifetimes: what each slot's storage is bounded by, and what its
/// own instances mean to its caller.
pub(crate) struct FrameCheck {
    pub state: Shared,
    pub shadows: Vec<Shadow>,
    scopes: Vec<Option<u32>>,
    /// `P_i` per parameter ordinal.
    parameters: Vec<u32>,
    /// `K_c` per borrowed capture, `F` for owned captures, by capture ordinal.
    captures: Vec<u32>,
    environment: u32,
    /// The frame's own region: `@noalloc` places its inferred blocks here.
    pub region: u32,
    owned: Vec<u32>,
    /// The caller's view of each instance this frame may return.
    lifetimes: Vec<(u32, Bound)>,
    /// `mut` write-backs map `P_j` to the argument's original bound.
    originals: Vec<(u32, Bound)>,
    pub returned: Option<Shadow>,
    pub noalloc: bool,
    pub closes_arena: Option<u32>,
    /// Locals whose whole value is a semantic copy: owned, so a result related
    /// to one lends only that local, never what its copy came from (MODE-1).
    pub copies: BTreeSet<u32>,
    /// Actual nonowning Use(Borrow/Mut) values, propagated through administrative
    /// moves. Their compiler slots are carriers, not additional storage bounds.
    pub borrowed: BTreeSet<u32>,
}
/// What a caller knows about one argument.
#[derive(Clone, Debug, Default)]
pub(crate) struct Argument {
    pub shadow: Shadow,
    /// Empty for a global value; otherwise its bound plus the borrowed slot's home.
    pub lifetime: Bound,
    pub mutable: bool,
}
/// What a caller knows about a closure callee.
#[derive(Clone, Debug, Default)]
pub(crate) struct Callee {
    pub captures: Vec<Shadow>,
    pub owned: Vec<bool>,
    pub lifetime: Bound,
}

impl FrameCheck {
    /// MODE-2: a parameter declared `unique` receives, at entry, a value no
    /// other live holder shares, whatever either checker proved.
    pub fn unique_parameters(
        &self,
        body: &Body,
        types: &hirsel_types::TypeInterner,
    ) -> Result<(), Error> {
        for local in &body.parameters {
            let declared = types
                .get(body.locals[local.0 as usize].ty)
                .is_some_and(|r| r.modes.uniqueness == hirsel_types::Uniqueness::Unique);
            if declared
                && !self
                    .shadows
                    .get(local.0 as usize)
                    .is_none_or(Shadow::sole_owner)
            {
                return Err(self.state.borrow().trap(
                    TrapRule::UniqueShared,
                    format!(
                        "parameter {} is declared unique, but its block has another live holder",
                        local.0
                    ),
                ));
            }
        }
        Ok(())
    }
    /// Bind a new frame: local parameters and borrowed captures are bounded by
    /// their own instances, which a declared local result maps back at return.
    pub fn enter(
        state: Shared,
        body: &Body,
        types: &hirsel_types::TypeInterner,
        slots: &[Option<Value>],
        arguments: Vec<Argument>,
        callee: Option<Callee>,
        noalloc: bool,
    ) -> Self {
        let mut shadows = vec![Shadow::default(); body.locals.len()];
        let mut owned = vec![];
        let mut lifetimes = vec![];
        let mut originals = vec![];
        let mut s = state.borrow_mut();
        let environment = s.root(&Shadow::default());
        let region = s.instance(Kind::Region);
        owned.extend([environment, region]);
        let mut parameters = vec![];
        for (ordinal, local) in body.parameters.iter().enumerate() {
            let mut argument = arguments.get(ordinal).cloned().unwrap_or_default();
            let instance = if argument.mutable {
                s.instance(Kind::MutableRoot(argument.shadow.total()))
            } else {
                s.root(&argument.shadow)
            };
            owned.push(instance);
            parameters.push(instance);
            if let Some(value) = slots.get(local.0 as usize).and_then(Option::as_ref) {
                let unique = types
                    .get(body.locals[local.0 as usize].ty)
                    .is_some_and(|r| r.modes.uniqueness == hirsel_types::Uniqueness::Unique);
                argument.shadow = argument.shadow.ownership(value, unique);
            }
            // Borrowed, `mut` and local-taken parameters are local (MODE-1).
            let row_local = body.locals.get(local.0 as usize).is_some_and(|l| {
                types.get(l.ty).is_some_and(|r| {
                    // An arena place sets placement; it is not a lifetime.
                    r.modes.relation.reaches().iter().any(|reach| {
                        !matches!(reach, hirsel_types::Reach::Place(o)
                            if crate::frame::arena_of(body, slots, o).is_some())
                    })
                })
            });
            let value = slots.get(local.0 as usize).and_then(Option::as_ref);
            shadows[local.0 as usize] = match value.filter(|v| row_local && holds_storage(v)) {
                Some(value) => argument.shadow.rebound_value(value, &vec![instance]),
                None => argument.shadow.clone(),
            };
            lifetimes.push((instance, argument.lifetime));
            originals.push((instance, argument.shadow.total()));
        }
        let mut captures = vec![];
        let callee = callee.unwrap_or_default();
        for (ordinal, (index, _)) in body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.storage == Storage::Capture)
            .enumerate()
        {
            let shadow = callee.captures.get(ordinal).cloned().unwrap_or_default();
            if callee.owned.get(ordinal).copied().unwrap_or(true) {
                captures.push(environment);
                shadows[index] = shadow;
            } else {
                let instance = s.root(&shadow);
                owned.push(instance);
                captures.push(instance);
                let value = slots.get(index).and_then(Option::as_ref);
                lifetimes.push((instance, shadow.lifetime()));
                shadows[index] = match value.filter(|v| holds_storage(v)) {
                    Some(value) => shadow.rebound_value(value, &vec![instance]),
                    None => Shadow::default(),
                };
            }
        }
        lifetimes.push((environment, callee.lifetime));
        drop(s);
        Self {
            state,
            shadows,
            scopes: vec![None; body.scopes.len()],
            parameters,
            captures,
            environment,
            region,
            owned,
            lifetimes,
            originals,
            returned: None,
            noalloc,
            closes_arena: None,
            copies: BTreeSet::new(),
            borrowed: BTreeSet::new(),
        }
    }
    /// Whether a bound names no scope, region or environment instance of
    /// this frame: only its caller's, mapped through parameters and captures.
    pub fn caller_owned(&self, bound: &[u32]) -> bool {
        bound.iter().all(|instance| {
            !self.owned.contains(instance)
                || self.parameters.contains(instance)
                || self.captures.contains(instance)
        })
    }
    /// The instance bounding a slot's storage.
    pub fn home(&mut self, body: &Body, local: LocalId) -> Bound {
        let Some(declared) = body.locals.get(local.0 as usize) else {
            return vec![];
        };
        match declared.storage {
            Storage::Parameter => body
                .parameters
                .iter()
                .position(|p| *p == local)
                .and_then(|i| self.parameters.get(i))
                .map_or_else(Vec::new, |i| vec![*i]),
            Storage::Capture => body
                .locals
                .iter()
                .enumerate()
                .filter(|(_, l)| l.storage == Storage::Capture)
                .position(|(i, _)| i == local.0 as usize)
                .and_then(|c| self.captures.get(c))
                .map_or_else(Vec::new, |i| vec![*i]),
            _ => vec![self.scope(declared.scope)],
        }
    }
    fn scope(&mut self, scope: ScopeId) -> u32 {
        if let Some(Some(instance)) = self.scopes.get(scope.0 as usize) {
            return *instance;
        }
        let instance = self.state.borrow_mut().root(&Shadow::default());
        self.owned.push(instance);
        if let Some(slot) = self.scopes.get_mut(scope.0 as usize) {
            *slot = Some(instance);
        }
        instance
    }
    /// `EndScope`: poison this scope's current instance and its descendants'.
    pub fn end_scope(&mut self, body: &Body, scope: ScopeId) {
        let mut state = self.state.borrow_mut();
        for (index, instance) in self.scopes.iter_mut().enumerate() {
            let mut current = Some(ScopeId(index as u32));
            while let Some(id) = current {
                if id == scope {
                    if let Some(instance) = instance.take() {
                        state.close(instance);
                    }
                    break;
                }
                current = body.scopes.get(id.0 as usize).and_then(|s| s.parent);
            }
        }
    }
    pub fn storage_live(&mut self, local: LocalId) {
        self.borrowed.remove(&local.0);
        if let Some(s) = self.shadows.get_mut(local.0 as usize) {
            *s = Shadow::default();
        }
    }
    /// Map the returned shadow into the caller's terms, by the body's declared result.
    pub fn returned_to_caller(&self, body: &Body, types: &hirsel_types::TypeInterner) -> Shadow {
        let mut shadow = self.returned.clone().unwrap_or_default();
        let relation = types.get(body.result).map(|t| &t.modes.relation);
        let map: Vec<(u32, Bound)> = match relation.and_then(|r| r.named_parameters()) {
            Some(listed) => listed
                .iter()
                .filter_map(|p| match p {
                    hirsel_types::ParameterRef::Body { scope, parameter }
                        if *scope == body.parameter_scope =>
                    {
                        self.parameters.get(parameter.0 as usize)
                    }
                    _ => None,
                })
                .filter_map(|i| self.lifetimes.iter().find(|(k, _)| k == i).cloned())
                .collect(),
            None if relation.is_some_and(|r| {
                r.has_environment()
                    || r.reaches()
                        .iter()
                        .any(|r| matches!(r, hirsel_types::Reach::Region(_)))
            }) =>
            {
                self.lifetimes.clone()
            }
            None => vec![],
        };
        shadow.substitute(&map);
        shadow
    }
    /// Caller-owned content keeps its argument's original bounds when moved
    /// out by Replace or handed back after a mutable call.
    pub fn caller_content(&self, parameter: usize, mut shadow: Shadow) -> Shadow {
        if let Some(entry) = self
            .parameters
            .get(parameter)
            .and_then(|i| self.originals.iter().find(|(k, _)| k == i))
        {
            shadow.substitute(std::slice::from_ref(entry));
        }
        self.state.borrow().caller_content(&mut shadow);
        shadow
    }
    /// A `mut` write-back keeps its own argument's original bound.
    pub fn written_back(&self, parameter: usize, local: LocalId) -> Shadow {
        self.caller_content(
            parameter,
            self.shadows
                .get(local.0 as usize)
                .cloned()
                .unwrap_or_default(),
        )
    }
    /// A self-tail restart substitutes surviving parameter contracts, then
    /// poisons every old invocation instance, including its lexical scopes.
    pub fn inherit(&mut self, mut old: FrameCheck) -> Result<(), Error> {
        let remap = |bound: &mut Bound, map: &[(u32, Bound)]| {
            let mut shadow = Shadow::of(std::mem::take(bound));
            shadow.substitute(map);
            *bound = shadow.total();
        };
        let environment = old
            .lifetimes
            .iter()
            .find(|(k, _)| *k == old.environment)
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        for (instance, bound) in &mut self.lifetimes {
            if *instance == self.environment {
                *bound = environment.clone();
            } else {
                remap(bound, &old.lifetimes);
            }
        }
        for (_, bound) in &mut self.originals {
            remap(bound, &old.originals);
        }
        self.closes_arena = old.closes_arena.take();
        // Parameter storage can survive only through the declared call contract.
        // Old scope bounds deliberately remain in the arguments and trap now.
        for shadow in &mut self.shadows {
            shadow.substitute(&old.lifetimes);
        }
        old.exit();
        for (_, lifetime) in &self.lifetimes {
            self.state
                .borrow()
                .access(&Shadow::of(lifetime.clone()), "tail restart argument")?;
        }
        Ok(())
    }
    /// The frame exits: every instance it owns ends, and so does its arena.
    pub fn exit(&mut self) {
        let mut state = self.state.borrow_mut();
        for instance in self.owned.drain(..) {
            state.close(instance);
        }
        if let Some(arena) = self.closes_arena.take() {
            state.close(arena);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovered_native_fields_keep_independent_lifetimes() {
        let shared = state(CheckedRuntime::default());
        let mut state = shared.borrow_mut();
        let borrowed = state.root(&Shadow::default());
        let value = Value::Tuple(vec![
            Some(Value::Bytes(vec![1])),
            Some(Value::Bytes(vec![2])),
        ]);
        let check = OperationCheck {
            result: Shadow::default(),
            updates: vec![],
            inputs: vec![Argument {
                shadow: Shadow::Parts(vec![Shadow::default(), Shadow::of(vec![borrowed])]),
                ..Default::default()
            }],
            callbacks: vec![],
            task_arguments: None,
        };
        let recovered = native_shadow(&value, &[value.clone()], &check);
        state.close(borrowed);
        assert!(state.access(&recovered.capture(0), "owned sibling").is_ok());
        let Err(Error::Trapped(trap)) = state.access(&recovered.capture(1), "escaped borrow")
        else {
            panic!("recovered borrowed field must still expire")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
    }

    #[test]
    fn ambiguous_native_sources_keep_all_possible_lifetimes() {
        let shared = state(CheckedRuntime::default());
        let mut state = shared.borrow_mut();
        let first = state.root(&Shadow::default());
        let second = state.root(&Shadow::default());
        let value = Value::Tuple(vec![
            Some(Value::Bytes(vec![1])),
            Some(Value::Bytes(vec![2])),
        ]);
        let check = OperationCheck {
            result: Shadow::default(),
            updates: vec![],
            inputs: [first, second]
                .map(|borrowed| Argument {
                    shadow: Shadow::Parts(vec![Shadow::default(), Shadow::of(vec![borrowed])]),
                    ..Default::default()
                })
                .into(),
            callbacks: vec![],
            task_arguments: None,
        };
        let recovered = native_shadow(&value, &[value.clone(), value.clone()], &check);
        for borrowed in [first, second] {
            assert!(recovered.capture(0).total().contains(&borrowed));
            assert!(recovered.capture(1).total().contains(&borrowed));
        }
        state.close(first);
        let Err(Error::Trapped(trap)) = state.access(&recovered.capture(0), "ambiguous source")
        else {
            panic!("ambiguous recovery must retain the possibly ended source")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
    }

    #[test]
    fn native_list_edits_keep_each_childs_actual_holder() {
        let list = |values: Vec<Value>| Value::Sequence {
            kind: hirsel_types::BuiltinType::List,
            elements: values.into_iter().map(Some).collect(),
            arena: None,
        };
        let index =
            |i| Value::Integer(crate::Integer::new(hirsel_types::Primitive::U64, i).unwrap());
        let first = Value::Bytes(vec![1]);
        let second = Value::Bytes(vec![2]);
        for name in [
            "list.push",
            "list.pop",
            "list.remove",
            "list.swap",
            "list.truncate",
        ] {
            let owned = Shadow::default().ownership(&first, true);
            let shared = Shadow::default().ownership(&second, true);
            let alias = shared.copied();
            shared.share();
            let before = if name == "list.push" {
                vec![owned.clone()]
            } else {
                vec![owned.clone(), shared.clone()]
            };
            let initial = list(if name == "list.push" {
                vec![first.clone()]
            } else {
                vec![first.clone(), second.clone()]
            });
            let (values, returned, expected, selected) = match name {
                "list.push" => (
                    vec![first.clone(), second.clone()],
                    Value::Unit,
                    vec![true, false],
                    None,
                ),
                "list.pop" => (
                    vec![first.clone()],
                    Value::Variant {
                        variant: 1,
                        fields: vec![Some(second.clone())],
                    },
                    vec![true],
                    Some(false),
                ),
                "list.remove" => (vec![second.clone()], first.clone(), vec![false], Some(true)),
                "list.swap" => (
                    vec![second.clone(), first.clone()],
                    Value::Unit,
                    vec![false, true],
                    None,
                ),
                "list.truncate" => (vec![first.clone()], Value::Unit, vec![true], None),
                _ => unreachable!(),
            };
            let after = list(values);
            let arguments = vec![after.clone(), index(0), index(1)];
            let mut check = OperationCheck {
                result: Shadow::default(),
                updates: vec![(0, Shadow::Parts(before).ownership(&initial, true))],
                inputs: vec![
                    Argument::default(),
                    Argument {
                        shadow: shared.clone(),
                        ..Default::default()
                    },
                ],
                callbacks: vec![],
                task_arguments: None,
            };
            let completion = crate::OperationResult::Complete {
                value: returned,
                arguments: if name == "list.truncate" {
                    vec![(0, after)]
                } else {
                    vec![]
                },
                consumed: vec![],
                released: vec![],
            };
            check.list_operation(
                &state(CheckedRuntime::default()).borrow(),
                &DefPath::core(name),
                &arguments,
                &completion,
            );
            for (i, expected) in expected.into_iter().enumerate() {
                assert_eq!(
                    check.updates[0].1.part(i).is_unique(),
                    expected,
                    "{name} child {i}"
                );
            }
            if let Some(expected) = selected {
                let result = if name == "list.pop" {
                    check.result.part(0)
                } else {
                    check.result.clone()
                };
                assert_eq!(result.is_unique(), expected, "{name} result");
                if expected {
                    owned.share();
                    assert!(!result.is_unique(), "removed holder identity was lost");
                } else {
                    assert!(!result.sole_owner(), "popped alias was lost");
                }
            }
            assert!(!alias.sole_owner());
        }
    }

    #[test]
    fn a_popped_element_keeps_where_the_callers_content_lives() {
        let shared = state(CheckedRuntime::default());
        let mut state = shared.borrow_mut();
        let arena = state.arena();
        for (original, expected) in [(vec![], vec![]), (vec![arena], vec![arena])] {
            // A callee's `mut` parameter rebinds the caller's elements to its
            // access root, which ends when the callee returns.
            let access = state.instance(Kind::MutableRoot(original));
            let first = Value::Bytes(vec![1]);
            let element = Shadow::of(vec![access]).ownership(&first, true);
            let mut check = OperationCheck {
                result: Shadow::default(),
                updates: vec![(0, Shadow::Parts(vec![element]))],
                inputs: vec![],
                callbacks: vec![],
                task_arguments: None,
            };
            let popped = Value::Variant {
                variant: 1,
                fields: vec![Some(first.clone())],
            };
            let emptied = Value::Sequence {
                kind: hirsel_types::BuiltinType::List,
                elements: Vec::new().into_iter().collect(),
                arena: None,
            };
            check.list_operation(
                &state,
                &DefPath::core("list.pop"),
                &[emptied],
                &crate::OperationResult::Complete {
                    value: popped,
                    arguments: vec![],
                    consumed: vec![],
                    released: vec![],
                },
            );
            assert_eq!(check.result.total(), expected);
        }
    }
    #[test]
    fn a_view_of_a_global_value_lends_no_lifetime() {
        let global = Shadow::View {
            content: Box::new(Shadow::default()),
            home: vec![3],
        };
        assert_eq!(global.lifetime(), Vec::<u32>::new());
        assert_eq!(global.total(), vec![3]);
        let local = Shadow::View {
            content: Box::new(Shadow::of(vec![1])),
            home: vec![3],
        };
        assert_eq!(local.lifetime(), vec![1, 3]);
    }
    #[test]
    fn plain_values_are_never_local() {
        assert_eq!(
            Shadow::of(vec![1]).for_value(&Value::Tuple(vec![Some(Value::Unit)])),
            Shadow::default()
        );
        let string = Value::String("s".into());
        assert_eq!(Shadow::of(vec![1]).for_value(&string), Shadow::of(vec![1]));
        // A core StrPos or Path keeps its evidence as a string; a record of
        // any other definition holding one owns that storage.
        let record = |definition: DefPath| Value::Record {
            definition: (&definition).into(),
            fields: vec![Some(string.clone()), Some(Value::Unit)],
        };
        let core = hirsel_syntax::DefPath::core;
        for plain in ["StrPos", "Path"] {
            assert_eq!(
                Shadow::of(vec![1]).for_value(&record(core(plain))),
                Shadow::default()
            );
        }
        let mut user = core("StrPos");
        user.0.package.name = "app".into();
        for counted in [core("Instant"), core("string.StrPos"), user] {
            assert_eq!(
                Shadow::of(vec![1]).for_value(&record(counted)),
                Shadow::of(vec![1])
            );
        }
    }
    #[test]
    fn deep_unique_removes_roots_but_keeps_placement_through_borrowed_roots() {
        let shared = state(CheckedRuntime::default());
        let mut state = shared.borrow_mut();
        let binding = state.root(&Shadow::default());
        let region = state.instance(Kind::Region);
        let arena = state.arena();
        // Two borrowed calls obscure the placement behind their own roots.
        let first = state.root(&Shadow::of(vec![binding, region, arena]));
        let second = state.root(&Shadow::of(vec![first]));
        let source = Shadow::Parts(vec![
            Shadow::of(vec![binding]),
            Shadow::View {
                content: Box::new(Shadow::of(vec![second])),
                home: vec![binding],
            },
        ]);
        let copied = source.fresh_copy(&state);
        assert_eq!(copied.part(0).total(), Vec::<u32>::new());
        assert_eq!(copied.part(1).total(), vec![region, arena]);
        assert_eq!(source.total(), vec![binding, second]);
        state.close(binding);
        state.close(first);
        state.close(second);
        assert!(state.access(&copied, "unique result").is_ok());
        assert!(state.access(&source, "original borrow").is_err());
        state.close(arena);
        let Err(Error::Trapped(trap)) = state.access(&copied, "unique result") else {
            panic!("deep unique must retain arena placement")
        };
        assert_eq!(trap.rule, TrapRule::ArenaUseAfterEnd);
        state.close(region);
        let Err(Error::Trapped(trap)) = state.access(&copied, "unique result") else {
            panic!("deep unique must retain region placement")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
    }

    #[test]
    fn displaced_content_removes_only_transitive_mutable_access_roots() {
        let shared = state(CheckedRuntime::default());
        let mut state = shared.borrow_mut();
        let binding = state.root(&Shadow::default());
        let region = state.instance(Kind::Region);
        let arena = state.arena();
        let outer = state.instance(Kind::MutableRoot(vec![binding, region, arena]));
        let inner = state.instance(Kind::MutableRoot(vec![outer]));
        let owned_outer = state.instance(Kind::MutableRoot(vec![]));
        let owned_inner = state.instance(Kind::MutableRoot(vec![owned_outer]));
        let mut displaced = Shadow::View {
            content: Box::new(Shadow::of(vec![inner])),
            home: vec![binding],
        };
        let mut owned = Shadow::of(vec![owned_inner]);
        state.caller_content(&mut displaced);
        state.caller_content(&mut owned);
        assert_eq!(displaced.total(), vec![binding, region, arena]);
        assert_eq!(owned, Shadow::default());
        for instance in [outer, inner, owned_outer, owned_inner] {
            state.close(instance);
        }
        assert!(state.access(&displaced, "displaced content").is_ok());
        assert!(state.access(&owned, "owned content").is_ok());
        state.close(arena);
        let Err(Error::Trapped(trap)) = state.access(&displaced, "displaced content") else {
            panic!("moving content must retain arena placement")
        };
        assert_eq!(trap.rule, TrapRule::ArenaUseAfterEnd);
        state.close(region);
        let Err(Error::Trapped(trap)) = state.access(&displaced, "displaced content") else {
            panic!("moving content must retain region placement")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
        state.close(binding);
        let Err(Error::Trapped(trap)) = state.access(&displaced, "displaced content") else {
            panic!("moving content must retain an ordinary borrowed binding")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
        assert!(state.access(&owned, "owned content").is_ok());
    }

    #[test]
    fn an_ended_instance_traps_by_its_kind() {
        let state = state(CheckedRuntime::default());
        let (region, arena) = {
            let mut s = state.borrow_mut();
            (s.instance(Kind::Region), s.arena())
        };
        let s = state.borrow();
        assert!(s.access(&Shadow::of(vec![region, arena]), "read").is_ok());
        drop(s);
        state.borrow_mut().close(arena);
        let Err(Error::Trapped(trap)) = state.borrow().access(&Shadow::of(vec![arena]), "read")
        else {
            panic!("expected a trap")
        };
        assert_eq!(trap.rule, TrapRule::ArenaUseAfterEnd);
        state.borrow_mut().close(region);
        let Err(Error::Trapped(trap)) = state.borrow().access(&Shadow::of(vec![region]), "read")
        else {
            panic!("expected a trap")
        };
        assert_eq!(trap.rule, TrapRule::RegionUseAfterScopeExit);
    }
}

/// The immutable nominal header grants this exact implicit call pair. A raw
/// caller flag cannot change checked lifetime transfer; stale receipts use the
/// ordinary conservative call path.
pub(crate) fn protocol_call(
    types: &hirsel_types::TypeInterner,
    definition: &hirsel_tir::Definition,
    body_id: hirsel_tir::BodyId,
    block_id: hirsel_tir::BlockId,
    runtime: &CheckedRuntime,
) -> bool {
    use hirsel_tir::{CallReturn, Callee, Terminator};
    use hirsel_types::{GenericArgument, ParamId, ParameterRef, SignatureKind, TypeKind, UseKind};
    definition.protocol_associations.iter().any(|receipt| {
        let valid = (|| {
            if receipt.body != body_id || !(receipt.open == block_id || receipt.next == block_id) { return None }
            let source = runtime.operations.get(&receipt.association.source)?;
            if source.fingerprint != receipt.association.fingerprint { return None }
            let SignatureKind::Type(nominal) = &source.kind else { return None };
            let association = nominal.loop_source.as_ref()?;
            let open = runtime.operations.get(&association.open)?;
            let next = runtime.operations.get(&association.next)?;
            let (SignatureKind::Function(open_f), SignatureKind::Function(next_f)) = (&open.kind,&next.kind) else { return None };
            let ([open_parameter],[next_parameter]) = (open_f.parameters.as_slice(),next_f.parameters.as_slice()) else { return None };
            if open_parameter.passing != hirsel_syntax::PassingMode::Borrow || next_parameter.passing != hirsel_syntax::PassingMode::Mut { return None }
            if !matches!(&types.get(open_parameter.ty)?.kind,TypeKind::Named {definition:owner,..} if owner.path() == &receipt.association.source) { return None }
            if !types.get(open_f.result)?.modes.relation.parameter_roots().contains(&ParameterRef::bound(ParamId(0))) { return None }
            if !matches!(&types.get(next_f.result)?.kind,TypeKind::Named {definition:option,arguments} if option.path() == &association.option && matches!(arguments.as_slice(),[GenericArgument::Type(_)])) { return None }
            let body = definition.bodies.get(body_id.0 as usize)?;
            let Terminator::Call {callee:Callee::Definition {path:open_path,..},arguments:open_args,returns: Some(CallReturn { destination, .. }),..} = &body.blocks.get(receipt.open.0 as usize)?.terminator else { return None };
            let Terminator::Call {callee:Callee::Definition {path:next_path,..},arguments:next_args,..} = &body.blocks.get(receipt.next.0 as usize)?.terminator else { return None };
            if open_path != &association.open || next_path != &association.next { return None }
            let ([receiver],[cursor]) = (open_args.as_slice(),next_args.as_slice()) else { return None };
            if receiver.value.usage != UseKind::Borrow || cursor.value.usage != UseKind::Mut { return None }
            let receiver = body.places.get(receiver.value.place.0 as usize)?;
            if !matches!(&types.get(receiver.ty)?.kind,TypeKind::Named {definition:owner,..} if owner.path() == &receipt.association.source) { return None }
            let cursor = body.places.get(cursor.value.place.0 as usize)?;
            let destination = body.places.get(destination.0 as usize)?;
            (cursor.root == destination.root && cursor.projection == destination.projection).then_some(())
        })();
        valid.is_some()
    })
}

#[cfg(test)]
#[path = "../tests/it/checked_shadow_cost_7.rs"]
mod checked_shadow_cost_7;
