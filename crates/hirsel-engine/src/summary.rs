//! What a checked definition tells the program-level passes about its calls
//! (design r5 §1(c) step 2): CT-5 instance selection, staged-constant
//! selection and GEN-5 read a `DefinitionSummary` and its `ParameterGrowth`,
//! never a TypedBody, so the body can be dropped once it is published.
//!
//! The checker's output is the only producer ([`summarize`]); the consumers
//! and the persisted form belong to the engine's scheduling.
//!
//! Frames are the lexical scopes that select calls: the item, each local
//! function and closure the checker planned, and each comptime region. A
//! frame's parent is the smallest frame whose span strictly encloses it;
//! frames are numbered in source order, so a parent precedes its children.
//!
//! Call arguments are **open**: they may name the owner's generic parameters
//! (its item binder and its local functions'). An S1 selection item closes
//! them with the bindings it carries. Their TypeIds are session interner rows;
//! lexical references inside them are qualified by their owner, so they need
//! no outer context.
use crate::*;
use hirsel_span::Span;
use hirsel_syntax::{DefId, ExprId, ExprKind, LexicalOwner};
use hirsel_types::{
    BinderRef, CallTarget, GenericArgument, NameSite, ResolvedName, SharedSlice, SiteRef, Size,
    SizeParamId, SizeParameterRef, TypeId, TypeInterner, TypeKind, TypeParamId, TypeParameterRef,
};
use std::collections::{BTreeSet, HashSet};
mod facet;
pub(crate) use facet::SettledSummary;
pub use facet::SummaryView;

/// A generic parameter named by its declaring binder (contract rev 3).
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
pub enum GenericParameterRef {
    Type(TypeParameterRef),
    Size(SizeParameterRef),
}
#[derive(
    serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
pub enum FrameKind {
    Item,
    LocalFunction,
    Closure,
    ComptimeRegion,
}
/// How a call's instance is selected.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CallClass {
    /// A written call outside every comptime region: selected when its frame is.
    Runtime,
    /// Any call inside a comptime region: executing the region selects it, so
    /// a branch that never runs selects nothing.
    ComptimeExecuted,
    /// An instance the checker planned with no written call at its site (a
    /// named generic function value, a fact helper, a loop protocol call),
    /// outside every comptime region: selected with its frame.
    StaticSelection,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub site: NameSite,
    pub parent: Option<u32>,
    pub kind: FrameKind,
    /// The frame declares generic parameters of its own: a selection enters it
    /// only through a call with its actuals, never by enclosing it.
    pub generic: bool,
    /// The enclosing frames' size parameters this frame (or a frame inside
    /// it) names in a call argument or a staged input, each bound to itself:
    /// a selection entering the frame carries exactly these from its parent.
    pub size_bindings: Box<[(SizeParameterRef, Size)]>,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct SummaryCall {
    pub callee: DefId,
    /// The callee's generic binder: its item, or for a call of a local
    /// function by name, that function's site in the owner.
    pub binder: SiteRef,
    pub arguments: Box<[GenericArgument]>, /* open, owner-relative */
    pub owning_frame: u32,
    /// The callee's frame when it is a local function of this owner.
    pub target_frame: Option<u32>,
    pub class: CallClass,
    pub site: Span,
}
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct DefinitionSummary {
    pub owner: DefId,
    pub frames: SharedSlice<Frame>,
    pub calls: SharedSlice<SummaryCall>,
    /// Each comptime region that reads enclosing sizes (a hidden size-generic
    /// constant), with those inputs in its constant's parameter order. The
    /// region is the `ComptimeRegion` frame with the same site.
    pub staged: SharedSlice<(NameSite, Box<[SizeParameterRef]>)>,
}
/// One `ParameterGrowth` edge: from an owner parameter to a callee parameter.
pub type GrowthEdge = (
    (DefId, GenericParameterRef),
    (DefId, GenericParameterRef),
    bool,
    Span,
);
/// GEN-5's input: each owner parameter that flows into a callee parameter,
/// whether it flows nested (strict growth), and the call site.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, Eq, PartialEq)]
pub struct ParameterGrowth {
    pub edges: SharedSlice<GrowthEdge>,
}

/// GEN-5's checked input, before modes add planned calls. Retained separately
/// from the final summary so releasing bodies never changes that input.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CheckedSummary {
    pub summary: DefinitionSummary,
    pub growth: ParameterGrowth,
}

/// The compact published input of CT-5, staged selection and GEN-5. Its reads
/// share the memo's allocation; retaining it never retains a checked body.
/// Durable encodings use the same canonical type table as the other outputs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PublishedSummary {
    /// Query identity; generated instances can retain another lexical owner.
    pub definition: DefId,
    pub summary: DefinitionSummary,
    pub growth: ParameterGrowth,
    /// Source-only compact equations. V reconstructs its own executable graph.
    #[serde(default)]
    pub storage: Option<std::sync::Arc<hirsel_types::storage::graph::StorageDefinition>>,
    #[serde(default)]
    pub checked: Option<std::sync::Arc<CheckedSummary>>,
    pub reads: std::sync::Arc<crate::memo::Reads>,
    /// The owner's body and package-manifest key, as for a body memo.
    pub text: hirsel_syntax::ContentHash,
}

impl PublishedSummary {
    pub(crate) fn share_checked_storage(&mut self) {
        if let Some(checked) = &self.checked {
            if self.summary.frames == checked.summary.frames {
                self.summary.frames = checked.summary.frames.clone();
            }
            if self.summary.calls == checked.summary.calls {
                self.summary.calls = checked.summary.calls.clone();
            }
            if self.summary.staged == checked.summary.staged {
                self.summary.staged = checked.summary.staged.clone();
            }
            if self.growth.edges == checked.growth.edges {
                self.growth.edges = checked.growth.edges.clone();
            }
        }
    }
}

impl ParameterGrowth {
    /// Sites of growing edges that lie on a cycle, in source order.
    pub fn growing_sites(&self) -> Vec<Span> {
        growing_sites(&self.edges)
    }
}
impl DefinitionSummary {
    /// The owner's item binder, as callers name it.
    pub fn item_binder(&self) -> SiteRef {
        LexicalOwner::first(self.owner.path()).site(NameSite::Item)
    }
}

/// The summary and growth edges of one checked definition.
pub fn summarize(t: &TypeInterner, typed: &TypedBody) -> (DefinitionSummary, ParameterGrowth) {
    let syntax = &*typed.syntax;
    let checked = &typed.checked;
    let tree = &syntax.tree;
    let lexical = syntax.owner();
    let span = |site: NameSite| source_site(syntax, site);
    let comptime = |expression: ExprId| {
        matches!(
            tree.expressions[expression.0 as usize].kind,
            ExprKind::Comptime(_)
        )
    };

    let generic = |site: NameSite| {
        checked
            .resolution
            .generics
            .iter()
            .any(|generic| generic.owner == site)
    };

    // Frames in source order: outer before inner, ties by site.
    let mut sites: BTreeSet<(NameSite, FrameKind)> = BTreeSet::new();
    for plan in &typed.plans.functions {
        let kind = match plan.owner {
            NameSite::Statement(_) => FrameKind::LocalFunction,
            _ => FrameKind::Closure,
        };
        sites.insert((plan.owner, kind));
    }
    for row in &checked.expressions {
        if comptime(row.expression) {
            sites.insert((
                NameSite::Expression(row.expression),
                FrameKind::ComptimeRegion,
            ));
        }
    }
    let mut ordered: Vec<_> = sites
        .into_iter()
        .filter(|(site, _)| *site != NameSite::Item)
        .map(|(site, kind)| (span(site), site, kind))
        .collect();
    ordered.sort_by_key(|(span, site, _)| (span.start, std::cmp::Reverse(span.end), *site));
    let item = span(NameSite::Item);
    let mut spans = vec![item];
    let mut frames = vec![Frame {
        site: NameSite::Item,
        parent: None,
        kind: FrameKind::Item,
        generic: generic(NameSite::Item),
        size_bindings: Box::new([]),
    }];
    for (at, site, kind) in ordered {
        let parent = innermost(&spans, at);
        spans.push(at);
        frames.push(Frame {
            site,
            parent: Some(parent),
            kind,
            generic: generic(site),
            size_bindings: Box::new([]),
        });
    }
    let frame_of = |site: NameSite| {
        frames
            .iter()
            .position(|frame| frame.site == site)
            .map(|i| i as u32)
    };
    let staged_frame = |mut frame: u32| loop {
        if frames[frame as usize].kind == FrameKind::ComptimeRegion {
            return true;
        }
        match frames[frame as usize].parent {
            Some(parent) => frame = parent,
            None => return false,
        }
    };

    // Written calls first, then the checker's planned instantiations; a
    // planned instance at a written call's own site adds nothing.
    let mut calls: Vec<SummaryCall> = vec![];
    let mut written: BTreeSet<(ExprId, DefId)> = BTreeSet::new();
    let mut push = |calls: &mut Vec<SummaryCall>,
                    expression: ExprId,
                    callee: DefId,
                    binder: SiteRef,
                    arguments: &[GenericArgument],
                    target_frame: Option<u32>,
                    planned: bool| {
        let site = span(NameSite::Expression(expression));
        let owning_frame = innermost(&spans, site);
        let class = if staged_frame(owning_frame) {
            CallClass::ComptimeExecuted
        } else if planned && !written.contains(&(expression, callee)) {
            CallClass::StaticSelection
        } else {
            CallClass::Runtime
        };
        if !planned {
            written.insert((expression, callee));
        }
        let call = SummaryCall {
            callee,
            binder,
            arguments: arguments.into(),
            owning_frame,
            target_frame,
            class,
            site,
        };
        if !calls.iter().any(|known| same_call(known, &call)) {
            calls.push(call);
        }
    };
    for call in &checked.calls {
        match &call.target {
            CallTarget::Definition(callee) => push(
                &mut calls,
                call.expression,
                DefId::intern(callee),
                LexicalOwner::first(callee).site(NameSite::Item),
                &call.generic_arguments,
                None,
                false,
            ),
            CallTarget::Value(value) => {
                let Some(binder) = local_binder(checked, &frame_of, *value) else {
                    continue;
                };
                push(
                    &mut calls,
                    call.expression,
                    lexical.def,
                    lexical.site(binder),
                    &call.generic_arguments,
                    frame_of(binder),
                    false,
                );
            }
            CallTarget::Constructor(_) => (),
        }
    }
    for instance in &checked.instantiations {
        push(
            &mut calls,
            instance.expression,
            DefId::intern(&instance.callee),
            LexicalOwner::first(&instance.callee).site(NameSite::Item),
            &instance.arguments,
            None,
            true,
        );
    }

    let staged: Vec<(NameSite, Box<[SizeParameterRef]>)> = checked
        .expressions
        .iter()
        .filter(|row| comptime(row.expression))
        .filter_map(|row| {
            let inputs =
                hirsel_check_protocol::body::comptime_inputs(t, syntax, checked, row.expression);
            (!inputs.is_empty()).then(|| (NameSite::Expression(row.expression), inputs.into()))
        })
        .collect();

    // A frame captures the enclosing size parameters its own or an inner
    // frame's calls and staged inputs name.
    let mut captured: Vec<BTreeSet<SizeParameterRef>> = vec![BTreeSet::new(); frames.len()];
    let mut capture = |frame: u32, parameter: SizeParameterRef| {
        let BinderRef::Body { scope, .. } = parameter else {
            return;
        };
        if scope.owner != lexical {
            return;
        }
        let Some(declaring) = frame_of(scope.site) else {
            return;
        };
        let mut at = frame;
        while at != declaring {
            captured[at as usize].insert(parameter);
            match frames[at as usize].parent {
                Some(parent) => at = parent,
                None => break,
            }
        }
    };
    for call in &calls {
        for argument in call.arguments.iter() {
            for parameter in size_parameters(t, argument) {
                capture(call.owning_frame, parameter);
            }
        }
    }
    for (site, inputs) in &staged {
        let frame = frame_of(*site).expect("staged region frame");
        for parameter in inputs.iter() {
            capture(frame, *parameter);
        }
    }
    for (frame, captured) in frames.iter_mut().zip(captured) {
        frame.size_bindings = captured
            .into_iter()
            .map(|parameter| (parameter, Size::Parameter(parameter)))
            .collect();
    }

    let mut edges = vec![];
    for instance in &checked.instantiations {
        let callee = DefId::intern(&instance.callee);
        let binder = LexicalOwner::first(&instance.callee).site(NameSite::Item);
        let site = span(NameSite::Expression(instance.expression));
        for (ordinal, argument) in instance.arguments.iter().enumerate() {
            let to = match argument {
                GenericArgument::Type(_) => GenericParameterRef::Type(TypeParameterRef::body(
                    binder,
                    TypeParamId(ordinal as u32),
                )),
                GenericArgument::Size(_) => GenericParameterRef::Size(SizeParameterRef::body(
                    binder,
                    SizeParamId(ordinal as u32),
                )),
            };
            for (parameter, grows) in parameters(t, argument, None) {
                let edge: GrowthEdge = ((lexical.def, parameter), (callee, to), grows, site);
                if !edges.contains(&edge) {
                    edges.push(edge);
                }
            }
        }
    }

    (
        DefinitionSummary {
            owner: lexical.def,
            frames: frames.into(),
            calls: calls.into(),
            staged: staged.into(),
        },
        ParameterGrowth {
            edges: edges.into(),
        },
    )
}

/// GEN-5 over a growth graph: the sites of growing edges that lie on a
/// parameter cycle, in source order. Depends only on the edge set.
pub fn growing_sites(edges: &[GrowthEdge]) -> Vec<Span> {
    use std::collections::BTreeMap;
    let mut vertices = BTreeMap::new();
    for (from, to, _, _) in edges {
        for vertex in [from, to] {
            let next = vertices.len();
            vertices.entry(vertex).or_insert(next);
        }
    }
    let mut graph = vec![Vec::new(); vertices.len()];
    for (from, to, _, _) in edges {
        graph[vertices[from]].push(vertices[to]);
    }
    // An edge lies on a cycle exactly when its endpoints share an SCC.
    // Reuse the iterative Tarjan walk, including targets with no outgoing edges.
    let mut component = vec![0; vertices.len()];
    for (id, members) in hirsel_graph::components(&graph).into_iter().enumerate() {
        for vertex in members {
            component[vertex] = id;
        }
    }
    let mut sites = BTreeMap::new();
    for (from, to, _, site) in edges.iter().filter(|edge| edge.2) {
        if component[vertices[from]] == component[vertices[to]] {
            sites.insert((site.anchor, site.start, site.end), *site);
        }
    }
    sites.into_values().collect()
}

/// The innermost frame whose span contains `at`; frames are in source order,
/// so the last containing one is the innermost.
fn innermost(spans: &[Span], at: Span) -> u32 {
    spans
        .iter()
        .rposition(|span| span.start <= at.start && span.end >= at.end)
        .unwrap_or(0) as u32
}

/// The binder of a call through a value: a local function named by the
/// callee expression, or a closure frame written at the callee itself.
fn local_binder(
    checked: &hirsel_types::CheckedBody,
    frame_of: &impl Fn(NameSite) -> Option<u32>,
    value: ExprId,
) -> Option<NameSite> {
    let local = checked
        .resolution
        .expressions
        .iter()
        .find_map(|(id, name)| match name {
            ResolvedName::Local(local) if *id == value => Some(*local),
            _ => None,
        });
    local
        .and_then(|local| {
            checked
                .resolution
                .local_functions
                .iter()
                .find(|(_, id)| *id == local)
                .map(|(statement, _)| NameSite::Statement(*statement))
        })
        .or_else(|| {
            let site = NameSite::Expression(value);
            frame_of(site).is_some().then_some(site)
        })
}

fn same_call(a: &SummaryCall, b: &SummaryCall) -> bool {
    a.site == b.site
        && a.callee == b.callee
        && a.binder == b.binder
        && a.owning_frame == b.owning_frame
        && a.arguments == b.arguments
}

pub(crate) fn source_site(syntax: &hirsel_syntax::ItemSyntax, site: NameSite) -> Span {
    let tree = &syntax.tree;
    match site {
        NameSite::Item => syntax.item.origin.span,
        NameSite::Statement(id) => tree.statements[id.0 as usize].origin.span,
        NameSite::Expression(id) => tree.expressions[id.0 as usize].origin.span,
        NameSite::Pattern(id) => tree.patterns[id.0 as usize].origin.span,
        NameSite::Type(id) => tree.types[id.0 as usize].origin.span,
    }
}

/// Every size parameter an argument names, at any depth of its type.
fn size_parameters(t: &TypeInterner, argument: &GenericArgument) -> BTreeSet<SizeParameterRef> {
    parameters(t, argument, None)
        .into_iter()
        .filter_map(|(parameter, _)| match parameter {
            GenericParameterRef::Size(parameter) => Some(parameter),
            GenericParameterRef::Type(_) => None,
        })
        .collect()
}

/// Owner parameters inside one argument, each with whether it is nested
/// (strict growth). A callable type introduces a binder, so `Bound` references
/// name the owner only at the owner's own depth.
pub(crate) fn parameters(
    types: &TypeInterner,
    argument: &GenericArgument,
    owner_binder: Option<SiteRef>,
) -> Vec<(GenericParameterRef, bool)> {
    let mut result = BTreeSet::new();
    let size = |size: &Size, depth: u32, nested: bool, result: &mut BTreeSet<_>| {
        if let Size::Parameter(parameter) = size
            && let Some(parameter) = owned(*parameter, depth, owner_binder)
        {
            result.insert((GenericParameterRef::Size(parameter), nested));
        }
    };
    let root = match argument {
        GenericArgument::Type(ty) => *ty,
        GenericArgument::Size(s) => {
            size(s, 0, false, &mut result);
            return result.into_iter().collect();
        }
    };
    let mut todo: Vec<(TypeId, bool, u32)> = vec![(root, false, 0)];
    let mut seen = HashSet::new();
    while let Some((ty, nested, depth)) = todo.pop() {
        if !seen.insert((ty, nested, depth)) {
            continue;
        }
        match &types.get(ty).expect("instance argument type").kind {
            TypeKind::Parameter(parameter) => {
                if let Some(parameter) = owned(*parameter, depth, owner_binder) {
                    result.insert((GenericParameterRef::Type(parameter), nested));
                }
            }
            TypeKind::Tuple(ts) => todo.extend(ts.iter().map(|t| (*t, true, depth))),
            TypeKind::Record(fs) => todo.extend(fs.iter().map(|f| (f.ty, true, depth))),
            TypeKind::Variant(vs) => todo.extend(
                vs.iter()
                    .flat_map(|v| v.fields.iter().map(move |(_, t)| (*t, true, depth))),
            ),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                for argument in arguments.iter() {
                    match argument {
                        GenericArgument::Type(t) => todo.push((*t, true, depth)),
                        GenericArgument::Size(s) => size(s, depth, true, &mut result),
                    }
                }
            }
            TypeKind::Box(t) => todo.push((*t, true, depth)),
            TypeKind::Function(f) => {
                todo.extend(f.parameters.iter().map(|p| (p.ty, true, depth + 1)));
                todo.push((f.result, true, depth + 1));
            }
            _ => (),
        }
    }
    result.into_iter().collect()
}
/// The owner's view of a reference: its own body binders, or its signature
/// binder at the owner's depth when the store is signature-owned.
fn owned<P: Copy>(
    reference: BinderRef<P>,
    depth: u32,
    owner_binder: Option<SiteRef>,
) -> Option<BinderRef<P>> {
    match reference {
        BinderRef::Body { .. } => Some(reference),
        BinderRef::Bound {
            depth: bound,
            parameter,
        } if bound == depth => owner_binder.map(|scope| BinderRef::Body { scope, parameter }),
        BinderRef::Bound { .. } => None,
    }
}
