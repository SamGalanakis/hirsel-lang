//! Conservative borrowed-data equations, settled in the private module query.
//! Local aliases form a finite union lattice. Calls use only settled summaries;
//! a recursive cycle cannot bootstrap a nonretention promise.
use super::*;
use hirsel_syntax::{ExprId, ExprKind, ItemKind, PatternId, StmtKind};
use hirsel_types::{CallTarget, ParamId, PassingMode, ResolvedName, TypeKind};
use std::collections::BTreeSet;

pub(super) fn infer(
    types: &hirsel_types::TypeInterner,
    bodies: &[(Arc<TypedBody>, SignatureRef)],
    headers: &mut BTreeMap<DefId, SignatureRef>,
    provider: &mut dyn SignatureProvider,
) -> Result<(), QueryError> {
    let positions: BTreeMap<_, _> = bodies
        .iter()
        .enumerate()
        .map(|(i, (_, s))| (s.definition, i))
        .collect();
    let graph: Vec<Vec<usize>> = bodies
        .iter()
        .map(|(body, _)| {
            body.checked
                .calls
                .iter()
                .filter_map(|call| {
                    let CallTarget::Definition(path) = &call.target else {
                        return None;
                    };
                    positions.get(&DefId::intern(path)).copied()
                })
                .collect()
        })
        .collect();
    let cyclic: BTreeSet<_> = hirsel_graph::components(&graph)
        .into_iter()
        .filter(|c| c.len() > 1 || graph[c[0]].contains(&c[0]))
        .flatten()
        .collect();
    loop {
        let mut additions = Vec::new();
        for (index, (body, declared)) in bodies.iter().enumerate() {
            if cyclic.contains(&index) {
                continue;
            }
            let Some(header) = headers.get(&declared.definition) else {
                continue;
            };
            let hirsel_types::SignatureKind::Function(f) = &header.kind else {
                continue;
            };
            // The expression proof does not cover generated requires/ensures
            // calls. Their data parameters have no nonretaining entry contract.
            if !f.contracts.is_empty() {
                continue;
            }
            let mut next = (**header).clone();
            for (i, parameter) in f.parameters.iter().enumerate() {
                let parameter_id = ParamId(i as u32);
                if parameter.passing != PassingMode::Borrow
                    || !concrete_data_parameter(types, parameter.ty)
                    || matches!(&types.row(parameter.ty).kind, TypeKind::Primitive(p) if !matches!(p, hirsel_types::Primitive::Bytes | hirsel_types::Primitive::String))
                    || matches!(&types.row(parameter.ty).kind, TypeKind::Tuple(fields) if fields.is_empty())
                    || next
                        .metadata
                        .inferred_unique
                        .nonretaining
                        .contains(&parameter_id)
                {
                    continue;
                }
                let mut proof = Proof {
                    types,
                    body,
                    headers,
                    provider,
                    parameter: parameter_id,
                    locals: BTreeSet::new(),
                    failed: false,
                };
                let ItemKind::Function {
                    body: Some(root), ..
                } = body.syntax.item.kind
                else {
                    continue;
                };
                loop {
                    let previous = proof.locals.clone();
                    if proof.expression(root)? {
                        proof.failed = true;
                    }
                    if proof.failed || previous == proof.locals {
                        break;
                    }
                }
                if !proof.failed {
                    next.metadata
                        .inferred_unique
                        .nonretaining
                        .push(parameter_id);
                }
            }
            next.metadata.inferred_unique.nonretaining.sort();
            if next.metadata.inferred_unique != header.metadata.inferred_unique {
                additions.push((declared.definition, Arc::new(next)));
            }
        }
        if additions.is_empty() {
            return Ok(());
        }
        headers.extend(additions);
    }
}

// A physical specialization must not invent an entry contract for a callable
// hidden by a source generic. Concrete resource/opaque shapes remain eligible.
fn concrete_data_parameter(types: &hirsel_types::TypeInterner, ty: TypeId) -> bool {
    let mut pending = vec![ty];
    let mut seen = std::collections::HashSet::new();
    while let Some(ty) = pending.pop() {
        if !seen.insert(ty) {
            continue;
        }
        let Some(row) = types.get(ty) else {
            return false;
        };
        if matches!(
            &row.kind,
            TypeKind::Error | TypeKind::Function(_) | TypeKind::Parameter(_)
        ) {
            return false;
        }
        pending.extend(row.kind.graph_edges().into_iter().map(|edge| edge.ty));
    }
    true
}

struct Proof<'a> {
    types: &'a hirsel_types::TypeInterner,
    body: &'a TypedBody,
    headers: &'a BTreeMap<DefId, SignatureRef>,
    provider: &'a mut dyn SignatureProvider,
    parameter: ParamId,
    locals: BTreeSet<hirsel_types::LocalId>,
    failed: bool,
}
impl Proof<'_> {
    fn bind(&mut self, pattern: PatternId, held: bool) {
        if !held {
            return;
        }
        let resolution = &self.body.checked.resolution;
        let mut pending = vec![pattern];
        while let Some(pattern) = pending.pop() {
            for (p, _, local) in &resolution.bindings {
                if *p == pattern {
                    self.locals.insert(*local);
                }
            }
            if let Some(row) = self
                .body
                .checked
                .patterns
                .iter()
                .find(|p| p.pattern == pattern)
            {
                pending.extend(row.subpatterns());
            }
        }
    }
    fn expression(&mut self, expression: ExprId) -> Result<bool, QueryError> {
        let tree = &self.body.syntax.tree;
        let held = match &tree.expressions[expression.0 as usize].kind {
            ExprKind::Path(_) => match self
                .body
                .checked
                .resolution
                .expressions
                .iter()
                .find(|(e, _)| *e == expression)
                .map(|(_, n)| n)
            {
                Some(ResolvedName::Parameter(p)) => *p == self.parameter,
                Some(ResolvedName::Local(l)) => self.locals.contains(l),
                _ => false,
            },
            ExprKind::Literal(_)
            | ExprKind::Heap
            | ExprKind::Result
            | ExprKind::Shape(_)
            | ExprKind::Todo(_) => false,
            ExprKind::Tuple(values) | ExprKind::Sequence(values) => {
                let mut held = false;
                for value in values {
                    held |= self.expression(*value)?;
                }
                // Construction of an owning carrier is deliberately unproved.
                if held {
                    self.failed = true;
                }
                held
            }
            ExprKind::Field { base, .. } => self.expression(*base)?,
            ExprKind::Index { base, index } => {
                let held = self.expression(*base)?;
                if self.expression(*index)? {
                    self.failed = true;
                }
                held
            }
            ExprKind::Unary { op, value } => {
                let held = self.expression(*value)?;
                if held && !matches!(op, hirsel_syntax::UnaryOp::Copy) {
                    self.failed = true;
                }
                held
            }
            ExprKind::Binary { left, right, .. } => {
                let held = self.expression(*left)? | self.expression(*right)?;
                if held {
                    self.failed = true;
                }
                false
            }
            ExprKind::Block {
                label,
                statements,
                tail,
            } => {
                if label.is_some() {
                    self.failed = true;
                }
                for statement in statements {
                    match &tree.statements[statement.0 as usize].kind {
                        StmtKind::Let {
                            pattern,
                            value,
                            otherwise,
                            ..
                        } => {
                            let held = self.expression(*value)?;
                            self.bind(*pattern, held);
                            if let Some(otherwise) = otherwise {
                                self.expression(*otherwise)?;
                            }
                        }
                        StmtKind::Assign { place, value, .. } => {
                            self.expression(*place)?;
                            let held = self.expression(*value)?;
                            if held {
                                match self
                                    .body
                                    .checked
                                    .resolution
                                    .expressions
                                    .iter()
                                    .find(|(e, _)| e == place)
                                    .map(|(_, n)| n)
                                {
                                    Some(ResolvedName::Local(local))
                                        if matches!(
                                            tree.expressions[place.0 as usize].kind,
                                            ExprKind::Path(_)
                                        ) =>
                                    {
                                        self.locals.insert(*local);
                                    }
                                    _ => self.failed = true,
                                }
                            }
                        }
                        StmtKind::Expr(value) => {
                            self.expression(*value)?;
                        }
                        // Loop protocols and nested lexical environments need their
                        // own equations; absence of proof remains conservative.
                        StmtKind::While { .. }
                        | StmtKind::For { .. }
                        | StmtKind::Function { .. } => self.failed = true,
                    }
                }
                match tail {
                    Some(tail) => self.expression(*tail)?,
                    None => false,
                }
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if self.expression(*condition)? {
                    self.failed = true;
                }
                let mut held = self.expression(*then_branch)?;
                if let Some(otherwise) = else_branch {
                    held |= self.expression(*otherwise)?;
                }
                held
            }
            ExprKind::Match { value, arms } => {
                let input = self.expression(*value)?;
                let mut held = false;
                for arm in arms {
                    self.bind(arm.pattern, input);
                    if let Some(guard) = arm.guard {
                        if self.expression(guard)? {
                            self.failed = true;
                        }
                    }
                    held |= self.expression(arm.body)?;
                }
                held
            }
            ExprKind::Call { callee, arguments } => {
                if self.expression(*callee)? {
                    self.failed = true;
                }
                let Some(call) = self
                    .body
                    .checked
                    .calls
                    .iter()
                    .find(|c| c.expression == expression)
                else {
                    self.failed = true;
                    return Ok(true);
                };
                for argument in arguments {
                    if !self.expression(argument.value)? {
                        continue;
                    }
                    let parameter = call
                        .arguments
                        .iter()
                        .find(|a| a.expression == argument.value)
                        .and_then(|a| a.parameter.as_ref());
                    let allowed =
                        if let (CallTarget::Definition(path), Some((ordinal, parameter))) =
                            (&call.target, parameter)
                        {
                            if parameter.passing != PassingMode::Borrow {
                                false
                            } else if *ordinal == 0
                                && ["list.len", "array.len", "bytes.len", "string.byte_len"]
                                    .iter()
                                    .any(|name| path.is_core(name))
                            {
                                true
                            } else {
                                let id = DefId::intern(path);
                                let header = match self.headers.get(&id) {
                                    Some(header) => header.clone(),
                                    None => self.provider.signature(id)?,
                                };
                                header
                                    .metadata
                                    .inferred_unique
                                    .nonretaining
                                    .contains(&ParamId(*ordinal))
                            }
                        } else {
                            false
                        };
                    if !allowed {
                        self.failed = true;
                    }
                }
                // A checked observer returns no borrowed input content.
                false
            }
            ExprKind::Return(value) | ExprKind::Break { value, .. } => {
                if let Some(value) = value {
                    if self.expression(*value)? {
                        self.failed = true;
                    }
                }
                false
            }
            ExprKind::Closure { .. }
            | ExprKind::Try { .. }
            | ExprKind::Before(_)
            | ExprKind::Comptime(_)
            | ExprKind::With { .. }
            | ExprKind::Formatter(_)
            | ExprKind::Continue(_)
            | ExprKind::Error => {
                self.failed = true;
                false
            }
        };
        // Copying scalar bits observes a root without retaining its storage.
        let bits = self.body.checked.expressions.iter().find(|e| e.expression == expression).is_some_and(|e| {
            matches!(&self.types.row(e.ty).kind, TypeKind::Primitive(p) if !matches!(p, hirsel_types::Primitive::Bytes | hirsel_types::Primitive::String))
                || matches!(&self.types.row(e.ty).kind, TypeKind::Tuple(fields) if fields.is_empty())
        });
        Ok(held && !bits)
    }
}
