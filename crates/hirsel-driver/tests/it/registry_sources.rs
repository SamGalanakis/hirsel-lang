//! Builtin registry signatures agree with the same declarations written as source
//! and normalized, checked and lowered by the real front phases.
use hirsel_engine::registry::review::describe_function;
use hirsel_engine::registry::*;
use hirsel_engine::*;
use hirsel_intern::Interner;
use hirsel_types::*;

#[test]
fn std_catalogue_signatures_match_the_complete_ordinary_source_contracts() {
    use hirsel_check_protocol::header::Input;
    use hirsel_signature::normalize;
    use hirsel_span::SourceMap;
    use hirsel_syntax::Visibility;

    hirsel_engine::with_compiler_stack(|| {
        // Read the normative declarations independently of executable source.
        let catalogue = include_str!("../../../../docs/spec/std-library.md")
            .split("\n## The catalogue\n")
            .nth(1)
            .unwrap()
            .split("```text\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/std_source/main.hir");
        let mut loaded = hirsel_driver::load_program_source(&root, Some("")).unwrap();
        let registry = BuiltinRegistry::new();
        let mut metadata = Metadata {
            unsafe_capability: nominal_roles().unsafe_capability,
            result_type: nominal_roles().result_type,
            tasks_capability: nominal_roles().tasks_capability,
            scope_capability: nominal_roles().scope_capability,
            heap: nominal_roles().heap,
            ..Default::default()
        };
        for signature in registry.signatures() {
            metadata.definitions.insert(
                signature.definition.clone(),
                DefinitionMetadata {
                    public: true,
                    alias: signature.metadata.alias,
                },
            );
        }
        let mut checked = Vec::new();
        for line in catalogue.lines() {
            let (caller, line) = match line.strip_prefix("@track_caller ") {
                Some(declaration) => ("@track_caller\n", declaration),
                None => ("", line),
            };
            // A `@preserves_unique(p) ` prefix is the declaration's own attribute.
            let (promise, declaration) = match line.strip_prefix("@preserves_unique(") {
                Some(rest) => {
                    let (selected, declaration) = rest.split_once(") ").unwrap();
                    (format!("@preserves_unique({selected})\n"), declaration)
                }
                None => (String::new(), line),
            };
            let noalloc = declaration.starts_with("N ");
            let text = declaration
                .trim_start_matches("N ")
                .strip_prefix("fn ")
                .unwrap();
            let qualified = text.split(['[', '(']).next().unwrap();
            let name = qualified;
            let mut path = hirsel_syntax::DefPath::core(name);
            path.0.package.name = "std".into();
            let context = FileContext {
                package: path.0.package.clone(),
                modules: path.0.modules.clone(),
            };
            let source = format!(
                "{caller}{promise}{}pub fn {}{}\n",
                if noalloc { "@noalloc\n" } else { "" },
                path.0.name,
                &text[qualified.len()..]
            );
            let mut sources = SourceMap::default();
            let file = sources.add("catalogue.hir", source).unwrap();
            let mut symbols = Interner::default();
            let lexed = hirsel_lexer::lex(file, &sources, &mut symbols);
            assert!(lexed.diagnostics.is_empty());
            let parsed = hirsel_parser::parse(file, lexed.tokens, &context, &sources, &symbols);
            assert!(
                parsed.diagnostics.is_empty(),
                "{qualified}: {:?}",
                parsed.diagnostics
            );
            let expected = normalize(
                Input {
                    tree: &parsed.syntax.tree,
                    item: parsed.syntax.roots[0],
                    symbols: &symbols,
                    context: &context,
                    resolution: &Resolution::default(),
                    metadata: &metadata,
                    declaration_sites: &metadata.declarations,
                },
                &mut loaded.engine,
            )
            .unwrap()
            .value;
            let actual = loaded.engine.signature(&path).unwrap();
            assert_eq!(actual.metadata.visibility, Visibility::Public);
            assert_eq!(actual.definition, expected.definition);
            // Hash both complete contracts with the same empty dependency list:
            // package resolution may record additional transitive read hashes.
            // Every type, bound, mode, relation and effect still participates.
            assert_eq!(
                hirsel_types::encoding::fingerprint(&actual, &[]).unwrap(),
                hirsel_types::encoding::fingerprint(&expected, &[]).unwrap(),
                "{qualified}"
            );
            assert!(
                registry.get(&hirsel_syntax::DefPath::core(name)).is_none(),
                "core alias: {name}"
            );
            checked.push(name.to_owned());
        }
        assert_eq!(
            checked,
            [
                "ascii.is_digit",
                "ascii.is_alpha",
                "ascii.is_alnum",
                "ascii.is_space",
                "ascii.is_upper",
                "ascii.is_lower",
                "ascii.to_upper",
                "ascii.to_lower",
                "ascii.digit_value",
                "char.from_ascii",
                "char.from_u8",
                "char.format_into",
                "char.to_string",
                "path.normalize",
                "path.join",
                "path.parent",
                "path.filename",
                "path.extension",
                "path.components",
                "duration.from_parts",
                "duration.parse",
                "duration.to_string",
                "time.parse",
                "time.to_string",
                "time.elapsed",
                "time.between",
                "time.compare",
                "time.deadline",
                "time.sleep",
                "bytes.replace",
                "bytes.from_inline",
                "map.same_entries",
                "unicode.graphemes",
                "json.parse",
                "json.stringify",
                "json.stringify_into",
                "test.equal",
                "test.approx",
                "test.assert_eq",
                "test.property",
                "test.expect_error",
                "input.read_all",
                "input.read_line",
                "prng.seed",
                "prng.u64",
                "prng.below",
                "prng.bytes",
                "random.u64",
                "random.below"
            ]
        );
    });
}

#[test]
fn ordinary_ledger_signatures_agree_with_the_separate_source_normalizer() {
    use hirsel_check_protocol::header::Input;
    use hirsel_signature::normalize;
    use hirsel_span::SourceMap;
    use hirsel_syntax::Visibility;
    let registry = BuiltinRegistry::new();
    let mut checked = 0;
    for line in SECTION22.lines() {
        // A `@preserves_unique(p) ` prefix is the declaration's own attribute.
        let (promise, declaration) = match line.strip_prefix("@preserves_unique(") {
            Some(rest) => {
                let (selected, declaration) = rest.split_once(") ").unwrap();
                (format!("@preserves_unique({selected})\n"), declaration)
            }
            None => (String::new(), line),
        };
        // An `@unretained(q, ..) ` prefix after it has no source form: the
        // oracle below supplies it by parameter name.
        let (unretained, declaration) = match declaration.strip_prefix("@unretained(") {
            Some(rest) => {
                let (names, declaration) = rest.split_once(") ").unwrap();
                (names.split(", ").collect::<Vec<_>>(), declaration)
            }
            None => (Vec::new(), declaration),
        };
        if !(declaration.starts_with("fn ") || declaration.starts_with("N fn ")) {
            continue;
        }
        let noalloc = declaration.starts_with("N ");
        let text = declaration
            .trim_start_matches("N ")
            .strip_prefix("fn ")
            .unwrap();
        let name = text.split(['[', '(']).next().unwrap();
        // Shared FunctionType deliberately has no numeric-schema representation:
        // those domains/default plain modes are intrinsic signature metadata.
        if name.starts_with("int.")
            || name.starts_with("float.")
            || name.starts_with("simd.")
            || name.starts_with("atomic.")
        {
            continue;
        }
        let path = hirsel_syntax::DefPath::core(name);
        let source = format!(
            "{promise}{}pub fn {}{}\n",
            if noalloc { "@noalloc\n" } else { "" },
            "ledger",
            &text[name.len()..]
        );
        let context = FileContext {
            package: path.0.package.clone(),
            modules: path.0.modules.clone(),
        };
        let mut sources = SourceMap::default();
        let file = sources.add("ledger.hir", source).unwrap();
        let mut symbols = Interner::default();
        let lexed = hirsel_lexer::lex(file, &sources, &mut symbols);
        assert!(lexed.diagnostics.is_empty());
        let parsed = hirsel_parser::parse(file, lexed.tokens, &context, &sources, &symbols);
        assert!(
            parsed.diagnostics.is_empty(),
            "{name}: {:?}",
            parsed.diagnostics
        );
        let mut metadata = Metadata {
            unsafe_capability: nominal_roles().unsafe_capability,
            result_type: nominal_roles().result_type,
            tasks_capability: nominal_roles().tasks_capability,
            scope_capability: nominal_roles().scope_capability,
            heap: nominal_roles().heap,
            ..Default::default()
        };
        for signature in registry.signatures() {
            metadata.definitions.insert(
                signature.definition.clone(),
                DefinitionMetadata {
                    public: true,
                    alias: signature.metadata.alias,
                },
            );
        }
        let mut provider = registry.clone();
        let mut normalized = normalize(
            Input {
                tree: &parsed.syntax.tree,
                item: parsed.syntax.roots[0],
                symbols: &symbols,
                context: &context,
                resolution: &Resolution::default(),
                metadata: &metadata,
                declaration_sites: &metadata.declarations,
            },
            &mut provider,
        )
        .unwrap_or_else(|e| panic!("{name}: {e:?}"))
        .value;
        // The written header oracle remains exact. SEM-4 additionally specifies
        // the erased environments of these two constructors; source syntax has
        // no capture-selection clause. Supply that independent semantic oracle,
        // and compare it along with every previously checked header property.
        let expected_captures = match name {
            "iter.from" | "iter.map" => Some(ErasedCaptures::FromArguments(
                vec![
                    ParameterRef::bound(ParamId(0)).into(),
                    ParameterRef::bound(ParamId(1)).into(),
                ]
                .into(),
            )),
            _ => None,
        };
        if let SignatureKind::Function(f) = &mut normalized.kind {
            if !unretained.is_empty() {
                let ordinals: Vec<ParamId> = unretained
                    .iter()
                    .map(|n| {
                        let p = f
                            .parameters
                            .iter()
                            .position(|p| p.name.as_deref() == Some(n));
                        ParamId(p.unwrap() as u32)
                    })
                    .collect();
                let mut promises = f.state_promises.to_vec();
                promises[0].unretained = ordinals.into();
                f.state_promises = promises.into();
            }
            let mut result = normalized.types.get(f.result).unwrap().clone();
            assert!(result.erased_captures.is_none());
            result.erased_captures = expected_captures;
            f.result = normalized.types.push(result);
            // Spec #91: an advance-only row's state parameter keeps the
            // argument's own captures.
            if name == "iter.next" {
                let parameter = &mut f.parameters[0];
                let mut state = normalized.types.get(parameter.ty).unwrap().clone();
                assert!(state.erased_captures.is_none());
                state.erased_captures = Some(ErasedCaptures::FromArguments(
                    vec![ParameterRef::bound(ParamId(0)).into()].into(),
                ));
                parameter.ty = normalized.types.push(state);
            }
        }
        let declared = registry.get(&path).unwrap();
        if let (SignatureKind::Function(a), SignatureKind::Function(b)) =
            (&declared.kind, &normalized.kind)
        {
            assert_eq!(
                describe_function(declared.as_ref(), a),
                describe_function(&normalized, b),
                "{name}"
            );
            assert_eq!(a.generics, b.generics, "{name}");
        } else {
            panic!("{name}")
        }
        assert_eq!(normalized.metadata.visibility, Visibility::Public);
        checked += 1;
    }
    assert_eq!(checked, 172);
}

#[test]
fn builtin_print_result_agrees_with_the_same_written_source_result() {
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let file = engine
        .sources
        .add(
            "forward.hir",
            "fn forward(out:Stdout, text:String) -> Result[(),OutputError] { out.print(out,text) }"
                .into(),
        )
        .unwrap();
    engine
        .parse_file(file, &hirsel_engine::default_context())
        .unwrap();
    let mut path = hirsel_engine::entry_path(&hirsel_engine::default_context());
    path.0.name = "forward".into();
    let source = engine.item_signature(&path).unwrap().value;
    let registry = BuiltinRegistry::new();
    let builtin = registry
        .get(&hirsel_syntax::DefPath::core("out.print"))
        .unwrap();
    let SignatureKind::Function(a) = &source.kind else {
        panic!()
    };
    let SignatureKind::Function(b) = &builtin.kind else {
        panic!()
    };
    assert_eq!(
        source.types.get(a.result).unwrap().drop_allocation,
        DropAllocation::NoAlloc
    );
    assert!(semantic_equal(&source.types, a.result, &builtin.types, b.result).unwrap());
}

#[test]
fn declaration_body_and_builtin_cleanup_agree_on_owned_aggregate_results() {
    use hirsel_engine::DefinitionQueries;
    for (result, body, builtin_name) in [
        ("()", "()", "test.check"),
        ("Option[u8]", "Some(1u8)", "int.try_narrow"),
        ("Result[(),OutputError]", "Ok(())", "out.print"),
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let source = format!("fn probe() -> {result} {{ {body} }}");
        let file = engine.sources.add("cleanup.hir", source).unwrap();
        engine
            .parse_file(file, &hirsel_engine::default_context())
            .unwrap();
        let mut path = hirsel_engine::entry_path(&hirsel_engine::default_context());
        path.0.name = "probe".into();
        let signature = engine.item_signature(&path).unwrap().value;
        let checked = engine.check_definition(&path).unwrap().value.checked;
        let SignatureKind::Function(function) = &signature.kind else {
            panic!()
        };
        let declared = &signature
            .types
            .get(function.result)
            .unwrap()
            .drop_allocation;
        assert_eq!(*declared, DropAllocation::NoAlloc, "{result}");
        let actual = checked.expressions.last().unwrap().ty;
        assert_eq!(
            &checked.types.get(actual).unwrap().drop_allocation,
            declared,
            "{body}"
        );
        let builtin = BuiltinRegistry::new()
            .get(&hirsel_syntax::DefPath::core(builtin_name))
            .unwrap();
        let SignatureKind::Function(function) = &builtin.kind else {
            panic!()
        };
        assert_eq!(
            &builtin.types.get(function.result).unwrap().drop_allocation,
            declared,
            "{builtin_name}"
        );
    }
}
