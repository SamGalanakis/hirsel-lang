//! Retained signature code is adapted to an ordinary checked function snapshot.
use crate::*;
use hirsel_syntax::*;
use std::sync::Arc;
pub(crate) fn snapshot(
    file: &SyntaxFile,
    id: ItemId,
    symbols: &mut Interner,
) -> Option<(Arc<SyntaxFile>, ItemId)> {
    let source = &file.tree.items[id.0 as usize];
    let ItemKind::Type {
        generics,
        body: TypeBody::Distinct {
            field,
            fact: Some(predicate),
        },
        ..
    } = &source.kind
    else {
        return None;
    };
    let path = hirsel_check_protocol::fact::fact_path(&source.definition);
    let name = symbols.intern(&path.0.name);
    let boolean = symbols.intern("bool");
    let mut adapted = file.clone();
    adapted.symbols = Arc::new(symbols.clone());
    let result = TypeExprId(adapted.tree.types.len() as u32);
    adapted.tree.types.push(TypeExpr {
        origin: field.origin,
        kind: TypeExprKind::Named {
            path: Path {
                package: None,
                segments: vec![boolean],
            },
            arguments: vec![],
        },
    });
    let mut item = source.clone();
    item.name = name;
    item.definition = path;
    item.visibility = Visibility::Private;
    item.attributes.clear();
    item.kind = ItemKind::Function {
        abi: None,
        body: Some(*predicate),
        signature: FunctionSyntax {
            generics: generics.clone(),
            parameters: vec![Parameter {
                label: None,
                name: Some(field.name),
                passing: PassingMode::Borrow,
                ty: field.ty,
            }],
            result: Some(result),
            uses: Some(vec![]),
            ..Default::default()
        },
    };
    let helper = ItemId(adapted.tree.items.len() as u32);
    adapted.tree.items.push(item);
    Some((Arc::new(adapted), helper))
}
