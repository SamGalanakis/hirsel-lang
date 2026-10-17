use hirsel_engine::{check_operation_coverage, registry::*};
use hirsel_interp::library::{OperationDisposition, operation_dispatch};
use hirsel_types::{PassingMode, SignatureKind};
use std::collections::BTreeSet;
#[test]
fn registry_and_dispatch_have_exact_bidirectional_callable_coverage() {
    let registry = BuiltinRegistry::new();
    let records = registry.operation_inventory();
    let declarations: BTreeSet<_> = registry
        .signatures()
        .filter_map(|s| {
            matches!(s.kind, SignatureKind::Function(_)).then_some(s.definition.clone())
        })
        .collect();
    let dispatch = operation_dispatch();
    assert_eq!(
        records
            .keys()
            .map(|k| k.definition.clone())
            .collect::<BTreeSet<_>>(),
        declarations
    );
    check_operation_coverage(declarations.clone(), dispatch.keys().cloned()).unwrap();
    assert!(records.values().all(|r| r.reference.is_some()));
    assert_eq!(
        dispatch[&hirsel_syntax::DefPath::core("task.async")],
        OperationDisposition::Capability
    );
    assert_eq!(
        dispatch[&hirsel_syntax::DefPath::core("forget")],
        OperationDisposition::Evaluator
    );
    let inserted = hirsel_syntax::DefPath::core("new.declaration.without.implementation");
    let mut extra_row = declarations.clone();
    extra_row.insert(inserted.clone());
    let missing = check_operation_coverage(extra_row, dispatch.keys().cloned()).unwrap_err();
    assert_eq!(missing.missing_implementations, BTreeSet::from([inserted]));
    let inserted = hirsel_syntax::DefPath::core("new.implementation.without.declaration");
    let mut extra_dispatch: BTreeSet<_> = dispatch.keys().cloned().collect();
    extra_dispatch.insert(inserted.clone());
    let missing = check_operation_coverage(declarations, extra_dispatch).unwrap_err();
    assert_eq!(
        missing.undeclared_implementations,
        BTreeSet::from([inserted])
    );
}
#[test]
fn operation_identity_retains_typed_modes_cleanup_and_the_declaration_digest() {
    let records = BuiltinRegistry::new().operation_inventory();
    let record = records
        .values()
        .find(|r| r.key.definition == hirsel_syntax::DefPath::core("file.narrow"))
        .unwrap();
    assert_eq!(
        record.key.declaration_digest,
        record.declaration.fingerprint
    );
    let SignatureKind::Function(function) = &record.declaration.kind else {
        unreachable!()
    };
    assert_eq!(record.parameters[1].passing, PassingMode::Take);
    assert_eq!(
        record.parameters[1].cleanup,
        record
            .declaration
            .types
            .get(function.parameters[1].ty)
            .unwrap()
            .drop_allocation
    );
    assert_eq!(
        record.result_cleanup,
        record
            .declaration
            .types
            .get(function.result)
            .unwrap()
            .drop_allocation
    );
    assert_eq!(record.invocation_allocation, function.allocation);
    assert!(record.effects.observable && record.effects.may_fail);
}

#[test]
fn network_operations_publish_their_current_engine_dispositions() {
    use hirsel_core_contract::{
        CallOwnership, CleanupEffect, Disposition, OperationId, OperationRow, ParameterOwnership,
        Passing, Unavailable,
    };
    use hirsel_interp::{Error, OperationCall, OperationContext, Operations};
    let registry = BuiltinRegistry::new();
    let records = registry.operation_inventory();
    let run = hirsel_tir::RunConfiguration {
        entry: hirsel_syntax::DefPath::core("test.net"),
        hash_seed: 0,
        inputs: vec![],
    };
    let sources = hirsel_span::SourceMap::default();
    let mut library = hirsel_interp::library::Library::new(&run, Default::default()).unwrap();
    let mut stdout = vec![];
    let mut trace = vec![];
    let names = [
        "net.narrow",
        "net.resolve",
        "net.connect",
        "net.connect_addr",
        "net.read",
        "net.read_into",
        "net.write",
        "net.write_all",
        "net.shutdown",
        "net.set_read_timeout",
        "net.set_write_timeout",
        "net.peer_addr",
        "net.close",
        "tls.connect",
        "tls.close",
        "net.listen",
        "net.accept",
        "net.local_addr",
        "test.net",
    ];
    for name in names {
        let record = records
            .values()
            .find(|r| r.key.definition == hirsel_syntax::DefPath::core(name))
            .unwrap();
        assert_eq!(
            record.reference,
            Some(
                if matches!(name, "net.listen" | "net.accept" | "net.local_addr") {
                    OperationDisposition::Unavailable
                } else {
                    OperationDisposition::Capability
                }
            ),
            "{name}"
        );
        let mut row = OperationRow::unclassified(OperationId {
            definition: record.key.definition.clone(),
            declaration_digest: record.key.declaration_digest,
        });
        assert_eq!(
            record.result_cleanup,
            hirsel_types::DropAllocation::NoAlloc,
            "{name}"
        );
        row.ownership = Some(CallOwnership {
            parameters: record
                .parameters
                .iter()
                .map(|p| {
                    assert_eq!(p.cleanup, hirsel_types::DropAllocation::NoAlloc, "{name}");
                    ParameterOwnership {
                        ordinal: p.ordinal.0,
                        passing: match p.passing {
                            PassingMode::Borrow => Passing::Borrow,
                            PassingMode::Mut => Passing::Mut,
                            PassingMode::Take => Passing::Take,
                        },
                        cleanup: CleanupEffect::NoAlloc,
                    }
                })
                .collect(),
            result_cleanup: CleanupEffect::NoAlloc,
        });
        let native = hirsel_rt::operations::classify(row).unwrap();
        // Native entries land per ticket: T2's stream exports are
        // runtime primitives and read_into a compiled body over its fixed entry; the rest
        // stay explicitly unavailable until their tickets.
        assert_eq!(
            native.dispositions.native,
            if name == "test.net" || hirsel_rt::operations::NET_EXPORTS.contains(&name) {
                Disposition::RuntimePrimitive(format!("hirsel_rt_v2_{}", name.replace('.', "_")))
            } else if name == "net.read_into" {
                Disposition::Body
            } else {
                Disposition::Unavailable(Unavailable::NotImplemented)
            },
            "{name}"
        );
        let SignatureKind::Function(function) = &record.declaration.kind else {
            panic!("{name}")
        };
        if !matches!(name, "net.listen" | "net.accept" | "net.local_addr") {
            continue;
        }
        // M2 unavailability precedes argument access or any simulated effect.
        let result = library.call(
            OperationCall {
                path: &record.key.definition,
                generics: &[],
                arguments: &[],
                result_type: function.result,
                types: &record.declaration.types,
                site: hirsel_span::Span::default(),
            },
            &mut OperationContext {
                trace_mode: Default::default(),
                anchor: None,
                configuration: &run,
                sources: &sources,
                stdout: &mut stdout,
                trace: &mut trace,
            },
        );
        assert!(
            matches!(result, Err(Error::Unavailable(_))),
            "{name}: {result:?}"
        );
    }
    assert!(stdout.is_empty() && trace.is_empty());
}
