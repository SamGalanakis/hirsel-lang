//! M's declared private cursor operations fail closed without their plans.
use super::{support::*, *};
use hirsel_syntax::*;
use hirsel_types::*;

fn core(name: &str) -> DefPath {
    let mut parts: Vec<_> = name.split('.').map(str::to_owned).collect();
    let name = parts.pop().unwrap();
    DefPath(ItemPath {
        package: PackageId {
            name: "core".into(),
            version: "1".into(),
        },
        modules: parts,
        name,
    })
}
fn declaration(f: &mut Fixture, path: DefPath, mut kind: SignatureKind) {
    let types = session();
    if let SignatureKind::Function(function) = &mut kind {
        let mut abstract_type = |t| {
            map_parameter_references(types, t, |p, depth| {
                Ok(match p {
                    ParameterRef::Body { scope, parameter } if scope == f.scope(f.owner) => {
                        ParameterRef::Bound { depth, parameter }
                    }
                    p => p,
                })
            })
            .unwrap()
        };
        for p in &mut function.parameters {
            p.ty = abstract_type(p.ty);
        }
        function.result = abstract_type(function.result);
    }
    f.register(Signature {
        metadata: SignatureMetadata::default(),
        definition: DefId::intern(&path),
        kind,
        fingerprint: ContentHash::default(),
    });
}
fn nominal(f: &mut Fixture, name: &str, arity: usize, representation: Option<TypeId>) {
    declaration(
        f,
        core(name),
        SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
            generics: (0..arity)
                .map(|i| GenericParameter::Type {
                    name: format!("T{i}"),
                    bounds: vec![].into(),
                })
                .collect(),
            representation,
            destructor_allocation: Allocation::NoAlloc,
            bounds: if name == "StrPos" {
                vec![Bound::Plain]
            } else {
                vec![]
            },
            authority: AuthoritySummary::inert(),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        })),
    );
}
fn operation(
    f: &mut Fixture,
    path: DefPath,
    input: TypeId,
    result: TypeId,
    passing: PassingMode,
    arity: usize,
) {
    declaration(
        f,
        path,
        SignatureKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: (0..arity)
                .map(|i| GenericParameter::Type {
                    name: format!("T{i}"),
                    bounds: vec![].into(),
                })
                .collect(),
            parameters: vec![hirsel_types::Parameter {
                name: None,
                label: None,
                passing,
                ty: input,
            }]
            .into(),
            result,
            error: None,
            allocation: Allocation::NoAlloc,
            authority: AuthoritySummary::Known(
                vec![AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                    ParamId(0),
                ))]
                .into(),
            ),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
    );
}
fn fixture() -> (Fixture, ExprId) {
    let mut f = Fixture::new(Primitive::U32);
    let item = f.scalar;
    let source_ty = f.type_(TypeKind::Named {
        definition: DefId::intern(&core("ScalarView")),
        arguments: vec![].into(),
    });
    let shape = f.type_(TypeKind::Builtin {
        kind: BuiltinType::Array,
        arguments: vec![GenericArgument::Type(item)].into(),
    });
    nominal(&mut f, "ScalarView", 0, Some(shape));
    let actuals = vec![];
    // The private cursor result keeps counted items in the source locality;
    // plain StrPos bits retain their Global declaration (OWN-8).
    let item = normalize_type_properties(
        session(),
        item,
        &GenericDeclarations::default(),
        &mut |path| {
            Ok::<_, hirsel_types::QueryError>(
                f.declarations.get(path).cloned().map(std::sync::Arc::new),
            )
        },
    )
    .unwrap();
    let cursor_name = "$intrinsic.ViewCursor";
    let next_arguments = vec![GenericArgument::Type(item)];
    let mut cursor_ty = ty(TypeKind::Named {
        definition: DefId::intern(&core(cursor_name)),
        arguments: next_arguments.clone().into(),
    });
    cursor_ty.modes.relation = Relation::environment();
    let cursor = session().intern(cursor_ty.clone());
    cursor_ty.kind = TypeKind::Named {
        definition: DefId::intern(&core("Option")),
        arguments: vec![GenericArgument::Type(item)].into(),
    };
    let option = session().intern(cursor_ty);
    nominal(&mut f, cursor_name, 1, None);
    let option_shape = f.type_(TypeKind::Variant(
        vec![
            Variant {
                name: "None".into(),
                fields: vec![].into(),
            },
            Variant {
                name: "Some".into(),
                fields: vec![(None, item)].into(),
            },
        ]
        .into(),
    ));
    nominal(&mut f, "Option", 1, Some(option_shape));
    let option = normalize_type_properties(
        session(),
        option,
        &GenericDeclarations::default(),
        &mut |path| {
            Ok::<_, hirsel_types::QueryError>(
                f.declarations.get(path).cloned().map(std::sync::Arc::new),
            )
        },
    )
    .unwrap();
    let source_use = UseKind::Borrow;
    let parameter = f.parameter(0, source_ty, source_use);
    let source = f.read(parameter, source_ty, source_use);
    let zero = f.integer(0, 1);
    let (count, binding) = f.let_(zero);
    let (pattern, _) = f.binding(item, UseKind::Borrow);
    let place = f.read(binding.clone(), f.scalar, UseKind::Copy);
    let one = f.integer(1, 12);
    let add = f.assign(place, AssignOp(Some(BinaryOp::Add)), one, f.scalar, 12);
    let tail = f.unit_();
    let body = f.block(vec![add], tail);
    let statement = f.stmt(
        StmtKind::For {
            pattern,
            source,
            body,
            label: None,
        },
        10,
    );
    let mut producer = hirsel_check_protocol::body::Plans::default();
    producer.loops.push(hirsel_check_protocol::body::ForPlan {
        protocol: hirsel_check_protocol::body::ForProtocol::Iterator,
        kind: hirsel_check_protocol::body::ForPlanKind::Cursor {
            cursor: hirsel_types::CursorPlan {
                association: None,
                statement,
                source_use,
                source,
                open: core("$intrinsic.UnicodeScalars_open"),
                next: core("$intrinsic.UnicodeScalars_next"),
                open_arguments: actuals.clone(),
                next_arguments: next_arguments.clone(),
                cursor,
                step: hirsel_types::OptionLayout {
                    result: option,
                    item,
                    definition: core("Option"),
                    none_variant: VariantId(0),
                    some_variant: VariantId(1),
                },
            },
        },
    });
    let result = f.read(binding, f.scalar, UseKind::Copy);
    let root = f.block(vec![count, statement], result);
    f.plans = Plans::from_body(session(), &f.syntax.tree, &f.checked, &producer, root).unwrap();
    let open = core("$intrinsic.UnicodeScalars_open");
    let next = core("$intrinsic.UnicodeScalars_next");
    operation(
        &mut f,
        open.clone(),
        source_ty,
        cursor,
        PassingMode::Borrow,
        actuals.len(),
    );
    operation(
        &mut f,
        next.clone(),
        cursor,
        option,
        PassingMode::Mut,
        next_arguments.len(),
    );
    f.checked.instantiations.push(Instantiation {
        expression: source,
        callee: open.clone(),
        arguments: actuals,
    });
    f.checked.instantiations.push(Instantiation {
        expression: source,
        callee: next.clone(),
        arguments: next_arguments,
    });
    (f, root)
}
#[test]
fn cursor_loops_fail_closed_without_unique_plans_or_actuals() {
    let (mut f, root) = fixture();
    let plans = std::mem::take(&mut f.plans.cursors);
    assert!(
        f.lower(root, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("protocol plan")
    );
    f.plans.cursors = plans;
    f.plans.cursors.push(f.plans.cursors[0].clone());
    assert!(
        f.lower(root, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("duplicate cursor plan")
    );
    f.plans.cursors.pop();
    f.checked.instantiations.clear();
    assert!(
        f.lower(root, "main", f.scalar)
            .unwrap_err()
            .message
            .contains("checked instantiation")
    );
}
