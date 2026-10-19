//! Authority checking of corpus programs, from hirsel-authority's `corpus` unit tests.
use hirsel_syntax as ast;
use hirsel_types::*;

#[test]
fn divergent_alias_joins_publish_fresh_identities_and_preserve_original_owners() {
    use hirsel_engine::DefinitionQueries;
    let prefix = "type R{n:u64}\nfn make(r:R)->Option[u8 of r]{None}\nfn inspect(r:R,p:Option[u8 of r])->(){}\n";
    for branch in [
        "if c{r=copy z}",
        "match c{true=>{r=copy z},false=>{}}",
        "while c{r=copy z;c=false}",
        "for i in 0u8..2u8{r=copy z}",
        "let xs:List[u8]=[1u8,2u8];for i in xs{r=copy z}",
        "let xs:List[u8]=[1u8,2u8];for i in take xs{r=copy z}",
    ] {
        for (tail, accepted) in [
            ("let q=make(r);inspect(r,q)", true),
            ("inspect(x,p)", true),
            ("inspect(r,p)", false),
        ] {
            let source = format!(
                "{prefix}fn probe(take x:R,take z:R,flag:bool){{let mut c=flag;let mut r=copy x;let p=make(r);{branch};{tail}}}"
            );
            let mut engine = hirsel_engine::QueryEngine::new(
                hirsel_engine::default_environment(),
                hirsel_driver::compiler_adapters(),
            );
            let context = hirsel_engine::default_context();
            let file = engine
                .sources
                .add("join-identity.hir", source.clone())
                .unwrap();
            engine.parse_file(file, &context).unwrap();
            let mut definition = hirsel_engine::entry_path(&context);
            definition.0.name = "probe".into();
            let out = match engine.mode_definition(&definition) {
                Ok(out) => out,
                Err(QueryError::Diagnostics(diagnostics)) => {
                    assert!(!accepted, "{source}: {diagnostics:?}");
                    assert_eq!(
                        diagnostics.first().map(|d| d.rule.as_str()),
                        Some("PROV-1"),
                        "{source}"
                    );
                    assert!(!diagnostics[0].secondary.is_empty());
                    continue;
                }
                Err(error) => panic!("{source}: {error:?}"),
            };
            assert_eq!(
                out.diagnostics.first().map(|d| d.rule.as_str()),
                (!accepted).then_some("PROV-1"),
                "{source}: {:?}",
                out.diagnostics
            );
            let syntax = &out.value.typed.syntax;
            let root = out
                .value
                .typed
                .checked
                .bindings
                .iter()
                .find_map(|binding| {
                    let NameSite::Pattern(pattern) = binding.site else {
                        return None;
                    };
                    let ast::PatternKind::Binding(name) =
                        syntax.tree.patterns[pattern.0 as usize].kind
                    else {
                        return None;
                    };
                    let ResolvedName::Local(local) = binding.binding else {
                        return None;
                    };
                    (name.as_str() == "r").then_some(OriginRoot::Local(syntax.owner(), local))
                })
                .expect("r binding");
            assert_eq!(
                out.value
                    .ownership
                    .identity_joins
                    .iter()
                    .filter(|(_, _, origin)| origin.root == root && origin.fields.is_empty())
                    .count(),
                1,
                "one fresh r identity at its control head: {source}"
            );
        }
    }
}
