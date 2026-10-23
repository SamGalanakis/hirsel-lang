//! Source sites of hirsel-check's audit diagnostics, checked through the driver's mode query.
use hirsel_driver::*;
use hirsel_engine::*;
fn modes(source: &str, name: &str) -> (QueryEngine, Vec<hirsel_diag::Diagnostic>) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("audit.hir", source.into()).unwrap();
    engine.parse_file(file, &default_context()).unwrap();
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    let diagnostics = match engine.mode_definition(&path) {
        Ok(o) => o.diagnostics,
        Err(QueryError::Diagnostics(ds)) => ds,
        Err(e) => panic!("{e:?}"),
    };
    (engine, diagnostics)
}
#[test]
fn growable_fields_require_flat_list_and_final_position() {
    for source in [
        "type Value { items: List[u8] @ growable }",
        "flat type Value { items: u8 @ growable }",
        "flat type Value { items: List[u8] @ growable, tail: u64 }",
        "flat type Value { a: List[u8] @ growable, b: List[u8] @ growable }",
    ] {
        let (engine, ds) = modes(source, "Value");
        assert_eq!(ds[0].rule.as_str(), "MEM-12", "{source}");
        assert!(
            engine
                .sources
                .slice(ds[0].primary)
                .unwrap()
                .contains("growable")
        );
    }
    for source in [
        "flat type Value { tail: u64, items: List[u8] @ growable }",
        "flat type Value { items: List[u8] @ growable }",
    ] {
        assert!(modes(source, "Value").1.is_empty());
    }
}

#[test]
fn recursive_layout_requires_a_block_boundary_at_the_cycle_field() {
    for source in [
        "type Tree = Leaf | Node(child: Tree)",
        "type Tree { child: Tree }",
        "type Tree { child: Inline[Tree, 1] }",
        "type Wrapper[T] { value: T }\ntype Tree { child: Wrapper[Tree] }",
    ] {
        let (engine, ds) = modes(source, "Tree");
        assert_eq!(ds[0].rule.as_str(), "MEM-7", "{source}");
        assert!(
            engine
                .sources
                .slice(ds[0].primary)
                .unwrap()
                .contains("Tree")
        );
    }
    for source in [
        "type Tree = Leaf | Node(child: box Tree)",
        "type Tree { children: List[Tree] }",
        "type Tree { child: Inline[Tree, 0] }",
        "type Pair { a: u64, b: u64 }",
    ] {
        let name = if source.contains("Pair") {
            "Pair"
        } else {
            "Tree"
        };
        assert!(modes(source, name).1.is_empty(), "{source}");
    }
}

#[test]
fn contracts_retain_cleanup_obligations_before_unconditional_transfers() {
    for contract in ["requires flag", "ensures flag"] {
        let (_, ds) = modes(
            &format!("@noalloc fn caller[T](take x: T, flag: bool) -> T {contract} {{ take x }}"),
            "caller",
        );
        assert_eq!(ds[0].rule.as_str(), "MEM-5");
        assert!(modes(&format!("@noalloc fn caller[T: noalloc_drop](take x: T, flag: bool) -> T {contract} {{ take x }}"), "caller").1.is_empty());
    }
    assert!(
        modes(
            "@noalloc fn caller[T: noalloc_drop](take x: T) { let xs: Array[T] = [take x]; }",
            "caller"
        )
        .1
        .is_empty()
    );
}

#[test]
fn cleanup_diagnostics_name_the_declared_destructor() {
    let (_, ds) = modes(
        "resource type Token { n: u32 } drop(x) {}\n@noalloc fn caller(take x: Token) {}",
        "caller",
    );
    assert!(
        ds[0]
            .notes
            .iter()
            .any(|n| n.contains("destructor `Token` permits allocation"))
    );
    assert!(
        modes(
            "@noalloc fn caller[T](take x: T) -> T requires true { take x }",
            "caller"
        )
        .1
        .is_empty()
    );
}

#[test]
fn variant_places_and_packed_alias_replacement_use_mem13() {
    let choice = "type Choice = Small(value:u64) | Large(value:u64,extra:u64)\n";
    for (body, site) in [
        (
            "fn edit(mut x:Choice) { x.Small.value = 2 }",
            "x.Small.value = 2",
        ),
        ("fn read(x:Choice)->u64 { x.Small.value }", "x.Small.value"),
        (
            "type Outer { x:Choice }\nfn edit(mut x:Outer) { x.x.Small.value = 2 }",
            "x.x.Small.value = 2",
        ),
        (
            "fn edit(mut xs:List[Packed[Choice]]) { for x in mut xs { x = Large(value:1,extra:2) } }",
            "x = Large(value:1,extra:2)",
        ),
    ] {
        let (engine, ds) = modes(
            &format!("{choice}{body}"),
            if body.starts_with("fn read") {
                "read"
            } else {
                "edit"
            },
        );
        assert_eq!(ds[0].rule.as_str(), "MEM-13", "{ds:?}");
        assert_eq!(engine.sources.slice(ds[0].primary), Some(site));
    }
    for body in [
        "fn edit(mut x:Packed[Choice]) { x.Small.value = 2 }",
        "fn edit(mut xs:List[Choice]) { for x in mut xs { x = Large(value:1,extra:2) } }",
        "fn edit(mut xs:List[Packed[Choice]],take ys:List[Packed[Choice]]) { xs = take ys }",
    ] {
        assert!(
            modes(&format!("{choice}{body}"), "edit").1.is_empty(),
            "{body}"
        );
    }
}

#[test]
fn repeated_inferred_closure_calls_keep_verdicts_and_each_call_site() {
    for source in [
        "@noalloc fn example(n:u64)->u64 { let f = fn(){n}; f(); f() }",
        "@noalloc fn example()->u64 { let outer = fn(n:u64){ let f = fn(){n}; f(); f() }; outer(1); outer(2) }",
    ] {
        let (_, diagnostics) = modes(source, "example");
        assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
    }
    let source = "fn allocating()->u64 { 1 }\n@noalloc fn example()->u64 { let f = fn(){allocating()}; f(); f() }";
    let (engine, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    for diagnostic in &diagnostics {
        assert_eq!(diagnostic.rule.as_str(), "MEM-5");
        assert_eq!(engine.sources.slice(diagnostic.primary), Some("f()"));
        assert_eq!(
            diagnostic.message,
            "every call under noalloc requires a noalloc signature"
        );
        assert!(diagnostic.notes.is_empty());
    }
    let start = |span| engine.sources.resolve(span).start;
    assert!(start(diagnostics[0].primary) < start(diagnostics[1].primary));
}

#[test]
fn annotated_and_aliased_local_closures_keep_declared_call_effects() {
    let source = "@noalloc fn example(n:u64)->u64 { let f: (fn()->u64) in f = fn(){n}; f() }";
    // The written type states neither `@noalloc` nor a cleanup summary, so
    // the binding permits allocating cleanup and its call may allocate.
    let (engine, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    for (diagnostic, site) in diagnostics.iter().zip(["f", "f()"]) {
        assert_eq!(diagnostic.rule.as_str(), "MEM-5");
        assert_eq!(engine.sources.slice(diagnostic.primary), Some(site));
    }
    assert_eq!(
        diagnostics[0].notes,
        [
            "`f` is owned here and may need destruction before it can be transferred",
            "declared destruction may allocate; this value has no `noalloc_drop` promise",
            "help: direct transfer avoids this cleanup obligation only when no possible failure intervenes; preserve the intended operations and their order",
        ]
    );

    // An alias holds the literal's row, so it keeps the `@noalloc` evidence
    // and the cleanup summary that row carries (MEM-5).
    let source = "@noalloc fn example(n:u64)->u64 { let f = fn(){n}; let g = f; g() }";
    let (_, diagnostics) = modes(source, "example");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let source = "fn allocating()->u64 { 1 }\n@noalloc fn example()->u64 { let f = fn(){allocating()}; let g = f; g() }";
    let (engine, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].rule.as_str(), "MEM-5");
    assert_eq!(engine.sources.slice(diagnostics[0].primary), Some("g()"));
}

#[test]
fn a_refused_mut_hand_back_names_both_types_and_the_fix() {
    // CLO-2: the binding's inferred type, the parameter's, and the annotation
    // that makes them agree.
    let source = "fn demand(mut action: unique fn() -> ()) -> () {}\nfn example(text: u64) -> () { let mut action = fn() { let _ = text }; demand(mut action) }";
    let (_, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].rule.as_str(), "CLO-2");
    for text in [
        "the inferred type of `action` is `@noalloc fn() -> () uses {}`",
        "the `mut` parameter is `unique fn() -> ()`",
        "`demand` may store in it any function of the parameter's type",
        "`let mut action: unique fn() -> () = ...`",
        "or narrow the parameter",
    ] {
        assert!(diagnostics[0].message.contains(text), "{diagnostics:?}");
    }
    let written = source.replace("let mut action =", "let mut action: unique fn() -> () =");
    let (_, diagnostics) = modes(&written, "example");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    // A written binding is quoted as declared.
    let source = "fn set(mut p: fn() -> u64) -> () {}\nfn example() -> () { let mut p: fn() -> u64 uses {} = fn() { 1 }; set(mut p) }";
    let (_, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert!(
        diagnostics[0].message.contains(
            "`p` is declared `fn() -> u64 uses {}` and the `mut` parameter is `fn() -> u64`"
        ),
        "{diagnostics:?}"
    );
}

#[test]
fn closure_construction_and_invocation_keep_separate_destinations() {
    let positive = "@noalloc fn example(a:Arena)->String in a { let f = fn()->String in a { \"arena\" }; f() }";
    let (_, diagnostics) = modes(positive, "example");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let source =
        "@noalloc fn example(a:Arena)->String in a { let f = fn()->String { \"heap\" }; f() }";
    // `in heap` is the empty relation (v22 R3): the global closure result
    // meets the arena result, and only the allocating invocation remains.
    let (engine, diagnostics) = modes(source, "example");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].rule.as_str(), "MEM-5");
    for diagnostic in diagnostics {
        assert_eq!(engine.sources.slice(diagnostic.primary), Some("f()"));
    }
}

#[test]
fn noalloc_audits_implicit_loop_calls_and_cursor_cleanup() {
    for body in [
        "fn caller(take xs: Inline[u64,1]) { for x in take xs {} }",
        "fn caller(mut xs: Iter[u64]) { for x in mut xs {} }",
        "fn caller(take xs: Iter[u64]) { for x in take xs {} }",
    ] {
        let (engine, ds) = modes(&format!("@noalloc {body}"), "caller");
        assert!(
            ds.iter().any(|d| d.rule.as_str() == "MEM-5"),
            "{body}: {ds:?}"
        );
        assert!(
            ds.iter()
                .filter(|d| d.rule.as_str() == "MEM-5")
                .all(|d| engine
                    .sources
                    .slice(d.primary)
                    .is_some_and(|s| s == "take xs" || s == "mut xs" || s == "Iter[u64]")),
            "{ds:?}"
        );
        let twin = modes(body, "caller").1;
        assert!(twin.is_empty(), "{body}: {twin:?}");
    }
    for body in [
        "fn caller(take xs: List[u64]) { for x in take xs {} }",
        "fn caller(take xs: Array[u64]) { for x in take xs {} }",
        "fn caller(xs: Map[u64,u64]) { for (k,v) in xs {} }",
        "fn caller(take xs: Map[u64,u64]) { for (k,v) in take xs {} }",
        "fn caller(xs: Set[u64]) { for x in xs {} }",
        "fn caller(xs: List[u64]) { for x in xs {} }",
    ] {
        let ds = modes(&format!("@noalloc {body}"), "caller").1;
        assert!(ds.is_empty(), "{body}: {ds:?}");
    }
}
