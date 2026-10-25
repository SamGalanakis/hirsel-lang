//! The rows a body's mode passes know: the closure of its checked tables and
//! every row a pass interns, in first-seen order. Rows are session-wide in the
//! interner, so this is the body's own view of them, which replaces iterating
//! a per-body store.
use hirsel_types::{CheckedBody, GenericArgument, TypeId, TypeInterner};
type HashSet<T> = std::collections::HashSet<T, hirsel_intern::IdHasher>;

/// Every row id the checked tables of `body` name, in table order.
pub fn body_roots(body: &CheckedBody) -> Vec<TypeId> {
    fn arguments<'a>(arguments: &'a [GenericArgument]) -> impl Iterator<Item = TypeId> + 'a {
        arguments.iter().filter_map(|argument| match argument {
            GenericArgument::Type(ty) => Some(*ty),
            GenericArgument::Size(_) => None,
        })
    }
    let mut roots = vec![];
    for call in &body.calls {
        roots.extend(arguments(&call.generic_arguments));
        roots.push(call.result);
        roots.extend(
            call.arguments
                .iter()
                .filter_map(|a| a.parameter.as_ref().map(|(_, p)| p.ty)),
        );
    }
    roots.extend(body.bindings.iter().map(|b| b.ty));
    roots.extend(body.patterns.iter().map(|p| p.ty));
    for expression in &body.expressions {
        roots.push(expression.ty);
        roots.extend(arguments(&expression.instantiated_arguments));
    }
    for instantiation in &body.instantiations {
        roots.extend(arguments(&instantiation.arguments));
    }
    for closure in &body.closures {
        roots.extend(closure.result_evidence);
        roots.push(closure.ty);
        roots.extend(closure.captures.iter().map(|c| c.ty));
    }
    roots
}

/// The rows of one body, each once, with the graphs they reach.
#[derive(Clone, Debug, Default)]
pub struct BodyRows {
    order: Vec<TypeId>,
    seen: HashSet<TypeId>,
}
impl BodyRows {
    pub fn new(t: &TypeInterner, body: &CheckedBody) -> Self {
        let mut rows = Self::default();
        for root in body_roots(body) {
            rows.note(t, root);
        }
        rows
    }
    /// Add `root` and the rows it reaches that are not yet known.
    pub fn note(&mut self, t: &TypeInterner, root: TypeId) {
        if self.seen.contains(&root) {
            return;
        }
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let Some(row) = t.get(id) else { continue };
            if !self.seen.insert(id) {
                continue;
            }
            self.order.push(id);
            pending.extend(row.kind.graph_edges().into_iter().rev().map(|e| e.ty));
        }
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    /// The known rows in first-seen order.
    pub fn ids(&self) -> &[TypeId] {
        &self.order
    }
    pub fn contains(&self, id: TypeId) -> bool {
        self.seen.contains(&id)
    }
}

/// Facts indexed by the dense expression ordinals of one item. Ported from
/// rust-analyzer's la-arena `ArenaMap` (lib/la-arena/src/map.rs:9-11,56-69).
/// Global session ids are deliberately not indexes into this body-local map.
#[derive(Clone, Debug)]
pub struct ExprRows<T>(Vec<Option<T>>);
impl<T> Default for ExprRows<T> {
    fn default() -> Self {
        Self(vec![])
    }
}
impl<T> ExprRows<T> {
    pub fn get(&self, expression: &hirsel_syntax::ExprId) -> Option<&T> {
        self.0.get(expression.0 as usize)?.as_ref()
    }
    pub fn entry(&mut self, expression: hirsel_syntax::ExprId) -> &mut Option<T> {
        let index = expression.0 as usize;
        if self.0.len() <= index {
            self.0.resize_with(index + 1, || None);
        }
        &mut self.0[index]
    }
}
impl<T> FromIterator<(hirsel_syntax::ExprId, T)> for ExprRows<T> {
    fn from_iter<I: IntoIterator<Item = (hirsel_syntax::ExprId, T)>>(rows: I) -> Self {
        let mut out = Self::default();
        for (expression, value) in rows {
            *out.entry(expression) = Some(value);
        }
        out
    }
}
#[cfg(test)]
mod expression_rows_tests {
    use super::*;
    use hirsel_syntax::ExprId;
}
