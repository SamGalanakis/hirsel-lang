use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::*;
use std::sync::Arc;

fn function(width: usize, ty: TypeId, path: &DefPath) -> FunctionType {
    FunctionType {
        state_promises: vec![
            StatePromise {
                selector: RelationPlace {
                    parameter: ParameterRef::bound(ParamId(0)),
                    fields: (0..width).map(|i| FieldId(i as u32)).collect(),
                },
                advance_only: false,
                preserve_captures: false,
                preserve_unique: false,
                unretained: vec![ParamId(0); width].into(),
            };
            width
        ]
        .into(),
        generics: (0..width)
            .map(|i| GenericParameter::Type {
                name: format!("parameter{i}"),
                bounds: vec![Bound::Copyable; width].into(),
            })
            .collect::<Vec<_>>()
            .into(),
        parameters: (0..width)
            .map(|i| Parameter {
                name: Some(format!("name{i}")),
                label: Some(format!("label{i}")),
                passing: hirsel_syntax::PassingMode::Borrow,
                ty,
            })
            .collect::<Vec<_>>()
            .into(),
        result: ty,
        error: None,
        allocation: Allocation::NoAlloc,
        authority: AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(path.clone()); width].into(),
        ),
        uses: Uses::Only(AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(path.clone()); width].into(),
        )),
        contracts: vec![
            Contract {
                kind: hirsel_syntax::ContractKind::Requires,
                ordinal: 0,
                code_hash: ContentHash::default()
            };
            width
        ]
        .into(),
        incomplete: false,
        foreign: Some(ForeignContract {
            abi: "C".repeat(width),
            blocking: false,
            aliasing_contract: "retained contract".repeat(width),
        }),
    }
}

#[test]
fn local_type_interner_bound_covers_owned_modes_and_callable_payloads_without_loading_base() {
    let path = DefPath::core("source.type.payload");
    hirsel_syntax::DefId::intern(&path);
    let symbol = hirsel_intern::Symbol::intern("field");
    for width in [1, 4, 16] {
        let (types, actual) = crate::test_alloc::measured(|| {
            let types = TypeInterner::new(None);
            let unit = types.intern(Type::unit());
            let kinds = [
                TypeKind::Tuple(vec![unit; width].into()),
                TypeKind::Record(
                    vec![
                        Field {
                            growable: false,
                            name: symbol,
                            ty: unit
                        };
                        width
                    ]
                    .into(),
                ),
                TypeKind::Variant(
                    vec![
                        Variant {
                            name: symbol,
                            fields: vec![(None, unit); width].into()
                        };
                        width
                    ]
                    .into(),
                ),
                TypeKind::Function(function(width, unit, &path)),
                TypeKind::Named {
                    definition: hirsel_syntax::DefId::intern(&path),
                    arguments: vec![GenericArgument::Type(unit); width].into(),
                },
            ];
            for kind in kinds {
                let origin = Origin {
                    root: OriginRoot::Definition(path.clone()),
                    fields: (0..width).map(|i| FieldId(i as u32)).collect(),
                };
                let mut row = Type::with_kind(kind);
                row.modes.relation = Relation::new([Reach::Place(origin.clone())]);
                row.brand.alternatives = vec![origin; width].into();
                row.modes.sendability = Sendability::DependsOn(
                    vec![TypeParameterRef::bound(TypeParamId(0)); width].into(),
                );
                row.drop_allocation = DropAllocation::DependsOn(
                    vec![TypeParameterRef::bound(TypeParamId(0)); width].into(),
                );
                row.erased_captures = Some(ErasedCaptures::Known(AuthoritySummary::Known(
                    vec![AuthorityTerm::Capability(path.clone()); width].into(),
                )));
                types.intern(row);
            }
            types
        });
        let (bound, inspection) = crate::test_alloc::measured(|| types.retained_local_bytes());
        assert_eq!(inspection.allocations, 0);
        assert!(
            actual.live as u64 <= bound,
            "width {width}: actual {} exceeds {bound}",
            actual.live
        );
        let clone = types.clone();
        let (_, release) = crate::test_alloc::measured(|| drop(types));
        assert_eq!(release.live, 0);
        let (_, release) = crate::test_alloc::measured(|| drop(clone));
        assert_eq!(-release.live, actual.live);
    }
    struct Unloaded;
    impl TypeBaseSource for Unloaded {
        fn rows(&self, _: u32, _: &TypeBase) -> Option<Vec<(u32, Type)>> {
            panic!("inspection loaded a base row");
        }
        fn candidates(&self, _: &Type, _: &TypeBase) -> Vec<u32> {
            panic!("inspection queried a base provider");
        }
    }
    let base = TypeBase::from_source(32, Arc::new(Unloaded));
    let types = TypeInterner::new(Some(base));
    let (bound, inspection) = crate::test_alloc::measured(|| types.retained_local_bytes());
    assert_eq!(inspection.allocations, 0);
    assert!(bound >= size_of::<TypeInterner>() as u64);
    assert_eq!(types.stats().rows, 0);
}
