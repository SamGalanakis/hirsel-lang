use super::*;
#[test]
fn hex_digits_never_start_a_float_suffix() {
    let fixed = |s: &str| literals::fixed_primitive(&ast::Literal::Integer(s.into()));
    assert_eq!(fixed("0x1f32"), None);
    assert_eq!(fixed("0xf64"), None);
    assert_eq!(fixed("0xf32u16"), Some(Primitive::U16));
    assert_eq!(fixed("0xfu8"), Some(Primitive::U8));
    assert_eq!(
        literals::fixed_primitive(&ast::Literal::Float("1f32".into())),
        Some(Primitive::F32)
    );
}
mod contracts;
mod cursors;
mod hand;
mod properties;
mod support;

#[test]
fn reordered_record_patterns_map_children_to_declared_fields() {
    let (ds, outputs, _) = support::check_source(
        "type Rec { a: u8, b: u16 }\nfn main(r: Rec) { let Rec(b: unused, a: _) = r }",
    );
    assert!(ds.is_empty(), "{ds:?}");
    let body = &outputs[1].checked.value;
    let record = body
        .patterns
        .iter()
        .find(|p| matches!(p.kind, PatternDecisionKind::Constructor(_)))
        .unwrap();
    assert_eq!(
        record.fields.iter().map(|(f, _)| f.0).collect::<Vec<_>>(),
        vec![1, 0]
    );
    assert_eq!(body.bindings.len(), 2);
}
#[test]
fn callee_type_error_cites_the_complete_call_and_does_not_cascade() {
    let source = "fn takes(x: u16) {}\nfn main() { takes(1u8) }";
    let (ds, _, map) = support::check_source(source);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].rule.as_str(), "LANG-7");
    assert_eq!(map.slice(ds[0].primary), Some("takes(1u8)"));
    let (ds, _, _) =
        support::check_source("fn id[T](x:T) -> T { x }\nfn main() { let unused = id(1) }");
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(ds[0].rule.as_str(), "LANG-2");
}
#[test]
fn numeric_intrinsics_validate_types_and_static_conversion_ranges() {
    for (code, rule) in [
        ("fn main() -> u8 { int.widen(1u16) }", "SEM-1"),
        ("fn main() -> i8 { int.widen(1u8) }", "SEM-1"),
        ("fn main() -> u16 { int.truncate(1u8) }", "SEM-1"),
        ("fn main() -> u8 { float.min(1u8,2u8) }", "SEM-2"),
    ] {
        let (ds, _, _) = support::check_source(code);
        assert_eq!(ds.len(), 1, "{code}: {ds:?}");
        assert_eq!(ds[0].rule.as_str(), rule);
    }
    let (ds, _, _) = support::check_source("fn main() -> u16 { int.widen(1u8) }");
    assert!(ds.is_empty(), "{ds:?}");
}

#[test]
fn iterator_plan_uses_provider_option_tags_without_assuming_order() {
    let (file, _, diagnostics) =
        support::parse("fn main(mut it:Iter[u8]) { for x in mut it {} }", &[]);
    assert!(diagnostics.is_empty());
    let mut provider = support::registry(&file);
    let option = provider
        .signatures
        .get_mut(&support::core("Option"))
        .unwrap();
    let option = std::sync::Arc::make_mut(option);
    let SignatureKind::Type(td) = &mut option.kind else {
        panic!()
    };
    let mut row = option
        .types
        .get(td.representation.unwrap())
        .unwrap()
        .clone();
    let TypeKind::Variant(variants) = &mut row.kind else {
        panic!()
    };
    variants.reverse();
    td.representation = Some(option.types.push(row));
    option.fingerprint = ast::ContentHash::of_bytes(b"manufactured reversed Option tags");
    support::normalize_file(&file, &mut provider).unwrap();
    let item = file.roots[0];
    let resolution =
        hirsel_resolve::resolve_item(&file, item, &provider.index.clone(), &mut provider);
    assert!(resolution.query.diagnostics.is_empty());
    let signature = provider.signatures[&file.tree.items[item.0 as usize].definition].clone();
    let output = check_with_loop_protocols(
        LoopInput {
            body: Input {
                tree: &file.tree,
                item,
                context: &support::context(),
                resolution: &resolution,
                signature: &signature,
            },
            protocols: &hirsel_types::core_roles::loop_protocols(),
        },
        &mut provider,
    )
    .unwrap();
    assert!(output.checked.diagnostics.is_empty());
    let ForPlanKind::Direct {
        execution: ForExecution::Advance { operation, step },
        ..
    } = &output.plans.loops[0].kind
    else {
        panic!()
    };
    assert_eq!(operation, &support::core("iter.next"));
    assert_eq!(step.none_variant, VariantId(1));
    assert_eq!(step.some_variant, VariantId(0));
    assert!(
        output
            .checked
            .dependencies
            .iter()
            .any(|d| d.definition == support::core("Option")
                && d.fingerprint
                    == ast::ContentHash::of_bytes(b"manufactured reversed Option tags"))
    );
}

#[test]
fn intrinsic_domains_follow_the_callee_metadata_instead_of_its_name() {
    let source = "fn convert[T](value:T)->T { value }\nfn main()->u8 { convert(1u8) }";
    let (file, map, diagnostics) = support::parse(source, &[]);
    assert!(diagnostics.is_empty());
    let mut registry = support::registry(&file);
    support::normalize_file(&file, &mut registry).unwrap();
    let callee = &file.tree.items[file.roots[0].0 as usize].definition;
    let main = file.roots[1];
    let signature = registry.signatures[&file.tree.items[main.0 as usize].definition].clone();
    let resolution =
        hirsel_resolve::resolve_item(&file, main, &registry.index.clone(), &mut registry);
    let context = support::context();
    let input = || Input {
        tree: &file.tree,
        item: main,
        context: &context,
        resolution: &resolution,
        signature: &signature,
    };
    assert!(
        check_with_loop_protocols(
            LoopInput {
                body: input(),
                protocols: &hirsel_types::core_roles::loop_protocols()
            },
            &mut registry
        )
        .map(|out| out.checked)
        .unwrap()
        .diagnostics
        .is_empty()
    );
    std::sync::Arc::make_mut(registry.signatures.get_mut(callee).unwrap())
        .metadata
        .schema = vec![SchemaConstraint::Domain {
        parameter: TypeParameterRef::bound(TypeParamId(0)),
        domain: ScalarDomain::Float,
        rule: const { RuleId::of("SEM-2") },
    }];
    let output = check_with_loop_protocols(
        LoopInput {
            body: input(),
            protocols: &hirsel_types::core_roles::loop_protocols(),
        },
        &mut registry,
    )
    .map(|out| out.checked)
    .unwrap();
    assert_eq!(output.diagnostics.len(), 1);
    assert_eq!(output.diagnostics[0].rule.as_str(), "SEM-2");
    assert_eq!(
        map.slice(output.diagnostics[0].primary),
        Some("convert(1u8)")
    );
}

#[test]
fn same_spelled_user_operations_have_only_their_written_contracts() {
    let (diagnostics, _, _) = support::check_source(
        "fn widen(value:String)->String { value }\nfn main()->String { let s:String=\"hello\"; widen(s) }",
    );
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn comptime_runtime_dependencies_are_reported_at_the_name() {
    for (source, name) in [
        ("fn probe(x:u8)->u8{comptime x}", "x"),
        ("fn probe(x:u8)->u8{comptime (x+1u8)}", "x"),
        ("fn probe()->u8{let x=1u8;comptime x}", "x"),
        ("fn probe(x:u8){let f:fn()->u8=fn()->u8{comptime x}}", "x"),
    ] {
        let (diagnostics, _, sources) = support::check_source(source);
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            Some("CT-2"),
            "{source}: {diagnostics:?}"
        );
        assert_eq!(sources.slice(diagnostics[0].primary), Some(name));
        assert!(
            sources
                .slice(diagnostics[0].secondary[0].span)
                .unwrap()
                .starts_with("comptime")
        );
    }
    for source in [
        "fn probe(x:u8)->u8{comptime 1u8}",
        "fn probe(x:u8)->u8 requires false {comptime 1u8}",
        "fn probe[T]()->u8{comptime 1u8}",
        "fn probe[n:size]()->u64{comptime n}",
        "fn probe(x:u8)->u8{comptime {let y=1u8;y}}",
        "fn probe(x:u8)->u8{comptime (fn(z:u8)->u8{z})(1u8)}",
        "const VALUE:u8=1u8\nfn probe(x:u8)->u8{comptime VALUE}",
    ] {
        let (diagnostics, _, _) = support::check_source(source);
        assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
    }
}

#[test]
fn shape_mismatches_print_both_source_types() {
    for (source, expected, actual) in [
        (
            "fn g() -> Result[String, ()] { Ok(\"x\") }\nfn f() -> Result[String, ()] { g()? }",
            "Result[String, ()]",
            "String",
        ),
        (
            "fn decode(t: Bytes) -> Result[String, ()] { Ok(\"x\") }\nfn first(t: Bytes) -> String { decode(t) }",
            "String",
            "Result[String, ()]",
        ),
        (
            "fn f(x: u8) -> u8 { if true { x } else { false } }",
            "u8",
            "bool",
        ),
        ("fn f() -> bool { 1u8 == 2u16 }", "u8", "u16"),
        ("fn f() -> (u8, u64) { (1u8, false) }", "u64", "bool"),
    ] {
        let (diagnostics, _, _) = support::check_source(source);
        let diagnostic = diagnostics
            .first()
            .unwrap_or_else(|| panic!("{source}: {diagnostics:?}"));
        assert!(
            diagnostic
                .message
                .contains(&format!("expected `{expected}`")),
            "{source}: {diagnostic:?}"
        );
        assert!(
            diagnostic.message.contains(&format!("found `{actual}`")),
            "{source}: {diagnostic:?}"
        );
    }
}

#[test]
fn generic_function_value_ambiguity_names_the_missing_parameter() {
    for (declaration, missing) in [
        ("fn id[T, Missing](take x: T) -> T { x }", "Missing"),
        ("fn id[T, count: size](take x: T) -> T { x }", "count"),
    ] {
        let source =
            format!("{declaration}\nfn main() {{ let f: fn(take u64) -> u64 uses {{}} = id }}");
        let (diagnostics, _, map) = support::check_source(&source);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.rule.as_str(), "LANG-2");
        assert_eq!(map.slice(diagnostic.primary), Some("id"));
        assert!(diagnostic.message.contains(missing), "{diagnostic:?}");
    }
}
/// Usefulness splits a column only on constructors its rows name (Maranget), so
/// a box-recursive type is explored no deeper than its patterns: matrices grow
/// linearly with nesting depth and tuple width instead of unfolding the type.
#[test]
fn usefulness_work_grows_with_patterns_not_recursive_types() {
    let chain = "type Chain = End | Link(u8, box Chain)\n";
    let links =
        |n: usize, inner: &str| (0..n).fold(inner.to_owned(), |p, _| format!("Link(_, {p})"));
    let measure = |arms: &[String], parameters: &str, scrutinee: &str| {
        let source = format!(
            "{chain}fn order({parameters}) -> u8 {{ match {scrutinee} {{ {} }} }}",
            arms.join(" ")
        );
        let before = work::read(Counter::BodyUsefulnessMatrices);
        let (ds, _, _) = support::check_source(&source);
        let rules = ds
            .iter()
            .map(|d| d.rule.as_str().to_owned())
            .collect::<Vec<_>>();
        (rules, work::read(Counter::BodyUsefulnessMatrices) - before)
    };
    for n in [1, 2, 4, 8, 16, 32] {
        // Depth: End(), Link(_, End()), ... and n links over a wildcard.
        let mut arms: Vec<_> = (0..n)
            .map(|k| format!("{} => 0,", links(k, "End()")))
            .collect();
        arms.push(format!("{} => 1,", links(n, "_")));
        let (rules, work) = measure(&arms, "c: Chain", "c");
        eprintln!("depth {n}: {work} matrices");
        assert!(rules.is_empty(), "depth {n}: {rules:?}");
        assert!(work <= 4 * n as u64 + 4, "depth {n}: {work} matrices");
        arms.remove(n / 2);
        assert_eq!(measure(&arms, "c: Chain", "c").0, ["LANG-7"], "depth {n}");
        // Width: a tuple of n chains, End() in each position, then all links.
        if n == 1 {
            continue;
        }
        let wild = |i: usize, p: &str| {
            let cells: Vec<_> = (0..n).map(|j| if i == j { p } else { "_" }).collect();
            format!("({})", cells.join(", "))
        };
        let mut arms: Vec<_> = (0..n)
            .map(|i| format!("{} => 0,", wild(i, "End()")))
            .collect();
        arms.push(format!("({}) => 1,", vec!["Link(_, _)"; n].join(", ")));
        let parameters = (0..n)
            .map(|i| format!("c{i}: Chain"))
            .collect::<Vec<_>>()
            .join(", ");
        let scrutinee = format!(
            "({})",
            (0..n)
                .map(|i| format!("c{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let (rules, work) = measure(&arms, &parameters, &scrutinee);
        eprintln!("width {n}: {work} matrices");
        assert!(rules.is_empty(), "width {n}: {rules:?}");
        assert!(work <= 4 * n as u64 + 4, "width {n}: {work} matrices");
        arms.remove(n / 2);
        assert_eq!(
            measure(&arms, &parameters, &scrutinee).0,
            ["LANG-7"],
            "width {n}"
        );
    }
}
