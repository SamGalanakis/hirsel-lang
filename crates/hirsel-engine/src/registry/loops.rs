//! Compiler-only collection operations and nominal borrowed-loop declarations.
use super::{build::Builder, *};
use hirsel_syntax::Visibility;

pub(super) fn signatures(types: &Arc<TypeInterner>) -> Vec<Signature> {
    let b = Builder::new(types, "T");
    let cursor = TypeDefinition {
        loop_source: None,
        format: None,
        generics: b.generics.clone(),
        representation: None,
        bounds: vec![Bound::NoallocDrop],
        authority: AuthoritySummary::Known(
            vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))]
            .into(),
        ),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    };
    let mut out = vec![b.signature(
        "$intrinsic.Cursor",
        SignatureKind::Type(Box::new(cursor.clone())),
        true,
    )];
    out.push(Builder::new(types, "T").signature(
        "$intrinsic.ViewCursor",
        SignatureKind::Type(Box::new(cursor)),
        true,
    ));
    for (protocol, generics, source, item) in [
        ("SetElements", "T: key", "Set[T]", "T"),
        ("UnicodeScalars", "", "ScalarView", "char"),
        ("ScalarPositions", "", "PositionView", "StrPos of source"),
    ] {
        let cursor_name = if protocol == "SetElements" {
            "Cursor"
        } else {
            "ViewCursor"
        };
        let mut b = Builder::new(types, generics);
        let mut f = b.function(
            &format!("source: {source}"),
            &format!("$intrinsic.{cursor_name}[{item}] in source uses {{source}}"),
            true,
        );
        f.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                ParamId(0),
            ))]
            .into(),
        );
        if protocol == "ScalarPositions" {
            f.result = hirsel_types::map_brand_origins(&mut b.types, f.result, |origin, _| {
                let mut origin = origin.clone();
                if let OriginRoot::Parameter(p) = origin.root {
                    origin.root = OriginRoot::ArgumentBrand(p);
                }
                Ok(origin)
            })
            .unwrap();
        }
        let mut result = b.types.get(f.result).unwrap().clone();
        result.drop_allocation = DropAllocation::NoAlloc;
        f.result = b.types.intern(result);
        out.push(b.signature(
            &format!("$intrinsic.{protocol}_open"),
            SignatureKind::Function(f),
            false,
        ));
        let mut b = Builder::new(types, "T");
        let mut f = b.function(
            &format!("mut cursor: $intrinsic.{cursor_name}[T]"),
            "Option[T] in cursor uses {cursor}",
            true,
        );
        f.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                ParamId(0),
            ))]
            .into(),
        );
        out.push(b.signature(
            &format!("$intrinsic.{protocol}_next"),
            SignatureKind::Function(f),
            false,
        ));
    }
    let mut b = Builder::new(types, "T");
    let representation = b.ty("Array[T]", PassingMode::Take);
    let owner = TypeDefinition {
        loop_source: None,
        format: None,
        generics: b.generics.clone(),
        representation: Some(representation),
        bounds: vec![],
        authority: AuthoritySummary::Known(
            vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                TypeParamId(0),
            ))]
            .into(),
        ),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: true,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    };
    out.push(b.signature(
        "$intrinsic.OwnedCursor",
        SignatureKind::Type(Box::new(owner)),
        true,
    ));
    for (protocol, generics, source) in [
        ("ArrayConsume", "T", "Array[T]"),
        ("ListConsume", "T", "List[T]"),
        ("InlineConsume", "T,n:size", "Inline[T,n]"),
    ] {
        let mut b = Builder::new(types, generics);
        let mut f = b.function(
            &format!("take source: {source} in source"),
            "$intrinsic.OwnedCursor[T] in source uses {source}",
            protocol != "InlineConsume",
        );
        f.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
        );
        out.push(b.signature(
            &format!("$intrinsic.{protocol}_open"),
            SignatureKind::Function(f),
            false,
        ));
        let mut b = Builder::new(types, "T");
        let mut f = b.function(
            "mut cursor: $intrinsic.OwnedCursor[T]",
            "Option[T] in cursor uses {cursor}",
            true,
        );
        f.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
        );
        out.push(b.signature(
            &format!("$intrinsic.{protocol}_next"),
            SignatureKind::Function(f),
            false,
        ));
    }
    for (protocol, generics, source, cursor) in [
        (
            "MapInsertionOrder",
            "K: key,V",
            "Map[K,V]",
            "$intrinsic.OwnedMapCursor",
        ),
        (
            "SortedMapOrder",
            "K,V",
            "SortedMap[K,V]",
            "$intrinsic.OwnedSortedMapCursor",
        ),
    ] {
        let mut b = Builder::new(types, generics);
        let representation = b.ty(source, PassingMode::Take);
        let owner = TypeDefinition {
            loop_source: None,
            format: None,
            generics: b.generics.clone(),
            representation: Some(representation),
            bounds: vec![],
            authority: AuthoritySummary::Known(
                vec![
                    AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(0))),
                    AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(1))),
                ]
                .into(),
            ),
            capability: None,
            implementation_authority: None,
            serializable: false,
            serializable_check: None,
            layout: Layout::default(),
            distinct: false,
            resource: true,
            drop: None,
            fact: None,
            captures: None,
            destructor_allocation: Allocation::NoAlloc,
        };
        out.push(b.signature(cursor, SignatureKind::Type(Box::new(owner)), true));
        for (suffix, parameters, result) in [
            ("len", format!("source: {source}"), "u64".to_owned()),
            (
                "open",
                format!("take source: {source} in source"),
                format!("{cursor}[K,V] in source uses {{source}}"),
            ),
            (
                "next",
                format!("mut cursor: {cursor}[K,V]"),
                "Option[(K,V)] in cursor uses {cursor}".to_owned(),
            ),
        ] {
            let mut b = Builder::new(types, generics);
            let mut f = b.function(&parameters, &result, true);
            if suffix == "len" {
                f.uses = Uses::Only(AuthoritySummary::inert());
            }
            f.authority = if suffix == "len" {
                AuthoritySummary::Known(vec![].into())
            } else {
                AuthoritySummary::Known(
                    vec![AuthorityTerm::Parameter(ParameterRef::bound(ParamId(0)))].into(),
                )
            };
            out.push(b.signature(
                &format!("$intrinsic.{protocol}_{suffix}"),
                SignatureKind::Function(f),
                false,
            ));
        }
    }
    let mut b = Builder::new(types, "K: key,V");
    let mut f = b.function("source: Map[K,V], index: u64", "u64", true);
    f.uses = Uses::Only(AuthoritySummary::inert());
    f.authority = AuthoritySummary::inert();
    out.push(b.signature(
        "$intrinsic.MapInsertionOrder_advance",
        SignatureKind::Function(f),
        false,
    ));
    for signature in &mut out {
        signature.metadata.visibility = if *signature.definition.path()
            == hirsel_syntax::DefPath::core("$intrinsic.ViewCursor")
        {
            Visibility::PublicOpaque
        } else {
            Visibility::Private
        };
    }
    out
}
