use crate::support;
use hirsel_interp::{library::*, *};
use hirsel_syntax::{ContentHash, Visibility};
use hirsel_tir::*;
use hirsel_types::*;
use support::{Provider, ty};
fn signature(name: &str, parameters: &[&str]) -> Signature {
    let mut types = TypeStore::default();
    let unit = types.push(ty(TypeKind::Tuple(vec![].into())));
    let error = types.push(ty(TypeKind::Named {
        definition: support::path("E"),
        arguments: vec![].into(),
    }));
    let result = types.push(ty(TypeKind::Named {
        definition: hirsel_syntax::DefPath::core("Result"),
        arguments: vec![GenericArgument::Type(unit), GenericArgument::Type(error)].into(),
    }));
    let parameters = parameters
        .iter()
        .map(|name| Parameter {
            name: Some(name.to_string()),
            label: None,
            passing: PassingMode::Borrow,
            ty: types.push(ty(TypeKind::Named {
                definition: hirsel_syntax::DefPath::core(name),
                arguments: vec![].into(),
            })),
        })
        .collect();
    Signature {
        definition: support::path(name),
        types,
        fingerprint: ContentHash::default(),
        metadata: SignatureMetadata {
            visibility: Visibility::Public,
            ..Default::default()
        },
        kind: SignatureKind::Function(FunctionType {
            state_promises: Default::default(),
            generics: vec![].into(),
            parameters,
            result,
            error: Some(error),
            allocation: Allocation::MayAllocate,
            authority: AuthoritySummary::inert(),
            uses: Uses::Unrestricted,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }),
    }
}
#[test]
fn invalid_and_unavailable_entries_never_query_a_body_or_partly_grant() {
    let run = RunConfiguration {
        entry: support::path("main"),
        hash_seed: 0,
        inputs: vec![],
    };
    for roots in [vec!["Io", "Unsafe"], vec!["Dir"]] {
        let sig = signature("main", &roots);
        let mut provider = Provider {
            definitions: vec![],
            queries: vec![],
        };
        let error = launch(
            &mut provider,
            &support::sources(),
            &sig,
            &run,
            Default::default(),
            Limits::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            LaunchError::CannotGrant(_) | LaunchError::InvalidEntry(_)
        ));
        assert!(provider.queries.is_empty());
    }
    let mut sig = signature("main", &[]);
    sig.metadata.visibility = Visibility::Private;
    let mut lib = Library::new(&run, Default::default()).unwrap();
    assert!(matches!(
        lib.entry_arguments(&sig),
        Err(LaunchError::InvalidEntry(_))
    ));
    sig.metadata.visibility = Visibility::Public;
    let unit = sig.types.push(ty(TypeKind::Tuple(vec![].into())));
    if let SignatureKind::Function(f) = &mut sig.kind {
        f.result = unit;
    }
    assert!(matches!(
        lib.entry_arguments(&sig),
        Err(LaunchError::InvalidEntry(_))
    ));
    let mut sig = signature("main", &["Io"]);
    let imposter = sig.types.push(ty(TypeKind::Named {
        definition: support::path("Io"),
        arguments: vec![].into(),
    }));
    if let SignatureKind::Function(f) = &mut sig.kind {
        f.parameters[0].ty = imposter;
    }
    assert!(matches!(
        lib.entry_arguments(&sig),
        Err(LaunchError::InvalidEntry(_))
    ));
}
#[test]
fn explicitly_configured_unsafe_is_granted_and_configuration_is_deterministic() {
    let sig = signature("main", &["Unsafe"]);
    let run = RunConfiguration {
        entry: sig.definition.clone(),
        hash_seed: 1,
        inputs: vec![],
    };
    let mut config = SimulationConfig::default();
    config.grants.insert("Unsafe".into());
    let mut library = Library::new(&run, config.clone()).unwrap();
    let first = library.entry_arguments(&sig).unwrap();
    assert!(matches!(&first[0],Value::Handle{kind,..}if kind=="Unsafe"));
    let mut other = Library::new(&run, config).unwrap();
    assert_eq!(other.entry_arguments(&sig).unwrap(), first);
}
