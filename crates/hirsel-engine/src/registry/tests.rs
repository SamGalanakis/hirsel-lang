use super::declarations::{build, schema};
use super::review::*;
use super::*;
use hirsel_intern::Symbol;
use hirsel_syntax::PackageId;
use hirsel_types::core_roles::{intrinsic_roles, loop_protocols};
use std::collections::BTreeSet;
/// The interner the shared registry's rows live in.
fn types() -> &'static TypeInterner {
    shared_types()
}

#[test]
fn every_declared_signature_matches_the_review_table_and_canonical_hash() {
    let registry = BuiltinRegistry::new();
    let table=registry.signatures().map(|s|{
        let path=format!("core@1/{}{}",if s.definition.path().0.modules.is_empty(){String::new()}else{format!("{}.",s.definition.path().0.modules.join("."))},s.definition.path().0.name);
        let declaration=match &s.kind{
            SignatureKind::Function(f)=>format!("{:?} {}",f.generics,describe_function(types(),f)),
            SignatureKind::Const { ty: id, .. }=>format!("const {}",describe_type(types(),*id)),
            SignatureKind::Type(t)=>format!("type {:?} {} ; bounds={:?} ; authority={:?} ; capability={:?} ; resource={} ; drop={:?}",t.generics,t.representation.map(|id|describe_type(types(),id)).unwrap_or_else(||"opaque".into()),t.bounds,t.authority,t.capability,t.resource,t.destructor_allocation),
            SignatureKind::Family(_)|SignatureKind::Implementation(_)=>unreachable!("core declares no family"),
        };
        let hash=s.fingerprint.0.iter().map(|b|format!("{b:02x}")).collect::<String>();format!("{path} | {declaration} | {hash}\n")
    }).collect::<String>();
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/registry/review-table.txt"),
            &table,
        )
        .unwrap();
        return;
    }
    assert_eq!(table, include_str!("review-table.txt"));
}
#[test]
fn section22_ledger_is_a_complete_verbatim_transcription() {
    // Independent normative oracle: no runtime code or Builder is used here.
    let spec = include_str!("../../../../docs/spec/core-library.md");
    let section = spec
        .split("\n## The ledger\n")
        .nth(1)
        .unwrap()
        .split("```text\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    assert_eq!(SECTION22.trim(), section.trim());
    let registry = BuiltinRegistry::new();
    let mut names = BTreeSet::new();
    for line in SECTION22
        .lines()
        .map(|l| {
            ["@preserves_captures(", "@preserves_unique(", "@unretained("]
                .iter()
                .fold(l, |l, prefix| {
                    l.strip_prefix(prefix)
                        .and_then(|rest| rest.split_once(") "))
                        .map_or(l, |(_, rest)| rest)
                })
        })
        .filter(|l| l.starts_with("fn ") || l.starts_with("N fn "))
    {
        let head = line.trim_start_matches("N ").strip_prefix("fn ").unwrap();
        let name = head.split(['[', '(']).next().unwrap();
        assert!(names.insert(name), "duplicate ledger declaration: {name}");
        assert!(
            matches!(
                registry
                    .get(&hirsel_syntax::DefPath::core(name))
                    .unwrap()
                    .kind,
                SignatureKind::Function(_)
            ),
            "{name}"
        );
    }
    // DROP-4's forget is declared outside the ledger. Every other source-callable
    // function must have a ledger row, so removing or forgetting one cannot hide
    // in a total.
    let expected: BTreeSet<_> = names
        .into_iter()
        .map(hirsel_syntax::DefPath::core)
        .chain(std::iter::once(hirsel_syntax::DefPath::core("forget")))
        .collect();
    let declared: BTreeSet<_> = registry
        .signatures()
        .filter(|signature| {
            matches!(signature.kind, SignatureKind::Function(_))
                && !signature
                    .definition
                    .path()
                    .0
                    .modules
                    .iter()
                    .any(|module| module.starts_with('$'))
        })
        .map(|signature| signature.definition.path().clone())
        .collect();
    assert_eq!(
        expected, declared,
        "source-callable functions and ledger rows"
    );
}
#[test]
fn roles_and_provider_lookup_are_nominal_and_read_only() {
    let mut registry = BuiltinRegistry::new();
    let path = hirsel_syntax::Path {
        package: None,
        segments: vec![Symbol::intern("out"), Symbol::intern("print")],
    };
    let context = FileContext {
        package: PackageId {
            name: "user".into(),
            version: "1".into(),
        },
        modules: vec![],
    };
    let lookup = registry.lookup(&context, &path);
    assert_eq!(
        lookup.result.unwrap(),
        hirsel_syntax::DefPath::core("out.print")
    );
    assert_eq!(lookup.manifest, registry.revision());
    let mut foreign = path.clone();
    foreign.package = Some(context.package.clone());
    assert!(registry.lookup(&context, &foreign).result.is_err());
    let role = registry
        .get(&hirsel_syntax::DefPath::core("Unsafe"))
        .unwrap();
    assert_eq!(*role.metadata.roles, nominal_roles());
    for (path, _) in intrinsic_roles() {
        assert!(registry.get(&path).is_some());
    }
}
#[test]
fn compiler_loop_protocols_are_declared_and_cannot_resolve_from_source() {
    let r = BuiltinRegistry::new();
    let mut protocols = BTreeSet::new();
    let mut operations = BTreeSet::new();
    for (protocol, open, next) in loop_protocols() {
        assert!(
            protocols.insert(protocol),
            "duplicate loop protocol: {protocol}"
        );
        assert_eq!(open.0.modules, ["$intrinsic"]);
        assert_eq!(open.0.name, format!("{protocol}_open"));
        for path in [&open, &next] {
            assert!(
                operations.insert(path.clone()),
                "duplicate loop operation: {path}"
            );
            let signature = r.get(path).unwrap();
            assert_eq!(
                signature.metadata.visibility,
                hirsel_syntax::Visibility::Private
            );
            let source = Path {
                package: Some(path.0.package.clone()),
                segments: vec![Symbol::intern("$intrinsic"), Symbol::intern(&path.0.name)],
            };
            assert!(r.lookup_name(&source).is_none());
            let mut lex_sources = hirsel_span::SourceMap::default();
            let file = lex_sources
                .add(
                    "intrinsic.hir",
                    format!("fn main() -> () {{ core@1/$intrinsic.{}() }}", path.0.name),
                )
                .unwrap();
            assert!(!hirsel_lexer::lex(file, &lex_sources).diagnostics.is_empty());
        }
        let open = r.get(&open).unwrap();
        let SignatureKind::Function(f) = &open.kind else {
            panic!()
        };
        let map = matches!(protocol, "MapInsertionOrder" | "SortedMapOrder");
        let generics = if map || protocol == "InlineConsume" {
            2
        } else {
            1
        };
        assert_eq!(f.generics.len(), generics);
        let consuming = protocol.ends_with("Consume")
            || matches!(protocol, "MapInsertionOrder" | "SortedMapOrder");
        assert_eq!(
            f.parameters[0].passing,
            if consuming {
                PassingMode::Take
            } else {
                PassingMode::Borrow
            }
        );
        assert_eq!(
            f.allocation,
            if protocol == "InlineConsume" {
                Allocation::MayAllocate
            } else {
                Allocation::NoAlloc
            }
        );
        assert_eq!(
            f.authority,
            AuthoritySummary::Known(
                vec![AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                    ParamId(0)
                ))]
                .into()
            )
        );
        let result = types().get(f.result).unwrap();
        // An owning cursor is as local as the collection it takes.
        assert_eq!(
            result.modes.relation,
            Relation::parameters([hirsel_types::ParameterRef::bound(ParamId(0))])
        );
        let cursor_name = match protocol {
            "MapInsertionOrder" => "$intrinsic.OwnedMapCursor",
            "SortedMapOrder" => "$intrinsic.OwnedSortedMapCursor",
            _ if consuming => "$intrinsic.OwnedCursor",
            "UnicodeScalars" | "ScalarPositions" => "$intrinsic.ViewCursor",
            _ => "$intrinsic.Cursor",
        };
        assert!(
            matches!(&result.kind,TypeKind::Named {definition,arguments} if *definition.path()==hirsel_syntax::DefPath::core(cursor_name) && arguments.len()==if map {2} else {1})
        );
        let next = r.get(&next).unwrap();
        let SignatureKind::Function(f) = &next.kind else {
            panic!()
        };
        assert_eq!(f.generics.len(), if map { 2 } else { 1 });
        assert_eq!(f.parameters[0].passing, PassingMode::Mut);
        assert_eq!(f.allocation, Allocation::NoAlloc);
        assert!(
            matches!(&types().get(f.result).unwrap().kind,TypeKind::Named {definition,arguments} if *definition.path()==hirsel_syntax::DefPath::core("Option") && arguments.len()==1)
        );
    }
    // Signature-selected view protocols supplement the compiler's protocol list.
    // Compare identities to catch an omitted protocol without pinning the size.
    for signature in r.signatures() {
        if let SignatureKind::Type(t) = &signature.kind {
            if let Some(source) = &t.loop_source {
                operations.extend([source.open.clone(), source.next.clone()]);
            }
        }
    }
    let declared: BTreeSet<_> = r
        .signatures()
        .filter(|signature| {
            signature.definition.path().0.modules == ["$intrinsic"]
                && (signature.definition.path().0.name.ends_with("_open")
                    || signature.definition.path().0.name.ends_with("_next"))
        })
        .map(|signature| signature.definition.path().clone())
        .collect();
    assert_eq!(operations, declared, "loop protocol operation coverage");
    let cursor = r
        .get(&hirsel_syntax::DefPath::core("$intrinsic.Cursor"))
        .unwrap();
    let SignatureKind::Type(cursor) = &cursor.kind else {
        panic!()
    };
    assert!(cursor.representation.is_none());
    assert_eq!(cursor.bounds, vec![Bound::NoallocDrop]);
    let owned = r
        .get(&hirsel_syntax::DefPath::core("$intrinsic.OwnedCursor"))
        .unwrap();
    let SignatureKind::Type(owner) = &owned.kind else {
        panic!()
    };
    assert!(owner.resource);
    assert!(owner.bounds.is_empty());
    assert_eq!(
        types()
            .get(owner.representation.unwrap())
            .unwrap()
            .drop_allocation,
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into())
    );
}

#[test]
fn generic_option_results_derive_sendability_after_substitution() {
    let registry = BuiltinRegistry::new();
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("slots.get"))
        .unwrap();
    let SignatureKind::Function(function) = &signature.kind else {
        panic!()
    };
    assert_eq!(
        types().get(function.result).unwrap().modes.sendability,
        // The unbounded Local declaration has no proof.
        Sendability::NotSendable
    );
    for (kind, sendability) in [
        (Primitive::U64, Sendability::Sendable),
        (Primitive::Bytes, Sendability::NotSendable),
    ] {
        let target = crate::registry::session_types();
        let memo = hirsel_types::TypeMemo::new();
        let actual = target.intern(Type {
            modes: Modes {
                sendability: sendability.clone(),
                ..Modes::global()
            },
            ..Type::with_kind(TypeKind::Primitive(kind))
        });
        let result = hirsel_types::substitute(
            &target,
            &memo,
            function.result,
            &Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &Default::default(),
                arguments: &[GenericArgument::Type(actual)],
                authorities: &[AuthoritySummary::inert()],
            },
        )
        .unwrap();
        let result =
            canonical_current_type(&target, result, &GenericDeclarations::default(), &mut |d| {
                Ok::<_, QueryError>(registry.get(d))
            })
            .unwrap();
        assert_eq!(target.get(result).unwrap().modes.sendability, sendability);
    }
}

#[test]
fn schema_cleanup_uses_declared_ordinals_instead_of_metavariable_spelling() {
    for (generics, input, output) in [
        ("I,J", "I", "J"),
        ("Source,Destination", "Source", "Destination"),
    ] {
        let mut builder = build::Builder::new(&crate::registry::session_types(), generics)
            .with_schema(schema("int.widen"));
        let function = builder.function(&format!("value:{input}"), output, true);
        assert_eq!(
            builder.types.get(function.result).unwrap().drop_allocation,
            DropAllocation::NoAlloc
        );
        assert_eq!(
            builder
                .types
                .get(function.parameters[0].ty)
                .unwrap()
                .drop_allocation,
            DropAllocation::NoAlloc
        );
    }
    let mut ordinary = build::Builder::new(&crate::registry::session_types(), "I");
    let function = ordinary.function("value:I", "I", true);
    assert_eq!(
        ordinary.types.get(function.result).unwrap().drop_allocation,
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into())
    );
}

#[test]
fn schema_constraints_participate_in_signature_fingerprints() {
    let signature = BuiltinRegistry::new()
        .get(&hirsel_syntax::DefPath::core("int.widen"))
        .unwrap();
    let mut changed = signature.as_ref().clone();
    changed.metadata.schema.clear();
    assert_ne!(
        signature.fingerprint,
        hirsel_types::encoding::fingerprint(types(), &changed, &[]).unwrap()
    );
    for index in 0..signature.metadata.schema.len() {
        let mut changed = signature.as_ref().clone();
        changed.metadata.schema.remove(index);
        assert_ne!(
            signature.fingerprint,
            hirsel_types::encoding::fingerprint(types(), &changed, &[]).unwrap()
        );
    }
}

#[test]
fn a_schema_domain_diagnostic_identity_changes_its_fingerprint() {
    let registry = BuiltinRegistry::new();
    let mut signature = registry
        .get(&hirsel_syntax::DefPath::core("simd.add"))
        .unwrap()
        .as_ref()
        .clone();
    if let SchemaConstraint::Domain { rule, .. } = &mut signature.metadata.schema[0] {
        *rule = const { hirsel_diag::RuleId::of("MEM-8") };
    } else {
        panic!("missing domain");
    }
    assert_ne!(
        hirsel_types::encoding::fingerprint(types(), &signature, &[]).unwrap(),
        signature.fingerprint
    );
}

#[test]
fn core_generics_hand_back_borrowed_content_only_related_to_it() {
    // MODE-2's parametric rule lets a borrowed argument of a checked generic
    // body supply nothing, because whatever the body derives from it through a
    // core call stays local to it. So a core generic whose result can hold
    // values of a parameter it cannot copy, which a borrowed argument holds
    // too, either relates its result to that argument or returns it fresh.
    // `$intrinsic` declarations are not callable from source.
    let r = BuiltinRegistry::new();
    let mut checked = 0;
    let mut unrelated = Vec::new();
    for s in r.signatures() {
        let SignatureKind::Function(f) = &s.kind else {
            continue;
        };
        if s.definition
            .path()
            .0
            .modules
            .iter()
            .any(|m| m.as_str() == "$intrinsic")
        {
            continue;
        }
        let (count, found) = borrowed_content(types(), f, &s.metadata.schema);
        checked += count;
        unrelated.extend(
            found
                .into_iter()
                .map(|ordinal| format!("{:?} parameter {ordinal}", s.definition.path().0)),
        );
    }
    assert!(checked > 0);
    assert!(
        unrelated.is_empty(),
        "results that can hold borrowed content unrelated to it: {unrelated:#?}"
    );
    // A declaration handing back a borrowed list's element unrelated to it is
    // what the test exists to catch.
    let mut builder = build::Builder::new(&crate::registry::session_types(), "T");
    let function = builder.function("xs:List[T]", "T", true);
    assert_eq!(
        borrowed_content(&builder.types, &function, &[]),
        (1, vec![0])
    );
}

/// How many borrowed parameters of `f` hold values of a parameter its result
/// can hold and cannot copy, and which of them its result does not relate to.
fn borrowed_content(
    types: &TypeInterner,
    f: &FunctionType,
    schema: &[SchemaConstraint],
) -> (usize, Vec<usize>) {
    // Declared copying bounds; a local result adds its own (GEN-1).
    let mut copying: BTreeSet<TypeParamId> = f
        .generics
        .iter()
        .enumerate()
        .filter_map(|(ordinal, parameter)| {
            matches!(parameter, GenericParameter::Type { bounds, .. } if bounds.iter().any(copies))
                .then_some(TypeParamId(ordinal as u32))
        })
        .collect();
    copying.extend(schema.iter().filter_map(|constraint| match constraint {
        SchemaConstraint::Domain { parameter, .. } => Some(parameter.parameter()),
        SchemaConstraint::Bound {
            parameter, bound, ..
        } if copies(bound) => Some(parameter.parameter()),
        _ => None,
    }));
    let result = types.get(f.result).unwrap();
    if result.modes.uniqueness == Uniqueness::Unique {
        return (0, vec![]);
    }
    let held: BTreeSet<_> = content_parameters(types, f.result)
        .into_iter()
        .filter(|p| !copying.contains(p))
        .collect();
    let related = result.modes.relation.named_parameters().unwrap_or_default();
    let mut count = 0;
    let mut unrelated = vec![];
    for (ordinal, parameter) in f.parameters.iter().enumerate() {
        if parameter.passing != hirsel_syntax::PassingMode::Borrow
            || matches!(types.get(parameter.ty).unwrap().kind, TypeKind::Function(_))
            || content_parameters(types, parameter.ty).is_disjoint(&held)
        {
            continue;
        }
        count += 1;
        if !related.contains(&ParameterRef::bound(ParamId(ordinal as u32))) {
            unrelated.push(ordinal);
        }
    }
    (count, unrelated)
}

fn copies(bound: &Bound) -> bool {
    matches!(
        bound,
        Bound::Copyable | Bound::Data | Bound::Key | Bound::Flat | Bound::Plain
    )
}

/// The type parameters whose values a value of `root` may hold. A Sender or a
/// SlotId names its payload type but holds none of its values.
fn content_parameters(store: &TypeInterner, root: TypeId) -> BTreeSet<TypeParamId> {
    let mut found = BTreeSet::new();
    let mut pending = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        match &store.get(id).unwrap().kind {
            TypeKind::Parameter(p) if p.binder() == GenericBinder::Bound(0) => {
                found.insert(p.parameter());
            }
            TypeKind::Builtin {
                kind: BuiltinType::Sender | BuiltinType::SlotId,
                ..
            } => {}
            TypeKind::Tuple(parts) => pending.extend(parts.iter().copied()),
            TypeKind::Box(t) => pending.push(*t),
            TypeKind::Record(fields) => pending.extend(fields.iter().map(|f| f.ty)),
            TypeKind::Variant(variants) => {
                pending.extend(variants.iter().flat_map(|v| v.fields.iter().map(|f| f.1)))
            }
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => pending
                .extend(arguments.iter().filter_map(|a| match a {
                    GenericArgument::Type(t) => Some(*t),
                    GenericArgument::Size(_) => None,
                })),
            _ => {}
        }
    }
    found
}

#[test]
fn embedded_registry_matches_declaration_builder() {
    let types = TypeInterner::new(None);
    let expected = BuiltinRegistry::build(&types);
    let actual = BuiltinRegistry::shared();
    assert_eq!(actual.revision(), expected.revision());
    assert_eq!(actual.signatures().count(), expected.signatures().count());
    for signature in expected.signatures() {
        let decoded = actual.get(signature.definition).unwrap();
        assert_eq!(decoded.fingerprint, signature.fingerprint);
        assert_eq!(
            hirsel_types::encoding::fingerprint(actual.types(), &decoded, &[]).unwrap(),
            hirsel_types::encoding::fingerprint(&types, signature, &[]).unwrap()
        );
    }
}
