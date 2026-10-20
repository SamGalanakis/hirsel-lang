//! P0: the validated snapshot as the data the native crates read. Nothing here
//! selects semantics; it copies declarations, revisions, instance requirements
//! and owner representations out of `NativeInput`.
use hirsel_core_contract::{
    CallOwnership, CleanupEffect, Manifest, OperationId, OperationRow, ParameterOwnership, Passing,
    SemanticEffects,
};
use hirsel_engine::{InstanceKind, NativeInput, ValidatedNativeInput};
use hirsel_layout::layout::{OwnerRepresentation, Owners};

use hirsel_lir::stage::InputIdentity;
use hirsel_rc::instance::{Program, Requirement, RequirementKind};
use hirsel_syntax::ContentHash;
use hirsel_types::{BinderRef, DropAllocation, NameSite, PassingMode, SignatureKind};

/// The identity every native artifact of this snapshot carries: the interface
/// version and a digest of what the snapshot selects (environment, registry,
/// every declaration fingerprint, implementation revision, representation
/// revision and instance requirement).
pub fn input_identity(input: &NativeInput) -> InputIdentity {
    let mut parts: Vec<Vec<u8>> = vec![
        hirsel_tir::TIR_VERSION.to_le_bytes().to_vec(),
        input.environment.interface_version.to_le_bytes().to_vec(),
        input.environment.compiler_version.as_bytes().to_vec(),
        input.environment.spec_hash.0.to_vec(),
        input.environment.package_manifest_hash.0.to_vec(),
        input.environment.builtin_signatures_hash.0.to_vec(),
        input.registry_revision.0.to_vec(),
    ];
    for (path, signature) in &input.signatures {
        parts.push(path.fingerprint().0.to_vec());
        parts.push(signature.fingerprint.0.to_vec());
    }
    for (path, owner) in &input.source_owners {
        parts.push(path.fingerprint().0.to_vec());
        parts.push(owner.implementation_revision.0.to_vec());
    }
    for (path, record) in &input.representations {
        parts.push(path.fingerprint().0.to_vec());
        parts.push(match record {
            Some(record) => record.representation_revision.0.to_vec(),
            None => Vec::new(),
        });
    }
    for edge in &input.instances.edges {
        parts.push(edge.owner.fingerprint().0.to_vec());
        parts.push(edge.callee.fingerprint().0.to_vec());
        parts.push(format!("{:?}", (edge.kind, edge.site.start, edge.site.end)).into_bytes());
    }
    for link in &input.foreign_links {
        parts.push(format!("{link:?}").into_bytes());
    }
    if input.foreign_unreplayable {
        parts.push(b"foreign-unreplayable".to_vec());
    }
    for (package, threads) in &input.foreign_threads {
        parts.push(format!("threads {package:?} {}", threads.name()).into_bytes());
    }
    InputIdentity {
        interface_version: input.environment.interface_version,
        digest: ContentHash::of_parts("hirsel.native-input.v2", parts.iter().map(Vec::as_slice)),
    }
}

/// The program the native passes expand, with the owner representations
/// layout derivation reads.
pub fn program(validated: &ValidatedNativeInput) -> Program {
    let input = validated.snapshot();
    let kind = |k: InstanceKind| match k {
        InstanceKind::Call => RequirementKind::Call,
        InstanceKind::Reference => RequirementKind::Reference,
        InstanceKind::Drop => RequirementKind::Drop,
        InstanceKind::Representation => RequirementKind::Representation,
        InstanceKind::Materialized => RequirementKind::Materialized,
    };
    let mut owners = Owners::default();
    for (path, record) in &input.representations {
        let signature = &input.signatures[path];
        let SignatureKind::Type(t) = &signature.kind else {
            continue;
        };
        let owner = match record {
            Some(record) => OwnerRepresentation::Defined(record.clone()),
            // Primitive capabilities are logical run-context handles. Instant uses
            // the same opaque one-word layout for ticks, with no handle allocation.
            None if t.capability.is_some() || *path == hirsel_syntax::DefPath::core("Instant") => {
                OwnerRepresentation::Handle {
                    revision: signature.fingerprint,
                    resource: t.resource,
                }
            }
            // A library nominal whose shape is public (Result, Option): its
            // record is its declaration.
            None => match t.representation {
                Some(shape) => OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
                    definition: path.clone(),
                    representation_revision: signature.fingerprint,
                    generics: t.generics.clone(),
                    types: signature.types.clone(),
                    private_shape: shape,
                    layout_promises: t.layout.clone(),
                    resource: t.resource,
                    finalizer: None,
                    unsafe_ffi_package: signature.metadata.unsafe_ffi,
                }),
                // An opaque library nominal has no native layout: layout
                // derivation reports it unavailable where it is used.
                None => continue,
            },
        };
        owners.nominals.insert(path.clone(), owner);
    }
    Program {
        input: input_identity(input),
        definitions: input.definitions.clone(),
        signatures: input.signatures.clone(),
        revisions: input
            .source_owners
            .iter()
            .map(|(p, o)| (p.clone(), o.implementation_revision))
            .collect(),
        binders: input
            .source_owners
            .iter()
            .map(|(p, o)| (p.clone(), o.syntax.owner().site(NameSite::Item)))
            .collect(),
        operations: input
            .operations
            .keys()
            .map(|k| {
                (
                    k.definition.clone(),
                    OperationId {
                        definition: k.definition.clone(),
                        declaration_digest: k.declaration_digest,
                    },
                )
            })
            .collect(),
        requirements: input
            .instances
            .edges
            .iter()
            .map(|e| Requirement {
                owner: e.owner.clone(),
                owner_binder: e.owner_binder,
                site: e.site,
                kind: kind(e.kind),
                callee: e.callee.clone(),
                binder: e.binder,
                types: e.types.clone(),
                arguments: e.arguments.clone(),
            })
            .collect(),
        owners,
        instance_bodies: input.instance_bodies.clone(),
        constants: input.constants.clone(),
    }
}

fn cleanup(value: &DropAllocation) -> Result<CleanupEffect, String> {
    Ok(match value {
        DropAllocation::NoAlloc => CleanupEffect::NoAlloc,
        DropAllocation::MayAllocate => CleanupEffect::MayAllocate,
        DropAllocation::DependsOn(parameters) => CleanupEffect::DependsOn(
            parameters
                .iter()
                .map(|p| match p {
                    BinderRef::Bound { depth: 0, .. } => Ok(p.parameter().0),
                    _ => Err("an operation's cleanup depends on a parameter it does not bind"),
                })
                .collect::<Result<_, _>>()?,
        ),
    })
}

/// The operation manifest of the snapshot's registry inventory, with the
/// runtime's native disposition for every row. A row the runtime has not
/// reviewed is an error, never an unclassified row.
pub fn manifest(input: &NativeInput) -> Result<Manifest, String> {
    let rows = input
        .operations
        .values()
        .map(|record| {
            let mut row = OperationRow::unclassified(OperationId {
                definition: record.key.definition.clone(),
                declaration_digest: record.key.declaration_digest,
            });
            row.ownership = Some(CallOwnership {
                parameters: record
                    .parameters
                    .iter()
                    .map(|p| {
                        Ok(ParameterOwnership {
                            ordinal: p.ordinal.0,
                            passing: match p.passing {
                                PassingMode::Borrow => Passing::Borrow,
                                PassingMode::Mut => Passing::Mut,
                                PassingMode::Take => Passing::Take,
                            },
                            cleanup: cleanup(&p.cleanup)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                result_cleanup: cleanup(&record.result_cleanup)?,
            });
            row.effects = SemanticEffects {
                reads: record.effects.reads,
                writes: record.effects.writes,
                owns: record.effects.owns,
                may_fail: record.effects.may_fail,
                allocates: record.effects.allocates,
                observable: record.effects.observable,
            };
            Ok(row)
        })
        .collect::<Result<Vec<_>, String>>()?;
    hirsel_rt_operations::manifest(rows)
}
