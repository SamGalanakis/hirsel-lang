//! Immutable inputs of a typed generator. Constructing its private constant
//! and interpreting its Result needs no session or mutable source map. Like
//! rustc's per-owner tasks (rustc_hir_analysis/src/lib.rs:166-220), publication
//! of generated syntax remains separate from a worker's preparation/execution.
use super::*;

pub(crate) struct GeneratorInput {
    pub(crate) syntax: Arc<ItemSyntax>,
    pub(crate) value: ExprId,
    site: Span,
}
/// The existing immutable phase adapters; no mutable session crosses this seam.
pub(crate) struct GeneratorPhases<'a> {
    pub(crate) signature: &'a dyn SignaturePhase,
    pub(crate) resolver: &'a dyn ResolverPhase,
    pub(crate) checker: &'a dyn CheckerPhase,
    pub(crate) modes: &'a dyn ModesPhase,
}
pub(crate) struct GeneratorBody {
    pub(crate) analyzed: AnalyzedBody,
    pub(crate) signature: Signature,
    pub(crate) ty: hirsel_types::TypeId,
}
impl GeneratorInput {
    pub(crate) fn new(
        definition: &DefPath,
        snapshot: &Arc<ItemSyntax>,
        body: ExprId,
    ) -> Result<Self, QueryError> {
        let source = &snapshot.item;
        let ItemKind::Function { signature, .. } = &source.kind else {
            return Err(QueryError::NotImplemented);
        };
        let ExprKind::Comptime(value) = snapshot.tree.expressions[body.0 as usize].kind else {
            return Err(QueryError::NotImplemented);
        };
        let site = snapshot.tree.expressions[body.0 as usize].origin;
        let helper = renamed(definition, "generator");
        // The helper reads the declaration's own arenas, at its anchor.
        let mut file = (**snapshot).clone();
        file.occurrence = 0;
        let core = |name: &str| Path {
            package: Some(PackageId {
                name: "core".into(),
                version: "1".into(),
            }),
            segments: vec![Symbol::intern(name)],
        };
        let push = |kind: TypeExprKind, tree: &mut hirsel_syntax::SyntaxTree| {
            tree.types.push(TypeExpr { origin: site, kind });
            TypeExprId(tree.types.len() as u32 - 1)
        };
        // F: the header's parameters with their passing modes, result and `uses`,
        // without uniqueness, which the declaration alone decides.
        for ty in signature
            .parameters
            .iter()
            .map(|p| p.ty)
            .chain(signature.result)
        {
            without_uniqueness(&mut file.tree, ty);
        }
        let function = push(
            TypeExprKind::Function(FunctionSyntax {
                parameters: signature.parameters.clone(),
                result: signature.result,
                uses: signature.uses.clone(),
                ..Default::default()
            }),
            &mut file.tree,
        );
        let body_type = push(
            TypeExprKind::Named {
                path: core("Body"),
                arguments: vec![TypeArgument::Type(function)],
            },
            &mut file.tree,
        );
        let string = push(
            TypeExprKind::Named {
                path: core("String"),
                arguments: vec![],
            },
            &mut file.tree,
        );
        let ty = push(
            TypeExprKind::Named {
                path: core("Result"),
                arguments: vec![TypeArgument::Type(body_type), TypeArgument::Type(string)],
            },
            &mut file.tree,
        );
        let mut item = source.clone();
        item.name = Symbol::intern(&helper.0.name);
        item.definition = helper.clone();
        item.visibility = Visibility::Private;
        item.attributes.clear();
        item.kind = ItemKind::Const { ty, value };
        item.generated = Some(Generated::Generator(
            signature.parameters.iter().filter_map(|p| p.name).collect(),
        ));
        file.item = item;
        Ok(Self {
            syntax: Arc::new(file),
            value,
            site: site.span,
        })
    }
    /// Normalize and check the private constant with a caller-owned provider.
    /// Its complete products remain explicit, including a phase that fails.
    pub(crate) fn check(
        &self,
        types: &hirsel_types::TypeInterner,
        memo: &hirsel_types::TypeMemo,
        declarations: &Declarations,
        phases: &GeneratorPhases<'_>,
        products: &mut PhaseProducts,
        provider: &mut dyn SignatureProvider,
    ) -> Result<GeneratorBody, QueryError> {
        *products = PhaseProducts::default();
        let constant = phases.signature.signature(
            types,
            memo,
            self.syntax.clone(),
            declarations,
            products,
            provider,
        )?;
        if has_errors(&constant.diagnostics) {
            return Err(QueryError::Diagnostics(constant.diagnostics));
        }
        let mut constant = constant.value;
        constant.fingerprint =
            hirsel_types::encoding::fingerprint(types, &constant, &[]).map_err(|error| {
                QueryError::Operational(hirsel_types::OperationalFailure {
                    operation: "typed generation".into(),
                    message: format!("generator signature: {error:?}"),
                })
            })?;
        *products = PhaseProducts::default();
        let resolution = phases.resolver.resolve(
            types,
            memo,
            self.syntax.clone(),
            declarations,
            products,
            provider,
        )?;
        if has_errors(&resolution.diagnostics) {
            return Err(QueryError::Diagnostics(resolution.diagnostics));
        }
        let typed = phases.checker.check(
            types,
            memo,
            self.syntax.clone(),
            resolution.value,
            &constant,
            provider,
        )?;
        if has_errors(&typed.diagnostics) {
            return Err(QueryError::Diagnostics(typed.diagnostics));
        }
        let analyzed = phases.modes.check_modes(
            types,
            memo,
            Arc::new(typed.value),
            &constant,
            products,
            provider,
        )?;
        if has_errors(&analyzed.diagnostics) {
            return Err(QueryError::Diagnostics(analyzed.diagnostics));
        }
        let SignatureKind::Const { ty, .. } = constant.kind else {
            return Err(QueryError::NotImplemented);
        };
        let ty = analyzed
            .value
            .typed
            .checked
            .expressions
            .iter()
            .find(|row| row.expression == self.value)
            .map_or(ty, |row| row.ty);
        Ok(GeneratorBody {
            analyzed: analyzed.value,
            signature: constant,
            ty,
        })
    }
    pub(crate) fn decode(&self, result: hirsel_interp::Value) -> Result<String, QueryError> {
        let site = self.site;
        let unexpected = || {
            QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "typed generation".into(),
                message: "generator returned no Result[Body, String]".into(),
            })
        };
        let hirsel_interp::Value::Variant { variant, fields } = result else {
            return Err(unexpected());
        };
        let Some(Some(hirsel_interp::Value::String(text))) = fields.into_iter().next() else {
            return Err(unexpected());
        };
        match variant {
            0 => Ok(text),
            _ => {
                let mut d = Diagnostic::error(
                    const { hirsel_diag::RuleId::of("CT-1") },
                    site,
                    "the generator returned an error",
                );
                d.notes.push(text);
                Err(QueryError::Diagnostics(vec![d]))
            }
        }
    }
}

/// A worker's tentative generator success. Source files and check memos are
/// published only after the serial merger verifies its complete input record.
#[derive(Clone)]
pub(crate) struct GeneratedText {
    pub(crate) definition: DefPath,
    pub(crate) syntax: Arc<ItemSyntax>,
    pub(crate) body: ExprId,
    pub(crate) text: Result<String, QueryError>,
    pub(crate) reads: Reads,
    pub(crate) detail: DependencyReads,
    pub(crate) depth: crate::memo::DepthEvidence<Span>,
    pub(crate) products: Arc<PhaseProducts>,
    pub(crate) acceptance: Option<hirsel_validate::ExecutionReceipt>,
    pub(crate) selections: Vec<crate::comptime::Selection>,
}
pub(crate) struct PreparedGenerator {
    pub(crate) syntax: Arc<ItemSyntax>,
    pub(crate) body: ExprId,
    pub(crate) input: GeneratorInput,
    pub(crate) checked: Result<GeneratorBody, QueryError>,
    pub(crate) detail: DependencyReads,
    pub(crate) products: Arc<PhaseProducts>,
}
