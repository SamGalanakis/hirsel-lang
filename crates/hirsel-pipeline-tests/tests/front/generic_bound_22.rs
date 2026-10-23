//! GEN-1: complete expected callable types discharge the direct-call bounds.
//! Binder isolation follows Koka Type/Unify.hs:177-214,374-404 and
//! test/type/wrong/subsume.kk:1-7 at 198fc3bbd158246e7604210ee6692ee72c2d9c50
//! (Apache-2.0). Only the test shape is adapted: Hirsel uses structural bounds
//! and prefix contexts, with no Koka effect inference or type-class predicates.
#[path = "generic_bound_22/model.rs"]
mod model;
use hirsel_engine::{
    QueryEngine, check_snapshot, default_context, default_environment, entry_path,
};

#[test]
fn generic_bound_22_source_forms_match_bounded_model() {
    let mut accepted = 0;
    let mut rejected = 0;
    for (seed, shape) in model::shapes().iter().enumerate() {
        let mut declarations = String::new();
        let actual = shape.source(&mut declarations, &mut 0);
        for key in [false, true] {
            let bound = if key { "key" } else { "data" };
            for form in ["direct", "named", "lexical", "dictionary"] {
                let signature = format!("fn({actual},Inline[u8,2])->bool uses {{}}");
                let generic = format!(
                    "fn same[n:size,T:{bound}](value:T,marker:Inline[u8,n])->bool uses {{}} {{value==value}}\n"
                );
                let body = match form {
                    "direct" => "same(value,marker)".into(),
                    "named" => format!("let f:{signature}=same;f(value,marker)"),
                    "lexical" => format!(
                        // Reuse T/n in distinct binders, with the positions reversed.
                        "fn equal_inner[T:{bound},n:size](input:T,tag:Inline[u8,n])->bool uses {{}} {{input==input}}\nlet f:{signature}=equal_inner;f(value,marker)"
                    ),
                    "dictionary" => "(OP).run(value,marker)".into(),
                    _ => unreachable!(),
                };
                let dictionary = if form == "dictionary" {
                    format!("type Op {{run:{signature}}}\nconst OP:Op=Op(run:same)\n")
                } else {
                    String::new()
                };
                let source = format!(
                    "{declarations}{generic}{dictionary}fn probe(value:{actual},marker:Inline[u8,2])->bool uses {{}} {{{body}}}\npub fn main(){{}}"
                );
                let mut engine =
                    QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
                let file = engine
                    .sources
                    .add("generic-bound-22.hir", source.clone())
                    .unwrap();
                let diagnostics = match check_snapshot(&mut engine, file, &default_context()) {
                    Ok(ds) | Err(hirsel_types::QueryError::Diagnostics(ds)) => ds,
                    Err(e) => panic!("seed={seed} {bound}/{form}\n{source}\n{e:?}"),
                };
                if shape.accepts(key) {
                    assert!(
                        diagnostics.is_empty(),
                        "seed={seed} {bound}/{form}\n{source}\n{diagnostics:?}"
                    );
                    // Explicitly lower and validate probe, even with an empty main.
                    // export_native_input alone would not prove this body's validity.
                    use hirsel_engine::DefinitionQueries;
                    let mut probe = entry_path(&default_context());
                    probe.0.name = "probe".into();
                    let definition = engine.lower_to_tir(&probe).unwrap().value.clone();
                    let input = engine.export_native_input().unwrap();
                    let mut signatures = hirsel_validate::SignatureMap::new(engine.types().clone());
                    for signature in input.signatures.values() {
                        signatures.insert(signature.definition, (**signature).clone());
                    }
                    hirsel_validate::assert_valid(&definition, engine.types(), &signatures);
                    accepted += 1;
                } else {
                    assert!(
                        !diagnostics.is_empty(),
                        "seed={seed} {bound}/{form}\n{source}"
                    );
                    assert!(
                        diagnostics.iter().all(|d| d.rule.as_str() == "GEN-1"),
                        "seed={seed} {bound}/{form}\n{source}\n{diagnostics:?}"
                    );
                    rejected += 1;
                }
            }
        }
    }
    assert_eq!((accepted, rejected), (32, 16));
    eprintln!(
        "generic_bound_22: 48 source cells, {accepted} explicit probe V accepts, {rejected} source GEN-1 rejects"
    );
}
