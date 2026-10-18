//! Signature-owned contracts use the ordinary expression checker and its plans.
use super::*;
impl Checker<'_, '_> {
    pub(super) fn check_contracts(&mut self) -> Result<(), QueryError> {
        for frame in 0..self.frames.len() {
            let mut syntax = match self.frames[frame].owner {
                NameSite::Item(i) => match &self.input.tree.items[i.0 as usize].kind {
                    ast::ItemKind::Function { signature, .. } => signature.clone(),
                    _ => continue,
                },
                NameSite::Statement(i) => match &self.input.tree.statements[i.0 as usize].kind {
                    ast::StmtKind::Function { signature, .. } => signature.clone(),
                    _ => continue,
                },
                NameSite::Expression(i) => match &self.input.tree.expressions[i.0 as usize].kind {
                    ast::ExprKind::Closure { signature, .. } => signature.clone(),
                    _ => continue,
                },
                _ => continue,
            };
            let fact = match self.frames[frame].owner {
                NameSite::Item(i)
                    if hirsel_check_protocol::fact::fact_owner(
                        &self.input.tree.items[i.0 as usize].definition,
                    )
                    .is_some() =>
                {
                    match self.input.tree.items[i.0 as usize].kind {
                        ast::ItemKind::Function {
                            body: Some(body), ..
                        } => Some(body),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(expression) = fact {
                syntax.contracts.push(ast::ContractSyntax {
                    kind: ast::ContractKind::Requires,
                    expression,
                });
            }
            for contract in syntax.contracts.clone() {
                let root = contract.expression;
                let rule = if fact == Some(root) {
                    const { RuleId::of("FACT-2") }
                } else {
                    const { RuleId::of("FACT-1") }
                };
                let mut snapshots = vec![];
                if !self.invalid(self.value(root))
                    && self.kind(self.value(root)) != TypeKind::Primitive(Primitive::Bool)
                {
                    self.report(rule.clone(), self.span(root), "contract must be boolean");
                }
                let mut todo = vec![(root, false)];
                while let Some((id, before)) = todo.pop() {
                    let kind = self.input.tree.expressions[id.0 as usize].kind.clone();
                    if matches!(
                        kind,
                        ast::ExprKind::Unary {
                            op: ast::UnaryOp::Take | ast::UnaryOp::Mut,
                            ..
                        }
                    ) {
                        let target = match kind {
                            ast::ExprKind::Unary { value, .. } => value,
                            _ => unreachable!(),
                        };
                        let consumes = !matches!(
                            kind,
                            ast::ExprKind::Unary {
                                op: ast::UnaryOp::Take,
                                ..
                            }
                        ) || !self.bound(self.value(target), ast::Bound::Plain)?;
                        if consumes && self.source_place(target).is_some_and(|p| {
                            matches!(&p.root, SourceRoot::Binding { owner, binding: ResolvedName::Parameter(_) } if *owner == self.frames[frame].owner)
                        }) {
                            self.report(rule.clone(), self.span(root), "contract parameters are read-only");
                        }
                    }
                    match kind {
                        ast::ExprKind::Before(value) => {
                            snapshots.push(id);
                            if !self.invalid(self.value(value)) {
                                if self.bound(self.value(value), ast::Bound::Plain)? {
                                    // A snapshot owns a copy, even when its source is a mut
                                    // parameter. Its loan must not span the function body.
                                    if let Some(row) = self.elaboration(value) {
                                        row.usage = UseKind::Copy;
                                    }
                                } else {
                                    self.report(
                                        rule.clone(),
                                        self.span(id),
                                        "entry snapshot must be plain",
                                    );
                                }
                            }
                            todo.push((value, true));
                        }
                        ast::ExprKind::Path(_) => {
                            if contract.kind == ast::ContractKind::Ensures && !before {
                                if let Some((_, ResolvedName::Parameter(p))) = self.body_name(id) {
                                    if syntax
                                        .parameters
                                        .get(p.0 as usize)
                                        .is_some_and(|p| p.passing == ast::PassingMode::Take)
                                    {
                                        self.report(
                                            rule.clone(),
                                            self.span(root),
                                            "taken postcondition parameter requires before",
                                        );
                                    }
                                }
                            }
                        }
                        ast::ExprKind::Call { callee, arguments } => {
                            let checked_arguments = self
                                .body
                                .calls
                                .iter()
                                .find(|call| call.expression == id)
                                .map(|call| call.arguments.clone())
                                .unwrap_or_default();
                            for argument in checked_arguments {
                                let Some((_, parameter)) = argument.parameter else {
                                    continue;
                                };
                                let consumes = parameter.passing == ast::PassingMode::Mut
                                    || parameter.passing == ast::PassingMode::Take
                                        && !self.bound(
                                            self.value(argument.expression),
                                            ast::Bound::Plain,
                                        )?;
                                if consumes && self.source_place(argument.expression).is_some_and(|place| {
                                    matches!(&place.root, SourceRoot::Binding {owner, binding: ResolvedName::Parameter(_)} if *owner == self.frames[frame].owner)
                                }) {
                                    self.report(rule.clone(), self.span(root), "contract parameters are read-only");
                                    break;
                                }
                            }
                            if let TypeKind::Function(f) = self.kind(self.value(callee)) {
                                let known_literal = f.authority == AuthoritySummary::Unknown
                                    && self
                                        .body
                                        .closures
                                        .iter()
                                        .any(|c| c.ty == self.value(callee));
                                if f.authority != AuthoritySummary::inert() && !known_literal {
                                    self.report(
                                        rule.clone(),
                                        self.span(root),
                                        "contract calls must be pure",
                                    );
                                }
                            }
                            todo.push((callee, before));
                            todo.extend(arguments.into_iter().map(|a| (a.value, before)));
                        }
                        _ => todo.extend(
                            children(self.input.tree, id)
                                .into_iter()
                                .map(|e| (e, before)),
                        ),
                    }
                }
                snapshots.sort_by_key(|e| self.span(*e).start);
                snapshots.dedup();
                if fact == Some(root) {
                    continue;
                }
                let Some((function, parameters)) =
                    self.clause_frame(frame, &syntax, &contract, &snapshots)?
                else {
                    continue;
                };
                self.plans.contracts.push(ContractPlan {
                    owner: self.frames[frame].owner,
                    expression: root,
                    kind: contract.kind,
                    snapshots,
                    function,
                    parameters,
                });
            }
        }
        Ok(())
    }

    /// Rebind executable clause occurrences to real borrowed formal rows. Generic
    /// references keep their declaring frame; a clause introduces no generics.
    fn clause_frame(
        &mut self,
        frame: usize,
        syntax: &ast::FunctionSyntax,
        contract: &ast::ContractSyntax,
        snapshots: &[ast::ExprId],
    ) -> Result<Option<(TypeId, Vec<ContractParameter>)>, QueryError> {
        let original = self.frames[frame].owner;
        let scope = NameSite::Expression(contract.expression);
        let mut parameters = vec![];
        let mut types = vec![];
        let mut ordinals = BTreeMap::new();
        for (ordinal, parameter) in syntax.parameters.iter().enumerate() {
            if contract.kind == ast::ContractKind::Ensures
                && parameter.passing == ast::PassingMode::Take
            {
                continue;
            }
            let old = ParamId(ordinal as u32);
            ordinals.insert(old, ParamId(parameters.len() as u32));
            parameters.push(ContractParameter::Parameter(old));
            types.push(self.frames[frame].parameters[ordinal]);
        }
        for snapshot in snapshots {
            parameters.push(ContractParameter::Snapshot(*snapshot));
            types.push(self.value(*snapshot));
        }
        if contract.kind == ast::ContractKind::Ensures {
            parameters.push(ContractParameter::Result);
            types.push(self.frames[frame].result.unwrap());
        }
        let remap_graph = |store: &mut TypeStore, ty| {
            map_parameter_references(store, ty, |p, _| match p {
                ParameterRef::Body {
                    scope: owner,
                    parameter,
                } if owner == original => ordinals
                    .get(&parameter)
                    .copied()
                    .map(|p| ParameterRef::body(scope, p))
                    .ok_or(GraphError::UnboundParameter(p)),
                p => Ok(p),
            })
        };
        let transport_error = |error| {
            QueryError::Operational(OperationalFailure {
                operation: "contract clause parameter transport".into(),
                message: format!("{error:?}"),
            })
        };
        let mut formal_types = vec![];
        for ty in types {
            match remap_graph(&mut self.body.types, ty) {
                Ok(ty) => formal_types.push(ty),
                // Authority checking owns illegal taken-parameter brands and
                // placements. Do not construct an exit frame from that invalid
                // signature or abort before its declaration diagnostic.
                Err(GraphError::UnboundParameter(_)) => return Ok(None),
                Err(error) => return Err(transport_error(error)),
            }
        }
        let remap = |store: &mut TypeStore, ty| remap_graph(store, ty).map_err(transport_error);
        // Only the executable clause is rebound. Snapshot children are entry
        // expressions in the original frame and are evaluated there exactly once.
        let mut expressions = BTreeSet::new();
        let mut patterns = BTreeSet::new();
        let mut todo = vec![contract.expression];
        while let Some(id) = todo.pop() {
            if !expressions.insert(id) {
                continue;
            }
            if matches!(
                self.input.tree.expressions[id.0 as usize].kind,
                ast::ExprKind::Before(_)
            ) {
                continue;
            }
            if let ast::ExprKind::Closure { body, .. } =
                self.input.tree.expressions[id.0 as usize].kind
            {
                todo.push(body);
            }
            if let ast::ExprKind::Block { ref statements, .. } =
                self.input.tree.expressions[id.0 as usize].kind
            {
                for statement in statements {
                    if let ast::StmtKind::Function { body, .. } =
                        self.input.tree.statements[statement.0 as usize].kind
                    {
                        todo.push(body);
                    }
                    if let ast::StmtKind::Let { pattern, .. } =
                        self.input.tree.statements[statement.0 as usize].kind
                    {
                        let mut pending = vec![pattern];
                        while let Some(pattern) = pending.pop() {
                            patterns.insert(pattern);
                            if let Some(row) =
                                self.body.patterns.iter().find(|p| p.pattern == pattern)
                            {
                                pending.extend(row.subpatterns());
                            }
                        }
                    }
                }
            }
            todo.extend(children(self.input.tree, id));
        }
        for row in &mut self.body.expressions {
            if expressions.contains(&row.expression) {
                row.ty = remap(&mut self.body.types, row.ty)?;
            }
        }
        for row in &mut self.body.calls {
            if !expressions.contains(&row.expression) {
                continue;
            }
            row.result = remap(&mut self.body.types, row.result)?;
            for argument in &mut row.arguments {
                if let Some((_, parameter)) = &mut argument.parameter {
                    parameter.ty = remap(&mut self.body.types, parameter.ty)?;
                }
            }
            for argument in &mut row.generic_arguments {
                if let GenericArgument::Type(ty) = argument {
                    *ty = remap(&mut self.body.types, *ty)?;
                }
            }
        }
        for row in &mut self.body.instantiations {
            if !expressions.contains(&row.expression) {
                continue;
            }
            for argument in &mut row.arguments {
                if let GenericArgument::Type(ty) = argument {
                    *ty = remap(&mut self.body.types, *ty)?;
                }
            }
        }
        for row in &mut self.body.patterns {
            if patterns.contains(&row.pattern) {
                row.ty = remap(&mut self.body.types, row.ty)?;
            }
        }
        for info in &mut self.body.closures {
            if expressions.contains(&info.expression) {
                info.ty = remap(&mut self.body.types, info.ty)?;
                if let Some(result) = &mut info.result_evidence {
                    *result = remap(&mut self.body.types, *result)?;
                }
                for capture in &mut info.captures {
                    capture.ty = remap(&mut self.body.types, capture.ty)?;
                }
            }
        }
        for row in &mut self.body.bindings {
            if matches!(row.owner, NameSite::Expression(e) if expressions.contains(&e))
                || matches!(row.site, NameSite::Pattern(p) if patterns.contains(&p))
            {
                row.ty = remap(&mut self.body.types, row.ty)?;
            }
            if row.owner == original
                && matches!(row.site, NameSite::Pattern(p) if patterns.contains(&p))
            {
                row.owner = scope;
                row.ty = remap(&mut self.body.types, row.ty)?;
            }
        }
        for (expression, name) in &mut self.body.resolution.expressions {
            if expressions.contains(expression)
                && self
                    .body
                    .resolution
                    .reference_scopes
                    .iter()
                    .any(|(site, owner)| {
                        *site == NameSite::Expression(*expression) && *owner == original
                    })
            {
                if let ResolvedName::Parameter(parameter) = name {
                    if let Some(new) = ordinals.get(parameter) {
                        *parameter = *new;
                    }
                }
            }
        }
        for (site, owner) in &mut self.body.resolution.reference_scopes {
            if matches!(site, NameSite::Expression(e) if expressions.contains(e))
                && *owner == original
            {
                *owner = scope;
            }
        }
        for place in self.source_places.values_mut() {
            if !expressions.contains(&place.expression) {
                continue;
            }
            if let SourceRoot::Binding { owner, binding } = &mut place.root {
                if *owner == original {
                    *owner = scope;
                    if let ResolvedName::Parameter(parameter) = binding {
                        if let Some(new) = ordinals.get(parameter) {
                            *parameter = *new;
                        }
                    }
                }
            }
        }
        let mut formal = vec![];
        for (ordinal, mut ty) in formal_types.into_iter().enumerate() {
            if !self.bound(ty, ast::Bound::Plain)? {
                let mut row = self.body.types.get(ty).unwrap().clone();
                row.modes.relation = Relation::environment();
                ty = self.body.types.push(row);
                ty = self.normalize_properties(ty);
            }
            let parameter = ParamId(ordinal as u32);
            self.body.bindings.push(BindingDecision {
                owner: scope,
                binding: ResolvedName::Parameter(parameter),
                site: scope,
                ty,
                usage: UseKind::Borrow,
                mutable: false,
            });
            formal.push(Parameter {
                name: None,
                label: None,
                passing: ast::PassingMode::Borrow,
                ty,
            });
        }
        let boolean = self.value(contract.expression);
        let function = self.ty(TypeKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters: formal.into(),
            result: boolean,
            error: None,
            allocation: if syntax
                .attributes
                .iter()
                .any(|a| self.name(a.name) == "noalloc")
            {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            },
            authority: AuthoritySummary::inert(),
            uses: Uses::Only(AuthoritySummary::inert()),
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }));
        let function =
            abstract_parameter_scope(&mut self.body.types, function, scope).map_err(|e| {
                QueryError::Operational(OperationalFailure {
                    operation: "contract clause header".into(),
                    message: format!("{e:?}"),
                })
            })?;
        self.body
            .resolution
            .generic_declarations
            .insert(scope, vec![]);
        let clause_frame = self.empty_frame(scope, Some(frame), Some(boolean), None, false);
        for child in 0..clause_frame {
            let in_clause = match self.frames[child].owner {
                NameSite::Expression(e) => expressions.contains(&e),
                NameSite::Statement(s) => {
                    matches!(self.input.tree.statements[s.0 as usize].kind, ast::StmtKind::Function { body, .. } if expressions.contains(&body))
                }
                _ => false,
            };
            if in_clause {
                if let Some(result) = &mut self.frames[child].result {
                    *result = remap(&mut self.body.types, *result)?;
                }
                for ty in &mut self.frames[child].parameters {
                    *ty = remap(&mut self.body.types, *ty)?;
                }
                if self.frames[child].parent == Some(frame) {
                    self.frames[child].parent = Some(clause_frame);
                }
            }
        }
        Ok(Some((function, parameters)))
    }
}
/// Written evaluation children, excluding bodies with their own lexical frame.
pub(super) fn children(tree: &ast::SyntaxTree, id: ast::ExprId) -> Vec<ast::ExprId> {
    use ast::ExprKind as E;
    match &tree.expressions[id.0 as usize].kind {
        E::Tuple(es) | E::Sequence(es) => es.clone(),
        E::Call { callee, arguments } => std::iter::once(*callee)
            .chain(arguments.iter().map(|a| a.value))
            .collect(),
        E::Unary { value, .. } | E::Before(value) | E::Comptime(value) | E::Try { value, .. } => {
            vec![*value]
        }
        E::Field { base, .. } => vec![*base],
        E::Index { base, index } => vec![*base, *index],
        E::Binary { left, right, .. } => vec![*left, *right],
        E::If {
            condition,
            then_branch,
            else_branch,
        } => [Some(*condition), Some(*then_branch), *else_branch]
            .into_iter()
            .flatten()
            .collect(),
        E::Match { value, arms } => std::iter::once(*value)
            .chain(
                arms.iter()
                    .flat_map(|a| a.guard.into_iter().chain(std::iter::once(a.body))),
            )
            .collect(),
        E::Return(e) => e.iter().copied().collect(),
        E::Block { statements, tail } => statements
            .iter()
            .flat_map(|s| match &tree.statements[s.0 as usize].kind {
                ast::StmtKind::Let {
                    value, otherwise, ..
                } => std::iter::once(*value).chain(*otherwise).collect(),
                ast::StmtKind::Expr(value) => vec![*value],
                ast::StmtKind::Assign { place, value, .. } => vec![*place, *value],
                ast::StmtKind::While {
                    condition, body, ..
                } => vec![*condition, *body],
                ast::StmtKind::For { source, body, .. } => vec![*source, *body],
                ast::StmtKind::Function { .. } => vec![],
            })
            .chain(tail.iter().copied())
            .collect(),
        _ => vec![],
    }
}
