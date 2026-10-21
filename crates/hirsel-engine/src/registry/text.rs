//! Compiler-created text builders whose every append preserves UTF-8.
use super::{build::Builder, *};

pub(super) fn signatures() -> Vec<Signature> {
    let mut b = Builder::new("");
    let mut f = b.function("take builder: List[u8]", "String uses {}", false);
    f.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(0)).into(),
        advance_only: false,
        preserve_captures: false,
        preserve_unique: true,
        unretained: Default::default(),
    }]
    .into();
    let mut signature = b.signature("$intrinsic.TextFinish", SignatureKind::Function(f), false);
    signature.metadata.visibility = hirsel_syntax::Visibility::Private;
    vec![signature]
}
