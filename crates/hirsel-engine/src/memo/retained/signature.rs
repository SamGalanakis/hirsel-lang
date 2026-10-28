//! Strict source-header payload bound. Unlike diagnostic sharing estimates,
//! each shared occurrence is charged and no temporary identity set is allocated.
use super::Weight;
use hirsel_types::{FamilyMemberKind, SignatureKind, SignatureRef};

pub(crate) fn retained_signature_payload_bytes(signature: &SignatureRef) -> u64 {
    let mut weight = Weight {
        count_shared_repeats: true,
        ..Default::default()
    };
    signature_payload(&mut weight, signature);
    weight.bytes
}

pub(super) fn signature_payload(weight: &mut Weight<'_>, signature: &SignatureRef) {
    weight.signature(signature);
    // These owned containers are outside the old diagnostic signature visitor.
    weight.vec(&signature.metadata.implementations, |w, binder| {
        w.add(binder.name.capacity());
        w.vec(&binder.members, |w, (name, _)| w.add(name.capacity()));
    });
    for constraint in &signature.metadata.schema {
        match constraint {
            hirsel_types::SchemaConstraint::Bound { rule, .. }
            | hirsel_types::SchemaConstraint::Domain { rule, .. } => {
                weight.add(rule.retained_bytes())
            }
            _ => (),
        }
    }
    match &signature.kind {
        SignatureKind::Family(family) => {
            weight.flat_vec(&family.members);
            for member in &family.members {
                if let FamilyMemberKind::Type { bounds } = &member.kind {
                    weight.shared_slice(bounds);
                }
            }
        }
        SignatureKind::Implementation(implementation) => weight.flat_vec(&implementation.members),
        SignatureKind::Const { .. } | SignatureKind::Function(_) | SignatureKind::Type(_) => (),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_types::*;
    use std::sync::Arc;
    #[test]
    fn source_headers_charge_shared_roles_once_with_a_complete_allocation_bound() {
        let owner = hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("role_header"));
        let ((headers, bound), allocations) = crate::test_alloc::measured(|| {
            let roles = Arc::new(NominalRoles {
                result_type: Some(hirsel_syntax::DefPath::core(&"Result".repeat(64))),
                ..Default::default()
            });
            let headers = std::array::from_fn::<_, 4, _>(|_| Arc::new(Signature {
                definition: owner,
                metadata: SignatureMetadata { roles: roles.clone(), ..Default::default() },
                kind: SignatureKind::Const { ty: TypeId::DANGLING, generics: vec![] },
                fingerprint: Default::default(),
            }));
            let (bound, inspection) = crate::test_alloc::measured_independent(|| {
                let mut names = super::super::NameAllocations::<64>::default();
                let mut weight = Weight { count_shared_repeats: true, source_names: Some(&mut names), ..Default::default() };
                for header in &headers { signature_payload(&mut weight, header); }
                weight.bytes
            });
            assert_eq!(inspection.allocations, 0);
            assert!(bound < headers.iter().map(retained_signature_payload_bytes).sum());
            (headers, bound)
        });
        assert!(allocations.live as u64 <= bound, "{} > {bound}", allocations.live);
        let (_, released) = crate::test_alloc::measured(|| drop(headers));
        assert_eq!(-released.live, allocations.live);
    }

    #[test]
    fn retained_headers_cover_owned_binders_members_rules_and_shared_generics_without_allocating() {
        let owner = hirsel_syntax::DefId::intern(&hirsel_syntax::DefPath::core("source.header"));
        let target = hirsel_syntax::DefPath::core("source.header.target");
        for width in [1, 4, 16] {
            for kind in 0..3 {
                let (signature, actual) = crate::test_alloc::measured(|| {
                    let metadata = SignatureMetadata {
                        implementations: (0..width)
                            .map(|i| ImplementationBinder {
                                name: format!("implementation{i}"),
                                family: owner,
                                members: (0..width)
                                    .map(|j| (format!("member{j}"), j as u32))
                                    .collect(),
                            })
                            .collect(),
                        schema: vec![SchemaConstraint::Bound {
                            parameter: BinderRef::Bound {
                                depth: 0,
                                parameter: TypeParamId(0),
                            },
                            bound: Bound::Copyable,
                            rule: hirsel_diag::RuleId::new("HEADER-1").unwrap(),
                        }],
                        ..Default::default()
                    };
                    let kind = match kind {
                        0 => SignatureKind::Family(FamilySignature {
                            members: (0..width)
                                .map(|i| FamilyMember {
                                    name: format!("family_member{i}"),
                                    kind: FamilyMemberKind::Type {
                                        bounds: vec![Bound::Copyable; width].into(),
                                    },
                                })
                                .collect(),
                        }),
                        1 => SignatureKind::Implementation(ImplementationSignature {
                            family: owner,
                            members: (0..width)
                                .map(|i| {
                                    (
                                        format!("operation{i}"),
                                        MemberActual::Operation(target.clone()),
                                    )
                                })
                                .collect(),
                        }),
                        _ => SignatureKind::Const {
                            ty: TypeId::DANGLING,
                            generics: (0..width)
                                .map(|i| GenericParameter::Type {
                                    name: format!("parameter{i}"),
                                    bounds: vec![Bound::Copyable; width].into(),
                                })
                                .collect::<Vec<_>>()
                                .into(),
                        },
                    };
                    Arc::new(Signature {
                        metadata,
                        definition: owner,
                        kind,
                        fingerprint: Default::default(),
                    })
                });
                let (bound, inspection) =
                    crate::test_alloc::measured(|| retained_signature_payload_bytes(&signature));
                assert_eq!(inspection.allocations, 0);
                assert!(
                    actual.live as u64 <= bound,
                    "width {width} kind {kind}: actual {} exceeds {bound}",
                    actual.live
                );
                let (_, release) = crate::test_alloc::measured(|| drop(signature));
                assert_eq!(-release.live, actual.live);
            }
        }
    }
}
