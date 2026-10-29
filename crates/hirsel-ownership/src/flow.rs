use super::cfg::{Context, Graph, Op, expression_type, place};
use super::liveness::{Holder, LiveSet, LiveSets};
use super::properties::{Properties, PropertyTable};
use super::*;
use hirsel_diag::{Obligation, RuleId};
use hirsel_syntax::{DefId, ExprKind, PassingMode, UnaryOp};
use hirsel_type_analysis::parametric::{ParametricInput, parametric_inputs, unique_rows};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod borrowed_match;
mod causes;
mod confined;
mod creation;
mod cursors;
mod loans;
mod parameter_parts;
mod sealed;
mod state;
mod worklist;
use hirsel_modes::explain::{Cause, Explanation, Fact, Requirement};
use loans::{Kind, Loan, LoanSet, On, Site};
use worklist::Pending;

const LIVE: u8 = 1;
const MOVED: u8 = 2;
const DEAD: u8 = 4;
/// A value's content loans: those bounding the whole, and those installed at
/// each part. Only installed parts are selected by a field projection; the
/// root union is a read. Content holds read loans on bindings, and bound and
/// read loans on scopes: a read loan on a cursor's scope is its access, which
/// acquiring an owned copy ends.
#[derive(Clone, Debug, Default)]
struct Content {
    whole: LoanSet,
    parts: Option<Arc<Vec<(Vec<Projection>, LoanSet)>>>,
}
impl Content {
    fn summary(&self) -> std::borrow::Cow<'_, LoanSet> {
        let Some(parts) = &self.parts else {
            return std::borrow::Cow::Borrowed(&self.whole);
        };
        let mut summary = self.whole.clone();
        for (_, loans) in parts.iter() {
            summary.join(loans);
        }
        std::borrow::Cow::Owned(summary)
    }
    fn clear_access(&mut self) {
        // Deep extraction ends content loans and cursor access, but an
        // unplaced copy still keeps every source's storage lifetime (MEM-5).
        let mut whole = self.summary().into_owned();
        whole.retain(|loan| loan.kind == Kind::Bound);
        *self = Self { whole, parts: None };
    }
}
impl PartialEq for Content {
    fn eq(&self, other: &Self) -> bool {
        self.parts == other.parts
            && (self.whole == other.whole || self.summary() == other.summary())
    }
}
impl Eq for Content {}

/// MODE-2: the parts of a value whose content is currently aliased, though
/// the row of an inferred construction that holds them may claim them
/// unique (J types an inferred part by its operand). The root's own
/// uniqueness stays `Modes::uniqueness`, which joins its parts. Paths travel
/// with values; I checks nested `unique` contracts and publishes rows by them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SharedParts(Arc<Vec<(Vec<Projection>, ExprId, bool)>>);
impl SharedParts {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn paths(&self) -> impl Iterator<Item = &[Projection]> {
        self.0.iter().map(|(path, _, _)| path.as_slice())
    }
    /// A shallow entry loses only its root proof; a real share also aliases
    /// descendants. An ordinary unqualified wrapper may still own unique parts.
    fn insert(&mut self, path: Vec<Projection>, site: ExprId) {
        self.insert_with(path, site, false);
    }
    fn insert_deep(&mut self, path: Vec<Projection>, site: ExprId) {
        self.insert_with(path, site, true);
    }
    fn insert_with(&mut self, path: Vec<Projection>, site: ExprId, deep: bool) {
        if self
            .0
            .iter()
            .any(|(p, _, d)| (*d && path.starts_with(p)) || *p == path && (!deep || *d))
        {
            return;
        }
        let entries = Arc::make_mut(&mut self.0);
        entries.retain(|(p, _, _)| *p != path && !(deep && p.starts_with(&path)));
        entries.push((path, site, deep));
        entries.sort_by_key(|(p, site, deep)| {
            (
                p.iter().map(projection_key).collect::<Vec<_>>(),
                *site,
                *deep,
            )
        });
    }
    fn install(&mut self, path: &[Projection], incoming: &Self) {
        for (p, site, deep) in incoming.0.iter() {
            let mut installed = path.to_vec();
            installed.extend(p.iter().cloned());
            self.insert_with(installed, *site, *deep);
        }
    }
    fn project(&mut self, path: &[Projection]) {
        let mut parts = Self::default();
        for (p, site, deep) in self.0.iter() {
            if p.len() > path.len() && p.starts_with(path) {
                parts.insert_with(p[path.len()..].to_vec(), *site, *deep);
            } else if *deep && path.starts_with(p) {
                parts.insert_deep(vec![], *site);
            }
        }
        *self = parts;
    }
    fn replace(&mut self, path: &[Projection], incoming: &Self) {
        if self.paths().any(|p| p.starts_with(path)) {
            Arc::make_mut(&mut self.0).retain(|(p, _, _)| !p.starts_with(path));
        }
        self.install(path, incoming);
    }
    fn join(&mut self, other: &Self) {
        if self == other {
            return;
        }
        for (p, site, deep) in other.0.iter() {
            self.insert_with(p.clone(), *site, *deep);
        }
    }
    fn truncate(&mut self, depth: usize) {
        if self.paths().any(|p| p.len() > depth) {
            let entries = self.0.as_ref().clone();
            let mut parts = Self::default();
            for (p, site, deep) in entries {
                parts.insert_with(
                    p[..p.len().min(depth)].to_vec(),
                    site,
                    deep || p.len() > depth,
                );
            }
            *self = parts;
        }
    }
}

fn projection_key(p: &Projection) -> (u8, u32, u32) {
    match p {
        Projection::Field(f) => (0, *f, 0),
        Projection::VariantField { variant, field } => (1, *variant, *field),
        Projection::Elements => (2, 0, 0),
    }
}

/// Current part evidence overrides physical declarations after a share or
/// replacement. Paths travel with values; defaults never repair a lost proof.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CrossingParts(Arc<Vec<(Vec<Projection>, Sendability)>>);
impl CrossingParts {
    fn get(&self, path: &[Projection]) -> Option<Sendability> {
        self.0
            .iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, s)| s.clone())
    }
    fn set(&mut self, path: &[Projection], proof: Sendability) {
        let entries = Arc::make_mut(&mut self.0);
        if path.contains(&Projection::Elements)
            && proof == Sendability::Sendable
            && entries
                .iter()
                .any(|(p, s)| path.starts_with(p) && *s != Sendability::Sendable)
        {
            return;
        }
        entries.retain(|(p, _)| !p.starts_with(path));
        if entries
            .iter()
            .filter(|(p, _)| path.starts_with(p))
            .max_by_key(|(p, _)| p.len())
            .is_some_and(|(_, known)| *known == proof)
        {
            return;
        }
        entries.push((path.to_vec(), proof));
        entries.sort_by_key(|(p, _)| {
            p.iter()
                .map(|p| match p {
                    Projection::Field(f) => (0, *f, 0),
                    Projection::VariantField { variant, field } => (1, *variant, *field),
                    Projection::Elements => (2, 0, 0),
                })
                .collect::<Vec<_>>()
        });
    }
    fn install(&mut self, path: &[Projection], incoming: &Self) {
        for (p, proof) in incoming.0.iter() {
            let mut installed = path.to_vec();
            installed.extend(p.iter().cloned());
            self.set(&installed, proof.clone());
        }
    }
    fn project(&mut self, path: &[Projection]) {
        let root = self.get(path);
        let mut parts = Self::default();
        if let Some(root) = root {
            parts.set(&[], root);
        }
        for (p, s) in self.0.iter().filter(|(p, _)| p.starts_with(path)) {
            parts.set(&p[path.len()..], s.clone());
        }
        *self = parts;
    }
    fn join(&mut self, other: &Self) {
        if self == other {
            return;
        }
        let mut joined = Self::default();
        let mut entries = Vec::new();
        for (p, _) in self.0.iter().chain(other.0.iter()) {
            let proof = match (self.get(p), other.get(p)) {
                (Some(a), Some(b)) => Some(join_sendability(a, b)),
                (Some(a), None) | (None, Some(a)) if a != Sendability::Sendable => Some(a),
                _ => None,
            };
            if let Some(proof) = proof {
                entries.push((p.clone(), proof));
            }
        }
        entries.sort_by_key(|(path, _)| path.len());
        for (path, proof) in entries {
            joined.set(&path, proof);
        }
        *self = joined;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CrossingBorrow {
    within: Vec<Projection>,
    binding: usize,
    source: Vec<Projection>,
}

/// A mutable loop's element is a place of its source (SEM-4): copying it, or
/// storing an unsendable value into it, shares a part of the binding that
/// owns the source (MODE-2). A temporary's part has no owning binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ElementPlace {
    owner: Option<usize>,
}

// Rust alloc/src/rcs/rc.rs:2220-2225: branch snapshots retain payloads;
// a transfer clones only the Value it edits. Keep ordered maps and joins.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Value(std::rc::Rc<ValueData>);
#[derive(Clone, Debug, Eq, PartialEq)]
struct ValueData {
    /// Actual creation inputs; None is an unproved producer, never an empty environment.
    cursor_views: cursors::Views,
    borrowed_only: bool,
    envelope: Option<BTreeSet<usize>>,
    modes: Modes,
    usage: UseKind,
    /// A mutable collection loop's element place (SEM-4).
    element_place: Option<ElementPlace>,
    /// A borrowed collection loop's element place (SEM-4): copying it shares
    /// a part of its owner, which ends the owner's uniqueness (MODE-2).
    element_view: bool,
    /// The loans the value holds itself: an identity loan on the binding it
    /// aliases; bound loans on the bindings its content depends on and on a
    /// placement a join lost (PROV-4); read loans on the bindings a scoped
    /// environment borrows (MODE-5); and, on its own environment, a bound
    /// loan when it is a scoped callable and a read loan when its call
    /// results are local to that environment (CLO-1).
    loans: LoanSet,
    content: Content,
    crossing_parts: CrossingParts,
    shared_parts: SharedParts,
    unique_parts: BTreeSet<Vec<Projection>>,
    crossing_borrows: Arc<Vec<CrossingBorrow>>,
    invalid: bool,
    deep_copy: bool,
    site: Option<ExprId>,
    /// The counted input that prevents deep uniqueness, independently of
    /// the input that bounds locality.
    uniqueness_site: Option<ExprId>,
    /// Counted-copy provenance used only for a secondary diagnostic label.
    copy_site: Option<ExprId>,
    /// Explicit deep-copy origin retained for a rejected storage transfer.
    unique_copy_site: Option<ExprId>,
    /// An owned call receipt whose outer root has never been shared.
    fresh_receipt: bool,
    /// The parameter of this body and the field paths within it that this
    /// value was read from, as finite prefix states per joined alternative. A
    /// result related to `p.a` must stay within `p.a` (#77).
    part_of: Option<(usize, BTreeSet<usize>)>,
    /// MODE-2: aliased only by a related call result's counted shares. They
    /// are local to this storage's borrow, so none outlives the body except
    /// through a value derived from it: once no live binding or temporary
    /// holds one, the storage is unique again (`restore_borrowed`).
    borrow_only: bool,
    /// The evaluated temporaries past their last use that may still hold
    /// such a share until their statement releases them.
    parked: Arc<BTreeSet<ExprId>>,
}
impl std::ops::Deref for Value {
    type Target = ValueData;
    fn deref(&self) -> &ValueData {
        &self.0
    }
}
impl std::ops::DerefMut for Value {
    fn deref_mut(&mut self) -> &mut ValueData {
        std::rc::Rc::make_mut(&mut self.0)
    }
}
impl Value {
    fn join(&mut self, other: &Self) {
        // Rust a7c37f9f library/alloc/src/rcs/rc.rs:2782-2794 checks retained
        // identity before inspecting a payload. Do it before make_mut too:
        // unchanged branch values need neither a payload copy nor a row walk.
        // A first join can still install the uniqueness witness or normalize
        // parked-share bookkeeping; those cases use the original operation.
        if std::rc::Rc::ptr_eq(&self.0, &other.0)
            && (self.modes.uniqueness != Uniqueness::Aliased
                || self.uniqueness_site.is_some()
                || self.site.is_none())
            && (!self.borrow_only || self.modes.uniqueness != Uniqueness::Unique)
            && (self.borrow_only || self.parked.is_empty())
        {
            work::count(Counter::OwnershipValueJoins);
            return;
        }
        std::ops::DerefMut::deref_mut(self).join(other);
    }
    fn new(modes: Modes) -> Self {
        Self(std::rc::Rc::new(ValueData {
            cursor_views: cursors::Views::default(),
            borrowed_only: false,
            envelope: None,
            modes,
            usage: UseKind::Borrow,
            element_place: None,
            element_view: false,
            loans: LoanSet::default(),
            content: Content::default(),
            crossing_parts: CrossingParts::default(),
            shared_parts: SharedParts::default(),
            unique_parts: BTreeSet::new(),
            crossing_borrows: Arc::new(Vec::new()),
            invalid: false,
            deep_copy: false,
            site: None,
            uniqueness_site: None,
            copy_site: None,
            unique_copy_site: None,
            fresh_receipt: false,
            part_of: None,
            borrow_only: false,
            parked: Arc::default(),
        }))
    }
}
impl ValueData {
    /// A related call result holds counted shares of this storage (MODE-2).
    fn borrow_share(&mut self) {
        self.borrow_only |= self.modes.uniqueness == Uniqueness::Unique;
        self.modes.uniqueness = Uniqueness::Aliased;
        self.fresh_receipt = false;
    }
    /// No value holds a related call result's share any more (MODE-2).
    fn restore_borrowed(&mut self) {
        self.modes.uniqueness = Uniqueness::Unique;
        self.borrow_only = false;
        self.parked = Arc::default();
    }
    fn share_root(&mut self) {
        self.unique_parts.clear();
        self.borrow_only = false;
        self.parked = Arc::default();
        self.modes.uniqueness = Uniqueness::Aliased;
        self.fresh_receipt = false;
    }
    fn share(&mut self) {
        self.unique_parts.clear();
        // A real share invalidates the complete owned payload, on both aliases.
        // G11's borrow-only shares use borrow_share and retain their restoration.
        if let Some(site) = self.site {
            self.shared_parts.insert_deep(vec![], site);
        }
        self.borrow_only = false;
        self.parked = Arc::default();
        self.modes.uniqueness = Uniqueness::Aliased;
        self.fresh_receipt = false;
    }
    /// The binding whose storage this value is, with its issue site.
    /// The same checked path abstraction as retained content loans. Facts
    /// beyond every consumed projection cannot grow across recursive wraps:
    /// discard deep positive evidence and retain negative obligations at their
    /// prefix. A containment dependency at that prefix applies to any child.
    fn restrict_crossing_paths(&mut self, depth: usize) {
        self.shared_parts.truncate(depth);
        if self
            .crossing_parts
            .0
            .iter()
            .any(|(path, _)| path.len() > depth)
        {
            let entries = self.crossing_parts.0.as_ref().clone();
            let mut parts = CrossingParts::default();
            for (path, proof) in entries.iter().filter(|(path, _)| path.len() <= depth) {
                parts.set(path, proof.clone());
            }
            for (path, proof) in entries.iter().filter(|(path, _)| path.len() > depth) {
                if *proof != Sendability::Sendable {
                    parts.set(&path[..depth], Sendability::NotSendable);
                }
            }
            self.crossing_parts = parts;
        }
        if self
            .crossing_borrows
            .iter()
            .any(|borrow| borrow.within.len() > depth)
        {
            let mut borrows = Vec::new();
            for borrow in self.crossing_borrows.iter() {
                let mut borrow = borrow.clone();
                borrow.within.truncate(depth);
                if !borrows.contains(&borrow) {
                    borrows.push(borrow);
                }
            }
            self.crossing_borrows = Arc::new(borrows);
        }
    }
    fn install_crossing_borrows(&mut self, prefix: &[Projection], part: &Self) {
        for borrow in part.crossing_borrows.iter() {
            let mut borrow = borrow.clone();
            let mut path = prefix.to_vec();
            path.extend(borrow.within);
            borrow.within = path;
            if !self.crossing_borrows.contains(&borrow) {
                Arc::make_mut(&mut self.crossing_borrows).push(borrow);
            }
        }
    }
    fn alias(&self) -> Option<(usize, Site)> {
        self.loans.iter().find_map(|loan| match loan.on {
            On::Binding(binding) if loan.kind == Kind::Identity => Some((binding, loan.site)),
            _ => None,
        })
    }
    fn aliased(&self) -> Option<usize> {
        self.alias().map(|(binding, _)| binding)
    }
    /// The binding a loop element place is a part of (SEM-4).
    fn element_owner(&self) -> Option<usize> {
        match self.element_place {
            Some(place) => place.owner,
            None => self.element_view.then(|| self.aliased()).flatten(),
        }
    }
    fn set_alias(&mut self, alias: Option<(usize, Site)>) {
        self.loans.retain(|loan| loan.kind != Kind::Identity);
        if let Some((binding, site)) = alias {
            self.loans
                .insert(Kind::Identity, On::Binding(binding), site);
        }
    }
    /// A counted copy of an alias still depends on the storage it read.
    fn alias_bound(&mut self) {
        if let Some((binding, site)) = self.alias() {
            self.loans.insert(Kind::Bound, On::Binding(binding), site);
        }
    }
    fn sources(&self) -> impl Iterator<Item = usize> + '_ {
        self.loans.bindings(Kind::Bound)
    }
    fn captures(&self) -> impl Iterator<Item = usize> + '_ {
        self.loans.bindings(Kind::Read)
    }
    fn scoped(&self) -> bool {
        self.loans.contains(Kind::Bound, On::Environment)
    }
    fn environment_local(&self) -> bool {
        self.loans.contains(Kind::Read, On::Environment)
    }
    fn unplaced(&self) -> bool {
        self.loans.contains(Kind::Bound, On::Placement)
    }
    /// The bindings the value reaches: its alias, its sources and its
    /// content's read loans.
    fn reached(&self) -> BTreeSet<usize> {
        self.aliased()
            .into_iter()
            .chain(self.sources())
            .chain(self.content.summary().bindings(Kind::Read))
            .collect()
    }
    /// The bindings the value reaches or its environment reads, in the order
    /// causes name them: alias, sources, content read loans, captures.
    fn roots(&self) -> Vec<usize> {
        let mut roots: Vec<usize> = self.aliased().into_iter().collect();
        for root in self
            .sources()
            .chain(self.content.summary().bindings(Kind::Read))
            .chain(self.captures())
        {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }
    /// A bound on a scope, with the site that issued it.
    fn bound_by(&mut self, scope: usize, site: Site) {
        self.content
            .whole
            .insert(Kind::Bound, On::Scope(scope), site);
    }
    /// Bound by every scope `part`'s content is bound by, and by those it
    /// only accesses when `access`.
    fn inherit_crossing_borrows(&mut self, part: &Value) {
        for borrow in part.crossing_borrows.iter() {
            let mut borrow = borrow.clone();
            borrow.within.clear();
            if !self.crossing_borrows.contains(&borrow) {
                Arc::make_mut(&mut self.crossing_borrows).push(borrow);
            }
        }
    }
    fn inherit_callable_captures(&mut self, part: &Value) {
        for loan in part.loans.iter().filter(|loan| loan.kind == Kind::Read) {
            self.loans
                .insert_within(Kind::Read, loan.on, &loan.within, loan.site);
        }
    }
    fn bound_within(&mut self, part: &Value, access: bool) {
        for (kind, scope, site) in part.content.summary().scopes() {
            if kind == Kind::Bound || access {
                self.bound_by(scope, site);
            }
        }
    }
    /// Reads every binding `part` reaches or its environment reads.
    fn read_through(&mut self, part: &ValueData) {
        let content = part.content.summary();
        for loan in content.iter() {
            if let On::Binding(_) = loan.on {
                self.content
                    .whole
                    .insert_within(Kind::Read, loan.on, &loan.within, loan.site);
            }
        }
        for loan in part.loans.iter() {
            if let On::Binding(_) = loan.on {
                self.content.whole.insert(Kind::Read, loan.on, loan.site);
            }
        }
    }
    /// A read loan on a binding, issued at `site`.
    fn read(&mut self, binding: usize, site: Site) {
        self.read_within(binding, &[], site);
    }
    /// A read loan on a record field path within a binding (#77).
    fn read_within(&mut self, binding: usize, within: &[u32], site: Site) {
        self.content
            .whole
            .insert_within(Kind::Read, On::Binding(binding), within, site);
    }
    fn project_content(&mut self, path: &[Projection]) {
        if path.is_empty() {
            return;
        }
        if self.unique_parts.iter().any(|p| path.starts_with(p)) {
            self.modes.uniqueness = Uniqueness::Unique;
        }
        self.unique_parts = self
            .unique_parts
            .iter()
            .filter_map(|p| p.strip_prefix(path).map(|p| p.to_vec()))
            .collect();
        self.crossing_parts.project(path);
        self.shared_parts.project(path);
        self.crossing_borrows = Arc::new(
            self.crossing_borrows
                .iter()
                .filter_map(|borrow| {
                    if borrow.within.starts_with(path) {
                        let mut borrow = borrow.clone();
                        borrow.within = borrow.within[path.len()..].to_vec();
                        Some(borrow)
                    } else if path.starts_with(&borrow.within) {
                        let mut borrow = borrow.clone();
                        borrow.within.clear();
                        Some(borrow)
                    } else {
                        None
                    }
                })
                .collect(),
        );
        let Some(parts) = &self.content.parts else {
            return;
        };
        let mut selected = Vec::new();
        for (part, loans) in parts.iter() {
            if part.starts_with(path) {
                merge_part(&mut selected, part[path.len()..].to_vec(), loans);
            } else if path.starts_with(part) {
                // A whole-content dependency also bounds every projected read.
                merge_part(&mut selected, vec![], loans);
            }
        }
        self.install_content(selected);
    }
    fn install_content(&mut self, parts: Vec<(Vec<Projection>, LoanSet)>) {
        self.content = Content {
            whole: LoanSet::default(),
            parts: Some(Arc::new(parts)),
        };
    }
    /// Copies whose only counted parts are handles own independent counts on
    /// immutable roots or run-owned cells: global, with no borrowed source.
    fn detached_handle_copy(&mut self) {
        self.modes.relation = Relation::global();
        self.loans
            .retain(|loan| !matches!(loan.on, On::Binding(_)) || loan.kind == Kind::Read);
        self.content = Content::default();
    }
    /// `unique x` ends every loan but the scopes its content lives in. A
    /// deep copy is made where its source lives (MEM-5): it is global unless
    /// a join lost that placement, which bounds it by this frame. A deep copy
    /// shares nothing, but it cannot copy a closure's borrowed captures, so
    /// one that may hold them (`captures`: a function value or a type
    /// parameter's) keeps the loans and relation its source has, as in V.
    fn extract_unique(&mut self, captures: bool) {
        self.set_alias(None);
        self.content.clear_access();
        if self.unplaced() {
            self.bound_by(super::cfg::LifetimeScopes::FRAME, Site::Frame);
        } else if self.deep_copy {
            self.shared_parts = SharedParts::default();
            if !captures {
                self.crossing_parts = CrossingParts::default();
                self.content = Content::default();
                self.modes.relation = Relation::global();
            }
        }
    }
    /// An acquired copy ends cursor access.
    fn acquire_copy(&mut self) {
        let access = |loan: &Loan| !(loan.kind == Kind::Read && matches!(loan.on, On::Scope(_)));
        self.content.whole.retain(access);
        if let Some(parts) = &mut self.content.parts {
            for (_, loans) in Arc::make_mut(parts) {
                loans.retain(access);
            }
        }
    }
    fn replace_content(
        &mut self,
        path: &[Projection],
        incoming: &Self,
        depth: usize,
        arenas: &crate::arenas::Arenas,
    ) {
        let mut parts = match &self.content.parts {
            Some(parts) => parts.as_ref().clone(),
            None => {
                // Unknown old content remains bounded as a whole. A projected
                // write still installs its incoming dependencies at that path.
                let mut parts = Vec::new();
                append_parts(&mut parts, &[], self, depth);
                parts
            }
        };
        // Every sequence index overlaps for access, but replacing one element
        // cannot prove that all elements holding a source have been replaced.
        if !path.contains(&Projection::Elements) {
            parts.retain(|(part, _)| !part.starts_with(path));
        }
        append_parts(&mut parts, path, incoming, depth);
        // The stored value's alias and sources become content of the holder.
        let mut stored = LoanSet::default();
        for loan in incoming.loans.iter() {
            if let (Kind::Identity | Kind::Bound, On::Binding(_)) = (loan.kind, loan.on) {
                stored.insert(Kind::Read, loan.on, loan.site);
            }
        }
        merge_part(
            &mut parts,
            path.iter().take(depth).cloned().collect(),
            &stored,
        );
        self.install_content(parts);
        if !arenas.global(&incoming.modes.relation) {
            // Current deep content is weaker; the slot's receiving contract
            // and physical locality remain fixed independently.
            self.modes.relation = Relation::environment();
        }
    }
    fn join(&mut self, other: &Self) {
        work::count(Counter::OwnershipValueJoins);
        self.unique_copy_site = match (self.unique_copy_site, other.unique_copy_site) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.copy_site = match (self.copy_site, other.copy_site) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if other.modes.uniqueness == Uniqueness::Aliased {
            let incoming = other.uniqueness_site.or(other.site);
            self.uniqueness_site = if self.modes.uniqueness == Uniqueness::Unique {
                incoming
            } else {
                match (self.uniqueness_site, incoming) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            };
        }
        self.cursor_views.join(&other.cursor_views);
        self.borrowed_only &= other.borrowed_only;
        self.envelope = creation::join([self.envelope.clone(), other.envelope.clone()]);
        self.fresh_receipt &= other.fresh_receipt;
        self.crossing_parts.join(&other.crossing_parts);
        self.shared_parts.join(&other.shared_parts);
        self.unique_parts.retain(|p| other.unique_parts.contains(p));
        for borrow in other.crossing_borrows.iter() {
            if !self.crossing_borrows.contains(borrow) {
                Arc::make_mut(&mut self.crossing_borrows).push(borrow.clone());
            }
        }
        let whole = match (&self.content.parts, &other.content.parts) {
            (Some(_), None) => self.content.whole.clone(),
            (None, Some(_)) => other.content.whole.clone(),
            _ => {
                let mut whole = self.content.whole.clone();
                whole.join(&other.content.whole);
                whole
            }
        };
        self.content.parts = match (&self.content.parts, &other.content.parts) {
            (Some(a), Some(b)) if a == b => Some(a.clone()),
            (Some(a), Some(b)) => {
                let mut parts = a.as_ref().clone();
                for (path, loans) in b.iter() {
                    merge_part(&mut parts, path.clone(), loans);
                }
                Some(Arc::new(parts))
            }
            (None, None) => None,
            _ => {
                // Keep known installed prefixes at a join. Unknown content
                // contributes its conservative whole-content bounds.
                let mut parts = Vec::new();
                append_parts(&mut parts, &[], self, usize::MAX);
                append_parts(&mut parts, &[], other, usize::MAX);
                Some(Arc::new(parts))
            }
        };
        self.usage = join_usage(self.usage, other.usage);
        if !other.modes.relation.is_global() && self.modes.relation.is_global() {
            self.site = other.site;
        } else if self.modes.relation == other.modes.relation {
            self.site = match (self.site, other.site) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }

        let unique = |v: &Self| v.modes.uniqueness == Uniqueness::Unique;
        self.borrow_only = !(unique(self) && unique(other))
            && (self.borrow_only || unique(self))
            && (other.borrow_only || unique(other));
        if !self.borrow_only {
            self.parked = Arc::default();
        } else if !other.parked.is_subset(&self.parked) {
            Arc::make_mut(&mut self.parked).extend(other.parked.iter().copied());
        }
        self.modes = self.modes.join(&other.modes);
        // Values aliasing different storage depend on both, and the join
        // aliases the first.
        let (mine, theirs) = (self.alias(), other.alias());
        if mine.map(|a| a.0) != theirs.map(|a| a.0) {
            for (binding, site) in mine.into_iter().chain(theirs) {
                self.loans.insert(Kind::Bound, On::Binding(binding), site);
            }
        }
        self.loans.join(&other.loans);
        self.set_alias(match (mine, theirs) {
            (Some((a, x)), Some((b, y))) if a == b => Some((a, x.min(y))),
            (mine, theirs) => mine.or(theirs),
        });
        self.element_place = self.element_place.or(other.element_place);
        self.element_view |= other.element_view;
        self.invalid |= other.invalid;
        self.deep_copy |= other.deep_copy;
        self.content.whole = whole;
        // A join reads within the paths either side reads within.
        self.part_of = match (self.part_of.take(), &other.part_of) {
            (Some((a, mut p)), Some((b, q))) if a == *b => {
                p.extend(q.iter().cloned());
                Some((a, p))
            }
            _ => None,
        };
    }
}
// Normalize by path and source so repeated control-flow joins are idempotent.
fn merge_part(parts: &mut Vec<(Vec<Projection>, LoanSet)>, path: Vec<Projection>, loans: &LoanSet) {
    if loans.is_empty() {
        return;
    }
    if let Some((_, held)) = parts.iter_mut().find(|(part, _)| *part == path) {
        held.join(loans);
    } else {
        parts.push((path, loans.clone()));
    }
}
fn append_parts(
    parts: &mut Vec<(Vec<Projection>, LoanSet)>,
    prefix: &[Projection],
    value: &ValueData,
    depth: usize,
) {
    if let Some(known) = &value.content.parts {
        for (path, loans) in known.iter() {
            let mut target = prefix.to_vec();
            target.extend(path.iter().cloned());
            // Recursive construction cannot grow the flow lattice. Keep the
            // longest prefix that any checked access or pattern can observe.
            target.truncate(depth);
            merge_part(parts, target, loans);
        }
    } else {
        merge_part(parts, prefix.to_vec(), &value.content.whole);
    }
}
/// Paths that all hand over an owned value join to an owned value; a value
/// borrowed on any path is a borrow on every path.
fn join_usage(a: UseKind, b: UseKind) -> UseKind {
    if a == b {
        a
    } else if matches!(a, UseKind::Copy | UseKind::Take)
        && matches!(b, UseKind::Copy | UseKind::Take)
    {
        UseKind::Take
    } else {
        UseKind::Borrow
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Slot(std::rc::Rc<SlotData>);
/// A radix leaf copies pointers; only a mutated binding clones its flow facts.
/// Borrowed from Rust alloc/src/rcs/rc.rs:2220-2226 (Rc::make_mut).
#[derive(Clone, Debug, Eq, PartialEq)]
struct SlotData {
    available: u8,
    value: Value,
    /// The relation fixed at the slot's declaration (LANG-15): its receiving
    /// contract, which survives replacement by stronger current content, or
    /// for an alias the relation of the place it views.
    declared: Relation,
    cursor: Option<cursors::Contract>,
}
impl Default for Slot {
    fn default() -> Self {
        Self::new(SlotData {
            cursor: None,
            available: DEAD,
            value: Value::new(Modes::global()),
            declared: Relation::global(),
        })
    }
}
impl std::ops::Deref for Slot {
    type Target = SlotData;
    fn deref(&self) -> &SlotData {
        &self.0
    }
}
impl std::ops::DerefMut for Slot {
    fn deref_mut(&mut self) -> &mut SlotData {
        std::rc::Rc::make_mut(&mut self.0)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct State {
    slots: hirsel_types::rows::Rows<Slot>,
    values: BTreeMap<ExprId, Value>,
    iteration_loans: BTreeMap<ExprId, Place>,
    /// Storage an evaluated argument of a call not yet entered names: a
    /// `mut` place is reserved and a borrowed place is lent, and the call.
    argument_loans: BTreeMap<ExprId, (Place, ExprId)>,
    /// Calls with an argument loan already reported as violated.
    argument_conflicts: BTreeSet<ExprId>,
}
type SlotJoins = hirsel_types::rows::JoinCache<Slot, fn(&mut Slot, &Slot)>;
impl Slot {
    fn new(data: SlotData) -> Self {
        Self(std::rc::Rc::new(data))
    }
    fn join(&mut self, other: &Self) {
        if other.available & LIVE != 0 {
            self.declared = if self.available & LIVE == 0 {
                other.declared.clone()
            } else {
                self.declared.union(&other.declared)
            };
        }
        if other.available & LIVE != 0 {
            if self.available & LIVE == 0 {
                self.cursor = other.cursor.clone();
            } else {
                match (&mut self.cursor, &other.cursor) {
                    (Some(own), Some(other)) => own.join(other),
                    _ => self.cursor = None,
                }
            }
        }
        self.available |= other.available;
        self.value.join(&other.value);
    }
}
impl State {
    /// MODE-2: a share of a loop element's part ends the uniqueness and
    /// sendability of the binding owning it, and of each enclosing loop's
    /// owner in turn (`for x in mut xs { for y in mut x { .. } }`).
    fn share_element_owners(&mut self, owner: Option<usize>, site: Option<ExprId>) {
        let mut next = owner;
        while let Some(owner) = next {
            let value = &mut self.slots[owner].value;
            value.share();
            if site.is_some() {
                value.uniqueness_site = site;
            }
            value.modes.sendability = Sendability::NotSendable;
            value
                .crossing_parts
                .set(&[Projection::Elements], Sendability::NotSendable);
            next = value.element_owner().filter(|o| *o != owner);
        }
    }
    fn join(&mut self, other: &Self, slots: &mut SlotJoins) -> bool {
        // The slot join reports its change directly. Compare only the values
        // we merge rather than deep-cloning every map in the incoming state.
        let mut changed = self.slots.join_cached(&other.slots, slots);
        for (key, value) in &other.values {
            if let Some(old) = self.values.get_mut(key) {
                let before = old.clone();
                old.join(value);
                changed |= *old != before;
            } else {
                self.values.insert(*key, value.clone());
                changed = true;
            }
        }
        for (source, place) in &other.iteration_loans {
            changed |= self.iteration_loans.insert(*source, place.clone()).as_ref() != Some(place);
        }
        for (argument, loan) in &other.argument_loans {
            changed |= self.argument_loans.insert(*argument, loan.clone()).as_ref() != Some(loan);
        }
        for &conflict in &other.argument_conflicts {
            changed |= self.argument_conflicts.insert(conflict);
        }
        changed
    }
}
/// Primary frame inputs shared across the definition's ownership graphs. Source
/// binding modes are refreshed when publication changes the binding's type.
#[derive(Clone)]
pub(super) struct InitialStorage {
    inactive: hirsel_types::rows::Rows<Slot>,
    owned: hirsel_types::rows::Rows<Slot>,
    scoped: hirsel_types::rows::Rows<Slot>,
    owners: BTreeMap<NameSite, Vec<usize>>,
    bindings: BTreeMap<(NameSite, BindingKey), Vec<usize>>,
    closures: BTreeMap<ExprId, usize>,
    retention_depth: usize,
    patterns: Arc<BTreeMap<hirsel_syntax::PatternId, usize>>,
    let_patterns: Arc<BTreeSet<hirsel_syntax::PatternId>>,
    /// Captures, by closure and ordinal, of a closure local to its own
    /// environment: the capture slot holds a value local to it (CLO-1).
    environment_captures: BTreeSet<(ExprId, usize)>,
    crossing_captures: BTreeMap<(ExprId, usize), Arc<Vec<CrossingBorrow>>>,
    /// Closures whose inferred result is local to their environment because
    /// they capture such a closure: their bodies return local results.
    environment_frames: BTreeSet<ExprId>,
}
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
enum BindingKey {
    Local(LocalId),
    Parameter(ParamId),
    Definition(hirsel_syntax::DefPath),
    Variant(VariantId),
    Size(SizeParameterRef),
    Type(TypeParameterRef),
    Primitive(u8),
}
impl BindingKey {
    fn of(binding: &ResolvedName) -> Self {
        match binding {
            ResolvedName::Local(id) => Self::Local(*id),
            ResolvedName::Parameter(id) => Self::Parameter(*id),
            ResolvedName::Definition(path) => Self::Definition(path.clone()),
            ResolvedName::Variant(id) => Self::Variant(*id),
            ResolvedName::Size(id) => Self::Size(*id),
            ResolvedName::TypeParameter(id) => Self::Type(*id),
            ResolvedName::Primitive(kind) => Self::Primitive(*kind as u8),
        }
    }
}
impl InitialStorage {
    pub(super) fn new(
        t: &TypeInterner,
        tree: &hirsel_syntax::SyntaxTree,
        body: &CheckedBody,
        properties: &PropertyTable,
    ) -> Self {
        let count = body.bindings.len();
        let mut storage = Self {
            inactive: hirsel_types::rows::Rows::new(Slot::default(), count)
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
            owned: hirsel_types::rows::Rows::new(Slot::default(), count)
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
            scoped: hirsel_types::rows::Rows::new(Slot::default(), count)
                .with_copy_counter(hirsel_intern::work::Counter::CopiedSlotRows),
            owners: BTreeMap::new(),
            bindings: BTreeMap::new(),
            closures: BTreeMap::new(),
            retention_depth: retention_depth(body),
            patterns: Arc::new(
                body.patterns
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(i, p)| (p.pattern, i))
                    .collect(),
            ),
            let_patterns: Arc::new(
                tree.statements
                    .iter()
                    .filter_map(|s| match s.kind {
                        hirsel_syntax::StmtKind::Let { pattern, .. } => Some(pattern),
                        _ => None,
                    })
                    .collect(),
            ),
            environment_captures: BTreeSet::new(),
            crossing_captures: BTreeMap::new(),
            environment_frames: BTreeSet::new(),
        };
        for (index, binding) in body.bindings.iter().enumerate() {
            storage.owners.entry(binding.owner).or_default().push(index);
            storage
                .bindings
                .entry((binding.owner, BindingKey::of(&binding.binding)))
                .or_default()
                .push(index);
            storage.refresh(t, index, body, properties);
        }
        for (index, closure) in body.closures.iter().enumerate() {
            storage.closures.entry(closure.expression).or_insert(index);
        }
        storage
    }
    fn refresh(
        &mut self,
        t: &TypeInterner,
        index: usize,
        body: &CheckedBody,
        properties: &PropertyTable,
    ) {
        work::count(Counter::OwnershipInitializedRows);
        let binding = &body.bindings[index];
        let declared = t.get(binding.ty).unwrap().modes.relation.clone();
        let plain = properties[binding.ty].plain;
        let mut value = Value::new(t.get(binding.ty).unwrap().modes.clone());
        if plain {
            value.modes.relation = Relation::global();
        }
        self.inactive.set(
            index,
            Slot::new(SlotData {
                cursor: None,
                available: DEAD,
                value: value.clone(),
                declared: declared.clone(),
            }),
        );
        self.owned.set(
            index,
            Slot::new(SlotData {
                cursor: None,
                available: LIVE,
                value: value.clone(),
                declared: declared.clone(),
            }),
        );
        value.set_alias(Some((index, Site::Binding(index))));
        if !plain {
            value.modes.relation = Relation::environment();
        }
        self.scoped.set(
            index,
            Slot::new(SlotData {
                cursor: None,
                available: LIVE,
                value,
                declared,
            }),
        );
    }
    pub(super) fn postcondition_inputs(
        &mut self,
        t: &TypeInterner,
        syntax: &ItemSyntax,
        owner: NameSite,
        body: &mut CheckedBody,
        properties: &mut PropertyTable,
        uniqueness: &BTreeMap<ParamId, bool>,
        sendability: &BTreeMap<ParamId, bool>,
    ) {
        let tree = &syntax.tree;
        let syntax = match owner {
            NameSite::Item => match &syntax.item.kind {
                hirsel_syntax::ItemKind::Function { signature, .. } => signature,
                _ => return,
            },
            NameSite::Statement(statement) => match &tree.statements[statement.0 as usize].kind {
                hirsel_syntax::StmtKind::Function { signature, .. } => signature,
                _ => return,
            },
            NameSite::Expression(expression) => match &tree.expressions[expression.0 as usize].kind
            {
                ExprKind::Closure { signature, .. } => signature,
                _ => return,
            },
            _ => return,
        };
        for clause in syntax
            .contracts
            .iter()
            .filter(|clause| clause.kind == hirsel_syntax::ContractKind::Ensures)
        {
            let scope = NameSite::Expression(clause.expression);
            let mut ordinal = 0;
            for (parameter, declaration) in syntax.parameters.iter().enumerate() {
                if declaration.passing == PassingMode::Take {
                    continue;
                }
                let unique = uniqueness.get(&ParamId(parameter as u32)).copied();
                if let Some(unique) = unique {
                    if let Some(&index) = self
                        .bindings
                        .get(&(scope, BindingKey::Parameter(ParamId(ordinal))))
                        .and_then(|indices| indices.first())
                    {
                        let old = body.bindings[index].ty;
                        let mut row = t.get(old).unwrap().clone();
                        row.modes.uniqueness = if unique {
                            Uniqueness::Unique
                        } else {
                            Uniqueness::Aliased
                        };
                        if let Some(sendable) = sendability.get(&ParamId(parameter as u32)) {
                            row.modes.sendability = if *sendable {
                                Sendability::Sendable
                            } else {
                                Sendability::NotSendable
                            };
                            if !sendable {
                                row.sendability_requirement = SendabilityRequirement::None;
                            }
                        }
                        if &row != t.get(old).unwrap() {
                            body.bindings[index].ty = t.intern(row);
                            properties.inherit(body.bindings[index].ty, old);
                            self.refresh(t, index, body, properties);
                        }
                    }
                }
                ordinal += 1;
            }
        }
    }
    pub(super) fn environment_frames(&self) -> &BTreeSet<ExprId> {
        &self.environment_frames
    }
    fn initial(&self, checker: &Checker<'_, '_>) -> State {
        let mut slots = if matches!(checker.graph.owner, NameSite::Item) {
            self.inactive.clone()
        } else if checker.graph.scoped {
            self.scoped.clone()
        } else {
            self.owned.clone()
        };
        for &index in self.owners.get(&checker.graph.owner).into_iter().flatten() {
            work::count(Counter::OwnershipInitializedRows);
            let binding = &checker.body.bindings[index];
            let mut slot = self.inactive[index].clone();
            if let ResolvedName::Parameter(parameter) = binding.binding {
                slot.available = LIVE;
                slot.value.envelope = checker
                    .creation_selectors()
                    .map(|_| BTreeSet::from([index]));
                let row = checker.t.get(binding.ty).unwrap();
                let property = checker.properties[binding.ty];
                let bound = matches!(row.kind, TypeKind::Parameter(p) if checker.body.resolution.generic_declarations.bounds(p).contains(&hirsel_syntax::Bound::Sendable));
                let passing = checker.parameter_passing(&binding.owner, parameter);
                // MODE-4: a taken noncopyable root has no other holder, since
                // nothing can share it (OWN-5), so it keeps its type's crossing
                // proof exactly as a call receipt of the same type does.
                let unshared_root = passing == PassingMode::Take && !property.copyable;
                if row.modes.sendability == Sendability::Sendable
                    && row.sendability_requirement != SendabilityRequirement::Sendable
                    && row.modes.uniqueness != Uniqueness::Unique
                    && !property.plain
                    && !property.flat
                    && !property.shared
                    && !property.function
                    && !property.sendable_aliased
                    && !bound
                    && !unshared_root
                {
                    slot.value.modes.sendability = Sendability::NotSendable;
                }
                if passing != PassingMode::Take && !checker.properties[binding.ty].plain {
                    slot.value.set_alias(Some((index, Site::Binding(index))));
                    // A borrowed parameter is local to itself, and a written
                    // relation adds to that root (MODE-1, S2).
                    slot.value.modes.relation =
                        Relation::environment().union(&slot.value.modes.relation);
                } else if !checker.arenas.global(&slot.value.modes.relation) {
                    // A taken Local argument bounds its content by that
                    // argument, through every part that keeps its value.
                    // Literal and local function results can name their own
                    // parameters too; keep the same source proof in each frame.
                    slot.value
                        .loans
                        .insert(Kind::Bound, On::Binding(index), Site::Binding(index));
                }
                if checker.properties[binding.ty].function && passing != PassingMode::Take {
                    slot.value
                        .loans
                        .insert(Kind::Bound, On::Environment, Site::Binding(index));
                }
            }
            if checker.finalizer_self(index) {
                slot.available = LIVE;
                slot.value.set_alias(Some((index, Site::Binding(index))));
                slot.value.modes.relation = Relation::environment();
            }
            if checker.properties[binding.ty].plain {
                slot.value.modes.relation = Relation::global();
            }
            slots.set(index, slot);
        }
        let expression = match checker.graph.owner {
            NameSite::Expression(expression) => Some(expression),
            NameSite::Statement(statement) => {
                match checker.tree.statements[statement.0 as usize].kind {
                    hirsel_syntax::StmtKind::Function { body, .. } => Some(body),
                    _ => None,
                }
            }
            _ => None,
        };
        let layout = expression
            .and_then(|expression| self.closures.get(&expression))
            .map(|index| &checker.body.closures[*index]);
        let mut selected = BTreeSet::new();
        for (owner, binding, ordinal) in &checker.graph.captures {
            for &index in self
                .bindings
                .get(&(*owner, BindingKey::of(binding)))
                .into_iter()
                .flatten()
            {
                // The first capture selection wins, even when its layout row is
                // missing. This preserves the original find-then-get policy.
                if !selected.insert(index) {
                    continue;
                }
                work::count(Counter::OwnershipInitializedRows);
                let Some(capture) = layout.and_then(|layout| layout.captures.get(*ordinal)) else {
                    continue;
                };
                let mut slot = slots[index].clone();
                slot.available = LIVE;
                slot.value.modes = checker.t.get(capture.ty).unwrap().modes.clone();
                if let Some(dependencies) =
                    expression.and_then(|e| self.crossing_captures.get(&(e, *ordinal)))
                {
                    slot.value.crossing_borrows = dependencies.clone();
                }

                // CLO-4: creation owns copied captures, but a Many invocation
                // only borrows them. Keep their global content relation while
                // preventing implicit moves out of the environment's slots.
                let many = layout.is_some_and(|layout| {
                    checker.t.get(layout.ty).unwrap().modes.multiplicity != Multiplicity::Once
                });
                slot.value.set_alias(
                    (many || checker.graph.scoped && capture.usage != UseKind::Take)
                        .then_some((index, Site::Binding(index))),
                );
                if checker.properties[checker.body.bindings[index].ty].plain {
                    slot.value.modes.relation = Relation::global();
                }
                // CLO-1: this capture holds a closure local to its environment.
                let local =
                    expression.is_some_and(|e| self.environment_captures.contains(&(e, *ordinal)));
                slot.value.loans.set(
                    Kind::Read,
                    On::Environment,
                    local.then_some(Site::Binding(index)),
                );
                slots.set(index, slot);
            }
        }
        State {
            slots,
            values: BTreeMap::new(),
            iteration_loans: BTreeMap::new(),
            argument_loans: BTreeMap::new(),
            argument_conflicts: BTreeSet::new(),
        }
    }
}

struct Checker<'a, 's> {
    t: &'a TypeInterner,
    proof_error: std::cell::RefCell<Option<QueryError>>,
    unique_parts_cache: std::cell::RefCell<std::collections::HashMap<TypeId, Vec<Vec<Projection>>>>,
    syntax: &'a ItemSyntax,
    tree: &'a hirsel_syntax::SyntaxTree,
    /// Diagnostics of this graph awaiting their causes (R1).
    explanations: Vec<(usize, causes::Pending)>,
    /// Sites logged by the trace pass of a graph that reported a diagnostic.
    trace: Option<causes::Trace>,
    /// The node being transferred, in either pass.
    node: usize,
    own: &'a Signature,
    body: &'a mut CheckedBody,
    /// Signatures the property passes read, for proofs over flow modes.
    signatures: &'a super::properties::Signatures<'s>,
    source_rows: &'a super::source_rows::SourceRows,
    properties: &'a mut PropertyTable,
    result_sharing: &'a BTreeSet<ExprId>,
    projection_lifetimes: &'a BTreeMap<ExprId, super::properties::ProjectionLifetime>,
    pattern_declarations: &'a BTreeMap<hirsel_syntax::PatternId, TypeId>,
    /// Call arguments whose result reads only a field path of them (#77).
    field_loan_places: &'a BTreeMap<ExprId, super::properties::FieldPlace>,
    graph: &'a Graph,
    parameter_parts: parameter_parts::ParameterParts,
    cursor_observations: cursors::Observations,
    capture_crossing_uses: &'a BTreeMap<ExprId, Vec<(usize, Vec<Projection>)>>,
    facts: &'a mut OwnershipFacts,
    introductions: &'a BTreeMap<NameSite, Vec<(NameSite, Origin, usize)>>,
    diagnostics: &'a mut Vec<Diagnostic>,
    emit: bool,
    modes: BTreeMap<ExprId, Modes>,
    /// The published use of each expression whose value arrives from a child
    /// on one of several paths (a branch, an arm).
    forwarded: BTreeMap<ExprId, UseKind>,
    binding_modes: BTreeMap<usize, Modes>,
    /// The currently aliased parts of each published expression and binding
    /// value, whose rows are published with those parts aliased.
    shared_parts: BTreeMap<ExprId, SharedParts>,
    binding_parts: BTreeMap<usize, SharedParts>,
    pattern_modes: BTreeMap<hirsel_syntax::PatternId, Modes>,
    physical_modes: BTreeMap<ExprId, Modes>,
    capture_physical_modes: BTreeMap<usize, Modes>,
    /// The authority pass's frame results, by closure.
    frame_results: &'a BTreeMap<NameSite, TypeId>,
    /// Expressions, bindings and patterns whose rows hold a closure local to
    /// its environment, published with a local result.
    environment_results: BTreeSet<ExprId>,
    environment_bindings: BTreeSet<usize>,
    environment_patterns: BTreeSet<hirsel_syntax::PatternId>,
    environment_captures: BTreeSet<(ExprId, usize)>,
    crossing_captures: BTreeMap<(ExprId, usize), Arc<Vec<CrossingBorrow>>>,
    environment_frames: BTreeSet<ExprId>,
    /// This graph is the body of a closure whose inferred result is local to
    /// its environment: its returns are checked as local results.
    environment_frame: bool,
    pattern_indices: Arc<BTreeMap<hirsel_syntax::PatternId, usize>>,
    let_patterns: Arc<BTreeSet<hirsel_syntax::PatternId>>,
    mutation_sites: Vec<ExprId>,
    replacement_live: BTreeMap<ExprId, Option<bool>>,
    /// Extraction is one static action at each source site. A converged join
    /// requiring a deep copy must not retain availability from earlier moves.
    unique_copies: BTreeSet<ExprId>,
    unique_moves: BTreeSet<ExprId>,
    retention_depth: usize,
    parameters: Vec<usize>,
    return_uniqueness: BTreeMap<ParamId, bool>,
    return_sendability: BTreeMap<ParamId, bool>,
    /// `mut x: unique T` parameters already reported for a lost hand-back.
    lost_handbacks: BTreeSet<usize>,
    /// Statements whose end releases a call's temporary with a user-visible
    /// destruction, which may fail (FAIL-3).
    destroying: BTreeSet<hirsel_syntax::StmtId>,
    confined: BTreeSet<usize>,
    confined_nodes: BTreeSet<usize>,
    /// Proving `@preserves_unique` on this parameter: it and every non-plain
    /// taken or `mut` parameter enter unique, and it must leave unique.
    unique_proof: Option<ParamId>,
    unique_results: Option<BTreeSet<UniqueResultRow>>,
    unique_failures: Vec<UniqueFailure>,
    arenas: crate::arenas::Arenas,
    /// The holders live after each graph node (LANG-3).
    live: &'a LiveSets,
    /// Each constant temporary and, once evaluated, its value. A plain fresh
    /// leaf's value depends on its expression alone, so it is held here once
    /// instead of in every state its temporary is live in.
    constants: BTreeMap<ExprId, Option<Value>>,
    /// The callable type this graph implements, once computed.
    callable: std::cell::OnceCell<Option<(FunctionType, bool)>>,
    /// Declared payload paths read only this graph's fixed generic context
    /// and the signatures retained before flow; transfers cannot change them.
    crossing_receipts: std::cell::RefCell<
        std::collections::HashMap<
            TypeId,
            Result<Vec<(Vec<Projection>, bool)>, QueryError>,
            hirsel_intern::IdHasher,
        >,
    >,
}

/// The diagnostic of a `@preserves_unique` body that cannot prove its promise.
pub(super) const UNIQUE_PROOF_FAILED: &str =
    "this body does not prove its @preserves_unique promise";

pub(super) struct FlowOutput {
    pub unique_results: Vec<UniqueResultRow>,
    pub unique_failures: Vec<UniqueFailure>,
    pub mutation_sites: Vec<ExprId>,
    pub replacements: BTreeMap<ExprId, Option<bool>>,
    pub return_uniqueness: BTreeMap<ParamId, bool>,
    pub return_sendability: BTreeMap<ParamId, bool>,
}

fn retention_depth(body: &CheckedBody) -> usize {
    let mut retention_depth = body
        .places
        .iter()
        .map(|p| p.projections.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let pattern_rows: BTreeMap<_, _> = body.patterns.iter().map(|p| (p.pattern, p)).collect();
    let pattern_children: BTreeSet<_> =
        body.patterns.iter().flat_map(|p| p.subpatterns()).collect();
    let mut patterns: Vec<_> = pattern_rows
        .keys()
        .filter(|p| !pattern_children.contains(p))
        .map(|p| (*p, 0))
        .collect();
    while let Some((pattern, depth)) = patterns.pop() {
        retention_depth = retention_depth.max(depth);
        if let Some(row) = pattern_rows.get(&pattern) {
            patterns.extend(row.subpatterns().map(|child| (child, depth + 1)));
        }
    }
    retention_depth
}

/// CLO-1: a closure type whose global result only a global environment would
/// justify, over an environment that is not, promises a local result.
fn environment_result(
    t: &TypeInterner,
    properties: &mut PropertyTable,
    ty: &mut hirsel_types::Type,
    arenas: &crate::arenas::Arenas,
) {
    if let TypeKind::Function(f) = &mut ty.kind {
        let mut result = t.get(f.result).unwrap().clone();
        result.modes.relation = arenas.relocate(&result.modes.relation, &Relation::environment());
        let old = f.result;
        f.result = t.intern(result);
        properties.inherit(f.result, old);
    }
}
/// The crossing mode of an environment holding both captures (MODE-4), as
/// `hirsel_types::instantiate_parameter_captures` joins them.
fn join_sendability(current: Sendability, actual: Sendability) -> Sendability {
    match (current, actual) {
        (Sendability::NotSendable, _) | (_, Sendability::NotSendable) => Sendability::NotSendable,
        (Sendability::Sendable, actual) => actual,
        (current, Sendability::Sendable) => current,
        (Sendability::DependsOn(ps), Sendability::DependsOn(other)) => {
            let mut ps = ps.to_vec();
            ps.extend(other.iter().copied());
            ps.sort();
            ps.dedup();
            Sendability::DependsOn(ps.into())
        }
    }
}
/// Flow refines a row's lifetime; its placement stays the row's own.
fn publish_row(
    t: &TypeInterner,
    _body: &mut CheckedBody,
    properties: &mut PropertyTable,
    root: TypeId,
    mut modes: Modes,
    environment: bool,
    arenas: &crate::arenas::Arenas,
) -> TypeId {
    let mut ty = t.get(root).unwrap().clone();
    modes.relation = arenas.relocate(&ty.modes.relation, &modes.relation);
    if environment {
        environment_result(t, properties, &mut ty, arenas);
    }
    if ty.modes == modes && !environment {
        return root;
    }
    ty.modes = modes;
    let id = t.intern(ty);
    properties.inherit(id, root);
    id
}

/// MODE-2: `root`'s row with each of `parts` aliased where the row claims
/// it unique, as an inferred construction's row may.
fn shared_parts_row(
    t: &TypeInterner,
    body: &mut CheckedBody,
    properties: &mut PropertyTable,
    root: TypeId,
    parts: &SharedParts,
) -> TypeId {
    parts.paths().fold(root, |root, path| {
        shared_part_row(t, body, properties, root, path)
    })
}
fn shared_part_row(
    t: &TypeInterner,
    body: &mut CheckedBody,
    properties: &mut PropertyTable,
    ty: TypeId,
    path: &[Projection],
) -> TypeId {
    let Some(mut row) = t.get(ty).cloned() else {
        return ty;
    };
    let Some(property) = properties.get(ty).copied() else {
        return ty;
    };
    match path.split_first() {
        None => {
            if row.modes.uniqueness != Uniqueness::Unique || property.plain || property.shared {
                return ty;
            }
            row.modes.uniqueness = Uniqueness::Aliased;
            if !property.sendable_aliased {
                row.modes.sendability = Sendability::NotSendable;
            }
        }
        Some((Projection::Field(index), rest)) => {
            let index = *index as usize;
            match &row.kind {
                TypeKind::Tuple(parts) if index < parts.len() => {
                    let part = shared_part_row(t, body, properties, parts[index], rest);
                    if part == parts[index] {
                        return ty;
                    }
                    let mut parts = parts.to_vec();
                    parts[index] = part;
                    row.kind = TypeKind::Tuple(parts.into());
                }
                TypeKind::Box(part) if index == 0 => {
                    let part = *part;
                    let weakened = shared_part_row(t, body, properties, part, rest);
                    if weakened == part {
                        return ty;
                    }
                    row.kind = TypeKind::Box(weakened);
                }
                _ => return ty,
            }
        }
        Some(_) => return ty,
    }
    let id = t.intern(row);
    properties.insert(id, property);
    id
}

fn fold_part_locality(
    arenas: &crate::arenas::Arenas,
    root: Relation,
    parts: &[(Relation, bool)],
) -> Relation {
    parts.iter().fold(root, |owner, (declared, plain)| {
        if *plain {
            Relation::global()
        } else if arenas.global(declared) {
            owner
        } else {
            declared.clone()
        }
    })
}

/// Borrowed frame inputs whose proof is consumed at a qualified call. This is
/// private capture validation, separate from generic signature admissibility.
pub(super) fn capture_crossing_uses(
    t: &TypeInterner,
    body: &CheckedBody,
    graphs: &[Graph],
    rows: &super::source_rows::SourceRows,
    own: &Signature,
    signatures: &super::properties::Signatures<'_>,
) -> Result<BTreeMap<ExprId, Vec<(usize, Vec<Projection>)>>, QueryError> {
    let mut result = BTreeMap::new();
    for graph in graphs {
        let NameSite::Expression(expression) = graph.owner else {
            continue;
        };
        let captures: BTreeSet<_> = graph
            .captures
            .iter()
            .filter_map(|(owner, binding, _)| rows.binding(body, Some(*owner), binding))
            .collect();
        let mut uses = Vec::new();
        for node in &graph.nodes {
            let Op::Call {
                expression,
                callee,
                arguments,
                requirements,
                ..
            } = &node.op
            else {
                continue;
            };
            for (argument, requirement) in arguments.iter().zip(requirements) {
                let Some(requirement) = requirement else {
                    continue;
                };
                let Some(place) =
                    place(body, argument.value, rows).filter(|p| captures.contains(&p.binding))
                else {
                    continue;
                };
                let mut expected = requirement.ty;
                if let Some(call) = rows.call(*expression).map(|index| &body.calls[index]) {
                    if let Some(Type {
                        kind: TypeKind::Function(function),
                        ..
                    }) = rows
                        .expression(*callee)
                        .and_then(|index| t.get(body.expressions[index].ty))
                    {
                        if let Some(qualified) = generic_crossing_parameter(
                            t,
                            signatures.m,
                            function,
                            requirement.parameter as usize,
                            &call.generic_arguments,
                            &own.metadata.roles,
                        )
                        .map_err(|error| {
                            QueryError::Operational(OperationalFailure {
                                operation: "capture crossing contract".into(),
                                message: format!("{error:?}"),
                            })
                        })? {
                            expected = qualified;
                        }
                    }
                }
                let mut paths = owned_crossing_receipts(
                    t,
                    expected,
                    &body.resolution.generic_declarations,
                    &mut |d| Ok(signatures.cached(d)),
                )?
                .into_iter()
                .filter(|(_, stable)| !stable)
                .map(|(path, _)| {
                    path.into_iter()
                        .map(|part| match part {
                            OwnedTypePart::Field(field) => Projection::Field(field),
                            OwnedTypePart::VariantField { variant, field } => {
                                Projection::VariantField { variant, field }
                            }
                            OwnedTypePart::Elements => Projection::Elements,
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
                if requirement.modes.sendability == Sendability::Sendable
                    || requirement.sendable_bound
                {
                    paths.push(vec![]);
                }
                for suffix in paths {
                    let mut path = place.projections.clone();
                    path.extend(suffix);
                    let key = (place.binding, path);
                    if !uses.contains(&key) {
                        uses.push(key);
                    }
                }
            }
        }
        if !uses.is_empty() {
            result.insert(expression, uses);
        }
    }
    Ok(result)
}

pub(super) struct Prepared {
    constants: BTreeSet<ExprId>,
    live: LiveSets,
    order: Vec<usize>,
    rank: Vec<usize>,
}

/// Demand and traversal depend on the checked graph and structural properties,
/// independently of the uniqueness selection used for a particular entry.
pub(super) fn prepare(
    t: &TypeInterner,
    syntax: &ItemSyntax,
    body: &CheckedBody,
    source_rows: &super::source_rows::SourceRows,
    properties: &PropertyTable,
    projection_lifetimes: &BTreeMap<ExprId, super::properties::ProjectionLifetime>,
    graph: &Graph,
) -> Prepared {
    let tree = &syntax.tree;
    let constants =
        constant_temporaries(graph, body, source_rows, properties, projection_lifetimes);
    let live = super::liveness::Liveness {
        t,
        tree,
        body,
        rows: source_rows,
        properties,
        graph,
        constants: &constants,
    }
    .after();
    let successors: Vec<Vec<usize>> = graph
        .nodes
        .iter()
        .map(|node| node.next.iter().rev().copied().collect())
        .collect();
    let order: Vec<_> =
        super::liveness::graph_postorder(graph.nodes.len(), graph.entry, |id| &successors[id])
            .into_iter()
            .rev()
            .collect();
    let mut rank = vec![0; graph.nodes.len()];
    for (index, &id) in order.iter().enumerate() {
        rank[id] = index;
    }
    Prepared {
        constants,
        live,
        order,
        rank,
    }
}

pub(super) fn check(
    t: &TypeInterner,
    syntax: &ItemSyntax,
    own: &Signature,
    body: &mut CheckedBody,
    signatures: &super::properties::Signatures<'_>,
    properties: &mut PropertyTable,
    initial_storage: &mut InitialStorage,
    source_rows: &super::source_rows::SourceRows,
    result_sharing: &BTreeSet<ExprId>,
    projection_lifetimes: &BTreeMap<ExprId, super::properties::ProjectionLifetime>,
    pattern_declarations: &BTreeMap<hirsel_syntax::PatternId, TypeId>,
    field_loan_places: &BTreeMap<ExprId, super::properties::FieldPlace>,
    graph: Graph,
    prepared: Option<&Prepared>,
    capture_crossing_uses: &BTreeMap<ExprId, Vec<(usize, Vec<Projection>)>>,
    facts: &mut OwnershipFacts,
    diagnostics: &mut Vec<Diagnostic>,
    frame_results: &BTreeMap<NameSite, TypeId>,
    introductions: &BTreeMap<NameSite, Vec<(NameSite, Origin, usize)>>,
    proof: Option<ParamId>,
) -> Result<FlowOutput, QueryError> {
    let tree = &syntax.tree;
    let local = prepared.is_none().then(|| {
        prepare(
            t,
            syntax,
            body,
            source_rows,
            properties,
            projection_lifetimes,
            &graph,
        )
    });
    let prepared = prepared.or(local.as_ref()).unwrap();
    let live = &prepared.live;
    let parameters = initial_storage
        .owners
        .get(&graph.owner)
        .into_iter()
        .flatten()
        .copied()
        .filter(|index| matches!(body.bindings[*index].binding, ResolvedName::Parameter(_)))
        .collect();
    let capture_expression = match graph.owner {
        NameSite::Expression(e) => Some(e),
        NameSite::Statement(s) => match tree.statements[s.0 as usize].kind {
            hirsel_syntax::StmtKind::Function { body, .. } => Some(body),
            _ => None,
        },
        _ => None,
    };
    let capture_layout = capture_expression
        .and_then(|e| initial_storage.closures.get(&e))
        .map(|i| &body.closures[*i]);
    let mut capture_physical_modes = BTreeMap::new();
    for (owner, binding, ordinal) in &graph.captures {
        for index in initial_storage
            .bindings
            .get(&(*owner, BindingKey::of(binding)))
            .into_iter()
            .flatten()
        {
            if let Some(capture) = capture_layout.and_then(|c| c.captures.get(*ordinal)) {
                capture_physical_modes
                    .entry(*index)
                    .or_insert_with(|| t.get(capture.ty).unwrap().modes.clone());
            }
        }
    }
    let arenas = crate::arenas::Arenas::new(t, body, own);
    let parameter_parts = parameter_parts::ParameterParts::new(body, &graph, source_rows);
    let cursor_observations = cursors::Observations::new(body, t, properties);
    let mut checker = Checker {
        t,
        arenas,
        source_rows,
        syntax,
        tree,
        explanations: vec![],
        trace: None,
        node: 0,
        own,
        body,
        signatures,
        properties,
        result_sharing,
        projection_lifetimes,
        pattern_declarations,
        field_loan_places,
        graph: &graph,
        parameter_parts,
        cursor_observations,
        capture_crossing_uses,
        facts,
        introductions,
        diagnostics,
        emit: false,
        callable: std::cell::OnceCell::new(),
        crossing_receipts: Default::default(),
        proof_error: std::cell::RefCell::new(None),
        unique_parts_cache: Default::default(),
        modes: BTreeMap::new(),
        forwarded: BTreeMap::new(),
        binding_modes: BTreeMap::new(),
        shared_parts: BTreeMap::new(),
        binding_parts: BTreeMap::new(),
        pattern_modes: BTreeMap::new(),
        physical_modes: BTreeMap::new(),
        capture_physical_modes,
        frame_results,
        environment_results: BTreeSet::new(),
        environment_bindings: BTreeSet::new(),
        environment_patterns: BTreeSet::new(),
        environment_captures: BTreeSet::new(),
        crossing_captures: BTreeMap::new(),
        environment_frames: BTreeSet::new(),
        environment_frame: capture_expression
            .is_some_and(|e| initial_storage.environment_frames.contains(&e)),
        pattern_indices: initial_storage.patterns.clone(),
        let_patterns: initial_storage.let_patterns.clone(),
        mutation_sites: Vec::new(),
        replacement_live: BTreeMap::new(),
        unique_copies: BTreeSet::new(),
        unique_moves: BTreeSet::new(),
        retention_depth: initial_storage.retention_depth,
        parameters,
        return_uniqueness: BTreeMap::new(),
        return_sendability: BTreeMap::new(),
        lost_handbacks: BTreeSet::new(),
        destroying: BTreeSet::new(),
        confined: BTreeSet::new(),
        confined_nodes: BTreeSet::new(),
        unique_proof: proof,
        unique_results: None,
        unique_failures: vec![],
        live,
        constants: prepared
            .constants
            .iter()
            .copied()
            .map(|e| (e, None))
            .collect(),
    };
    checker.destroying = graph
        .statements
        .temporaries
        .iter()
        .filter(|(expression, _)| {
            matches!(
                tree.expressions[expression.0 as usize].kind,
                ExprKind::Call { .. }
            ) && checker.destroyed(**expression)
        })
        .map(|(_, statement)| *statement)
        .collect();
    checker.confined_nodes = checker.confined_prefix();
    checker.confined = checker.confined_candidates();
    let initial = checker.proof_entry(initial_storage.initial(&checker));
    let finalizer = graph.owner == NameSite::Item
        && matches!(
            syntax.item.kind,
            hirsel_syntax::ItemKind::Type { drop: Some(_), .. }
        );
    for &index in initial_storage
        .owners
        .get(&graph.owner)
        .into_iter()
        .flatten()
    {
        let b = &mut checker.body.bindings[index];
        if matches!(b.binding, ResolvedName::Parameter(_)) || finalizer && b.site == graph.owner {
            checker
                .binding_modes
                .insert(index, initial.slots[index].value.modes.clone());
            b.usage = if b.mutable {
                UseKind::Mut
            } else if initial.slots[index].value.aliased().is_some() {
                UseKind::Borrow
            } else if checker.properties[b.ty].plain {
                UseKind::Copy
            } else {
                UseKind::Take
            };
        }
    }
    // Each site publishes one extraction action, even when its loop executes
    // first with unique content and later with shared content. The first solve
    // can propagate MOVED before discovering that the final action is a copy.
    // Restart only those changed sites; retaining their moves would make the
    // replacement facts disagree with the emitted copy. Each restart adds a
    // site to a finite set, with no iteration cap or weakened availability.
    let inputs = loop {
        let inputs = solve(&mut checker, initial.clone(), prepared);
        let mut changed = false;
        for (node, input) in graph.nodes.iter().zip(&inputs) {
            let (Op::Finish { expression, .. }, Some(input)) = (&node.op, input) else {
                continue;
            };
            let ExprKind::Unary {
                op: UnaryOp::Unique,
                value: child,
            } = tree.expressions[expression.0 as usize].kind
            else {
                continue;
            };
            let value = checker.value(input, child);
            if value.deep_copy && checker.unique_moves.contains(&child) {
                changed |= checker.unique_copies.insert(child);
            }
        }
        if !changed {
            break inputs;
        }
    };
    checker.emit = true;
    for (id, input) in inputs.iter().cloned().enumerate() {
        checker.node = id;
        if let Some(mut input) = input {
            checker.transfer(&graph.nodes[id].op, &mut input);
        } else {
            checker.unreachable_facts(&graph.nodes[id].op);
        }
    }
    if !checker.explanations.is_empty() {
        // Only a graph that reported a diagnostic pays for its explanation.
        checker.explain(inputs);
        checker.emit = true;
    }
    // Node storage order is not evaluation order. Publish extraction facts
    // after all uses so a child use cannot overwrite its enclosing unique
    // decision. Deep copy reads the source without consuming its slot.
    for decision in checker.facts.unique.clone() {
        let ExprKind::Unary { value: child, .. } =
            tree.expressions[decision.expression.0 as usize].kind
        else {
            continue;
        };
        let extraction = decision.action.usage();
        checker.usage(child, extraction);
    }
    // Published modes are judgments at a use, not mutable arena rows shared by
    // unrelated expressions. Append one refined immutable row per changed type.
    for (expression, modes) in checker.modes {
        if let Some(index) = checker.source_rows.expression(expression) {
            let ty = publish_row(
                t,
                checker.body,
                checker.properties,
                checker.body.expressions[index].ty,
                modes,
                checker.environment_results.contains(&expression),
                &checker.arenas,
            );
            let ty = match checker.shared_parts.get(&expression) {
                Some(parts) => shared_parts_row(t, checker.body, checker.properties, ty, parts),
                None => ty,
            };
            // An expression publishes current evidence. Its source's written
            // root requirement belongs to that storage, not to a new receipt.
            // Owned part contracts remain in the graph.
            let ty = if checker.t.get(ty).unwrap().sendability_requirement
                != SendabilityRequirement::None
            {
                let mut row = checker.t.get(ty).unwrap().clone();
                row.sendability_requirement = SendabilityRequirement::None;
                let root = checker.t.intern(row);
                checker.properties.inherit(root, ty);
                root
            } else {
                ty
            };
            let row = &mut checker.body.expressions[index];
            row.ty = ty;
            if matches!(
                tree.expressions[expression.0 as usize].kind,
                ExprKind::Closure { .. }
            ) {
                if let Some(index) = checker.source_rows.closure(expression) {
                    checker.body.closures[index].ty = row.ty;
                }
            }
        }
    }
    for (expression, modes) in checker.physical_modes {
        let plan = &checker.projection_lifetimes[&expression];
        let mut ty = checker.t.get(plan.physical).unwrap().clone();
        let relation = checker.arenas.relocate(&ty.modes.relation, &modes.relation);
        ty.modes = modes;
        ty.modes.relation = relation;
        let physical = checker.t.intern(ty);
        checker.properties.inherit(physical, plan.physical);
        checker
            .facts
            .physical_projections
            .push((expression, physical));
    }
    for pattern in checker.environment_patterns.clone() {
        checker.pattern_modes.entry(pattern).or_insert_with(|| {
            let row = &checker.body.patterns[checker.pattern_indices[&pattern]];
            checker.t.get(row.ty).unwrap().modes.clone()
        });
    }
    for (pattern, modes) in checker.pattern_modes {
        let index = checker.pattern_indices[&pattern];
        checker.body.patterns[index].ty = publish_row(
            t,
            checker.body,
            checker.properties,
            checker.body.patterns[index].ty,
            modes,
            checker.environment_patterns.contains(&pattern),
            &checker.arenas,
        );
    }
    let mut updated_bindings = vec![];
    for (binding, modes) in checker.binding_modes {
        let previous = checker.body.bindings[binding].ty;
        let ty = publish_row(
            t,
            checker.body,
            checker.properties,
            previous,
            modes,
            checker.environment_bindings.contains(&binding),
            &checker.arenas,
        );
        let ty = match checker.binding_parts.get(&binding) {
            Some(parts) => shared_parts_row(t, checker.body, checker.properties, ty, parts),
            None => ty,
        };
        let row = &mut checker.body.bindings[binding];
        row.ty = ty;
        if ty != previous {
            updated_bindings.push(binding);
        }
        if let NameSite::Statement(statement) = row.site {
            if !matches!(row.binding, ResolvedName::Local(_)) {
                continue;
            }
            if let hirsel_syntax::StmtKind::Function { body, .. } =
                tree.statements[statement.0 as usize].kind
            {
                if let Some(index) = checker.source_rows.closure(body) {
                    checker.body.closures[index].ty = row.ty;
                }
            }
        }
    }
    for binding in updated_bindings {
        initial_storage.refresh(t, binding, checker.body, checker.properties);
    }
    initial_storage
        .environment_captures
        .extend(checker.environment_captures);
    initial_storage
        .crossing_captures
        .extend(checker.crossing_captures);
    initial_storage
        .environment_frames
        .extend(checker.environment_frames);
    if let Some(error) = checker.proof_error.into_inner() {
        return Err(error);
    }
    Ok(FlowOutput {
        unique_failures: checker.unique_failures,
        unique_results: checker
            .unique_results
            .unwrap_or_else(|| {
                checker
                    .graph
                    .result_ty
                    .map(|root| {
                        hirsel_types::unique_summary::result_rows(
                            t,
                            root,
                            &own.metadata.roles,
                            &mut |d| signatures.cached(d),
                        )
                        .into_iter()
                        .map(|(row, _)| row)
                        .collect()
                    })
                    .unwrap_or_default()
            })
            .into_iter()
            .collect(),
        mutation_sites: checker.mutation_sites,
        replacements: checker.replacement_live,
        return_uniqueness: checker.return_uniqueness,
        return_sendability: checker.return_sendability,
    })
}
/// The temporaries of fresh leaves with a plain type: a literal or a nullary
/// constructor. Their value is the same on every path and in every state, and
/// they hold no loan, so neither the states nor the live sets carry them; a
/// table literal's elements would otherwise make both quadratic in its length.
fn constant_temporaries(
    graph: &Graph,
    body: &CheckedBody,
    rows: &super::source_rows::SourceRows,
    properties: &PropertyTable,
    projection_lifetimes: &BTreeMap<ExprId, super::properties::ProjectionLifetime>,
) -> BTreeSet<ExprId> {
    graph
        .nodes
        .iter()
        .filter_map(|node| match &node.op {
            Op::Finish {
                expression,
                children,
                fresh: true,
                ..
            } if children.is_empty() && !projection_lifetimes.contains_key(expression) => {
                Some(*expression)
            }
            _ => None,
        })
        .filter(|expression| {
            expression_type(body, *expression, rows)
                .and_then(|ty| properties.get(ty))
                .is_some_and(|p| p.valid && p.plain)
        })
        .collect()
}
/// The converged input of every node.
fn solve(checker: &mut Checker<'_, '_>, initial: State, prepared: &Prepared) -> Vec<Option<State>> {
    let graph = checker.graph;
    let mut inputs: Vec<Option<State>> = vec![None; graph.nodes.len()];
    inputs[graph.entry] = Some(initial);
    let Prepared {
        order, rank, live, ..
    } = prepared;
    let mut slots = SlotJoins::new(Slot::join);
    let mut pending = Pending::new(graph.nodes.len());
    pending.insert(rank[graph.entry]);
    while let Some(index) = pending.pop_first() {
        let id = order[index];
        work::count(Counter::OwnershipFlowSteps);
        let mut output = inputs[id].clone().unwrap();
        work::count_by(Counter::OwnershipStateRows, output.values.len() as u64);
        checker.node = id;
        checker.transfer(&graph.nodes[id].op, &mut output);
        checker.release_temporaries(&mut output, live.at(id));
        forward_output(
            output,
            &graph.nodes[id].next,
            &mut inputs,
            &mut |successor| pending.insert(rank[successor]),
            &mut slots,
        );
    }
    inputs
}
#[cfg(test)]
thread_local! { static FORWARD_STATE_CLONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn forward_output(
    output: State,
    next: &[usize],
    inputs: &mut [Option<State>],
    schedule: &mut impl FnMut(usize),
    slots: &mut SlotJoins,
) {
    // Rustc19833af7 mir_dataflow/framework/mod.rs:351-375 minimizes
    // state copies around propagation. A new final target can take ownership:
    // no later edge needs this output, and earlier snapshots stay shared.
    for (index, successor) in next.iter().enumerate() {
        if index + 1 == next.len() && inputs[*successor].is_none() {
            inputs[*successor] = Some(output);
            schedule(*successor);
            return;
        }
        let changed = if let Some(input) = &mut inputs[*successor] {
            input.join(&output, slots)
        } else {
            #[cfg(test)]
            FORWARD_STATE_CLONES.with(|n| n.set(n.get() + 1));
            inputs[*successor] = Some(output.clone());
            true
        };
        if changed {
            schedule(*successor);
        }
    }
}

impl Checker<'_, '_> {
    fn finalizer_self(&self, index: usize) -> bool {
        let binding = &self.body.bindings[index];
        binding.site == binding.owner
            && matches!(binding.binding, ResolvedName::Local(_))
            && binding.owner == NameSite::Item
            && matches!(
                self.syntax.item.kind,
                hirsel_syntax::ItemKind::Type { drop: Some(_), .. }
            )
    }
    fn parameter_passing(&self, owner: &NameSite, parameter: ParamId) -> PassingMode {
        if parameter == ParamId(0)
            && let NameSite::Expression(literal) = owner
            && self.body.calls.iter().any(|call| {
                let CallTarget::Definition(path) = &call.target else {
                    return false;
                };
                hirsel_types::core_roles::arena_scope_callback(path).is_some_and(|slot| {
                    call.arguments.iter().any(|argument| {
                        argument.expression == *literal
                            && argument.parameter.as_ref().is_some_and(|(p, _)| *p == slot)
                    })
                })
            })
        {
            return PassingMode::Take;
        }
        if let NameSite::Item = owner {
            if let SignatureKind::Function(f) = &self.own.kind {
                return f
                    .parameters
                    .get(parameter.0 as usize)
                    .map_or(PassingMode::Borrow, |p| p.passing);
            }
        }
        let syntax = match owner {
            NameSite::Expression(id) => match &self.tree.expressions[id.0 as usize].kind {
                ExprKind::Closure { signature, .. } => Some(signature),
                _ => None,
            },
            NameSite::Statement(id) => match &self.tree.statements[id.0 as usize].kind {
                hirsel_syntax::StmtKind::Function { signature, .. } => Some(signature),
                _ => None,
            },
            _ => None,
        };
        syntax
            .and_then(|s| s.parameters.get(parameter.0 as usize))
            .map_or(PassingMode::Borrow, |p| p.passing)
    }
    fn intern(&self, row: hirsel_types::Type) -> TypeId {
        self.t.intern(row)
    }
    fn ty(&self, expression: ExprId) -> Option<TypeId> {
        expression_type(self.body, expression, self.source_rows)
    }
    /// Whether `expression`'s value may be a closure holding borrowed
    /// captures: a function value, or a type parameter's, which one may
    /// instantiate. Its deep copy keeps the captures' loans.
    /// MODE-2/MODE-4: a deep copy builds fresh blocks that it owns alone, so it
    /// crosses exactly when its type does once unique, whatever its source's
    /// evidence. One that may keep borrowed captures keeps its source's proof,
    /// and one built in an arena or region never crosses (MEM-5).
    fn deep_copy_sendability(&self, expression: ExprId, value: &mut Value) {
        if !value.deep_copy || self.may_capture(expression) {
            return;
        }
        let Some(root) = self.ty(expression) else {
            return;
        };
        let mut row = self.t.get(root).unwrap().clone();
        if self.arenas.placed(&row.modes.relation) {
            value.modes.sendability = Sendability::NotSendable;
            return;
        }
        row.modes.uniqueness = Uniqueness::Unique;
        row.modes.sendability = Sendability::NotSendable;
        let root = self.intern(row);
        value.modes.sendability = self
            .signatures
            .prove_flow(|memo| hirsel_modes::proofs::content_sendability(memo, root))
            .unwrap_or_else(|error| {
                *self.proof_error.borrow_mut() = Some(error);
                Sendability::NotSendable
            });
    }
    fn may_capture(&self, expression: ExprId) -> bool {
        self.ty(expression)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| matches!(row.kind, TypeKind::Function(_) | TypeKind::Parameter(_)))
    }
    /// The captures a closure call's result may share (#80), when each is
    /// Global or arena-only storage: the callee is a closure literal, or an immutable
    /// binding of one. Plain and Arena captures share no content. The flag is
    /// set when a literal captures a taken parameter's storage, which, as its
    /// copy, a result must name (P2); a named closure owns its environment, as
    /// `let m = take z` owns z's storage. None when the captures are unknown
    /// or one may share other content.
    fn shared_captures(&self, state: &State, call: ExprId) -> Option<(Vec<usize>, bool)> {
        let ExprKind::Call { callee, .. } = &self.tree.expressions[call.0 as usize].kind else {
            return None;
        };
        let named = !matches!(
            self.tree.expressions[callee.0 as usize].kind,
            ExprKind::Closure { .. }
        );
        let literal = match &self.tree.expressions[callee.0 as usize].kind {
            ExprKind::Closure { .. } => *callee,
            _ => {
                let binding = place(self.body, *callee, self.source_rows)
                    .filter(|p| p.projections.is_empty())?
                    .binding;
                let decision = &self.body.bindings[binding];
                let NameSite::Pattern(pattern) = decision.site else {
                    return None;
                };
                if decision.mutable {
                    return None;
                }
                let value = self.tree.statements.iter().find_map(|s| match &s.kind {
                    hirsel_syntax::StmtKind::Let {
                        pattern: p, value, ..
                    } if *p == pattern => Some(*value),
                    _ => None,
                })?;
                matches!(
                    self.tree.expressions[value.0 as usize].kind,
                    ExprKind::Closure { .. }
                )
                .then_some(value)?
            }
        };
        let closure = self
            .body
            .closures
            .iter()
            .find(|c| c.expression == literal)?;
        let (mut shared, mut taken) = (vec![], false);
        for capture in &closure.captures {
            let binding = self.source_rows.binding(
                self.body,
                Some(self.graph.owner.clone()),
                &capture.binding,
            )?;
            let ty = self.body.bindings[binding].ty;
            let arena = self
                .arenas
                .definition
                .as_ref()
                .is_some_and(|arena| hirsel_types::is_arena_type(self.t, ty, arena));
            if arena || self.properties[ty].plain {
                continue;
            }
            taken |= !named && self.taken_storage(state, binding);
            if !self
                .arenas
                .global(&state.slots[binding].value.modes.relation)
            {
                return None;
            }
            shared.push(binding);
        }
        Some((shared, taken))
    }
    /// The first non-plain place of an outer binding in the transparent tail
    /// of a block, branch or arm (LANG-11, #50). A binding the block declares
    /// leaves it, so its tail moves out; a written operator is its own use.
    /// Whether a place the value may be, through written use modes,
    /// transparent tails (LANG-11) and closure calls, is a taken parameter's
    /// storage: a copy of it in a block is still that storage for a result
    /// (P2).
    fn tail_taken(&self, state: &State, expression: ExprId) -> bool {
        let mut positions = vec![expression];
        while let Some(position) = positions.pop() {
            match &self.tree.expressions[position.0 as usize].kind {
                ExprKind::Block { tail, .. } => {
                    positions.extend(tail);
                    positions.extend(self.graph.block_breaks.get(&position).into_iter().flatten());
                }
                ExprKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    positions.extend(else_branch);
                    positions.push(*then_branch);
                }
                ExprKind::Match { arms, .. } => positions.extend(arms.iter().map(|arm| arm.body)),
                ExprKind::Unary {
                    op: UnaryOp::Copy | UnaryOp::Take | UnaryOp::Mut | UnaryOp::Unique,
                    value,
                } => positions.push(*value),
                // A closure call may share what its environment captures
                // (#80); an unknown environment is the callee's storage.
                ExprKind::Call { callee, .. }
                    if self.source_rows.call(position).is_some_and(|i| {
                        matches!(self.body.calls[i].target, CallTarget::Value(_))
                    }) =>
                {
                    match self.shared_captures(state, position) {
                        Some((_, true)) => return true,
                        Some(_) => {}
                        None => positions.push(*callee),
                    }
                }
                _ => {
                    if place(self.body, position, self.source_rows)
                        .is_some_and(|p| self.taken_storage(state, p.binding))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
    fn outer_place_tail(&self, expression: ExprId) -> Option<ExprId> {
        if !matches!(
            self.tree.expressions[expression.0 as usize].kind,
            ExprKind::Block { .. } | ExprKind::If { .. } | ExprKind::Match { .. }
        ) {
            return None;
        }
        let mut positions = vec![expression];
        while let Some(position) = positions.pop() {
            match &self.tree.expressions[position.0 as usize].kind {
                ExprKind::Block { tail, .. } => {
                    positions.extend(tail);
                    positions.extend(self.graph.block_breaks.get(&position).into_iter().flatten());
                }
                ExprKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    positions.extend(else_branch);
                    positions.push(*then_branch);
                }
                ExprKind::Match { arms, .. } => {
                    positions.extend(arms.iter().rev().map(|arm| arm.body))
                }
                ExprKind::Unary { .. } => {}
                _ => {
                    if self.source_rows.source_place(position).is_some()
                        && place(self.body, position, self.source_rows).is_some()
                        && !self.graph.leaving.contains(&position)
                        && !self.property(position).is_some_and(|p| p.plain)
                    {
                        return Some(position);
                    }
                }
            }
        }
        None
    }
    fn property(&self, expression: ExprId) -> Option<Properties> {
        self.ty(expression)
            .and_then(|id| self.properties.get(id).copied())
            .filter(|p| p.valid)
    }
    /// MODE-4, SEM-4: an erased environment captures an argument's current
    /// value, which the validator reads from the operand row this pass
    /// publishes. A binding's receiving contract, such as a let relation's
    /// frame bound, does not make the captured content local (#88).
    fn crossing_borrows_live(&self, state: &State, value: &Value) -> bool {
        let mut pending = value.crossing_borrows.as_ref().clone();
        let mut seen = BTreeSet::new();
        while let Some(borrow) = pending.pop() {
            let current = &state.slots[borrow.binding].value;
            if borrow.source.is_empty() {
                if current.modes.sendability != Sendability::Sendable {
                    return false;
                }
            } else if current
                .crossing_parts
                .get(&borrow.source)
                .is_some_and(|proof| proof != Sendability::Sendable)
            {
                return false;
            }
            if seen.insert(borrow.binding) {
                pending.extend(current.crossing_borrows.iter().cloned());
            }
        }
        true
    }
    fn captured_sendability(&self, expression: ExprId, value: &Value) -> Sendability {
        // The flow value owns current evidence. A declaration or old part row
        // cannot restore a proof invalidated by a real counted share/write.
        if value.modes.sendability == Sendability::NotSendable
            && value.sources().any(|source| {
                let binding = &self.body.bindings[source];
                matches!(binding.binding, ResolvedName::Parameter(p) if self.parameter_passing(&binding.owner, p) == PassingMode::Take)
                    && !self.arenas.global(&self.t.get(binding.ty).unwrap().modes.relation)
            }) {
            return Sendability::NotSendable;
        }
        if value.modes.sendability == Sendability::NotSendable
            && value.modes.uniqueness != Uniqueness::Unique
            && !self.property(expression).is_some_and(|p| p.plain || p.flat)
            && !self.sealed_owner(expression)
        {
            return Sendability::NotSendable;
        }
        let Some(root) = expression_type(self.body, expression, self.source_rows) else {
            return Sendability::NotSendable;
        };
        let mut row = self.t.get(root).unwrap().clone();
        if value.modes.sendability == Sendability::NotSendable
            && row.modes.relation.named_parameters().is_some()
        {
            return Sendability::NotSendable;
        }
        if self.arenas.placed(&row.modes.relation) {
            return Sendability::NotSendable;
        }
        let mut modes = value.modes.clone();
        modes.relation = self.arenas.relocate(&row.modes.relation, &modes.relation);
        let root = if row.modes == modes {
            root
        } else {
            row.modes = modes;
            self.intern(row)
        };
        self.signatures
            .prove_flow(|memo| hirsel_modes::proofs::content_sendability(memo, root))
            .unwrap_or_else(|error| {
                *self.proof_error.borrow_mut() = Some(error);
                Sendability::NotSendable
            })
    }
    /// MODE-4: a global construction whose every part currently crosses
    /// crosses too, as V proves a construction field by field. The typed row
    /// knows only the parts' declared rows, so a part unique by flow evidence
    /// alone (a parametric or `@preserves_unique` result) beside a handle part,
    /// which keeps the aggregate aliased, would otherwise lose its proof.
    fn parts_sendable(
        &self,
        state: &State,
        aggregate: ExprId,
        value: &Value,
        parts: &[ExprId],
    ) -> bool {
        let Some(root) = expression_type(self.body, aggregate, self.source_rows) else {
            return false;
        };
        if !self.arenas.global(&value.modes.relation)
            || self
                .arenas
                .placed(&self.t.get(root).unwrap().modes.relation)
        {
            return false;
        }
        parts.iter().all(|part| {
            self.property(*part).is_some_and(|p| p.plain)
                || self.captured_sendability(*part, &self.value(state, *part))
                    == Sendability::Sendable
        })
    }
    fn value(&self, state: &State, expression: ExprId) -> Value {
        state
            .values
            .get(&expression)
            .or_else(|| self.constants.get(&expression)?.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                Value::new(
                    self.ty(expression)
                        .and_then(|id| self.t.get(id))
                        .map_or_else(Modes::global, |t| t.modes.clone()),
                )
            })
    }
    /// An argument's value as the callee receives it. A borrowed or `mut`
    /// place is passed as itself, and the later arguments may still read and
    /// share it (LANG-3), so its uniqueness and crossing proof are the place's
    /// state at callee entry, after every argument, not its snapshot from
    /// evaluation (MODE-2).
    fn entry_value(&self, state: &State, argument: ExprId) -> Value {
        let mut actual = self.value(state, argument);
        if !matches!(actual.usage, UseKind::Borrow | UseKind::Mut) {
            return actual;
        }
        let Some(place) = place(self.body, argument, self.source_rows) else {
            return actual;
        };
        let current = &state.slots[place.binding].value;
        if !self.property(argument).is_some_and(|p| p.sendable_aliased) {
            let proof = if place.projections.is_empty() {
                Some(if current.modes.uniqueness == Uniqueness::Unique {
                    self.captured_sendability(argument, current)
                } else {
                    current.modes.sendability.clone()
                })
            } else {
                current.crossing_parts.get(&place.projections)
            };
            if let Some(proof) = proof {
                actual.modes.sendability = proof;
            }
        }
        if current.modes.uniqueness == Uniqueness::Aliased {
            actual.modes.uniqueness = Uniqueness::Aliased;
        }
        actual
    }
    fn projection_locality(
        &self,
        state: &State,
        projection: &super::properties::ProjectionLifetime,
    ) -> Relation {
        let root = match projection.root {
            super::properties::ProjectionRoot::Value(root) => {
                self.value(state, root).modes.relation.clone()
            }
            super::properties::ProjectionRoot::Binding(root) => {
                state.slots[root].value.modes.relation.clone()
            }
            super::properties::ProjectionRoot::Constant(root) => {
                self.t.get(root).unwrap().modes.relation.clone()
            }
        };
        fold_part_locality(&self.arenas, root, &projection.parts)
    }
    /// A `mut` binding written `once` (MODE-3).
    fn once_storage(&self, binding: usize) -> bool {
        let binding = &self.body.bindings[binding];
        let NameSite::Pattern(pattern) = binding.site else {
            return false;
        };
        binding.mutable
            && self
                .pattern_indices
                .get(&pattern)
                .and_then(|&row| self.body.patterns[row].annotation)
                .is_some_and(|written| written.once)
    }
    /// Explicit callable/Iter annotations retain their erased environment default.
    fn written_crossing_contract(&self, binding: usize) -> bool {
        let row = &self.body.bindings[binding];
        let NameSite::Pattern(pattern) = row.site else {
            return false;
        };
        row.mutable
            && self
                .pattern_indices
                .get(&pattern)
                .and_then(|&index| self.body.patterns[index].annotation)
                .is_some_and(|written| {
                    written.function
                        || matches!(
                            self.t.get(row.ty).unwrap().kind,
                            TypeKind::Builtin {
                                kind: BuiltinType::Iter,
                                ..
                            }
                        )
                })
    }
    /// A block yields its innermost tail's storage (#50), which lowering
    /// projects from, so a tail place is the physical root.
    fn tail_root_modes(&self, state: &State, expression: ExprId) -> Option<Modes> {
        let mut tail = expression;
        // A labelled block joins its breaks into its own storage (LANG-11).
        while let ExprKind::Block {
            label: None,
            tail: Some(inner),
            ..
        } = self.tree.expressions[tail.0 as usize].kind
        {
            tail = inner;
        }
        if tail == expression || self.source_rows.source_place(tail).is_none() {
            return None;
        }
        if let Some(plan) = self.projection_lifetimes.get(&tail) {
            return Some(self.physical_projection_modes(state, plan));
        }
        place(self.body, tail, self.source_rows)
            .filter(|place| place.projections.is_empty())
            .map(|place| {
                self.physical_root_modes(
                    state,
                    super::properties::ProjectionRoot::Binding(place.binding),
                )
            })
    }
    fn physical_root_modes(&self, state: &State, root: super::properties::ProjectionRoot) -> Modes {
        use super::properties::ProjectionRoot;
        match root {
            ProjectionRoot::Value(expression) => self
                .tail_root_modes(state, expression)
                .unwrap_or_else(|| self.value(state, expression).modes.clone()),
            ProjectionRoot::Constant(root) => self.t.get(root).unwrap().modes.clone(),
            ProjectionRoot::Binding(index) => {
                let binding = &self.body.bindings[index];
                if let Some(modes) = self.capture_physical_modes.get(&index) {
                    return modes.clone();
                }
                // Frame inputs are physically published as read views, unlike
                // ordinary local storage whose receiving lifetime is fixed.
                if matches!(binding.binding, ResolvedName::Parameter(_))
                    || self.finalizer_self(index)
                {
                    if let Some(modes) = self.binding_modes.get(&index) {
                        return modes.clone();
                    }
                }
                // Ordinary storage keeps its receiving lifetime. Owned
                // acquisition publishes stronger current content separately.
                let mut modes = self.t.get(binding.ty).unwrap().modes.clone();
                modes.relation = state.slots[index].declared.clone();
                modes
            }
        }
    }
    /// Lowering projects every step of a place chain (`xs[0]` in
    /// `xs[0].n`), so each step publishes its physical row, not only the
    /// place that is read. A computed root, such as a call result, is
    /// consumed by the innermost step, which publishes its row while the root
    /// is live. An outer step, reached after that, folds its own part over
    /// its live base step, whose value carries the root's lifetime.
    fn record_physical(&mut self, state: &State, mut expression: ExprId) {
        while let Some(plan) = self.projection_lifetimes.get(&expression) {
            let base = match self.tree.expressions[expression.0 as usize].kind {
                ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => Some(base),
                _ => None,
            };
            let consumed = matches!(plan.root, super::properties::ProjectionRoot::Value(root)
                if !state.values.contains_key(&root) && self.tail_root_modes(state, root).is_none());
            let modes = if !consumed {
                Some(self.physical_projection_modes(state, plan))
            } else {
                base.filter(|base| self.projection_lifetimes.contains_key(base))
                    .and_then(|base| state.values.get(&base))
                    .map(|value| {
                        let mut modes = plan.declared.clone();
                        modes.relation = fold_part_locality(
                            &self.arenas,
                            value.modes.relation.clone(),
                            &plan.parts[plan.parts.len() - 1..],
                        );
                        modes
                    })
            };
            if let Some(modes) = modes {
                self.physical_modes.insert(expression, modes);
            }
            let Some(base) = base else { break };
            expression = base;
        }
    }
    fn physical_projection_modes(
        &self,
        state: &State,
        plan: &super::properties::ProjectionLifetime,
    ) -> Modes {
        let mut modes = plan.declared.clone();
        modes.relation = fold_part_locality(
            &self.arenas,
            self.physical_root_modes(state, plan.root).relation,
            &plan.parts,
        );
        modes
    }
    /// Join two lifetimes as a call result names them: an equal lifetime is
    /// kept and differing ones are local to this frame.
    /// MODE-1, caller side: content moved in through any relation-free `mut`
    /// parameter may come back out through any of them, or as the result. So
    /// after the call the result and each relation-free `mut` argument's
    /// content are bounded by the join of every such argument's content, as
    /// well as by what they already were. Nothing is required of the
    /// arguments, and callee bodies keep treating that content as global.
    fn mut_content_bounds(
        &mut self,
        state: &mut State,
        value: &mut Value,
        expression: ExprId,
        arguments: &[hirsel_syntax::Argument],
        requirements: &[Option<super::cfg::Requirement>],
    ) {
        // (content, its type, the storage binding it was passed from)
        let mut contents = vec![];
        for (argument, requirement) in arguments.iter().zip(requirements) {
            let Some(requirement) = requirement else {
                continue;
            };
            if requirement.passing != PassingMode::Mut
                || requirement
                    .modes
                    .relation
                    .reaches()
                    .iter()
                    .any(|r| matches!(r, Reach::Place(_)))
                || self.property(argument.value).is_some_and(|p| p.plain)
            {
                continue;
            }
            let Some(ty) = self.ty(argument.value) else {
                continue;
            };
            let mut content = self.value(state, argument.value);
            let mut storage = None;
            if let Some(p) = place(self.body, argument.value, self.source_rows) {
                // A `mut` parameter's content is held to its declared
                // receiving contract; being local to itself is its access
                // root, which ends with this body, not where it lives.
                let binding = &self.body.bindings[p.binding];
                if let ResolvedName::Parameter(parameter) = binding.binding
                    && self.parameter_passing(&binding.owner, parameter) == PassingMode::Mut
                {
                    content.modes.relation = self.receiving_contract(p.binding);
                }
                storage = Some(p.binding);
            }
            contents.push((content, ty, storage));
        }
        let local = |this: &Self, content: &Value| {
            !this.arenas.global(&content.modes.relation)
                || content
                    .content
                    .summary()
                    .scopes()
                    .any(|(kind, _, _)| kind == Kind::Bound)
        };
        if !contents.iter().any(|(content, _, _)| local(self, content)) {
            return;
        }
        // Content reaches only storage whose type can hold a part of it.
        let store = self.t;
        let signatures = &self.signatures;
        let mut lookup = |path: hirsel_syntax::DefId| signatures.cached(path);
        let mut sources = |holder: TypeId| -> Vec<usize> {
            contents
                .iter()
                .enumerate()
                .filter(|(_, (content, ty, _))| {
                    local(self, content)
                        && hirsel_type_analysis::representation::may_hold_content(
                            store,
                            signatures.type_memo(),
                            holder,
                            *ty,
                            &mut lookup,
                        )
                })
                .map(|(i, _)| i)
                .collect()
        };
        let result = (!self.property(expression).is_some_and(|p| p.plain))
            .then(|| self.ty(expression))
            .flatten()
            .map(|ty| sources(ty));
        let holders: Vec<_> = contents
            .iter()
            .filter_map(|(_, ty, storage)| Some((storage.as_ref().copied()?, sources(*ty))))
            .collect();
        // What a content holds, without the loans on its own storage, which
        // the call ends: its lifetime, its bounds and the places it reads.
        let contents: Vec<_> = contents
            .into_iter()
            .map(|(mut content, _, storage)| {
                let mut whole = content.content.summary().into_owned();
                whole.retain(|loan| !matches!(loan.on, On::Binding(b) if Some(b) == storage));
                content.content = Content { whole, parts: None };
                content
            })
            .collect();
        let bounded = |this: &Self, target: &mut Value, from: &[usize]| {
            for i in from {
                let content = &contents[*i];
                if !this.arenas.global(&content.modes.relation) {
                    target.modes.relation =
                        this.join_lifetime(&target.modes.relation, &content.modes.relation);
                }
                target.bound_within(content, false);
                target.read_through(content);
            }
        };
        if let Some(from) = result {
            bounded(self, value, &from);
        }
        for (binding, from) in holders {
            let mut slot = state.slots[binding].value.clone();
            bounded(self, &mut slot, &from);
            state.slots[binding].value = slot;
        }
    }
    /// The intrinsic's checked parameter ordinals, independently of argument order.
    fn replacement_operands(
        &self,
        callee: ExprId,
        arguments: &[hirsel_syntax::Argument],
        requirements: &[Option<super::cfg::Requirement>],
    ) -> Option<(ExprId, ExprId)> {
        if self
            .callee_definition(callee)
            .and_then(hirsel_types::core_roles::LoweredIntrinsic::for_definition)
            != Some(hirsel_types::core_roles::LoweredIntrinsic::Replace)
        {
            return None;
        }
        let argument = |parameter| {
            arguments.iter().zip(requirements).find_map(|(a, r)| {
                r.as_ref()
                    .filter(|r| r.parameter == parameter)
                    .map(|_| a.value)
            })
        };
        Some((argument(0)?, argument(1)?))
    }
    /// `replace` writes the destination's fixed receiving storage. Its
    /// inferred type argument can be Local because the first argument is a
    /// borrowed access; that does not admit Local content into caller storage.
    fn replacement_content(
        &mut self,
        state: &State,
        expression: ExprId,
        callee: ExprId,
        arguments: &[hirsel_syntax::Argument],
        requirements: &[Option<super::cfg::Requirement>],
    ) -> bool {
        let Some((destination, incoming)) =
            self.replacement_operands(callee, arguments, requirements)
        else {
            return true;
        };
        let Some(destination) = place(self.body, destination, self.source_rows) else {
            return true;
        };
        let receiving = if self.body.bindings[destination.binding].usage == UseKind::Mut {
            self.receiving_contract(destination.binding)
        } else {
            state.slots[destination.binding].declared.clone()
        };
        let actual = self.entry_value(state, incoming);
        if actual.invalid
            || !self.arenas.global(&receiving)
            || self.arenas.global(&actual.modes.relation)
        {
            return true;
        }
        let copy = actual.unique_copy_site.map(|site| self.span_of(site));
        let removal = actual.unique_copy_site.and_then(|site| {
            let ExprKind::Unary {
                op: UnaryOp::Unique,
                value,
            } = self.tree.expressions[site.0 as usize].kind
            else {
                return None;
            };
            let ExprKind::Index { base, index } = self.tree.expressions[value.0 as usize].kind
            else {
                return None;
            };
            Some(format!(
                "list.remove(mut {}, {})",
                self.spelled(base)?,
                self.spelled(index)?
            ))
        });
        if let Some(diagnostic) = self.report(
            const { RuleId::of("MODE-1") },
            self.span_of(expression),
            "replace cannot install a local value into global storage",
            |c| c.locality(state, &actual, c.receiver_requirement(destination.binding)),
        ) {
            if let Some(span) = copy {
                diagnostic.secondary.push(hirsel_diag::Label {
                    span,
                    message: "this unique copy keeps its generic source's lifetime; it may contain borrowed captures".into(),
                });
                if let Some(removal) = removal {
                    diagnostic.notes.push(format!("Move the candidate out with `{removal}`, then pass `take candidate` to `replace`. Moving transfers ownership; a generic `unique` copy cannot copy borrowed captures."));
                } else {
                    diagnostic.notes.push("Move the candidate out of its owner before `replace`; a generic `unique` copy cannot copy borrowed captures.".into());
                }
            }
        }
        false
    }
    /// What a `mut` parameter's storage may hold: its written relation, or
    /// global without one; its own access bound is not content (MODE-1).
    fn receiving_contract(&self, binding: usize) -> Relation {
        let row = self.t.get(self.body.bindings[binding].ty).unwrap();
        Relation::new(
            row.modes
                .relation
                .reaches()
                .iter()
                .filter(|r| !matches!(r, Reach::Environment))
                .cloned(),
        )
    }
    fn join_lifetime(&self, value: &Relation, part: &Relation) -> Relation {
        let (a, b) = (self.arenas.lifetime(value), self.arenas.lifetime(part));
        if a.is_global() || a == b {
            b
        } else {
            Relation::environment()
        }
    }
    fn diagnostic(&mut self, rule: RuleId, site: Span, message: &str) -> Option<&mut Diagnostic> {
        if !self.emit {
            return None;
        }
        self.diagnostics
            .push(Diagnostic::error(rule, site, message));
        self.diagnostics.last_mut()
    }
    /// A result annotation in the current syntax, or the caller's callee
    /// site when the declaration is in another item/module. Never interpret
    /// an expression ID from another definition in this body's syntax.
    fn cursor_result_site(&self, callee: ExprId) -> Span {
        let owner = if matches!(
            self.tree.expressions[callee.0 as usize].kind,
            ExprKind::Closure { .. }
        ) {
            Some(NameSite::Expression(callee))
        } else if self.callee_definition(callee) == Some(&self.syntax.item.definition) {
            Some(NameSite::Item)
        } else {
            place(self.body, callee, self.source_rows)
                .map(|place| self.body.bindings[place.binding].site)
                .filter(|site| matches!(site, NameSite::Statement(_)))
        };
        owner
            .filter(|owner| self.sites().signature(*owner).is_some())
            .map_or_else(|| self.span_of(callee), |owner| self.sites().result(owner))
    }
    /// Recover a pattern binding's input using checked source rows. In
    /// particular, a computed tuple's fields belong to the tuple temporary,
    /// even when its input expressions were borrowed names.
    fn cursor_pattern_source(&self, binding: usize) -> Option<ExprId> {
        let NameSite::Pattern(target) = self.body.bindings[binding].site else {
            return None;
        };
        for node in &self.graph.nodes {
            let Op::Bind { pattern, value, .. } = &node.op else {
                continue;
            };
            let mut pending = vec![*pattern];
            while let Some(pattern) = pending.pop() {
                if pattern == target {
                    return Some(*value);
                }
                if let Some(&index) = self.pattern_indices.get(&pattern) {
                    pending.extend(self.body.patterns[index].subpatterns());
                }
            }
        }
        None
    }
    // Dead operations still need explicit protocol entries. They have no flow
    // judgment and must be removed before emitting executable instructions.
    fn unreachable_facts(&mut self, op: &Op) {
        match op {
            Op::Finish { expression, .. }
                if matches!(
                    self.tree.expressions[expression.0 as usize].kind,
                    ExprKind::Unary {
                        op: UnaryOp::Unique,
                        ..
                    }
                ) =>
            {
                let action = if self.property(*expression).is_some_and(|p| p.plain) {
                    UniqueAction::AlreadyUnique
                } else {
                    UniqueAction::DeepCopy
                };
                self.facts.unique.push(UniqueDecision {
                    expression: *expression,
                    action,
                });
            }
            Op::Bind {
                pattern,
                value,
                guarded: true,
                test: false,
                ..
            } => {
                let borrowed = place(self.body, *value, self.source_rows).is_some()
                    && !matches!(
                        self.tree.expressions[value.0 as usize].kind,
                        ExprKind::Unary {
                            op: UnaryOp::Take | UnaryOp::Copy | UnaryOp::Unique,
                            ..
                        }
                    );
                let mut pending = vec![*pattern];
                while let Some(pattern) = pending.pop() {
                    if let Some(row) = self.body.patterns.iter_mut().find(|p| p.pattern == pattern)
                    {
                        pending.extend(row.subpatterns());
                        let plain = self.properties[row.ty].plain;
                        let commit = if plain {
                            UseKind::Copy
                        } else if borrowed {
                            UseKind::Borrow
                        } else {
                            UseKind::Take
                        };
                        row.usage = commit;
                        for binding in self
                            .body
                            .bindings
                            .iter_mut()
                            .filter(|b| b.site == NameSite::Pattern(pattern))
                        {
                            binding.usage = commit;
                        }
                        self.facts.guarded_patterns.push(GuardedPatternUse {
                            pattern,
                            test: if plain {
                                UseKind::Copy
                            } else {
                                UseKind::Borrow
                            },
                            commit,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    fn record(&mut self, expression: ExprId, value: &mut Value) {
        // Ordinary root sharing already has the root MODE-2 judgment. Carry
        // the additional descendant obligation only for a type that actually
        // promised unique descendants before it was shared.
        if value
            .shared_parts
            .0
            .iter()
            .any(|(path, _, deep)| path.is_empty() && *deep)
            && self
                .ty(expression)
                .is_some_and(|ty| !self.unique_contract_on(ty, &[], true))
        {
            Arc::make_mut(&mut value.shared_parts.0)
                .retain(|(path, _, deep)| !path.is_empty() || !*deep);
        }
        if value.modes.uniqueness == Uniqueness::Unique
            || self
                .property(expression)
                .is_some_and(|p| p.plain || p.shared)
        {
            value.uniqueness_site = None;
        } else {
            value.uniqueness_site.get_or_insert(expression);
        }
        value.restrict_crossing_paths(self.retention_depth);
        if value.modes.sendability == Sendability::NotSendable
            && (value.modes.uniqueness == Uniqueness::Unique
                || self.sealed_owner(expression) && self.arenas.global(&value.modes.relation))
        {
            // A Region cannot be copied or have its payload mutated. Its
            // complete closed root type, rather than outer deep uniqueness,
            // decides whether this affine owner can cross a task boundary.
            value.modes.sendability = self.captured_sendability(expression, value);
        }
        if self.property(expression).is_some_and(|p| p.plain) {
            // Plain reads copy a value, not an owner's blocks (MODE-1/OWN-8).
            // Freshness and container modes cannot change that scalar evidence.
            let declared = &self.t.get(self.ty(expression).unwrap()).unwrap().modes;
            value.modes.relation = Relation::global();
            value.modes.uniqueness = declared.uniqueness;
            // MODE-4: a structural plain proof establishes sendability even
            // when a generic producer's conservative root summary lacked it.
            value.modes.sendability = Sendability::Sendable;
            value.set_alias(None);
            value.content = Content::default();
            value.crossing_borrows = Arc::new(Vec::new());
        }
        if self.arenas.global(&value.modes.relation) {
            value
                .loans
                .retain(|loan| !(loan.kind == Kind::Bound && matches!(loan.on, On::Binding(_))));
        }
        if self.emit {
            // Every row that holds such a closure promises its local result.
            if value.environment_local() {
                self.environment_results.insert(expression);
            }
            self.modes
                .entry(expression)
                .and_modify(|m| {
                    let mut v = Value::new(m.clone());
                    v.join(value);
                    *m = v.modes.clone();
                })
                .or_insert_with(|| value.modes.clone());
            if !value.shared_parts.is_empty() {
                self.shared_parts
                    .entry(expression)
                    .or_default()
                    .join(&value.shared_parts);
            }
        }
    }
    fn usage(&mut self, expression: ExprId, usage: UseKind) {
        if !self.emit {
            return;
        }
        if let Some(index) = self.source_rows.expression(expression) {
            let row = &mut self.body.expressions[index];
            row.usage = usage;
        }
    }
    /// OWN-3: storage an earlier argument of a call not yet entered names
    /// cannot be written, moved or lent exclusively by a later argument. A
    /// `mut` argument's exclusive loan begins when the callee is entered, so
    /// a later argument may still read its storage (spec #66).
    fn argument_conflict(&mut self, state: &mut State, place: &Place, at: Span) -> bool {
        let Some((argument, call)) = state
            .argument_loans
            .iter()
            .find(|(_, (lent, _))| lent.overlaps(place))
            .map(|(argument, (_, call))| (*argument, *call))
        else {
            return false;
        };
        if state.argument_conflicts.insert(call) {
            self.report(
                const { RuleId::of("OWN-3") },
                self.span_of(call),
                "exclusive call arguments overlap storage",
                |c| {
                    let (name, _) = c.binding_site(place.binding);
                    Explanation::default()
                        .provided(Fact::new(
                            Cause::ArgumentLoan,
                            c.span_of(argument),
                            &[&name],
                        ))
                        .required(Fact::new(Requirement::Exclusive, at, &[&name]))
                        .into()
                },
            );
        }
        true
    }
    /// A loan lasts until the last use of each value holding it (LANG-3): only
    /// holders live after this operation keep their loans.
    fn holder_live(&self, holder: Holder) -> bool {
        self.live.at(self.node).contains(&holder)
    }
    fn loaned(&self, state: &State, place: &Place) -> bool {
        self.lender(state, place).is_some()
            || self.iterated(state, place)
            || (!self.properties[self.body.bindings[place.binding].ty].plain
                && self.capturer(state, place.binding).is_some())
    }
    /// The values live after this operation, which keep their loans (LANG-3),
    /// with the binding that holds each (none for an evaluated temporary).
    fn live_values<'s>(
        &self,
        state: &'s State,
        root: usize,
    ) -> impl Iterator<Item = (Option<usize>, &'s Value)> {
        // Backward demand already indexes the live binding holders in binding
        // order. Preserve that order and the same availability filter without
        // walking every tracked slot at each access.
        self.live
            .at(self.node)
            .iter()
            .filter_map(move |holder| match holder {
                Holder::Binding(index) if *index != root => Some(*index),
                _ => None,
            })
            .filter(move |index| state.slots[*index].available & LIVE != 0)
            .map(|index| (Some(index), &state.slots[index].value))
            .chain(state.values.values().map(|value| (None, value)))
    }
    /// The live value holding a loan on `root`: directly, or through the loan
    /// sets of the bindings it borrows or captures, which it holds as their
    /// union (MODE-5), even once those bindings are dead. A binding also holds
    /// the storage it aliases; an evaluated temporary's alias is only its
    /// operand's address. Returns the holding binding, if it is one.
    pub(super) fn lender(&self, state: &State, place: &Place) -> Option<Option<usize>> {
        let root = place.binding;
        // The bindings a value holds loans on, each within a field path.
        let direct = |value: &Value, binding: bool| -> Vec<(usize, Vec<u32>)> {
            let content = value.content.summary();
            let mut held: Vec<_> = content
                .iter()
                .filter(|loan| loan.kind == Kind::Read)
                .filter_map(|loan| match loan.on {
                    On::Binding(held) => Some((held, loan.within.clone())),
                    _ => None,
                })
                .collect();
            held.extend(
                value
                    .aliased()
                    .filter(|_| binding)
                    .map(|held| (held, vec![])),
            );
            held
        };
        // A loan on a field path conflicts only with places overlapping it.
        let conflicts = |(held, within): &(usize, Vec<u32>)| {
            *held == root
                && Place {
                    binding: root,
                    projections: within.iter().map(|f| Projection::Field(*f)).collect(),
                }
                .overlaps(place)
        };
        self.live_values(state, root)
            .find(|(holder, value)| {
                let held = direct(value, holder.is_some());
                if held.iter().any(conflicts) {
                    return true;
                }
                let mut seen = BTreeSet::new();
                let mut pending: Vec<usize> = held
                    .into_iter()
                    .map(|(binding, _)| binding)
                    .chain(value.captures())
                    .collect();
                while let Some(held) = pending.pop() {
                    if held == root || !seen.insert(held) {
                        continue;
                    }
                    let through = &state.slots[held].value;
                    let held = direct(through, true);
                    if held.iter().any(conflicts) || through.captures().any(|c| c == root) {
                        return true;
                    }
                    pending.extend(held.into_iter().map(|(binding, _)| binding));
                    pending.extend(through.captures());
                }
                false
            })
            .map(|(holder, _)| holder)
    }
    /// A borrowed loop keeps the iterated place lent while it may advance
    /// again (SEM-4): the source of the loop that does, if any.
    pub(super) fn iteration(&self, state: &State, place: &Place) -> Option<ExprId> {
        state
            .iteration_loans
            .iter()
            .find(|(source, loan)| {
                loan.overlaps(place) && self.holder_live(Holder::Iteration(**source))
            })
            .map(|(source, _)| *source)
    }
    fn iterated(&self, state: &State, place: &Place) -> bool {
        self.iteration(state, place).is_some()
    }
    /// The live scoped environment that keeps `root` borrowed by capturing it,
    /// and its binding, if it is held in one.
    pub(super) fn capturer(&self, state: &State, root: usize) -> Option<Option<usize>> {
        self.live_values(state, root)
            .find(|(_, value)| value.captures().any(|held| held == root))
            .map(|(holder, _)| holder)
    }
    fn callee_preserves_state(&self, callee: ExprId, ordinal: usize, advance: bool) -> bool {
        self.ty(callee)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| {
                let TypeKind::Function(function) = &row.kind else {
                    return false;
                };
                function.state_promises.iter().any(|p| {
                    p.selector.parameter == ParameterRef::bound(ParamId(ordinal as u32))
                        && p.selector.fields.is_empty()
                        && p.preserve_captures
                        && (!advance || p.advance_only)
                })
            })
    }
    /// A `@preserves_unique` proof of a taken parameter checks the result.
    fn proves_result(&self) -> bool {
        self.unique_proof.is_some_and(|selected| {
            self.parameters.iter().any(|&index| {
                let binding = &self.body.bindings[index];
                binding.binding == ResolvedName::Parameter(selected)
                    && self.parameter_passing(&binding.owner, selected) == PassingMode::Take
            })
        })
    }
    /// A `@preserves_unique` proof enters with its selection and every taken
    /// parameter unique; another `mut` parameter enters as declared, so the
    /// call needs nothing of that argument (MODE-2).
    fn proof_entry(&self, mut state: State) -> State {
        if let Some(selected) = self.unique_proof {
            for &index in &self.parameters {
                let binding = &self.body.bindings[index];
                let ResolvedName::Parameter(parameter) = binding.binding else {
                    continue;
                };
                let passing = self.parameter_passing(&binding.owner, parameter);
                if (parameter == selected || passing == PassingMode::Take)
                    && !self.properties[binding.ty].plain
                {
                    state.slots[index].value.modes.uniqueness = Uniqueness::Unique;
                }
            }
        }
        state
    }
    /// Only a direct call to a declaration uses `@preserves_unique`: its body or
    /// ledger entry proves it, while a function value may come from anywhere.
    fn callee_preserves_unique(&self, callee: ExprId, ordinal: usize) -> bool {
        let promised = self
            .ty(callee)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| {
                let TypeKind::Function(function) = &row.kind else {
                    return false;
                };
                function.state_promises.iter().any(|p| {
                    p.selector.parameter == ParameterRef::bound(ParamId(ordinal as u32))
                        && p.preserve_unique
                }) || ordinal == 1
                    && self.callee_definition(callee).is_some_and(|path| {
                        super::properties::format_association(
                            self.t,
                            function,
                            path,
                            self.signatures,
                        )
                        .is_some()
                    })
            });
        (promised && self.callee_definition(callee).is_some())
            || self.callee_definition(callee).is_some_and(|path| {
                hirsel_types::unique_summary::same_module(path, self.own.definition.path())
                    && self
                        .signatures
                        .cached(DefId::intern(path))
                        .is_some_and(|s| {
                            s.metadata
                                .inferred_unique
                                .preserves_unique
                                .contains(&ParamId(ordinal as u32))
                        })
            })
    }
    /// User-visible destruction (DROP-1/2): a noncopyable value, which may
    /// hold a resource, or a once callable.
    fn destroyed(&self, expression: ExprId) -> bool {
        self.property(expression).is_some_and(|p| !p.copyable)
            || self
                .ty(expression)
                .and_then(|ty| self.t.get(ty))
                .is_some_and(|row| row.modes.multiplicity == Multiplicity::Once)
    }
    fn binding_destroyed(&self, binding: usize) -> bool {
        let ty = self.body.bindings[binding].ty;
        !self.properties[ty].copyable
            || self.t.get(ty).unwrap().modes.multiplicity == Multiplicity::Once
    }
    /// MODE-2: a direct generic call whose result holds only values of type
    /// parameters it cannot copy returns a unique result when every argument
    /// that may supply them is unique at callee entry (`unique_arguments`).
    fn parametric_unique(
        &self,
        callee: ExprId,
        arguments: &[hirsel_syntax::Argument],
        requirements: &[Option<super::cfg::Requirement>],
        unique_arguments: &BTreeSet<ExprId>,
    ) -> bool {
        let Some(path) = self.callee_definition(callee) else {
            return false;
        };
        let Some(TypeKind::Function(function)) = self
            .ty(callee)
            .and_then(|ty| self.t.get(ty))
            .map(|row| &row.kind)
        else {
            return false;
        };
        let mut lookup = |d: DefId| Ok(self.signatures.cached(d));
        // A bodyless core declaration has no checked body.
        let checked = !path.package_is("core", "1");
        let Some(inputs) = parametric_inputs(self.t, function, checked, &mut lookup) else {
            return false;
        };
        arguments
            .iter()
            .zip(requirements)
            .all(|(argument, requirement)| {
                let Some(requirement) = requirement else {
                    return false;
                };
                match inputs.get(requirement.parameter) {
                    Some(ParametricInput::Free) => true,
                    Some(ParametricInput::Unique) => {
                        unique_arguments.contains(&argument.value)
                            || self.property(argument.value).is_some_and(|p| p.handle_root)
                    }
                    Some(ParametricInput::UniqueResult) => self
                        .ty(argument.value)
                        .and_then(|ty| match &self.t.get(ty)?.kind {
                            TypeKind::Function(f) => Some(f.result),
                            _ => None,
                        })
                        .is_some_and(|result| {
                            unique_rows(
                                self.t,
                                result,
                                &self.body.resolution.generic_declarations,
                                &mut |path| Ok(self.signatures.cached(path)),
                            )
                        }),
                    None => false,
                }
            })
    }
    /// A borrowed parameter a bodyless core promise reads and stores nothing of.
    fn callee_unretained(&self, callee: ExprId, ordinal: usize) -> bool {
        self.ty(callee)
            .and_then(|ty| self.t.get(ty))
            .is_some_and(|row| {
                let TypeKind::Function(function) = &row.kind else {
                    return false;
                };
                function
                    .state_promises
                    .iter()
                    .any(|p| p.preserve_unique && p.unretained.contains(&ParamId(ordinal as u32)))
            })
    }
    /// The declaration a call names directly, if any.
    /// The rule that owns a core operation's argument requirements, with the
    /// general rules it specialises there: freezing needs a unique root
    /// (MEM-11), a mutex a unique, sendable payload (CONC-5) and a channel a
    /// sendable one (CONC-6).
    fn intrinsic_owner(&self, callee: ExprId) -> Option<(RuleId, &'static [&'static str])> {
        use hirsel_types::core_roles::{Intrinsic, intrinsic_roles};
        let path = self.callee_definition(callee)?;
        let (_, role) = intrinsic_roles().into_iter().find(|(p, _)| p == path)?;
        match role {
            Intrinsic::SharedFreeze => Some((const { RuleId::of("MEM-11") }, &["MODE-2"])),
            Intrinsic::MutexCreate => Some((const { RuleId::of("CONC-5") }, &["MODE-2", "MODE-4"])),
            Intrinsic::ChannelSend => Some((const { RuleId::of("CONC-6") }, &["MODE-4"])),
            _ => None,
        }
    }
    /// MODE-2: the failure edge of `value ? mapper` takes the error payload,
    /// which is unique exactly when the Result currently is. A mapper whose
    /// parameter is `unique` needs that, unless the payload is Plain or Shared.
    fn map_error(&mut self, state: &State, expression: ExprId, value: ExprId) {
        let Some(path) = self
            .body
            .resolution
            .paths
            .iter()
            .find_map(|(site, name)| match name {
                ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                    Some(path.clone())
                }
                _ => None,
            })
        else {
            return;
        };
        let Some(signature) = self.signatures.cached(DefId::intern(&path)) else {
            return;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return;
        };
        let [parameter] = function.parameters.as_slice() else {
            return;
        };
        if self.t.get(parameter.ty).map(|row| row.modes.uniqueness) != Some(Uniqueness::Unique) {
            return;
        }
        let Some(TypeKind::Named { arguments, .. }) = self
            .ty(value)
            .and_then(|ty| self.t.get(ty))
            .map(|row| &row.kind)
        else {
            return;
        };
        let [_, hirsel_types::GenericArgument::Type(error)] = arguments.as_slice() else {
            return;
        };
        if self
            .properties
            .get(*error)
            .is_some_and(|p| p.plain || p.shared)
            || self.value(state, value).modes.uniqueness == Uniqueness::Unique
        {
            return;
        }
        let span = self.span_of(expression);
        let operand = self.span_of(value);
        self.report(
            const { RuleId::of("MODE-2") },
            span,
            "the error mapper requires a unique error payload",
            |_| {
                Explanation::default()
                    .provided(Fact::new(Cause::Aliased, operand, &[]))
                    .required(Fact::new(Requirement::Unique, span, &[]))
                    .into()
            },
        );
    }
    fn callee_definition(&self, callee: ExprId) -> Option<&hirsel_syntax::DefPath> {
        self.body
            .resolution
            .expressions
            .iter()
            .find_map(|(e, name)| match name {
                ResolvedName::Definition(path) if *e == callee => Some(path),
                _ => None,
            })
    }
    /// The bindings a value may hold storage of: those it reaches, and those
    /// they reach in turn, even once they are dead (MODE-5).
    fn reached_bindings(&self, state: &State, value: &Value) -> BTreeSet<usize> {
        let mut seen = BTreeSet::new();
        let mut pending = value.roots();
        while let Some(binding) = pending.pop() {
            if seen.insert(binding) {
                pending.extend(state.slots[binding].value.roots());
            }
        }
        seen
    }
    /// Temporaries past their last use leave the flow state. One that may
    /// hold a related call result's share stays parked on that storage until
    /// its statement releases it (MODE-2).
    fn release_temporaries(&self, state: &mut State, live: LiveSet<'_>) {
        let released: Vec<ExprId> = state
            .values
            .keys()
            .copied()
            .filter(|key| !live.contains(&Holder::Temporary(*key)))
            .collect();
        for key in released {
            let value = state.values.remove(&key).unwrap();
            if self.property(key).is_some_and(|p| p.plain) {
                continue;
            }
            for root in self.reached_bindings(state, &value) {
                let slot = &mut state.slots[root].value;
                if slot.borrow_only && !slot.parked.contains(&key) {
                    Arc::make_mut(&mut slot.parked).insert(key);
                }
            }
        }
    }
    /// MODE-2: storage aliased only by related call results' shares is unique
    /// again once no live binding, evaluated temporary or parked temporary
    /// holds one.
    fn restore_borrowed(&self, state: &mut State) {
        let borrowed: Vec<usize> = state
            .slots
            .entries()
            .into_iter()
            .filter(|(_, slot)| slot.value.borrow_only && slot.value.parked.is_empty())
            .map(|(index, _)| index)
            .collect();
        if borrowed.is_empty() {
            return;
        }
        let mut held = BTreeSet::new();
        for (index, slot) in state.slots.entries() {
            if slot.available & LIVE != 0 && !self.properties[self.body.bindings[index].ty].plain {
                held.extend(
                    self.reached_bindings(state, &slot.value)
                        .into_iter()
                        .filter(|root| *root != index),
                );
            }
        }
        for (key, value) in &state.values {
            if !self.property(*key).is_some_and(|p| p.plain) {
                held.extend(self.reached_bindings(state, value));
            }
        }
        for root in borrowed {
            if !held.contains(&root) {
                state.slots[root].value.restore_borrowed();
            }
        }
    }
    /// `held`: the bindings a returned value may hold storage of.
    fn mutable_crossing_exit(&mut self, state: &State, site: Span, held: &BTreeSet<usize>) {
        if !self.emit {
            return;
        }
        let parameters = self.parameters.clone();
        for index in parameters {
            let binding = &self.body.bindings[index];
            let ResolvedName::Parameter(parameter) = binding.binding else {
                continue;
            };
            if self.parameter_passing(&binding.owner, parameter) != PassingMode::Mut {
                continue;
            }
            // MODE-2: a `mut x: unique T` parameter, or a `@preserves_unique` proof's
            // selection, is handed back unique at every exit and failure site.
            let promised = self.unique_proof == Some(parameter);
            let nested_unique_lost = state.slots[index]
                .value
                .shared_parts
                .0
                .iter()
                .any(|(path, _, deep)| self.unique_contract_on(binding.ty, path, *deep));
            if self.graph.clause.is_none()
                && state.slots[index].available == LIVE
                && (nested_unique_lost
                    || (promised
                        || self.unique_proof.is_none()
                            && self.t.get(binding.ty).unwrap().modes.uniqueness
                                == Uniqueness::Unique)
                        && state.slots[index].value.modes.uniqueness != Uniqueness::Unique)
                && !(state.slots[index].value.borrow_only && !held.contains(&index))
                && !self.properties[binding.ty].plain
                && !self.properties[binding.ty].shared
                && self.lost_handbacks.insert(index)
            {
                self.report(
                    const { RuleId::of("MODE-2") },
                    site,
                    if promised {
                        UNIQUE_PROOF_FAILED
                    } else {
                        "mutable hand-back requires a unique value"
                    },
                    |c| {
                        let (name, declaration) = c.binding_site(index);
                        let mut pending: causes::Pending = Explanation::default()
                            .provided(Fact::new(Cause::Aliased, site, &[&name]))
                            .required(Fact::new(Requirement::Unique, declaration, &[&name]))
                            .into();
                        pending.uniqueness = Some((index, c.node));
                        pending
                    },
                );
            }
            let binding = &self.body.bindings[index];
            let nested_lost =
                self.crossing_receipts(binding.ty)
                    .iter()
                    .any(|(path, independent)| {
                        !path.is_empty()
                            && !independent
                            && state.slots[index]
                                .value
                                .crossing_parts
                                .get(path)
                                .is_some_and(|proof| proof != Sendability::Sendable)
                    });
            if state.slots[index].available == LIVE
                && (nested_lost
                    || crossing_requirement(self.t.get(binding.ty).unwrap())
                        == Sendability::Sendable
                        && state.slots[index].value.modes.sendability != Sendability::Sendable)
            {
                if self.diagnostics.iter().any(|d| d.rule.as_str() == "MODE-4") {
                    continue;
                }
                self.report(
                    const { RuleId::of("MODE-4") },
                    site,
                    "mutable hand-back requires current sendable content",
                    |c| {
                        let (name, declaration) = c.binding_site(index);
                        Explanation::default()
                            .provided(Fact::new(Cause::NotSendable, site, &[]))
                            .required(Fact::new(Requirement::SendableType, declaration, &[&name]))
                            .into()
                    },
                );
            }
        }
    }
    fn record_return_uniqueness(&mut self, state: &State, held: &BTreeSet<usize>) {
        let mut site = self.graph.nodes[self.node].span;
        site.start = site.end.saturating_sub(1);
        self.mutable_crossing_exit(state, site, held);
        if self.emit && self.graph.clause.is_none() {
            for &index in &self.parameters {
                let ResolvedName::Parameter(parameter) = self.body.bindings[index].binding else {
                    unreachable!()
                };
                let unique = state.slots[index].value.modes.uniqueness == Uniqueness::Unique;
                self.return_uniqueness
                    .entry(parameter)
                    .and_modify(|old| *old &= unique)
                    .or_insert(unique);
                let sendable = state.slots[index].value.modes.sendability == Sendability::Sendable;
                self.return_sendability
                    .entry(parameter)
                    .and_modify(|old| *old &= sendable)
                    .or_insert(sendable);
            }
        }
    }
    fn clause_parameter(&self, state: &State, binding: usize) -> Option<ExprId> {
        let root = self.graph.clause?;
        let mut current = Some(binding);
        let mut seen = BTreeSet::new();
        while let Some(index) = current {
            if !seen.insert(index) {
                break;
            }
            let binding = &self.body.bindings[index];
            if binding.owner == NameSite::Expression(root)
                && matches!(binding.binding, ResolvedName::Parameter(_))
            {
                return Some(root);
            }
            current = state.slots[index].value.aliased();
        }
        None
    }
    fn access_diagnostic(
        &mut self,
        state: &State,
        place: &Place,
        rule: RuleId,
        site: Span,
        message: &str,
        explain: impl FnOnce(&Self) -> causes::Pending,
    ) {
        if let Some(root) = self.clause_parameter(state, place.binding) {
            self.diagnostic(
                const { RuleId::of("FACT-1") },
                self.tree.expressions[root.0 as usize].origin.span,
                "contract parameters are read-only",
            );
        } else {
            self.report(rule, site, message, explain);
        }
    }
    /// Escaping environments own their capture slots even while a Many
    /// invocation borrows them. This permits another legal copy of global
    /// content, but never a take through the invocation's borrowed slot.
    fn owns_capture(&self, binding: usize) -> bool {
        !self.graph.scoped && self.capture_physical_modes.contains_key(&binding)
    }
    /// The binding grants write access to `place`, and nothing else may use
    /// its storage (published Shared or region storage is never mutated).
    fn access(&mut self, place: &Place, state: &State, operation: Span) -> bool {
        let binding = &self.body.bindings[place.binding];
        let rule = if binding.owner != self.graph.owner && self.graph.scoped {
            Some(const { RuleId::of("CLO-1") })
        } else if let ResolvedName::Parameter(p) = binding.binding {
            match self.parameter_passing(&binding.owner, p) {
                PassingMode::Borrow => Some(const { RuleId::of("OWN-1") }),
                PassingMode::Mut => None,
                PassingMode::Take => Some(const { RuleId::of("LANG-5") }),
            }
        } else if !binding.mutable && place.projections.is_empty() {
            Some(const { RuleId::of("LANG-5") })
        } else if state.slots[place.binding].value.aliased().is_some()
            && binding.usage != UseKind::Mut
        {
            Some(const { RuleId::of("LANG-15") })
        } else if !binding.mutable {
            Some(const { RuleId::of("LANG-5") })
        } else {
            None
        };
        if let Some(rule) = rule {
            let cause = match rule.as_str() {
                "LANG-15" => Some(Cause::MutAlias),
                "LANG-5" => Some(Cause::Immutable),
                "OWN-1" => Some(Cause::ReadOnlyParameter),
                _ => Some(Cause::BorrowedCapture),
            };
            self.access_diagnostic(
                state,
                place,
                rule,
                operation,
                "this binding grants read-only access",
                |c| {
                    let (name, _) = c.binding_site(place.binding);
                    c.binding_explanation(
                        place.binding,
                        cause,
                        Fact::new(Requirement::Write, operation, &[&name]),
                    )
                },
            );
            return false;
        }
        if !place.projections.is_empty() {
            let ty = self.body.bindings[place.binding].ty;
            if self.properties[ty].shared
                || matches!(
                    self.t.get(ty).unwrap().kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::Region,
                        ..
                    }
                )
            {
                self.report(
                    const { RuleId::of("OWN-7") },
                    operation,
                    "published Shared and region storage cannot be mutated",
                    |c| {
                        let (name, _) = c.binding_site(place.binding);
                        c.binding_explanation(
                            place.binding,
                            Some(Cause::Published),
                            Fact::new(Requirement::Mutate, operation, &[&name]),
                        )
                    },
                );
                return false;
            }
        }
        true
    }
    /// No value live after this operation holds a loan on `place`. A `mut`
    /// argument's exclusive loan starts at callee entry (LANG-3), so the call
    /// checks it there, against the values still live once the callee runs.
    fn unlent(&mut self, place: &Place, state: &State, operation: Span, call: bool) -> bool {
        if self.loaned(state, place) {
            // A borrowed loop's source is a loan like any other; whether
            // SEM-4 owns its conflicts instead is spec #89.
            let retained = self.lender(state, place).flatten().and_then(|holder| {
                let copy = state.slots[holder].value.unique_copy_site?;
                let ExprKind::Unary {
                    op: UnaryOp::Unique,
                    value,
                } = self.tree.expressions[copy.0 as usize].kind
                else {
                    return None;
                };
                let source = super::cfg::place(self.body, value, self.source_rows)?;
                Some((
                    self.span_of(copy),
                    self.binding_site(source.binding).1,
                    self.binding_site(place.binding).0,
                    self.binding_site(holder).0,
                ))
            });
            let diagnostic = self.report(
                if call
                    && (self.capturer(state, place.binding).is_some()
                        || self.iterated(state, place)
                        || self.lender(state, place).flatten().is_some_and(|holder| {
                            let value = &state.slots[holder].value;
                            value.borrowed_only && value.cursor_views.cursor_derived()
                        }))
                {
                    const { RuleId::of("OWN-3") }
                } else {
                    const { RuleId::of("MODE-1") }
                },
                operation,
                "a live result or alias keeps this storage borrowed",
                |c| {
                    let (name, _) = c.binding_site(place.binding);
                    c.lent(
                        state,
                        place,
                        Fact::new(Requirement::Exclusive, operation, &[&name]),
                    )
                },
            );
            if let (Some(diagnostic), Some((copy, source, name, holder))) = (diagnostic, retained) {
                diagnostic.secondary.push(hirsel_diag::Label {
                    span: copy,
                    message: "this abstract unique copy retains its source's borrowed dependencies"
                        .into(),
                });
                diagnostic.secondary.push(hirsel_diag::Label {
                    span: source,
                    message: format!("the declared source relation keeps a dependency on `{name}`"),
                });
                diagnostic.notes.push(format!("End `{holder}` and every value derived from it before mutating `{name}`, or use a concrete type whose deep copy proves it retains no borrowed captures."));
            }
            return false;
        }
        true
    }
    fn capture_value(
        &mut self,
        state: &mut State,
        expression: ExprId,
        escaping: bool,
        owning: bool,
        mut value: Value,
    ) -> Value {
        value.site = Some(expression);
        value.modes.relation = if escaping {
            Relation::global()
        } else {
            Relation::environment()
        };
        let site = Site::Expression(expression);
        value
            .loans
            .set(Kind::Bound, On::Environment, (!escaping).then_some(site));
        value.modes.uniqueness = Uniqueness::Unique;
        value.modes.sendability = Sendability::Sendable;
        let captures = self
            .source_rows
            .closure(expression)
            .map(|i| &self.body.closures[i])
            .map(|c| c.captures.clone())
            .unwrap_or_default();
        let mut environment_global = true;
        let mut environment_captured = false;
        for (ordinal, mut capture) in captures.into_iter().enumerate() {
            let metadata = self.graph.capture_modes.get(expression, ordinal as u32);
            let capture_owner =
                metadata.map_or_else(|| self.graph.owner.clone(), |c| c.owner.clone());
            if let Some(metadata) = metadata {
                capture.usage = metadata.usage;
            }

            let owner =
                (!matches!(capture.binding, ResolvedName::Local(_))).then_some(capture_owner);
            let Some(index) = self.source_rows.binding(self.body, owner, &capture.binding) else {
                continue;
            };
            let binding = &self.body.bindings[index];
            let ty = binding.ty;
            let mutable = binding.mutable;
            let part = state.slots[index].value.clone();
            // CLO-1: a closure local to its environment is captured as a value
            // local to it; this environment is then local to the same one.
            if part.environment_local() {
                environment_captured = true;
                if self.emit {
                    self.environment_captures.insert((expression, ordinal));
                }
            }
            if matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                if let Some(uses) = self.capture_crossing_uses.get(&expression) {
                    for (binding, source) in uses.iter().filter(|(binding, _)| *binding == index) {
                        let borrow = CrossingBorrow {
                            within: vec![],
                            binding: *binding,
                            source: source.clone(),
                        };
                        if !value.crossing_borrows.contains(&borrow) {
                            Arc::make_mut(&mut value.crossing_borrows).push(borrow);
                        }
                    }
                }
                for borrow in part.crossing_borrows.iter() {
                    let mut borrow = borrow.clone();
                    borrow.within.clear();
                    if !value.crossing_borrows.contains(&borrow) {
                        Arc::make_mut(&mut value.crossing_borrows).push(borrow);
                    }
                }
            }
            value.bound_within(
                &part,
                matches!(capture.usage, UseKind::Borrow | UseKind::Mut),
            );
            let plain = self.properties[ty].plain;
            if !plain
                && matches!(capture.usage, UseKind::Borrow | UseKind::Mut)
                && !self.arenas.global(&part.modes.relation)
            {
                environment_global = false;
            }
            if !escaping && !plain && matches!(capture.usage, UseKind::Borrow | UseKind::Mut) {
                // The environment borrows this incarnation of the binding,
                // even when its content is known to be global. A local result
                // of calling it cannot outlive that storage.
                if let Some(&scope) = self.graph.lifetimes.bindings.get(&index) {
                    value.bound_by(scope, Site::Expression(expression));
                }
            }
            let flat = self.properties[ty].flat;
            let specialized = metadata.is_some_and(|m| m.context == CaptureContext::Task)
                || self.properties[ty].scoped_capability;
            let closure = self.span_of(expression);
            if state.slots[index].available != LIVE {
                self.report(
                    const { RuleId::of("OWN-2") },
                    closure,
                    "capture uses a moved value",
                    |c| {
                        let (name, _) = c.binding_site(index);
                        c.moved(
                            index,
                            expression,
                            Fact::new(Requirement::Capture, closure, &[&name]),
                        )
                    },
                );
                continue;
            }
            if self.finalizer_self(index) && matches!(capture.usage, UseKind::Take | UseKind::Copy)
            {
                self.report(
                    const { RuleId::of("DROP-3") },
                    closure,
                    "a finalizer cannot move or copy its whole resource into a closure",
                    |c| {
                        c.binding_explanation(
                            index,
                            Some(Cause::FinalizerResource),
                            Fact::new(Requirement::TakeIntoClosure, closure, &[]),
                        )
                    },
                );
                continue;
            }
            if escaping
                && !specialized
                && (mutable || !plain && part.aliased().is_some() && !self.owns_capture(index))
            {
                self.report(
                    const { RuleId::of("CLO-4") },
                    closure,
                    "escaping captures need immutable owned global values",
                    |c| {
                        let required = Fact::new(Requirement::EscapingCapture, closure, &[]);
                        if mutable {
                            c.mutable_capture_explanation(
                                index,
                                &part,
                                capture.usage == UseKind::Copy,
                                required,
                            )
                        } else {
                            c.locality(state, &part, required)
                        }
                    },
                );
                continue;
            }
            let take = capture.usage == UseKind::Take && (escaping || owning);
            // OWN-1: an environment that takes a capture owns it, so the
            // captured value must be the creating frame's to give. A borrowed
            // parameter or an enclosing closure's borrowed capture is not.
            if take && !plain && part.aliased().is_some() {
                self.report(
                    const { RuleId::of("OWN-1") },
                    closure,
                    "a borrowed value cannot be taken into a closure",
                    |c| {
                        c.locality(
                            state,
                            &part,
                            Fact::new(Requirement::TakeIntoClosure, closure, &[]),
                        )
                    },
                );
                continue;
            }
            // CLO-3: a taken task capture crosses with its current content.
            // J judges the captured type; whether a share since made it
            // unsendable is I's.
            let receiver = matches!(
                self.t.get(ty).map(|row| &row.kind),
                Some(TypeKind::Builtin {
                    kind: BuiltinType::Receiver,
                    ..
                })
            );
            if take
                && metadata.is_some_and(|m| m.context == CaptureContext::Task)
                && !plain
                && !flat
                && !receiver
                && !self.properties[ty].shared
                && part.modes.sendability == Sendability::NotSendable
            {
                self.report(
                    const { RuleId::of("CLO-3") },
                    closure,
                    "task captures must copy or take sendable values; Receivers must be taken",
                    |c| {
                        c.binding_explanation(
                            index,
                            Some(Cause::NotSendable),
                            Fact::new(Requirement::TaskCapture, closure, &[]),
                        )
                    },
                );
                continue;
            }
            // MODE-2: only transferred unique blocks enter a unique
            // environment. Shared is an explicit owned-graph boundary.
            let copied_inline = !take && (escaping || plain) && flat;
            if !plain
                && !copied_inline
                && !self.properties[ty].shared
                && (!take || part.modes.uniqueness != Uniqueness::Unique)
            {
                value.modes.uniqueness = Uniqueness::Aliased;
            }
            if take {
                state.slots[index].available = MOVED;
                self.log_move(index, expression);
                value.modes.multiplicity = Multiplicity::Once;
            } else if escaping {
                // A task closure copies its other free uses too (CLO-3), so a
                // noncopyable capture must be taken there as well.
                let task = metadata.is_some_and(|m| m.context == CaptureContext::Task);
                if !self.properties[ty].scoped_capability && !self.properties[ty].copyable {
                    self.report(
                        const { RuleId::of("OWN-5") },
                        closure,
                        if task {
                            "this task capture must be taken"
                        } else {
                            "this escaping capture must be taken"
                        },
                        |c| {
                            c.binding_explanation(
                                index,
                                Some(Cause::Noncopyable),
                                Fact::new(
                                    if task {
                                        Requirement::TaskCapture
                                    } else {
                                        Requirement::EscapingCapture
                                    },
                                    closure,
                                    &[],
                                ),
                            )
                        },
                    );
                }
                if !plain && !flat {
                    state.slots[index].value.share();
                    if !self.properties[self.body.bindings[index].ty].sendable_aliased {
                        state.slots[index].value.modes.sendability = Sendability::NotSendable;
                        state.slots[index]
                            .value
                            .crossing_parts
                            .set(&[], Sendability::NotSendable);
                    }
                }
            } else {
                value
                    .loans
                    .insert(Kind::Read, On::Binding(index), Site::Expression(expression));
            }
            if escaping && !specialized && !self.arenas.global(&part.modes.relation) && !plain {
                self.report(
                    const { RuleId::of("CLO-4") },
                    closure,
                    "local values cannot be escaping captures",
                    |c| {
                        c.locality(
                            state,
                            &part,
                            Fact::new(Requirement::EscapingCapture, closure, &[]),
                        )
                    },
                );
            }
            let usage = if take {
                UseKind::Take
            } else if escaping || plain {
                UseKind::Copy
            } else {
                UseKind::Borrow
            };
            // The environment owns the captured value after extraction.
            // Copying a counted payload cannot retain its source's proof.
            let mut captured = self.t.get(capture.ty).unwrap().clone();
            captured.modes = part.modes.clone();
            let captured = self.intern(captured);
            let proof = if usage == UseKind::Copy {
                self.signatures.prove_flow(|memo| {
                    hirsel_modes::proofs::sendability_after_sharing(memo, captured)
                })
            } else {
                self.signatures
                    .prove_flow(|memo| hirsel_modes::proofs::content_sendability(memo, captured))
            }
            .unwrap_or_else(|error| {
                *self.proof_error.borrow_mut() = Some(error);
                Sendability::NotSendable
            });
            value.modes.sendability = join_sendability(value.modes.sendability.clone(), proof);
            // Provisional MODE-3/MODE-5 reading, spec #37 option B:
            // borrowing a resource does not consume it; capturing a
            // once callable still requires Once, even when borrowed.
            if part.modes.multiplicity == Multiplicity::Once
                || (usage != UseKind::Borrow && !self.properties[ty].copyable)
            {
                value.modes.multiplicity = Multiplicity::Once;
            }
            if self.emit {
                self.crossing_captures
                    .insert((expression, ordinal), part.crossing_borrows.clone());
                // J selected the capture graph; I owns the source's modes
                // at creation and the effect of copying or taking it.
                // Publish that judgment on the capture as well as its use.
                let mut ty = self.t.get(capture.ty).unwrap().clone();
                let row = ty.modes.relation.clone();
                ty.modes = part.modes.clone();
                if usage == UseKind::Copy {
                    // A copied Shared root, Mutex or Atomic is an independent
                    // global handle (MEM-11, CONC-2), also inside an aggregate
                    // of handles and plain bits.
                    if plain || self.properties[capture.ty].copy_detaches {
                        ty.modes.relation = Relation::global();
                    }
                    if flat {
                        ty.modes.uniqueness = Uniqueness::Unique;
                    } else if !plain {
                        ty.modes.uniqueness = Uniqueness::Aliased;
                    }
                    ty.sendability_requirement = SendabilityRequirement::None;
                    if !plain && !flat {
                        let root = self.intern(ty.clone());
                        ty.modes.sendability = self
                            .signatures
                            .prove_flow(|memo| {
                                hirsel_modes::proofs::sendability_after_sharing(memo, root)
                            })
                            .unwrap_or_else(|error| {
                                *self.proof_error.borrow_mut() = Some(error);
                                Sendability::NotSendable
                            });
                    }
                }
                // CLO-1: the body knows a borrowed capture as a borrowed
                // parameter of the same type, as the authority pass typed it.
                if matches!(usage, UseKind::Borrow | UseKind::Mut) && !plain {
                    ty.modes.relation = Relation::environment();
                }
                ty.modes.relation = self.arenas.relocate(&row, &ty.modes.relation);
                if part.environment_local() {
                    environment_result(self.t, self.properties, &mut ty, &self.arenas);
                }
                let ty = if self.t.get(capture.ty) == Some(&ty) {
                    capture.ty
                } else {
                    let ty = self.intern(ty);
                    self.properties.inherit(ty, capture.ty);
                    ty
                };
                if let Some(index) = self.source_rows.closure(expression) {
                    let info = &mut self.body.closures[index];
                    if let Some(row) = info.captures.get_mut(ordinal) {
                        row.usage = usage;
                        row.ty = ty;
                    }
                }
            }
        }
        // CLO-1: the authority pass promises a global result over a global
        // environment, judged from its receiving rows. When a borrowed
        // capture's current content is not global, the result is local to
        // the environment, as its frame result says.
        let inferred = environment_captured && self.environment_inferred(expression);
        if inferred || !environment_global && self.environment_specialised(expression) {
            value
                .loans
                .insert(Kind::Read, On::Environment, Site::Expression(expression));
            if self.emit {
                self.environment_results.insert(expression);
                if inferred {
                    self.environment_frames.insert(expression);
                }
            }
        }
        value
    }
    /// Whether the closure's inferred result is promised global and is not
    /// plain: over a captured closure local to its environment, the body may
    /// return that closure's local results, so its own result is local to
    /// the same environment. A written result stays the body's contract.
    fn environment_inferred(&self, expression: ExprId) -> bool {
        let written = match &self.tree.expressions[expression.0 as usize].kind {
            ExprKind::Closure { signature, .. } => signature.result.is_some(),
            _ => true,
        };
        let Some(TypeKind::Function(f)) = self
            .ty(expression)
            .and_then(|ty| self.t.get(ty))
            .map(|row| &row.kind)
        else {
            return false;
        };
        !written
            && self
                .arenas
                .global(&self.t.get(f.result).unwrap().modes.relation)
            && !self.properties[f.result].plain
    }
    /// Whether the closure's type promises a global result its frame result
    /// does not: the authority pass's environment specialisation.
    fn environment_specialised(&self, expression: ExprId) -> bool {
        let Some(frame) = self.frame_results.get(&NameSite::Expression(expression)) else {
            return false;
        };
        let Some(TypeKind::Function(f)) = self
            .ty(expression)
            .and_then(|ty| self.t.get(ty))
            .map(|row| &row.kind)
        else {
            return false;
        };
        self.arenas
            .global(&self.t.get(f.result).unwrap().modes.relation)
            && !self
                .arenas
                .global(&self.t.get(*frame).unwrap().modes.relation)
    }
    /// `receiver` is the binding that receives the value, if one does.
    fn receiving_scope(
        &mut self,
        value: &Value,
        receiving: usize,
        site: Span,
        receiver: Option<usize>,
    ) {
        if value.invalid || self.arenas.global(&value.modes.relation) {
            return;
        }
        let ended: Vec<(usize, Site)> = value
            .content
            .summary()
            .scopes()
            .filter(|(_, source, _)| !self.graph.lifetimes.contains(*source, receiving))
            .map(|(_, source, site)| (source, site))
            .collect();
        if let Some(&(first, _)) = ended.first() {
            self.report(
                const { RuleId::of("MODE-1") },
                site,
                "a local view cannot outlive its temporary source scope",
                |c| {
                    let required = match receiver {
                        Some(binding) => {
                            let (name, span) = c.binding_site(binding);
                            Fact::new(Requirement::Outlives, span, &[&name])
                        }
                        None => Fact::new(Requirement::OutlivesScope, site, &[]),
                    };
                    causes::Pending {
                        explanation: Explanation {
                            required: Some(required),
                            root: Some(hirsel_modes::explain::root_key(
                                c.own.definition.path(),
                                ("locality", (2u8, first)),
                            )),
                            ..Explanation::default()
                        },
                        temporary: ended.iter().map(|(_, site)| *site).min(),
                        moved: None,
                        installed: None,
                        uniqueness: None,
                    }
                },
            );
        }
    }
    /// LANG-11 (#50): a block whose value projects a counted part of a
    /// binding the block declares only borrows that part, and the binding's
    /// storage ends with the block. Nothing moves a part out implicitly
    /// (OWN-8), so a consumer that borrows the value would read storage that
    /// has ended; `copy` shares the part instead.
    fn block_part_outlives(
        &mut self,
        tail: ExprId,
        block: ExprId,
        consumer: ExprId,
        binding: usize,
    ) {
        let block = self.span_of(block);
        let end = Span {
            start: block.end.saturating_sub(1).max(block.start),
            ..block
        };
        let consumer = self.span_of(consumer);
        self.report(
            const { RuleId::of("MODE-1") },
            self.span_of(tail),
            "a borrowed part cannot outlive the block that declares its owner",
            |c| {
                let (name, _) = c.binding_site(binding);
                let part = c.spelled(tail).unwrap_or_else(|| name.clone());
                Explanation {
                    repairs: vec![hirsel_modes::explain::Repair::CopyPart(part.clone())],
                    fix: Some(hirsel_diag::Fix {
                        span: Span {
                            end: c.span_of(tail).start,
                            ..c.span_of(tail)
                        },
                        replacement: "copy ".into(),
                        message: "share the counted part before its owner's block ends".into(),
                    }),
                    root: Some(hirsel_modes::explain::root_key(
                        c.own.definition.path(),
                        ("block part", tail.0),
                    )),
                    ..Explanation::default()
                }
                .provided(Fact::new(Cause::BlockBinding, end, &[&name]))
                .required(Fact::new(
                    Requirement::OutlivesBlock,
                    consumer,
                    &[&part, &name],
                ))
                .into()
            },
        );
    }
    /// MODE-1: relations compose. A local binding of this body whose value is
    /// related to other places (a call result related to its arguments, a
    /// borrowed view, a field read through its owner) lends those places, and
    /// neither its own scope nor a read of itself, to a `data` result related
    /// to it: such a result holds no callable or iterator that could reach the
    /// binding's own storage. An owned binding, including any copy, holds a
    /// lasting source bound instead and lends only itself. Lowering lends the
    /// result the binding's content loans.
    fn related_view(&self, state: &State, binding: usize, result: ExprId) -> bool {
        let declared = &self.body.bindings[binding];
        let value = &state.slots[binding].value;
        declared.owner == self.graph.owner
            && !matches!(declared.binding, ResolvedName::Parameter(_))
            && !self.taken_storage(state, binding)
            && !self.arenas.global(&value.modes.relation)
            // A join retains each incoming alias as a Bound obligation.
            // Those bounds do not turn an all-borrowed view into an owner;
            // read_through still lends every one of its underlying sources.
            && (value.borrowed_only || value.sources().next().is_none())
            && value.reached().iter().any(|root| *root != binding)
            && self.property(result).is_some_and(|p| p.data)
    }
    fn retain_binding_scope(&mut self, value: &mut Value, binding: usize) {
        if let Some(&scope) = self
            .graph
            .lifetimes
            .bindings
            .get(&binding)
            .filter(|_| !self.arenas.global(&value.modes.relation))
        {
            // OWN-1/MEM-10: a local result can be placed in a region bounded
            // by this named owner. Acquiring a copy ends access, not that bound.
            value.bound_by(scope, Site::Binding(binding));
        }
    }
    fn transfer(&mut self, op: &Op, state: &mut State) {
        let creation = self.creation_effect(op, state);
        self.transfer_value(op, state);
        self.install_creation_effect(creation, state);
        if let Op::Use { expression, .. } | Op::Finish { expression, .. } = op {
            self.creation_shared(*expression, state);
        }
    }
    fn transfer_value(&mut self, op: &Op, state: &mut State) {
        self.check_state_operation(op, state);
        // Address checks fail before an indexed replacement installs fresh
        // content. Cleanup cannot restore evidence lost by an earlier share.
        if let Op::Write {
            place: expression,
            operation,
            ..
        } = op
            && place(self.body, *expression, self.source_rows).is_some_and(|place| {
                place
                    .projections
                    .iter()
                    .any(|part| matches!(part, Projection::Elements))
            })
        {
            self.mutable_crossing_exit(state, *operation, &BTreeSet::new());
        }
        // MODE-2: releasing a value with user-visible destruction runs code
        // that may fail (DROP-1/2, FAIL-3), so it is a failure site too.
        let releases = match op {
            Op::Kill(bindings) => bindings.iter().any(|binding| {
                state.slots[*binding].available & LIVE != 0 && self.binding_destroyed(*binding)
            }),
            Op::StatementEnd(statement) => self.destroying.contains(statement),
            Op::Write {
                place: expression, ..
            } => place(self.body, *expression, self.source_rows).is_some_and(|place| {
                state.slots[place.binding].available & LIVE != 0 && self.destroyed(*expression)
            }),
            _ => false,
        };
        if releases {
            let site = match op {
                Op::Write { operation, .. } => *operation,
                _ => self.graph.nodes[self.node].span,
            };
            self.mutable_crossing_exit(state, site, &BTreeSet::new());
        }
        match op {
            Op::Nop => {}
            Op::ReturnUnit => self.record_return_uniqueness(state, &BTreeSet::new()),
            Op::IdentityReady(site) => {
                if self.emit {
                    for (owner, origin, ordinal) in
                        self.introductions.get(site).into_iter().flatten()
                    {
                        if state.slots[*ordinal].available == LIVE {
                            self.facts
                                .identity_joins
                                .push((*site, *owner, origin.clone()));
                        }
                    }
                }
            }
            Op::LocalFunction(statement) => {
                let hirsel_syntax::StmtKind::Function {
                    body: expression, ..
                } = self.tree.statements[statement.0 as usize].kind
                else {
                    return;
                };
                let bindings: Vec<_> = self
                    .body
                    .bindings
                    .iter()
                    .enumerate()
                    .filter(|(_, binding)| {
                        binding.site == NameSite::Statement(*statement)
                            && matches!(binding.binding, ResolvedName::Local(_))
                    })
                    .map(|(index, binding)| (index, binding.ty))
                    .collect();
                for (index, ty) in bindings {
                    let initial = Value::new(self.t.get(ty).unwrap().modes.clone());
                    let mut value = self.capture_value(state, expression, false, false, initial);
                    value.usage = UseKind::Take;
                    state.slots[index] = Slot::new(SlotData {
                        cursor: None,
                        available: LIVE,
                        value: value.clone(),
                        declared: value.modes.relation.clone(),
                    });
                    if self.emit {
                        self.body.bindings[index].usage = UseKind::Take;
                        if value.environment_local() {
                            self.environment_bindings.insert(index);
                        }
                        self.binding_modes.insert(index, value.modes.clone());
                    }
                }
            }
            Op::Kill(bindings) => {
                for binding in bindings {
                    state.slots[*binding].available = DEAD;
                    state.slots[*binding].value.content = Content::default();
                    state.slots[*binding].value.set_alias(None);
                }
                self.restore_borrowed(state);
            }
            Op::StatementEnd(statement) => {
                let statements = &self.graph.statements;
                let ended = |key: &ExprId| {
                    statements
                        .temporaries
                        .get(key)
                        .is_some_and(|s| statements.within(*s, *statement))
                };
                let parked: Vec<usize> = state
                    .slots
                    .entries()
                    .into_iter()
                    .filter(|(_, slot)| slot.value.parked.iter().any(ended))
                    .map(|(index, _)| index)
                    .collect();
                for index in parked {
                    Arc::make_mut(&mut state.slots[index].value.parked).retain(|key| !ended(key));
                }
                self.release_confined(state, *statement);
                self.restore_borrowed(state);
            }
            Op::Use {
                expression,
                context,
                operation,
            } => {
                let context = &self.receiving_context(state, *context);
                let Some(mut property) = self.property(*expression) else {
                    return;
                };
                let Some(place) = place(self.body, *expression, self.source_rows) else {
                    return;
                };
                if matches!(context, Context::Read | Context::View | Context::Invoke)
                    && !property.plain
                    && let Some(&(block, consumer)) = self.graph.ending.get(expression)
                {
                    self.block_part_outlives(*expression, block, consumer, place.binding);
                }
                let slot = &state.slots[place.binding];
                let global_owned = slot.value.aliased().is_none()
                    && self.arenas.global(&slot.value.modes.relation)
                    && slot.value.content.summary().is_empty();
                let mut value = slot.value.clone();
                value.project_content(&place.projections);
                if self.cursor_observations.active
                    && value.cursor_views == cursors::Views::default()
                {
                    value.cursor_views = cursors::Views::root(place.binding);
                }
                value
                    .cursor_views
                    .project(&place.projections, &self.cursor_observations);
                let element_owner = value.element_owner().filter(|o| *o != place.binding);
                let environment = self
                    .ty(*expression)
                    .and_then(|t| self.t.get(t))
                    .is_some_and(|t| {
                        matches!(
                            t.kind,
                            TypeKind::Function(_)
                                | TypeKind::Builtin {
                                    kind: BuiltinType::Iter,
                                    ..
                                }
                        )
                    });
                let current_part = value
                    .crossing_parts
                    .get(&[])
                    .unwrap_or_else(|| value.modes.sendability.clone());
                if environment && current_part == Sendability::Sendable {
                    property.sendable_aliased = true;
                }
                value.part_of = self.read_part(&value, &place);
                if let Some(projection) = self.projection_lifetimes.get(expression) {
                    value.modes.relation = self.projection_locality(state, projection);
                    if self.emit {
                        self.record_physical(state, *expression);
                    }
                }
                // Spec #43 permits this result read, not arbitrary copies in
                // the generic body. All retained borrowed roots must be selected.
                if *context == Context::ResultShare && self.result_sharing.contains(expression) {
                    let roots: BTreeSet<_> =
                        value.aliased().into_iter().chain(value.sources()).collect();
                    let result = self.graph.result.relation.named_parameters();
                    let selected = |root: &usize| {
                        let binding = &self.body.bindings[*root];
                        binding.owner == self.graph.owner
                            && matches!(binding.binding,
                            ResolvedName::Parameter(parameter) if !self.arenas.arena(&Origin {
                                root: OriginRoot::Parameter(ParameterRef::body(self.body.lexical.site(binding.owner), parameter)),
                                fields: vec![].into(),
                            }) && matches!(&result,
                                Some(sources) if sources.iter().any(|source|
                                    matches!(source, ParameterRef::Bound { depth: 0, parameter: p } if *p == parameter)
                                    || matches!(source, ParameterRef::Body { scope, parameter: p } if *scope == self.body.lexical.site(self.graph.owner) && *p == parameter))))
                    };
                    if !roots.is_empty() && roots.iter().all(selected) {
                        property.copyable = true;
                    }
                }
                value.site = Some(*expression);
                if !place.projections.is_empty() {
                    // A field's invocation count is its own contract. The
                    // aggregate owner's Many mode cannot make a Once part
                    // repeatable or copyable.
                    value.modes.multiplicity = self
                        .t
                        .get(self.ty(*expression).unwrap())
                        .unwrap()
                        .modes
                        .multiplicity;
                }
                let read_site = self.tree.expressions[expression.0 as usize].origin.span;
                let operation_site = self.tree.expressions[operation.0 as usize].origin.span;
                let root_address = *context == Context::Address && place.projections.is_empty();
                if !root_address && slot.available != LIVE {
                    self.report(
                        if *context == Context::Invoke
                            && value.modes.multiplicity == Multiplicity::Once
                        {
                            const { RuleId::of("MODE-3") }
                        } else {
                            const { RuleId::of("OWN-2") }
                        },
                        read_site,
                        "the value is moved or not initialised on every path",
                        |c| {
                            let (name, _) = c.binding_site(place.binding);
                            c.moved(
                                place.binding,
                                *expression,
                                Fact::new(Requirement::Read, operation_site, &[&name]),
                            )
                        },
                    );
                    value.invalid = true;
                    state.values.insert(*expression, value);
                    return;
                }
                let usage = match context {
                    Context::Address | Context::Mut => UseKind::Mut,
                    Context::Copy => UseKind::Copy,
                    Context::Take => UseKind::Take,
                    // An owning environment can lend a global field to this
                    // invocation without making a returned copy local.
                    Context::Own
                        if !property.plain
                            && property.copyable
                            && value.aliased().is_some()
                            && self.owns_capture(place.binding) =>
                    {
                        UseKind::Copy
                    }
                    Context::Store | Context::ResultShare
                        if !property.plain
                            && (value.aliased().is_some()
                                || !place.projections.is_empty()
                                || value.element_place.is_some()) =>
                    {
                        if property.copyable {
                            UseKind::Copy
                        } else {
                            UseKind::Take
                        }
                    }
                    Context::Own if value.element_place.is_some() && !property.plain => {
                        if property.copyable {
                            UseKind::Copy
                        } else {
                            UseKind::Take
                        }
                    }
                    Context::Own | Context::Store | Context::ResultShare
                        if !property.plain
                            && value.aliased().is_none()
                            && place.projections.is_empty() =>
                    {
                        UseKind::Take
                    }
                    Context::Unique
                        if !property.plain
                            && value.aliased().is_none()
                            && place.projections.is_empty()
                            && value.element_place.is_none()
                            && value.modes.uniqueness == Uniqueness::Unique
                            && !self.unique_copies.contains(expression) =>
                    {
                        UseKind::Take
                    }
                    Context::Unique if !property.plain => UseKind::Borrow,
                    Context::Invoke if value.modes.multiplicity == Multiplicity::Once => {
                        UseKind::Take
                    }
                    // A borrow-only use of a part of any type is a view: a read
                    // loan on the part's place, no count, and the owner stays
                    // unique (MODE-2/OWN-8, #76 Option 2).
                    Context::View if !property.plain => UseKind::Borrow,
                    // A cursor projection is still a view of its fixed owner.
                    // Staging a match scrutinee must not acquire a part copy and
                    // make its payload appear to originate in a fresh local.
                    Context::Read
                        if !property.plain
                            && value.borrowed_only
                            && value.cursor_views.cursor_derived() =>
                    {
                        UseKind::Borrow
                    }
                    _ if property.plain || !place.projections.is_empty() && property.copyable => {
                        UseKind::Copy
                    }
                    _ => UseKind::Borrow,
                };
                if matches!(usage, UseKind::Mut | UseKind::Take)
                    && self.argument_conflict(state, &place, operation_site)
                {
                    value.invalid = true;
                    state.values.insert(*expression, value);
                    return;
                }
                if (matches!(usage, UseKind::Take | UseKind::Copy) || *context == Context::Unique)
                    && place.projections.is_empty()
                    && self.finalizer_self(place.binding)
                {
                    self.report(
                        const { RuleId::of("DROP-3") },
                        operation_site,
                        "a finalizer cannot move or copy its whole resource",
                        |c| {
                            c.binding_explanation(
                                place.binding,
                                Some(Cause::FinalizerResource),
                                Fact::new(Requirement::Owned, operation_site, &[]),
                            )
                        },
                    );
                    value.invalid = true;
                    state.values.insert(*expression, value);
                    return;
                }
                // The call itself checks a `mut` argument's loans at callee
                // entry; later arguments may still read the place (LANG-3).
                if usage == UseKind::Mut
                    && *context != Context::Address
                    && (!self.access(&place, state, operation_site)
                        || !matches!(
                            self.tree.expressions[operation.0 as usize].kind,
                            ExprKind::Call { .. }
                        ) && !self.unlent(&place, state, operation_site, true))
                {
                    value.invalid = true;
                    state.values.insert(*expression, value);
                    return;
                }
                if usage == UseKind::Borrow
                    && value.aliased().is_none()
                    && *context != Context::Unique
                {
                    value.set_alias(Some((place.binding, Site::Expression(*expression))));
                }
                if usage == UseKind::Take {
                    if matches!(context, Context::Store | Context::ResultShare)
                        && value.aliased().is_some()
                    {
                        self.access_diagnostic(
                            state,
                            &place,
                            const { RuleId::of("OWN-1") },
                            operation_site,
                            "an owned result cannot move out of borrowed storage",
                            |c| {
                                c.borrowed_match_explanation(
                                    state,
                                    &place,
                                    &value,
                                    Fact::new(Requirement::OwnedResult, operation_site, &[]),
                                )
                            },
                        );
                        value.invalid = true;
                    } else if !place.projections.is_empty() || value.element_place.is_some() {
                        self.report(
                            const { RuleId::of("OWN-8") },
                            operation_site,
                            "destructure the whole owned value to move a part",
                            |c| c.part_explanation(state, &place, &value, operation_site),
                        );
                        value.invalid = true;
                    } else if value.aliased().is_some() {
                        self.access_diagnostic(
                            state,
                            &place,
                            const { RuleId::of("OWN-1") },
                            operation_site,
                            "a borrowed value cannot be taken",
                            |c| {
                                c.borrowed_match_explanation(
                                    state,
                                    &place,
                                    &value,
                                    Fact::new(Requirement::Owned, operation_site, &[]),
                                )
                            },
                        );
                        value.invalid = true;
                    } else if self.loaned(state, &place) {
                        self.report(
                            const { RuleId::of("MODE-1") },
                            operation_site,
                            "a live local result keeps this value borrowed",
                            |c| {
                                c.lent(
                                    state,
                                    &place,
                                    Fact::new(Requirement::Owned, operation_site, &[]),
                                )
                            },
                        );
                        value.invalid = true;
                    } else {
                        state.slots[place.binding].available = MOVED;
                        self.log_move(place.binding, *expression);
                    }
                }
                if usage == UseKind::Copy
                    && (!property.copyable || value.modes.multiplicity == Multiplicity::Once)
                {
                    let generic = matches!(
                        self.t.get(self.ty(*expression).unwrap()).unwrap().kind,
                        TypeKind::Parameter(_)
                    );
                    if let Some(d) = self.report(
                        if generic {
                            const { RuleId::of("GEN-1") }
                        } else {
                            const { RuleId::of("OWN-5") }
                        },
                        operation_site,
                        "this value is noncopyable",
                        |_| {
                            Explanation::default()
                                .provided(Fact::new(Cause::Noncopyable, read_site, &[]))
                                .required(Fact::new(Requirement::Copy, operation_site, &[]))
                                .into()
                        },
                    ) {
                        d.obligations.push(Obligation::CopyProof);
                    }
                    value.invalid = true;
                }
                if matches!(usage, UseKind::Copy | UseKind::Take) && global_owned {
                    // Content with no retained bounds remains independent of
                    // its holder, and a part is as global as that content.
                    if value.content.parts.is_none() {
                        value.install_content(Vec::new());
                    }
                    value.modes.relation = Relation::global();
                }
                if usage == UseKind::Copy && !self.arenas.global(&value.modes.relation) {
                    // Counted copies of a local part still depend on its owner.
                    // Copying the handle does not allocate an independent graph.
                    value.alias_bound();
                    if !place.projections.is_empty()
                        && (value.aliased().is_some() || value.content.parts.is_none())
                    {
                        // An owned part with known installed content retains
                        // that content's sources, independently of its holder.
                        value.loans.insert(
                            Kind::Bound,
                            On::Binding(place.binding),
                            Site::Expression(*expression),
                        );
                    }
                }
                if *context == Context::Copy
                    || usage == UseKind::Copy
                        && (matches!(context, Context::Store | Context::ResultShare)
                            || value.element_place.is_some())
                {
                    value.set_alias(None);
                }
                if usage == UseKind::Copy {
                    value.acquire_copy();
                    value.element_place = None;
                    value.element_view = false;
                }
                if usage == UseKind::Copy && property.copy_detaches {
                    value.detached_handle_copy();
                }
                if usage == UseKind::Copy && property.flat {
                    // MEM-12 copies complete flat bytes, creating an independent
                    // owned value without sharing the source's storage.
                    value.modes.uniqueness = Uniqueness::Unique;
                }
                if usage == UseKind::Copy
                    && !property.plain
                    && !property.flat
                    && !property.shared
                    && *context != Context::Unique
                {
                    state.slots[place.binding].value.share_root();
                    state.slots[place.binding].value.copy_site = Some(*operation);
                    state.slots[place.binding]
                        .value
                        .shared_parts
                        .insert_deep(place.projections.clone(), *expression);
                    let copied_independently = property.sendable_aliased
                        || value.modes.sendability == Sendability::Sendable
                            && self
                                .ty(*expression)
                                .and_then(|t| self.t.get(t))
                                .is_some_and(|t| {
                                    matches!(
                                        t.kind,
                                        TypeKind::Function(_)
                                            | TypeKind::Builtin {
                                                kind: BuiltinType::Iter,
                                                ..
                                            }
                                    )
                                });
                    value.share();
                    if !copied_independently {
                        // The copied element is what ends its owners' uniqueness.
                        state.share_element_owners(element_owner, Some(*expression));
                        state.slots[place.binding].value.modes.sendability =
                            Sendability::NotSendable;
                        state.slots[place.binding]
                            .value
                            .crossing_parts
                            .set(&place.projections, Sendability::NotSendable);
                        value.modes.sendability = Sendability::NotSendable;
                        value.crossing_parts.set(&[], Sendability::NotSendable);
                    }
                }
                if !place.projections.is_empty() {
                    // OWN-8 carries locality from the owner, but sendability
                    // belongs to the checked part (a flat owner's counted
                    // field is not itself a flat value).
                    value.modes.sendability = self
                        .t
                        .get(self.ty(*expression).unwrap())
                        .unwrap()
                        .modes
                        .sendability
                        .clone();
                    if environment && current_part == Sendability::Sendable {
                        value.modes.sendability = Sendability::Sendable;
                    } else if !property.sendable_aliased {
                        if let Some(proof) = value.crossing_parts.get(&[]) {
                            value.modes.sendability = proof;
                        }
                        if usage == UseKind::Copy {
                            value.modes.sendability = Sendability::NotSendable;
                        }
                    }
                    // A projection through a borrowed view still reaches its
                    // underlying storage. Naming the intermediate pattern slot
                    // here would retain a dead arm-local source after descent.
                    let alias = if usage == UseKind::Borrow && value.borrowed_only {
                        value
                            .alias()
                            .or(Some((place.binding, Site::Expression(*expression))))
                    } else {
                        (usage != UseKind::Copy)
                            .then_some((place.binding, Site::Expression(*expression)))
                    };
                    value.set_alias(alias);
                    if *context == Context::Mut
                        && state.slots[place.binding].value.modes.uniqueness == Uniqueness::Unique
                    {
                        // MODE-2: a unique owner exclusively owns each part, so
                        // exclusive access to a part is unique access.
                        value.modes.uniqueness = Uniqueness::Unique;
                    } else if !property.plain && !(usage == UseKind::Copy && property.flat) {
                        value.modes.uniqueness = Uniqueness::Aliased;
                    }
                }
                if *context == Context::Unique {
                    if usage == UseKind::Take {
                        self.unique_moves.insert(*expression);
                    }
                    if (self.unique_copies.contains(expression)
                        || value.modes.uniqueness != Uniqueness::Unique
                        || value.aliased().is_some()
                        || !place.projections.is_empty()
                        || value.element_place.is_some())
                        && !property.plain
                    {
                        if !property.copyable {
                            self.report(
                                const { RuleId::of("OWN-5") },
                                operation_site,
                                "deep copying requires a copyable value",
                                |_| {
                                    Explanation::default()
                                        .provided(Fact::new(Cause::Noncopyable, read_site, &[]))
                                        .required(Fact::new(
                                            Requirement::DeepCopy,
                                            operation_site,
                                            &[],
                                        ))
                                        .into()
                                },
                            );
                        }
                        value.deep_copy = true;
                    }
                    value.modes.uniqueness = Uniqueness::Unique;
                    value.extract_unique(self.may_capture(*expression));
                    self.deep_copy_sendability(*expression, &mut value);
                    // A value already unique is not copied (MEM-5), so it keeps
                    // its own relation: `unique x` of a local x stays local.
                }
                if property.plain {
                    value.modes.relation = Relation::global();
                    value.set_alias(None);
                    value.content = Content::default();
                }
                value.usage = usage;
                value.borrowed_only = usage == UseKind::Borrow;
                self.usage(*expression, usage);
                self.record(*expression, &mut value);
                state.values.insert(*expression, value);
            }
            Op::MapError { expression, value } => self.map_error(state, *expression, *value),
            Op::Forward { expression, child } => {
                let mut value = self.value(state, *child);
                if matches!(
                    self.tree.expressions[expression.0 as usize].kind,
                    ExprKind::Try { .. }
                ) {
                    // H checked a registered Result; inspect its declaration's
                    // success tag rather than inheriting the whole root's modes.
                    let success = self
                        .ty(*child)
                        .and_then(|ty| match &self.t.get(ty)?.kind {
                            TypeKind::Named { definition, .. } => {
                                self.signatures.cached(*definition)
                            }
                            _ => None,
                        })
                        .and_then(|signature| {
                            let SignatureKind::Type(declaration) = &signature.kind else {
                                return None;
                            };
                            let TypeKind::Variant(variants) =
                                &self.t.get(declaration.representation?)?.kind
                            else {
                                return None;
                            };
                            variants.iter().position(|variant| {
                                variant.name.as_str() == "Ok" && variant.fields.len() == 1
                            })
                        });
                    if let Some(variant) = success {
                        value.project_content(&[Projection::VariantField {
                            variant: variant as u32,
                            field: 0,
                        }]);
                    } else {
                        *self.proof_error.borrow_mut() =
                            Some(QueryError::Operational(OperationalFailure {
                                operation: "ownership Result projection".into(),
                                message: "registered Result lacks its declared success payload"
                                    .into(),
                            }));
                    }
                    if value.usage == UseKind::Take
                        && value.fresh_receipt
                        && let Some(ty) = self.ty(*expression)
                        && self.t.get(ty).unwrap().modes.uniqueness == Uniqueness::Unique
                    {
                        value.modes.uniqueness = Uniqueness::Unique;
                    }
                    value.fresh_receipt &= value.modes.uniqueness == Uniqueness::Unique;
                }
                self.receiving_scope(
                    &value,
                    self.graph.lifetimes.expressions[expression],
                    self.tree.expressions[value.site.unwrap_or(*expression).0 as usize]
                        .origin
                        .span,
                    None,
                );
                // A join of values that live in different places has no
                // placement of its own, at any depth: it is as local as every
                // source, and a scoped placement is the bound (PROV-4). The
                // authority pass published the joined row.
                if let (Some(joined), Some(source)) = (self.ty(*expression), self.ty(*child)) {
                    if self.arenas.unplaced_scoped(self.t, joined, source) {
                        value.modes.relation = Relation::environment();
                        value.loans.insert(
                            Kind::Bound,
                            On::Placement,
                            Site::Expression(*expression),
                        );
                        // A value that borrows nothing has no source a result
                        // could name: only its lost placement bounded it, so
                        // it ends with the function like any temporary.
                        if value.reached().is_empty() {
                            value.bound_by(super::cfg::LifetimeScopes::FRAME, Site::Frame);
                        }
                    }
                }
                self.record(*expression, &mut value);
                // Publication visits CFG nodes by their arena index, which need
                // not put a child before its wrapper. Forward the analyzed use
                // rather than reading a previously published expression row.
                // Each branch or arm forwards separately: publish their join,
                // as for the modes.
                let usage = if self.emit {
                    *self
                        .forwarded
                        .entry(*expression)
                        .and_modify(|u| *u = join_usage(*u, value.usage))
                        .or_insert(value.usage)
                } else {
                    value.usage
                };
                self.usage(*expression, usage);
                state.values.insert(*expression, value);
            }
            Op::Finish {
                expression,
                context,
                children,
                fresh,
            } => {
                let context = &self.receiving_context(state, *context);
                let Some(property) = self.property(*expression) else {
                    return;
                };
                let mut value = self.value(state, *expression);
                if let Some(projection) = self.projection_lifetimes.get(expression) {
                    value.modes.relation = self.projection_locality(state, projection);
                    if self.emit {
                        self.record_physical(state, *expression);
                    }
                }
                if *fresh {
                    value.modes.uniqueness = Uniqueness::Unique;
                    value.modes.relation = Relation::global();
                }
                if let ExprKind::Unary {
                    op: UnaryOp::Take | UnaryOp::Copy | UnaryOp::Mut | UnaryOp::Unique,
                    value: child,
                } = self.tree.expressions[expression.0 as usize].kind
                {
                    value = self.value(state, child);
                    if matches!(
                        self.tree.expressions[expression.0 as usize].kind,
                        ExprKind::Unary {
                            op: UnaryOp::Mut | UnaryOp::Take,
                            ..
                        }
                    ) && place(self.body, child, self.source_rows).is_none()
                        && !value.invalid
                        && matches!(value.usage, UseKind::Borrow | UseKind::Mut)
                    {
                        // The operand is a computed value that only borrows
                        // other storage (a block, branch or arm naming it, or a
                        // part of one; spec issue #50). It is not a source
                        // place: there is nothing here to lend exclusively or
                        // to move out of, and no storage whose identity and
                        // uniqueness the exclusive use would end.
                        let operand = self.span_of(child);
                        let site = self.span_of(*expression);
                        self.report(
                            const { RuleId::of("OWN-3") },
                            site,
                            "this operand is a borrowed value, not a place or an owned temporary",
                            |_| {
                                Explanation::default()
                                    .provided(Fact::new(Cause::OperandBorrows, operand, &[]))
                                    .required(Fact::new(Requirement::Owned, site, &[]))
                                    .into()
                            },
                        );
                        value.invalid = true;
                    }
                    let usage = match self.tree.expressions[expression.0 as usize].kind {
                        ExprKind::Unary {
                            op: UnaryOp::Take, ..
                        } => UseKind::Take,
                        ExprKind::Unary {
                            op: UnaryOp::Copy, ..
                        } => UseKind::Copy,
                        ExprKind::Unary {
                            op: UnaryOp::Mut, ..
                        } => UseKind::Mut,
                        _ => UseKind::Take,
                    };
                    if matches!(
                        self.tree.expressions[expression.0 as usize].kind,
                        ExprKind::Unary {
                            op: UnaryOp::Unique,
                            ..
                        }
                    ) {
                        if (value.modes.uniqueness != Uniqueness::Unique
                            || value.aliased().is_some())
                            && !property.plain
                        {
                            value.deep_copy = true;
                        }
                        if self.emit {
                            // A plain place copies its empty owned graph; unlike
                            // an owned temporary, extracting it must not move the
                            // caller's scalar slot. This needs no allocation.
                            let copy_plain_place = property.plain
                                && place(self.body, child, self.source_rows).is_some();
                            let action = if value.deep_copy || copy_plain_place {
                                UniqueAction::DeepCopy
                            } else {
                                UniqueAction::AlreadyUnique
                            };
                            self.facts.unique.push(UniqueDecision {
                                expression: *expression,
                                action,
                            });
                        }
                        if value.deep_copy {
                            if !property.copyable {
                                let site = self.span_of(*expression);
                                let operand = self.span_of(child);
                                self.report(
                                    const { RuleId::of("OWN-5") },
                                    site,
                                    "deep copying requires a copyable value",
                                    |_| {
                                        Explanation::default()
                                            .provided(Fact::new(Cause::Noncopyable, operand, &[]))
                                            .required(Fact::new(Requirement::DeepCopy, site, &[]))
                                            .into()
                                    },
                                );
                            }
                            value.unique_copy_site = Some(*expression);
                            let captures = self.may_capture(*expression);
                            value.extract_unique(captures);
                            if captures {
                                // An abstract/function copy retains the named
                                // source dependencies as well as its frame bound.
                                // Unlike a concrete deep copy, it cannot prove
                                // that no borrowed capture still reads them.
                                for reach in value.modes.relation.reaches().to_vec() {
                                    let Reach::Place(origin) = reach else {
                                        continue;
                                    };
                                    if self.arenas.arena(&origin) {
                                        continue;
                                    }
                                    let binding = self.body.bindings.iter().position(|binding| {
                                        match (&origin.root, &binding.binding) {
                                            (
                                                OriginRoot::Parameter(ParameterRef::Body {
                                                    scope,
                                                    parameter,
                                                }),
                                                ResolvedName::Parameter(actual),
                                            ) => {
                                                *scope == self.body.lexical.site(binding.owner)
                                                    && parameter == actual
                                            }
                                            (
                                                OriginRoot::Local(owner, local),
                                                ResolvedName::Local(actual),
                                            ) => *owner == self.body.lexical && local == actual,
                                            _ => false,
                                        }
                                    });
                                    if let Some(binding) = binding {
                                        let fields: Vec<_> =
                                            origin.fields.iter().map(|field| field.0).collect();
                                        value.read_within(
                                            binding,
                                            &fields,
                                            Site::Expression(*expression),
                                        );
                                    }
                                }
                            }
                            self.deep_copy_sendability(*expression, &mut value);
                        }
                        value.modes.uniqueness = Uniqueness::Unique;
                    }
                    // OWN-5 holds for a temporary as for a place: the place case
                    // was judged where it was read.
                    if usage == UseKind::Copy
                        && !property.copyable
                        && place(self.body, child, self.source_rows).is_none()
                    {
                        let generic = self
                            .ty(*expression)
                            .and_then(|ty| self.t.get(ty))
                            .is_some_and(|row| matches!(row.kind, TypeKind::Parameter(_)));
                        let site = self.span_of(*expression);
                        let operand = self.span_of(child);
                        if let Some(d) = self.report(
                            if generic {
                                const { RuleId::of("GEN-1") }
                            } else {
                                const { RuleId::of("OWN-5") }
                            },
                            site,
                            "this value is noncopyable",
                            |_| {
                                Explanation::default()
                                    .provided(Fact::new(Cause::Noncopyable, operand, &[]))
                                    .required(Fact::new(Requirement::Copy, site, &[]))
                                    .into()
                            },
                        ) {
                            d.obligations.push(Obligation::CopyProof);
                        }
                        value.invalid = true;
                    }
                    // MODE-2: a counted copy shares storage, also with a fresh
                    // temporary. A flat copy is independent bytes (MEM-12).
                    if usage == UseKind::Copy
                        && !property.plain
                        && !property.shared
                        && !property.flat
                    {
                        value.share();
                        if !property.sendable_aliased {
                            value.modes.sendability = Sendability::NotSendable;
                            value.crossing_parts.set(&[], Sendability::NotSendable);
                        }
                    }
                    value.usage = usage;
                    self.usage(*expression, usage);
                } else {
                    let parts = self
                        .ty(*expression)
                        .and_then(|ty| self.t.get(ty))
                        .map(|row| {
                            match (
                                &self.tree.expressions[expression.0 as usize].kind,
                                &row.kind,
                            ) {
                                (ExprKind::Tuple(_), TypeKind::Tuple(parts)) => parts.to_vec(),
                                (
                                    ExprKind::Unary {
                                        op: UnaryOp::Box, ..
                                    },
                                    TypeKind::Box(part),
                                ) => vec![*part],
                                (ExprKind::Sequence(_), TypeKind::Builtin { arguments, .. }) => {
                                    match arguments.first() {
                                        Some(hirsel_types::GenericArgument::Type(element)) => {
                                            vec![*element; children.len()]
                                        }
                                        _ => vec![],
                                    }
                                }
                                _ => vec![],
                            }
                        })
                        .unwrap_or_default();
                    for (child, part) in children.iter().zip(parts) {
                        self.required_part(state, part, *child, *expression);
                    }
                    for child in children {
                        let mut part = self.value(state, *child);
                        if self
                            .property(*child)
                            .is_some_and(|p| p.handle_root || p.plain)
                        {
                            part.modes.uniqueness = Uniqueness::Unique;
                        }
                        value.join(&part);
                    }
                    match &self.tree.expressions[expression.0 as usize].kind {
                        ExprKind::Tuple(_) | ExprKind::Sequence(_) => {
                            let sequence = matches!(
                                self.tree.expressions[expression.0 as usize].kind,
                                ExprKind::Sequence(_)
                            );
                            let mut retained = Vec::new();
                            value.crossing_parts = CrossingParts::default();
                            value.shared_parts = SharedParts::default();
                            value.crossing_borrows = Arc::new(Vec::new());
                            for (index, child) in children.iter().enumerate() {
                                let projection = if sequence {
                                    Projection::Elements
                                } else {
                                    Projection::Field(index as u32)
                                };
                                let part = self.value(state, *child);
                                self.construction_part(&mut value, &projection, *child, &part);

                                value
                                    .crossing_parts
                                    .set(&[projection.clone()], part.modes.sendability.clone());
                                value
                                    .crossing_parts
                                    .install(&[projection.clone()], &part.crossing_parts);
                                value.install_crossing_borrows(&[projection.clone()], &part);
                                append_parts(
                                    &mut retained,
                                    &[projection],
                                    &part,
                                    self.retention_depth,
                                );
                            }
                            value.install_content(retained);
                            if self.parts_sendable(state, *expression, &value, children) {
                                value.modes.sendability = Sendability::Sendable;
                            }
                        }
                        ExprKind::Field { .. } | ExprKind::Index { .. } => {
                            if let Some(base) = children.first() {
                                let part = self.value(state, *base);
                                value.content = part.content.clone();
                            }
                            if let Some(checked) = self
                                .body
                                .places
                                .iter()
                                .find(|p| p.expression == *expression)
                            {
                                if let Some(projection) = checked.projections.last() {
                                    let projection = match projection.kind {
                                        SourceProjectionKind::Field(field) => {
                                            Projection::Field(field.0)
                                        }
                                        SourceProjectionKind::VariantField { variant, field } => {
                                            Projection::VariantField {
                                                variant: variant.0,
                                                field: field.0,
                                            }
                                        }
                                        SourceProjectionKind::Index(_) => Projection::Elements,
                                    };
                                    value.project_content(&[projection.clone()]);
                                    value.cursor_views = children
                                        .first()
                                        .map(|base| self.value(state, *base).cursor_views.clone())
                                        .unwrap_or_default();
                                    value
                                        .cursor_views
                                        .project(&[projection], &self.cursor_observations);
                                }
                            }
                        }
                        ExprKind::Unary {
                            op: UnaryOp::Box, ..
                        } => {
                            if let Some(child) = children.first() {
                                let part = self.value(state, *child);

                                value.crossing_parts = CrossingParts::default();
                                value.shared_parts = SharedParts::default();
                                self.construction_part(
                                    &mut value,
                                    &Projection::Field(0),
                                    *child,
                                    &part,
                                );
                                value.crossing_borrows = Arc::new(Vec::new());
                                value.install_crossing_borrows(&[Projection::Field(0)], &part);
                                value
                                    .crossing_parts
                                    .set(&[Projection::Field(0)], part.modes.sendability.clone());
                                value
                                    .crossing_parts
                                    .install(&[Projection::Field(0)], &part.crossing_parts);
                                if self.parts_sendable(state, *expression, &value, &[*child]) {
                                    value.modes.sendability = Sendability::Sendable;
                                }
                            }
                        }
                        _ => {}
                    }
                    if let Some(row) = self.ty(*expression).and_then(|ty| self.t.get(ty)) {
                        if !matches!(row.kind, TypeKind::Function(_)) {
                            // MODE-3 describes invocation of the callable itself.
                            // Aggregates retain Once in their field types and are
                            // structurally noncopyable, without becoming callables.
                            value.modes.multiplicity = row.modes.multiplicity;
                        }
                    }
                    if property.plain {
                        value.modes.relation = Relation::global();
                        value.set_alias(None);
                        value.content = Content::default();
                    }
                    // A part of a computed value is taken out of that
                    // temporary. A part of a value that was only borrowed (a
                    // block or branch that names outer storage, spec issue #50)
                    // is read through the same borrow.
                    let projection = matches!(
                        self.tree.expressions[expression.0 as usize].kind,
                        ExprKind::Field { .. } | ExprKind::Index { .. }
                    );
                    let borrowed_projection = projection
                        && children
                            .first()
                            .is_some_and(|base| self.value(state, *base).usage == UseKind::Borrow);
                    let copy_projection = borrowed_projection
                        && property.copyable
                        && matches!(
                            context,
                            Context::Own | Context::Store | Context::ResultShare | Context::Copy
                        );
                    value.usage = if property.plain || copy_projection {
                        UseKind::Copy
                    } else if borrowed_projection {
                        UseKind::Borrow
                    } else {
                        UseKind::Take
                    };
                    value.borrowed_only = borrowed_projection
                        && !copy_projection
                        && !property.plain
                        && children
                            .first()
                            .is_some_and(|base| self.value(state, *base).borrowed_only);
                    if copy_projection && !property.plain {
                        // A computed borrowed root has the same field-read
                        // receipt as a binding root: own a copy of the part.
                        // Local counted copies still retain their source lifetime.
                        if !self.arenas.global(&value.modes.relation) {
                            value.alias_bound();
                        }
                        if property.copy_detaches {
                            value.detached_handle_copy();
                        }
                        if property.flat {
                            // MEM-12 copies the bytes, so the copy is unique,
                            // but OWN-1 keeps it in its borrowed owner's
                            // locality exactly as a binding root's part copy.
                            value.modes.uniqueness = Uniqueness::Unique;
                        } else if !property.shared {
                            if let Some(root) = value.aliased() {
                                state.slots[root].value.share();
                                if !self.properties[self.body.bindings[root].ty].sendable_aliased {
                                    state.slots[root].value.modes.sendability =
                                        Sendability::NotSendable;
                                    state.slots[root]
                                        .value
                                        .crossing_parts
                                        .set(&[], Sendability::NotSendable);
                                }
                            }
                            value.share();
                            if !property.sendable_aliased {
                                value.modes.sendability = Sendability::NotSendable;
                                value.crossing_parts.set(&[], Sendability::NotSendable);
                            }
                        }
                        value.set_alias(None);
                    }
                    self.usage(*expression, value.usage);
                }
                value.site = Some(*expression);
                self.receiving_scope(
                    &value,
                    self.graph.lifetimes.expressions[expression],
                    self.tree.expressions[expression.0 as usize].origin.span,
                    None,
                );
                self.record(*expression, &mut value);
                if let Some(constant) = self.constants.get_mut(expression) {
                    *constant = Some(value);
                } else {
                    state.values.insert(*expression, value);
                }
            }
            Op::ArgumentLoan { argument, call } => {
                let mut source = *argument;
                let mut exclusive = false;
                if let ExprKind::Unary { op, value } = self.tree.expressions[source.0 as usize].kind
                {
                    match op {
                        UnaryOp::Mut => {
                            exclusive = true;
                            source = value;
                        }
                        _ => return,
                    }
                }
                let Some(lent) = place(self.body, source, self.source_rows) else {
                    return;
                };
                let value = self.value(state, *argument);
                let shared = value.usage == UseKind::Borrow
                    && self.property(*argument).is_some_and(|p| !p.plain);
                if !value.invalid && (exclusive || shared) {
                    state.argument_loans.insert(*argument, (lent, *call));
                }
            }
            Op::IterationLoan { source, begin } => {
                if *begin {
                    if let Some(place) = place(self.body, *source, self.source_rows) {
                        state.iteration_loans.insert(*source, place);
                    }
                } else {
                    state.iteration_loans.remove(source);
                }
            }
            Op::Bind {
                pattern,
                value,
                guarded,
                test,
                choice,
            } => {
                let source = *value;
                let mut value = self.value(state, source);
                let iteration = self.tree.statements.iter().find_map(|s| match s.kind {
                    hirsel_syntax::StmtKind::For {
                        pattern: p,
                        source: e,
                        ..
                    } if p == *pattern && e == source => Some(e),
                    _ => None,
                });
                let iteration_mut = iteration.is_some_and(|e| {
                    matches!(
                        self.tree.expressions[e.0 as usize].kind,
                        ExprKind::Unary {
                            op: UnaryOp::Mut,
                            ..
                        }
                    )
                });
                // A mutable loop's element place is a part of its source binding.
                let mut_source = iteration.filter(|_| iteration_mut).and_then(|e| {
                    match self.tree.expressions[e.0 as usize].kind {
                        ExprKind::Unary {
                            op: UnaryOp::Mut,
                            value,
                        } => place(self.body, value, self.source_rows).map(|p| p.binding),
                        _ => None,
                    }
                });
                let iterator = iteration.is_some()
                    && self.ty(source).is_some_and(|ty| {
                        matches!(
                            self.t.get(ty).unwrap().kind,
                            TypeKind::Builtin {
                                kind: BuiltinType::Iter,
                                ..
                            }
                        )
                    });
                let associated = iteration.is_some() && self.ty(source)
                    .and_then(|ty| self.t.get(ty))
                    .and_then(|row| match &row.kind {
                        TypeKind::Named { definition, .. } => self.signatures.cached(*definition),
                        _ => None,
                    })
                    .is_some_and(|s| matches!(&s.kind, SignatureKind::Type(t) if t.loop_source.is_some()));
                let borrowed_iteration = iteration.is_some_and(|e| {
                    !matches!(
                        self.tree.expressions[e.0 as usize].kind,
                        ExprKind::Unary {
                            op: UnaryOp::Take | UnaryOp::Mut,
                            ..
                        }
                    )
                }) && !iterator
                    && !associated;
                let consuming = iteration.is_some()
                    && !iteration_mut
                    && !borrowed_iteration
                    && !iterator
                    && !associated;
                let declared_source = self
                    .ty(source)
                    .map(|ty| self.t.get(ty).unwrap().modes.relation.clone());
                let annotation = self
                    .pattern_indices
                    .get(pattern)
                    .map(|&row| &self.body.patterns[row])
                    .and_then(|row| row.annotation.map(|written| (written, row.ty)));
                // A written cursor cover is a prerequisite of the binding's
                // receiving modes. Check a simple declaration before those
                // modes can mistake a malformed cover for global storage.
                let mut written_cursor = annotation
                    .filter(|(written, _)| written.relation)
                    .and_then(|_| {
                        self.body
                            .bindings
                            .iter()
                            .position(|binding| binding.site == NameSite::Pattern(*pattern))
                    })
                    .map(|index| {
                        self.cursor_contract(state, index, &mut value, annotation, *pattern, source)
                    });
                if !test {
                    if let Some((written, declared)) = annotation {
                        let annotated = declared;
                        let row = self.t.get(declared).unwrap();
                        let crossing = matches!(
                            row.kind,
                            TypeKind::Builtin {
                                kind: BuiltinType::Iter,
                                ..
                            }
                        );
                        let declared = row.modes.clone();
                        let modes = annotation_modes(written, &declared, &value.modes, crossing);
                        let pattern_node = &self.tree.patterns[pattern.0 as usize];
                        let name = match pattern_node.kind {
                            hirsel_syntax::PatternKind::Binding(symbol) => {
                                symbol.as_str().to_owned()
                            }
                            _ => "_".into(),
                        };
                        let context = Fact::new(
                            Requirement::AnnotatedBinding,
                            pattern_node.origin.span,
                            &[&name],
                        );
                        let site = self.tree.expressions[source.0 as usize].origin.span;
                        if self.required(state, &value, &modes, source, site, context) {
                            self.required_shared_parts(&value, annotated, source, site);
                        } else {
                            // The failed annotation is the primary site. Values
                            // derived from its binding carry recovery evidence.
                            value.invalid = true;
                        }
                    }
                }
                let root_pattern = *pattern;
                // Match stages a checked value; borrowed lets and indexed loops
                // retain the physical source storage.
                let indexed_iteration =
                    (borrowed_iteration || iteration_mut) && !iterator && !associated;
                let physical_root = if indexed_iteration
                    || (iteration.is_none()
                        && value.aliased().is_some()
                        && self.let_patterns.contains(&root_pattern))
                {
                    if let Some(plan) = self.projection_lifetimes.get(&source) {
                        self.physical_projection_modes(state, plan)
                    } else if let Some(place) = place(self.body, source, self.source_rows) {
                        self.physical_root_modes(
                            state,
                            super::properties::ProjectionRoot::Binding(place.binding),
                        )
                    } else {
                        value.modes.clone()
                    }
                } else {
                    value.modes.clone()
                };
                // LANG-13: an or-pattern binds through the alternative this
                // node takes; a root alternative stands for the whole pattern.
                let chosen = |pattern: hirsel_syntax::PatternId| match &self.tree.patterns
                    [pattern.0 as usize]
                    .kind
                {
                    hirsel_syntax::PatternKind::Or(alternatives) => choice
                        .get(&pattern)
                        .copied()
                        .or(alternatives.first().copied()),
                    _ => None,
                };
                let mut root_pattern = root_pattern;
                while let Some(alternative) = chosen(root_pattern) {
                    root_pattern = alternative;
                }
                let mut pending =
                    vec![(root_pattern, Vec::new(), physical_root, value.fresh_receipt)];
                while let Some((pattern, mut path, parent_modes, fresh_receipt)) = pending.pop() {
                    if let Some(alternative) = chosen(pattern) {
                        pending.push((alternative, path, parent_modes, fresh_receipt));
                        continue;
                    }
                    if let Some(&index) = self.pattern_indices.get(&pattern) {
                        let row = &self.body.patterns[index];
                        if !matches!(
                            row.kind,
                            PatternDecisionKind::Binding(_) | PatternDecisionKind::Wildcard
                        ) {
                            let mut ty = row.ty;
                            while let TypeKind::Box(part) = self.t.get(ty).unwrap().kind {
                                path.push(Projection::Field(0));
                                ty = part;
                            }
                        }
                    }
                    let mut physical_modes = self
                        .pattern_indices
                        .get(&pattern)
                        .map(|index| {
                            self.t
                                .get(self.body.patterns[*index].ty)
                                .unwrap()
                                .modes
                                .clone()
                        })
                        .unwrap_or_else(Modes::global);
                    if iteration.is_none() || indexed_iteration {
                        if pattern == root_pattern {
                            if indexed_iteration {
                                // SEM-4 projects a place in the source storage.
                                // Its receiving relation may have been refined
                                // since J typed the item; keep the item's own
                                // promises and refresh only inherited locality.
                                if declared_source.as_ref().is_some_and(|declared| {
                                    self.arenas.lifetime(&physical_modes.relation)
                                        == self.arenas.lifetime(declared)
                                }) {
                                    physical_modes.relation = self
                                        .arenas
                                        .relocate(&physical_modes.relation, &parent_modes.relation);
                                }
                            } else {
                                physical_modes = parent_modes;
                            }
                        } else if let Some(&declared) = self.pattern_declarations.get(&pattern) {
                            physical_modes.relation = if self.properties[declared].plain {
                                Relation::global()
                            } else {
                                let locality = &self.t.get(declared).unwrap().modes.relation;
                                if self.arenas.global(locality) {
                                    parent_modes.relation
                                } else {
                                    locality.clone()
                                }
                            };
                        }
                        if self.emit {
                            self.pattern_modes.insert(pattern, physical_modes.clone());
                        }
                    }
                    let site = self
                        .body
                        .bindings
                        .iter()
                        .position(|b| b.site == NameSite::Pattern(pattern));
                    // Every alternative's binder names the first's slot.
                    let binding = site.map(|i| {
                        let row = &self.body.bindings[i];
                        self.source_rows
                            .binding(self.body, Some(row.owner), &row.binding)
                            .unwrap_or(i)
                    });
                    let ty = binding.map(|i| self.body.bindings[i].ty).or_else(|| {
                        self.body
                            .patterns
                            .iter()
                            .find(|p| p.pattern == pattern)
                            .map(|p| p.ty)
                    });
                    let plain = ty.is_some_and(|ty| self.properties[ty].plain);
                    let mut part = value.clone();
                    part.fresh_receipt = fresh_receipt;
                    part.project_content(&path);
                    part.cursor_views.project(&path, &self.cursor_observations);
                    // SEM-4 binds sequence elements as places, rather than
                    // readonly aliases of a mut parameter.
                    let checked_use = self
                        .body
                        .patterns
                        .iter()
                        .find(|p| p.pattern == pattern)
                        .map(|p| p.usage);
                    part.element_place =
                        (iteration_mut && !iterator && checked_use == Some(UseKind::Mut))
                            .then_some(ElementPlace { owner: mut_source });
                    if iteration.is_some()
                        && !iterator
                        && !plain
                        && !self.arenas.global(&value.modes.relation)
                        && matches!(
                            self.tree.expressions[source.0 as usize].kind,
                            ExprKind::Unary {
                                op: UnaryOp::Take,
                                ..
                            }
                        )
                    {
                        let scope = self.graph.lifetimes.expressions[&source];
                        part.content.whole.insert(
                            Kind::Read,
                            On::Scope(scope),
                            Site::Expression(source),
                        );
                    }
                    if iteration.is_some() {
                        // The item keeps its declared locality and multiplicity,
                        // but a shared owner cannot prove nested uniqueness.
                        if let Some(ty) = ty {
                            let current = part.modes.uniqueness;
                            part.modes = self.t.get(ty).unwrap().modes.clone();
                            // MODE-2: an element annotation is a receiving
                            // contract, not evidence that a shared owner's
                            // current element is still exclusively owned.
                            if current != Uniqueness::Unique && !self.properties[ty].shared {
                                part.modes.uniqueness = Uniqueness::Aliased;
                            } else if consuming && !self.properties[ty].shared {
                                // MODE-2 is deep: consuming a currently unique
                                // collection transfers its elements' exclusive
                                // ownership, even with unqualified element rows.
                                // Borrowed loops and indirect iterator results
                                // cannot inherit this proof.
                                part.modes.uniqueness = current;
                            }
                        }
                        // An owning cursor and its elements are as local as
                        // the collection taken. The checked rows follow the
                        // source's declared row; the collection's locality is
                        // this pass's judgment of the value consumed.
                        if consuming && !plain {
                            if let Some(declared) = declared_source.as_ref() {
                                if self.arenas.lifetime(&part.modes.relation)
                                    == self.arenas.lifetime(declared)
                                {
                                    part.modes.relation = value.modes.relation.clone();
                                }
                                // The physical row of each part follows.
                                if let Some(&row) =
                                    self.pattern_indices.get(&pattern).filter(|_| self.emit)
                                {
                                    let mut modes = self
                                        .t
                                        .get(self.body.patterns[row].ty)
                                        .unwrap()
                                        .modes
                                        .clone();
                                    if self.arenas.lifetime(&modes.relation)
                                        == self.arenas.lifetime(declared)
                                    {
                                        modes.relation = value.modes.relation.clone();
                                        self.pattern_modes.insert(pattern, modes);
                                    }
                                }
                            }
                        }
                    }
                    if pattern != root_pattern {
                        if let Some(ty) = ty {
                            // A part is typed as its field is, like a field
                            // read. Physical declaration modes remain on the
                            // pattern; receiving evidence follows the owned graph.
                            // J's receiving binding may inherit uniqueness from
                            // the initially instantiated owner. Use the actual
                            // declaration's field promise after call-result
                            // weakening; only the current owner can strengthen it.
                            let declared = self
                                .pattern_declarations
                                .get(&pattern)
                                .copied()
                                .unwrap_or(ty);
                            let field = &self.t.get(declared).unwrap().modes;
                            part.modes.multiplicity = field.multiplicity;
                            // MODE-2 is deep: a taken part of the current
                            // unique whole receives that stronger evidence.
                            // Shared roots stop the owned-graph inference.
                            if self.properties[ty].shared {
                                part.modes.uniqueness = field.uniqueness;
                            } else if part.aliased().is_some() {
                                part.modes.uniqueness = Uniqueness::Aliased;
                            } else if field.uniqueness == Uniqueness::Unique && part.fresh_receipt {
                                part.modes.uniqueness = Uniqueness::Unique;
                            }
                            if self.properties[ty].sendable_aliased {
                                part.modes.sendability = Sendability::Sendable;
                            } else if let Some(proof) = part.crossing_parts.get(&[]) {
                                part.modes.sendability = proof;
                            } else if field.sendability != Sendability::Sendable {
                                part.modes.sendability = Sendability::NotSendable;
                            }
                        }
                    }
                    if borrowed_iteration && !plain {
                        // SEM-4's source marker selects the element access mode,
                        // independently of any copy made while evaluating a field
                        // source. The checked item carries its own locality.
                        let alias = part
                            .alias()
                            .or_else(|| {
                                place(self.body, source, self.source_rows)
                                    .map(|p| (p.binding, Site::Expression(source)))
                            })
                            .or(binding.map(|b| (b, Site::Binding(b))));
                        part.set_alias(alias);
                        part.element_view = true;
                        if let Some(ty) = ty {
                            let current = part.modes.uniqueness;
                            part.modes = self.t.get(ty).unwrap().modes.clone();
                            if current != Uniqueness::Unique && !self.properties[ty].shared {
                                part.modes.uniqueness = Uniqueness::Aliased;
                            }
                        }
                    }
                    if indexed_iteration && !plain {
                        // SEM-4 reads the same source place in flow and in the
                        // physical pattern. J's item row predates the source's
                        // refined receiving lifetime; do not restore that stale
                        // locality after refreshing the physical view above.
                        part.modes.relation = physical_modes.relation.clone();
                    }
                    if iterator {
                        part.set_alias(None);
                        part.content = Content::default();
                        if let Some(ty) = ty {
                            part.modes = self.t.get(ty).unwrap().modes.clone();
                        }
                    }
                    if associated {
                        // The borrowed receiver is not the yielded payload's
                        // storage. H/J composed next -> cursor -> open -> source;
                        // retain the source content, never the element's slot.
                        part.set_alias(None);
                        if let Some(ty) = ty {
                            part.modes = self.t.get(ty).unwrap().modes.clone();
                            if self.arenas.global(&part.modes.relation) {
                                part.content = Content::default();
                            }
                        }
                    }
                    if *test && !plain {
                        let alias = binding.map(|b| (b, Site::Binding(b))).or(part.alias());
                        part.set_alias(alias);
                    }
                    if plain {
                        // Plain payloads carry their own checked value modes;
                        // the aggregate owner's modes describe different storage.
                        if let Some(ty) = ty {
                            part.modes = self.t.get(ty).unwrap().modes.clone();
                        }
                        part.modes.relation = Relation::global();
                        part.set_alias(None);
                        part.content = Content::default();
                    }
                    // An unqualified part can have aliases outside this receipt
                    // (a counted copy of it shares its declared unique
                    // descendants), so only independently unique payloads carry
                    // the unshared-root proof into their new owned root.
                    if !path.is_empty() && part.modes.uniqueness != Uniqueness::Unique {
                        part.fresh_receipt = false;
                    }
                    if let Some(row) = self.body.patterns.iter().find(|p| p.pattern == pattern) {
                        for (field, child) in &row.fields {
                            let projection = match &row.kind {
                                PatternDecisionKind::Constructor(ContextualName::Variant {
                                    variant,
                                    ..
                                }) => Projection::VariantField {
                                    variant: variant.0,
                                    field: field.0,
                                },
                                _ => Projection::Field(field.0),
                            };
                            let mut child_path = path.clone();
                            child_path.push(projection);
                            pending.push((
                                *child,
                                child_path,
                                physical_modes.clone(),
                                part.fresh_receipt,
                            ));
                        }
                    }
                    if self.emit && *guarded {
                        let commit = if plain {
                            UseKind::Copy
                        } else if value.aliased().is_some() {
                            UseKind::Borrow
                        } else {
                            UseKind::Take
                        };
                        self.facts.guarded_patterns.push(GuardedPatternUse {
                            pattern,
                            test: if plain {
                                UseKind::Copy
                            } else {
                                UseKind::Borrow
                            },
                            commit,
                        });
                    }
                    let usage = if iteration_mut && !iterator {
                        checked_use.unwrap_or(UseKind::Mut)
                    } else if plain {
                        UseKind::Copy
                    } else if borrowed_iteration {
                        UseKind::Borrow
                    } else if part.aliased().is_some() {
                        UseKind::Borrow
                    } else {
                        UseKind::Take
                    };
                    // MODE-2/7: uniqueness at this point is value evidence,
                    // not a permanent promise of the binding's storage. Keep
                    // that evidence in flow, but default receiving storage to
                    // aliased unless the binding explicitly requires unique.
                    let mut storage_modes = part.modes.clone();
                    if iteration.is_none() && binding.is_some_and(|i| self.body.bindings[i].mutable)
                    {
                        storage_modes.uniqueness = Uniqueness::Aliased;
                        if !ty.is_some_and(|ty| {
                            matches!(
                                self.t.get(ty).unwrap().kind,
                                TypeKind::Function(_)
                                    | TypeKind::Builtin {
                                        kind: BuiltinType::Iter,
                                        ..
                                    }
                            )
                        }) {
                            storage_modes.sendability = Sendability::NotSendable;
                        }
                        if let Some((written, _)) = annotation.filter(|_| pattern == root_pattern) {
                            let crossing =
                                binding.is_some_and(|i| self.written_crossing_contract(i));
                            storage_modes = annotation_modes(
                                written,
                                &ty.and_then(|ty| self.t.get(ty))
                                    .map(|ty| ty.modes.clone())
                                    .unwrap_or_else(|| part.modes.clone()),
                                &storage_modes,
                                crossing,
                            );
                            // MODE-3: once storage holds a once function,
                            // whatever it currently received; MODE-4: it
                            // crosses tasks as its written type does.
                            if written.once {
                                part.modes.multiplicity = Multiplicity::Once;
                            }
                            if written.function || crossing {
                                part.modes.sendability = storage_modes.sendability.clone();
                            }
                        }
                    }
                    if let Some(index) = binding {
                        self.receiving_scope(
                            &part,
                            self.graph
                                .lifetimes
                                .bindings
                                .get(&index)
                                .copied()
                                .unwrap_or(0),
                            self.tree.expressions[source.0 as usize].origin.span,
                            Some(index),
                        );
                        let cursor = written_cursor.take().unwrap_or_else(|| {
                            self.cursor_contract(
                                state,
                                index,
                                &mut part,
                                annotation,
                                root_pattern,
                                source,
                            )
                        });
                        if usage != UseKind::Borrow {
                            part.cursor_views = cursors::Views::default();
                        }
                        state.slots[index] = Slot::new(SlotData {
                            cursor,
                            available: LIVE,
                            value: part.clone(),
                            // A borrowed pattern alias keeps its physical source
                            // view, not its receipt modes.
                            declared: if part.aliased().is_some() {
                                physical_modes.relation.clone()
                            } else {
                                storage_modes.relation.clone()
                            },
                        });
                        if self.emit && !test {
                            self.body.bindings[site.unwrap_or(index)].usage = usage;
                            self.binding_modes.insert(index, storage_modes.clone());
                            if !part.shared_parts.is_empty() {
                                self.binding_parts
                                    .entry(index)
                                    .or_default()
                                    .join(&part.shared_parts);
                            }
                            if part.environment_local() {
                                self.environment_bindings.insert(index);
                                self.environment_patterns.insert(pattern);
                            }
                        }
                    }
                    if self.emit && !test {
                        if let Some(row) =
                            self.body.patterns.iter_mut().find(|p| p.pattern == pattern)
                        {
                            // This row describes the physical scrutinee part.
                            // Receipt modes belong only to the bound name; a
                            // projection cannot change its declared field type.
                            row.usage = usage;
                        }
                    }
                }
            }
            Op::Write {
                place: expression,
                value: rhs,
                operation,
                compound,
            } => {
                let Some(place) = place(self.body, *expression, self.source_rows) else {
                    return;
                };
                if self.property(*expression).is_none() {
                    return;
                }
                if self.argument_conflict(state, &place, *operation) {
                    return;
                }
                if !place.projections.is_empty() || *compound {
                    if state.slots[place.binding].available != LIVE {
                        self.report(
                            const { RuleId::of("OWN-2") },
                            *operation,
                            "assignment reads a moved value",
                            |c| {
                                let (name, _) = c.binding_site(place.binding);
                                c.moved(
                                    place.binding,
                                    *expression,
                                    Fact::new(Requirement::Write, *operation, &[&name]),
                                )
                            },
                        );
                        return;
                    }
                }
                // A failed borrowed binding has no valid cursor contract. Its
                // later whole-name refills cannot establish cover judgments;
                // RHS availability/access/copying were checked by their uses.
                let expected = &state.slots[place.binding].value;
                if expected.invalid
                    && expected.borrowed_only
                    && expected.aliased().is_some()
                    && self.body.bindings[place.binding].mutable
                    && place.projections.is_empty()
                    && !*compound
                {
                    return;
                }
                if self.rebind_cursor(state, &place, *expression, *rhs, *operation, *compound) {
                    return;
                }
                if !self.access(&place, state, *operation)
                    || !self.unlent(&place, state, *operation, false)
                {
                    return;
                }
                let value = self.value(state, *rhs);
                let expected = state.slots[place.binding].value.clone();
                if value.invalid {
                    return;
                }
                self.receiving_scope(
                    &value,
                    self.graph
                        .lifetimes
                        .bindings
                        .get(&place.binding)
                        .copied()
                        .unwrap_or(0),
                    *operation,
                    Some(place.binding),
                );
                if !place.projections.is_empty() {
                    self.log_write(place.binding, *operation, &value);
                }
                if value.scoped() {
                    self.report(
                        const { RuleId::of("CLO-1") },
                        *operation,
                        "a scoped closure cannot be stored",
                        |c| {
                            Explanation::default()
                                .provided(Fact::new(
                                    Cause::ScopedClosure,
                                    value.site.map_or(*operation, |e| c.span_of(e)),
                                    &[],
                                ))
                                .required(Fact::new(Requirement::StoreClosure, *operation, &[]))
                                .into()
                        },
                    );
                } else if (self.arenas.global(&state.slots[place.binding].declared)
                    || self.body.bindings[place.binding].usage == UseKind::Mut)
                    && !self.arenas.global(&value.modes.relation)
                {
                    self.report(
                        const { RuleId::of("MODE-1") },
                        *operation,
                        "a local value cannot be stored into global storage",
                        |c| c.locality(state, &value, c.receiver_requirement(place.binding)),
                    );
                }
                let unique_storage = self
                    .binding_modes
                    .get(&place.binding)
                    .unwrap_or(
                        &self
                            .t
                            .get(self.body.bindings[place.binding].ty)
                            .unwrap()
                            .modes,
                    )
                    .uniqueness
                    == Uniqueness::Unique;
                if place.projections.is_empty()
                    && unique_storage
                    && value.modes.uniqueness != Uniqueness::Unique
                    && !self.property(*rhs).is_some_and(|p| p.plain)
                {
                    self.report(
                        const { RuleId::of("MODE-2") },
                        *operation,
                        "whole-value assignment must preserve declared uniqueness",
                        |c| {
                            let (name, span) = c.binding_site(place.binding);
                            Explanation::default()
                                .provided(Fact::new(Cause::Aliased, c.span_of(*rhs), &[]))
                                .required(Fact::new(Requirement::Unique, span, &[&name]))
                                .into()
                        },
                    );
                }
                if let Some(storage) = if place.projections.is_empty() {
                    Some(self.body.bindings[place.binding].ty)
                } else {
                    expression_type(self.body, *expression, self.source_rows)
                } {
                    let mut requirement = self.t.get(storage).unwrap().modes.clone();
                    requirement.sendability = crossing_requirement(self.t.get(storage).unwrap());
                    requirement.uniqueness = Uniqueness::Aliased;
                    requirement.multiplicity = Multiplicity::Once;
                    if requirement.sendability == Sendability::Sendable {
                        self.required(
                            state,
                            &value,
                            &requirement,
                            *rhs,
                            *operation,
                            Fact::new(Requirement::Sendable, self.span_of(*expression), &[]),
                        );
                    }
                }
                // MODE-3: storage of a many function type is called again
                // after the write, so it cannot receive a once function. A
                // part's type is its declared one.
                let storage = if place.projections.is_empty() {
                    Some(self.body.bindings[place.binding].ty)
                } else {
                    expression_type(self.body, *expression, self.source_rows)
                };
                if (!place.projections.is_empty() || !self.once_storage(place.binding))
                    && storage
                        .and_then(|ty| self.t.get(ty))
                        .is_some_and(|row| row.modes.multiplicity == Multiplicity::Many)
                    && value.modes.multiplicity == Multiplicity::Once
                    && self.property(*rhs).is_some_and(|p| p.function)
                {
                    if let Some(d) = self.report(
                        const { RuleId::of("MODE-3") },
                        *operation,
                        "a once function cannot satisfy a many function type",
                        |c| {
                            let (_, span) = c.binding_site(place.binding);
                            Explanation::default()
                                .provided(Fact::new(Cause::Once, c.span_of(*rhs), &[]))
                                .required(Fact::new(Requirement::Many, span, &[]))
                                .into()
                        },
                    ) {
                        d.obligations.push(Obligation::Multiplicity);
                    }
                }
                if self.emit {
                    let ty = self.t.get(self.body.bindings[place.binding].ty).unwrap();
                    // Arena and region receivers detach even when uniqueness
                    // alone would update them in place.
                    let arena_or_region = self.arenas.placed(&ty.modes.relation);
                    let allocating = !(expected.modes.uniqueness == Uniqueness::Unique
                        && place.projections.is_empty()
                        || self.properties[self.body.bindings[place.binding].ty].plain
                        || !arena_or_region
                            && (expected.modes.uniqueness == Uniqueness::Unique
                                || self
                                    .projection_lifetimes
                                    .get(expression)
                                    .is_some_and(|plan| !plan.counted_path)));
                    self.mutation_sites.push(*expression);
                    if allocating {
                        self.facts.allocating_mutations.push(*expression);
                    }
                    self.replacement_live.insert(
                        *expression,
                        match state.slots[place.binding].available {
                            LIVE => Some(true),
                            bits if bits & LIVE == 0 => Some(false),
                            _ => None,
                        },
                    );
                }
                if !place.projections.is_empty() {
                    state.slots[place.binding].value.unique_parts.retain(|p| {
                        !p.starts_with(&place.projections) && !place.projections.starts_with(p)
                    });
                    if value.modes.uniqueness == Uniqueness::Unique {
                        state.slots[place.binding]
                            .value
                            .unique_parts
                            .insert(place.projections.clone());
                    }
                    let mut shared = SharedParts::default();
                    if value.modes.uniqueness != Uniqueness::Unique
                        && !self.property(*rhs).is_some_and(|p| p.plain || p.shared)
                    {
                        shared.insert(vec![], *rhs);
                    }
                    shared.install(&[], &value.shared_parts);
                    state.slots[place.binding]
                        .value
                        .shared_parts
                        .replace(&place.projections, &shared);
                    state.slots[place.binding]
                        .value
                        .crossing_parts
                        .set(&place.projections, value.modes.sendability.clone());
                    state.slots[place.binding]
                        .value
                        .crossing_parts
                        .install(&place.projections, &value.crossing_parts);
                    Arc::make_mut(&mut state.slots[place.binding].value.crossing_borrows)
                        .retain(|borrow| !borrow.within.starts_with(&place.projections));
                    state.slots[place.binding]
                        .value
                        .install_crossing_borrows(&place.projections, &value);
                    state.slots[place.binding]
                        .value
                        .restrict_crossing_paths(self.retention_depth);
                    state.slots[place.binding].value.replace_content(
                        &place.projections,
                        &value,
                        self.retention_depth,
                        &self.arenas,
                    );
                }
                if place.projections.is_empty() {
                    let mut value = value;
                    if let Some(element) = expected.element_place {
                        value.element_place = Some(element);
                        if value.modes.sendability != Sendability::Sendable {
                            state.share_element_owners(
                                element.owner.filter(|owner| *owner != place.binding),
                                None,
                            );
                        }
                    }
                    // A Local binding's declared receiving lifetime remains
                    // bounded by its initial temporary source after replacement.
                    for (kind, scope, site) in expected.content.summary().scopes() {
                        value.content.whole.insert(kind, On::Scope(scope), site);
                    }
                    if self.body.bindings[place.binding].usage == UseKind::Mut {
                        // Replacement owns the stored value, but subsequent reads
                        // still have caller-bounded access (MODE-1/OWN-3).
                        value.set_alias(expected.alias());
                        value.modes.relation = expected.modes.relation.clone();
                    }
                    if self.once_storage(place.binding) {
                        value.modes.multiplicity = Multiplicity::Once;
                    }
                    state.slots[place.binding] = Slot::new(SlotData {
                        cursor: None,
                        available: LIVE,
                        declared: state.slots[place.binding].declared.clone(),
                        value,
                    });
                } else if !self.property(*rhs).is_some_and(|p| p.plain || p.shared)
                    && value.modes.uniqueness != Uniqueness::Unique
                {
                    // The root now reaches the stored counted aliases. Plain
                    // bits and Shared boundaries do not share its owned graph.
                    state.slots[place.binding].value.share();
                    if !self.properties[self.body.bindings[place.binding].ty].sendable_aliased {
                        state.slots[place.binding].value.modes.sendability =
                            Sendability::NotSendable;
                        state.slots[place.binding]
                            .value
                            .crossing_parts
                            .set(&place.projections, Sendability::NotSendable);
                    }
                }
            }
            Op::Call {
                expression,
                callee,
                arguments,
                requirements,
                result,
                constructor,
                parts,
                captures,
            } => {
                // MODE-2: a callee owns what it takes and may keep it beyond
                // this body, so a related result's share it receives is real.
                if !*constructor {
                    for (argument, requirement) in arguments.iter().zip(requirements) {
                        if requirement
                            .as_ref()
                            .is_some_and(|r| r.passing == PassingMode::Take)
                            && !self.property(argument.value).is_some_and(|p| p.plain)
                        {
                            let value = self.value(state, argument.value);
                            for root in self.reached_bindings(state, &value) {
                                if state.slots[root].value.borrow_only {
                                    state.slots[root].value.share();
                                }
                            }
                        }
                    }
                }
                let mut loans: Vec<(Place, bool, ExprId)> = vec![];
                let call_site = self.span_of(*expression);
                let callee_name = self.spelled(*callee).unwrap_or_else(|| "the callee".into());
                let callable = self.value(state, *callee);
                // Copying an abstract selected callable result duplicates its
                // opaque environment, not the contents its view borrows. The
                // declared abstract parameter cannot invoke that callable.
                let copied_callable_result = self.ty(*expression).is_some_and(|ty| matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_)))
                    && self.ty(*callee).is_some_and(|ty| {
                        let TypeKind::Function(function) = &self.t.get(ty).unwrap().kind else { return false };
                        let TypeKind::Parameter(result) = self.t.get(function.result).unwrap().kind else { return false };
                        function.parameters.iter().any(|parameter| parameter.passing == PassingMode::Borrow && matches!(self.t.get(parameter.ty).unwrap().kind, TypeKind::Parameter(input) if input == result))
                    });
                let argument_inputs_live = *constructor
                    || arguments
                        .iter()
                        .zip(requirements)
                        .all(|(argument, requirement)| {
                            let Some(requirement) = requirement else {
                                return true;
                            };
                            let value = self.value(state, argument.value);
                            if value.crossing_borrows.is_empty() {
                                return true;
                            }
                            let may_invoke = requirement.passing != PassingMode::Borrow
                                || self
                                    .ty(*callee)
                                    .and_then(|ty| {
                                        let TypeKind::Function(function) = &self.t.get(ty)?.kind
                                        else {
                                            return None;
                                        };
                                        function.parameters.get(requirement.parameter).map(|p| p.ty)
                                    })
                                    .is_some_and(|root| {
                                        let mut pending = vec![root];
                                        let mut seen = std::collections::HashSet::new();
                                        while let Some(ty) = pending.pop() {
                                            if !seen.insert(ty) {
                                                continue;
                                            }
                                            let row = self.t.get(ty).unwrap();
                                            if matches!(
                                                row.kind,
                                                TypeKind::Function(_)
                                                    | TypeKind::Named { .. }
                                                    | TypeKind::Builtin {
                                                        kind: BuiltinType::Iter,
                                                        ..
                                                    }
                                            ) {
                                                return true;
                                            }
                                            pending.extend(row.kind.value_parts());
                                        }
                                        false
                                    });
                            !may_invoke || self.crossing_borrows_live(state, &value)
                        });
                if !self.crossing_borrows_live(state, &callable) || !argument_inputs_live {
                    self.report(
                        const { RuleId::of("MODE-4") },
                        call_site,
                        "a borrowed capture has lost its required crossing proof",
                        |_| {
                            Explanation::default()
                                .provided(Fact::new(Cause::NotSendable, call_site, &[]))
                                .required(Fact::new(
                                    Requirement::SendableType,
                                    call_site,
                                    &["the borrowed capture"],
                                ))
                                .into()
                        },
                    );
                }
                state.argument_loans.retain(|_, loan| loan.1 != *expression);
                let mut failed = state.argument_conflicts.remove(expression)
                    || arguments.iter().any(|a| self.value(state, a.value).invalid);
                if !self.seal_entry(state, *expression, arguments, requirements) {
                    failed = true;
                }
                if !failed
                    && !self.replacement_content(
                        state,
                        *expression,
                        *callee,
                        arguments,
                        requirements,
                    )
                {
                    failed = true;
                }
                for (index, arg) in arguments.iter().enumerate() {
                    if self.property(arg.value).is_none() {
                        continue;
                    }
                    let value = self.value(state, arg.value);
                    let mut mode = Context::Read;
                    if let ExprKind::Unary { op, .. } =
                        self.tree.expressions[arg.value.0 as usize].kind
                    {
                        mode = match op {
                            UnaryOp::Take | UnaryOp::Copy | UnaryOp::Unique => Context::Take,
                            UnaryOp::Mut => Context::Mut,
                            _ => Context::Read,
                        };
                    }
                    if !failed && *constructor && mode == Context::Mut {
                        // A constructor stores its operands; it has no place to
                        // lend exclusively (OWN-3).
                        let operand = self.span_of(arg.value);
                        self.report(
                            const { RuleId::of("OWN-3") },
                            call_site,
                            "a constructor operand cannot be passed mut",
                            |_| {
                                Explanation::default()
                                    .provided(Fact::new(Cause::MutLend, operand, &[]))
                                    .required(Fact::new(
                                        Requirement::ConstructorOperand,
                                        call_site,
                                        &[],
                                    ))
                                    .into()
                            },
                        );
                        failed = true;
                    }
                    if let Some(requirement) = requirements.get(index).and_then(Option::as_ref) {
                        if !failed
                            && requirement.passing == PassingMode::Mut
                            && mode != Context::Mut
                        {
                            let operand = self.span_of(arg.value);
                            let callee_site = self.span_of(*callee);
                            self.report(
                                const { RuleId::of("OWN-3") },
                                call_site,
                                "a mut parameter requires mut at the call site",
                                |_| {
                                    Explanation::default()
                                        .provided(Fact::new(Cause::NotMut, operand, &[]))
                                        .required(Fact::new(
                                            Requirement::MutArgument,
                                            callee_site,
                                            &[],
                                        ))
                                        .into()
                                },
                            );
                            failed = true;
                        }
                        // A plain place is copied into a take parameter, but
                        // `mut` lends the place itself and a lent place is not
                        // an owned argument. A block, branch or arm is
                        // transparent to the take (LANG-11, #50): an outer
                        // place in its tail is passed as itself (#83).
                        let lent_tail = (mode == Context::Read)
                            .then(|| self.outer_place_tail(arg.value))
                            .flatten();
                        if !failed
                            && requirement.passing == PassingMode::Take
                            && mode != Context::Take
                            && (lent_tail.is_some()
                                || place(self.body, arg.value, self.source_rows).is_some()
                                    && (mode == Context::Mut
                                        || !self.property(arg.value).is_some_and(|p| p.plain)))
                        {
                            let operand = self.span_of(lent_tail.unwrap_or(arg.value));
                            let callee_site = self.span_of(*callee);
                            let lent_at = lent_tail.unwrap_or(arg.value);
                            let lent = place(self.body, lent_at, self.source_rows);
                            self.report(
                                const { RuleId::of("OWN-2") },
                                call_site,
                                "a take parameter requires an owned argument",
                                |c| {
                                    let mut explanation = Explanation::default()
                                        .provided(Fact::new(Cause::NotTaken, operand, &[]))
                                        .required(Fact::new(
                                            Requirement::TakeArgument,
                                            callee_site,
                                            &[],
                                        ));
                                    // At its last use, an owned binding can be moved.
                                    if let Some(lent) = lent.filter(|p| {
                                        p.projections.is_empty()
                                            && mode == Context::Read
                                            && c.body.bindings[p.binding].owner == c.graph.owner
                                            && !matches!(
                                                c.body.bindings[p.binding].binding,
                                                ResolvedName::Parameter(parameter)
                                                    if c.parameter_passing(&c.graph.owner, parameter)
                                                        != PassingMode::Take
                                            )
                                            && c.unused_after(p.binding, lent_at).is_some()
                                    }) {
                                        let (name, _) = c.binding_site(lent.binding);
                                        explanation.fix = Some(hirsel_diag::Fix {
                                            span: Span { end: operand.start, ..operand },
                                            replacement: "take ".into(),
                                            message: format!("move `{name}` at its last use"),
                                        });
                                        explanation
                                            .repairs
                                            .push(hirsel_modes::explain::Repair::TakeAtLastUse(name));
                                    }
                                    explanation.into()
                                },
                            );
                            failed = true;
                        }
                    }
                    if mode != Context::Take {
                        if let Some(p) = place(self.body, arg.value, self.source_rows) {
                            loans.push((p, mode == Context::Mut, arg.value));
                        }
                    }
                    // Scoped closure environments and local-result provenance count
                    // as borrowed call arguments, not only the closure's own slot.
                    let content = value.content.summary();
                    for root in content.bindings(Kind::Read).chain(value.captures()) {
                        loans.push((
                            Place {
                                binding: root,
                                projections: vec![].into(),
                            },
                            false,
                            arg.value,
                        ));
                    }
                }
                if !failed {
                    for i in 0..loans.len() {
                        for j in 0..i {
                            if loans[i].0.overlaps(&loans[j].0) && (loans[i].1 || loans[j].1) {
                                let (earlier, later) = (loans[j].2, loans[i].2);
                                let binding = loans[i].0.binding;
                                self.report(
                                    const { RuleId::of("OWN-3") },
                                    call_site,
                                    "exclusive call arguments overlap storage",
                                    |c| {
                                        let (name, _) = c.binding_site(binding);
                                        Explanation::default()
                                            .provided(Fact::new(
                                                Cause::ArgumentLoan,
                                                c.span_of(earlier),
                                                &[&name],
                                            ))
                                            .required(Fact::new(
                                                Requirement::Exclusive,
                                                c.span_of(later),
                                                &[&name],
                                            ))
                                            .into()
                                    },
                                );
                                failed = true;
                            }
                        }
                    }
                }
                if !failed {
                    // Callee entry starts each `mut` argument's exclusive loan:
                    // only values still live then conflict with it (LANG-3).
                    // The arguments themselves were checked against each
                    // other above.
                    let evaluated: Vec<_> = arguments
                        .iter()
                        .filter_map(|a| state.values.remove_entry(&a.value))
                        .collect();
                    for arg in arguments {
                        if let ExprKind::Unary {
                            op: UnaryOp::Mut,
                            value,
                        } = self.tree.expressions[arg.value.0 as usize].kind
                            && let Some(place) = place(self.body, value, self.source_rows)
                            && !self.unlent(&place, state, call_site, true)
                        {
                            failed = true;
                            break;
                        }
                    }
                    state.values.extend(evaluated);
                }
                if !failed {
                    for (arg, requirement) in arguments.iter().zip(requirements) {
                        let Some(requirement) = requirement else {
                            continue;
                        };
                        if self.property(arg.value).is_some() {
                            let actual = self.entry_value(state, arg.value);
                            let before = self.diagnostics.len();
                            let satisfied = self.required(
                                state,
                                &actual,
                                &requirement.modes,
                                arg.value,
                                call_site,
                                Fact::new(
                                    Requirement::Argument,
                                    self.span_of(*callee),
                                    &[&callee_name],
                                ),
                            );
                            let satisfied = satisfied
                                && self.required_nested_crossing(
                                    &actual,
                                    requirement.ty,
                                    arg.value,
                                    call_site,
                                )
                                && self.required_generic_crossing(
                                    &actual,
                                    *expression,
                                    *callee,
                                    requirement.parameter,
                                    arg.value,
                                    call_site,
                                )
                                && self.required_shared_parts(
                                    &actual,
                                    requirement.ty,
                                    arg.value,
                                    call_site,
                                );
                            if satisfied
                                && requirement.sendable_bound
                                && self.captured_sendability(arg.value, &actual)
                                    != Sendability::Sendable
                            {
                                let operand = self.span_of(arg.value);
                                self.report(
                                    const { RuleId::of("MODE-4") },
                                    call_site,
                                    "each actual generic occurrence must be sendable before parameter weakening",
                                    |_| {
                                        Explanation::default()
                                            .provided(Fact::new(Cause::Aliased, operand, &[]))
                                            .required(Fact::new(Requirement::Sendable, call_site, &[]))
                                            .into()
                                    },
                                );
                            }
                            let argument = self.tree.expressions[arg.value.0 as usize].origin.span;
                            // A core operation's own rule names its argument's
                            // failed uniqueness or crossing requirement.
                            let owner = if self.diagnostics.len() > before {
                                self.intrinsic_owner(*callee)
                            } else {
                                None
                            };
                            for diagnostic in &mut self.diagnostics[before..] {
                                if let Some((owner, rules)) = &owner
                                    && rules.contains(&diagnostic.rule.as_str())
                                {
                                    diagnostic.rule = owner.clone();
                                }
                                diagnostic.secondary.push(hirsel_diag::Label {
                                    span: argument,
                                    message: "parameter mode requirement".into(),
                                });
                                diagnostic
                                    .obligations
                                    .push(Obligation::ParameterMode { argument });
                            }
                        }
                    }
                }
                // MODE-2: the arguments unique at callee entry, before the
                // result's own reads share anything.
                let unique_arguments: BTreeSet<ExprId> = arguments
                    .iter()
                    .filter(|arg| {
                        self.property(arg.value)
                            .is_some_and(|p| p.plain || p.shared)
                            || self.entry_value(state, arg.value).modes.uniqueness
                                == Uniqueness::Unique
                    })
                    .map(|arg| arg.value)
                    .collect();
                if self.emit && !failed && !*constructor && self.arenas.global(&result.relation) {
                    let values: Vec<ExprId> = arguments.iter().map(|a| a.value).collect();
                    for (arg, requirement) in arguments.iter().zip(requirements) {
                        if requirement
                            .as_ref()
                            .is_some_and(|r| r.passing == PassingMode::Borrow)
                            && !self.value(state, arg.value).invalid
                        {
                            self.redundant_copy(*callee, arg.value, &values);
                        }
                    }
                }
                let environment_local =
                    !*constructor && self.value(state, *callee).environment_local();
                let replacement = self
                    .replacement_operands(*callee, arguments, requirements)
                    .and_then(|(destination, incoming)| {
                        let storage = place(self.body, destination, self.source_rows)?;
                        let mut outgoing = state.slots[storage.binding].value.clone();
                        outgoing.project_content(&storage.projections);
                        Some((
                            destination,
                            storage,
                            self.entry_value(state, incoming),
                            outgoing,
                        ))
                    });
                let mut value = Value::new(result.clone());
                value.site = Some(*expression);
                value.invalid = failed;
                if environment_local {
                    value.modes.relation = Relation::environment();
                }
                if let Some(captures) = captures.as_ref().filter(|_| !failed) {
                    if value.modes.relation.is_global() {
                        let mut state_contracts = BTreeMap::new();
                        if let Some(callee_ty) = self.ty(*callee) {
                            if let TypeKind::Function(function) =
                                &self.t.get(callee_ty).unwrap().kind
                            {
                                for (state, callback, slot) in
                                    mutable_capture_states(self.t, function)
                                {
                                    if let Some(argument) =
                                        arguments.get(callback).and_then(|a| self.ty(a.value))
                                    {
                                        let proof = mutable_capture_state_sendability(
                                            self.t,
                                            argument,
                                            slot,
                                            &self.body.resolution.generic_declarations,
                                            &mut |d| Ok(self.signatures.cached(d)),
                                        )
                                        .unwrap_or_else(|error| {
                                            *self.proof_error.borrow_mut() = Some(error);
                                            Sendability::NotSendable
                                        });
                                        let prior = state_contracts
                                            .remove(&state)
                                            .unwrap_or(Sendability::Sendable);
                                        state_contracts
                                            .insert(state, join_sendability(prior, proof));
                                    }
                                }
                            }
                        }
                        value.modes.sendability =
                            captures
                                .iter()
                                .fold(Sendability::Sendable, |sendability, index| {
                                    let argument = self.entry_value(state, arguments[*index].value);
                                    let captured = self
                                        .captured_sendability(arguments[*index].value, &argument);
                                    let captured = join_sendability(
                                        captured,
                                        state_contracts
                                            .get(index)
                                            .cloned()
                                            .unwrap_or(Sendability::Sendable),
                                    );
                                    join_sendability(sendability, captured)
                                });
                    }
                }
                if *constructor {
                    value.modes.uniqueness = Uniqueness::Unique;
                    for (arg, part) in arguments.iter().zip(parts) {
                        if let (false, Some(part)) = (failed, part) {
                            self.required_part(state, *part, arg.value, *expression);
                        }
                    }
                    for arg in arguments {
                        let mut part = self.value(state, arg.value);
                        if self
                            .property(arg.value)
                            .is_some_and(|p| p.handle_root || p.plain)
                        {
                            part.modes.uniqueness = Uniqueness::Unique;
                        }
                        value.join(&part);
                    }
                    let parts: Vec<_> = arguments.iter().map(|arg| arg.value).collect();
                    if !failed && self.parts_sendable(state, *expression, &value, &parts) {
                        value.modes.sendability = Sendability::Sendable;
                    }
                    if let Some(call) = self
                        .source_rows
                        .call(*expression)
                        .map(|i| &self.body.calls[i])
                    {
                        let mut retained = Vec::new();
                        value.crossing_borrows = Arc::new(Vec::new());
                        for argument in &call.arguments {
                            if let Some((ordinal, _)) = &argument.parameter {
                                let projection = match &call.target {
                                    CallTarget::Constructor(ContextualName::Variant {
                                        variant,
                                        ..
                                    }) => Projection::VariantField {
                                        variant: variant.0,
                                        field: *ordinal,
                                    },
                                    _ => Projection::Field(*ordinal),
                                };
                                self.construction_part(
                                    &mut value,
                                    &projection,
                                    argument.expression,
                                    &self.value(state, argument.expression),
                                );
                                value.install_crossing_borrows(
                                    &[projection.clone()],
                                    &self.value(state, argument.expression),
                                );
                                append_parts(
                                    &mut retained,
                                    &[projection],
                                    &self.value(state, argument.expression),
                                    self.retention_depth,
                                );
                            }
                        }
                        if let CallTarget::Constructor(ContextualName::Variant { variant, .. }) =
                            &call.target
                            && let Some(root) = self.ty(*expression)
                        {
                            for (row, _) in hirsel_types::unique_summary::result_rows(
                                self.t,
                                root,
                                &self.own.metadata.roles,
                                &mut |d| self.signatures.cached(d),
                            ) {
                                if let UniqueResultRow::Payload {
                                    variant: other,
                                    field,
                                } = row
                                    && other != variant.0
                                {
                                    value.unique_parts.insert(vec![Projection::VariantField {
                                        variant: other,
                                        field,
                                    }]);
                                }
                            }
                        }
                        value.install_content(retained);
                    }
                    value.modes.multiplicity = result.multiplicity;
                } else if let Some(parameters) = result.relation.named_parameters() {
                    value.modes.relation = Relation::global();
                    if parameters
                        .iter()
                        .any(|parameter| !matches!(parameter, ParameterRef::Bound { depth: 0, .. }))
                    {
                        value.bound_within(&self.value(state, *callee), true);
                    }
                    for parameter in &parameters {
                        if let ParameterRef::Body { scope, parameter } = parameter {
                            if let Some(index) = self.body.bindings.iter().position(|b| {
                                self.body.lexical.site(b.owner) == *scope
                                    && b.binding == ResolvedName::Parameter(*parameter)
                            }) {
                                let part = &state.slots[index].value;
                                if !self.arenas.global(&part.modes.relation) {
                                    value.modes.relation = self
                                        .join_lifetime(&value.modes.relation, &part.modes.relation);
                                }
                                if self.arenas.placed(
                                    &self
                                        .t
                                        .get(self.body.bindings[index].ty)
                                        .unwrap()
                                        .modes
                                        .relation,
                                ) {
                                    value.modes.relation = Relation::environment();
                                }
                                value.read(index, Site::Expression(*expression));
                                value.read_through(part);
                                value.inherit_crossing_borrows(part);
                                if self.ty(*expression).is_some_and(|ty| {
                                    matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_))
                                }) {
                                    value.inherit_callable_captures(part);
                                }
                            } else {
                                value.modes.relation = Relation::environment();
                            }
                            continue;
                        }
                        let ParameterRef::Bound {
                            depth: 0,
                            parameter,
                        } = parameter
                        else {
                            value.modes.relation = Relation::environment();
                            continue;
                        };
                        let position = requirements.iter().position(|r| {
                            r.as_ref()
                                .is_some_and(|r| r.parameter == parameter.0 as usize)
                        });
                        let requirement_passing = position
                            .and_then(|i| requirements[i].as_ref())
                            .map(|r| r.passing);
                        if let Some(arg) = position.and_then(|i| arguments.get(i)) {
                            let part = self.value(state, arg.value);
                            let mut retained = part.clone();
                            if let Some(source) = place(self.body, arg.value, self.source_rows) {
                                if let Some(within) = self.field_loan(state, arg.value, &source) {
                                    retained.project_content(
                                        &within
                                            .into_iter()
                                            .map(Projection::Field)
                                            .collect::<Vec<_>>(),
                                    );
                                }
                            }
                            value.inherit_crossing_borrows(&retained);
                            if self.ty(*expression).is_some_and(|ty| {
                                matches!(self.t.get(ty).unwrap().kind, TypeKind::Function(_))
                            }) {
                                value.inherit_callable_captures(&retained);
                            }
                            if !self.arenas.global(&part.modes.relation) {
                                value.modes.relation =
                                    self.join_lifetime(&value.modes.relation, &part.modes.relation);
                            }
                            let placed = self.ty(arg.value).is_some_and(|ty| {
                                self.arenas.placed(&self.t.get(ty).unwrap().modes.relation)
                            });
                            let source = place(self.body, arg.value, self.source_rows);
                            // An arena-only argument, computed, named or
                            // copied, contributes its arena bound and a read
                            // loan on its place, which ends at return (MODE-1,
                            // P2). A taken parameter's storage is not one: a
                            // result holding it must name it (MODE-1). A
                            // closure call's result may share what its
                            // environment captures (#80): it is arena-only
                            // when the closure's captures are known and each
                            // is, and it reads them. A written use mode on the
                            // call shares what the call does.
                            let mut call = arg.value;
                            while let ExprKind::Unary {
                                op:
                                    hirsel_syntax::UnaryOp::Copy
                                    | hirsel_syntax::UnaryOp::Take
                                    | hirsel_syntax::UnaryOp::Mut
                                    | hirsel_syntax::UnaryOp::Unique,
                                value,
                            } = &self.tree.expressions[call.0 as usize].kind
                            {
                                call = *value;
                            }
                            let closure_call = self.source_rows.call(call).is_some_and(|i| {
                                matches!(self.body.calls[i].target, CallTarget::Value(_))
                            });
                            let shared = if closure_call {
                                self.shared_captures(state, call)
                            } else {
                                Some((vec![], false))
                            };
                            let arena_only = placed
                                && shared.as_ref().is_some_and(|(_, taken)| !taken)
                                && self.arenas.global(&part.modes.relation)
                                && !source
                                    .as_ref()
                                    .is_some_and(|p| self.taken_storage(state, p.binding))
                                && !self.tail_taken(state, arg.value)
                                && !part
                                    .roots()
                                    .into_iter()
                                    .any(|root| self.taken_storage(state, root));
                            if placed && !arena_only {
                                value.modes.relation = Relation::environment();
                            }
                            // A written copy or a moved value is a new value,
                            // as a computed argument is: neither is the place
                            // it reads. One that is local bounds the result to
                            // a temporary; a global heap value is its own
                            // share.
                            // So is a closure call over known captures none of
                            // which is taken storage: it shares them as their
                            // copy would (#80). One over a taken capture is
                            // bounded by its temporary, as lowering keeps it.
                            let computed = matches!(
                                self.tree.expressions[arg.value.0 as usize].kind,
                                ExprKind::Unary {
                                    op: hirsel_syntax::UnaryOp::Copy | hirsel_syntax::UnaryOp::Take,
                                    ..
                                }
                            ) || closure_call
                                && shared.as_ref().is_some_and(|(_, taken)| !taken);
                            if (computed || source.is_none())
                                && (!self.arenas.global(&part.modes.relation)
                                    || (placed && !computed && !arena_only))
                            {
                                let scope = self.graph.lifetimes.expressions[&arg.value];
                                value.bound_by(scope, Site::Expression(arg.value));
                            }
                            let mut part = part;
                            if let Some(p) = source {
                                // A result reads the argument's field place,
                                // extended by the field path of this parameter
                                // its relation names (#77).
                                let copied = matches!(
                                    self.tree.expressions[arg.value.0 as usize].kind,
                                    ExprKind::Unary {
                                        op: hirsel_syntax::UnaryOp::Copy,
                                        ..
                                    }
                                );
                                let within = (!computed || copied)
                                    .then(|| self.field_loan(state, arg.value, &p))
                                    .flatten();
                                if within.is_some() && part.aliased() == Some(p.binding) {
                                    part.set_alias(None);
                                }
                                // MODE-1: relations compose through a local view.
                                let composed = !computed
                                    && within.is_none()
                                    && requirement_passing == Some(PassingMode::Borrow)
                                    && self.related_view(state, p.binding, *expression);
                                if composed {
                                    // It reads what the binding reaches, not
                                    // the binding itself.
                                    if part.aliased() == Some(p.binding) {
                                        part.set_alias(None);
                                    }
                                    if self.emit
                                        && !self.facts.composed_arguments.contains(&arg.value)
                                    {
                                        self.facts.composed_arguments.push(arg.value);
                                    }
                                } else {
                                    value.read_within(
                                        p.binding,
                                        within.as_deref().unwrap_or(&[]),
                                        Site::Expression(arg.value),
                                    );
                                }
                                if !computed && !arena_only && !composed {
                                    self.retain_binding_scope(&mut value, p.binding);
                                }
                            }
                            for capture in shared
                                .into_iter()
                                .flat_map(|(captures, _)| captures)
                                .filter(|_| arena_only)
                            {
                                value.read(capture, Site::Expression(arg.value));
                            }
                            value.bound_within(&part, true);
                            value.read_through(&part);
                        }
                    }
                    if self.property(*expression).is_some_and(|p| p.plain) {
                        value
                            .content
                            .whole
                            .retain(|loan| !matches!(loan.on, On::Scope(_)));
                    }
                    if !copied_callable_result
                        && self
                            .property(*expression)
                            .is_some_and(|p| !p.plain && !p.flat && !p.shared)
                    {
                        let mut roots = BTreeSet::new();
                        for root in value.content.summary().bindings(Kind::Read) {
                            let source = &state.slots[root].value;
                            roots.insert(root);
                            roots.extend(source.aliased().into_iter().chain(source.sources()));
                        }
                        // A function value's body is unproved: its result's
                        // share may be kept anywhere.
                        let direct = self.callee_definition(*callee).is_some();
                        for root in roots {
                            if direct {
                                state.slots[root].value.borrow_share();
                            } else {
                                state.slots[root].value.share();
                            }
                            if !self.properties[self.body.bindings[root].ty].sendable_aliased {
                                state.slots[root].value.modes.sendability =
                                    Sendability::NotSendable;
                                state.slots[root]
                                    .value
                                    .crossing_parts
                                    .set(&[], Sendability::NotSendable);
                            }
                        }
                    }
                }
                if !*constructor
                    && (environment_local
                        || result.relation.has_environment()
                        || result
                            .relation
                            .reaches()
                            .iter()
                            .any(|r| matches!(r, Reach::Region(_))))
                {
                    let callable = self.value(state, *callee);
                    value.bound_within(&callable, true);
                    value.read_through(&callable);
                    if let Some(p) = place(self.body, *callee, self.source_rows) {
                        value.read(p.binding, Site::Expression(*callee));
                    }
                    for argument in arguments {
                        let part = self.value(state, argument.value);
                        // A closure's or function type's bare local result may
                        // be local to any argument (#72), and a copied, moved
                        // or computed argument is a temporary of this body.
                        let computed = matches!(
                            self.tree.expressions[argument.value.0 as usize].kind,
                            ExprKind::Unary {
                                op: hirsel_syntax::UnaryOp::Copy | hirsel_syntax::UnaryOp::Take,
                                ..
                            }
                        ) || place(self.body, argument.value, self.source_rows)
                            .is_none();
                        if computed && self.property(argument.value).is_some_and(|p| !p.plain) {
                            let scope = self.graph.lifetimes.expressions[&argument.value];
                            value.bound_by(scope, Site::Expression(argument.value));
                        }
                        value.bound_within(&part, true);
                        value.read_through(&part);
                        if let Some(p) = place(self.body, argument.value, self.source_rows) {
                            value.read(p.binding, Site::Expression(argument.value));
                            if !computed {
                                self.retain_binding_scope(&mut value, p.binding);
                            }
                        }
                    }
                    if !copied_callable_result
                        && self
                            .property(*expression)
                            .is_some_and(|p| !p.plain && !p.flat && !p.shared)
                    {
                        for root in value.content.summary().bindings(Kind::Read) {
                            state.slots[root].value.share();
                            if !self.properties[self.body.bindings[root].ty].sendable_aliased {
                                state.slots[root].value.modes.sendability =
                                    Sendability::NotSendable;
                                state.slots[root]
                                    .value
                                    .crossing_parts
                                    .set(&[], Sendability::NotSendable);
                            }
                        }
                    }
                }
                // MODE-2: a counted result of calling a closure may share
                // what its environment borrows, as a part read of that
                // storage does, so its borrowed captures stop being unique,
                // wherever they live (#80). It adds no loan.
                if !*constructor
                    && self
                        .property(*expression)
                        .is_some_and(|p| !p.plain && !p.flat && !p.shared)
                {
                    let mut shared = BTreeSet::new();
                    for root in self.value(state, *callee).captures() {
                        shared.insert(root);
                        shared.extend(self.reached_bindings(state, &state.slots[root].value));
                    }
                    for root in shared {
                        let owner = state.slots[root].value.element_owner();
                        state.share_element_owners(owner.filter(|owner| *owner != root), None);
                        state.slots[root].value.share();
                        if !self.properties[self.body.bindings[root].ty].sendable_aliased {
                            state.slots[root].value.modes.sendability = Sendability::NotSendable;
                            state.slots[root]
                                .value
                                .crossing_parts
                                .set(&[], Sendability::NotSendable);
                        }
                    }
                }
                if self.property(*expression).is_some_and(|p| p.plain) {
                    value.modes.relation = Relation::global();
                    value.content = Content::default();
                }
                // MODE-2: what a `@preserves_unique` callee may install comes
                // from its taken and `mut` arguments only, and its proof
                // covers other `mut` arguments aliased. A bodyless core
                // callee has no proof: its other `mut` arguments must be
                // unique too, and it may also store a share of a borrowed
                // argument, so that argument must be Plain or Shared, unless
                // the ledger marks it unretained.
                let ledger = self
                    .callee_definition(*callee)
                    .is_some_and(|path| path.package_is("core", "1"));
                let unique_inputs = arguments
                    .iter()
                    .zip(requirements)
                    .all(|(arg, requirement)| {
                        let borrowed = requirement
                            .as_ref()
                            .is_none_or(|r| r.passing == PassingMode::Borrow);
                        let retained = ledger
                            && requirement
                                .as_ref()
                                .is_none_or(|r| !self.callee_unretained(*callee, r.parameter));
                        let mutable = requirement
                            .as_ref()
                            .is_some_and(|r| r.passing == PassingMode::Mut);
                        borrowed && !retained
                            || mutable && !ledger
                            || self
                                .property(arg.value)
                                .is_some_and(|p| p.plain || p.handle_root)
                            || !borrowed && unique_arguments.contains(&arg.value)
                    });
                let parametric =
                    self.parametric_unique(*callee, arguments, requirements, &unique_arguments);
                for (storage, mutable, argument) in &loans {
                    if *mutable {
                        state.slots[storage.binding].value.unique_parts.retain(|p| {
                            !p.starts_with(&storage.projections)
                                && !storage.projections.starts_with(p)
                        });
                    }
                    let declared = self.t.get(self.body.bindings[storage.binding].ty).unwrap();
                    let preserves_state = arguments.iter().zip(requirements).any(|(arg, req)| {
                        arg.value == *argument
                            && req.as_ref().is_some_and(|r| {
                                self.callee_preserves_state(*callee, r.parameter, false)
                            })
                    });
                    if *mutable && preserves_state {
                        continue;
                    }
                    // MODE-2: a plain `mut` callee may install a share anywhere
                    // in the place it receives, which ends its root's uniqueness.
                    // A `unique` parameter, or a declared preservation whose
                    // inputs are all unique, hands it back unique; a Plain or
                    // Shared part holds no counted content of the owner's.
                    let preserves_unique =
                        arguments
                            .iter()
                            .zip(requirements)
                            .any(|(arg, requirement)| {
                                arg.value == *argument
                                    && requirement.as_ref().is_some_and(|r| {
                                        r.modes.uniqueness == Uniqueness::Unique
                                            || unique_inputs
                                                && unique_arguments.contains(argument)
                                                && self
                                                    .callee_preserves_unique(*callee, r.parameter)
                                    })
                            })
                            || !storage.projections.is_empty()
                                && self
                                    .property(*argument)
                                    .is_some_and(|p| p.plain || p.shared);
                    if *mutable && !preserves_unique {
                        // A checked mutable hand-back may weaken the root, but
                        // must preserve its declared nested payload contracts.
                        // Keep prior real shares; this call creates no new copy.
                        state.slots[storage.binding].value.share_root();
                        if !storage.projections.is_empty() {
                            state.slots[storage.binding]
                                .value
                                .shared_parts
                                .insert(storage.projections.clone(), *argument);
                        }
                    }
                    if *mutable && storage.projections.is_empty() {
                        let receipts =
                            self.crossing_receipts(self.body.bindings[storage.binding].ty);
                        state.slots[storage.binding].value.crossing_parts =
                            CrossingParts::default();
                        for (path, _) in receipts {
                            state.slots[storage.binding]
                                .value
                                .crossing_parts
                                .set(&path, Sendability::Sendable);
                        }
                        // An arbitrary mut callee may replace the current value.
                        // Only its checked fixed contract (or structural Plain)
                        // supplies an outgoing proof; fresh input alone does not.
                        let crossing = crossing_requirement(declared);
                        state.slots[storage.binding].value.modes.sendability =
                            if self.properties[self.body.bindings[storage.binding].ty].plain {
                                Sendability::Sendable
                            } else {
                                crossing
                            };
                    } else if *mutable {
                        let preserves_crossing =
                            arguments
                                .iter()
                                .zip(requirements)
                                .any(|(arg, requirement)| {
                                    arg.value == *argument
                                        && requirement.as_ref().is_some_and(|r| {
                                            r.modes.sendability == Sendability::Sendable
                                        })
                                })
                                || self
                                    .property(*argument)
                                    .is_some_and(|p| p.plain || p.shared);
                        if !preserves_crossing {
                            state.slots[storage.binding].value.modes.sendability =
                                Sendability::NotSendable;
                            state.slots[storage.binding]
                                .value
                                .crossing_parts
                                .set(&storage.projections, Sendability::NotSendable);
                        }
                    }
                }
                let replaced = replacement.is_some();
                if let Some((destination, storage, mut incoming, outgoing)) = replacement {
                    // The intrinsic transfers the old content's loans to its
                    // result and the incoming loans to storage, as assignment
                    // does. Its invocation access is neither one's content.
                    if !self.property(*expression).is_some_and(|p| p.plain) {
                        value.content = outgoing.content.clone();
                        value.loans = outgoing.loans.clone();
                        value.shared_parts = outgoing.shared_parts.clone();
                        value.crossing_parts = outgoing.crossing_parts.clone();
                        value.crossing_borrows = outgoing.crossing_borrows.clone();
                        value.set_alias(None);
                    }
                    let access = match self.tree.expressions[destination.0 as usize].kind {
                        ExprKind::Unary {
                            op: UnaryOp::Mut,
                            value,
                        } => value,
                        _ => destination,
                    };
                    // Fixed receiving lifetime, independent of the current
                    // value and of the localized access used to infer T.
                    value.modes.relation =
                        if self.body.bindings[storage.binding].usage == UseKind::Mut {
                            self.receiving_contract(storage.binding)
                        } else if let Some(plan) = self.projection_lifetimes.get(&access) {
                            self.physical_projection_modes(state, plan).relation
                        } else {
                            state.slots[storage.binding].declared.clone()
                        };
                    if !failed {
                        if storage.projections.is_empty() {
                            // A Local binding keeps the lifetime fixed at its
                            // introduction, including any initial temporary bound.
                            for (kind, scope, site) in outgoing.content.summary().scopes() {
                                incoming.content.whole.insert(kind, On::Scope(scope), site);
                            }
                            if self.body.bindings[storage.binding].usage == UseKind::Mut {
                                incoming.set_alias(outgoing.alias());
                                incoming.modes.relation = outgoing.modes.relation.clone();
                            }
                            state.slots[storage.binding].value = incoming;
                        } else {
                            // Apply the same part receipts as assignment while
                            // preserving every sibling's content and evidence.
                            let mut shared = SharedParts::default();
                            if incoming.modes.uniqueness != Uniqueness::Unique {
                                shared.insert(vec![], *expression);
                            }
                            shared.install(&[], &incoming.shared_parts);
                            let target = &mut state.slots[storage.binding].value;
                            target.shared_parts.replace(&storage.projections, &shared);
                            target
                                .crossing_parts
                                .set(&storage.projections, incoming.modes.sendability.clone());
                            target
                                .crossing_parts
                                .install(&storage.projections, &incoming.crossing_parts);
                            Arc::make_mut(&mut target.crossing_borrows)
                                .retain(|borrow| !borrow.within.starts_with(&storage.projections));
                            target.install_crossing_borrows(&storage.projections, &incoming);
                            target.restrict_crossing_paths(self.retention_depth);
                            target.replace_content(
                                &storage.projections,
                                &incoming,
                                self.retention_depth,
                                &self.arenas,
                            );
                        }
                    }
                }
                self.receiving_scope(
                    &value,
                    self.graph.lifetimes.expressions[expression],
                    self.tree.expressions[expression.0 as usize].origin.span,
                    None,
                );
                value.fresh_receipt = true;
                value.usage = UseKind::Take;
                self.usage(*expression, value.usage);
                self.record(*expression, &mut value);
                // Flow evidence, as V derives it; the published row stays
                // the declared one (MODE-1).
                if !*constructor && !replaced {
                    self.mut_content_bounds(
                        state,
                        &mut value,
                        *expression,
                        arguments,
                        requirements,
                    );
                }
                // MODE-2: a `@preserves_unique` taken input passes its
                // uniqueness on to the result. This is flow evidence, as V
                // derives it; the call's published row stays the declared one.
                if unique_inputs
                    && requirements.iter().flatten().any(|r| {
                        r.passing == PassingMode::Take
                            && self.callee_preserves_unique(*callee, r.parameter)
                    })
                {
                    value.modes.uniqueness = Uniqueness::Unique;
                }
                // MODE-2: a parametric result holds only values its unique
                // inputs supply (`parametric_inputs`).
                if parametric {
                    value.modes.uniqueness = Uniqueness::Unique;
                }
                if !*constructor
                    && !replaced
                    // A bare generic result keeps its declared aliased root.
                    // Its instantiated actual is not an independent uniqueness
                    // promise; parametric/inferred evidence is handled above.
                    && self.ty(*callee).is_some_and(|ty| {
                        let TypeKind::Function(function) = &self.t.row(ty).kind else {
                            return false;
                        };
                        let result = self.t.row(function.result);
                        !matches!(result.kind, TypeKind::Parameter(_))
                            || result.modes.uniqueness == Uniqueness::Unique
                    })
                    && let Some(root) = self.ty(*expression)
                    && matches!(
                        self.t.row(root).kind,
                        TypeKind::Named { .. } | TypeKind::Tuple(_)
                    )
                {
                    for (row, ty) in hirsel_types::unique_summary::result_rows(
                        self.t,
                        root,
                        &self.own.metadata.roles,
                        &mut |d| self.signatures.cached(d),
                    ) {
                        if self.t.row(ty).modes.uniqueness != Uniqueness::Unique {
                            continue;
                        }
                        match row {
                            UniqueResultRow::Root => value.modes.uniqueness = Uniqueness::Unique,
                            UniqueResultRow::Tuple(field) => {
                                value.unique_parts.insert(vec![Projection::Field(field)]);
                            }
                            UniqueResultRow::Payload { variant, field } => {
                                value
                                    .unique_parts
                                    .insert(vec![Projection::VariantField { variant, field }]);
                            }
                        }
                    }
                }
                if let Some(path) = self.callee_definition(*callee)
                    && hirsel_types::unique_summary::same_module(path, self.own.definition.path())
                    && let Some(header) = self.signatures.cached(DefId::intern(path))
                {
                    for row in &header.metadata.inferred_unique.results {
                        match row {
                            UniqueResultRow::Root => value.modes.uniqueness = Uniqueness::Unique,
                            UniqueResultRow::Tuple(field) => {
                                value.unique_parts.insert(vec![Projection::Field(*field)]);
                            }
                            UniqueResultRow::Payload { variant, field } => {
                                value.unique_parts.insert(vec![Projection::VariantField {
                                    variant: *variant,
                                    field: *field,
                                }]);
                            }
                        }
                    }
                }
                state.values.insert(*expression, value);
            }
            Op::Capture {
                expression,
                escaping,
                owning,
            } => {
                let initial = self.value(state, *expression);
                let mut value = self.capture_value(state, *expression, *escaping, *owning, initial);
                self.record(*expression, &mut value);
                value.usage = UseKind::Take;
                self.usage(*expression, value.usage);
                state.values.insert(*expression, value);
            }
            Op::Propagate {
                expression,
                value,
                result,
            } => self.propagate(state, *expression, *value, result),
            Op::Return { value, result } => {
                self.observe_unique_result(&self.value(state, *value), self.span_of(*value));
                self.creation_return(state, *value);
                let mut result = result.clone();
                if self.environment_frame {
                    result.relation = Relation::environment();
                }
                let result = &result;
                // MODE-2: the result keeps only the shares it derives from.
                let held = if self.property(*value).is_some_and(|p| !p.plain) {
                    let returned = self.value(state, *value);
                    self.reached_bindings(state, &returned)
                } else {
                    BTreeSet::new()
                };
                self.record_return_uniqueness(state, &held);
                if self.property(*value).is_some() {
                    let actual = self.value(state, *value);
                    let cause = if result.uniqueness == Uniqueness::Unique
                        && self.arenas.global(&actual.modes.relation)
                    {
                        actual.uniqueness_site.or(actual.site)
                    } else {
                        actual.site
                    };
                    let site = self.tree.expressions[cause.unwrap_or(*value).0 as usize]
                        .origin
                        .span;
                    if self.proves_result()
                        && actual.modes.uniqueness != Uniqueness::Unique
                        && !self.property(*value).is_some_and(|p| p.plain || p.shared)
                    {
                        self.report(
                            const { RuleId::of("MODE-2") },
                            site,
                            UNIQUE_PROOF_FAILED,
                            |_| {
                                Explanation::default()
                                    .provided(Fact::new(Cause::Aliased, site, &[]))
                                    .required(Fact::new(Requirement::Unique, site, &[]))
                                    .into()
                            },
                        );
                    }
                    if actual.invalid {
                        return;
                    }
                    let requirement = self.result_requirement(result);
                    if let Some(root) = self.graph.result_ty {
                        if self.required_nested_crossing(&actual, root, *value, site) {
                            self.required_shared_parts(&actual, root, *value, site);
                        }
                    }
                    let property = self.property(*value);
                    self.result_locality(
                        state,
                        &actual,
                        *value,
                        property,
                        site,
                        result,
                        requirement,
                    );
                }
            }
        }
        // These operations publish real failure edges in TIR. Check the
        // caller-owned contract after their operand/capture sharing, separately
        // from the normal-return summaries exported below. Frame cleanup drops
        // holders but never manufactures new crossing evidence (#96, 306838).
        let failure_site = match op {
            Op::Call { expression, .. } | Op::Capture { expression, .. } => Some(*expression),
            Op::Finish { expression, .. } => {
                match &self.tree.expressions[expression.0 as usize].kind {
                    ExprKind::Unary {
                        op: UnaryOp::Copy | UnaryOp::Take | UnaryOp::Mut,
                        ..
                    } => None,
                    ExprKind::Binary {
                        op: hirsel_syntax::BinaryOp::And | hirsel_syntax::BinaryOp::Or,
                        ..
                    } => None,
                    ExprKind::Unary { .. }
                    | ExprKind::Binary { .. }
                    | ExprKind::Tuple(_)
                    | ExprKind::Sequence(_)
                    | ExprKind::Index { .. } => Some(*expression),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(expression) = failure_site {
            self.mutable_crossing_exit(state, self.span_of(expression), &BTreeSet::new());
        }
    }
    fn observe_unique_result(&mut self, actual: &Value, site: Span) {
        if !self.emit
            || self.unique_proof.is_some()
            || self.graph.owner != NameSite::Item
            || self.graph.clause.is_some()
        {
            return;
        }
        let proven: BTreeSet<_> = self
            .graph
            .result_ty
            .into_iter()
            .flat_map(|root| {
                hirsel_types::unique_summary::result_rows(
                    self.t,
                    root,
                    &self.own.metadata.roles,
                    &mut |d| self.signatures.cached(d),
                )
            })
            .filter_map(|(row, ty)| {
                let path = match row {
                    UniqueResultRow::Root => vec![],
                    UniqueResultRow::Tuple(field) => vec![Projection::Field(field)],
                    UniqueResultRow::Payload { variant, field } => {
                        vec![Projection::VariantField { variant, field }]
                    }
                };
                let plain = self
                    .properties
                    .get(ty)
                    .is_some_and(|p| p.plain || p.handle_root);
                let unique = actual.modes.uniqueness == Uniqueness::Unique
                    || plain
                    || !path.is_empty() && actual.unique_parts.iter().any(|p| path.starts_with(p));
                unique.then_some(row)
            })
            .collect();
        for (row, _) in self.graph.result_ty.into_iter().flat_map(|root| {
            hirsel_types::unique_summary::result_rows(
                self.t,
                root,
                &self.own.metadata.roles,
                &mut |d| self.signatures.cached(d),
            )
        }) {
            let claim = UniqueClaim::Result(row);
            if !proven.contains(&row) && !self.unique_failures.iter().any(|f| f.claim == claim) {
                self.unique_failures.push(UniqueFailure {
                    claim,
                    span: site,
                    message: "this return may retain counted shared storage".into(),
                });
            }
        }
        match &mut self.unique_results {
            Some(old) => old.retain(|row| proven.contains(row)),
            None => self.unique_results = Some(proven),
        }
    }
    /// LANG-8: `value ?` returns `Error(e)` for the error payload `e` of
    /// `value`, so `e` (or what its mapper makes of it) needs the locality
    /// of the result, exactly as an explicit `return Error(e)` does.
    fn propagate(&mut self, state: &State, expression: ExprId, value: ExprId, result: &Modes) {
        let Some((error, payload)) = self.ty(value).and_then(|ty| {
            let TypeKind::Named {
                definition,
                arguments,
            } = &self.t.get(ty)?.kind
            else {
                return None;
            };
            let [_, hirsel_types::GenericArgument::Type(payload)] = arguments.as_slice() else {
                return None;
            };
            let signature = self.signatures.cached(*definition)?;
            let SignatureKind::Type(declaration) = &signature.kind else {
                return None;
            };
            let TypeKind::Variant(variants) = &self.t.get(declaration.representation?)?.kind else {
                return None;
            };
            let error = variants.iter().position(|variant| {
                variant.name.as_str() == "Error" && variant.fields.len() == 1
            })?;
            Some((error as u32, *payload))
        }) else {
            *self.proof_error.borrow_mut() = Some(QueryError::Operational(OperationalFailure {
                operation: "ownership Result propagation".into(),
                message: "registered Result lacks its declared error payload".into(),
            }));
            return;
        };
        let property = self.properties.get(payload).copied();
        let mut actual = self.value(state, value);
        if actual.invalid {
            return;
        }
        actual.project_content(&[Projection::VariantField {
            variant: error,
            field: 0,
        }]);
        // A fresh call receipt owns its outer variant, but not an
        // unqualified counted error payload. A written nested unique
        // receipt, or a currently unique whole, supplies that evidence.
        if property.is_some_and(|p| p.plain || p.shared)
            || actual.fresh_receipt
                && self
                    .t
                    .get(payload)
                    .is_some_and(|row| row.modes.uniqueness == Uniqueness::Unique)
        {
            actual.modes.uniqueness = Uniqueness::Unique;
        }
        let target_error = self
            .graph
            .result_ty
            .and_then(|ty| match &self.t.get(ty)?.kind {
                TypeKind::Named { arguments, .. } => match arguments.as_slice() {
                    [_, GenericArgument::Type(error)] => Some(*error),
                    _ => None,
                },
                _ => None,
            });
        if let Some(target) = target_error
            && self.t.get(target).unwrap().modes.uniqueness == Uniqueness::Unique
            && !self
                .properties
                .get(target)
                .is_some_and(|p| p.plain || p.shared)
        {
            let mapper = self
                .body
                .resolution
                .paths
                .iter()
                .find_map(|(site, name)| match name {
                    ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                        Some(path)
                    }
                    _ => None,
                })
                .and_then(|path| {
                    self.signatures
                        .cached(DefId::intern(path))
                        .map(|signature| (path, signature))
                });
            let unique = if let Some((path, signature)) = mapper
                && let SignatureKind::Function(function) = &signature.kind
            {
                self.t.get(function.result).unwrap().modes.uniqueness == Uniqueness::Unique
                    || parametric_inputs(
                        self.t,
                        function,
                        !path.package_is("core", "1"),
                        &mut |d| Ok(self.signatures.cached(d)),
                    )
                    .is_some_and(|inputs| {
                        inputs.iter().all(|input| match input {
                            ParametricInput::Free => true,
                            ParametricInput::Unique => {
                                actual.modes.uniqueness == Uniqueness::Unique
                            }
                            ParametricInput::UniqueResult => {
                                match &self.t.get(payload).unwrap().kind {
                                    TypeKind::Function(callback) => unique_rows(
                                        self.t,
                                        callback.result,
                                        &self.body.resolution.generic_declarations,
                                        &mut |d| Ok(self.signatures.cached(d)),
                                    ),
                                    _ => false,
                                }
                            }
                        })
                    })
            } else {
                actual.modes.uniqueness == Uniqueness::Unique
            };
            if !unique {
                let site = self.span_of(expression);
                let operand = self.span_of(value);
                self.report(
                    const { RuleId::of("MODE-2") },
                    site,
                    "the propagated error does not prove the enclosing unique error contract",
                    |_| {
                        Explanation::default()
                            .provided(Fact::new(Cause::Aliased, operand, &[]))
                            .required(Fact::new(Requirement::Unique, site, &[]))
                            .into()
                    },
                );
            }
        }
        let mapper = self
            .body
            .resolution
            .paths
            .iter()
            .find_map(|(site, name)| match name {
                ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                    Some(path)
                }
                _ => None,
            })
            .and_then(|path| {
                self.signatures
                    .cached(DefId::intern(path))
                    .map(|s| (path, s))
            });
        let mapped_unique = if let Some((path, signature)) = mapper {
            if let SignatureKind::Function(function) = &signature.kind {
                self.t.row(function.result).modes.uniqueness == Uniqueness::Unique
                    || hirsel_types::unique_summary::same_module(path, self.own.definition.path())
                        && signature
                            .metadata
                            .inferred_unique
                            .results
                            .contains(&UniqueResultRow::Root)
            } else {
                false
            }
        } else {
            actual.modes.uniqueness == Uniqueness::Unique
                || property.is_some_and(|p| p.plain || p.handle_root)
        };
        let mut returned = Value::new(Modes::global());
        returned.modes.uniqueness = if mapped_unique {
            Uniqueness::Unique
        } else {
            Uniqueness::Aliased
        };
        if let Some(root) = self.graph.result_ty {
            for (row, _) in hirsel_types::unique_summary::result_rows(
                self.t,
                root,
                &self.own.metadata.roles,
                &mut |d| self.signatures.cached(d),
            ) {
                if let UniqueResultRow::Payload { variant, field } = row
                    && (variant != error || mapped_unique)
                {
                    returned
                        .unique_parts
                        .insert(vec![Projection::VariantField { variant, field }]);
                }
            }
        }
        self.observe_unique_result(&returned, self.span_of(expression));
        if property.is_some_and(|p| p.plain) || self.mapper_releases(expression) {
            return;
        }
        let mut result = result.clone();
        if self.environment_frame {
            result.relation = Relation::environment();
        }
        let requirement = self.result_requirement(&result);
        let site = self.span_of(expression);
        self.result_locality(state, &actual, value, property, site, &result, requirement);
    }
    /// A `?` mapper function whose result names no source returns a value
    /// independent of the error it was given; a variant mapper keeps it.
    fn mapper_releases(&self, expression: ExprId) -> bool {
        let Some(path) = self
            .body
            .resolution
            .paths
            .iter()
            .find_map(|(site, name)| match name {
                ResolvedName::Definition(path) if *site == NameSite::Expression(expression) => {
                    Some(path.clone())
                }
                _ => None,
            })
        else {
            return false;
        };
        let Some(signature) = self.signatures.cached(DefId::intern(&path)) else {
            return false;
        };
        let SignatureKind::Function(function) = &signature.kind else {
            return false;
        };
        self.t
            .get(function.result)
            .is_some_and(|row| row.modes.relation.is_global())
    }
    /// The locality a returned value needs to leave this frame as `result`
    /// (MODE-1, OWN-1, CLO-1): for a whole result, or the error a `?`
    /// propagates.
    #[allow(clippy::too_many_arguments)]
    fn result_locality(
        &mut self,
        state: &State,
        actual: &Value,
        value: ExprId,
        property: Option<Properties>,
        site: Span,
        result: &Modes,
        requirement: Fact<Requirement>,
    ) {
        let result_site = requirement.span;
        if actual.aliased().is_some() && !property.is_some_and(|p| p.copyable) {
            self.report(
                const { RuleId::of("OWN-1") },
                site,
                "an owned result cannot move out of borrowed storage",
                |c| {
                    c.locality(
                        state,
                        actual,
                        Fact::new(Requirement::OwnedResult, result_site, &[]),
                    )
                },
            );
        } else if actual.scoped() {
            self.report(
                const { RuleId::of("CLO-1") },
                site,
                "a scoped closure cannot be returned",
                |_| {
                    Explanation::default()
                        .provided(Fact::new(Cause::ScopedClosure, site, &[]))
                        .required(Fact::new(Requirement::ReturnClosure, result_site, &[]))
                        .into()
                },
            );
        } else if self.arenas.global(&result.relation)
            && actual.usage == UseKind::Borrow
            && actual.aliased().is_some()
            && !property.is_some_and(|p| p.plain)
        {
            // A scoped capture is a borrow of a possibly global value
            // (CLO-1): reading it does not make an owned result.
            self.report(
                const { RuleId::of("MODE-1") },
                site,
                "this operation requires a global value",
                |c| c.locality(state, actual, requirement),
            );
        } else if !self.arenas.global(&actual.modes.relation)
            && actual.content.summary().scopes().next().is_some()
        {
            // Whatever the result declares, its sources are named
            // storage of the caller; a temporary of this body is not.
            self.report(
                const { RuleId::of("MODE-1") },
                site,
                "a local result retains a temporary that ends with the function",
                |c| c.temporary_result(actual, result_site),
            );
        } else if self.arenas.lifetime(&result.relation).has_environment()
            && !self.arenas.global(&actual.modes.relation)
            && self.taken_source(state, actual).is_some()
        {
            // Only a result that names a taken parameter is as
            // local as that argument was. A callable's unnamed
            // local result cannot name one.
            self.report(
                const { RuleId::of("MODE-1") },
                site,
                "a local result retains a taken parameter its relation does not name",
                |c| {
                    let taken = c.taken_source(state, actual).unwrap();
                    let (name, span) = c.binding_site(taken);
                    Explanation::default()
                        .provided(Fact::new(Cause::TakenParameter, span, &[&name]))
                        .required(Fact::new(
                            Requirement::ResultTakenSource,
                            result_site,
                            &[
                                "",
                                &hirsel_modes::explain::qualifier(
                                    hirsel_modes::explain::Qualifier::In(&[&name]),
                                ),
                            ],
                        ))
                        .into()
                },
            );
        } else if self.parameter_parts.outside(actual) {
            self.report(
                const { RuleId::of("MODE-1") },
                site,
                "a local result reads a part of a parameter its relation does not name",
                |c| c.locality(state, actual, requirement),
            );
        } else {
            if let Some(parameters) = result.relation.named_parameters() {
                if !self.arenas.global(&actual.modes.relation)
                    && !self.result_sources_allowed(state, actual, &parameters)
                {
                    self.report(
                        const { RuleId::of("MODE-1") },
                        site,
                        "a local result retains a source its relation does not name",
                        |c| c.locality(state, actual, requirement),
                    );
                    return;
                }
            }
            self.required(state, actual, result, value, site, requirement);
        }
    }
    /// Project the finite parameter-prefix observations used by this frame's
    /// result check. Recursive reads cannot grow an unbounded path lattice.
    fn read_part(&self, value: &Value, place: &Place) -> Option<(usize, BTreeSet<usize>)> {
        let binding = &self.body.bindings[place.binding];
        let (root, states) = if binding.owner == self.graph.owner
            && matches!(binding.binding, ResolvedName::Parameter(_))
        {
            (
                place.binding,
                BTreeSet::from([self.parameter_parts.start(place.binding)?]),
            )
        } else {
            value.part_of.clone()?
        };
        let fields: Vec<u32> = place
            .projections
            .iter()
            .map_while(|projection| match projection {
                Projection::Field(field) => Some(*field),
                _ => None,
            })
            .collect();
        let states = states
            .into_iter()
            .map(|state| self.parameter_parts.project(state, fields.iter().copied()))
            .collect();
        Some((root, states))
    }

    fn result_sources_allowed(
        &self,
        state: &State,
        value: &Value,
        parameters: &[ParameterRef],
    ) -> bool {
        let allowed: BTreeSet<_> = parameters
            .iter()
            .filter_map(|p| {
                let (scope, parameter) = match p {
                    ParameterRef::Bound {
                        depth: 0,
                        parameter,
                    } => (self.graph.owner, *parameter),
                    ParameterRef::Body { scope, parameter } if scope.owner == self.body.lexical => {
                        (scope.site, *parameter)
                    }
                    _ => return None,
                };
                self.body.bindings.iter().position(|b| {
                    b.owner == scope && b.binding == ResolvedName::Parameter(parameter)
                })
            })
            .collect();
        let roots = value.reached();
        if roots.is_empty() {
            // A Local receipt may acquire content with no retained source
            // bounds. Its locality still prevents a Global result promise.
            return value.content.parts.is_some()
                && value.aliased().is_none()
                && value.content.summary().scopes().next().is_none();
        }
        // The result borrows the storage of the bindings it lends from. A
        // binding of this body that is not global (a counted copy of a
        // borrowed value, say) ends with the function, whatever its own value
        // was derived from.
        if value.content.summary().bindings(Kind::Read).any(|index| {
            let binding = &self.body.bindings[index];
            !matches!(binding.binding, ResolvedName::Parameter(_))
                && binding.owner == self.graph.owner
                && !self.arenas.global(&state.slots[index].value.modes.relation)
                && state.slots[index].value.aliased().is_none()
        }) {
            return false;
        }
        let mut pending: Vec<_> = roots.iter().copied().collect();
        let mut parents = BTreeMap::new();
        let mut proven = allowed;
        while let Some(index) = pending.pop() {
            if proven.contains(&index) || parents.contains_key(&index) {
                continue;
            }
            if self.taken_storage(state, index) {
                return false;
            }
            let source = &state.slots[index].value;
            if self.arenas.global(&source.modes.relation) {
                proven.insert(index);
                continue;
            }
            let mut dependencies = source.reached();
            dependencies.remove(&index);
            if dependencies.is_empty() {
                return false;
            }
            pending.extend(&dependencies);
            parents.insert(index, dependencies);
        }
        loop {
            let before = proven.len();
            for (index, dependencies) in &parents {
                if dependencies.is_subset(&proven) {
                    proven.insert(*index);
                }
            }
            if before == proven.len() {
                break;
            }
        }
        roots.is_subset(&proven)
    }

    /// The record field path within the argument binding `argument` that
    /// the call result of `expression` reads, when it reads only one (#77).
    /// The field place is published, typed as a physical part of the
    /// binding, for lowering to loan.
    fn field_loan(
        &mut self,
        state: &State,
        expression: ExprId,
        argument: &Place,
    ) -> Option<Vec<u32>> {
        // A projected argument is a field place of its binding.
        let prefix: Vec<u32> = argument
            .projections
            .iter()
            .map(|p| match p {
                Projection::Field(f) => Some(*f),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let Some(field) = self.field_loan_places.get(&expression) else {
            return (!prefix.is_empty()).then_some(prefix);
        };
        if self.emit && !self.facts.field_loans.iter().any(|(e, _)| *e == expression) {
            let (root, steps) = match self.projection_lifetimes.get(&expression) {
                Some(lifetime) if !prefix.is_empty() => (lifetime.root, lifetime.parts.as_slice()),
                _ if prefix.is_empty() => (
                    super::properties::ProjectionRoot::Binding(argument.binding),
                    &[][..],
                ),
                _ => return Some(prefix),
            };
            let owner = fold_part_locality(
                &self.arenas,
                self.physical_root_modes(state, root).relation,
                steps,
            );
            let relation = fold_part_locality(&self.arenas, owner, &field.parts);
            let mut row = self.t.get(field.declared).unwrap().clone();
            row.modes.relation = self.arenas.relocate(&row.modes.relation, &relation);
            let ty = self.intern(row);
            self.properties.inherit(ty, field.declared);
            self.facts.field_loans.push((expression, ty));
        }
        Some(
            prefix
                .into_iter()
                .chain(field.path.iter().copied())
                .collect(),
        )
    }
    /// The first taken parameter of this body whose storage ends with the
    /// function that the value reads, is bound by or aliases.
    fn taken_source(&self, state: &State, value: &Value) -> Option<usize> {
        let content = value.content.summary();
        content
            .bindings(Kind::Read)
            .chain(value.sources())
            .chain(value.aliased())
            .find(|index| self.taken_storage(state, *index))
    }
    /// MODE-1: a taken parameter of this body whose storage ends with the
    /// function: local, or placed in an arena or region. Only a result that
    /// names it is as local as the argument was; a global heap value is its
    /// own share.
    fn taken_storage(&self, state: &State, index: usize) -> bool {
        let binding = &self.body.bindings[index];
        matches!(binding.binding, ResolvedName::Parameter(p)
            if binding.owner == self.graph.owner
                && self.parameter_passing(&binding.owner, p) == PassingMode::Take)
            && (!self.arenas.global(&state.slots[index].value.modes.relation)
                || self
                    .arenas
                    .placed(&self.t.get(binding.ty).unwrap().modes.relation))
    }

    /// A part's declared uniqueness and multiplicity are contracts of the
    /// aggregate, so the value built into it must satisfy them where the
    /// aggregate is constructed (MODE-2, MODE-3). Locality is not required of
    /// a part: it lives in its owner's (OWN-8).
    fn crossing_receipts(&self, root: TypeId) -> Vec<(Vec<Projection>, bool)> {
        let mut receipts = self.crossing_receipts.borrow_mut();
        receipts
            .entry(root)
            .or_insert_with(|| {
                owned_crossing_receipts(
                    self.t,
                    root,
                    &self.body.resolution.generic_declarations,
                    &mut |d| Ok(self.signatures.cached(d)),
                )
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|(path, independent)| {
                            (
                                path.into_iter()
                                    .map(|p| match p {
                                        OwnedTypePart::Field(f) => Projection::Field(f),
                                        OwnedTypePart::VariantField { variant, field } => {
                                            Projection::VariantField { variant, field }
                                        }
                                        OwnedTypePart::Elements => Projection::Elements,
                                    })
                                    .collect(),
                                independent,
                            )
                        })
                        .collect()
                })
            })
            .clone()
            .unwrap_or_else(|error| {
                *self.proof_error.borrow_mut() = Some(error);
                Vec::new()
            })
    }
    fn required_nested_crossing(
        &mut self,
        actual: &Value,
        root: TypeId,
        expression: ExprId,
        site: Span,
    ) -> bool {
        self.required_crossing_paths(actual, self.crossing_receipts(root), expression, site)
    }
    fn required_generic_crossing(
        &mut self,
        actual: &Value,
        call: ExprId,
        callee: ExprId,
        parameter: usize,
        expression: ExprId,
        site: Span,
    ) -> bool {
        let Some(ty) = expression_type(self.body, callee, self.source_rows) else {
            return true;
        };
        let Some(Type {
            kind: TypeKind::Function(function),
            ..
        }) = self.t.get(ty)
        else {
            return true;
        };
        let function = function.clone();
        let Some(call) = self.source_rows.call(call).map(|i| &self.body.calls[i]) else {
            return true;
        };
        let arguments = call.generic_arguments.clone();
        let receiving = generic_crossing_parameter(
            self.t,
            self.signatures.m,
            &function,
            parameter,
            &arguments,
            &NominalRoles::default(),
        )
        .map(|root| root.map(|root| self.crossing_receipts(root)));
        match receiving {
            Ok(Some(receipts)) => self.required_crossing_paths(actual, receipts, expression, site),
            Ok(None) => true,
            Err(error) => {
                *self.proof_error.borrow_mut() =
                    Some(QueryError::Operational(OperationalFailure {
                        operation: "generic crossing receipt".into(),
                        message: format!("{error:?}"),
                    }));
                false
            }
        }
    }
    fn required_crossing_paths(
        &mut self,
        actual: &Value,
        paths: Vec<(Vec<Projection>, bool)>,
        expression: ExprId,
        site: Span,
    ) -> bool {
        for (path, independent) in paths {
            if !path.is_empty()
                && !independent
                && actual
                    .crossing_parts
                    .get(&path)
                    .is_some_and(|proof| proof != Sendability::Sendable)
            {
                let provided = self.span_of(actual.site.unwrap_or(expression));
                self.report(const { RuleId::of("MODE-4") }, site, "the current owned part has lost the evidence required by its sendable receiving type", |_| {
                    Explanation::default().provided(Fact::new(Cause::NotSendable, provided, &[]))
                        .required(Fact::new(Requirement::SendableType, site, &["the receiving type"])).into()
                });
                return false;
            }
        }
        true
    }
    fn required_part(&mut self, state: &State, part: TypeId, value: ExprId, aggregate: ExprId) {
        let Some(mut required) = self.t.get(part).map(|row| row.modes.clone()) else {
            return;
        };
        required.relation = Relation::environment();
        required.sendability = crossing_requirement(self.t.get(part).unwrap());
        // MODE-2: J types an inferred part by its operand's row, which is
        // evidence, not a contract: the part's current uniqueness is the
        // operand's, kept in `shared_parts`. A part typed by an expected type
        // is a contract.
        // Constructor parameters are receiving contracts even when their
        // row equals an operand's pre-flow row. Copying that operand may
        // already have ended its uniqueness without changing its row.
        let inferred = !matches!(
            self.tree.expressions[aggregate.0 as usize].kind,
            ExprKind::Call { .. }
        ) && self.ty(value) == Some(part);
        if inferred {
            required.uniqueness = Uniqueness::Aliased;
        }
        let actual = self.value(state, value);
        let span = self.tree.expressions[aggregate.0 as usize].origin.span;
        let satisfied = self.required(
            state,
            &actual,
            &required,
            value,
            span,
            Fact::new(Requirement::Part, span, &[]),
        );
        if satisfied && !inferred {
            self.required_shared_parts(&actual, part, value, span);
        }
    }
    /// An aggregate's part at `projection` holds `part`, the value of `child`.
    fn construction_part(
        &self,
        aggregate: &mut Value,
        projection: &Projection,
        child: ExprId,
        part: &Value,
    ) {
        if part.modes.uniqueness != Uniqueness::Unique
            && !self
                .property(child)
                .is_some_and(|p| p.plain || p.handle_root)
        {
            aggregate
                .shared_parts
                .insert(vec![projection.clone()], child);
        }
        if part.modes.uniqueness == Uniqueness::Unique
            || self
                .property(child)
                .is_some_and(|p| p.plain || p.handle_root)
        {
            aggregate.unique_parts.insert(vec![projection.clone()]);
        }
        for path in &part.unique_parts {
            let mut installed = vec![projection.clone()];
            installed.extend(path.iter().cloned());
            if installed.len() <= self.retention_depth {
                aggregate.unique_parts.insert(installed);
            }
        }
        aggregate
            .shared_parts
            .install(std::slice::from_ref(projection), &part.shared_parts);
    }
    /// MODE-2: a nested `unique` contract of the receiving type cannot hold a
    /// part that is currently aliased. J compares the rows; this is the part
    /// whose row claims more than its content holds.
    fn required_shared_parts(
        &mut self,
        actual: &Value,
        root: TypeId,
        expression: ExprId,
        site: Span,
    ) -> bool {
        if matches!(actual.usage, UseKind::Borrow | UseKind::Mut) {
            return true;
        }
        let row = self.ty(expression);
        let Some(&(_, cause, _)) = actual.shared_parts.0.iter().find(|(path, _, deep)| {
            self.unique_contract_on(root, path, *deep)
                && row.is_none_or(|row| self.unique_contract_on(row, path, *deep))
        }) else {
            return true;
        };
        let operand = self.span_of(cause);
        self.report(
            const { RuleId::of("MODE-2") },
            operand,
            "the receiving type cannot change this nested uniqueness contract",
            |_| {
                Explanation::default()
                    .provided(Fact::new(Cause::Aliased, operand, &[]))
                    .required(Fact::new(Requirement::Unique, site, &[]))
                    .into()
            },
        );
        false
    }
    /// Whether a row on `path` below `root` promises unique counted content.
    fn unique_contract_on(&self, root: TypeId, path: &[Projection], deep: bool) -> bool {
        if !deep {
            return self.shallow_unique_contract_on(root, path);
        }
        if let Some(parts) = self.unique_parts_cache.borrow().get(&root) {
            return parts
                .iter()
                .any(|parts| parts.starts_with(path) || path.starts_with(parts));
        }
        let parts = match owned_unique_parts(
            self.t,
            root,
            &self.body.resolution.generic_declarations,
            &mut |p| Ok(self.signatures.cached(p)),
        ) {
            Ok(parts) => parts,
            Err(error) => {
                *self.proof_error.borrow_mut() = Some(error);
                return false;
            }
        };
        let parts: Vec<Vec<_>> = parts
            .into_iter()
            .map(|parts| {
                parts
                    .into_iter()
                    .map(|p| match p {
                        OwnedTypePart::Field(f) => Projection::Field(f),
                        OwnedTypePart::VariantField { variant, field } => {
                            Projection::VariantField { variant, field }
                        }
                        OwnedTypePart::Elements => Projection::Elements,
                    })
                    .collect()
            })
            .collect();
        let required = parts
            .iter()
            .any(|parts| parts.starts_with(path) || path.starts_with(parts));
        self.unique_parts_cache.borrow_mut().insert(root, parts);
        required
    }

    fn shallow_unique_contract_on(&self, root: TypeId, path: &[Projection]) -> bool {
        let mut ty = root;
        for projection in path {
            let Some(row) = self.t.get(ty) else {
                return false;
            };
            let child = match (&row.kind, projection) {
                (TypeKind::Tuple(parts), Projection::Field(i)) => parts.get(*i as usize).copied(),
                (TypeKind::Box(part), Projection::Field(0)) => Some(*part),
                (TypeKind::Record(fields), Projection::Field(i)) => {
                    fields.get(*i as usize).map(|field| field.ty)
                }
                (TypeKind::Builtin { arguments, .. }, Projection::Elements) => {
                    match arguments.first() {
                        Some(hirsel_types::GenericArgument::Type(element)) => Some(*element),
                        _ => None,
                    }
                }
                _ => None,
            };
            let Some(child) = child else {
                return false;
            };
            ty = child;
            let unique = self
                .t
                .get(ty)
                .is_some_and(|row| row.modes.uniqueness == Uniqueness::Unique);
            if unique && !self.properties.get(ty).is_some_and(|p| p.plain || p.shared) {
                return true;
            }
        }
        false
    }

    /// CLO-1: only the rows that hold a closure local to its environment
    /// publish its local result. A declared slot, or another closure's
    /// capture, promises a global result that environment cannot give.
    fn environment_slot(&mut self, site: ExprId, context: Fact<Requirement>) {
        let span = self.span_of(site);
        self.report(
            const { RuleId::of("MODE-1") },
            span,
            "this closure's result is local to its environment",
            |_| {
                Explanation::default()
                    .provided(Fact::new(Cause::Environment, span, &[]))
                    .required(Fact::new(Requirement::FunctionSlot, context.span, &[]))
                    .into()
            },
        );
    }
    /// `context` says what imposes `required`: a parameter, an annotation,
    /// an aggregate part or the function result.
    /// Diagnose the counted operand that prevents an aggregate from being unique.
    /// Inspect only known constructions; an ordinary call's inputs do not say
    /// which value its result aliases. This runs only after a failed proof.
    fn aliased_source(&self, state: &State, expression: ExprId) -> ExprId {
        let mut pending = vec![expression];
        let mut seen = BTreeSet::new();
        while let Some(value) = pending.pop() {
            if !seen.insert(value) || self.property(value).is_some_and(|p| p.plain) {
                continue;
            }
            let actual = self.value(state, value);
            if actual.modes.uniqueness == Uniqueness::Unique {
                continue;
            }
            let children = match &self.tree.expressions[value.0 as usize].kind {
                ExprKind::Tuple(parts) | ExprKind::Sequence(parts) => parts.clone(),
                ExprKind::Unary {
                    op: UnaryOp::Box,
                    value,
                } => vec![*value],
                ExprKind::Block {
                    tail: Some(tail), ..
                } => std::iter::once(*tail)
                    .chain(
                        self.graph
                            .block_breaks
                            .get(&value)
                            .into_iter()
                            .flatten()
                            .copied(),
                    )
                    .collect(),
                ExprKind::Call { arguments, .. }
                    if self.body.calls.iter().any(|call| {
                        call.expression == value
                            && matches!(call.target, CallTarget::Constructor(_))
                    }) =>
                {
                    arguments.iter().map(|a| a.value).collect()
                }
                _ => return actual.site.unwrap_or(value),
            };
            pending.extend(children.into_iter().rev());
        }
        expression
    }

    fn required(
        &mut self,
        state: &State,
        actual: &Value,
        required: &Modes,
        expression: ExprId,
        span: Span,
        context: Fact<Requirement>,
    ) -> bool {
        if actual.invalid {
            // Recovery suppresses dependent mode requirements.
            return true;
        }
        let value_site = self.span_of(actual.site.unwrap_or(expression));
        if actual.environment_local() {
            self.environment_slot(expression, context);
        } else if self.arenas.global(&required.relation)
            && !self.arenas.global(&actual.modes.relation)
        {
            if actual.scoped() {
                self.report(
                    const { RuleId::of("CLO-1") },
                    span,
                    "this operation requires a global value",
                    |_| {
                        Explanation::default()
                            .provided(Fact::new(Cause::ScopedClosure, value_site, &[]))
                            .required(context)
                            .into()
                    },
                );
            } else {
                // The context names who requires it; the axis is locality.
                let context = match context.kind {
                    Requirement::Argument => Fact {
                        kind: Requirement::GlobalArgument,
                        ..context
                    },
                    Requirement::AnnotatedBinding => Fact {
                        kind: Requirement::AnnotatedGlobal,
                        ..context
                    },
                    _ => context,
                };
                self.report(
                    const { RuleId::of("MODE-1") },
                    span,
                    "this operation requires a global value",
                    |c| c.locality(state, actual, context),
                );
            }
        } else if required.uniqueness == Uniqueness::Unique
            && actual.modes.uniqueness != Uniqueness::Unique
            && !self.property(expression).is_some_and(|p| p.plain)
        {
            let source = self.aliased_source(state, expression);
            let copied_owner = actual.copy_site.map(|cause| self.span_of(cause));
            let cause = actual.uniqueness_site.unwrap_or(source);
            let cause_span = self.span_of(cause);
            let source_span = self.span_of(source);
            // A transfer is the counted field expression; retain its written
            // mode when the propagated uniqueness cause names its operand.
            let provided = if matches!(
                self.tree.expressions[source.0 as usize].kind,
                ExprKind::Unary {
                    op: UnaryOp::Take | UnaryOp::Copy,
                    ..
                }
            ) && source_span.start <= cause_span.start
                && source_span.end >= cause_span.end
            {
                source_span
            } else {
                cause_span
            };
            let primary = if span == value_site || span == cause_span {
                provided
            } else {
                span
            };
            if let Some(diagnostic) = self.report(
                const { RuleId::of("MODE-2") },
                primary,
                "this operation requires a unique value",
                |c| {
                    let mut pending: causes::Pending = Explanation::default()
                        .provided(Fact::new(Cause::Aliased, provided, &[]))
                        .required(Fact::new(Requirement::Unique, context.span, &[]))
                        .into();
                    pending.uniqueness = c.uniqueness_origin(expression);
                    if let Some(call) = c.owned_call_source(state, expression)
                        && let ExprKind::Call { callee, .. } =
                            c.tree.expressions[call.0 as usize].kind
                        && let Some(path) = c.callee_definition(callee)
                        && hirsel_types::unique_summary::same_module(path, c.own.definition.path())
                        && let Some(failure) = c
                            .signatures
                            .unique_failure(path, UniqueClaim::Result(UniqueResultRow::Root))
                    {
                        pending.explanation.provided.push(Fact::new(
                            Cause::UniquenessLoss,
                            failure.span,
                            &[
                                "the result",
                                &format!(
                                    "`{}` no longer returns unique content: {}",
                                    path.0.name, failure.message
                                ),
                            ],
                        ));
                    }
                    pending
                },
            ) {
                if let Some(copy_span) = copied_owner {
                    diagnostic.secondary.push(hirsel_diag::Label {
                        span: copy_span,
                        message: "this copy ended the owner's nested uniqueness".into(),
                    });
                    diagnostic.notes.push("A counted copy ends the owner's nested uniqueness proof; ending or dropping the alias does not restore it. Supply independently unique content. Deep copying requires copyable data, permitted allocation and compatible placement; a `take` repair requires the supplier to be already unique.".into());
                }
            }
        } else if required.multiplicity == Multiplicity::Many
            && actual.modes.multiplicity == Multiplicity::Once
            && self.property(expression).is_some_and(|p| p.function)
        {
            if let Some(d) = self.report(
                const { RuleId::of("MODE-3") },
                span,
                "a once function cannot satisfy a many function type",
                |_| {
                    Explanation::default()
                        .provided(Fact::new(Cause::Once, value_site, &[]))
                        .required(Fact::new(Requirement::Many, context.span, &[]))
                        .into()
                },
            ) {
                d.obligations.push(Obligation::Multiplicity);
            }
        } else if required.sendability == Sendability::Sendable
            && self.captured_sendability(expression, actual) != Sendability::Sendable
        {
            self.report(
                const { RuleId::of("MODE-4") },
                span,
                "current content is not proved sendable",
                |_| {
                    Explanation::default()
                        .provided(Fact::new(Cause::NotSendable, value_site, &[]))
                        .required(Fact::new(Requirement::Sendable, context.span, &[]))
                        .into()
                },
            );
        } else {
            return true;
        }
        false
    }
}

/// The modes a received value must have, and its binding's storage contract:
/// the written axes from the checked row, the others from the value.
fn annotation_modes(
    written: WrittenModes,
    declared: &Modes,
    actual: &Modes,
    crossing: bool,
) -> Modes {
    let mut required = actual.clone();
    if written.relation || written.function {
        required.relation = declared.relation.clone();
    } else if written.sendable {
        required.relation = Relation::global();
    }
    if written.unique {
        required.uniqueness = Uniqueness::Unique;
    }
    if written.once {
        required.multiplicity = Multiplicity::Once;
    } else if written.function {
        required.multiplicity = Multiplicity::Many;
    }
    // MODE-4: a write may store any value of the written callable/Iter type, so
    // the storage crosses tasks only as that type does.
    required.sendability = if written.sendable {
        Sendability::Sendable
    } else if written.function {
        Sendability::NotSendable
    } else if crossing {
        declared.sendability.clone()
    } else {
        Sendability::NotSendable
    };
    required
}

#[cfg(test)]
mod loan_tests {
    use super::*;

    #[test]
    fn retained_value_joins_match_the_original_witness_and_bookkeeping_rules() {
        // Exercise the non-idempotent first-join bookkeeping as well as
        // settled values. The reference deliberately enters make_mut first.
        let mut retained = 0;
        for bits in 0..256 {
            let mut value = Value::new(Modes::global());
            value.modes.uniqueness = if bits & 1 == 0 {
                Uniqueness::Unique
            } else {
                Uniqueness::Aliased
            };
            value.site = (bits & 2 != 0).then_some(ExprId(7));
            value.uniqueness_site = (bits & 4 != 0).then_some(ExprId(3));
            value.borrow_only = bits & 8 != 0;
            if bits & 16 != 0 {
                value.parked = Arc::new(BTreeSet::from([ExprId(9)]));
            }
            value.envelope = (bits & 32 != 0).then(|| BTreeSet::from([1, 3]));
            value.part_of = (bits & 64 != 0).then(|| (2, BTreeSet::from([4, 5])));
            value.set_alias(Some((1, Site::Expression(ExprId(8)))));
            value.read_within(2, &[0, 1], Site::Binding(3));
            value.bound_by(4, Site::Frame);
            if bits & 128 != 0 {
                let loans = value.content.whole.clone();
                value.install_content(vec![(vec![Projection::Field(1)], loans)]);
                value
                    .shared_parts
                    .insert(vec![Projection::Field(0)], ExprId(6));
            }
            let original = value.clone();
            let mut reference = value.clone();
            std::ops::DerefMut::deref_mut(&mut reference).join(&original);
            value.join(&original);
            assert_eq!(value, reference, "bits {bits}");
            retained += usize::from(std::rc::Rc::ptr_eq(&value.0, &original.0));
            // Distinct payloads must still enter the ordinary merge and
            // select the earliest witness, even when their summaries match.
            let mut different = original.clone();
            different.read_within(2, &[0, 2], Site::Binding(1));
            let mut expected = original.clone();
            std::ops::DerefMut::deref_mut(&mut expected).join(&different);
            let mut actual = original.clone();
            actual.join(&different);
            assert_eq!(actual, expected, "different bits {bits}");
        }
        assert!(retained > 0);
    }

    #[test]
    fn unchanged_value_joins_retain_payload_allocations_across_branch_snapshots() {
        let mut original = Value::new(Modes::global());
        original.envelope = Some(BTreeSet::from([1, 2, 3]));
        original.part_of = Some((1, BTreeSet::from([2, 3])));
        original.bound_by(2, Site::Frame);
        original.read_within(1, &[2, 3], Site::Binding(4));
        let mut before = 0;
        let mut after = 0;
        for _ in 0..128 {
            let mut reference = original.clone();
            std::ops::DerefMut::deref_mut(&mut reference).join(&original);
            before += usize::from(!std::rc::Rc::ptr_eq(&reference.0, &original.0));
            let mut value = original.clone();
            value.join(&original);
            after += usize::from(!std::rc::Rc::ptr_eq(&value.0, &original.0));
            assert_eq!(value, reference);
        }
        assert_eq!((before, after), (128, 0));
        eprintln!("unchanged branch joins: payload allocations {before} -> {after}");
    }

    #[test]
    fn successor_forwarding_moves_last_new_state_and_keeps_original_joins() {
        let mut output = State {
            slots: hirsel_types::rows::Rows::new(Slot::default(), 4),
            values: BTreeMap::from([(ExprId(3), Value::new(Modes::global()))]),
            iteration_loans: BTreeMap::new(),
            argument_loans: BTreeMap::new(),
            argument_conflicts: BTreeSet::from([ExprId(9)]),
        };
        output.slots[1].available = LIVE;
        output.slots[1].value.modes.uniqueness = Uniqueness::Unique;
        output.slots[1].value.bound_by(2, Site::Frame);
        output
            .values
            .get_mut(&ExprId(3))
            .unwrap()
            .bound_by(1, Site::Binding(2));
        let mut previous = output.clone();
        previous.slots[1].available = MOVED;
        previous.slots[1].value.share();
        previous.values.get_mut(&ExprId(3)).unwrap().share();
        let retained = output.clone();
        let rank = [3, 2, 1, 0];
        let routes: &[&[usize]] = &[
            &[],
            &[1],
            &[1, 2],
            &[1, 1],
            &[0],
            &[1, 0],
            &[0, 1],
            &[2, 1, 2],
            &[3, 2, 1],
            &[1, 2, 3, 1],
        ];
        let mut removable_clones = 0;
        for mask in 0..16u32 {
            for &next in routes {
                let mut inputs: Vec<_> = (0..4)
                    .map(|i| (mask & (1 << i) != 0).then(|| previous.clone()))
                    .collect();
                let initial = inputs.clone();
                let mut expected = inputs.clone();
                let mut reference_pending = BTreeSet::from([(99, 99)]);
                let mut reference_slots = SlotJoins::new(Slot::join);
                let mut expected_clones = 0;
                // Independent original algorithm: preserve successor order,
                // clone to initialize and join every already-reached target.
                for (index, &successor) in next.iter().enumerate() {
                    let changed = if let Some(input) = &mut expected[successor] {
                        input.join(&output, &mut reference_slots)
                    } else {
                        expected[successor] = Some(output.clone());
                        expected_clones += usize::from(index + 1 != next.len());
                        true
                    };
                    if changed {
                        reference_pending.insert((rank[successor], successor));
                    }
                }
                let mut pending = BTreeSet::from([(99, 99)]);
                let mut slots = SlotJoins::new(Slot::join);
                let before = FORWARD_STATE_CLONES.with(std::cell::Cell::get);
                forward_output(
                    output.clone(),
                    next,
                    &mut inputs,
                    &mut |successor| {
                        pending.insert((rank[successor], successor));
                    },
                    &mut slots,
                );
                let clones = FORWARD_STATE_CLONES.with(std::cell::Cell::get) - before;
                assert_eq!(inputs, expected, "mask {mask} route {next:?}");
                assert_eq!(pending, reference_pending);
                assert_eq!(output, retained);
                for value in initial.iter().flatten() {
                    assert_eq!(value, &previous);
                }
                removable_clones += clones.saturating_sub(expected_clones);
                // Branch snapshots must remain independent after publication.
                if let Some(input) = &mut inputs[1] {
                    input.slots[1].value.share();
                }
                assert_eq!(output, retained);
            }
        }
        assert_eq!(
            removable_clones, 0,
            "move the consumed output to a final uninitialized successor"
        );
    }

    #[test]
    fn value_snapshots_isolate_loans_and_keep_compact_branch_entries() {
        let mut value = Value::new(Modes::global());
        value.modes.uniqueness = Uniqueness::Unique;
        value.envelope = Some(BTreeSet::from([2, 5]));
        value.part_of = Some((4, BTreeSet::from([1, 8])));
        value.bound_by(3, Site::Frame);
        value.read_within(6, &[1, 2], Site::Binding(9));
        let base = BTreeMap::from([(1, value.clone()), (9, value)]);
        let mut left = base.clone();
        let mut right = base.clone();
        let l = left.get_mut(&1).unwrap();
        l.share();
        l.part_of.as_mut().unwrap().1.insert(11);
        l.envelope.as_mut().unwrap().insert(7);
        let r = right.get_mut(&1).unwrap();
        r.bound_by(3, Site::Binding(7));
        r.read_within(6, &[1, 3], Site::Binding(8));
        r.part_of.as_mut().unwrap().1.insert(12);

        assert_eq!(base[&1].modes.uniqueness, Uniqueness::Unique);
        assert_eq!(right[&1].modes.uniqueness, Uniqueness::Unique);
        assert_eq!(base[&1].part_of, Some((4, BTreeSet::from([1, 8]))));
        assert_eq!(base[&1].envelope, Some(BTreeSet::from([2, 5])));
        assert_eq!(base[&9], left[&9]);
        assert_eq!(base[&9], right[&9]);
        let right_before = right.clone();
        left.get_mut(&1).unwrap().join(&right[&1]);
        let joined = &left[&1];
        assert_eq!(joined.modes.uniqueness, Uniqueness::Aliased);
        assert_eq!(joined.part_of, Some((4, BTreeSet::from([1, 8, 11, 12]))));
        let content = joined.content.summary();
        assert_eq!(
            content.scopes().collect::<Vec<_>>(),
            vec![(Kind::Bound, 3, Site::Binding(7))]
        );
        let read = content
            .iter()
            .find(|loan| loan.kind == Kind::Read && loan.on == On::Binding(6))
            .unwrap();
        assert_eq!(read.within, vec![1]);
        assert_eq!(read.site, Site::Binding(8));
        assert_eq!(right, right_before);
        assert_eq!(
            base[&1].content.summary().scopes().collect::<Vec<_>>(),
            vec![(Kind::Bound, 3, Site::Frame)]
        );
        let fixed = left.clone();
        left.get_mut(&1).unwrap().join(&right[&1]);
        assert_eq!(left, fixed);
        // Branch-map clones must retain only a payload handle per temporary.
        assert!(std::mem::size_of::<Value>() <= 2 * std::mem::size_of::<usize>());
    }

    #[test]
    fn slot_snapshots_isolate_branch_updates_and_join_the_same_flow_facts() {
        let mut base = hirsel_types::rows::Rows::new(Slot::default(), 16);
        for index in [1, 9] {
            base[index].available = LIVE;
            base[index].value.modes.uniqueness = Uniqueness::Unique;
        }
        base[1].value.bound_by(3, Site::Frame);
        let mut left = base.clone();
        let mut right = base.clone();
        left[1].value.share();
        left[9].available = MOVED;
        right[1].value.bound_by(3, Site::Binding(7));

        assert_eq!(base[1].value.modes.uniqueness, Uniqueness::Unique);
        assert_eq!(base[9].available, LIVE);
        assert_eq!(right[1].value.modes.uniqueness, Uniqueness::Unique);
        // Changing a neighboring row does not clone this binding's payload.
        assert!(std::rc::Rc::ptr_eq(&base[2].0, &left[2].0));
        assert!(std::rc::Rc::ptr_eq(&base[10].0, &left[10].0));

        let mut joins = SlotJoins::new(Slot::join);
        assert!(left.join_cached(&right, &mut joins));
        assert_eq!(left[1].value.modes.uniqueness, Uniqueness::Aliased);
        assert_eq!(left[9].available, LIVE | MOVED);
        assert_eq!(
            left[1].value.content.summary().scopes().collect::<Vec<_>>(),
            vec![(Kind::Bound, 3, Site::Binding(7))]
        );
        assert_eq!(base[1].value.modes.uniqueness, Uniqueness::Unique);
        assert_eq!(right[9].available, LIVE);
        assert!(!left.join_cached(&right, &mut joins));
    }

    #[test]
    fn a_join_keeps_the_earliest_issue_site_of_each_loan() {
        let mut a = Value::new(Modes::global());
        a.bound_by(3, Site::Frame);
        let mut b = Value::new(Modes::global());
        b.bound_by(3, Site::Binding(7));
        a.join(&b);
        let content = a.content.summary();
        assert_eq!(
            content.scopes().collect::<Vec<_>>(),
            vec![(Kind::Bound, 3, Site::Binding(7))]
        );
    }
}
