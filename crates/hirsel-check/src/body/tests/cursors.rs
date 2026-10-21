use super::super::*;
use super::{properties, support};
use hirsel_engine::registry::BuiltinRegistry;
use hirsel_types::core_roles::loop_protocols;
use std::sync::Arc;

const ALL: &str = "fn main(take m:Map[u8,unique Bytes], take s:SortedMap[u16,unique String], scalar:ScalarView, positions:PositionView) { for item in take m {}\n for item in take s {}\n for item in scalar {}\n for item in positions {} }";

fn run(
    source: &str,
    seed: &[String],
    protocols: &[LoopProtocolPaths],
    change: impl FnOnce(&mut support::Registry),
) -> (Result<Output, QueryError>, support::Registry) {
    let (file, _, ds) = support::parse(source, seed);
    assert!(ds.is_empty(), "{ds:?}");
    let mut provider = support::registry(&file);
    for signature in BuiltinRegistry::new().signatures() {
        provider
            .signatures
            .insert(signature.definition.path().clone(), signature.clone());
    }
    support::normalize_file(&file, &mut provider).unwrap();
    let item = &file.items[0];
    let resolution = hirsel_resolve::resolve_item(
        &file,
        item,
        &provider.index.clone(),
        super::support::session(),
        &mut provider,
    );
    assert!(
        resolution.query.diagnostics.is_empty(),
        "{:?}",
        resolution.query.diagnostics
    );
    let signature = provider.signatures[&item.item.definition].clone();
    change(&mut provider);
    provider.reads.clear();
    let output = check_with_loop_protocols(
        LoopInput {
            body: Input {
                types: super::support::session(),
                memo: super::support::memo(),
                syntax: item,
                context: &support::context(),
                resolution: &resolution,
                signature: &signature,
            },
            protocols,
        },
        &mut provider,
    );
    (output, provider)
}

#[test]
fn consuming_map_loops_require_both_owning_protocol_declarations() {
    let mappings = loop_protocols();
    for source in [
        "fn main(take m:Map[u8,u16]) {for item in take m {}}",
        "fn main(take m:SortedMap[u8,u16]) {for item in take m {}}",
    ] {
        assert!(run(source, &[], &[], |_| {}).0.is_err());
    }
    let output = run(ALL, &[], &mappings[..1], |_| {}).0;
    assert!(output.is_err());
}

#[test]
fn cursor_plans_do_not_depend_on_symbols_mapping_order_or_signature_store_ids() {
    let mappings = loop_protocols();
    let (first, _) = run(ALL, &[], &mappings, |_| {});
    let first = first.unwrap();
    let mut reverse = mappings.clone();
    reverse.reverse();
    let seed = vec!["noise".into(), "main".into(), "String".into()];
    let (second, _) = run(ALL, &seed, &reverse, |provider| {
        provider.signatures = provider
            .signatures
            .iter()
            .rev()
            .map(|(p, s)| (p.clone(), Arc::new(properties::transport(s, 7))))
            .collect();
    });
    let second = second.unwrap();
    properties::equivalent(&first.checked, &second.checked);
    for (a, b) in first.plans.loops.iter().zip(&second.plans.loops) {
        let (a, b) = (a.cursor().unwrap(), b.cursor().unwrap());
        assert_eq!(
            (
                &a.open,
                &a.next,
                a.source,
                a.step.none_variant,
                a.step.some_variant
            ),
            (
                &b.open,
                &b.next,
                b.source,
                b.step.none_variant,
                b.step.some_variant
            )
        );
        assert!(support::session().semantic_eq(a.cursor, b.cursor));
        assert!(support::session().semantic_eq(a.step.result, b.step.result));
    }
}

#[test]
fn cursor_tags_come_from_the_provider_layout() {
    let (output, _) = run(ALL, &[], &loop_protocols(), |provider| {
        let option = Arc::make_mut(
            provider
                .signatures
                .get_mut(&support::core("Option"))
                .unwrap(),
        );
        let SignatureKind::Type(td) = &mut option.kind else {
            panic!()
        };
        let mut row = super::support::session()
            .row(td.representation.unwrap())
            .clone();
        let TypeKind::Variant(variants) = &mut row.kind else {
            panic!()
        };
        variants.reverse();
        td.representation = Some(super::support::session().intern(row));
        option.fingerprint = ast::ContentHash::of_bytes(b"reversed cursor Option");
    });
    let output = output.unwrap();
    assert!(output.checked.diagnostics.is_empty());
    for plan in &output.plans.loops {
        let cursor = plan.cursor().unwrap();
        assert_eq!(
            (cursor.step.none_variant, cursor.step.some_variant),
            (VariantId(1), VariantId(0))
        );
    }
}

#[test]
fn failed_cursor_signature_reads_propagate_and_remain_in_the_provider() {
    let mappings = loop_protocols();
    for missing in [&mappings[0].1, &mappings[0].2] {
        let (output, provider) = run(
            "fn main(take m:Map[u8,u16]) { for item in take m {} }",
            &[],
            &mappings,
            |provider| {
                provider.signatures.remove(missing);
            },
        );
        assert!(matches!(output, Err(QueryError::NotImplemented)));
        assert!(provider.reads.contains(missing));
    }
}

#[test]
fn mappings_use_supplied_identities_and_malformed_protocols_fail_operationally() {
    let mappings = loop_protocols();
    let source = "fn main(take m:Map[u8,u16]) { for item in take m {} }";
    let mut renamed = mappings.clone();
    renamed[0].1 = support::path("private_open");
    renamed[0].2 = support::path("private_next");
    let (output, provider) = run(source, &[], &renamed, |provider| {
        for (old, new) in [
            (&mappings[0].1, &renamed[0].1),
            (&mappings[0].2, &renamed[0].2),
        ] {
            let mut signature = (*provider.signatures.remove(old).unwrap()).clone();
            signature.definition = new.clone().into();
            provider.signatures.insert(new.clone(), Arc::new(signature));
        }
    });
    let output = output.unwrap();
    assert_eq!(output.plans.loops[0].cursor().unwrap().open, renamed[0].1);
    assert!(!provider.reads.contains(&mappings[0].1));
    for operation in [&mappings[0].1, &mappings[0].2] {
        let (output, _) = run(source, &[], &mappings, |provider| {
            let signature = Arc::make_mut(provider.signatures.get_mut(operation).unwrap());
            let SignatureKind::Function(function) = &mut signature.kind else {
                panic!()
            };
            function.parameters[0].passing = ast::PassingMode::Borrow;
        });
        assert!(matches!(output, Err(QueryError::Operational(_))));
    }
    let mut duplicate = mappings.clone();
    duplicate.push(mappings[0].clone());
    let (output, _) = run(source, &[], &duplicate, |_| {});
    assert!(matches!(output, Err(QueryError::Operational(_))));
}

#[test]
fn consuming_sequences_require_taking_open_declarations() {
    let source = "fn main(take xs:Array[Bytes]) { for item in take xs { let _ = take item } }";
    assert!(run(source, &[], &[], |_| {}).0.is_err());
    let mappings = loop_protocols();
    let open = mappings
        .iter()
        .find(|(name, _, _)| *name == "ArrayConsume")
        .unwrap()
        .1
        .clone();
    let (result, _) = run(source, &[], &mappings, |provider| {
        let signature = Arc::make_mut(provider.signatures.get_mut(&open).unwrap());
        let SignatureKind::Function(function) = &mut signature.kind else {
            panic!()
        };
        function.parameters[0].passing = ast::PassingMode::Borrow;
    });
    assert!(result.is_err());
}
