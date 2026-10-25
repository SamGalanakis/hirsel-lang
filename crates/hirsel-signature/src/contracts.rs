use super::*;
use hirsel_types::encoding::Encoder;
impl Normalizer<'_, '_> {
    pub(super) fn contract(
        &mut self,
        kind: ast::ContractKind,
        id: ast::ExprId,
        ordinal: u32,
    ) -> Result<Contract, QueryError> {
        let mut e = Encoder::default();
        e.tag("hirsel.contract.encoding.v1");
        self.expression(id, &mut e, &mut BTreeSet::new(), &mut BTreeSet::new())?;
        // Include used signatures and earlier header reads as conservative inputs.
        // All of these reads also remain in the signature query dependencies.
        e.number(self.dependencies.len() as u64);
        for (definition, fingerprint) in self.ordered_dependency_rows() {
            e.path(definition.path());
            e.atom(&fingerprint.0);
        }
        Ok(Contract {
            kind,
            ordinal,
            code_hash: ast::ContentHash::of_parts(
                "hirsel.contract.sha256.v1",
                [e.bytes.as_slice()],
            ),
        })
    }
    fn contract_path(
        &mut self,
        p: &ast::Path,
        id: Option<ast::ExprId>,
        e: &mut Encoder,
        locals: &BTreeSet<String>,
    ) -> Result<(), QueryError> {
        // A dot path rooted in the declared parameter is a projection, not a
        // provider definition. Preserve each field spelling in contract identity.
        if p.package.is_none()
            && let Some(root) = p.segments.first()
            && let Some(ordinal) = self
                .parameters
                .iter()
                .position(|name| name.as_deref() == Some(root.as_str()))
        {
            e.tag("parameter");
            e.number(ordinal as u64);
            if p.segments.len() > 1 {
                e.tag("projection");
                e.number((p.segments.len() - 1) as u64);
                for field in &p.segments[1..] {
                    e.tag(field.as_str());
                }
            }
            return Ok(());
        }
        if let Some(id) = id {
            if let Some((_, name)) = self
                .input
                .resolution
                .expressions
                .iter()
                .find(|(x, _)| *x == id)
            {
                match name {
                    ResolvedName::Definition(p) => {
                        let p = p.clone();
                        let s = self.snapshot(&p)?;
                        e.tag("definition");
                        e.path(&p);
                        e.atom(&s.fingerprint.0);
                        return Ok(());
                    }
                    ResolvedName::Parameter(p) => {
                        e.tag("parameter");
                        e.number(p.0 as u64);
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
        if let Some(name) = self.path_name(p) {
            if locals.contains(name) {
                e.tag("local");
                e.tag(name);
                return Ok(());
            }

            if let Some(i) = self
                .parameters
                .iter()
                .position(|n| n.as_deref() == Some(name))
            {
                e.tag("parameter");
                e.number(i as u64);
                return Ok(());
            }
        }
        let d = self.definition(p, None)?;
        let s = self.snapshot(&d)?;
        e.tag("definition");
        e.path(&d);
        e.atom(&s.fingerprint.0);
        Ok(())
    }
    fn expression(
        &mut self,
        id: ast::ExprId,
        e: &mut Encoder,
        active: &mut BTreeSet<ast::ExprId>,
        locals: &mut BTreeSet<String>,
    ) -> Result<(), QueryError> {
        let expr = self
            .input
            .syntax
            .tree
            .expressions
            .get(id.0 as usize)
            .cloned()
            .ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "missing contract expression",
                )
            })?;
        if !active.insert(id) {
            return Err(diagnostic(
                const { RuleId::of("LANG-6") },
                expr.origin.span,
                "recursive contract syntax graph",
            ));
        }
        let old = self.span;
        self.span = expr.origin.span;
        match &expr.kind {
            ast::ExprKind::Literal(l) => {
                e.tag("literal");
                match l {
                    ast::Literal::Integer(s) => {
                        e.tag("integer");
                        e.tag(&s.replace('_', ""));
                    }
                    ast::Literal::Float(s) => {
                        e.tag("float");
                        e.tag(&s.replace('_', ""));
                    }
                    ast::Literal::Character(s) => {
                        e.tag("character");
                        e.tag(s);
                    }
                    ast::Literal::Byte(s) => {
                        e.tag("byte");
                        e.tag(s);
                    }
                    ast::Literal::String(s) => {
                        e.tag("string");
                        e.tag(s);
                    }
                    ast::Literal::Bytes(s) => {
                        e.tag("bytes");
                        e.tag(s);
                    }
                    ast::Literal::Bool(b) => {
                        e.tag("bool");
                        e.boolean(*b);
                    }
                }
            }
            ast::ExprKind::Path(p) => self.contract_path(p, Some(id), e, locals)?,
            ast::ExprKind::Tuple(xs) | ast::ExprKind::Sequence(xs) => {
                e.tag(if matches!(expr.kind, ast::ExprKind::Tuple(_)) {
                    "tuple"
                } else {
                    "sequence"
                });
                e.number(xs.len() as u64);
                for x in xs {
                    self.expression(*x, e, active, locals)?;
                }
            }
            ast::ExprKind::Call { callee, arguments } => {
                e.tag("call");
                self.expression(*callee, e, active, locals)?;
                e.number(arguments.len() as u64);
                for a in arguments {
                    e.option(a.label, |e, s| e.tag(s.as_str()));
                    self.expression(a.value, e, active, locals)?;
                }
            }
            ast::ExprKind::Field { base, field } => {
                e.tag("field");
                self.expression(*base, e, active, locals)?;
                e.tag(field.as_str());
            }
            ast::ExprKind::Index { base, index } => {
                e.tag("index");
                self.expression(*base, e, active, locals)?;
                self.expression(*index, e, active, locals)?;
            }
            ast::ExprKind::Unary { op, value } => {
                e.tag("unary");
                e.tag(match op {
                    ast::UnaryOp::Neg => "neg",
                    ast::UnaryOp::Not => "not",
                    ast::UnaryOp::BitNot => "bit-not",
                    ast::UnaryOp::Take => "take",
                    ast::UnaryOp::Copy => "copy",
                    ast::UnaryOp::Mut => "mut",
                    ast::UnaryOp::Unique => "unique",
                    ast::UnaryOp::Box => "box",
                });
                self.expression(*value, e, active, locals)?;
            }
            ast::ExprKind::Binary { op, left, right } => {
                e.tag("binary");
                e.tag(binary(*op));
                self.expression(*left, e, active, locals)?;
                self.expression(*right, e, active, locals)?;
            }
            ast::ExprKind::Try { value, map_error } => {
                e.tag("try");
                self.expression(*value, e, active, locals)?;
                e.boolean(map_error.is_some());
                if let Some(p) = map_error {
                    self.contract_path(p, None, e, locals)?;
                }
            }
            ast::ExprKind::Block {
                label,
                statements,
                tail,
            } => {
                // An unlabelled block keeps its encoding (LANG-11).
                if let Some(label) = label {
                    e.tag("label");
                    e.tag(label.as_str());
                }
                e.tag("block");
                let mut scoped = locals.clone();
                e.number(statements.len() as u64);
                for s in statements {
                    self.statement(*s, e, active, &mut scoped)?;
                }
                e.boolean(tail.is_some());
                if let Some(t) = tail {
                    self.expression(*t, e, active, &mut scoped)?;
                }
            }
            ast::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                e.tag("if");
                self.expression(*condition, e, active, locals)?;
                self.expression(*then_branch, e, active, locals)?;
                e.boolean(else_branch.is_some());
                if let Some(x) = else_branch {
                    self.expression(*x, e, active, locals)?;
                }
            }
            ast::ExprKind::Match { value, arms } => {
                e.tag("match");
                self.expression(*value, e, active, locals)?;
                e.number(arms.len() as u64);
                for a in arms {
                    let mut scoped = locals.clone();
                    self.pattern(a.pattern, e, &mut scoped, &mut BTreeSet::new())?;
                    e.boolean(a.guard.is_some());
                    if let Some(g) = a.guard {
                        self.expression(g, e, active, &mut scoped)?;
                    }
                    self.expression(a.body, e, active, &mut scoped)?;
                }
            }
            ast::ExprKind::Closure { signature, body } => {
                e.tag("closure");
                let f = self.function(signature, &Attributes::default(), true, None)?;
                e.function(&f, &self.types).map_err(|_| {
                    diagnostic(
                        const { RuleId::of("LANG-6") },
                        self.span,
                        "invalid contract closure signature",
                    )
                })?;
                let old_p = self.parameters.clone();
                self.parameters = f.parameters.iter().map(|p| p.name.clone()).collect();
                self.expression(*body, e, active, &mut locals.clone())?;
                self.parameters = old_p;
            }
            ast::ExprKind::Return(x) => {
                e.tag("return");
                e.boolean(x.is_some());
                if let Some(x) = x {
                    self.expression(*x, e, active, locals)?;
                }
            }
            ast::ExprKind::Break { label, value } => {
                e.tag("break");
                if let Some(label) = label {
                    e.tag("label");
                    e.tag(label.as_str());
                }
                // A valueless break keeps its encoding (LANG-11).
                if let Some(value) = value {
                    e.tag("value");
                    self.expression(*value, e, active, locals)?;
                }
            }
            ast::ExprKind::Continue(label) => {
                e.tag("continue");
                if let Some(label) = label {
                    e.tag("label");
                    e.tag(label.as_str());
                }
            }
            ast::ExprKind::Shape(t) => {
                e.tag("shape");
                let t = self.ty(*t, Position::Field)?;
                e.ty(t, &self.types).map_err(|_| {
                    diagnostic(
                        const { RuleId::of("LANG-6") },
                        self.span,
                        "invalid shape type",
                    )
                })?;
            }
            ast::ExprKind::Before(x) | ast::ExprKind::Comptime(x) => {
                e.tag(if matches!(expr.kind, ast::ExprKind::Before(_)) {
                    "before"
                } else {
                    "comptime"
                });
                self.expression(*x, e, active, locals)?;
            }
            ast::ExprKind::Heap => e.tag("heap"),
            ast::ExprKind::Formatter(step) => {
                e.tag("formatter");
                e.tag(match step {
                    ast::FormatStep::Append => "append",
                    ast::FormatStep::AppendSpec(_) => "append-spec",
                    ast::FormatStep::Mark => "mark",
                    ast::FormatStep::Pad(_) => "pad",
                    ast::FormatStep::Finish => "finish",
                });
            }
            ast::ExprKind::Result => e.tag("result"),
            ast::ExprKind::With { callee, bindings } => {
                e.tag("with");
                self.expression(*callee, e, active, locals)?;
                for binding in bindings {
                    e.tag(binding.name.as_str());
                    if let Some(package) = &binding.implementation.package {
                        e.tag(&format!("{}@{}", package.name, package.version));
                    }
                    for segment in &binding.implementation.segments {
                        e.tag(segment.as_str());
                    }
                }
            }
            ast::ExprKind::Todo(_) | ast::ExprKind::Error => {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "contract must contain complete written code",
                ));
            }
        }
        self.span = old;
        active.remove(&id);
        Ok(())
    }
    fn statement(
        &mut self,
        id: ast::StmtId,
        e: &mut Encoder,
        active: &mut BTreeSet<ast::ExprId>,
        locals: &mut BTreeSet<String>,
    ) -> Result<(), QueryError> {
        let s = self
            .input
            .syntax
            .tree
            .statements
            .get(id.0 as usize)
            .cloned()
            .ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "missing contract statement",
                )
            })?;
        match &s.kind {
            ast::StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
                otherwise,
            } => {
                e.tag(if otherwise.is_some() {
                    "let-else"
                } else {
                    "let"
                });
                e.boolean(*mutable);
                e.boolean(ty.is_some());
                if let Some(t) = ty {
                    let t = self.ty(*t, Position::Field)?;
                    e.ty(t, &self.types).map_err(|_| {
                        diagnostic(
                            const { RuleId::of("LANG-6") },
                            s.origin.span,
                            "invalid contract binding type",
                        )
                    })?;
                }
                self.expression(*value, e, active, locals)?;
                if let Some(otherwise) = otherwise {
                    self.expression(*otherwise, e, active, locals)?;
                }
                self.pattern(*pattern, e, locals, &mut BTreeSet::new())?;
            }
            ast::StmtKind::Assign { place, op, value } => {
                e.tag("assign");
                e.option(op.0, |e, b| e.tag(binary(b)));
                self.expression(*place, e, active, locals)?;
                self.expression(*value, e, active, locals)?;
            }
            ast::StmtKind::Expr(x) => {
                e.tag("expr");
                self.expression(*x, e, active, locals)?;
            }
            ast::StmtKind::While {
                condition,
                body,
                label,
            } => {
                e.tag("while");
                if let Some(label) = label {
                    e.tag("label");
                    e.tag(label.as_str());
                }
                self.expression(*condition, e, active, locals)?;
                self.expression(*body, e, active, locals)?;
            }
            ast::StmtKind::For {
                pattern,
                source,
                body,
                ..
            } => {
                e.tag("for");
                if let Some(label) = s.kind.loop_label() {
                    e.tag("label");
                    e.tag(label.as_str());
                }
                self.expression(*source, e, active, locals)?;
                let mut scoped = locals.clone();
                self.pattern(*pattern, e, &mut scoped, &mut BTreeSet::new())?;
                self.expression(*body, e, active, &mut scoped)?;
            }
            ast::StmtKind::Function {
                name,
                signature,
                body,
            } => {
                e.tag("local-function");
                e.tag(name.as_str());
                let f = self.function(signature, &Attributes::default(), true, None)?;
                e.function(&f, &self.types).map_err(|_| {
                    diagnostic(
                        const { RuleId::of("LANG-6") },
                        s.origin.span,
                        "invalid local function",
                    )
                })?;
                let old = self.parameters.clone();
                self.parameters = f.parameters.iter().map(|p| p.name.clone()).collect();
                self.expression(*body, e, active, locals)?;
                self.parameters = old;
                locals.insert(name.as_str().to_owned());
            }
        }
        Ok(())
    }
    fn pattern(
        &mut self,
        id: ast::PatternId,
        e: &mut Encoder,
        locals: &mut BTreeSet<String>,
        active: &mut BTreeSet<ast::PatternId>,
    ) -> Result<(), QueryError> {
        if !active.insert(id) {
            return Err(diagnostic(
                const { RuleId::of("LANG-6") },
                self.span,
                "recursive contract pattern",
            ));
        }
        let p = self
            .input
            .syntax
            .tree
            .patterns
            .get(id.0 as usize)
            .cloned()
            .ok_or_else(|| {
                diagnostic(
                    const { RuleId::of("LANG-6") },
                    self.span,
                    "missing contract pattern",
                )
            })?;
        match &p.kind {
            ast::PatternKind::Wildcard => e.tag("wildcard"),
            ast::PatternKind::Binding(s) => {
                let n = s.as_str().to_owned();
                e.tag("binding");
                e.tag(&n);
                locals.insert(n);
            }
            ast::PatternKind::Tuple(ps) => {
                e.tag("tuple-pattern");
                e.number(ps.len() as u64);
                for p in ps {
                    self.pattern(*p, e, locals, active)?;
                }
            }
            ast::PatternKind::Constructor { path, fields, rest } => {
                e.tag("constructor-pattern");
                self.contract_path(path, None, e, locals)?;
                e.boolean(*rest);
                e.number(fields.len() as u64);
                for f in fields {
                    e.boolean(f.label.is_some());
                    if let Some(s) = f.label {
                        e.tag(s.as_str());
                    }
                    self.pattern(f.pattern, e, locals, active)?;
                }
            }
            ast::PatternKind::Literal(l) => {
                e.tag("literal-pattern");
                literal_pattern(l, e);
            }
            ast::PatternKind::Range { start, end } => {
                e.tag("range-pattern");
                literal_pattern(start, e);
                literal_pattern(end, e);
            }
            ast::PatternKind::Or(alternatives) => {
                e.tag("or-pattern");
                e.number(alternatives.len() as u64);
                for alternative in alternatives {
                    self.pattern(*alternative, e, locals, active)?;
                }
            }
            ast::PatternKind::Error => {
                return Err(diagnostic(
                    const { RuleId::of("LANG-6") },
                    p.origin.span,
                    "invalid contract pattern",
                ));
            }
        }
        active.remove(&id);
        Ok(())
    }
}
fn binary(b: ast::BinaryOp) -> &'static str {
    match b {
        ast::BinaryOp::Add => "add",
        ast::BinaryOp::Sub => "sub",
        ast::BinaryOp::Mul => "mul",
        ast::BinaryOp::Div => "div",
        ast::BinaryOp::Rem => "rem",
        ast::BinaryOp::Shl => "shl",
        ast::BinaryOp::Shr => "shr",
        ast::BinaryOp::BitAnd => "bit-and",
        ast::BinaryOp::BitOr => "bit-or",
        ast::BinaryOp::BitXor => "bit-xor",
        ast::BinaryOp::Eq => "eq",
        ast::BinaryOp::Ne => "ne",
        ast::BinaryOp::Lt => "lt",
        ast::BinaryOp::Le => "le",
        ast::BinaryOp::Gt => "gt",
        ast::BinaryOp::Ge => "ge",
        ast::BinaryOp::And => "and",
        ast::BinaryOp::Or => "or",
        ast::BinaryOp::Range => "range",
        ast::BinaryOp::RangeInclusive => "range-inclusive",
    }
}

fn literal_pattern(l: &ast::Literal, e: &mut Encoder) {
    match l {
        ast::Literal::Integer(s) => {
            e.tag("integer");
            e.tag(&s.replace('_', ""));
        }
        ast::Literal::Float(s) => {
            e.tag("float");
            e.tag(&s.replace('_', ""));
        }
        ast::Literal::Character(s) => {
            e.tag("character");
            e.tag(s);
        }
        ast::Literal::Byte(s) => {
            e.tag("byte");
            e.tag(s);
        }
        ast::Literal::String(s) => {
            e.tag("string");
            e.tag(s);
        }
        ast::Literal::Bytes(s) => {
            e.tag("bytes");
            e.tag(s);
        }
        ast::Literal::Bool(b) => {
            e.tag("bool");
            e.boolean(*b);
        }
    }
}
