//! Isolated lowering fixtures with explicit declarations and analyzed facts.
use super::*;
use hirsel_intern::Symbol;
use hirsel_interp::{Execution, Limits, run};
use hirsel_span::{FileId, SourceMap};
use hirsel_syntax::*;
use hirsel_tir::{DefinitionRef, RunConfiguration, TirProvider};
use hirsel_types::*;
use std::sync::Arc;
pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}
pub fn site(line: u32) -> Span {
    Span::new(FileId(0), (line - 1) * 2, (line - 1) * 2 + 1).unwrap()
}
pub fn origin(line: u32) -> hirsel_syntax::Origin {
    hirsel_syntax::Origin {
        span: site(line),
        tokens: TokenRange::default(),
    }
}
pub struct Fixture {
    pub tree: SyntaxTree,
    pub checked: CheckedBody,
    pub plans: Plans,
    pub ownership: OwnershipFacts,
    pub result_types: Vec<(NameSite, TypeId)>,
    pub frames: Vec<(NameSite, TypeId)>,
    pub unit: TypeId,
    pub boolean: TypeId,
    pub scalar: TypeId,
    pub owner: NameSite,
    pub next_local: u32,
    pub declarations: hirsel_validate::Signatures,
    pub parameters: Vec<hirsel_types::Parameter>,
}
impl Fixture {
    pub fn new(primitive: Primitive) -> Self {
        let mut types = TypeStore::default();
        let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
        let boolean = types.push(ty(TypeKind::Primitive(Primitive::Bool)));
        let scalar = types.push(ty(TypeKind::Primitive(primitive)));
        Self {
            tree: SyntaxTree::default(),
            checked: CheckedBody {
                tails: vec![],
                places: vec![],
                calls: vec![],
                types,
                bindings: vec![],
                patterns: vec![],
                resolution: Resolution::default(),
                expressions: vec![],
                instantiations: vec![],
                closures: vec![],
            },
            plans: Plans::default(),
            ownership: OwnershipFacts::default(),
            result_types: vec![],
            frames: vec![],
            unit,
            boolean,
            scalar,
            owner: NameSite::Item(ItemId(0)),
            next_local: 0,
            declarations: Default::default(),
            parameters: vec![],
        }
    }
    pub fn type_(&mut self, kind: TypeKind) -> TypeId {
        self.checked.types.push(ty(kind))
    }
    pub fn expr(&mut self, kind: ExprKind, ty: TypeId, usage: UseKind, line: u32) -> ExprId {
        let id = ExprId(self.tree.expressions.len() as u32);
        self.tree.expressions.push(Expr {
            origin: origin(line),
            kind,
        });
        self.checked.expressions.push(Elaboration {
            expression: id,
            ty,
            usage,
            instantiated_arguments: vec![],
        });
        let root = hirsel_types::SourceRoot::Value(id);
        let mut place = hirsel_types::SourcePlace {
            expression: id,
            root,
            projections: vec![].into(),
            access: hirsel_types::SourceAccess::Read,
        };
        match &self.tree.expressions[id.0 as usize].kind {
            ExprKind::Field { base, field } => {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *base)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.projections.push(hirsel_types::SourceProjection {
                    expression: id,
                    kind: hirsel_types::SourceProjectionKind::Field(hirsel_types::FieldId(field.0)),
                });
            }
            ExprKind::Index { base, index } => {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *base)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.projections.push(hirsel_types::SourceProjection {
                    expression: id,
                    kind: hirsel_types::SourceProjectionKind::Index(*index),
                });
            }
            ExprKind::Unary { op, value }
                if matches!(
                    op,
                    UnaryOp::Copy | UnaryOp::Take | UnaryOp::Mut | UnaryOp::Unique
                ) =>
            {
                place = self
                    .checked
                    .places
                    .iter()
                    .find(|p| p.expression == *value)
                    .cloned()
                    .unwrap();
                place.expression = id;
                place.access = match op {
                    UnaryOp::Copy => hirsel_types::SourceAccess::Copy,
                    UnaryOp::Take => hirsel_types::SourceAccess::Take,
                    UnaryOp::Mut => hirsel_types::SourceAccess::Mut,
                    _ => hirsel_types::SourceAccess::Unique,
                };
            }
            _ => (),
        }
        self.checked.places.push(place);
        id
    }
    pub fn integer(&mut self, n: i128, line: u32) -> ExprId {
        if n < 0 {
            let value = self.integer(-n, line);
            self.expr(
                ExprKind::Unary {
                    op: UnaryOp::Neg,
                    value,
                },
                self.scalar,
                UseKind::Take,
                line,
            )
        } else {
            self.expr(
                ExprKind::Literal(Literal::Integer(n.to_string())),
                self.scalar,
                UseKind::Take,
                line,
            )
        }
    }
    pub fn bool_(&mut self, b: bool) -> ExprId {
        self.expr(
            ExprKind::Literal(Literal::Bool(b)),
            self.boolean,
            UseKind::Take,
            1,
        )
    }
    pub fn unit_(&mut self) -> ExprId {
        self.expr(ExprKind::Tuple(vec![]), self.unit, UseKind::Take, 1)
    }
    pub fn binary(&mut self, op: BinaryOp, left: ExprId, right: ExprId, line: u32) -> ExprId {
        let ty = if matches!(
            op,
            BinaryOp::Eq
                | BinaryOp::Ne
                | BinaryOp::Lt
                | BinaryOp::Le
                | BinaryOp::Gt
                | BinaryOp::Ge
                | BinaryOp::And
                | BinaryOp::Or
        ) {
            self.boolean
        } else {
            self.scalar
        };
        self.expr(
            ExprKind::Binary { op, left, right },
            ty,
            UseKind::Take,
            line,
        )
    }
    pub fn stmt(&mut self, kind: StmtKind, line: u32) -> StmtId {
        let id = StmtId(self.tree.statements.len() as u32);
        self.tree.statements.push(Stmt {
            origin: origin(line),
            kind,
        });
        id
    }
    pub fn binding(&mut self, ty: TypeId, usage: UseKind) -> (PatternId, ResolvedName) {
        let pattern = PatternId(self.tree.patterns.len() as u32);
        let local = hirsel_types::LocalId(self.next_local);
        self.next_local += 1;
        self.tree.patterns.push(Pattern {
            origin: origin(1),
            kind: PatternKind::Binding(Symbol(local.0)),
        });
        self.checked.patterns.push(PatternDecision {
            pattern,
            ty,
            usage,
            kind: PatternDecisionKind::Binding(local),
            fields: vec![],
            annotation: None,
        });
        let binding = ResolvedName::Local(local);
        self.checked.bindings.push(BindingDecision {
            owner: self.owner.clone(),
            binding: binding.clone(),
            site: NameSite::Pattern(pattern),
            ty,
            usage,
            mutable: true,
        });
        (pattern, binding)
    }
    pub fn let_(&mut self, value: ExprId) -> (StmtId, ResolvedName) {
        let ty = self.checked.expressions[value.0 as usize].ty;
        let (pattern, binding) = self.binding(ty, UseKind::Take);
        (
            self.stmt(
                StmtKind::Let {
                    mutable: true,
                    pattern,
                    ty: None,
                    value,
                    otherwise: None,
                },
                1,
            ),
            binding,
        )
    }
    pub fn read(&mut self, binding: ResolvedName, ty: TypeId, usage: UseKind) -> ExprId {
        let id = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![],
            }),
            ty,
            usage,
            1,
        );
        self.checked
            .resolution
            .expressions
            .push((id, binding.clone()));
        self.checked
            .places
            .iter_mut()
            .find(|p| p.expression == id)
            .unwrap()
            .root = hirsel_types::SourceRoot::Binding {
            owner: self.owner.clone(),
            binding,
        };
        self.checked
            .resolution
            .reference_scopes
            .push((NameSite::Expression(id), self.owner.clone()));
        id
    }
    pub fn parameter(&mut self, ordinal: u32, ty: TypeId, usage: UseKind) -> ResolvedName {
        assert_eq!(ordinal as usize, self.parameters.len());
        self.parameters.push(hirsel_types::Parameter {
            name: None,
            label: None,
            passing: passing(usage),
            ty,
        });
        let binding = ResolvedName::Parameter(ParamId(ordinal));
        self.checked.bindings.push(BindingDecision {
            owner: self.owner.clone(),
            binding: binding.clone(),
            site: self.owner.clone(),
            ty,
            usage,
            mutable: usage == UseKind::Mut,
        });
        binding
    }
    pub fn block(&mut self, statements: Vec<StmtId>, tail: ExprId) -> ExprId {
        let ty = self.checked.expressions[tail.0 as usize].ty;
        self.expr(
            ExprKind::Block {
                statements,
                tail: Some(tail),
            },
            ty,
            UseKind::Take,
            1,
        )
    }
    pub fn named_call(
        &mut self,
        name: &str,
        arguments: Vec<ExprId>,
        parameters: Vec<(TypeId, UseKind)>,
        result: TypeId,
        line: u32,
    ) -> ExprId {
        if !self.declarations.contains_key(&path(name)) {
            self.declare(name, parameters.clone(), result);
        }
        let callee = self.expr(
            ExprKind::Path(Path {
                package: None,
                segments: vec![],
            }),
            self.unit,
            UseKind::Borrow,
            line,
        );
        self.checked
            .resolution
            .expressions
            .push((callee, ResolvedName::Definition(path(name))));
        let uses: Vec<_> = arguments
            .iter()
            .copied()
            .zip(parameters.iter().map(|(_, usage)| *usage))
            .collect();
        let call = self.expr(
            ExprKind::Call {
                callee,
                arguments: arguments
                    .into_iter()
                    .map(|value| Argument { label: None, value })
                    .collect(),
            },
            result,
            UseKind::Take,
            line,
        );
        self.checked.calls.push(hirsel_types::CheckedCall {
            expression: call,
            target: hirsel_types::CallTarget::Definition(path(name)),
            arguments: uses
                .iter()
                .enumerate()
                .map(|(i, (expression, _))| hirsel_types::CheckedArgument {
                    expression: *expression,
                    parameter: Some((
                        i as u32,
                        hirsel_types::Parameter {
                            name: None,
                            label: None,
                            passing: passing(parameters[i].1),
                            ty: parameters[i].0,
                        },
                    )),
                })
                .collect(),
            generic_arguments: vec![],
            result,
            state: hirsel_types::CallState::Complete,
        });
        self.ownership.call_arguments.extend(
            uses.into_iter()
                .map(|(argument, usage)| (call, argument, usage)),
        );
        call
    }
    pub fn call_plan(
        &mut self,
        expression: ExprId,
        target: hirsel_types::CallTarget,
        ordinals: Vec<u32>,
    ) {
        let ExprKind::Call { callee, arguments } =
            &self.tree.expressions[expression.0 as usize].kind
        else {
            panic!("call fixture")
        };
        let parameters = if let Some(row) = self
            .checked
            .calls
            .iter()
            .find(|r| r.expression == expression)
        {
            let mut parameters = row
                .arguments
                .iter()
                .map(|a| a.parameter.clone().unwrap())
                .collect::<Vec<_>>();
            parameters.sort_by_key(|(i, _)| *i);
            parameters.into_iter().map(|(_, p)| p).collect::<Vec<_>>()
        } else {
            let hirsel_types::TypeKind::Function(f) = &self
                .checked
                .types
                .get(self.checked.expressions[callee.0 as usize].ty)
                .unwrap()
                .kind
            else {
                panic!("callable fixture")
            };
            f.parameters.to_vec()
        };
        let row = hirsel_types::CheckedCall {
            expression,
            target,
            arguments: arguments
                .iter()
                .zip(ordinals)
                .map(|(a, i)| hirsel_types::CheckedArgument {
                    expression: a.value,
                    parameter: Some((i, parameters[i as usize].clone())),
                })
                .collect(),
            generic_arguments: vec![],
            result: self.checked.expressions[expression.0 as usize].ty,
            state: hirsel_types::CallState::Complete,
        };
        self.checked.calls.retain(|r| r.expression != expression);
        self.checked.calls.push(row);
    }
    pub fn indirect_call_plan(&mut self, expression: ExprId, ordinals: Vec<u32>) {
        let ExprKind::Call { callee, .. } = self.tree.expressions[expression.0 as usize].kind
        else {
            panic!("call fixture")
        };
        self.call_plan(
            expression,
            hirsel_types::CallTarget::Value(callee),
            ordinals,
        );
    }
    pub fn assign(
        &mut self,
        place: ExprId,
        op: AssignOp,
        value: ExprId,
        content: TypeId,
        line: u32,
    ) -> StmtId {
        self.ownership
            .replacements
            .push((place, content, Some(true)));
        self.stmt(StmtKind::Assign { place, op, value }, line)
    }
    pub fn declare(&mut self, name: &str, parameters: Vec<(TypeId, UseKind)>, result: TypeId) {
        let function = function(
            parameters
                .into_iter()
                .map(|(ty, usage)| hirsel_types::Parameter {
                    name: None,
                    label: None,
                    passing: passing(usage),
                    ty,
                })
                .collect(),
            result,
        );
        self.register(Signature {
            metadata: SignatureMetadata::default(),
            definition: path(name),
            types: self.checked.types.clone(),
            kind: SignatureKind::Function(function),
            fingerprint: ContentHash::default(),
        });
    }
    pub fn register(&mut self, signature: Signature) {
        self.declarations
            .insert(signature.definition.clone(), signature);
    }
    pub fn nominal(&mut self, name: &str, representation: TypeId) {
        self.register(Signature {
            metadata: SignatureMetadata::default(),
            definition: path(name),
            types: self.checked.types.clone(),
            kind: SignatureKind::Type(TypeDefinition {
                loop_source: None,
                format: None,
                generics: vec![],
                representation: Some(representation),
                destructor_allocation: Allocation::NoAlloc,
                bounds: vec![],
                authority: AuthoritySummary::inert(),
                capability: None,
                implementation_authority: None,
                serializable: false,
                layout: Layout::default(),
                distinct: false,
                resource: false,
                drop: None,
                fact: None,
            }),
            fingerprint: ContentHash::default(),
        });
    }
    pub fn closure(&mut self, info: ClosureInfo, result: TypeId, capture_owner: NameSite) {
        self.frames
            .push((NameSite::Expression(info.expression), result));
        self.ownership
            .captures
            .extend(
                info.captures
                    .iter()
                    .enumerate()
                    .map(|(i, capture)| CaptureMode {
                        expression: info.expression,
                        ordinal: i as u32,
                        owner: capture_owner,
                        usage: capture.usage,
                        context: CaptureContext::Scoped,
                    }),
            );
        self.checked.closures.push(info);
    }
    pub fn input<'a>(
        &'a self,
        expression: ExprId,
        name: &str,
        result_types: &'a [(NameSite, TypeId)],
    ) -> Input<'a> {
        Input {
            track_caller: false,
            tree: &self.tree,
            checked: &self.checked,
            plans: &self.plans,
            ownership: &self.ownership,
            result_types,
            owner: self.owner,
            expression,
            path: path(name),
            signature_hash: ContentHash::default(),
            unit: self.unit,
            boolean: self.boolean,
            arena: None,
        }
    }
    pub fn lower(&self, expression: ExprId, name: &str, result: TypeId) -> Result<Definition> {
        let mut declarations = self.declarations.clone();
        declarations.entry(path(name)).or_insert_with(|| Signature {
            metadata: SignatureMetadata::default(),
            definition: path(name),
            types: self.checked.types.clone(),
            kind: SignatureKind::Function(function(self.parameters.clone(), result)),
            fingerprint: ContentHash::default(),
        });
        let mut root = vec![(self.owner, result)];
        root.extend(self.frames.clone());
        let input = self.input(
            expression,
            name,
            if self.result_types.is_empty() {
                &root
            } else {
                &self.result_types
            },
        );
        let definition = lower(input)?;
        hirsel_validate::assert_valid(&definition, &declarations);
        Ok(definition)
    }
}
pub fn ty(kind: TypeKind) -> Type {
    Type::with_kind(kind)
}
pub struct Provider(pub Vec<Definition>);
impl TirProvider for Provider {
    fn constant(
        &mut self,
        _path: &hirsel_syntax::DefPath,
    ) -> std::result::Result<hirsel_tir::MaterializedRef, hirsel_types::QueryError> {
        Err(hirsel_types::QueryError::NotImplemented)
    }
    fn constant_definitions(
        &mut self,
    ) -> std::result::Result<Vec<hirsel_syntax::DefPath>, hirsel_types::QueryError> {
        Ok(vec![])
    }

    fn definition(&mut self, path: &DefPath) -> std::result::Result<DefinitionRef, QueryError> {
        self.0
            .iter()
            .find(|d| &d.path == path)
            .cloned()
            .map(Arc::new)
            .ok_or_else(|| QueryError::NotImplemented)
    }
}
pub fn execute(definitions: Vec<Definition>, arguments: Vec<hirsel_interp::Value>) -> Execution {
    let mut sources = SourceMap::default();
    sources.add("fixture.hir", "x\n".repeat(100)).unwrap();
    run(
        &mut Provider(definitions),
        &sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        arguments,
        Limits::default(),
    )
    .unwrap()
}
pub fn returned(execution: Execution) -> hirsel_interp::Value {
    match execution.outcome {
        hirsel_interp::Outcome::Returned { value, .. } => value,
        outcome => panic!("unexpected {outcome:?}"),
    }
}
pub fn bits(execution: Execution) -> u128 {
    match returned(execution) {
        hirsel_interp::Value::Integer(i) => i.bits(),
        value => panic!("expected integer, got {value:?}"),
    }
}

pub fn passing(usage: UseKind) -> PassingMode {
    match usage {
        UseKind::Take => PassingMode::Take,
        UseKind::Mut => PassingMode::Mut,
        UseKind::Borrow | UseKind::Copy => PassingMode::Borrow,
    }
}
pub fn function(parameters: Vec<hirsel_types::Parameter>, result: TypeId) -> FunctionType {
    FunctionType {
        state_promises: Default::default(),
        generics: vec![].into(),
        parameters: parameters.into(),
        result,
        error: None,
        allocation: Allocation::MayAllocate,
        authority: AuthoritySummary::inert(),
        uses: Uses::Unrestricted,
        contracts: vec![].into(),
        incomplete: false,
        foreign: None,
    }
}

/// Runs deep fixtures on a thread with the compiler's stack size
/// (`hirsel_engine::COMPILER_STACK_BYTES`), without linking the driver.
pub fn with_large_stack<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(128 * 1024 * 1024)
            .spawn_scoped(scope, work)
            .expect("fixture thread creation failed")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
