//! GEN-6 declaration graphs. Callable publication never executes an initializer.
use crate::comptime::{ComptimeValueSource, EvaluationInput};
use crate::*;
use hirsel_span::Span;
use hirsel_syntax::{ExprId, ExprKind, ItemKind, Visibility};
use hirsel_tir::{Constructor, Materialized, ValueKind, ValueNode};
use hirsel_types::{Bound, GenericBinder, SignatureKind, Substitution, TypeKind};
use std::sync::Arc;

fn rejected(site: Span, message: &str) -> QueryError {
    QueryError::Diagnostics(vec![Diagnostic::error(
        const { RuleId::of("GEN-6") },
        site,
        message,
    )])
}

pub(crate) fn literal_path(owner: &DefPath, expression: ExprId) -> DefPath {
    let mut path = owner.clone();
    path.0.name = format!("$dictionary${}${}", path.0.name, expression.0);
    path
}

/// A lifted literal's complete header comes from its declared receiving field,
/// so omitted literal result annotations retain ordinary expected-type inference.
/// This reads only the constant and record headers, never their implementations.
pub(crate) fn literal_signature(
    types: &hirsel_types::TypeInterner,
    memo: &hirsel_types::TypeMemo,
    syntax: &ItemSyntax,
    provider: &mut dyn hirsel_types::SignatureProvider,
) -> Result<Option<QueryOutput<Signature>>, QueryError> {
    let Some(name) = syntax.item.definition.0.name.strip_prefix("$dictionary$") else {
        return Ok(None);
    };
    let Some((owner, expression)) = name.rsplit_once('$') else {
        return Err(QueryError::NotImplemented);
    };
    let expression = ExprId(expression.parse().map_err(|_| QueryError::NotImplemented)?);
    let label = syntax
        .tree
        .expressions
        .iter()
        .find_map(|node| match &node.kind {
            ExprKind::Call { arguments, .. } => arguments
                .iter()
                .find(|a| a.value == expression)
                .and_then(|a| a.label),
            _ => None,
        })
        .ok_or(QueryError::NotImplemented)?;
    let mut owner_path = syntax.item.definition.clone();
    owner_path.0.name = owner.into();
    let constant = provider.signature(DefId::intern(&owner_path))?;
    let SignatureKind::Const { ty, .. } = constant.kind else {
        return Err(QueryError::NotImplemented);
    };
    let TypeKind::Named {
        definition,
        ref arguments,
    } = types.row(ty).kind
    else {
        return Ok(None);
    };
    let nominal = provider.signature(definition)?;
    let SignatureKind::Type(record) = &nominal.kind else {
        return Err(QueryError::NotImplemented);
    };
    let Some(representation) = record.representation else {
        return Ok(None);
    };
    let representation = hirsel_types::substitute(
        types,
        memo,
        representation,
        &Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &nominal.metadata.roles,
            arguments,
            authorities: &vec![hirsel_types::AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .map_err(|_| QueryError::NotImplemented)?;
    let TypeKind::Record(fields) = &types.row(representation).kind else {
        return Ok(None);
    };
    let field = fields
        .iter()
        .find(|field| field.name == label)
        .ok_or(QueryError::NotImplemented)?;
    let TypeKind::Function(function) = &types.row(field.ty).kind else {
        return Ok(None);
    };
    let dependencies = [&constant, &nominal]
        .into_iter()
        .map(|s| hirsel_types::SignatureDependency {
            definition: s.definition,
            fingerprint: s.fingerprint,
        })
        .collect::<Vec<_>>();
    let mut signature = Signature {
        definition: DefId::intern(&syntax.item.definition),
        metadata: hirsel_types::SignatureMetadata {
            roles: constant.metadata.roles.clone(),
            ..Default::default()
        },
        kind: SignatureKind::Function(function.clone()),
        fingerprint: ContentHash::default(),
    };
    signature.fingerprint = hirsel_types::encoding::fingerprint(types, &signature, &dependencies)
        .map_err(|_| QueryError::NotImplemented)?;
    Ok(Some(QueryOutput {
        value: signature,
        diagnostics: vec![],
        dependencies,
    }))
}

fn projected_constant(
    analyzed: &AnalyzedBody,
    expression: ExprId,
    constants: &[(ExprId, Materialized)],
) -> Option<ValueNode> {
    if let Some((_, value)) = constants.iter().find(|(id, _)| *id == expression) {
        return Some(value.root.clone());
    }
    let ExprKind::Field { base, .. } =
        analyzed.typed.syntax.tree.expressions[expression.0 as usize].kind
    else {
        return None;
    };
    let base = projected_constant(analyzed, base, constants)?;
    let projection = analyzed
        .typed
        .checked
        .places
        .iter()
        .find(|place| place.expression == expression)?
        .projections
        .last()?;
    let hirsel_types::SourceProjectionKind::Field(field) = projection.kind else {
        return None;
    };
    let ValueKind::Construct { fields, .. } = base.kind else {
        return None;
    };
    fields.get(field.0 as usize).cloned()
}

/// Retain the original closure for ordinary closure checking, including its
/// own-parameter relation restriction. Its capture-free body also gets an
/// ordinary private declaration, so every consumer uses a named callable.
/// This runs on engine-owned syntax, leaving the lossless parser untouched.
pub(crate) fn literal_snapshots(syntax: &ItemSyntax) -> Vec<Arc<ItemSyntax>> {
    let mut lifted = vec![];
    let ItemKind::Const { value, .. } = syntax.item.kind else {
        return lifted;
    };
    let ExprKind::Call { arguments, .. } = &syntax.tree.expressions[value.0 as usize].kind else {
        return lifted;
    };
    for argument in arguments {
        let ExprKind::Closure { signature, body } =
            &syntax.tree.expressions[argument.value.0 as usize].kind
        else {
            continue;
        };
        let mut helper = syntax.clone();
        helper.item.definition = literal_path(&syntax.item.definition, argument.value);
        helper.item.name = hirsel_intern::Symbol::intern(&helper.item.definition.0.name);
        helper.item.visibility = Visibility::Private;
        helper.item.attributes.clear();
        helper.item.kind = ItemKind::Function {
            signature: signature.clone(),
            body: Some(*body),
            abi: None,
        };
        lifted.push(Arc::new(helper));
    }
    lifted
}

/// `None` keeps CT-2's existing data materialization path. A dictionary is a
/// transparent nominal record with at least one callable and otherwise data.
/// The same declaration algorithm can suspend at data-field evaluation on
/// an admitted worker. Named callable publication remains header-only.
pub(crate) trait DeclarationSource {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError>;
    async fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError>;
}
struct SynchronousDeclaration<'a, S>(&'a mut S);
impl<S: ComptimeValueSource> DeclarationSource for SynchronousDeclaration<'_, S> {
    fn signature(&mut self, path: &DefPath) -> Result<hirsel_types::SignatureRef, QueryError> {
        self.0.signature(path)
    }
    async fn evaluate(&mut self, input: EvaluationInput<'_>) -> Result<Materialized, QueryError> {
        self.0.evaluate(input)
    }
}
pub(crate) fn declaration(
    types: &hirsel_types::TypeInterner,
    memo: &hirsel_types::TypeMemo,
    analyzed: &AnalyzedBody,
    signature: &Signature,
    constants: &[(ExprId, Materialized)],
    source: &mut impl ComptimeValueSource,
) -> Result<Option<Materialized>, QueryError> {
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };
    let mut source = SynchronousDeclaration(source);
    let future = declaration_async(types, memo, analyzed, signature, constants, &mut source);
    match std::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(result) => result,
        Poll::Pending => Err(QueryError::NotImplemented),
    }
}

pub(crate) async fn declaration_async(
    types: &hirsel_types::TypeInterner,
    memo: &hirsel_types::TypeMemo,
    analyzed: &AnalyzedBody,
    signature: &Signature,
    constants: &[(ExprId, Materialized)],
    source: &mut impl DeclarationSource,
) -> Result<Option<Materialized>, QueryError> {
    let SignatureKind::Const { ty, .. } = signature.kind else {
        return Ok(None);
    };
    let TypeKind::Named {
        definition,
        ref arguments,
    } = types.row(ty).kind
    else {
        return Ok(None);
    };
    let nominal = source.signature(definition.path())?;
    let SignatureKind::Type(ref record) = nominal.kind else {
        return Ok(None);
    };
    if record.resource
        || record.distinct
        || record.capability.is_some()
        || nominal.metadata.visibility == Visibility::PublicOpaque
    {
        return Ok(None);
    }
    let Some(representation) = record.representation else {
        return Ok(None);
    };
    let representation = hirsel_types::substitute(
        types,
        memo,
        representation,
        &Substitution {
            binder: Some(GenericBinder::Bound(0)),
            roles: &nominal.metadata.roles,
            arguments,
            authorities: &vec![hirsel_types::AuthoritySummary::Unknown; arguments.len()],
        },
    )
    .map_err(|_| QueryError::NotImplemented)?;
    let TypeKind::Record(ref fields) = types.row(representation).kind else {
        return Ok(None);
    };
    if !fields
        .iter()
        .any(|f| matches!(types.row(f.ty).kind, TypeKind::Function(_)))
    {
        return Ok(None);
    }
    let syntax = &analyzed.typed.syntax;
    let checked = &analyzed.typed.checked;
    let ItemKind::Const { value, .. } = syntax.item.kind else {
        unreachable!()
    };
    let site = syntax.tree.expressions[value.0 as usize].origin.span;
    for field in fields {
        if !matches!(types.row(field.ty).kind, TypeKind::Function(_))
            && !hirsel_types::structural_equality(
                types,
                field.ty,
                Bound::Data,
                &Default::default(),
                &mut |d| source.signature(d.path()).map(Some),
            )?
        {
            return Err(rejected(site, "dictionary fields must be callable or data"));
        }
    }
    if let Some((_, alias)) = constants.iter().find(|(id, _)| *id == value) {
        return Ok(Some(alias.clone()));
    }
    let ExprKind::Call {
        arguments: initializers,
        ..
    } = &syntax.tree.expressions[value.0 as usize].kind
    else {
        return Err(rejected(
            site,
            "a dictionary constant is a labelled record construction or a constant alias",
        ));
    };
    let constructor = checked.calls.iter().find(|call| call.expression == value);
    if !constructor.is_some_and(|call| matches!(&call.target,
        hirsel_types::CallTarget::Constructor(hirsel_types::ContextualName::Constructor(d)) if *d == definition.path().clone()))
    { return Err(rejected(site, "a dictionary constant cannot call a factory")) }
    if initializers.len() != fields.len() || initializers.iter().any(|a| a.label.is_none()) {
        return Err(rejected(site, "every dictionary field must be labelled"));
    }
    let mut nodes = vec![];
    for field in fields {
        let initializer = initializers
            .iter()
            .find(|a| a.label.is_some_and(|label| label == field.name))
            .ok_or_else(|| rejected(site, "missing dictionary field"))?
            .value;
        let at = syntax.tree.expressions[initializer.0 as usize].origin.span;
        let node = if matches!(types.row(field.ty).kind, TypeKind::Function(_)) {
            if let Some(mut node) = projected_constant(analyzed, initializer, constants) {
                node.ty = field.ty;
                node
            } else {
                let definition = match &syntax.tree.expressions[initializer.0 as usize].kind {
                    ExprKind::Closure { .. }
                        if checked
                            .closures
                            .iter()
                            .any(|c| c.expression == initializer && c.captures.is_empty()) =>
                    {
                        literal_path(&syntax.item.definition, initializer)
                    }
                    ExprKind::Path(_) => match checked
                        .resolution
                        .expressions
                        .iter()
                        .find(|(id, _)| *id == initializer)
                        .map(|(_, name)| name)
                    {
                        Some(hirsel_types::ResolvedName::Definition(path))
                            if matches!(
                                source.signature(path)?.kind,
                                SignatureKind::Function(_)
                            ) =>
                        {
                            path.clone()
                        }
                        _ => {
                            return Err(rejected(
                                at,
                                "a constant operation must name a declared function",
                            ));
                        }
                    },
                    _ => {
                        return Err(rejected(
                            at,
                            "a constant operation must be a named function or capture-free literal",
                        ));
                    }
                };
                source.signature(&definition)?;
                let arguments = checked
                    .instantiations
                    .iter()
                    .find(|i| i.expression == initializer)
                    .map(|i| i.arguments.clone())
                    .unwrap_or_default();
                ValueNode {
                    ty: field.ty,
                    kind: ValueKind::FunctionReference {
                        definition,
                        arguments,
                    },
                }
            }
        } else if let Some((_, constant)) = constants.iter().find(|(id, _)| *id == initializer) {
            constant.root.clone()
        } else {
            source
                .evaluate(EvaluationInput {
                    analyzed,
                    staged: &[],
                    signature,
                    expression: initializer,
                    child: initializer,
                    ty: field.ty,
                    constants,
                    sizes: &[],
                })
                .await?
                .root
        };
        nodes.push(node);
    }
    Ok(Some(Materialized {
        root: ValueNode {
            ty: checked
                .expressions
                .iter()
                .find(|row| row.expression == value)
                .ok_or(QueryError::NotImplemented)?
                .ty,
            kind: ValueKind::Construct {
                constructor: Constructor::Record(definition.path().clone()),
                fields: nodes,
            },
        },
    }))
}
