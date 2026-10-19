use super::super::*;
use super::support;
use proptest::prelude::*;
use std::sync::Arc;
pub(super) fn transport(s: &Signature, noise: u8) -> Signature {
    let mut result = s.clone();
    let mut source = s.types.clone();
    let mut dest = TypeStore::default();
    for _ in 0..noise {
        dest.push(Type::with_kind(TypeKind::Primitive(Primitive::Bool)));
    }
    match &mut result.kind {
        SignatureKind::Function(f) => {
            let root = source.push(Type {
                drop_allocation: DropAllocation::MayAllocate,
                ..Type::with_kind(TypeKind::Function(f.clone()))
            });
            let root = import_type(&source, root, &mut dest).unwrap();
            let TypeKind::Function(imported) = dest.get(root).unwrap().kind.clone() else {
                unreachable!()
            };
            *f = imported;
        }
        SignatureKind::Const { ty: root, .. } => {
            *root = import_type(&source, *root, &mut dest).unwrap()
        }
        SignatureKind::Type(td) => {
            if let Some(root) = td.representation {
                td.representation = Some(import_type(&source, root, &mut dest).unwrap());
            }
        }
    }
    result.types = dest;
    result
}
fn run(source: &str, seed: &[String], reverse: bool, noise: u8) -> QueryOutput<CheckedBody> {
    let (file, _, ds) = support::parse(source, seed);
    assert!(ds.is_empty());
    let mut registry = support::registry(&file);
    support::normalize_file(&file, &mut registry).unwrap();
    let id = file.items.last().unwrap();
    let r = hirsel_resolve::resolve_item(&file, id, &registry.index.clone(), &mut registry);
    assert!(r.query.diagnostics.is_empty(), "{:?}", r.query.diagnostics);
    let signature = registry.signatures[&id.item.definition].clone();
    let mut loaded: Vec<_> = registry.signatures.into_iter().collect();
    if reverse {
        loaded.reverse();
    }
    registry.signatures = loaded
        .into_iter()
        .map(|(p, s)| (p, Arc::new(transport(&s, noise))))
        .collect();
    check_with_loop_protocols(
        LoopInput {
            body: Input {
                syntax: id,
                context: &support::context(),
                resolution: &r,
                signature: &signature,
            },
            protocols: &hirsel_types::core_roles::loop_protocols(),
        },
        &mut registry,
    )
    .unwrap()
    .checked
}
pub(super) fn equivalent(a: &QueryOutput<CheckedBody>, b: &QueryOutput<CheckedBody>) {
    assert_eq!(a.diagnostics, b.diagnostics);
    assert!(a.diagnostics.is_empty());
    assert_eq!(a.dependencies, b.dependencies);
    let (a, b) = (&a.value, &b.value);
    assert_eq!(a.expressions.len(), b.expressions.len());
    assert_eq!(a.bindings.len(), b.bindings.len());
    assert_eq!(a.patterns.len(), b.patterns.len());
    assert_eq!(a.instantiations.len(), b.instantiations.len());
    for (x, y) in a.expressions.iter().zip(&b.expressions) {
        assert_eq!((x.expression, x.usage), (y.expression, y.usage));
        assert!(semantic_equal(&a.types, x.ty, &b.types, y.ty).unwrap());
    }
    for (x, y) in a.bindings.iter().zip(&b.bindings) {
        assert_eq!(
            (&x.owner, &x.site, &x.binding, x.usage, x.mutable),
            (&y.owner, &y.site, &y.binding, y.usage, y.mutable)
        );
        assert!(semantic_equal(&a.types, x.ty, &b.types, y.ty).unwrap());
    }
    for (x, y) in a.patterns.iter().zip(&b.patterns) {
        assert_eq!((&x.kind, &x.fields, x.usage), (&y.kind, &y.fields, y.usage));
        assert!(semantic_equal(&a.types, x.ty, &b.types, y.ty).unwrap());
    }
    for (x, y) in a.instantiations.iter().zip(&b.instantiations) {
        assert_eq!((&x.callee, x.expression), (&y.callee, y.expression));
        for (x, y) in x.arguments.iter().zip(&y.arguments) {
            match (x, y) {
                (GenericArgument::Type(x), GenericArgument::Type(y)) => {
                    assert!(semantic_equal(&a.types, *x, &b.types, *y).unwrap())
                }
                _ => assert_eq!(x, y),
            }
        }
    }
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn checking_is_independent_of_interning_and_signature_loading(
        names in prop::collection::vec("[a-z]{1,12}",0..12), noise in 0u8..8, reversed in any::<bool>(), n in 0u8..=127
    ) {
        let source=format!("fn first[T](x: T) -> T {{ x }}\nfn second[T](x: T) -> T {{ first(x) }}\nfn main() -> u8 {{ let (unused, x): (u8, u8) = ({n}, {n})\n second(x) }}");
        let a=run(&source,&[],false,0);let b=run(&source,&names,reversed,noise);let repeated=run(&source,&names,reversed,noise);equivalent(&a,&b);equivalent(&b,&repeated);
    }
}
