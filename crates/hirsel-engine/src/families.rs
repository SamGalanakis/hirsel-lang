//! GEN-6 declarations the engine derives from `signature` and `implementation`
//! items, and the specializations a closed `with` clause names. Every one is
//! an ordinary item over its source item's syntax, so each later phase and
//! engine sees ordinary declarations and direct calls.
use hirsel_syntax::{
    DefPath, ExprKind, Generated, GenericKind, GenericParam, ItemKind, ItemSyntax, Literal,
    MemberDefinition, MemberSpec, TypeArgument, TypeBody, TypeExprKind, TypeFlavor,
};
use std::sync::Arc;

/// Whether `path` names a declaration derived here rather than written.
pub(crate) fn derived(path: &DefPath) -> bool {
    path.0.name.contains("$member$")
}

/// Whether a body names a `with` clause. Its call targets a specialization
/// that may not exist before the body resolves, so the definition takes the
/// serial front path, where signatures materialize on demand.
pub(crate) fn closes(syntax: &ItemSyntax) -> bool {
    syntax
        .tree
        .expressions
        .iter()
        .any(|e| matches!(e.kind, hirsel_syntax::ExprKind::With { .. }))
}

/// A signature's operation headers, as bodyless functions generic over its
/// type and size members; an implementation's type members, as transparent
/// aliases. Both keep their item's visibility.
pub(crate) fn snapshots(syntax: &ItemSyntax) -> Vec<Arc<ItemSyntax>> {
    let item = &syntax.item;
    let derive = |path: DefPath, kind: ItemKind| {
        let mut helper = syntax.clone();
        helper.item.name = hirsel_intern::Symbol::intern(&path.0.name);
        helper.item.definition = path;
        helper.item.attributes.clear();
        helper.item.kind = kind;
        Arc::new(helper)
    };
    match &item.kind {
        ItemKind::Signature { members } => {
            let parameters: Vec<GenericParam> = members
                .iter()
                .filter_map(|member| match member {
                    MemberSpec::Type { name, bounds, .. } => Some(GenericParam {
                        name: *name,
                        kind: GenericKind::Type(bounds.clone()),
                    }),
                    MemberSpec::Size { name, .. } => Some(GenericParam {
                        name: *name,
                        kind: GenericKind::Size,
                    }),
                    MemberSpec::Operation { .. } => None,
                })
                .collect();
            members
                .iter()
                .filter_map(|member| match member {
                    MemberSpec::Operation {
                        name, signature, ..
                    } => {
                        let mut signature = signature.clone();
                        let own = std::mem::take(&mut signature.generics);
                        signature.generics = parameters.iter().cloned().chain(own).collect();
                        Some(derive(
                            hirsel_types::family_operation_path(&item.definition, name.as_str()),
                            ItemKind::Function {
                                signature,
                                body: None,
                                abi: None,
                            },
                        ))
                    }
                    _ => None,
                })
                .collect()
        }
        ItemKind::Implementation { members, .. } => members
            .iter()
            .filter_map(|member| match member {
                MemberDefinition::Type { name, ty, .. } => Some(derive(
                    hirsel_types::implementation_type_path(&item.definition, name.as_str()),
                    ItemKind::Type {
                        flavor: TypeFlavor::Ordinary,
                        generics: vec![],
                        bounds: None,
                        authority: None,
                        body: TypeBody::Alias(*ty),
                        drop: None,
                    },
                )),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

/// The specialization `path` names (GEN-6): `base`'s own syntax with every
/// binder bound to its implementation. A bound binder's size members are
/// rewritten to their literal values, read from each implementation's
/// syntax; its types and operations resolve to the implementation's actuals.
pub(crate) fn specialization(
    path: &DefPath,
    base: &ItemSyntax,
    implementations: &[(String, Option<Arc<ItemSyntax>>)],
) -> Option<Arc<ItemSyntax>> {
    let (_, bindings) = hirsel_types::specialization_parts(path)?;
    let ItemKind::Function { signature, .. } = &base.item.kind else {
        return None;
    };
    let binders: Vec<_> = signature
        .generics
        .iter()
        .filter(|g| matches!(g.kind, GenericKind::Implementation { .. }))
        .map(|g| g.name)
        .collect();
    if binders.len() != bindings.len()
        || binders
            .iter()
            .zip(&bindings)
            .any(|(binder, (name, _))| binder.as_str() != name)
    {
        return None;
    }
    let mut syntax = base.clone();
    syntax.item.definition = path.clone();
    // A closed instance may name its private implementation's associated aliases.
    syntax.item.visibility = hirsel_syntax::Visibility::Private;
    syntax.item.name = hirsel_intern::Symbol::intern(&path.0.name);
    syntax.item.generated = Some(Generated::Specialization(
        bindings
            .iter()
            .map(|(binder, implementation)| {
                (
                    hirsel_intern::Symbol::intern(binder),
                    implementation.clone(),
                )
            })
            .collect(),
    ));
    let sizes = |binder: &str, member: &str| {
        let (_, implementation) = implementations.iter().find(|(b, _)| b == binder)?;
        let ItemKind::Implementation { members, .. } = &implementation.as_ref()?.item.kind else {
            return None;
        };
        members.iter().find_map(|m| match m {
            MemberDefinition::Size { name, value, .. } if name.as_str() == member => Some(*value),
            _ => None,
        })
    };
    let tree = &mut syntax.tree;
    let size_of = |id: hirsel_syntax::TypeExprId, types: &[hirsel_syntax::TypeExpr]| {
        let TypeExprKind::Named { path, arguments } = &types[id.0 as usize].kind else {
            return None;
        };
        match (
            path.package.is_none(),
            path.segments.as_slice(),
            arguments.is_empty(),
        ) {
            (true, [binder, member], true) => sizes(binder.as_str(), member.as_str()),
            _ => None,
        }
    };
    for index in 0..tree.types.len() {
        let rewritten: Option<Vec<TypeArgument>> = match &tree.types[index].kind {
            TypeExprKind::Named { arguments, .. } => Some(
                arguments
                    .iter()
                    .map(|argument| match argument {
                        TypeArgument::Type(id) => size_of(*id, &tree.types)
                            .map_or(argument.clone(), TypeArgument::SizeLiteral),
                        _ => argument.clone(),
                    })
                    .collect(),
            ),
            _ => None,
        };
        if let (Some(rewritten), TypeExprKind::Named { arguments, .. }) =
            (rewritten, &mut tree.types[index].kind)
        {
            *arguments = rewritten;
        }
    }
    // A size member used as a value is its `u64` literal.
    for expression in &mut tree.expressions {
        let ExprKind::Path(path) = &expression.kind else {
            continue;
        };
        if let (true, [binder, member]) = (path.package.is_none(), path.segments.as_slice())
            && let Some(value) = sizes(binder.as_str(), member.as_str())
        {
            expression.kind = ExprKind::Literal(Literal::Integer(format!("{value}u64")));
        }
    }
    Some(Arc::new(syntax))
}
