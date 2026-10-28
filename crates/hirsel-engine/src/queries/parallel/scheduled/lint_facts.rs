//! Frozen package-lint facts use the source session's interned identities.
use super::*;

#[derive(Clone)]
pub(super) struct LintFacts {
    errors: Box<[DefId]>,
    constructed: Box<[(DefId, u32)]>,
}
impl LintFacts {
    pub(super) fn capture(uses: crate::tools::DeadArmUse) -> Self {
        Self {
            errors: uses
                .errors
                .into_iter()
                .map(|path| DefId::intern(&path))
                .collect(),
            constructed: uses
                .constructed
                .into_iter()
                .map(|(path, arm)| (DefId::intern(&path), arm))
                .collect(),
        }
    }
    pub(super) fn materialize(&self) -> crate::tools::DeadArmUse {
        crate::tools::DeadArmUse {
            errors: self.errors.iter().map(|id| id.path().clone()).collect(),
            constructed: self
                .constructed
                .iter()
                .map(|(id, arm)| (id.path().clone(), *arm))
                .collect(),
        }
    }
    pub(super) fn capture_bytes(uses: &crate::tools::DeadArmUse) -> u64 {
        uses.errors
            .len()
            .saturating_mul(size_of::<DefId>())
            .saturating_add(
                uses.constructed
                    .len()
                    .saturating_mul(size_of::<(DefId, u32)>()),
            ) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_lint_facts_preserve_paths_arms_and_wire_bytes_with_bounded_backing() {
        for count in [0, 1, 16] {
            let paths = (0..count.max(1))
                .map(|i| DefPath::core(&format!("{}_error{i}", "long".repeat(256))))
                .collect::<Vec<_>>();
            for path in &paths {
                DefId::intern(path);
            }
            let mut original = crate::tools::DeadArmUse::default();
            for (arm, path) in paths.iter().enumerate() {
                original.errors.insert(path.clone());
                original.constructed.insert((path.clone(), arm as u32));
            }
            if count == 0 {
                original.errors.remove(&paths[0]);
                original.constructed.remove(&(paths[0].clone(), 0));
            }
            let bound = LintFacts::capture_bytes(&original);
            let (repeated, sizing) =
                crate::test_alloc::measured_independent(|| LintFacts::capture_bytes(&original));
            assert_eq!(repeated, bound);
            assert_eq!(sizing.allocations, 0);
            assert_eq!(sizing.live, 0);
            let expected = serde_json::to_vec(&original).unwrap();
            let (compact, measured) =
                crate::test_alloc::measured(|| LintFacts::capture(original.clone()));
            assert!(measured.live >= 0);
            assert!(
                measured.live as u64 <= bound,
                "live={}, bound={bound}",
                measured.live
            );
            assert_eq!(compact.errors.len(), count as usize);
            assert_eq!(compact.constructed.len(), count as usize);
            let restored = compact.materialize();
            assert_eq!(restored.errors, original.errors);
            assert_eq!(restored.constructed, original.constructed);
            assert_eq!(serde_json::to_vec(&restored).unwrap(), expected);
            assert_eq!(
                bound,
                (size_of_val(&*compact.errors) + size_of_val(&*compact.constructed)) as u64
            );
            if count == 0 {
                assert_eq!(bound, 0);
            } else {
                assert!(bound > 0);
            }
        }
    }
}
