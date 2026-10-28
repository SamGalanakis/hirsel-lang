use super::support::{Fixture, function_signature, id, owner_sig, path, session};
use hirsel_syntax::ContentHash;
use hirsel_tir::BodyId;
use hirsel_types::{SignatureRef, TypeInterner, TypeKind};
use hirsel_validate::{
    PackageViews, SignatureLookup, VRead, ValidatedSignatures, View, ViewTable, validate_definition,
};
use std::sync::Arc;

fn table(
    types: &Arc<TypeInterner>,
    rows: impl IntoIterator<Item = SignatureRef>,
) -> Arc<ViewTable> {
    Arc::new(ViewTable::new(
        types,
        rows,
        None,
        &mut ValidatedSignatures::default(),
        None,
    ))
}

#[test]
fn checked_evidence_binds_body_type_policy_and_private_snapshot() {
    let mut f = Fixture::unit();
    owner_sig(&mut f, &[]);
    let types = session();
    let rows: Vec<_> = f.signatures.values().cloned().map(Arc::new).collect();
    let first = table(&types, rows.clone());
    let definition = Arc::new(f.definition);
    let policy = ContentHash::of_bytes(b"checked-source-policy");
    let (report, ran, _, evidence) = validate_definition(
        &definition,
        types.clone(),
        |_| Some((path("main"), 0)),
        &*first as &dyn hirsel_validate::CheckedEnvironment,
        policy,
        None,
    );
    assert!(ran && report.violations.is_empty(), "{report:?}");
    let evidence = evidence.unwrap();
    assert!(evidence.holds(
        &definition,
        &types,
        policy,
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(evidence.holds(
        &definition,
        &types,
        policy,
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(!evidence.holds(
        &Arc::new((*definition).clone()),
        &types,
        policy,
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(!evidence.holds(
        &definition,
        &TypeInterner::new(None),
        policy,
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(!evidence.holds(
        &definition,
        &types,
        ContentHash::default(),
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
    let equal = table(&types, rows.clone());
    assert!(evidence.holds(
        &definition,
        &types,
        policy,
        &*equal as &dyn hirsel_validate::CheckedEnvironment
    ));
    let mut changed = (*rows
        .iter()
        .find(|s| s.definition == definition.path)
        .unwrap())
    .as_ref()
    .clone();
    changed.metadata.visibility = hirsel_syntax::Visibility::Public;
    assert_eq!(changed.fingerprint, rows[0].fingerprint);
    let private = PackageViews::new(
        &types,
        first.clone(),
        [View::new(&types, Arc::new(changed), None)],
        &mut ValidatedSignatures::default(),
        None,
    );
    assert!(
        !evidence.environment_holds(policy, &private as &dyn hirsel_validate::CheckedEnvironment)
    );
    assert!(
        evidence.environment_holds(policy, &*first as &dyn hirsel_validate::CheckedEnvironment)
    );
    let mut malformed = (*definition).clone();
    malformed.entry = BodyId(u32::MAX);
    let (report, ran, accepted, proof) = validate_definition(
        &Arc::new(malformed),
        types.clone(),
        |_| Some((path("main"), 0)),
        &*first as &dyn hirsel_validate::CheckedEnvironment,
        policy,
        None,
    );
    assert!(ran && !report.violations.is_empty());
    assert!(accepted.is_none() && proof.is_none());
    let copy = Arc::new((*definition).clone());
    drop(definition);
    assert!(!evidence.holds(
        &copy,
        &types,
        policy,
        &*first as &dyn hirsel_validate::CheckedEnvironment
    ));
}

#[test]
fn checked_snapshot_stamp_cannot_hide_unrelated_universe_errors_or_missing_owner() {
    let mut f = Fixture::unit();
    owner_sig(&mut f, &[]);
    let types = session();
    let rows: Vec<_> = f.signatures.values().cloned().map(Arc::new).collect();
    let first = table(&types, rows.clone());
    let definition = Arc::new(f.definition);
    let policy = ContentHash::default();
    let (report, _, _, evidence) = validate_definition(
        &definition,
        types.clone(),
        |_| Some((path("main"), 0)),
        &*first as &dyn hirsel_validate::CheckedEnvironment,
        policy,
        None,
    );
    assert!(report.violations.is_empty());
    let evidence = evidence.unwrap();
    assert!(
        evidence
            .reads()
            .iter()
            .any(|r| matches!(r, VRead::UniverseErrors(_)))
    );
    let invalid = function_signature("unrelated", vec![], TypeKind::Error);
    let changed = table(&types, rows.iter().cloned().chain([Arc::new(invalid)]));
    assert!(!changed.universe_errors().1.is_empty());
    assert!(!evidence.environment_holds(
        policy,
        &*changed as &dyn hirsel_validate::CheckedEnvironment
    ));
    let missing = table(&types, []);
    assert!(
        evidence
            .reads()
            .iter()
            .any(|r| matches!(r, VRead::View(d, _) if *d == id("main")))
    );
    assert!(!evidence.environment_holds(
        policy,
        &*missing as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(
        evidence.environment_holds(policy, &*first as &dyn hirsel_validate::CheckedEnvironment)
    );
}

#[test]
fn foreign_interner_cannot_reuse_a_clean_environment() {
    let mut f = Fixture::unit();
    owner_sig(&mut f, &[]);
    let types = session();
    let rows: Vec<_> = f.signatures.values().cloned().map(Arc::new).collect();
    let first = table(&types, rows.clone());
    let definition = Arc::new(f.definition);
    let policy = ContentHash::default();
    let (report, _, _, evidence) = validate_definition(
        &definition,
        types.clone(),
        |_| Some((path("main"), 0)),
        &*first as &dyn hirsel_validate::CheckedEnvironment,
        policy,
        None,
    );
    assert!(report.violations.is_empty());
    let evidence = evidence.unwrap();
    // A foreign universe is rejected even if all of its numeric rows happen to
    // match. Its views retain their actual originating type owner.
    let foreign = TypeInterner::new(None);
    let foreign = table(&foreign, rows);
    assert!(!evidence.environment_holds(
        policy,
        &*foreign as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(
        evidence.environment_holds(policy, &*first as &dyn hirsel_validate::CheckedEnvironment)
    );
}

#[test]
fn checked_environment_revalidates_first_heap_even_when_all_prior_views_hold() {
    use super::support::arena_slot;
    let mut f = Fixture::call();
    arena_slot(&mut f, hirsel_tir::Storage::Temporary);
    owner_sig(&mut f, &[]);
    let types = session();
    let rows: Vec<_> = f.signatures.values().cloned().map(Arc::new).collect();
    let first = table(&types, rows.clone());
    let definition = Arc::new(f.definition);
    let policy = ContentHash::default();
    let (report, _, _, evidence) = validate_definition(
        &definition,
        types.clone(),
        |_| Some((path("main"), 0)),
        &*first as &dyn hirsel_validate::CheckedEnvironment,
        policy,
        None,
    );
    assert!(report.violations.is_empty(), "{report:?}");
    let evidence = evidence.unwrap();
    assert!(
        evidence
            .reads()
            .contains(&VRead::FirstHeap(Some(id("Arena"))))
    );
    let mut earlier = function_signature("0provider", vec![], TypeKind::Tuple(vec![].into()));
    earlier.metadata.roles = rows
        .iter()
        .find(|s| s.definition == id("Arena"))
        .unwrap()
        .metadata
        .roles
        .clone();
    let changed = table(&types, rows.into_iter().chain([Arc::new(earlier)]));
    assert!(changed.universe_errors().1.is_empty());
    assert!(
        evidence
            .reads()
            .iter()
            .filter(|r| matches!(r, VRead::View(_, _)))
            .all(|r| hirsel_validate::read_holds(r, &*changed))
    );
    assert!(!evidence.environment_holds(
        policy,
        &*changed as &dyn hirsel_validate::CheckedEnvironment
    ));
    assert!(
        evidence.environment_holds(policy, &*first as &dyn hirsel_validate::CheckedEnvironment)
    );
}
