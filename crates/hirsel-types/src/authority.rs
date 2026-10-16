//! Intrinsic held authority, distinct from permission to invoke an operation.
use crate::*;

/// Authority intrinsic to a builtin constructor, before joining its retained
/// payloads. Canonical capability identities come from the signature contract;
/// spellings and phase-private intrinsic maps are not evidence. Missing roles
/// and erased iterator captures remain unknown, never implicitly inert.
///
/// This summary does not supply an explicit governing capability operand.
/// Phantom type arguments of Ptr/FnPtr/SlotId/Shape do not contribute payloads.
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
