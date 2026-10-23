//! Authority checking of real source, from hirsel-authority's unit tests.
use hirsel_types::*;

#[test]
fn taking_a_directory_into_a_borrowed_call_ends_its_open_files_brand() {
    use hirsel_engine::DefinitionQueries;
    for (passing, rejected) in [("take ", true), ("", false)] {
        let source = format!(
            "pub fn main(t: Test) -> Result[(), FileError] {{\nlet p = path.of(\"sub/value\")\nlet dir = test.dir(t, [(p, b\"data\")])\nlet fresh = file.open(dir, p)?\nlet sub = dir.narrow({passing}dir, path.of(\"sub\"))?\nfile.narrow(sub, take fresh)\nOk(())\n}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("directory.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
            Ok(output) => output.diagnostics,
            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{error:?}"),
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            rejected.then_some("PROV-1"),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn moving_a_directory_ends_file_brands_across_whole_transfers() {
    use hirsel_engine::DefinitionQueries;
    for (movement, rule) in [
        ("let moved = take dir", Some("PROV-1")),
        ("let moved = move_dir(take dir)", Some("PROV-1")),
        ("let moved = copy dir", None),
    ] {
        for (storage, call) in [
            ("", "file.narrow(sub, take fresh)"),
            ("", "file.narrow(sub, { take fresh })"),
            ("", "borrow_file(take fresh)"),
            ("let carried = take fresh", "file.narrow(sub, take carried)"),
            ("let carried = take fresh", ""),
        ] {
            let source = format!(
                "fn move_dir(take d: Dir) -> Dir {{ take d }}\nfn borrow_file(f: File) {{}}\npub fn main(t: Test) -> Result[(), FileError] {{\nlet p = path.of(\"sub/value\")\nlet dir = test.dir(t, [(p, b\"data\")])\nlet fresh = file.open(dir, p)?\n{movement}\nlet sub = dir.narrow(moved, path.of(\"sub\"))?\n{storage}\n{call}\nOk(())\n}}"
            );
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let context = hirsel_engine::default_context();
            let file = engine.sources.add("moved-directory.hir", source).unwrap();
            engine.parse_file(file, &context).unwrap();
            let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
                Ok(output) => output.diagnostics,
                Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
                Err(error) => panic!("{error:?}"),
            };
            assert_eq!(
                diagnostics.first().map(|d| d.rule.as_str()),
                if call.is_empty() { None } else { rule },
                "{movement}, {storage}, {call}: {diagnostics:?}"
            );
        }
    }
}

#[test]
fn whole_file_call_arguments_check_expiry_in_evaluation_order() {
    use hirsel_engine::DefinitionQueries;
    for (call, rule) in [
        ("file_first(take fresh, take dir)", None),
        ("directory_first(take dir, take fresh)", Some("PROV-1")),
    ] {
        let source = format!(
            "fn file_first(take f: File, take d: Dir) {{}}\nfn directory_first(take d: Dir, take f: File) {{}}\npub fn main(t: Test) -> Result[(), FileError] {{\nlet p = path.of(\"value\")\nlet dir = test.dir(t, [(p, b\"data\")])\nlet fresh = file.open(dir, p)?\n{call}\nOk(())\n}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("argument-order.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
            Ok(output) => output.diagnostics,
            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{error:?}"),
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            rule,
            "{call}: {diagnostics:?}"
        );
    }
}

#[test]
fn error_mapper_occurrences_check_the_owned_payloads_receiving_modes() {
    use hirsel_engine::DefinitionQueries;
    for (header, rule) in [
        ("fn error[E](take e: unique E) {}", Some("MODE-2")),
        ("fn error[E](take e: unique sendable E) {}", Some("MODE-2")),
        ("fn error[E](take e: sendable E) {}", Some("MODE-4")),
        ("fn error[E: sendable](take e: E) {}", Some("MODE-4")),
        ("fn error[E](take e: E) {}", None),
        ("fn error[E](e: E) {}", None),
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let source = format!(
            "{header}\nfn main(take b: Bytes) -> Result[(), ()] {{\nstring.from_bytes(copy b) ? error\nOk(())\n}}"
        );
        let file = engine.sources.add("mapper.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let diagnostics = match engine.mode_definition(&hirsel_engine::entry_path(&context)) {
            Ok(output) => output.diagnostics,
            Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
            Err(error) => panic!("{error:?}"),
        };
        assert_eq!(
            diagnostics.first().map(|d| d.rule.as_str()),
            rule,
            "{header}: {diagnostics:?}"
        );
    }
    for result in [
        "unique Result[(), unique Bytes]",
        "unique Result[(), Bytes]",
    ] {
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let source = format!(
            "fn error[E](take e: unique E) {{}}\nfn main(take r: {result}) -> Result[(), ()] {{ (take r) ? error\nOk(()) }}"
        );
        let file = engine.sources.add("unique-mapper.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        engine
            .lower_to_tir(&hirsel_engine::entry_path(&context))
            .unwrap();
    }
}

#[test]
fn annotated_nested_receivers_keep_their_declared_payload_contract() {
    use hirsel_engine::DefinitionQueries;
    let source = "fn inspect(x:box List[Bytes]){}\nfn probe(){let xs:List[Bytes]=[b\"x\"];let built=box unique xs;let received:box List[Bytes]=take built;inspect(received)}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("nested.hir", source.into()).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut path = hirsel_engine::entry_path(&context);
    path.0.name = "probe".into();
    let output = engine.mode_definition(&path).unwrap();
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let checked = &output.value.typed.checked;
    let call = checked.calls.first().unwrap();
    let argument = call.arguments[0].expression;
    let actual = checked
        .expressions
        .iter()
        .find(|row| row.expression == argument)
        .unwrap()
        .ty;
    let TypeKind::Box(payload) = engine.types().get(actual).unwrap().kind else {
        panic!()
    };
    assert_eq!(
        engine.types().get(payload).unwrap().modes.uniqueness,
        Uniqueness::Aliased
    );
}

// Current capture evidence survives owning callable and aggregate transport.
use hirsel_driver::*;
use hirsel_engine::*;

fn capture_crossing_matrix(payload: &str, initial: &str, measure: &str, stable: bool) {
    with_compiler_stack(|| {
        for (history, setup, source, current) in [
            ("fresh", "", "xs", true),
            ("shared", "{let alias=copy xs;}", "xs", stable),
            (
                "independent",
                "let alias=copy xs;let fresh=unique copy xs;",
                "fresh",
                true,
            ),
        ] {
            for (usage, crosses) in [("take", current), ("copy", stable)] {
                for transport in ["direct", "record"] {
                    let callable = "once fn()->u64 uses{}";
                    let expression = format!("fn()->u64{{{measure}({usage} {source})}}");
                    let (declaration, received, expression) = if transport == "record" {
                        (
                            format!("type Holder{{value:{callable}}}"),
                            "Holder",
                            format!("Holder(value:{expression})"),
                        )
                    } else {
                        (String::new(), callable, expression)
                    };
                    let program = |requirement: &str| {
                        format!(
                            "fn identity(x:u64)->u64 uses{{}}{{x}}\n{declaration}\nfn need(take value:{requirement}{received}){{}}\nfn probe(){{let xs:unique {payload}={initial};{setup}need({expression})}}\npub fn main(){{probe()}}"
                        )
                    };
                    let label = format!("{payload}/{history}/{usage}/{transport}");
                    eprintln!(
                        "CAPTURE_CASE {}",
                        serde_json::json!({"label":label,"strict":program("sendable "),"relaxed":program(""),"crosses":crosses})
                    );
                    let mut strict = QueryEngine::new(default_environment(), compiler_adapters());
                    let file = strict
                        .sources
                        .add("capture.hir", program("sendable "))
                        .unwrap();
                    let checked = check_progress(&mut strict, file, &default_context()).unwrap();
                    assert_eq!(
                        checked.diagnostics.first().map(|d| d.rule.as_str()),
                        (!crosses).then_some("MODE-4"),
                        "{label}: {:?}",
                        checked.diagnostics
                    );
                    let mut relaxed = QueryEngine::new(default_environment(), compiler_adapters());
                    let file = relaxed.sources.add("capture-v.hir", program("")).unwrap();
                    let checked = check_progress(&mut relaxed, file, &default_context()).unwrap();
                    assert!(
                        checked.diagnostics.is_empty(),
                        "{label}: {:?}",
                        checked.diagnostics
                    );
                    let input = relaxed.export_native_input().unwrap();
                    let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
                    signatures
                        .extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
                    let mut path = entry_path(&default_context());
                    path.0.name = "probe".into();
                    hirsel_validate::assert_valid(
                        &input.definitions[&path],
                        &input.types,
                        &signatures,
                    );
                    let mut need = input
                        .signatures
                        .values()
                        .find(|s| s.definition.path().0.name == "need")
                        .unwrap()
                        .as_ref()
                        .clone();
                    let SignatureKind::Function(function) = &mut need.kind else {
                        panic!()
                    };
                    let mut parameters = function.parameters.to_vec();
                    let mut row = input.types.get(parameters[0].ty).unwrap().clone();
                    row.sendability_requirement = SendabilityRequirement::Sendable;
                    row.modes.sendability = Sendability::Sendable;
                    parameters[0].ty = input.types.intern(row);
                    function.parameters = parameters.into();
                    signatures.insert(need.definition, need);
                    let violations = hirsel_validate::validate(
                        &input.definitions[&path],
                        &input.types,
                        &signatures,
                        &mut vec![],
                    )
                    .violations;
                    if crosses {
                        assert!(violations.is_empty(), "{label}: {violations:?}");
                    } else {
                        assert!(
                            violations.iter().any(|v| matches!(
                                v.kind,
                                hirsel_validate::ViolationKind::TypeMismatch {
                                    entity: "call argument"
                                }
                            )),
                            "{label}: {violations:?}"
                        );
                    }
                    eprintln!("capture {label}: J/V crosses={crosses}");
                }
            }
        }
    });
}

#[test]
fn nested_owners_need_current_unique_or_independent_capture_content() {
    capture_crossing_matrix("List[String]", "[\"old\"]", "list.len", false);
}

#[test]
fn scalars_do_not_acquire_counted_capture_obligations() {
    capture_crossing_matrix("u64", "7u64", "identity", true);
}

#[test]
fn counted_strings_need_current_unique_or_independent_capture_content() {
    capture_crossing_matrix("String", "\"old\"", "string.byte_len", false);
}

#[test]
fn handle_capture_copies_cross_without_owning_the_payload() {
    with_compiler_stack(|| {
        for payload in ["Shared[List[String]]", "Mutex[List[String]]"] {
            for usage in ["copy", "take"] {
                let source = format!(
                    "fn keep(take x:{payload}) uses{{}}{{}}\nfn need(take x:sendable once fn()->()){{}}\nfn probe(take xs:{payload}){{let alias=copy xs;need(fn(){{keep({usage} xs)}} )}}\npub fn main(){{}}"
                );
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("handles.hir", source).unwrap();
                let output = check_progress(&mut engine, file, &default_context()).unwrap();
                assert!(
                    output.diagnostics.is_empty(),
                    "{payload}/{usage}: {:?}",
                    output.diagnostics
                );
                engine.export_native_input().unwrap();
                eprintln!("capture {payload}/{usage}: J/V crosses=true");
            }
        }
    });
}

#[test]
fn owning_factory_crossing_requires_a_proved_public_contract() {
    with_compiler_stack(|| {
        for (setup, argument, accepted) in [
            ("", "take xs", true),
            ("let alias=copy xs;", "take xs", false),
            ("let alias=copy xs;", "unique copy xs", true),
        ] {
            let source = format!(
                "fn wrap(take xs:sendable List[String])->sendable once fn()->u64 uses{{}}{{fn(){{list.len(take xs)}}}}\nfn need[T:sendable](take x:T){{}}\nfn probe(){{let xs:unique List[String]=[\"old\"];{setup}need(wrap({argument}) )}}\npub fn main(){{probe()}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("factory.hir", source.clone()).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                output.diagnostics.first().map(|d| d.rule.as_str()),
                (!accepted).then_some("MODE-4"),
                "{source}: {:?}",
                output.diagnostics
            );
            if accepted {
                engine.export_native_input().unwrap();
            }
        }
    });
}

#[test]
fn scoped_borrowing_cannot_be_converted_to_owning_by_current_uniqueness() {
    with_compiler_stack(|| {
        for setup in ["", "let alias=copy xs;"] {
            let source = format!(
                "fn consume(take xs:List[String]){{}}\nfn probe(){{let xs:unique List[String]=[\"old\"];{setup}let f=fn(){{consume(take xs)}};f()}}\npub fn main(){{probe()}}"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("scoped.hir", source).unwrap();
            let output = check_progress(&mut engine, file, &default_context()).unwrap();
            assert_eq!(
                output.diagnostics.first().map(|d| d.rule.as_str()),
                Some("OWN-1"),
                "{:?}",
                output.diagnostics
            );
        }
    });
}

#[test]
fn stale_capture_and_environment_claims_are_reproved_from_live_operands() {
    with_compiler_stack(|| {
        let source = "fn need(take f:once fn()->u64 uses{}){}\nfn probe(){let xs:unique List[String]=[\"old\"];{let alias=copy xs;}need(fn(){list.len(take xs)})}\npub fn main(){probe()}";
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("stale-capture.hir", source.into())
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let definition = &input.definitions[&path];
        hirsel_validate::assert_valid(definition, &input.types, &signatures);
        for claim in ["environment", "unique capture", "sendable capture"] {
            let claim_capture = claim != "environment";
            let mut forged = definition.as_ref().clone();
            let entry = forged.entry.0 as usize;
            let (destination, child, captured) = forged.bodies[entry]
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find_map(|i| match i.kind {
                    hirsel_tir::InstructionKind::Assign { destination, value } => {
                        match &forged.bodies[entry].rvalues[value.0 as usize] {
                            hirsel_tir::Rvalue::Closure { body, captures, .. } => {
                                Some((destination, *body, captures[0].place))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .unwrap();
            let stored = input
                .types
                .get(forged.bodies[entry].places[captured.0 as usize].ty)
                .unwrap();
            assert_eq!(stored.modes.uniqueness, Uniqueness::Unique);
            assert_eq!(stored.modes.sendability, Sendability::Sendable);
            let (body, local) = if claim_capture {
                let body = child.0 as usize;
                let local = forged.bodies[body]
                    .locals
                    .iter()
                    .position(|l| l.storage == hirsel_tir::Storage::Capture)
                    .unwrap();
                (body, local)
            } else {
                (
                    entry,
                    forged.bodies[entry].places[destination.0 as usize].root.0 as usize,
                )
            };
            let mut row = input
                .types
                .get(forged.bodies[body].locals[local].ty)
                .unwrap()
                .clone();
            assert_eq!(row.modes.sendability, Sendability::NotSendable);
            if claim == "unique capture" {
                row.modes.uniqueness = Uniqueness::Unique;
            } else {
                row.modes.sendability = Sendability::Sendable;
            }
            let ty = input.types.intern(row);
            let mut types = forged.types.to_vec();
            types.push(ty);
            forged.types = types.into();
            forged.bodies[body].locals[local].ty = ty;
            for place in &mut forged.bodies[body].places {
                if place.root.0 as usize == local && place.projection.is_empty() {
                    place.ty = ty;
                }
            }
            let violations =
                hirsel_validate::validate(&forged, &input.types, &signatures, &mut vec![])
                    .violations;
            let expected = match claim {
                "unique capture" => "value modes",
                "sendable capture" => "closure capture",
                _ => "closure sendability",
            };
            assert!(
                violations
                    .iter()
                    .any(|v| v.kind
                        == hirsel_validate::ViolationKind::TypeMismatch { entity: expected }),
                "{expected}: {violations:?}"
            );
            eprintln!("stale {expected} rejected from current shared capture: {violations:?}");
        }
    });
}

#[test]
fn shared_taken_capture_cannot_satisfy_a_unique_parameter() {
    with_compiler_stack(|| {
        let source = |requirement: &str| {
            format!(
                "fn consume(take xs:{requirement}List[String])->u64{{list.len(xs)}}\nfn need(take f:once fn()->u64 uses{{}}){{}}\nfn probe(){{let xs:unique List[String]=[\"old\"];{{let alias=copy xs;}}need(fn(){{consume(take xs)}})}}\npub fn main(){{probe()}}"
            )
        };
        let mut strict = QueryEngine::new(default_environment(), compiler_adapters());
        let file = strict
            .sources
            .add("unique-capture.hir", source("unique "))
            .unwrap();
        let checked = check_progress(&mut strict, file, &default_context()).unwrap();
        assert_eq!(
            checked.diagnostics.first().map(|d| d.rule.as_str()),
            Some("MODE-2"),
            "{:?}",
            checked.diagnostics
        );
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("unique-capture-v.hir", source(""))
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let input = engine.export_native_input().unwrap();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let definition = &input.definitions[&path];
        hirsel_validate::assert_valid(definition, &input.types, &signatures);
        let mut consume = input
            .signatures
            .values()
            .find(|s| s.definition.path().0.name == "consume")
            .unwrap()
            .as_ref()
            .clone();
        let SignatureKind::Function(header) = &mut consume.kind else {
            panic!()
        };
        let mut parameters = header.parameters.to_vec();
        let mut row = input.types.get(parameters[0].ty).unwrap().clone();
        row.modes.uniqueness = Uniqueness::Unique;
        parameters[0].ty = input.types.intern(row);
        header.parameters = parameters.into();
        signatures.insert(consume.definition, consume);
        let violations =
            hirsel_validate::validate(definition, &input.types, &signatures, &mut vec![])
                .violations;
        assert!(
            violations.iter().any(|v| v.kind
                == hirsel_validate::ViolationKind::TypeMismatch {
                    entity: "call argument"
                }),
            "{violations:?}"
        );
    });
}
