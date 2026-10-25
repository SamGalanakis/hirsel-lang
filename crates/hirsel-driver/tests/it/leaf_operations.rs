//! LANG-10 leaf operations against their reviewed core declarations: each is a
//! declared core primitive that needs no host authority, takes no callback and
//! releases nothing it is handed, so it can run no language code.
use hirsel_core_contract::depth::{LEAF_MODULES, LEAF_OPERATIONS, adds_activation};
use hirsel_engine::registry::{BuiltinRegistry, review::describe_function, shared_types};
use hirsel_syntax::DefPath;
use hirsel_types::SignatureKind;
use std::collections::BTreeSet;

#[test]
fn every_leaf_operation_is_a_declared_primitive_without_callbacks_or_host_authority() {
    let registry = BuiltinRegistry::new();
    let mut leaves = BTreeSet::new();
    let resources: BTreeSet<String> = registry
        .signatures()
        .filter_map(|s| match &s.kind {
            SignatureKind::Type(t) if t.resource => Some(s.definition.path().0.name.clone()),
            _ => None,
        })
        .collect();
    for signature in registry.signatures() {
        let SignatureKind::Function(function) = &signature.kind else {
            continue;
        };
        let path = signature.definition.path();
        if adds_activation(path) {
            continue;
        }
        let described = describe_function(shared_types(), function);
        let name = format!("{}.{}", path.0.modules.join("."), path.0.name);
        assert!(
            !described.contains("fn("),
            "{name} takes a callback: {described}"
        );
        // The only authority a scalar leaf may name is a borrowed `Unsafe` permit.
        assert!(
            described.contains("authority=Known([])")
                || described.starts_with("(Borrow permit:Unsafe[],"),
            "{name} needs host authority: {described}"
        );
        // A taken value could be released, running its finalizer: only list.push
        // takes a generic value (and stores it); any other taken type is concrete
        // and names no resource.
        for taken in taken_types(&described) {
            if name == "list.push" && taken == "T0" {
                continue;
            }
            for word in taken.split(|c: char| !c.is_ascii_alphanumeric()) {
                assert!(
                    !(word.len() > 1
                        && word.starts_with('T')
                        && word[1..].bytes().all(|b| b.is_ascii_digit())),
                    "{name} takes a generic value: {described}"
                );
                assert!(
                    !resources.contains(word),
                    "{name} takes a resource: {described}"
                );
            }
        }
        leaves.insert(name);
    }
    for name in LEAF_OPERATIONS {
        assert!(
            leaves.contains(*name),
            "{name} is not a declared core primitive"
        );
    }
    for module in LEAF_MODULES {
        assert!(
            leaves.iter().any(|l| l.starts_with(&format!("{module}."))),
            "{module} declares no operation"
        );
    }
    // Only direct calls of core declarations are leaves.
    assert!(adds_activation(&DefPath::core("list.set")));
    assert!(adds_activation(&DefPath::core("arena.with")));
    assert!(adds_activation(&DefPath::core("out.print")));
}

/// The declared types of `described`'s `Take` parameters, without relations.
fn taken_types(described: &str) -> Vec<&str> {
    let parameters = &described[1..described.find(")->").expect("a function")];
    let mut depth = 0;
    let mut start = 0;
    let mut taken = Vec::new();
    for (at, c) in parameters.char_indices().chain([(parameters.len(), ',')]) {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                let parameter = &parameters[start..at];
                if let Some(rest) = parameter.strip_prefix("Take ") {
                    let ty = rest.split_once(':').map_or(rest, |(_, ty)| ty);
                    taken.push(ty.split('@').next().unwrap_or(ty));
                }
                start = at + 1;
            }
            _ => (),
        }
    }
    taken
}
