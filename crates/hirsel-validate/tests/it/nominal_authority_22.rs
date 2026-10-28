//! Bounded, hand-built declaration controls. No source normalization verdict is
//! an input, and the owner's proof is checked separately from the public view.
use crate::support::*;
use hirsel_syntax::Visibility;
use hirsel_types::*;
use hirsel_validate::{ValidatedSignatures, ViolationKind};

fn nominal(name: &str) -> AuthoritySummary {
    AuthoritySummary::Known(vec![AuthorityTerm::Capability(path(name))].into())
}

fn declaration(kind: CapabilityKind, owner: bool) -> Signature {
    let representation = interner().intern(ty(TypeKind::Record(
        vec![Field {
            name: "payload".into(),
            ty: interner().intern(ty(TypeKind::Primitive(Primitive::U64))),
            growable: false,
        }]
        .into(),
    )));
    Signature {
        definition: id("Seal"),
        metadata: SignatureMetadata {
            visibility: Visibility::PublicOpaque,
            ..Default::default()
        },
        fingerprint: Default::default(),
        kind: SignatureKind::Type(Box::new(TypeDefinition {
            generics: vec![],
            representation: owner.then_some(representation),
            bounds: if kind == CapabilityKind::Global {
                vec![Bound::Plain]
            } else {
                vec![]
            },
            authority: nominal("Seal"),
            capability: Some(kind),
            implementation_authority: owner.then(AuthoritySummary::inert),
            serializable: false,
            serializable_check: None,
            layout: Default::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
            loop_source: None,
            format: None,
        })),
    }
}

fn data(s: &mut Signature) -> &mut TypeDefinition {
    let SignatureKind::Type(d) = &mut s.kind else {
        panic!("type")
    };
    d
}

fn verdict(s: Signature, accepted: bool) {
    let mut f = Fixture::unit();
    let name = s.definition;
    f.signatures.insert(name, s);
    let expected = if accepted {
        vec![]
    } else {
        vec![ViolationKind::InvalidSignature(name.path().clone())]
    };
    // Both the unused declaration universe and the full direct-TIR entry check
    // must reject a forged promise; neither consumes a producer's fingerprint.
    f.broken(expected.clone());
    let mut checked = ValidatedSignatures::default();
    let universe = checked.universe(
        interner(),
        f.signatures.keys().collect::<Vec<_>>(),
        &f.signatures,
    );
    assert_eq!(
        universe
            .errors
            .iter()
            .map(|v| v.kind.clone())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn nominal_authority_22_identity_is_exact_in_owner_and_public_views() {
    // Four pairs: package/global and version/local identity, in each view.
    // Only the summary's identity changes; the declaration key stays fixed.
    for (kind, axis) in [
        (CapabilityKind::Global, "package"),
        (CapabilityKind::Scoped, "version"),
    ] {
        for owner in [true, false] {
            let good = declaration(kind, owner);
            verdict(good.clone(), true);
            let mut wrong = path("Seal");
            match axis {
                "package" => wrong.0.package.name = "client".into(),
                "version" => wrong.0.package.version = "2".into(),
                _ => unreachable!(),
            }
            let mut bad = good;
            data(&mut bad).authority =
                AuthoritySummary::Known(vec![AuthorityTerm::Capability(wrong)].into());
            verdict(bad, false);
        }
    }
}

#[test]
fn nominal_authority_22_advertised_seal_bounds_are_not_proofs() {
    // Eight pairs. Even a scalar representation and empty private ceiling do
    // not prove Inert or Serializable, in either declaration view.
    for kind in [CapabilityKind::Global, CapabilityKind::Scoped] {
        for bound in [Bound::Inert, Bound::Serializable] {
            for owner in [true, false] {
                let good = declaration(kind, owner);
                verdict(good.clone(), true);
                let mut bad = good;
                data(&mut bad).bounds.push(bound);
                verdict(bad, false);
            }
        }
    }
}
