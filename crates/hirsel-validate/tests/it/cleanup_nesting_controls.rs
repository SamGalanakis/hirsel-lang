//! Hand-built controls for cleanup release order (DROP-2): an exit releases
//! each scope it leaves innermost first, each scope's temporaries before its
//! names. A global temporaries-first order breaks the nesting on failure exits.
use super::*;

/// `x` named and `t` a temporary of the outer scope; `inner` named in a nested
/// scope. The exit releases `releases` in order.
fn nested(releases: impl Fn(u32, u32, u32) -> Vec<u32>) -> Fixture {
    let mut f = Fixture::unit();
    let string = || TypeKind::Primitive(Primitive::String);
    let x = f.slot(string(), Storage::Local);
    let t = f.slot(string(), Storage::Temporary);
    f.body().scopes.push(Scope {
        parent: Some(ScopeId(0)),
        span: Span::default(),
    });
    let inner = f.slot(string(), Storage::Local);
    f.body().locals[inner as usize].scope = ScopeId(1);
    f.assign(x, Rvalue::Constant(Constant::String("x".into())), 0);
    let c1 = f.cleanup(vec![drop(x)]);
    f.assign(t, Rvalue::Constant(Constant::String("t".into())), c1);
    let c2 = f.cleanup(vec![drop(t), drop(x)]);
    f.assign(
        inner,
        Rvalue::Constant(Constant::String("inner".into())),
        c2,
    );
    let exit = f.cleanup(releases(x, t, inner).into_iter().map(drop).collect());
    f.body().blocks[0].terminator = ret(0, exit);
    f
}

#[test]
fn control_an_inner_name_releases_before_an_outer_temporary() {
    nested(|x, t, inner| vec![inner, t, x]).valid();
    // The former global order releases the outer temporary first.
    assert!(
        nested(|x, t, inner| vec![t, inner, x])
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::CleanupOrder(_))),
    );
    // Within the outer scope its temporary still precedes its name.
    assert!(
        nested(|x, t, inner| vec![inner, x, t])
            .kinds()
            .iter()
            .any(|kind| matches!(kind, ViolationKind::CleanupOrder(_))),
    );
}

#[test]
fn an_exact_exit_cannot_omit_the_newest_live_inner_owner() {
    let f = nested(|x, t, _inner| vec![t, x]);
    assert!(f.kinds().iter().any(|kind| matches!(
        kind,
        ViolationKind::CleanupMismatch { expected, actual, .. }
            if expected.len() == 3 && actual.len() == 2
                && expected.iter().any(|root| !actual.contains(root))
    )));
}
