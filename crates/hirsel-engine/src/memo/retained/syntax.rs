//! Source-only AST storage retained by an S0 revision index. Adapt the existing
//! allocative-style capacity visitor; symbols/DefIds stay with session interners.
use super::Weight;
use hirsel_syntax as ast;

pub(crate) fn retained_syntax_bytes(syntax: &ast::ItemSyntax) -> u64 {
    let mut weight = Weight::default();
    weight.add(super::arc_bytes(syntax));
    weight.syntax(syntax);
    weight.bytes
}
impl Weight<'_> {
    fn ast_path(&mut self, path: &ast::Path) {
        let ast::Path { package, segments } = path;
        if let Some(package) = package {
            self.package(package);
        }
        self.flat_vec(segments);
    }
    fn ast_literal(&mut self, literal: &ast::Literal) {
        match literal {
            ast::Literal::Integer(s)
            | ast::Literal::Float(s)
            | ast::Literal::Character(s)
            | ast::Literal::Byte(s)
            | ast::Literal::String(s)
            | ast::Literal::Bytes(s) => self.add(s.capacity()),
            ast::Literal::Bool(_) => (),
        }
    }
    fn ast_generic(&mut self, generic: &ast::GenericParam) {
        let ast::GenericParam { name: _, kind } = generic;
        match kind {
            ast::GenericKind::Type(bounds) => self.flat_vec(bounds),
            ast::GenericKind::Size => (),
            ast::GenericKind::Implementation { signature, sharing } => {
                self.ast_path(signature);
                self.flat_vec(sharing);
            }
        }
    }
    fn ast_attribute(&mut self, attribute: &ast::Attribute) {
        let ast::Attribute {
            name: _,
            arguments,
            origin: _,
        } = attribute;
        self.vec(arguments, |w, argument| match argument {
            ast::AttributeArgument::LoopSource(source) => {
                let ast::LoopSourceSyntax {
                    open,
                    next,
                    origin: _,
                } = source;
                w.ast_path(open);
                w.ast_path(next);
            }
            ast::AttributeArgument::Format(association) => w.ast_path(&association.function),
            ast::AttributeArgument::SerializableCheck(association) => {
                w.ast_path(&association.function)
            }
            ast::AttributeArgument::StatePromise(promise) => {
                w.vec(&promise.selectors, Self::ast_path)
            }
            ast::AttributeArgument::Integer(s) | ast::AttributeArgument::String(s) => {
                w.add(s.capacity())
            }
            ast::AttributeArgument::Name(_) | ast::AttributeArgument::Keyword(_) => (),
        });
    }
    fn ast_function(&mut self, function: &ast::FunctionSyntax) {
        let ast::FunctionSyntax {
            attributes,
            generics,
            parameters,
            result: _,
            uses,
            contracts,
        } = function;
        self.vec(attributes, Self::ast_attribute);
        self.vec(generics, Self::ast_generic);
        self.flat_vec(parameters);
        if let Some(paths) = uses {
            self.vec(paths, Self::ast_path);
        }
        self.flat_vec(contracts);
    }
    fn ast_expression(&mut self, expression: &ast::Expr) {
        match &expression.kind {
            ast::ExprKind::Literal(literal) => self.ast_literal(literal),
            ast::ExprKind::Path(path) => self.ast_path(path),
            ast::ExprKind::Tuple(elements) | ast::ExprKind::Sequence(elements) => {
                self.flat_vec(elements)
            }
            ast::ExprKind::Call { arguments, .. } => self.flat_vec(arguments),
            ast::ExprKind::Try { map_error, .. } => {
                if let Some(path) = map_error {
                    self.ast_path(path);
                }
            }
            ast::ExprKind::Block { statements, .. } => self.flat_vec(statements),
            ast::ExprKind::Match { arms, .. } => self.flat_vec(arms),
            ast::ExprKind::Closure { signature, .. } => self.ast_function(signature),
            ast::ExprKind::Todo(text) => {
                if let Some(text) = text {
                    self.add(text.capacity());
                }
            }
            ast::ExprKind::With { bindings, .. } => {
                self.vec(bindings, |w, binding| w.ast_path(&binding.implementation))
            }
            ast::ExprKind::Field { .. }
            | ast::ExprKind::Index { .. }
            | ast::ExprKind::Unary { .. }
            | ast::ExprKind::Binary { .. }
            | ast::ExprKind::If { .. }
            | ast::ExprKind::Return(_)
            | ast::ExprKind::Break { .. }
            | ast::ExprKind::Continue(_)
            | ast::ExprKind::Shape(_)
            | ast::ExprKind::Before(_)
            | ast::ExprKind::Comptime(_)
            | ast::ExprKind::Heap
            | ast::ExprKind::Result
            | ast::ExprKind::Formatter(_)
            | ast::ExprKind::Error => (),
        }
    }
    fn ast_pattern(&mut self, pattern: &ast::Pattern) {
        match &pattern.kind {
            ast::PatternKind::Literal(literal) => self.ast_literal(literal),
            ast::PatternKind::Range { start, end } => {
                self.ast_literal(start);
                self.ast_literal(end);
            }
            ast::PatternKind::Tuple(elements) | ast::PatternKind::Or(elements) => {
                self.flat_vec(elements)
            }
            ast::PatternKind::Constructor { path, fields, .. } => {
                self.ast_path(path);
                self.flat_vec(fields);
            }
            ast::PatternKind::Wildcard | ast::PatternKind::Binding(_) | ast::PatternKind::Error => {
                ()
            }
        }
    }
    fn ast_type(&mut self, ty: &ast::TypeExpr) {
        match &ty.kind {
            ast::TypeExprKind::Named { path, arguments } => {
                self.ast_path(path);
                self.flat_vec(arguments);
            }
            ast::TypeExprKind::Tuple(elements) => self.flat_vec(elements),
            ast::TypeExprKind::Function(signature) => self.ast_function(signature),
            ast::TypeExprKind::Qualified {
                modifiers,
                brand,
                relation,
                ..
            } => {
                self.flat_vec(modifiers);
                if let Some(brand) = brand {
                    self.ast_path(&brand.path);
                }
                if let Some(relation) = relation {
                    self.vec(&relation.places, |w, place| match place {
                        ast::RelationPlace::Path(path) => w.ast_path(path),
                        ast::RelationPlace::Heap | ast::RelationPlace::Environment => (),
                    });
                }
            }
            ast::TypeExprKind::Box(_) | ast::TypeExprKind::Error => (),
        }
    }
    pub(crate) fn syntax(&mut self, syntax: &ast::ItemSyntax) {
        let ast::ItemSyntax {
            item,
            tree,
            anchor: _,
            text_hash: _,
            occurrence: _,
        } = syntax;
        let ast::Item {
            name_span: _,
            origin: _,
            name: _,
            definition,
            visibility: _,
            attributes,
            kind,
            generated,
        } = item;
        self.def_path(definition);
        self.vec(attributes, Self::ast_attribute);
        match kind {
            ast::ItemKind::Function { signature, abi, .. } => {
                self.ast_function(signature);
                if let Some(abi) = abi {
                    self.add(abi.literal.capacity());
                    if let Some(contract) = &abi.contract {
                        self.add(contract.capacity());
                    }
                }
            }
            ast::ItemKind::Type {
                generics,
                bounds,
                authority,
                body,
                ..
            } => {
                self.vec(generics, Self::ast_generic);
                if let Some(bounds) = bounds {
                    self.flat_vec(bounds);
                }
                if let Some(authority) = authority {
                    self.vec(authority, Self::ast_path);
                }
                match body {
                    ast::TypeBody::Record(fields) => self.flat_vec(fields),
                    ast::TypeBody::Variants(variants) => {
                        self.vec(variants, |w, variant| w.flat_vec(&variant.fields))
                    }
                    ast::TypeBody::Alias(_)
                    | ast::TypeBody::Distinct { .. }
                    | ast::TypeBody::Opaque => (),
                }
            }
            ast::ItemKind::Signature { members } => self.vec(members, |w, member| match member {
                ast::MemberSpec::Type {
                    bounds, authority, ..
                } => {
                    w.flat_vec(bounds);
                    if let Some(authority) = authority {
                        w.vec(authority, Self::ast_path);
                    }
                }
                ast::MemberSpec::Operation { signature, .. } => w.ast_function(signature),
                ast::MemberSpec::Size { .. } => (),
            }),
            ast::ItemKind::Implementation { signature, members } => {
                self.ast_path(signature);
                self.vec(members, |w, member| match member {
                    ast::MemberDefinition::Operation { target, .. } => w.ast_path(target),
                    ast::MemberDefinition::Type { .. } | ast::MemberDefinition::Size { .. } => (),
                });
            }
            ast::ItemKind::Const { .. } => (),
        }
        if let Some(generated) = generated {
            match generated {
                ast::Generated::Body(names) => self.flat_vec(names),
                ast::Generated::Generator(names) => self.flat_vec(names),
                ast::Generated::Specialization(bindings) => {
                    self.vec(bindings, |w, (_, path)| w.def_path(path))
                }
            }
        }
        let ast::SyntaxTree {
            tokens,
            expressions,
            statements,
            patterns,
            types,
            formats,
            repeats,
            conditions,
            groups,
            analyses,
        } = tree;
        self.flat_vec(tokens);
        self.vec(expressions, Self::ast_expression);
        self.vec(statements, |w, statement| match &statement.kind {
            ast::StmtKind::Function { signature, .. } => w.ast_function(signature),
            ast::StmtKind::Let { .. }
            | ast::StmtKind::Assign { .. }
            | ast::StmtKind::Expr(_)
            | ast::StmtKind::While { .. }
            | ast::StmtKind::For { .. } => (),
        });
        self.vec(patterns, Self::ast_pattern);
        self.vec(types, Self::ast_type);
        self.vec(formats, |w, format| {
            let ast::FormatString {
                expression: _,
                pieces,
                holes,
                specs,
            } = format;
            w.vec(pieces, |w, value| w.add(value.capacity()));
            w.flat_vec(holes);
            w.vec(specs, |w, value| w.add(value.capacity()));
        });
        self.flat_vec(repeats);
        self.flat_vec(conditions);
        self.flat_vec(groups);
        self.vec(analyses, |w, annotation| {
            let ast::AnalysisAnnotation {
                site: _,
                name,
                payload,
                spaced,
                origin: _,
            } = annotation;
            w.add(name.capacity());
            w.vec(payload, |w, token| w.add(token.capacity()));
            w.flat_vec(spaced);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_intern::Symbol;
    use hirsel_span::{Anchor, Span};
    use std::sync::Arc;
    fn fixture(width: usize) -> ast::ItemSyntax {
        let origin = ast::Origin {
            span: Span::default(),
            tokens: Default::default(),
        };
        let path = ast::Path {
            package: Some(ast::PackageId {
                name: "source".into(),
                version: "1".into(),
            }),
            segments: vec![Symbol::intern("nested"); width],
        };
        let signature = ast::FunctionSyntax {
            attributes: vec![ast::Attribute {
                name: Symbol::intern("state"),
                origin,
                arguments: vec![
                    ast::AttributeArgument::StatePromise(ast::StatePromiseSyntax {
                        kind: ast::StatePromiseKind::PreserveCaptures,
                        selectors: vec![path.clone(); width],
                        origin,
                    }),
                    ast::AttributeArgument::String("payload".repeat(width)),
                ],
            }],
            generics: vec![ast::GenericParam {
                name: Symbol::intern("T"),
                kind: ast::GenericKind::Implementation {
                    signature: path.clone(),
                    sharing: Vec::with_capacity(width),
                },
            }],
            uses: Some(vec![path.clone(); width]),
            ..Default::default()
        };
        ast::ItemSyntax {
            item: ast::Item {
                name_span: origin.span,
                origin,
                name: Symbol::intern("main"),
                definition: ast::DefPath::core("source.memory.main"),
                visibility: ast::Visibility::Private,
                attributes: signature.attributes.clone(),
                kind: ast::ItemKind::Function {
                    signature: signature.clone(),
                    body: Some(ast::ExprId(0)),
                    abi: Some(ast::ForeignAbi {
                        literal: "\"C\"".into(),
                        contract: Some("contract".repeat(width)),
                    }),
                },
                generated: Some(ast::Generated::Specialization(vec![
                    (
                        Symbol::intern("I"),
                        ast::DefPath::core("source.binding")
                    );
                    width
                ])),
            },
            tree: ast::SyntaxTree {
                expressions: vec![
                    ast::Expr {
                        origin,
                        kind: ast::ExprKind::Closure {
                            signature: signature.clone(),
                            body: ast::ExprId(0),
                        },
                    },
                    ast::Expr {
                        origin,
                        kind: ast::ExprKind::Literal(ast::Literal::String("literal".repeat(width))),
                    },
                    ast::Expr {
                        origin,
                        kind: ast::ExprKind::Call {
                            callee: ast::ExprId(0),
                            arguments: Vec::with_capacity(width),
                        },
                    },
                ],
                statements: vec![ast::Stmt {
                    origin,
                    kind: ast::StmtKind::Function {
                        name: Symbol::intern("inner"),
                        signature: signature.clone(),
                        body: ast::ExprId(0),
                    },
                }],
                patterns: vec![ast::Pattern {
                    origin,
                    kind: ast::PatternKind::Constructor {
                        path: path.clone(),
                        fields: Vec::with_capacity(width),
                        rest: false,
                    },
                }],
                types: vec![ast::TypeExpr {
                    origin,
                    kind: ast::TypeExprKind::Qualified {
                        base: ast::TypeExprId(0),
                        modifiers: vec![ast::TypeModifier::Unique; width],
                        brand: Some(ast::BrandSyntax {
                            sibling: true,
                            path: path.clone(),
                        }),
                        relation: Some(ast::RelationSyntax {
                            places: vec![ast::RelationPlace::Path(path); width],
                            origin,
                        }),
                    },
                }],
                formats: vec![ast::FormatString {
                    expression: ast::ExprId(0),
                    pieces: vec!["piece".repeat(width)],
                    holes: vec![ast::ExprId(0)],
                    specs: vec![":>8".into()],
                }],
                analyses: vec![ast::AnalysisAnnotation {
                    site: ast::AnalysisSite::Item,
                    name: "analysis".into(),
                    payload: vec!["value".repeat(width)],
                    spaced: vec![true; width],
                    origin,
                }],
                ..Default::default()
            },
            anchor: Anchor(0),
            text_hash: Default::default(),
            occurrence: 0,
        }
    }
    #[test]
    fn indexed_syntax_inspection_covers_item_slots_and_owned_paths_without_query_work() {
        for width in [1, 4, 16] {
            let input = fixture(width);
            let mut engine =
                crate::QueryEngine::new(crate::default_environment(), Default::default());
            let (_, storage) = crate::test_alloc::measured(|| {
                for i in 0..width {
                    let mut syntax = input.clone();
                    syntax.item.definition.0.name = format!("item{i}");
                    engine
                        .items
                        .insert(syntax.item.definition.clone(), vec![Arc::new(syntax)]);
                }
            });
            let before = engine.work_stats();
            let (bound, inspection) = crate::test_alloc::measured(|| engine.indexed_syntax_bytes());
            assert_eq!(inspection.allocations, 0);
            assert_eq!(engine.work_stats(), before);
            assert!(
                storage.live as u64 <= bound.unwrap(),
                "width {width}: actual {} exceeds {:?}",
                storage.live,
                bound
            );
            let (_, release) = crate::test_alloc::measured(|| engine.items.clear());
            assert_eq!(-release.live, storage.live);
        }
    }
    #[test]
    fn source_syntax_hold_bound_covers_nested_physical_storage_without_allocating() {
        for width in [1, 4, 16] {
            let input = fixture(width);
            let (bound, inspection) = crate::test_alloc::measured(|| retained_syntax_bytes(&input));
            assert_eq!(inspection.allocations, 0);
            let (copy, storage) = crate::test_alloc::measured(|| Arc::new(input.clone()));
            assert!(
                storage.live as u64 <= bound,
                "width {width}: actual {} exceeds {bound}",
                storage.live
            );
            assert_eq!(*copy, input);
            let (_, release) = crate::test_alloc::measured(|| drop(copy));
            assert_eq!(-release.live, storage.live);
        }
    }
}
