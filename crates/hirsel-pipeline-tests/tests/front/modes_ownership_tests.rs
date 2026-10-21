//! Ownership checking of real source, from hirsel-ownership's unit tests.
use hirsel_syntax::*;
use hirsel_types::*;

#[test]
fn dependent_getters_preserve_selected_arena_placements() {
    for source in [
        "fn read(a: Arena, x: Bytes in a) -> Bytes in x { x }\nfn use(a: Arena) { let x: Bytes in a = b\"x\"; let y = read(a, x); let _ = y }",
        "fn choose(a: Arena, b: Arena, x: Bytes in a, y: Bytes in b) -> Bytes in {x, y} { x }\nfn use(a: Arena, b: Arena, x: Bytes in a, y: Bytes in b) -> Bytes in {x, y} { choose(a, b, x, y) }",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let file = engine.sources.add("getter.hir", source.into()).unwrap();
        let diagnostics =
            hirsel_engine::check_snapshot(&mut engine, file, &hirsel_engine::default_context())
                .unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}

#[test]
fn iteration_bindings_use_element_evidence_instead_of_container_uniqueness() {
    use hirsel_engine::DefinitionQueries;
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let source = "resource type Token { number:u8 }\ndrop(x) {}\nfn probe() { let xs:Array[Token]=[Token(number:1u8)]; for item in take xs { let _ = take item } }";
    let context = hirsel_engine::default_context();
    let file = engine
        .sources
        .add("cursor-element.hir", source.into())
        .unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut definition = hirsel_engine::entry_path(&context);
    definition.0.name = "probe".into();
    // Publication independently checks that binding storage receives the
    // element actually promised by next(), and owned items still finalize.
    let out = engine.lower_to_tir(&definition).unwrap();
    assert!(!out.value.bodies[0].blocks.is_empty());
}

#[test]
fn nested_generic_plain_bound_cannot_copy_a_mixed_outer_payload() {
    use hirsel_engine::DefinitionQueries;
    let source = "fn outer[T:copyable](x:T)->(){fn inner[U:plain](take y:U)->(){let pair=(copy x,y);let a=pair;let b=pair};()}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("mixed.hir", source.into()).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut definition = hirsel_engine::entry_path(&context);
    definition.0.name = "outer".into();
    let diagnostics = match engine.mode_definition(&definition) {
        Ok(out) => out.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(error) => panic!("{error:?}"),
    };
    assert_eq!(
        diagnostics.first().map(|d| d.rule.as_str()),
        Some("OWN-2"),
        "{diagnostics:?}"
    );
}

/// Spec issue #50: a branch, arm or block is transparent to its consumer's
/// use. The published use of the joined value is the join over every path, in
/// whichever order the paths were analysed, and a part read from a borrowed
/// computed base is read through that borrow.
#[test]
fn a_value_borrowed_on_any_path_is_published_as_a_borrow() {
    let source = "fn one(s:String)->u64 {1}\nfn probe(s:String,xs:Array[String],n:u64)->u64 {one(if n==0 {s} else {\"other\"})+one(if n==0 {\"other\"} else {s})+one(match n {0=>\"x\",_=>s})+one(match n {0=>s,_=>\"x\"})+one({xs}[0])+one(if n==0 {\"a\"} else {\"b\"})}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("joins.hir", source.into()).unwrap();
    {
        use hirsel_engine::DefinitionQueries;
        engine.parse_file(file, &context).unwrap();
    }
    let mut definition = hirsel_engine::entry_path(&context);
    definition.0.name = "probe".into();
    let out = engine.mode_definition(&definition).unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let typed = &out.value.typed;
    let uses: Vec<_> = typed
        .syntax
        .tree
        .expressions
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            matches!(
                e.kind,
                ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Index { .. }
            )
        })
        .filter_map(|(id, _)| {
            typed
                .checked
                .expressions
                .iter()
                .find(|row| row.expression == ExprId(id as u32))
                .map(|row| row.usage)
        })
        .collect();
    assert_eq!(
        uses,
        [
            UseKind::Borrow,
            UseKind::Borrow,
            UseKind::Borrow,
            UseKind::Borrow,
            UseKind::Borrow,
            UseKind::Take
        ]
    );
}

/// Spec issue #50: `mut` and `take` need a place or an owned temporary. A
/// borrowed computed value, or a choice between places, is neither.
#[test]
fn exclusive_and_moving_operands_cannot_be_borrowed_computed_values() {
    for (operand, rejected) in [
        ("mut { r }.items[0]", true),
        ("mut if c { r } else { q }.items[0]", true),
        ("mut { r.items[0] }", true),
        ("mut r.items[0]", false),
    ] {
        let source = format!(
            "type Rec {{items:Array[Bytes]}}\nfn set(mut b:Bytes) {{}}\nfn probe(mut r:Rec,mut q:Rec,c:bool) {{set({operand})}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("operand.hir", source.clone()).unwrap();
        {
            use hirsel_engine::DefinitionQueries;
            engine.parse_file(file, &context).unwrap();
        }
        let mut definition = hirsel_engine::entry_path(&context);
        definition.0.name = "probe".into();
        let rules: Vec<_> = match engine.mode_definition(&definition) {
            Ok(out) => out.diagnostics,
            Err(hirsel_engine::QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{source}: {error:?}"),
        }
        .iter()
        .map(|d| d.rule.as_str().to_owned())
        .collect();
        assert_eq!(
            rules,
            if rejected { vec!["OWN-3"] } else { vec![] },
            "{source}"
        );
    }
}

/// Physical fields retain their declared modes, while taken receipts retain
/// the current whole's deep uniqueness (MODE-2).
#[test]
fn destructured_parts_keep_physical_modes_and_receive_deep_uniqueness() {
    let source = "type Rec {items:Bytes,n:u64}\nfn probe(take r:unique Rec)->Bytes {let Rec(items:items,..)=take r\n take items}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("parts.hir", source.into()).unwrap();
    {
        use hirsel_engine::DefinitionQueries;
        engine.parse_file(file, &context).unwrap();
    }
    let mut definition = hirsel_engine::entry_path(&context);
    definition.0.name = "probe".into();
    let out = engine.mode_definition(&definition).unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let checked = &out.value.typed.checked;
    let parts: Vec<_> = checked
        .patterns
        .iter()
        .map(|p| engine.types().get(p.ty).unwrap())
        .filter(|t| matches!(t.kind, TypeKind::Primitive(Primitive::Bytes)))
        .collect();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].modes.uniqueness, Uniqueness::Aliased);
    assert_eq!(parts[0].modes.sendability, Sendability::NotSendable);
    let pattern = checked
        .patterns
        .iter()
        .find(|row| {
            matches!(
                engine.types().get(row.ty).unwrap().kind,
                TypeKind::Primitive(Primitive::Bytes)
            )
        })
        .unwrap()
        .pattern;
    let binding = checked
        .bindings
        .iter()
        .find(|row| row.site == NameSite::Pattern(pattern))
        .unwrap();
    assert_eq!(
        engine.types().get(binding.ty).unwrap().modes.uniqueness,
        Uniqueness::Unique
    );
    {
        use hirsel_engine::DefinitionQueries;
        engine.lower_to_tir(&definition).unwrap();
    }
}

/// A computed root (here a closure call, whose result is local to its
/// environment) is consumed by the innermost step of `f()[0].name`, so the
/// outer step publishes its row from its live base, not from the root's
/// pre-flow row. Both steps are local to the environment, as the IR checker
/// projects them from the call result.
#[test]
fn every_step_over_a_computed_root_publishes_its_physical_row() {
    let source = "type R{name:String}\nfn view(x:String)->String in x{copy x}\nfn probe(s:String)->u64{let xs:List[R]=[R(name:view(s))]\n string.byte_len((fn(){xs})()[0].name)}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("computed.hir", source.into()).unwrap();
    {
        use hirsel_engine::DefinitionQueries;
        engine.parse_file(file, &context).unwrap();
    }
    let mut definition = hirsel_engine::entry_path(&context);
    definition.0.name = "probe".into();
    let out = engine.mode_definition(&definition).unwrap();
    assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
    let rows: Vec<_> = out
        .value
        .ownership
        .physical_projections
        .iter()
        .map(|(_, ty)| engine.types().get(*ty).unwrap())
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in rows {
        assert!(row.modes.relation.has_environment(), "{row:?}");
    }
    {
        use hirsel_engine::DefinitionQueries;
        engine.lower_to_tir(&definition).unwrap();
    }
}

#[test]
fn physical_pattern_modes_are_distinct_from_the_bound_values_modes() {
    use hirsel_engine::DefinitionQueries;
    for (source, primitive, receiving_unique) in [
        (
            "fn make()->Result[unique String,()]{Ok(\"x\")}\nfn probe(){match make(){Ok(text) if text==\"x\"=>(),Ok(_)=>(),Error(_)=>()}}",
            Primitive::String,
            true,
        ),
        (
            "fn make()->Result[unique String,()]{Ok(\"x\")}\nfn probe(){match make(){Ok(text)=>(),Error(e)=>()}}",
            Primitive::String,
            true,
        ),
        (
            "type Rec {items:unique Bytes}\nfn probe(take r:Rec){let Rec(items:items)=take r}",
            Primitive::Bytes,
            false,
        ),
        (
            "type Rec {items:unique Bytes}\nfn probe(take r:unique Rec){let Rec(items:items)=take r}",
            Primitive::Bytes,
            true,
        ),
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("physical.hir", source.into()).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut definition = hirsel_engine::entry_path(&context);
        definition.0.name = "probe".into();
        let output = engine.mode_definition(&definition).unwrap();
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        let checked = &output.value.typed.checked;
        let pattern=checked.patterns.iter().find(|row|matches!(engine.types().get(row.ty).unwrap().kind,TypeKind::Primitive(p) if p==primitive)).unwrap();
        assert_eq!(
            engine.types().get(pattern.ty).unwrap().modes.uniqueness,
            Uniqueness::Unique,
            "physical: {source}"
        );
        let binding = checked
            .bindings
            .iter()
            .find(|row| row.site == NameSite::Pattern(pattern.pattern))
            .unwrap();
        assert_eq!(
            engine.types().get(binding.ty).unwrap().modes.uniqueness == Uniqueness::Unique,
            receiving_unique,
            "receiving: {source}"
        );
        engine
            .lower_to_tir(&definition)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
}

#[test]
fn local_temporary_views_require_a_receiving_lexical_scope() {
    use hirsel_engine::DefinitionQueries;
    for (body, accepted) in [
        (
            "fn probe(mut x:Bytes,mut y:Bytes){let mut slot=view(x);while false{slot=view(copy y)};()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes,mut y:Bytes){let mut slot=view(x);while false{slot=view((fn(){y})())};()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes,ys:List[Bytes]){let mut slot=view(x);for y in ys{slot=view(copy y)};()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes){let slot={let v=view(copy x);v};()}",
            false,
        ),
        (
            "type Pair{left:Bytes,right:Bytes}\nfn probe(mut x:Bytes){let pair=Pair(left:{let v=view(copy x);v},right:view(x));()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes){let pair=({let v=view(copy x);v},view(x));()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes,mut y:Bytes){let mut pair=(view(x),view(x));while false{pair=(view(copy y),view(y))};()}",
            false,
        ),
        (
            "type Pair{left:Bytes,right:Bytes}\nfn probe(mut x:Bytes,mut y:Bytes){let mut pair=Pair(left:view(x),right:view(x));while false{pair.left=view(copy y)};()}",
            false,
        ),
        (
            "fn probe(mut x:Bytes,mut y:Bytes){let mut slot=view(x);while false{slot=view(y)};()}",
            true,
        ),
        (
            "fn probe(mut x:Bytes){while false{let slot=view(copy x);()};()}",
            true,
        ),
        (
            "fn probe(mut x:Bytes,mut y:Bytes){let kept=copy y;let mut slot=view(x);while false{slot=view(kept)};()}",
            true,
        ),
        ("fn probe(mut x:Bytes){let slot={let v=view(x);v};()}", true),
        (
            "type Pair{left:Bytes,right:Bytes}\nfn probe(mut x:Bytes){let pair=Pair(left:{let v=view(x);v},right:view(x));()}",
            true,
        ),
        (
            "type Pair{left:Bytes,right:Bytes}\nfn probe(mut x:Bytes){while false{let pair=Pair(left:view(copy x),right:view(x));()};()}",
            true,
        ),
        (
            "fn probe(mut x:Bytes)->Bytes in x{({let pair=(view(copy x),view(x));let(a,b)=take pair;b})}",
            true,
        ),
    ] {
        let source = format!("fn view(x:Bytes)->Bytes in x{{copy x}}\n{body}");
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine
            .sources
            .add("temporary-scope.hir", source.clone())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut definition = hirsel_engine::entry_path(&context);
        definition.0.name = "probe".into();
        let output = hirsel_engine::check_progress(&mut engine, file, &context)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        assert_eq!(
            output.diagnostics.is_empty(),
            accepted,
            "{source}: {:?}",
            output.diagnostics
        );
        if accepted {
            engine
                .lower_to_tir(&definition)
                .unwrap_or_else(|error| panic!("{source}: {error:?}"));
        } else {
            assert!(
                output
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.rule.as_str() == "MODE-1"),
                "{source}: {:?}",
                output.diagnostics
            );
        }
    }
}

#[test]
fn constant_field_receipts_preserve_the_physical_declaration() {
    use hirsel_engine::DefinitionQueries;
    for expression in ["box (C).xs", "box ((C).xs)", "copy (C).xs"] {
        let source = format!(
            "type R{{xs:unique List[Bytes]}}\nconst C:R=R(xs:[b\"x\"])\nfn probe(){{let built={expression}}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine
            .sources
            .add("constant-projection.hir", source.clone())
            .unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut definition = hirsel_engine::entry_path(&context);
        definition.0.name = "probe".into();
        let output = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
        assert!(
            output.diagnostics.is_empty(),
            "{source}: {:?}",
            output.diagnostics
        );
        let modes = engine.mode_definition(&definition).unwrap();
        let checked = &modes.value.typed.checked;
        let (expression, physical) = modes
            .value
            .ownership
            .physical_projections
            .iter()
            .find(|(_, ty)| {
                matches!(
                    engine.types().get(*ty).unwrap().kind,
                    TypeKind::Builtin {
                        kind: BuiltinType::List,
                        ..
                    }
                )
            })
            .unwrap();
        assert_eq!(
            engine.types().get(*physical).unwrap().modes.uniqueness,
            Uniqueness::Unique
        );
        let receipt = checked
            .expressions
            .iter()
            .find(|row| row.expression == *expression)
            .unwrap();
        assert_eq!(
            engine.types().get(receipt.ty).unwrap().modes.uniqueness,
            Uniqueness::Aliased
        );
        engine
            .lower_to_tir(&definition)
            .unwrap_or_else(|error| panic!("{source}: {error:?}"));
    }
}

#[test]
fn illegal_taken_result_origins_reach_the_declaration_diagnostic() {
    for source in [
        "fn probe(take x:Bytes)->(()) of x ensures true {()}",
        "type R{value:Bytes}\nfn probe(take x:R)->(()) of x.value ensures true {()}",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine
            .sources
            .add("invalid-clause-origin.hir", source.into())
            .unwrap();
        let output = hirsel_engine::check_progress(&mut engine, file, &context).unwrap();
        assert_eq!(
            output.diagnostics.len(),
            1,
            "{source}: {:?}",
            output.diagnostics
        );
        assert_eq!(output.diagnostics[0].rule.as_str(), "PROV-1");
        assert!(
            engine
                .sources
                .slice(output.diagnostics[0].primary)
                .unwrap()
                .starts_with("fn probe")
        );
    }
}
