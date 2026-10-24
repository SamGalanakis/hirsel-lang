//! Bounds and acyclicity of flat parser arenas before a restored tree is used.
//! Source semantics still belong to the ordinary checker and independent V.
use crate::*;

struct Graph {
    edges: Vec<Vec<usize>>,
    sizes: [usize; 4],
}
impl Graph {
    fn new(tree: &SyntaxTree) -> Self {
        let sizes = [
            tree.expressions.len(),
            tree.statements.len(),
            tree.patterns.len(),
            tree.types.len(),
        ];
        Self {
            edges: vec![vec![]; sizes.iter().sum()],
            sizes,
        }
    }
    fn node(&self, arena: usize, id: u32) -> Option<usize> {
        ((id as usize) < self.sizes[arena])
            .then(|| self.sizes[..arena].iter().sum::<usize>() + id as usize)
    }
    fn edge(&mut self, owner: Option<usize>, arena: usize, id: u32) -> Option<()> {
        let node = self.node(arena, id)?;
        if let Some(owner) = owner {
            self.edges[owner].push(node);
        }
        Some(())
    }
    fn e(&mut self, owner: Option<usize>, id: ExprId) -> Option<()> {
        self.edge(owner, 0, id.0)
    }
    fn s(&mut self, owner: Option<usize>, id: StmtId) -> Option<()> {
        self.edge(owner, 1, id.0)
    }
    fn p(&mut self, owner: Option<usize>, id: PatternId) -> Option<()> {
        self.edge(owner, 2, id.0)
    }
    fn t(&mut self, owner: Option<usize>, id: TypeExprId) -> Option<()> {
        self.edge(owner, 3, id.0)
    }
    fn function(&mut self, owner: Option<usize>, signature: &FunctionSyntax) -> Option<()> {
        for parameter in &signature.parameters {
            self.t(owner, parameter.ty)?;
        }
        if let Some(result) = signature.result {
            self.t(owner, result)?;
        }
        for contract in &signature.contracts {
            self.e(owner, contract.expression)?;
        }
        Some(())
    }
    fn acyclic(self) -> bool {
        // Kahn's algorithm visits every node, including unreachable arenas.
        // An explicit queue avoids recursion on untrusted nested expressions.
        let mut incoming = vec![0usize; self.edges.len()];
        for edges in &self.edges {
            for &node in edges {
                incoming[node] += 1;
            }
        }
        let mut ready: Vec<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(n, &count)| (count == 0).then_some(n))
            .collect();
        let mut at = 0;
        while at < ready.len() {
            let node = ready[at];
            at += 1;
            for &next in &self.edges[node] {
                incoming[next] -= 1;
                if incoming[next] == 0 {
                    ready.push(next);
                }
            }
        }
        ready.len() == self.edges.len()
    }
}
impl ItemSyntax {
    /// Check all arena references, ownership spans and token ranges, including
    /// metadata unused by the current semantic demand. The caller checks spans
    /// against its immutable observed source bytes, before placing any anchors.
    pub fn structurally_valid(&self, valid_span: impl Fn(Span) -> bool) -> bool {
        self.structure(&valid_span).is_some()
    }
    fn structure(&self, valid_span: &impl Fn(Span) -> bool) -> Option<()> {
        let tree = &self.tree;
        let span = |s: Span| (s.anchor == self.anchor && valid_span(s)).then_some(());
        let origin = |o: Origin| {
            span(o.span)?;
            (o.tokens.start <= o.tokens.end && o.tokens.end as usize <= tree.tokens.len())
                .then_some(())
        };
        let attributes = |attributes: &[Attribute]| -> Option<()> {
            for attribute in attributes {
                origin(attribute.origin)?;
                for argument in &attribute.arguments {
                    match argument {
                        AttributeArgument::LoopSource(s) => origin(s.origin)?,
                        AttributeArgument::Format(s) => origin(s.origin)?,
                        AttributeArgument::SerializableCheck(s) => origin(s.origin)?,
                        AttributeArgument::StatePromise(s) => origin(s.origin)?,
                        _ => (),
                    }
                }
            }
            Some(())
        };
        for token in &tree.tokens {
            span(token.span)?;
        }
        origin(self.item.origin)?;
        span(self.item.name_span)?;
        attributes(&self.item.attributes)?;
        if let Some(Generated::Body(names)) = &self.item.generated {
            for (_, site) in names {
                span(*site)?;
            }
        }
        let mut graph = Graph::new(tree);
        for (i, expression) in tree.expressions.iter().enumerate() {
            origin(expression.origin)?;
            let owner = graph.node(0, i as u32);
            match &expression.kind {
                ExprKind::Tuple(ids) | ExprKind::Sequence(ids) => {
                    for &id in ids {
                        graph.e(owner, id)?;
                    }
                }
                ExprKind::Call { callee, arguments } => {
                    graph.e(owner, *callee)?;
                    for argument in arguments {
                        graph.e(owner, argument.value)?;
                    }
                }
                ExprKind::Field { base, .. } => graph.e(owner, *base)?,
                ExprKind::Index { base, index } => {
                    graph.e(owner, *base)?;
                    graph.e(owner, *index)?;
                }
                ExprKind::Unary { value, .. } | ExprKind::Try { value, .. } => {
                    graph.e(owner, *value)?
                }
                ExprKind::Binary { left, right, .. } => {
                    graph.e(owner, *left)?;
                    graph.e(owner, *right)?;
                }
                ExprKind::Block {
                    statements, tail, ..
                } => {
                    for &id in statements {
                        graph.s(owner, id)?;
                    }
                    if let Some(id) = tail {
                        graph.e(owner, *id)?;
                    }
                }
                ExprKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    graph.e(owner, *condition)?;
                    graph.e(owner, *then_branch)?;
                    if let Some(id) = else_branch {
                        graph.e(owner, *id)?;
                    }
                }
                ExprKind::Match { value, arms } => {
                    graph.e(owner, *value)?;
                    for arm in arms {
                        graph.p(owner, arm.pattern)?;
                        graph.e(owner, arm.body)?;
                        if let Some(id) = arm.guard {
                            graph.e(owner, id)?;
                        }
                    }
                }
                ExprKind::Closure { signature, body } => {
                    attributes(&signature.attributes)?;
                    graph.function(owner, signature)?;
                    graph.e(owner, *body)?;
                }
                ExprKind::Return(id) | ExprKind::Break { value: id, .. } => {
                    if let Some(id) = id {
                        graph.e(owner, *id)?;
                    }
                }
                ExprKind::Shape(id) => graph.t(owner, *id)?,
                ExprKind::Before(id)
                | ExprKind::Comptime(id)
                | ExprKind::Formatter(FormatStep::Pad(id)) => graph.e(owner, *id)?,
                ExprKind::Literal(_)
                | ExprKind::Path(_)
                | ExprKind::Continue(_)
                | ExprKind::Heap
                | ExprKind::Result
                | ExprKind::Todo(_)
                | ExprKind::Formatter(_)
                | ExprKind::Error => (),
            }
        }
        for (i, statement) in tree.statements.iter().enumerate() {
            origin(statement.origin)?;
            let owner = graph.node(1, i as u32);
            match &statement.kind {
                StmtKind::Let {
                    pattern,
                    ty,
                    value,
                    otherwise,
                    ..
                } => {
                    graph.p(owner, *pattern)?;
                    graph.e(owner, *value)?;
                    if let Some(id) = ty {
                        graph.t(owner, *id)?;
                    }
                    if let Some(id) = otherwise {
                        graph.e(owner, *id)?;
                    }
                }
                StmtKind::Assign { place, value, .. } => {
                    graph.e(owner, *place)?;
                    graph.e(owner, *value)?;
                }
                StmtKind::Expr(id) => graph.e(owner, *id)?,
                StmtKind::While {
                    condition, body, ..
                } => {
                    graph.e(owner, *condition)?;
                    graph.e(owner, *body)?;
                }
                StmtKind::For {
                    pattern,
                    source,
                    body,
                    ..
                } => {
                    graph.p(owner, *pattern)?;
                    graph.e(owner, *source)?;
                    graph.e(owner, *body)?;
                }
                StmtKind::Function {
                    signature, body, ..
                } => {
                    attributes(&signature.attributes)?;
                    graph.function(owner, signature)?;
                    graph.e(owner, *body)?;
                }
            }
        }
        for (i, pattern) in tree.patterns.iter().enumerate() {
            origin(pattern.origin)?;
            let owner = graph.node(2, i as u32);
            match &pattern.kind {
                PatternKind::Tuple(ids) | PatternKind::Or(ids) => {
                    for &id in ids {
                        graph.p(owner, id)?;
                    }
                }
                PatternKind::Constructor { fields, .. } => {
                    for field in fields {
                        graph.p(owner, field.pattern)?;
                    }
                }
                PatternKind::Wildcard
                | PatternKind::Binding(_)
                | PatternKind::Literal(_)
                | PatternKind::Range { .. }
                | PatternKind::Error => (),
            }
        }
        for (i, ty) in tree.types.iter().enumerate() {
            origin(ty.origin)?;
            let owner = graph.node(3, i as u32);
            match &ty.kind {
                TypeExprKind::Named { arguments, .. } => {
                    for argument in arguments {
                        if let TypeArgument::Type(id) = argument {
                            graph.t(owner, *id)?;
                        }
                    }
                }
                TypeExprKind::Tuple(ids) => {
                    for &id in ids {
                        graph.t(owner, id)?;
                    }
                }
                TypeExprKind::Function(signature) => {
                    attributes(&signature.attributes)?;
                    graph.function(owner, signature)?;
                }
                TypeExprKind::Box(id) => graph.t(owner, *id)?,
                TypeExprKind::Qualified { base, relation, .. } => {
                    graph.t(owner, *base)?;
                    if let Some(relation) = relation {
                        origin(relation.origin)?;
                    }
                }
                TypeExprKind::Error => (),
            }
        }
        match &self.item.kind {
            ItemKind::Function {
                signature, body, ..
            } => {
                attributes(&signature.attributes)?;
                graph.function(None, signature)?;
                if let Some(id) = body {
                    graph.e(None, *id)?;
                }
            }
            ItemKind::Const { ty, value } => {
                graph.t(None, *ty)?;
                graph.e(None, *value)?;
            }
            ItemKind::Type { body, drop, .. } => {
                match body {
                    TypeBody::Record(fields) => {
                        for field in fields {
                            origin(field.origin)?;
                            graph.t(None, field.ty)?;
                        }
                    }
                    TypeBody::Variants(variants) => {
                        for variant in variants {
                            for (_, id) in &variant.fields {
                                graph.t(None, *id)?;
                            }
                        }
                    }
                    TypeBody::Alias(id) => graph.t(None, *id)?,
                    TypeBody::Distinct { field, fact } => {
                        origin(field.origin)?;
                        graph.t(None, field.ty)?;
                        if let Some(id) = fact {
                            graph.e(None, *id)?;
                        }
                    }
                    TypeBody::Opaque => (),
                }
                if let Some(drop) = drop {
                    graph.e(None, drop.body)?;
                }
            }
        }
        for format in &tree.formats {
            graph.e(None, format.expression)?;
            if format.pieces.len() != format.holes.len().checked_add(1)?
                || format.specs.len() != format.holes.len()
            {
                return None;
            }
            for &id in &format.holes {
                graph.e(None, id)?;
            }
        }
        for repeat in &tree.repeats {
            for id in [
                repeat.expression,
                repeat.sequence,
                repeat.value,
                repeat.count,
                repeat.evaluated_count,
            ] {
                graph.e(None, id)?;
            }
        }
        for condition in &tree.conditions {
            graph.e(None, condition.matching())?;
            if let LetCondition::While { body, .. } = condition {
                graph.e(None, *body)?;
            }
        }
        for group in &tree.groups {
            origin(group.origin)?;
            graph.e(None, group.expression)?;
        }
        for analysis in &tree.analyses {
            origin(analysis.origin)?;
            if analysis.payload.len() != analysis.spaced.len() {
                return None;
            }
            let valid = match (&self.item.kind, analysis.site) {
                (_, AnalysisSite::Item) => true,
                (ItemKind::Function { signature, .. }, AnalysisSite::Parameter(i)) => {
                    (i as usize) < signature.parameters.len()
                }
                (
                    ItemKind::Type {
                        body: TypeBody::Record(fields),
                        ..
                    },
                    AnalysisSite::Field(i),
                ) => (i as usize) < fields.len(),
                (
                    ItemKind::Type {
                        body: TypeBody::Distinct { .. },
                        ..
                    },
                    AnalysisSite::Field(0),
                ) => true,
                (
                    ItemKind::Type {
                        body: TypeBody::Variants(variants),
                        ..
                    },
                    AnalysisSite::Payload { variant, field },
                ) => variants
                    .get(variant as usize)
                    .is_some_and(|v| (field as usize) < v.fields.len()),
                _ => false,
            };
            if !valid {
                return None;
            }
        }
        graph.acyclic().then_some(())
    }
}
