use super::*;
use crate::layout::OwnerRepresentation;
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::RepresentationRecord;
use hirsel_types::*;
use std::sync::Arc;

#[test]
fn opaque_nominal_structural_access_uses_independent_private_owner() {
    let types = TypeInterner::new(None);
    let path = DefPath::core("PrivateEventKey");
    let definition = DefId::intern(&path);
    let string = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::String)));
    let shape = types.intern(Type::with_kind(TypeKind::Tuple(vec![string].into())));
    let root = types.intern(Type::with_kind(TypeKind::Named {
        definition,
        arguments: Default::default(),
    }));
    let signature = Arc::new(Signature {
        definition,
        metadata: Default::default(),
        fingerprint: ContentHash::of_bytes(b"public opaque key"),
        kind: SignatureKind::Type(Box::new(TypeDefinition {
            loop_source: None,
            format: None,
            destructor_allocation: Allocation::NoAlloc,
            generics: vec![],
            representation: None,
            bounds: vec![],
            authority: AuthoritySummary::Unknown,
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Default::default(),
            distinct: false,
            resource: false,
            drop: None,
            fact: None,
            captures: None,
        })),
    });
    let record = RepresentationRecord {
        definition: path,
        representation_revision: ContentHash::of_bytes(b"private string tuple"),
        generics: vec![],
        private_shape: shape,
        layout_promises: Default::default(),
        resource: false,
        finalizer: None,
        unsafe_ffi_package: false,
    };
    let owners = Owners {
        nominals: [(definition, OwnerRepresentation::Defined(record))].into(),
        ..Default::default()
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut builder = LayoutBuilder::new(&target, &owners);
    builder.layout(&types, root).unwrap();
    let table = builder.finish();
    let mut lookup = |d| Ok((d == definition).then(|| signature.clone()));
    let plan = derive(&types, root, &mut lookup, &owners, &target, &table)
        .expect("validated private owner supplies opaque structure for native import");
    validate(&plan, &types, root, &mut lookup, &owners, &target, &table).unwrap();
    // Neither a candidate layout nor its own structural graph can replace
    // the independent declaration and private representation inputs.
    assert!(
        derive(
            &types,
            root,
            &mut lookup,
            &Owners::default(),
            &target,
            &table
        )
        .is_err()
    );
    assert!(derive(&types, root, &mut |_| Ok(None), &owners, &target, &table).is_err());
}
