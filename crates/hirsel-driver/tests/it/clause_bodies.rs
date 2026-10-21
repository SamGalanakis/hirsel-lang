#[test]
fn executable_clauses_have_one_capture_free_borrowed_producer_across_early_returns() {
    use hirsel_engine::DefinitionQueries;
    let source = "fn probe(take x:u32, c:bool)->u32 requires x>0u32 ensures result==before(x) {if c {return x}; x}";
    let mut engine = hirsel_engine::QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = hirsel_engine::default_context();
    let file = engine.sources.add("clauses.hir", source.into()).unwrap();
    engine.parse_file(file, &context).unwrap();
    let mut path = hirsel_engine::entry_path(&context);
    path.0.name = "probe".into();
    let definition = engine.lower_to_tir(&path).unwrap().value;
    assert_eq!(definition.contracts.len(), 2);
    let mut calls = 0;
    for (_, clause) in &definition.contracts {
        let body = &definition.bodies[clause.0 as usize];
        assert!(
            !body
                .locals
                .iter()
                .any(|l| l.storage == hirsel_tir::Storage::Capture)
        );
        let mut producers = 0;
        for parent in &definition.bodies {
            for block in &parent.blocks {
                for instruction in &block.instructions {
                    if let hirsel_tir::InstructionKind::Assign { destination, value } =
                        instruction.kind
                    {
                        if let hirsel_tir::Rvalue::Closure {
                            body: target,
                            ref captures,
                            ..
                        } = parent.rvalues[value.0 as usize]
                        {
                            if target == *clause {
                                producers += 1;
                                assert!(captures.is_empty());
                                let hirsel_types::TypeKind::Function(f) = &engine
                                    .types()
                                    .get(parent.places[destination.0 as usize].ty)
                                    .unwrap()
                                    .kind
                                else {
                                    panic!()
                                };
                                assert!(
                                    f.parameters
                                        .iter()
                                        .all(|p| p.passing == hirsel_syntax::PassingMode::Borrow)
                                );
                            }
                        }
                    }
                }
                calls += usize::from(matches!(
                    block.terminator,
                    hirsel_tir::Terminator::Call {
                        callee: hirsel_tir::Callee::Indirect { .. },
                        ..
                    }
                ));
            }
        }
        assert_eq!(producers, 1);
    }
    assert_eq!(calls, 6); // Three calls (requires and two exits), visited for two clauses.
}
