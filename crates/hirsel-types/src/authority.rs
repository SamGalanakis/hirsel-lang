//! Intrinsic held authority, distinct from permission to invoke an operation.
use crate::*;

/// Authority intrinsic to a builtin constructor, before joining its retained
/// payloads. Canonical capability identities come from the signature contract;
/// spellings and phase-private intrinsic maps are not evidence. Missing roles
/// and erased iterator captures remain unknown, never implicitly inert.
///
/// This summary does not supply an explicit governing capability operand.
/// Phantom type arguments of Ptr/FnPtr/SlotId/TypeShape do not contribute payloads.
pub fn builtin_held_authority(kind: BuiltinType, roles: &NominalRoles) -> AuthoritySummary {
    let role = match kind {
        BuiltinType::Iter => return AuthoritySummary::Unknown,
        BuiltinType::Task => &roles.scope_capability,
        BuiltinType::Sender | BuiltinType::Receiver | BuiltinType::Mutex | BuiltinType::Atomic => {
            &roles.tasks_capability
        }
        _ => return AuthoritySummary::inert(),
    };
    role.as_ref()
        .map(|path| AuthoritySummary::Known(vec![AuthorityTerm::Capability(path.clone())].into()))
        .unwrap_or(AuthoritySummary::Unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path(name: &str) -> hirsel_syntax::DefPath {
        hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: hirsel_syntax::PackageId {
                name: "canonical".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        })
    }
    #[test]
    fn handle_authority_uses_nominal_roles_and_never_infers_missing_roles() {
        let roles = NominalRoles {
            tasks_capability: Some(path("Scheduler")),
            scope_capability: Some(path("ChildScope")),
            heap: None,
            ..Default::default()
        };
        for kind in [
            BuiltinType::Sender,
            BuiltinType::Receiver,
            BuiltinType::Mutex,
            BuiltinType::Atomic,
            BuiltinType::Task,
        ] {
            let expected = if kind == BuiltinType::Task {
                "ChildScope"
            } else {
                "Scheduler"
            };
            assert_eq!(
                builtin_held_authority(kind, &roles),
                AuthoritySummary::Known(vec![AuthorityTerm::Capability(path(expected))].into())
            );
            assert_eq!(
                builtin_held_authority(kind, &NominalRoles::default()),
                AuthoritySummary::Unknown
            );
        }
        assert_eq!(
            builtin_held_authority(BuiltinType::Iter, &roles),
            AuthoritySummary::Unknown
        );
        assert_eq!(
            builtin_held_authority(BuiltinType::List, &roles),
            AuthoritySummary::inert()
        );
    }
}

#[cfg(test)]
mod join_tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn authority_union_canonicalizes_once_without_copying_shared_slice_edits() {
        let terms = [
            AuthorityTerm::Capability(hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: hirsel_syntax::PackageId {
                    name: "registry".into(),
                    version: "1".into(),
                },
                modules: vec!["governing".into()],
                name: "Capability".into(),
            })),
            AuthorityTerm::Parameter(ParameterRef::bound(ParamId(3))),
            AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(2))),
        ];
        let choices = [
            vec![],
            vec![0],
            vec![1, 0],
            vec![2, 1, 0, 1, 2],
            vec![2, 2, 1, 0, 0],
        ];
        let before = crate::slice::copied_edit_rows();
        for left in &choices {
            for right in &choices {
                let a = AuthoritySummary::Known(left.iter().map(|i| terms[*i].clone()).collect());
                let b = AuthoritySummary::Known(right.iter().map(|i| terms[*i].clone()).collect());
                let a_snapshot = a.clone();
                let b_snapshot = b.clone();
                let expected: SharedSlice<_> = left
                    .iter()
                    .chain(right)
                    .map(|i| terms[*i].clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let joined = a.clone().join(b.clone());
                assert_eq!(joined, AuthoritySummary::Known(expected));
                assert_eq!(a, a_snapshot);
                assert_eq!(b, b_snapshot);
                assert_eq!(joined.clone().join(joined.clone()), joined);
                assert_eq!(b.clone().join(a.clone()), joined);
                assert_eq!(
                    a.clone().join(AuthoritySummary::Unknown),
                    AuthoritySummary::Unknown
                );
                assert_eq!(AuthoritySummary::Unknown.join(b), AuthoritySummary::Unknown);
            }
        }
        assert_eq!(
            crate::slice::copied_edit_rows() - before,
            0,
            "canonical union edits one owned Vec before freezing its terms"
        );
    }
}
