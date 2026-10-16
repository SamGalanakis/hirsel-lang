#[test]
fn a_fresh_global_replacement_keeps_fixed_local_storage_and_current_global_result() {
    use hirsel_engine::DefinitionQueries;
    for tail in ["slot", "copy slot"] {
        let source = format!(
            "fn view(x:Bytes)->Bytes in x{{copy x}}\nfn probe(x:Bytes)->Bytes{{let mut slot=view(x);slot=b\"fresh\";{tail}}}"
        );
        let mut engine = hirsel_engine::QueryEngine::new(
            hirsel_engine::default_environment(),
            hirsel_driver::compiler_adapters(),
        );
        let context = hirsel_engine::default_context();
        let file = engine.sources.add("receiving.hir", source).unwrap();
        engine.parse_file(file, &context).unwrap();
        let mut path = hirsel_engine::entry_path(&context);
        path.0.name = "probe".into();
        let definition = engine.lower_to_tir(&path).unwrap().value;
        let body = &definition.bodies[definition.entry.0 as usize];
        assert_eq!(
            definition.types.get(body.result).unwrap().modes.relation,
            hirsel_types::Relation::global()
        );
        let destination = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find_map(|instruction| match instruction.kind {
                hirsel_tir::InstructionKind::Replace { destination, .. } => Some(destination),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            definition
                .types
                .get(body.locals[body.places[destination.0 as usize].root.0 as usize].ty)
                .unwrap()
                .modes
                .relation,
            hirsel_types::Relation::environment()
        );
    }
}
