//! Existing-construct feasibility probes, before enabling source rebinds.
use crate::rc_support::*;

#[test]
#[ignore = "native: focused borrow-cursor projection parity"]
fn borrow_cursor_8_projection_native_parity() {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let mut matrix: Vec<_> = full_matrix()
        .into_iter()
        .filter(|configuration| {
            configuration.options.target.triple.starts_with("x86_64")
                && configuration.options.regions
                && configuration.options.elide
                && configuration.options.reuse
                && matches!(
                    configuration.engine,
                    Engine::Heap { .. }
                        | Engine::Native {
                            opt: OptLevel::Speed,
                            partition: Partition::Whole,
                            runtime: Runtime::Diagnostic
                        }
                        | Engine::Cranelift {
                            partition: Partition::Whole,
                            runtime: Runtime::Diagnostic
                        }
                )
        })
        .collect();
    matrix.push(Configuration {
        options: Options::new(target()),
        engine: Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    });
    assert_eq!(matrix.len(), 3);
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("borrow-cursor-8-projections");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    for (name, source) in [
        (
            "loop-protocol",
            r#"
type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)
fn leaf_loop_probe(root: box Tree) -> u64 {
    match root {
        Leaf(n: n) => n,
        Branch(left: child, right: _) => leaf_loop_probe(child),
    }
}
pub fn main() -> Result[(), u64] {
    let n = leaf_loop_probe(box Branch(left: box Branch(left: box Leaf(n: 7), right: box Leaf(n: 8)), right: box Leaf(n: 9)))
    if n == 7 { Ok(()) } else { Error(n) }
}
"#,
        ),
        (
            "box",
            r#"
type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)
fn leaf(a: box Tree, b: box Tree, flag: bool) -> u64 {
    match (if flag { a } else { b }) {
        Leaf(n: n) => n,
        Branch(left: child, right: _) => {
            match child { Leaf(n: n) => n, Branch(left: _, right: _) => 0 }
        },
    }
}
pub fn main() -> Result[(), u64] {
    let n = leaf(box Branch(left: box Leaf(n: 7), right: box Leaf(n: 9)), box Leaf(n: 3), true)
    if n == 7 { Ok(()) } else { Error(n) }
}
"#,
        ),
        (
            "inline-snapshot",
            r#"
type Value = Null | Number(String) | Array(List[Value]) | Object(Map[String, Value])
fn first(a: Value, b: Value, flag: bool) -> u64 {
    let selected = if flag { a } else { b }
    let saved = selected
    let other = if flag { b } else { a }
    let x = match other { Array(items) => list.len(items), Object(m) => map.len(m), Number(n) => string.byte_len(n), Null() => 0 }
    let y = match saved { Array(items) => list.len(items), Object(m) => map.len(m), Number(n) => string.byte_len(n), Null() => 0 }
    x + y
}
pub fn main() -> Result[(), u64] {
    let n = first(Array([Number("7")]), Number("123"), true)
    if n == 4 { Ok(()) } else { Error(n) }
}
"#,
        ),
    ] {
        let directory = directory.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        with_compiler_stack(|| {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine
                .sources
                .add("borrow-cursor-projection.hir", source.into())
                .unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{name}: {:?}",
                checked.diagnostics
            );
            let entry = entry_path(&context);
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot.clone()).unwrap();
            if name == "loop-protocol" {
                // The source attestation must reject edited TIR. This test uses
                // the public direct-program seam after independent V validation.
                let mut probe = snapshot;
                install_loop_body(&mut probe);
                let mut signatures = hirsel_validate::SignatureMap::new(probe.types.clone());
                for (path, signature) in &probe.signatures {
                    signatures.insert(path.clone(), (**signature).clone());
                }
                let verdict = hirsel_validate::validate(
                    probe.definitions.get(&path("leaf_loop_probe")).unwrap(),
                    &probe.types,
                    &signatures,
                    &mut vec![],
                );
                assert!(verdict.violations.is_empty(), "{verdict:?}");
                let compiled = crate::rc_support::program(&probe);
                let sites = Sites::new(validated.snapshot());
                let ready = build_program(
                    &compiled,
                    manifest(validated.snapshot()).unwrap(),
                    &Options::new(target()),
                )
                .unwrap();
                let heap = heap_campaign(&ready, &entry, &sites).unwrap();
                assert_eq!(heap.observation, reference);
                // Synthetic graph evidence ends at independently checked heap
                // execution. Native parity uses ordinary attested source below.
                return;
            }
            for (configuration, outcome) in differential(
                &validated, &entry, &reference, &matrix, &toolchain, &directory,
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
            }
        });
    }
}

// Phase-one test-only TIR: replace an already checked recursive helper with
// existing-construct loop transport, and revalidate the detached native input.
fn install_loop_body(input: &mut hirsel_engine::NativeInput) {
    use hirsel_tir::*;
    use hirsel_types::UseKind;
    let definition =
        std::sync::Arc::make_mut(input.definitions.get_mut(&path("leaf_loop_probe")).unwrap());
    let original = &definition.bodies[definition.entry.0 as usize];
    let root_ty = original.locals[original.parameters[0].0 as usize].ty;
    let tree_ty = match input.types.get(root_ty).unwrap().kind {
        hirsel_types::TypeKind::Box(tree) => tree,
        _ => panic!("box"),
    };
    let mut payload_row = input.types.get(tree_ty).unwrap().clone();
    payload_row.modes.relation = payload_row
        .modes
        .relation
        .union(&input.types.get(root_ty).unwrap().modes.relation);
    let tree_ty = input.types.intern(payload_row);
    let number = original.result;
    let span = original.locals[original.parameters[0].0 as usize].span;
    let local = |ty, storage, scope| Local {
        ty,
        storage,
        scope: ScopeId(scope),
        span,
    };
    let place = |root, projection, loan, ty| Place {
        root: LocalId(root),
        projection,
        loan,
        binding: None,
        ty,
    };
    let op = |place, usage| Operand {
        place: PlaceId(place),
        usage,
    };
    let ins = |kind| Instruction {
        kind,
        span,
        on_failure: CleanupId(1),
    };
    let assign = |destination, value| {
        ins(InstructionKind::Assign {
            destination: PlaceId(destination),
            value: RvalueId(value),
        })
    };
    let block = |instructions, terminator| Block {
        instructions,
        terminator,
        span,
    };
    let mut body = Body {
        parameter_scope: original.parameter_scope,
        locals: vec![
            local(number, Storage::ReturnSlot, 0),
            local(root_ty, Storage::Parameter, 0),
            local(root_ty, Storage::Temporary, 1),
            local(root_ty, Storage::Temporary, 2),
            local(root_ty, Storage::Temporary, 3),
            local(number, Storage::Temporary, 0),
        ],
        places: vec![
            place(0, vec![].into(), None, number),
            place(1, vec![].into(), None, root_ty),
            place(2, vec![].into(), None, root_ty),
            place(3, vec![].into(), None, root_ty),
            place(4, vec![].into(), None, root_ty),
            place(5, vec![].into(), None, number),
            place(1, vec![].into(), Some(LoanId(0)), root_ty),
            place(2, vec![Projection::Field(0)].into(), None, tree_ty),
            place(
                2,
                vec![
                    Projection::Field(0),
                    Projection::VariantField {
                        variant: 1,
                        field: 0,
                    },
                ]
                .into(),
                None,
                root_ty,
            ),
            place(
                2,
                vec![
                    Projection::Field(0),
                    Projection::VariantField {
                        variant: 0,
                        field: 0,
                    },
                ]
                .into(),
                None,
                number,
            ),
        ],
        scopes: vec![
            Scope { parent: None, span },
            Scope {
                parent: Some(ScopeId(0)),
                span,
            },
            Scope {
                parent: Some(ScopeId(0)),
                span,
            },
            Scope {
                parent: Some(ScopeId(0)),
                span,
            },
        ],
        blocks: vec![
            block(
                vec![
                    ins(InstructionKind::BeginLoan {
                        id: LoanId(0),
                        place: PlaceId(1),
                        mutable: false,
                        if_inactive: false,
                    }),
                    ins(InstructionKind::StorageLive(LocalId(2))),
                    assign(2, 0),
                ],
                Terminator::Goto(BlockId(1)),
            ),
            block(
                vec![],
                Terminator::Switch {
                    value: op(7, UseKind::Borrow),
                    arms: vec![(0, BlockId(3)), (1, BlockId(2))],
                    otherwise: BlockId(4),
                },
            ),
            block(
                vec![
                    ins(InstructionKind::StorageLive(LocalId(3))),
                    assign(3, 1),
                    ins(InstructionKind::StorageLive(LocalId(4))),
                    assign(4, 2),
                    assign(2, 3),
                    ins(InstructionKind::EndScope(ScopeId(3))),
                    ins(InstructionKind::EndScope(ScopeId(2))),
                ],
                Terminator::Goto(BlockId(1)),
            ),
            block(
                vec![
                    ins(InstructionKind::StorageLive(LocalId(5))),
                    assign(5, 4),
                    ins(InstructionKind::EndScope(ScopeId(1))),
                    ins(InstructionKind::EndLoan {
                        loan: LoanId(0),
                        if_active: false,
                    }),
                ],
                Terminator::Return {
                    value: op(5, UseKind::Take),
                    cleanup: CleanupId(0),
                },
            ),
            block(vec![], Terminator::Unreachable),
        ],
        rvalues: vec![
            Rvalue::Use(op(6, UseKind::Borrow)),
            Rvalue::Use(op(8, UseKind::Borrow)),
            Rvalue::Use(op(2, UseKind::Take)),
            Rvalue::Use(op(3, UseKind::Take)),
            Rvalue::Use(op(9, UseKind::Copy)),
        ],
        cleanups: vec![Cleanup::default()],
        cleanup_nodes: vec![],
        entry: BlockId(0),
        parameters: vec![LocalId(1)],
        result: number,
        borrow_cursors: vec![BorrowCursor {
            carrier: LocalId(2),
            initializer: RvalueId(0),
            cover: None,
        }],
        tail_sites: vec![],
        matches: vec![],
        arena_scope_grant: None,
        scoped_parameters: vec![],
    };
    body.add_cleanup(vec![CleanupAction::EndLoan {
        loan: LoanId(0),
        if_active: true,
    }]);
    definition.bodies[definition.entry.0 as usize] = body;
    definition.types = hirsel_tir::definition_types(&input.types, definition).into();
}

#[test]
fn borrow_cursor_8_source_tree_traversal() {
    lower_valid(
        r#"
type Tree = Leaf(n: u64) | Branch(left: box Tree, right: box Tree)
fn leaf(root: box Tree) -> u64 {
    let mut node = root
    while true {
        match node {
            Leaf(n: n) => { return n; },
            Branch(left: child, right: _) => { node = child; },
        }
    }
    0
}
fn main() -> u64 { leaf(box Branch(left: box Leaf(n: 7), right: box Leaf(n: 9))) }
"#,
    );
}

#[test]
fn borrow_cursor_8_source_explicit_cover_and_snapshot() {
    lower_valid(
        r#"
fn choose(a: List[u64], b: List[u64], flag: bool) -> u64 {
    let mut c: List[u64] in {a,b} = a
    let saved = c
    c = if flag { b } else { a }
    list.len(saved) + list.len(c)
}
fn main() -> u64 { choose([1],[2,3],true) }
"#,
    );
}

#[test]
fn borrow_cursor_8_source_negative_matrix() {
    for (name, source, rule) in [
        (
            "inferred-cover",
            "fn f(a: List[u64], b: List[u64]) -> u64 { let mut c=a; c=b; list.len(c) }",
            "MODE-1",
        ),
        (
            "written-cover",
            "fn f(a: List[u64], b: List[u64]) -> u64 { let mut c: List[u64] in a=a; c=b; list.len(c) }",
            "MODE-1",
        ),
        (
            "bad-arm",
            "fn f(a: List[u64], b: List[u64],flag:bool) -> u64 { let mut c=a; c=if flag {a}else{b}; list.len(c) }",
            "MODE-1",
        ),
        (
            "copy-refill",
            "fn f(a: List[u64]) -> u64 { let mut c=a; c=copy a; list.len(c) }",
            "LANG-15",
        ),
        (
            "constructor-refill",
            "fn f(a: List[u64]) -> u64 { let mut c=a; c=[1]; list.len(c) }",
            "LANG-15",
        ),
        (
            "call-refill",
            "fn id(a: List[u64]) -> List[u64] in a {a} fn f(a: List[u64]) -> u64 { let mut c=a; c=id(a); list.len(c) }",
            "LANG-15",
        ),
        (
            "immutable",
            "fn f(a: List[u64]) -> u64 { let c=a; c=a; list.len(c) }",
            "LANG-5",
        ),
        (
            "project-write",
            "fn f(a: List[u64]) -> u64 { let mut c=a; c[0]=9; list.len(c) }",
            "LANG-15",
        ),
        (
            "exclusive",
            "fn f(a: List[u64]) -> u64 { let mut c=a; list.push(mut c,9); list.len(c) }",
            "LANG-15",
        ),
        (
            "take",
            "fn use(take a:List[u64]) -> u64 {list.len(a)} fn f(a: List[u64]) -> u64 { let mut c=a; use(take c) }",
            "OWN-1",
        ),
        (
            "owning-arm",
            "fn f(a: List[u64],flag:bool) -> u64 { let mut c=a; c=if flag {a}else{copy a}; list.len(c) }",
            "LANG-15",
        ),
        (
            "heap-cover",
            "fn f(a: List[u64]) -> u64 { let mut c:List[u64] in heap=a; c=a; list.len(c) }",
            "LANG-15",
        ),
        (
            "snapshot-freeze",
            "fn f(mut a:List[u64], b:List[u64]) -> u64 {let mut c:List[u64] in {a,b}=a; let saved=c; c=b; list.push(mut a,9); list.len(saved)+list.len(c)}",
            "OWN-3",
        ),
    ] {
        let source = format!("{source}\nfn main()->u64 {{0}}")
            .replace("} fn ", "}\nfn ")
            .replace("fn use(", "fn consume(")
            .replace(" use(take c)", " consume(take c)");
        let error = match try_compile(&source) {
            Ok(_) => panic!("{name}: unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains(rule), "{name}: expected {rule}: {error}");
    }
}

#[test]
fn borrow_cursor_8_source_json_member_scan() {
    let source = r#"type Value = Null | Bool(bool) | Number(String) | Text(String) |
    Array(List[Value]) | Object(Map[String,Value])
type PointerToken { key: String, index: Option[u64] }
fn walk_json(document: Value, tokens: List[PointerToken]) -> Option[Value] in document {
    let mut target = document
    for token in tokens {
        match target {
            Object(members) => {
                let mut found = false
                for (key,child) in members {
                    if key == token.key {
                        target = child
                        found = true
                        break
                    }
                }
                if !found { return None() }
            },
            Array(items) => {
                let Some(index) = token.index else { return None() }
                if index >= list.len(items) { return None() }
                target = items[index]
            },
            _ => { return None() },
        }
    }
    Some(copy target)
}
fn make_document() -> Value {
    let mut inner: Map[String,Value] = map.empty()
    map.insert(mut inner,"b",Number("7"))
    let mut root: Map[String,Value] = map.empty()
    map.insert(mut root,"a",Array([Object(take inner)]))
    Object(take root)
}
fn result_n(value: Option[Value]) -> u64 {
    match value {
        Some(Number(n)) => if n == "7" { 7 } else { 1 },
        Some(_) => 2,
        None() => 0,
    }
}
pub fn main() -> Result[(),u64] {
    let doc = make_document()
    let n = result_n(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"0",index:Some(0)),PointerToken(key:"b",index:None())]))
    if n == 7 { Ok(()) } else { Error(n) }
}
"#;
    let program = compile(source);
    let lowered =
        hirsel_rc::import::lower(&program, &target()).unwrap_or_else(|error| panic!("{error:?}"));
    let v = violations(&program, &lowered);
    assert!(v.is_empty(), "{v:#?}");
}

#[test]
fn borrow_cursor_8_source_flow_matrix() {
    for (name, source) in [
        (
            "field-cover",
            "type Pair {a:List[u64], b:List[u64]}\nfn f(p:Pair)->u64 {let Pair(a:first,b:_)=p; let mut c:List[u64] in p=first; c=p.b; list.len(c)}",
        ),
        (
            "dead-old-root",
            "fn f(mut a:List[u64], b:List[u64])->u64 {let mut c:List[u64] in {a,b}=a; c=b; list.push(mut a,9); list.len(c)}",
        ),
        (
            "saved-last-use",
            "fn f(mut a:List[u64], b:List[u64])->u64 {let mut c:List[u64] in {a,b}=a; let saved=c; c=b; let n=list.len(saved)+list.len(c); list.push(mut a,9); n}",
        ),
        (
            "self-and-block",
            "fn f(a:List[u64])->u64 {let mut c=a; c=c; c={let x=c;x}; list.len(c)}",
        ),
    ] {
        let source = format!("{source}\nfn main()->u64 {{0}}");
        let program = try_compile(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        let entry = path("f");
        let _ = (program, entry);
        lower_valid(&source);
    }
}

#[test]
#[ignore = "native: attested borrowed cursor source parity"]
fn borrow_cursor_8_native_source_parity() {
    cursor_native_parity(&[(
        "tree",
        r#"
type Tree = Leaf(n:u64) | Branch(left:box Tree,right:box Tree)
fn descend(root:box Tree, right:bool, rounds:u64)->u64 {
    let mut node=root
    let mut sum:u64=0
    for i in 0u64..rounds {
        node=root
        while true {
            match node {
                Leaf(n:n)=> {sum+=n; break},
                Branch(left:a,right:b)=> {node=if right {b} else {a}},
            }
        }
    }
    sum
}
pub fn main()->Result[(),u64] {
    let root=box Branch(left:box Branch(left:box Leaf(n:7),right:box Leaf(n:8)),right:box Leaf(n:9))
    let zero=descend(root,false,0)
    let left=descend(root,false,3)
    let right=descend(root,true,3)
    if zero==0 && left==21 && right==27 {Ok(())} else {Error(left+right)}
}
"#,
    )]);
}

fn cursor_native_parity(cases: &[(&str, &str)]) {
    cursor_native_parity_with_stack(cases, None);
}
fn cursor_native_parity_with_stack(cases: &[(&str, &str)], stack: Option<u64>) {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    let options = Options::new(target());
    let matrix = vec![
        Configuration {
            options: options.clone(),
            engine: Engine::Heap { campaign: false },
        },
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("borrow-cursor-8-source");
    std::fs::create_dir_all(&directory).unwrap();
    let mut toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    if let Some(bytes) = stack {
        toolchain.wrapper = vec![
            "/usr/bin/prlimit".into(),
            format!("--stack={bytes}:{bytes}"),
            "--".into(),
        ];
    }
    for &(name, source) in cases {
        with_compiler_stack(|| {
            let (mut engine, entry) = if source.contains("std@1/") {
                let mut loaded = load_program_source(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
                    Some(source),
                )
                .unwrap();
                loaded.check_for_execution().unwrap();
                (loaded.engine, loaded.entry)
            } else {
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let context = default_context();
                let file = engine
                    .sources
                    .add("borrow-cursor-source.hir", source.into())
                    .unwrap();
                let checked = check_progress(&mut engine, file, &context).unwrap();
                assert!(
                    checked.diagnostics.is_empty(),
                    "{name}: {:?}",
                    checked.diagnostics
                );
                (engine, entry_path(&context))
            };
            let reference = run_observed_snapshot(
                &mut engine,
                &RunConfiguration {
                    entry: entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(reference.observation.as_ref().unwrap());
            if name == "failure-bounds" {
                assert_eq!(reference.failures.len(), 1, "{reference:?}");
                assert_eq!(reference.failures[0].rule, "MEM-14");
                assert_eq!(
                    (reference.failures[0].line, reference.failures[0].column),
                    (3, 4)
                );
            } else {
                assert_eq!(reference.exit_code, Some(0), "{name}: {reference:?}");
            }
            std::fs::create_dir_all(directory.join(name)).unwrap();
            let snapshot = engine.export_native_input().unwrap();
            let validated = engine.import_native_input(snapshot).unwrap();
            for (configuration, outcome) in differential(
                &validated,
                &entry,
                &reference,
                &matrix,
                &toolchain,
                &directory.join(name),
            ) {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}: {configuration}: {outcome:?}"
                );
            }
        });
    }
}

#[test]
fn borrow_cursor_8_source_block_alias_rhs() {
    let source = "fn f(a:List[u64])->u64 {let mut c=a; c={let alias=a;alias}; list.len(c)}\nfn main()->u64 {f([1])}";
    if let Err(error) = try_compile(source) {
        panic!("{error}");
    }
    lower_valid(source);
}

#[test]
fn borrow_cursor_8_source_nested_iteration() {
    lower_valid(
        "fn f(root:List[u64],rows:List[List[u64]])->u64 {let mut c:List[u64] in {root,rows}=root; for row in rows {for x in rows {c=x; continue}; c=row}; list.len(c)}\nfn main()->u64 {0}",
    );
}

const CURSOR_JSON_VECTORS: &str = r#"type Value = Null | Bool(bool) | Number(String) | Text(String) |
    Array(List[Value]) | Object(Map[String,Value])
type PointerToken { key: String, index: Option[u64] }
fn walk_json(document: Value, tokens: List[PointerToken]) -> Option[Value] in document {
    let mut target = document
    for token in tokens {
        match target {
            Object(members) => {
                let mut found = false
                for (key,child) in members {
                    if key == token.key {
                        target = child
                        found = true
                        break
                    }
                }
                if !found { return None() }
            },
            Array(items) => {
                let Some(index) = token.index else { return None() }
                if index >= list.len(items) { return None() }
                target = items[index]
            },
            _ => { return None() },
        }
    }
    Some(copy target)
}
fn make_document() -> Value {
    let mut inner: Map[String,Value] = map.empty()
    map.insert(mut inner,"b",Number("7"))
    let mut root: Map[String,Value] = map.empty()
    map.insert(mut root,"a",Array([Object(take inner)]))
    map.insert(mut root,"",Number("0"))
    map.insert(mut root,"a/b",Number("1"))
    map.insert(mut root,"m~n",Number("8"))
    map.insert(mut root,"~1",Number("9"))
    Object(take root)
}
fn score(value:Option[Value])->u64 {
    match value {
        None()=>0,
        Some(Number(n))=> if n=="7" {7} else {if n=="0" {10} else {if n=="1" {1} else {if n=="8" {8} else {if n=="9" {9} else {99}}}}},
        Some(Object(m))=>map.len(m)+100,
        Some(_)=>99,
    }
}
pub fn main()->Result[(),u64] {
    let doc=make_document()
    let mut failures:u64=0
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"0",index:Some(0u64)),PointerToken(key:"b",index:None())]))!=7 {failures+=1}
    if score(walk_json(doc,[]))!=105 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"",index:None())]))!=10 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a/b",index:None())]))!=1 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"m~n",index:None())]))!=8 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"~1",index:None())]))!=9 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"1",index:Some(1u64))]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"00",index:None())]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"0",index:Some(0u64)),PointerToken(key:"b",index:None()),PointerToken(key:"x",index:None())]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"missing",index:None())]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"-",index:None())]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"+0",index:None())]))!=0 {failures+=1}
    if score(walk_json(doc,[PointerToken(key:"a",index:None()),PointerToken(key:"18446744073709551616",index:None())]))!=0 {failures+=1}
    if failures==0 {Ok(())} else {Error(failures)}
}
"#;

#[test]
#[ignore = "native: attested JSON cursor vectors"]
fn borrow_cursor_8_native_json_vectors() {
    cursor_native_parity(&[("json-vectors", CURSOR_JSON_VECTORS)]);
}

#[test]
#[ignore = "native: deterministic borrow cursor RC and instruction evidence"]
fn borrow_cursor_8_native_cost_evidence() {
    use hirsel_driver::native::*;
    use hirsel_driver::*;
    use hirsel_engine::*;
    use hirsel_lir::ir::Inst;
    with_compiler_stack(|| {
        let dir = std::path::PathBuf::from("/tmp/borrow-cursor-impl-8-cost-artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        let target = target();
        let mut tools = Toolchain::in_tree_with_runtime(
            &dir.join("runtime"),
            &[&target],
            RuntimeBuild {
                measurement: true,
                ..Default::default()
            },
        )
        .unwrap();
        let mut results = vec![];
        for owned in [true, false] {
            let name = if owned { "owned" } else { "borrowed" };
            let source = r#"type Tree = Leaf(n:u64) | Branch(left:box Tree,right:box Tree)
fn scan(root:box Tree,right:bool,rounds:u64)->u64 {
    let mut node=INITIAL
    let mut sum:u64=0
    for i in 0u64..rounds {
        node=RESET
        while true {
            match node {
                Leaf(n:n)=> {sum+=n; break},
                Branch(left:a,right:b)=> {node=if right {b} else {a}},
            }
        }
    }
    sum
}
pub fn main()->Result[(),u64] {
    let root=box Branch(left:box Branch(left:box Leaf(n:7),right:box Leaf(n:8)),right:box Leaf(n:9))
    let left=scan(root,false,64)
    let right=scan(root,true,64)
    if left==448 && right==576 {Ok(())} else {Error(left+right)}
}
"#
            .replace("INITIAL", if owned { "copy root" } else { "root" })
            .replace("RESET", if owned { "copy root" } else { "root" });
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("cursor-cost.hir", source.clone())
                .unwrap();
            let checked = check_progress(&mut engine, file, &default_context()).unwrap();
            assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
            let entry = entry_path(&default_context());
            let snapshot = engine.export_native_input().unwrap();
            let profiles = profile_sources(&snapshot);
            let validated = engine.import_native_input(snapshot).unwrap();
            let mut options = Options::new(target.clone());
            options.elide = false;
            options.reuse = false;
            options.regions = false;
            let ready = build(&validated, &options).unwrap();
            let artifact = dir.join(name);
            std::fs::create_dir_all(&artifact).unwrap();
            std::fs::write(artifact.join("input.hir"), &source).unwrap();
            std::fs::write(artifact.join("identity.txt"), format!("{:?}", ready.input)).unwrap();
            let helper = ready
                .units
                .iter()
                .find(|unit| unit.unit().header.definition.0.name == "scan")
                .unwrap()
                .unit();
            let counts = helper
                .instructions
                .iter()
                .filter(|i| {
                    matches!(
                        i,
                        Inst::Dup { .. } | Inst::Release { .. } | Inst::Copy { .. }
                    )
                })
                .count();
            if !owned {
                assert_eq!(counts, 0, "borrowed scan retains counted instructions");
            }
            std::fs::write(artifact.join("ready.lir"), dump(helper)).unwrap();
            let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
            assert_eq!(heap.observation.exit_code, Some(0));
            let objects = emit_backend_objects(
                &ready,
                Backend::Llvm,
                OptLevel::Speed,
                Partition::Whole,
                true,
                Some(&profiles),
            )
            .unwrap();
            let exe = link_objects(&ready, &entry, &objects, &tools, &artifact).unwrap();
            let profile = artifact.join("profile.json");
            let root = artifact.join("root");
            std::fs::create_dir_all(&root).unwrap();
            let result = std::process::Command::new(&exe)
                .env_clear()
                .env("HIRSEL_RT_ROOT", &root)
                .env("HIRSEL_RT_PROFILE", &profile)
                .env("HIRSEL_RT_MEASUREMENT", artifact.join("measurement"))
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
            let report: serde_json::Value =
                serde_json::from_slice(&std::fs::read(profile).unwrap()).unwrap();
            let mut cursor = std::collections::BTreeMap::<String, u64>::new();
            for site in report["sites"].as_array().unwrap().iter().filter(|s| {
                s["line"]
                    .as_u64()
                    .is_some_and(|line| (2..=15).contains(&line))
            }) {
                for (kind, n) in site["counts"].as_object().unwrap() {
                    *cursor.entry(kind.clone()).or_default() += n.as_u64().unwrap();
                }
            }
            if !owned {
                assert_eq!(cursor.get("count_increments").copied().unwrap_or(0), 0);
                assert_eq!(cursor.get("count_decrements").copied().unwrap_or(0), 0);
            }
            tools.wrapper = vec![
                "/usr/bin/valgrind".into(),
                "--tool=callgrind".into(),
                format!(
                    "--callgrind-out-file={}",
                    artifact.join("callgrind").display()
                ),
            ];
            let run = launch(&exe, &target, &tools, &artifact, false).unwrap();
            assert_eq!(run.exit, Some(0), "{run:?}");
            tools.wrapper.clear();
            let callgrind = std::fs::read_to_string(artifact.join("callgrind")).unwrap();
            let instructions = callgrind
                .lines()
                .find_map(|line| line.strip_prefix("summary: "))
                .unwrap();
            results.push(serde_json::json!({"variant":name,"identity":format!("{:?}",ready.input),"cursor_counts":cursor,"total_counts":report["totals"],"heap":format!("{:?}",heap.counters),"instructions":instructions,"counted_lir_instructions":counts}));
        }
        std::fs::write(
            dir.join("results.json"),
            serde_json::to_vec_pretty(&results).unwrap(),
        )
        .unwrap();
        eprintln!("CURSOR_COST {}", serde_json::to_string(&results).unwrap());
    });
}

const CURSOR_HUFF: &str = r#"// LangArena 3c9f84d compress.rs:300-473, retained fidelity8 port.
fn zeros(n: u64) -> unique List[u64] {
  let mut out: List[u64] = []
  list.reserve(mut out, n)
  for _ in 0u64..n {
    list.push(mut out, 0)
  }
  take out
}

fn zero_bytes(n: u64) -> unique List[u8] {
  let mut out: List[u8] = []
  list.reserve(mut out, n)
  for _ in 0u64..n {
    list.push(mut out, 0)
  }
  take out
}

fn test_data(size: u64) -> unique List[u8] {
  let pattern = string.as_bytes("ABRACADABRA")
  let mut out: List[u8] = []
  list.reserve(mut out, size)
  for i in 0u64..size {
    list.push(mut out, pattern[i % 11])
  }
  take out
}

fn frequencies(data: List[u8]) -> unique List[u64] {
  let mut out = zeros(256)
  for byte in data {
    let i: u64 = int.widen(byte)
    out[i] += 1
  }
  take out
}

type Node = Leaf(frequency: u64, byte: u8) | Branch(
  frequency: u64,
  left: box Node,
  right: box Node
)

@noalloc
fn frequency(node: box Node) -> u64 {
  match node {
    Leaf(frequency: f, byte: _) => f,
    Branch(frequency: f, left: _, right: _) => f,
  }
}

// compress.rs:300-345. Stable initial ordering, then binary insertion.
fn huff_tree(freq: List[u64]) -> box Node {
  let mut nodes: List[box Node] = []
  for i in 0u64..256u64 {
    if freq[i] > 0 {
      let node: box Node = box Leaf(frequency: freq[i], byte: int.narrow(i))
      let mut pos = list.len(nodes)
      list.push(mut nodes, take node)
      while pos > 0 && frequency(nodes[pos - 1]) > freq[i] {
        list.swap(mut nodes, pos, pos - 1)
        pos -= 1
      }
    }
  }
  if list.len(nodes) == 0 {
    return box Leaf(frequency: 0, byte: 0)
  }
  if list.len(nodes) == 1 {
    return box Branch(
      frequency: frequency(nodes[0]),
      left: nodes[0],
      right: box Leaf(frequency: 0, byte: 0)
    )
  }
  while list.len(nodes) > 1 {
    let left = list.remove(mut nodes, 0)
    let right = list.remove(mut nodes, 0)
    let f = frequency(left) + frequency(right)
    let parent: box Node = box Branch(frequency: f, left: left, right: right)
    let mut lo: u64 = 0
    let mut hi = list.len(nodes)
    while lo < hi {
      let mid = (lo + hi) / 2
      if frequency(nodes[mid]) <= f {
        lo = mid + 1
      } else {
        hi = mid
      }
    }
    // Rust binary_search returns the last equal frequency, rather than after it.
    if lo > 0 && frequency(nodes[lo - 1]) == f {
      lo -= 1
    }
    list.push(mut nodes, take parent)
    let mut pos = list.len(nodes) - 1
    while pos > lo {
      list.swap(mut nodes, pos, pos - 1)
      pos -= 1
    }
  }
  nodes[0]
}

fn huff_codes(
  node: box Node,
  code: u64,
  length: u64,
  mut lengths: unique List[u64],
  mut codes: unique List[u64]
) {
  match node {
    Leaf(frequency: _, byte: b) => {
      if length > 0 || b != 0 {
        let index: u64 = int.widen(b)
        lengths[index] = length
        codes[index] = code
      }
    },
    Branch(frequency: _, left: l, right: r) => {
      huff_codes(l, code << 1, length + 1, mut lengths, mut codes)
      huff_codes(r, code << 1 | 1, length + 1, mut lengths, mut codes)
    },
  }
}

type Huff {
  data: List[u8],
  bits: u64,
  freq: List[u64],
}

// compress.rs:347-423.
fn huff_encode(data: List[u8]) -> Huff {
  let freq = frequencies(data)
  let tree = huff_tree(freq)
  let mut lengths = zeros(256)
  let mut codes = zeros(256)
  huff_codes(tree, 0, 0, mut lengths, mut codes)
  let mut out: List[u8] = []
  list.reserve(mut out, list.len(data) * 2)
  let mut byte: u8 = 0
  let mut position: u64 = 0
  let mut bits: u64 = 0
  for b in data {
    let index: u64 = int.widen(b)
    let code = codes[index]
    let mut i = lengths[index]
    while i > 0 {
      i -= 1
      if code & 1u64 << i != 0 {
        byte = byte | 1u8 << int.narrow(7u64 - position)
      }
      position += 1
      bits += 1
      if position == 8 {
        list.push(mut out, byte)
        byte = 0
        position = 0
      }
    }
  }
  if position > 0 {
    list.push(mut out, byte)
  }
  Huff(data: take out, bits: bits, freq: take freq)
}

// compress.rs:432-473. Keep the bit-count-sized scratch allocation.
fn huff_decode(encoded: Huff) -> unique List[u8] {
  let root = huff_tree(encoded.freq)
  huff_walk(root, encoded)
}

fn huff_walk(root: box Node, encoded: Huff) -> unique List[u8] {
  let mut node = root
  let mut out = zero_bytes(encoded.bits)
  let mut size: u64 = 0
  for bit in 0u64..encoded.bits {
    match node {
      Branch(frequency: _, left: l, right: r) => {
        node = if encoded.data[bit / 8] >> int.narrow(7u64 - bit % 8) & 1 == 1 {
          r
        } else {
          l
        }
      },
      Leaf(frequency: _, byte: _) => {},
    }
    match node {
      Leaf(frequency: _, byte: b) => {
        if b != 0 {
          out[size] = b
          size += 1
        }
        node = root
      },
      Branch(frequency: _, left: _, right: _) => {},
    }
  }
  // Hirsel has no list.truncate; pop the unused initialized tail.
  while list.len(out) > size {
    let _ = list.pop(mut out)
  }
  take out
}

pub fn main()->Result[(),u64] {
  let abracadabra = test_data(11)
  let huff = huff_encode(abracadabra)
  if huff.data != [105u8,207u8,104u8] || huff.bits != 23 {return Error(1)}
  if huff_decode(huff) != abracadabra {return Error(2)}
  let banana:List[u8]=[66,65,78,65,78,65]
  let banana_huff=huff_encode(banana)
  if banana_huff.data != [45u8,128u8] || banana_huff.bits != 9 {return Error(3)}
  if huff_decode(banana_huff) != banana {return Error(4)}
  let sizes:List[u64]=[0u64,1u64,2u64,11u64,53u64,257u64]
  for size in sizes {
    let data=test_data(size)
    if huff_decode(huff_encode(data)) != data {return Error(5)}
  }
  Ok(())
}
"#;
#[test]
#[ignore = "native: borrowed Huffman traversal exact vectors"]
fn borrow_cursor_8_native_huffman_vectors() {
    cursor_native_parity(&[("huffman", CURSOR_HUFF)]);
}

#[test]
fn borrow_cursor_8_source_additional_safety_matrix() {
    for (name, source, rule) in [
        (
            "parent-excluded",
            "type P {a:List[u64],b:List[u64]}\nfn f(p:P)->u64 {let P(a:first,b:_)=p; let mut c:List[u64] in p.a=first; c=p.b; list.len(c)}",
            "MODE-1",
        ),
        (
            "compound",
            "fn f(a:List[u64])->u64 {let mut c=a; c[0]+=1; list.len(c)}",
            "LANG-15",
        ),
        (
            "capture-storage",
            "fn f(a:List[u64])->u64 {let mut c=a; let watch=fn()->u64 {list.len(c)}; c=a; watch()}",
            "MODE-1",
        ),
        (
            "not-data",
            "fn f(a:fn()->u64,b:fn()->u64)->u64 {let mut c=a; c=b; c()}",
            "LANG-15",
        ),
        (
            "not-copyable",
            "resource type R {n:u64}\nfn f(a:R,b:R)->u64 {let mut c=a; c=b; c.n}",
            "LANG-15",
        ),
        (
            "unrelated-backedge",
            "fn f(a:List[u64],b:List[u64],flag:bool)->u64 {let mut c=a; while flag {c=b; continue}; list.len(c)}",
            "MODE-1",
        ),
        (
            "new-incarnation",
            "fn f(a:List[u64],rows:List[u64])->u64 {let mut c=a; for x in rows {let fresh:List[u64]=[x]; c=fresh}; list.len(c)}",
            "MODE-1",
        ),
    ] {
        let source = format!("{source}\nfn main()->u64 {{0}}");
        let error = match try_compile(&source) {
            Ok(_) => panic!("{name}: unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains(rule), "{name}: {error}");
    }
}

#[test]
fn borrow_cursor_8_source_owned_copy_of_box_cursor() {
    lower_valid(
        r#"type Tree=Leaf(n:u64)|Branch(left:box Tree,right:box Tree)
fn f(root:box Tree)->(box Tree) in root {let mut c=root; c=root; copy c}
fn main()->u64 {let tree=f(box Leaf(n:7)); match tree {Leaf(n:n)=>n,Branch(left:_,right:_)=>0}}
"#,
    );
}

#[test]
#[ignore = "native: deep cursors under a fixed 256 KiB native stack"]
fn borrow_cursor_8_native_deep_walks() {
    let tree = CURSOR_DEEP_TREE;
    let json = format!(
        "{}\n{}",
        CURSOR_JSON_VECTORS
            .split("fn make_document")
            .next()
            .unwrap(),
        r#"
pub fn main()->Result[(),u64] {
    let mut document:Value in document=Number("7")
    let mut tokens:List[PointerToken]=[]
    for i in 0u64..2048u64 {
        if i%2==0 {
            document=Array([take document])
        } else {
            let mut members:Map[String,Value]=map.empty()
            map.insert(mut members,"a",take document)
            document=Object(take members)
        }
    }
    for i in 0u64..2048u64 {
        list.push(mut tokens,if i%2==0 {PointerToken(key:"a",index:None())} else {PointerToken(key:"0",index:Some(0u64))})
    }
    let answer=walk_json(document,tokens)
    match answer {Some(Number(n))=>if n=="7" {Ok(())} else {Error(1)},_=>Error(2)}
}
"#
    );
    cursor_native_parity_with_stack(&[("deep-tree", tree), ("deep-json", &json)], Some(262144));
}

const CURSOR_DEEP_TREE: &str = r#"type Tree = Leaf(n:u64) | Branch(left:box Tree,right:box Tree)
fn scan(root:box Tree)->u64 {
    let mut node=root
    while true {
        match node {
            Leaf(n:n)=>{return n},
            Branch(left:child,right:_)=>{node=child},
        }
    }
    0
}
pub fn main()->Result[(),u64] {
    let mut root:(box Tree) in root=box Leaf(n:7)
    for i in 0u64..4096u64 {
        root=box Branch(left:take root,right:box Leaf(n:9))
    }
    let n=scan(root)
    if n==7 {Ok(())} else {Error(n)}
}
"#;

#[test]
#[ignore = "native: full JSON Pointer token decoding and canonical index vectors"]
fn borrow_cursor_8_native_json_pointer_decoding() {
    let source = format!(
        "{}\n{}\n{}",
        CURSOR_JSON_VECTORS.split("pub fn main").next().unwrap(),
        CURSOR_JSON_PARSER,
        CURSOR_JSON_POINTER_MAIN
    );
    cursor_native_parity(&[("json-pointer-decoding", &source)]);
}
#[test]
#[ignore = "native: saved inline snapshot and explicit owned box share"]
fn borrow_cursor_8_native_saved_inline_and_copy() {
    cursor_native_parity(&[
        (
            "saved-inline",
            r#"type Value=Text(String)|Array(List[Value])
fn f(root:Value)->u64 {
 let mut cursor=root
 let saved=cursor
 while true {
  match cursor {Text(n)=>{break},Array(items)=>{cursor=items[0]}}
 }
 let length=match saved {Array(items)=>list.len(items),Text(_)=>0}
 let n=match cursor {Text(n)=>string.byte_len(n),Array(_)=>0}
 length+n
}
pub fn main()->Result[(),u64] {if f(Array([Array([Text("seven")])]))==6 {Ok(())}else{Error(1)}}
"#,
        ),
        (
            "explicit-copy",
            r#"type Tree=Leaf(n:u64)|Branch(left:box Tree,right:box Tree)
fn f(root:box Tree)->(box Tree) in root {let mut c=root; c=root; copy c}
pub fn main()->Result[(),u64] {let tree=f(box Leaf(n:7)); match tree {Leaf(n:n)=>if n==7 {Ok(())}else{Error(1)},Branch(left:_,right:_)=>Error(2)}}
"#,
        ),
    ]);
}
#[test]
#[ignore = "native: bounds failure occurs before cursor transfer"]
fn borrow_cursor_8_native_bounds_failure_before_transfer() {
    super::rc_borrow_cursor_evidence_13::bounds_failure_evidence();
    cursor_native_parity(&[(
        "failure-bounds",
        super::rc_borrow_cursor_evidence_13::BOUNDS,
    )]);
}

const CURSOR_JSON_PARSER: &str = r#"// serde_json src/value/mod.rs:259,779. Tokens are decoded before the cursor loop.
fn parse_index(key:String)->Option[u64] {
    let octets=string.as_bytes(key)
    if bytes.len(octets)==0 {return None()}
    if octets[0]==48 && bytes.len(octets)!=1 {return None()}
    let mut index:u64=0
    for byte in octets {
        if byte<48 || byte>57 {return None()}
        let digit:u64=int.widen(byte-48)
        if index>(18446744073709551615u64-digit)/10 {return None()}
        index=index*10+digit
    }
    Some(index)
}
fn pointer(document:Value,take path:String)->Option[Value] in document {
    if string.byte_len(path)==0 {return walk_json(document,[])}
    if !string.starts_with(path,"/") {return None()}
    let parts=std@1/string.split(copy path,"/")
    let mut tokens:List[PointerToken]=[]
    let mut first=true
    for part in parts {
        if first {first=false} else {
            let key=std@1/string.replace(std@1/string.replace(part,"~1","/"),"~0","~")
            let index=parse_index(key)
            list.push(mut tokens,PointerToken(key:take key,index:take index))
        }
    }
    walk_json(document,tokens)
}
"#;
const CURSOR_JSON_POINTER_MAIN: &str = r#"pub fn main()->Result[(),u64] {
    let doc=make_document()
    let checks:List[(String,u64)]=[("/a/0/b",7),("",105),("/",10),("/a~1b",1),("/m~0n",8),("/~01",9),("/missing",0),("/a/1",0),("/a/00",0),("/a/+0",0),("/a/-",0),("/a/18446744073709551616",0),("/a/0/b/x",0),("invalid",0)]
    let mut failed:u64=0
    for (path,expected) in take checks {
        if score(pointer(doc,take path))!=expected {failed+=1}
    }
    if failed==0 {Ok(())} else {Error(failed)}
}
"#;

#[test]
fn borrow_cursor_8_source_mode_and_brand_guards() {
    for (name, source, rule) in [
        (
            "crossing",
            "fn send(take x:sendable List[u64])->u64 {list.len(x)}\nfn f(a:List[u64])->u64 {let mut c=a; c=a; send(copy c)}",
            "MODE-1",
        ),
        (
            "arena",
            "type Tree=Leaf|Branch(left:box Tree,right:box Tree)\nfn build(a:Arena)->(box Tree) in a {box Leaf()}\nfn walk(root:box Tree)->(box Tree) in root {let mut c=root; c=root; copy c}\nfn f()->box Tree {arena.with(fn(a:Arena)->box Tree {let root=build(a); walk(root)})}",
            "PROV-4",
        ),
        (
            "brand",
            "type P {values:List[u64]}\nfn stamp(p:P)->Option[u8 of p] {None()}\nfn inspect(p:P,token:Option[u8 of p])->u64 {list.len(p.values)}\nfn f(mut p:P,q:P)->u64 {let mut c:P in {p,q}=p; let token=stamp(p); c=q; p=P(values:[9]); inspect(p,token)+list.len(c.values)}",
            "PROV-1",
        ),
    ] {
        let source = format!("{source}\nfn main()->u64 {{0}}");
        let error = match try_compile(&source) {
            Ok(_) => panic!("{name}: unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains(rule), "{name}: {error}");
    }
}

#[test]
#[ignore = "native: nested cursor continuation and uniqueness"]
fn borrow_cursor_8_native_outer_continue() {
    cursor_native_parity(&[(
        "outer-continue",
        r#"
fn inspect(a:unique List[u64])->u64 {let mut c=a; c=a; list.len(c)}
fn keep(take a:unique List[u64])->u64 {list.len(a)}
fn f(take a:unique List[u64])->u64 {let n=inspect(a); keep(take a)+n}
fn g(a:List[u64],rows:List[List[u64]])->u64 {
 let mut c:List[u64] in {a,rows}=a
 'outer: for row in rows {
  let mut inner:List[u64] in rows=row
  for item in rows {inner=item; c=inner; continue 'outer}
  c=row
 }
 list.len(c)
}
pub fn main()->Result[(),u64] {
 let n=f([1])+g([2],[[3],[4]])+g([2],[])
 if n==4 {Ok(())} else {Error(n)}
}
"#,
    )]);
}
