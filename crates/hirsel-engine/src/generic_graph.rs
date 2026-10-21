//! GEN-5 is a program check, never a dependency on another definition's body
//! inside a cached definition query. Every edge's arguments are session
//! interner rows and name the callee's lexical binder until collected. A strict edge on a
//! parameter cycle witnesses unbounded growth; resetting a parameter breaks
//! that cycle. Size parameters are nodes too: a bare size argument is stable,
//! and one nested in a type argument grows that type.
//!
//! The source verdict reads each definition's growth edges, summarized from
//! its checked written and planned calls (closure bodies included). The
//! native graph adds what the selected TIR instantiates without a written
//! call: finalizers and nominal representations reached from dropped values,
//! closure environments and generic arguments, and the nominals of
//! materialized values. Whether those implicit instances belong to the source
//! rule is spec #87; until it rules, only native import rejects their growth.
use crate::summary::parameters;
use crate::*;
use hirsel_span::Span;
use hirsel_syntax::{DefId, ExprId};
use hirsel_tir::{Callee, CleanupAction, InstructionKind, Materialized, Rvalue, Terminator};
use hirsel_types::{
    GenericArgument, NameSite, SiteRef, SizeParamId, SizeParameterRef, TypeId, TypeInterner,
    TypeKind, TypeParamId, TypeParameterRef, UseKind,
};
use std::collections::{BTreeMap, HashSet};

/// Why an owner's instance requires the callee's instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum InstanceKind {
    /// A written or checker-planned call, in any body of the owner.
    Call,
    /// A named callable value (spec #42 forbids generic ones in source).
    Reference,
    /// A nominal reached from a dropped value, a closure environment or a
    /// generic argument the instance may drop: its finalizer and representation.
    Drop,
    /// A nominal reached from the owner's own private representation.
    Representation,
    /// A nominal reached from a value materialized into the owner.
    Materialized,
}
/// One instance requirement. `arguments` are session interner rows;
/// argument `i` binds `BinderRef::Body { scope: binder, parameter: i }`
/// of `callee`. Free `Body` references in the arguments name the owner's
/// parameters; when `owner_binder` is set (a signature-owned root), so do
/// `Bound` references at the owner's own depth.
#[derive(Clone, Debug)]
pub struct InstanceEdge {
    pub owner: DefPath,
    pub owner_binder: Option<SiteRef>,
    pub site: Span,
    pub kind: InstanceKind,
    pub callee: DefPath,
    pub binder: SiteRef,
    pub arguments: Vec<GenericArgument>,
}
type Node = (DefPath, GenericParameterRef);
struct ParameterEdge {
    from: Node,
    to: Node,
    grows: bool,
    site: Span,
}
impl InstanceEdge {
    /// Same requirement, comparing type arguments semantically in `t`.
    pub fn same(&self, other: &Self, t: &TypeInterner) -> bool {
        self.owner == other.owner
            && self.owner_binder == other.owner_binder
            && self.site == other.site
            && self.kind == other.kind
            && self.callee == other.callee
            && self.binder == other.binder
            && self.arguments.len() == other.arguments.len()
            && self
                .arguments
                .iter()
                .zip(&other.arguments)
                .all(|pair| match pair {
                    (GenericArgument::Type(a), GenericArgument::Type(b)) => t.semantic_eq(*a, *b),
                    (GenericArgument::Size(a), GenericArgument::Size(b)) => a == b,
                    _ => false,
                })
    }
    fn parameter_edges(&self, t: &TypeInterner) -> Vec<ParameterEdge> {
        let mut edges = vec![];
        for (ordinal, argument) in self.arguments.iter().enumerate() {
            let to = match argument {
                GenericArgument::Type(_) => GenericParameterRef::Type(TypeParameterRef::body(
                    self.binder,
                    TypeParamId(ordinal as u32),
                )),
                GenericArgument::Size(_) => GenericParameterRef::Size(SizeParameterRef::body(
                    self.binder,
                    SizeParamId(ordinal as u32),
                )),
            };
            for (parameter, grows) in parameters(t, argument, self.owner_binder) {
                edges.push(ParameterEdge {
                    from: (self.owner.clone(), parameter),
                    to: (self.callee.clone(), to),
                    grows,
                    site: self.site,
                });
            }
        }
        edges
    }
}
/// The instance requirements of a program, in collection order. The verdict
/// depends only on the edge set, never on the order units contributed them.
#[derive(Clone, Debug, Default)]
pub struct InstanceGraph {
    pub edges: Vec<InstanceEdge>,
}
impl InstanceGraph {
    /// Sites of growing edges that lie on a cycle, in source order.
    pub fn growing_sites(&self, t: &TypeInterner) -> Vec<Span> {
        let edges: Vec<_> = self
            .edges
            .iter()
            .flat_map(|edge| edge.parameter_edges(t))
            .map(|edge| {
                (
                    (DefId::intern(&edge.from.0), edge.from.1),
                    (DefId::intern(&edge.to.0), edge.to.1),
                    edge.grows,
                    edge.site,
                )
            })
            .collect();
        crate::summary::growing_sites(&edges)
    }
    /// Same edges in the same order.
    pub fn same(&self, other: &Self, t: &TypeInterner) -> bool {
        self.edges.len() == other.edges.len()
            && self
                .edges
                .iter()
                .zip(&other.edges)
                .all(|(a, b)| a.same(b, t))
    }
    /// What executing the selected snapshot instantiates, read only from the
    /// snapshot's own definitions, owners, representations and values.
    pub fn native(input: &NativeInput) -> Result<Self, QueryError> {
        let mut collector = Collector {
            input,
            edges: vec![],
            store_start: 0,
        };
        for (path, definition) in &input.definitions {
            collector.definition(path, definition)?;
        }
        for (path, record) in &input.representations {
            let (Some(record), Some(owner)) = (record, input.source_owners.get(path)) else {
                continue;
            };
            let site = owner.syntax.item.origin.span;
            collector.store_start = collector.edges.len();
            let edge = Edge {
                owner: path,
                owner_binder: Some(owner.syntax.owner().site(NameSite::Item)),
                site,
                kind: InstanceKind::Representation,
            };
            collector.nominals(&edge, &[record.private_shape]);
        }
        for (path, value) in &input.constants {
            let site = input
                .source_owners
                .get(path)
                .map(|owner| owner.syntax.item.origin.span);
            collector.materialized(path, site, value);
        }
        for (path, values) in &input.materialized {
            let owner = input.source_owners.get(path);
            for (expression, value) in values {
                let site = owner.and_then(|owner| expression_site(owner, *expression));
                collector.materialized(path, site, value);
            }
        }
        Ok(Self {
            edges: collector.edges,
        })
    }
}
fn expression_site(owner: &NativeSource, expression: ExprId) -> Option<Span> {
    let expressions = &owner.syntax.tree.expressions;
    expressions
        .get(expression.0 as usize)
        .map(|expression| expression.origin.span)
}
struct Edge<'a> {
    owner: &'a DefPath,
    owner_binder: Option<SiteRef>,
    site: Span,
    kind: InstanceKind,
}
struct Collector<'a> {
    input: &'a NativeInput,
    edges: Vec<InstanceEdge>,
    /// First edge of the current owner; cleanup chains repeat drops.
    store_start: usize,
}
impl Collector<'_> {
    /// Generic bodies in the selected program; library operations are runtime code.
    fn binder(&self, callee: &DefPath) -> Option<SiteRef> {
        self.input
            .source_owners
            .get(callee)
            .map(|owner| owner.syntax.owner().site(NameSite::Item))
    }
    fn push(&mut self, edge: &Edge<'_>, callee: &DefPath, arguments: &[GenericArgument]) {
        let Some(binder) = self.binder(callee) else {
            return;
        };
        if self.edges[self.store_start..].iter().any(|known| {
            known.site == edge.site
                && known.kind == edge.kind
                && known.callee == *callee
                && known.arguments == arguments
        }) {
            return;
        }
        self.edges.push(InstanceEdge {
            owner: edge.owner.clone(),
            owner_binder: edge.owner_binder,
            site: edge.site,
            kind: edge.kind,
            callee: callee.clone(),
            binder,
            arguments: arguments.to_vec(),
        });
    }
    /// Every generic nominal structurally inside `roots`, including those in
    /// another nominal's arguments: a nominal instance may drop its arguments.
    fn nominals(&mut self, edge: &Edge<'_>, roots: &[TypeId]) {
        let mut todo = roots.to_vec();
        let types = self.input.types.clone();
        let mut seen = HashSet::new();
        while let Some(ty) = todo.pop() {
            if !seen.insert(ty) {
                continue;
            }
            let Some(row) = types.get(ty) else {
                continue;
            };
            match &row.kind {
                TypeKind::Named {
                    definition,
                    arguments,
                } => {
                    if !arguments.is_empty() {
                        self.push(edge, definition.path(), arguments);
                    }
                    todo.extend(type_arguments(arguments));
                }
                TypeKind::Builtin { arguments, .. } => todo.extend(type_arguments(arguments)),
                TypeKind::Tuple(parts) => todo.extend(parts.iter().copied()),
                TypeKind::Record(fields) => todo.extend(fields.iter().map(|field| field.ty)),
                TypeKind::Variant(variants) => todo.extend(
                    variants
                        .iter()
                        .flat_map(|variant| variant.fields.iter().map(|(_, ty)| *ty)),
                ),
                TypeKind::Box(inner) => todo.push(*inner),
                _ => (),
            }
        }
    }
    fn definition(
        &mut self,
        path: &DefPath,
        definition: &hirsel_tir::Definition,
    ) -> Result<(), QueryError> {
        self.store_start = self.edges.len();
        for body in &definition.bodies {
            let edge = |site, kind| Edge {
                owner: path,
                owner_binder: None,
                site,
                kind,
            };
            let place = |place: hirsel_tir::PlaceId| {
                let place = &body.places[place.0 as usize];
                (place.ty, body.locals[place.root.0 as usize].span)
            };
            for block in &body.blocks {
                for instruction in &block.instructions {
                    match &instruction.kind {
                        InstructionKind::Drop { place: dropped, .. } => {
                            let (ty, site) = place(*dropped);
                            self.nominals(&edge(site, InstanceKind::Drop), &[ty]);
                        }
                        InstructionKind::Assign { value, .. } => {
                            match &body.rvalues[value.0 as usize] {
                                Rvalue::FunctionReference {
                                    definition: callee,
                                    arguments,
                                } if !arguments.is_empty() => {
                                    let mut reference =
                                        edge(instruction.span, InstanceKind::Reference);
                                    self.push(&reference, callee, arguments);
                                    reference.kind = InstanceKind::Drop;
                                    self.nominals(
                                        &reference,
                                        &type_arguments(arguments).collect::<Vec<_>>(),
                                    );
                                }
                                Rvalue::Closure { captures, .. } => {
                                    let owned: Vec<_> = captures
                                        .iter()
                                        .filter(|capture| capture.usage != UseKind::Borrow)
                                        .map(|capture| place(capture.place).0)
                                        .collect();
                                    self.nominals(
                                        &edge(instruction.span, InstanceKind::Drop),
                                        &owned,
                                    );
                                }
                                _ => (),
                            }
                        }
                        _ => (),
                    }
                }
                if let Terminator::Call { callee, .. } = &block.terminator {
                    match callee {
                        Callee::Definition {
                            path: callee,
                            arguments,
                        } if !arguments.is_empty() => {
                            let mut call = edge(block.span, InstanceKind::Call);
                            self.push(&call, callee, arguments);
                            call.kind = InstanceKind::Drop;
                            self.nominals(&call, &type_arguments(arguments).collect::<Vec<_>>());
                        }
                        // A call with generic arguments through a value is a
                        // generic nested function called by name (spec #42:
                        // such a function is a value only as a direct callee).
                        // Its instance is the owner's own closure body at the
                        // owner's and the call's actuals, which native
                        // expansion resolves from the body; it requires no
                        // other definition.
                        _ => (),
                    }
                }
            }
            for node in &body.cleanup_nodes {
                if let CleanupAction::Drop { place: dropped, .. } = node.action {
                    let (ty, site) = place(dropped);
                    self.nominals(&edge(site, InstanceKind::Drop), &[ty]);
                }
            }
        }
        Ok(())
    }
    fn materialized(&mut self, owner: &DefPath, site: Option<Span>, value: &Materialized) {
        let Some(site) = site else {
            return;
        };
        self.store_start = self.edges.len();
        let mut roots = vec![];
        let mut todo = vec![&value.root];
        while let Some(node) = todo.pop() {
            roots.push(node.ty);
            if let hirsel_tir::ValueKind::Construct { fields, .. } = &node.kind {
                todo.extend(fields);
            }
        }
        let edge = Edge {
            owner,
            owner_binder: None,
            site,
            kind: InstanceKind::Materialized,
        };
        self.nominals(&edge, &roots);
    }
}
fn type_arguments(arguments: &[GenericArgument]) -> impl Iterator<Item = TypeId> + '_ {
    arguments.iter().filter_map(|argument| match argument {
        GenericArgument::Type(ty) => Some(*ty),
        GenericArgument::Size(_) => None,
    })
}

impl QueryEngine {
    /// The source GEN-5 graph: every program definition's growth edges, from
    /// its definition summary. Library operations have no generic body to
    /// expand, and an edge into a definition without a summary is a dead end.
    pub fn gen5_source_graph(&mut self) -> Result<ParameterGrowth, QueryError> {
        let edges: Vec<_> = self
            .definition_summaries()?
            .into_iter()
            .flat_map(|(_, growth)| growth.edges.into_vec())
            .collect();
        Ok(ParameterGrowth {
            edges: edges.into(),
        })
    }
}

#[derive(PartialEq)]
struct VerdictInputs {
    revision: u64,
    environment: QueryEnvironment,
    limits: hirsel_interp::Limits,
    manifest: Option<ContentHash>,
    unavailable: Option<QueryKind>,
}
impl VerdictInputs {
    fn read(engine: &QueryEngine) -> Self {
        Self {
            revision: engine.graph.revision().get(),
            environment: engine.environment.clone(),
            limits: engine.comptime_limits,
            manifest: Some(engine.declarations.inputs().manifest_revision),
            unavailable: engine.unavailable,
        }
    }
}
/// A program verdict, separate from every definition and package receipt.
pub(crate) struct Verdict {
    inputs: VerdictInputs,
    result: Result<Vec<Diagnostic>, QueryError>,
}

impl QueryEngine {
    /// The program's generic-instance verdict, reused while its inputs are unchanged.
    pub fn generic_graph_diagnostics(&mut self) -> Result<Vec<Diagnostic>, QueryError> {
        check(self)
    }
}
pub(crate) fn check(engine: &mut QueryEngine) -> Result<Vec<Diagnostic>, QueryError> {
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::GenericGraph);
    let inputs = VerdictInputs::read(engine);
    if let Some(verdict) = &engine.generic_verdict {
        if verdict.inputs == inputs {
            return verdict.result.clone();
        }
    }
    let result = engine.gen5_source_graph().map(|graph| {
        crate::select_diagnostics(
            graph
                .growing_sites()
                .into_iter()
                .map(|site| {
                    Diagnostic::error(
                        const { RuleId::of("GEN-5") },
                        site,
                        "a recursive instantiation cycle grows its type arguments",
                    )
                })
                .collect(),
            Some(&engine.sources),
        )
    });
    // A query may index synthetic owners. Such a change requires a new audit;
    // unavailable/operational results never establish this proof.
    if inputs == VerdictInputs::read(engine) && !result.as_ref().is_err_and(|e| !e.cacheable()) {
        engine.generic_verdict = Some(Verdict {
            inputs,
            result: result.clone(),
        });
    }
    result
}
