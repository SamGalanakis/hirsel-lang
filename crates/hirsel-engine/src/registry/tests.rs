use super::review::*;
use super::*;
use hirsel_syntax::PackageId;
use hirsel_types::core_roles::{intrinsic_roles, loop_protocols};
#[test]
fn registry_has_canonical_signatures() {
    let r = BuiltinRegistry::new();
    for s in r.signatures() {
        assert_eq!(
            s.fingerprint,
            hirsel_types::encoding::fingerprint(s, &[]).unwrap()
        );
        assert_ne!(s.fingerprint, ContentHash::default());
    }
}

#[test]
fn every_declared_signature_matches_the_review_table_and_canonical_hash() {
    let registry = BuiltinRegistry::new();
    let table=registry.signatures().map(|s|{
        let path=format!("core@1/{}{}",if s.definition.0.modules.is_empty(){String::new()}else{format!("{}.",s.definition.0.modules.join("."))},s.definition.0.name);
        let declaration=match &s.kind{
            SignatureKind::Function(f)=>format!("{:?} {}",f.generics,describe_function(s,f)),
            SignatureKind::Const { ty: id, .. }=>format!("const {}",describe_type(s,*id)),
            SignatureKind::Type(t)=>format!("type {:?} {} ; bounds={:?} ; authority={:?} ; capability={:?} ; resource={} ; drop={:?}",t.generics,t.representation.map(|id|describe_type(s,id)).unwrap_or_else(||"opaque".into()),t.bounds,t.authority,t.capability,t.resource,t.destructor_allocation),
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
    for line in SECTION22
        .lines()
        .map(|l| {
            ["@preserves_unique(", "@unretained("]
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
}
#[test]
fn roles_and_provider_lookup_are_nominal_and_read_only() {
    let mut registry = BuiltinRegistry::new();
    let mut symbols = Interner::default();
    let path = hirsel_syntax::Path {
        package: None,
        segments: vec![symbols.intern("out"), symbols.intern("print")],
    };
    let context = FileContext {
        package: PackageId {
            name: "user".into(),
            version: "1".into(),
        },
        modules: vec![],
    };
    let lookup = registry.lookup(&context, &path, &symbols);
    assert_eq!(
        lookup.result.unwrap(),
        hirsel_syntax::DefPath::core("out.print")
    );
    assert_eq!(lookup.manifest, registry.revision());
    let mut foreign = path.clone();
    foreign.package = Some(context.package.clone());
    assert!(
        registry
            .lookup(&context, &foreign, &symbols)
            .result
            .is_err()
    );
    let role = registry
        .get(&hirsel_syntax::DefPath::core("Unsafe"))
        .unwrap();
    assert_eq!(role.metadata.roles, nominal_roles());
    for (path, _) in intrinsic_roles() {
        assert!(registry.get(&path).is_some());
    }
}
#[test]
fn compiler_loop_protocols_are_declared_and_cannot_resolve_from_source() {
    let r = BuiltinRegistry::new();
    assert_eq!(loop_protocols().len(), 6);
    assert_eq!(r.signatures().count(), 391);
    let mut symbols = Interner::default();
    for ((protocol, open, next), generics) in loop_protocols().into_iter().zip([2, 2, 1, 1, 1, 2]) {
        assert_eq!(open.0.modules, ["$intrinsic"]);
        assert_eq!(open.0.name, format!("{protocol}_open"));
        for path in [&open, &next] {
            let signature = r.get(path).unwrap();
            assert_eq!(
                signature.metadata.visibility,
                hirsel_syntax::Visibility::Private
            );
            let source = Path {
                package: Some(path.0.package.clone()),
                segments: vec![symbols.intern("$intrinsic"), symbols.intern(&path.0.name)],
            };
            assert!(r.lookup_name(&source, &symbols).is_none());
            let mut lex_sources = hirsel_span::SourceMap::default();
            let file = lex_sources
                .add(
                    "intrinsic.hir",
                    format!("fn main() -> () {{ core@1/$intrinsic.{}() }}", path.0.name),
                )
                .unwrap();
            assert!(
                !hirsel_lexer::lex(file, &lex_sources, &mut symbols)
                    .diagnostics
                    .is_empty()
            );
        }
        let open = r.get(&open).unwrap();
        let SignatureKind::Function(f) = &open.kind else {
            panic!()
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
        let result = open.types.get(f.result).unwrap();
        // An owning cursor is as local as the collection it takes.
        assert_eq!(
            result.modes.relation,
            Relation::parameters([hirsel_types::ParameterRef::bound(ParamId(0))])
        );
        let map = matches!(protocol, "MapInsertionOrder" | "SortedMapOrder");
        let cursor_name = match protocol {
            "MapInsertionOrder" => "$intrinsic.OwnedMapCursor",
            "SortedMapOrder" => "$intrinsic.OwnedSortedMapCursor",
            _ if consuming => "$intrinsic.OwnedCursor",
            "UnicodeScalars" | "ScalarPositions" => "$intrinsic.ViewCursor",
            _ => "$intrinsic.Cursor",
        };
        assert!(
            matches!(&result.kind,TypeKind::Named {definition,arguments} if *definition==hirsel_syntax::DefPath::core(cursor_name) && arguments.len()==if map {2} else {1})
        );
        let next = r.get(&next).unwrap();
        let SignatureKind::Function(f) = &next.kind else {
            panic!()
        };
        assert_eq!(f.generics.len(), if map { 2 } else { 1 });
        assert_eq!(f.parameters[0].passing, PassingMode::Mut);
        assert_eq!(f.allocation, Allocation::NoAlloc);
        assert!(
            matches!(&next.types.get(f.result).unwrap().kind,TypeKind::Named {definition,arguments} if *definition==hirsel_syntax::DefPath::core("Option") && arguments.len()==1)
        );
    }
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
        owned
            .types
            .get(owner.representation.unwrap())
            .unwrap()
            .drop_allocation,
        DropAllocation::DependsOn(vec![TypeParameterRef::bound(TypeParamId(0))].into())
    );
}

#[test]
fn generic_variant_cleanup_retains_conditional_owned_payload_obligations() {
    let registry = BuiltinRegistry::new();
    for (name, parameters) in [
        ("Option", vec![TypeParameterRef::bound(TypeParamId(0))]),
        (
            "Result",
            vec![
                TypeParameterRef::bound(TypeParamId(0)),
                TypeParameterRef::bound(TypeParamId(1)),
            ],
        ),
    ] {
        let s = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Type(t) = &s.kind else {
            panic!()
        };
        assert_eq!(
            s.types
                .get(t.representation.unwrap())
                .unwrap()
                .drop_allocation,
            DropAllocation::DependsOn(parameters.into())
        );
    }
}

#[test]
fn slot_id_results_are_plain_even_with_an_unbounded_payload_parameter() {
    let registry = BuiltinRegistry::new();
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("slots.insert"))
        .unwrap();
    let SignatureKind::Function(function) = &signature.kind else {
        panic!()
    };
    let result = signature.types.get(function.result).unwrap();
    assert!(matches!(
        result.kind,
        TypeKind::Builtin {
            kind: BuiltinType::SlotId,
            ..
        }
    ));
    assert_eq!(result.modes.relation, Relation::global());
    assert_eq!(result.modes.sendability, Sendability::Sendable);
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
        signature
            .types
            .get(function.result)
            .unwrap()
            .modes
            .sendability,
        // The unbounded Local declaration has no proof.
        Sendability::NotSendable
    );
    for (kind, sendability) in [
        (Primitive::U64, Sendability::Sendable),
        (Primitive::Bytes, Sendability::NotSendable),
    ] {
        let mut target = TypeStore::default();
        let actual = target.push(Type {
            modes: Modes {
                sendability: sendability.clone(),
                ..Modes::global()
            },
            ..Type::with_kind(TypeKind::Primitive(kind))
        });
        let result = substitute_type(
            &signature.types,
            function.result,
            &mut target,
            Substitution {
                binder: Some(hirsel_types::GenericBinder::Bound(0)),
                roles: &Default::default(),
                arguments: &[GenericArgument::Type(actual)],
                authorities: &[AuthoritySummary::inert()],
            },
        )
        .unwrap();
        let result = canonical_current_type(
            &mut target,
            result,
            &GenericDeclarations::default(),
            &mut |path| Ok::<_, QueryError>(registry.get(path)),
        )
        .unwrap();
        assert_eq!(target.get(result).unwrap().modes.sendability, sendability);
    }
}

#[test]
fn forget_is_the_declared_noalloc_consuming_resource_intrinsic() {
    let registry = BuiltinRegistry::new();
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("forget"))
        .unwrap();
    let SignatureKind::Function(function) = &signature.kind else {
        panic!()
    };
    assert_eq!(
        function.generics,
        vec![GenericParameter::Type {
            name: "T".into(),
            bounds: vec![].into()
        }]
    );
    assert_eq!(function.parameters.len(), 1);
    assert_eq!(
        function.parameters[0].passing,
        hirsel_syntax::PassingMode::Take
    );
    assert_eq!(
        signature.types.get(function.parameters[0].ty).unwrap().kind,
        TypeKind::Parameter(TypeParameterRef::bound(TypeParamId(0)))
    );
    assert!(
        matches!(signature.types.get(function.result).unwrap().kind, TypeKind::Tuple(ref fields) if fields.is_empty())
    );
    assert_eq!(function.allocation, Allocation::NoAlloc);
}

#[test]
fn set_cursor_open_preserves_the_core_key_bound() {
    let registry = BuiltinRegistry::new();
    let (_, open, _) = loop_protocols()
        .into_iter()
        .find(|(label, _, _)| *label == "SetElements")
        .unwrap();
    let signature = registry.get(&open).unwrap();
    let SignatureKind::Function(function) = &signature.kind else {
        panic!()
    };
    assert!(
        matches!(function.generics.as_slice(), [GenericParameter::Type { bounds, .. }] if bounds == &[Bound::Key])
    );
    assert!(matches!(
        signature.types.get(function.parameters[0].ty).unwrap().kind,
        TypeKind::Builtin {
            kind: BuiltinType::Set,
            ..
        }
    ));
}

#[test]
fn heap_operation_declares_the_canonical_ambient_allocator() {
    let registry = BuiltinRegistry::new();
    let role = nominal_roles().heap.unwrap();
    let s = registry.get(&role.operation).unwrap();
    assert_eq!(s.metadata.visibility, hirsel_syntax::Visibility::Private);
    let SignatureKind::Function(f) = &s.kind else {
        panic!()
    };
    assert!(f.parameters.is_empty() && f.generics.is_empty());
    assert_eq!(f.allocation, Allocation::NoAlloc);
    assert_eq!(f.authority, AuthoritySummary::inert());
    assert!(
        s.types
            .get(f.result)
            .unwrap()
            .is_heap_target(&nominal_roles())
    );
    let mut symbols = Interner::default();
    let source = Path {
        package: Some(role.operation.0.package.clone()),
        segments: vec![symbols.intern("$intrinsic"), symbols.intern("heap")],
    };
    assert!(registry.lookup_name(&source, &symbols).is_none());
}

#[test]
fn schema_cleanup_uses_declared_ordinals_instead_of_metavariable_spelling() {
    for (generics, input, output) in [
        ("I,J", "I", "J"),
        ("Source,Destination", "Source", "Destination"),
    ] {
        let mut builder = build::Builder::new(generics).with_schema(schema("int.widen"));
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
    let mut ordinary = build::Builder::new("I");
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
        hirsel_types::encoding::fingerprint(&changed, &[]).unwrap()
    );
    for index in 0..signature.metadata.schema.len() {
        let mut changed = signature.as_ref().clone();
        changed.metadata.schema.remove(index);
        assert_ne!(
            signature.fingerprint,
            hirsel_types::encoding::fingerprint(&changed, &[]).unwrap()
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
        hirsel_types::encoding::fingerprint(&signature, &[]).unwrap(),
        signature.fingerprint
    );
}

#[test]
fn shared_authority_retains_the_payload_promise() {
    let registry = BuiltinRegistry::new();
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("Shared"))
        .unwrap();
    let SignatureKind::Type(declaration) = &signature.kind else {
        panic!();
    };
    assert_eq!(
        declaration.authority,
        AuthoritySummary::Known(
            vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                TypeParamId(0)
            ))]
            .into()
        )
    );
    let freeze = registry
        .get(&hirsel_syntax::DefPath::core("shared.freeze"))
        .unwrap();
    let SignatureKind::Function(freeze) = &freeze.kind else {
        panic!();
    };
    assert!(
        matches!(&freeze.generics[0], GenericParameter::Type { bounds, .. } if bounds.contains(&Bound::Inert))
    );
}

#[test]
fn library_declarations_obey_the_named_source_and_part_locality_rules() {
    // A named function's local result names its sources (MODE-1), and a
    // declared field is as local as its owner (LANG-7): the owner's
    // 2026-10-10 rulings on spec #70 and #71 bind library signatures too.
    let r = BuiltinRegistry::new();
    for s in r.signatures() {
        let parts: Vec<TypeId> = match &s.kind {
            SignatureKind::Function(f) => {
                assert_ne!(
                    s.types.get(f.result).unwrap().modes.relation,
                    Relation::environment(),
                    "{:?}",
                    s.definition
                );
                continue;
            }
            SignatureKind::Type(t) => match t.representation.map(|r| &s.types.get(r).unwrap().kind)
            {
                Some(TypeKind::Record(fields)) => fields.iter().map(|f| f.ty).collect(),
                Some(TypeKind::Variant(variants)) => variants
                    .iter()
                    .flat_map(|v| v.fields.iter().map(|f| f.1))
                    .collect(),
                _ => continue,
            },
            _ => continue,
        };
        for part in parts {
            assert_eq!(
                s.types.get(part).unwrap().modes.relation,
                Relation::global(),
                "{:?}",
                s.definition
            );
        }
    }
}

#[test]
fn clocks_have_independent_nominal_authority_and_instant_has_no_public_representation_or_data_bounds()
 {
    let registry = BuiltinRegistry::new();
    assert!(
        registry
            .get(&hirsel_syntax::DefPath::core("Clock"))
            .is_none()
    );
    assert!(
        registry
            .get(&hirsel_syntax::DefPath::core("io.clock"))
            .is_none()
    );
    for name in ["MonotonicClock", "WallClock"] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Type(definition) = &signature.kind else {
            panic!()
        };
        assert_eq!(definition.capability, Some(CapabilityKind::Global));
        assert_eq!(
            definition.authority,
            AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                    name
                ))]
                .into()
            )
        );
    }
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("Instant"))
        .unwrap();
    let SignatureKind::Type(definition) = &signature.kind else {
        panic!()
    };
    assert_eq!(
        signature.metadata.visibility,
        hirsel_syntax::Visibility::PublicOpaque
    );
    assert!(definition.representation.is_none());
    assert_eq!(definition.capability, None);
    assert!(!definition.serializable);
    assert!(definition.authority == AuthoritySummary::inert());
    for bound in [Bound::Plain, Bound::Inert, Bound::Sendable] {
        assert!(definition.bounds.contains(&bound));
    }
    for bound in [Bound::Data, Bound::Key, Bound::Flat, Bound::Serializable] {
        assert!(!definition.bounds.contains(&bound));
    }
    let sample = registry
        .get(&hirsel_syntax::DefPath::core("clock.monotonic"))
        .unwrap();
    let SignatureKind::Function(function) = &sample.kind else {
        panic!()
    };
    let result = sample.types.get(function.result).unwrap();
    assert!(result.brand.alternatives.is_empty());
    let TypeKind::Named { arguments, .. } = &result.kind else {
        panic!()
    };
    let GenericArgument::Type(payload) = arguments[0] else {
        panic!()
    };
    assert_eq!(
        sample.types.get(payload).unwrap().brand.alternatives,
        [Origin {
            root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
            fields: Default::default()
        }]
    );
}
#[test]
fn ledger_never_is_the_primitive_and_core_names_have_providers() {
    // A ledger `Never` read as the nominal core@1/Never had no provider: every std body
    // calling test.fail stopped checking, and every std-dependent program with it.
    let registry = BuiltinRegistry::new();
    let signature = registry
        .get(&hirsel_syntax::DefPath::core("test.fail"))
        .unwrap();
    let SignatureKind::Function(function) = &signature.kind else {
        panic!()
    };
    assert_eq!(
        signature.types.get(function.result).unwrap().kind,
        TypeKind::Primitive(Primitive::Never)
    );
    for signature in registry.signatures() {
        for (_, t) in signature.types.iter() {
            if let TypeKind::Named { definition, .. } = &t.kind {
                if definition.0.package.name_is("core") {
                    assert!(registry.get(definition).is_some(), "{definition:?}");
                }
            }
        }
    }
}

#[test]
fn network_types_preserve_octet_sizes_variant_ordinals_and_script_fields() {
    let registry = BuiltinRegistry::new();
    for (name, expected) in [
        (
            "IpAddr",
            "V4(Inline[U8,Constant(4)])|V6(Inline[U8,Constant(16)])",
        ),
        ("SocketAddr", "{ip:IpAddr[],port:U16}"),
        (
            "NetOperation",
            "Connect()|ConnectTls()|Listen()|UdpBind()|UdpPeer()",
        ),
        (
            "NetRule",
            "{address:String,port:U16,operation:NetOperation[]}",
        ),
        ("NetTimeout", "Unbounded()|Nanos(U64)"),
        ("Shutdown", "Read()|Write()|Both()"),
        ("TlsRoots", "WebPki()|Pem(Bytes)"),
        (
            "TlsFailure",
            "UnknownIssuer()|Expired()|NotYetValid()|NameMismatch()|Revoked()|BadCertificate()|AlertReceived(U8)|HandshakeFailure()|PeerMisbehaved()|Truncated()",
        ),
        (
            "NetError",
            "NotGranted()|Canceled(CancelCause[])|InvalidInput()|UnknownHost()|ResolverFailure()|ConnectionRefused()|ConnectionReset()|ConnectionAborted()|NotConnected()|AddrInUse()|AddrNotAvailable()|HostUnreachable()|NetworkUnreachable()|NetworkDown()|BrokenPipe()|TimedOut()|Tls(TlsFailure[])|Fault(U32)",
        ),
        (
            "NetPeer",
            "{address:String,port:U16,resolves_to:List[IpAddr[]],tls:PeerTls[],connections:List[NetConnection[]]}",
        ),
        (
            "NetListener",
            "{address:SocketAddr[],clients:List[NetClient[]]}",
        ),
        (
            "UdpReceived",
            "{count:U64,peer:SocketAddr[],truncated:Bool}",
        ),
        ("NetClient", "{from:SocketAddr[],steps:List[NetStep[]]}"),
        (
            "PeerTls",
            "Plain()|Certificate(List[String])|Fails(TlsFailure[])",
        ),
        (
            "NetConnection",
            "Refused()|Unanswered()|Open(List[NetStep[]])",
        ),
        (
            "NetStep",
            "Send(Bytes)|Expect(Bytes)|Stall()|Close()|Reset()|Truncate()",
        ),
    ] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Type(declaration) = &signature.kind else {
            panic!("{name}")
        };
        assert_eq!(
            describe_type(&signature, declaration.representation.unwrap()),
            expected,
            "{name}"
        );
    }
}

#[test]
fn network_and_process_resources_are_sendable_noncopyable_and_close_without_allocation() {
    let registry = BuiltinRegistry::new();
    for (name, capability) in [
        ("Stream", "Net"),
        ("Listener", "Net"),
        ("UdpSocket", "Net"),
        ("Child", "Proc"),
    ] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Type(declaration) = &signature.kind else {
            panic!("{name}")
        };
        assert!(declaration.resource);
        assert!(declaration.representation.is_none());
        assert_eq!(
            signature.metadata.visibility,
            hirsel_syntax::Visibility::PublicOpaque
        );
        assert_eq!(declaration.bounds, [Bound::Sendable, Bound::NoallocDrop]);
        assert_eq!(declaration.destructor_allocation, Allocation::NoAlloc);
        assert_eq!(
            declaration.authority,
            AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                    capability
                ))]
                .into()
            )
        );
        assert_eq!(declaration.drop, Some(hirsel_syntax::DefPath::core(name)));
    }
    // Counted script/certificate data must not receive the Plain/Sendable promises
    // used by the scalar error variants above.
    for name in ["TlsRoots", "PeerTls", "NetConnection", "NetStep"] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Type(declaration) = &signature.kind else {
            panic!("{name}")
        };
        assert!(!declaration.bounds.contains(&Bound::Plain));
        assert!(!declaration.bounds.contains(&Bound::Sendable));
    }
}

#[test]
fn network_signatures_retain_stream_brands_and_tls_consumption() {
    let registry = BuiltinRegistry::new();
    for name in [
        "net.connect",
        "net.connect_addr",
        "net.udp_bind",
        "net.listen",
        "net.accept",
        "tls.connect",
    ] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Function(function) = &signature.kind else {
            panic!("{name}")
        };
        let TypeKind::Named { arguments, .. } = &signature.types.get(function.result).unwrap().kind
        else {
            panic!("{name}")
        };
        let GenericArgument::Type(payload) = arguments[0] else {
            panic!("{name}")
        };
        let payload = if name == "net.accept" {
            let TypeKind::Tuple(fields) = &signature.types.get(payload).unwrap().kind else {
                panic!("{name}")
            };
            fields[0]
        } else {
            payload
        };
        assert_eq!(
            signature.types.get(payload).unwrap().brand.alternatives,
            [Origin {
                root: OriginRoot::Parameter(ParameterRef::bound(ParamId(0))),
                fields: Default::default()
            }]
        );
    }
    for name in ["net.close", "udp.close", "tls.connect", "tls.close"] {
        let signature = registry.get(&hirsel_syntax::DefPath::core(name)).unwrap();
        let SignatureKind::Function(function) = &signature.kind else {
            panic!("{name}")
        };
        assert_eq!(function.parameters[1].passing, PassingMode::Take);
        assert_eq!(
            signature
                .types
                .get(function.parameters[1].ty)
                .unwrap()
                .drop_allocation,
            DropAllocation::NoAlloc
        );
    }
}
