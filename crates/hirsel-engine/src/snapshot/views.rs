pub use hirsel_validate::{PackageViews, View, ViewTable};

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::DefId;
    use hirsel_types::SignatureRef;
    use hirsel_validate::ValidatedSignatures;
    use std::sync::Arc;

    #[test]
    fn universe_input_digest_covers_valid_views_and_package_replacements() {
        let types = crate::registry::session_types();
        let registry = crate::registry::BuiltinRegistry::new();
        let option = registry
            .get(DefId::intern(&hirsel_syntax::DefPath::core("Option")))
            .unwrap();
        let result = registry
            .get(DefId::intern(&hirsel_syntax::DefPath::core("Result")))
            .unwrap();
        let table = |rows: Vec<SignatureRef>| {
            Arc::new(ViewTable::new(
                &types,
                rows,
                None,
                &mut ValidatedSignatures::default(),
                None,
            ))
        };
        let first = table(vec![option.clone(), result.clone()]);
        let reordered = table(vec![result.clone(), option.clone()]);
        assert_eq!(first.input_digest(), reordered.input_digest());
        let mut changed = (*option).clone();
        changed.metadata.visibility = hirsel_syntax::Visibility::PublicOpaque;
        // Even a forged unchanged producer fingerprint cannot hide V input changes.
        assert_eq!(changed.fingerprint, option.fingerprint);
        let changed = Arc::new(changed);
        let second = table(vec![changed.clone(), result]);
        assert_ne!(first.input_digest(), second.input_digest());
        assert_ne!(first.input_digest(), table(vec![option]).input_digest());
        let package = PackageViews::new(
            &types,
            first.clone(),
            [View::new(&types, changed, None)],
            &mut ValidatedSignatures::default(),
            None,
        );
        assert_eq!(package.input_digest(), second.input_digest());
        assert_eq!(
            first.input_digest(),
            reordered.input_digest(),
            "replacement leaves base immutable"
        );
    }
}
