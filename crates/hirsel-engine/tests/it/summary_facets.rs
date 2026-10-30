use hirsel_engine::*;
use hirsel_span::Span;
use hirsel_syntax::{DefId, DefPath, LexicalOwner};
use hirsel_types::{GenericArgument, NameSite, Primitive, Type, TypeId, TypeInterner, TypeKind};

fn summary(ty: TypeId) -> CheckedSummary {
    let owner = DefId::intern(&DefPath::core("facet-owner"));
    let binder = LexicalOwner::first(owner.path()).site(NameSite::Item);
    CheckedSummary {
        summary: DefinitionSummary {
            owner,
            frames: vec![Frame {
                site: NameSite::Item,
                parent: None,
                kind: FrameKind::Item,
                generic: false,
                size_bindings: Box::new([]),
            }]
            .into(),
            calls: vec![SummaryCall {
                callee: owner,
                binder,
                arguments: vec![GenericArgument::Type(ty)].into(),
                owning_frame: 0,
                target_frame: None,
                class: CallClass::Runtime,
                site: Span::default(),
            }]
            .into(),
            staged: [].into(),
        },
        growth: ParameterGrowth::default(),
    }
}

#[test]
fn independently_allocated_summary_projection_excludes_only_diagnostic_positions() {
    let types = TypeInterner::new(None);
    let ty = types.intern(Type::unit());
    let a = summary(ty);
    let mut b = summary(ty);
    b.summary.calls[0].site = Span {
        start: 10,
        end: 20,
        ..Span::default()
    };
    assert!(SummaryView::from(&a).equivalent(&types, SummaryView::from(&b), &types));
    assert_ne!(a.summary.calls[0].site, b.summary.calls[0].site);
    for field in 0..9 {
        let mut b = b.clone();
        match field {
            0 => b.summary.owner = DefId::intern(&DefPath::core("different")),
            1 => b.summary.frames[0].generic = true,
            2 => b.summary.frames[0].parent = Some(0),
            3 => b.summary.calls[0].callee = DefId::intern(&DefPath::core("different")),
            4 => b.summary.calls[0].class = CallClass::StaticSelection,
            5 => b.summary.calls[0].target_frame = Some(0),
            6 => b.summary.calls[0].owning_frame = 1,
            7 => b.summary.calls[0].arguments = Box::new([]),
            8 => b.summary.calls = [].into(),
            _ => unreachable!(),
        }
        assert!(
            !SummaryView::from(&a).equivalent(&types, SummaryView::from(&b), &types),
            "field {field}"
        );
    }
    let mut a_view = SummaryView::from(&a);
    a_view.checked = Some(&a);
    let mut b_view = SummaryView::from(&a);
    b_view.checked = Some(&b);
    assert!(a_view.equivalent(&types, b_view, &types));
    b.summary.frames[0].generic = true;
    let mut b_view = SummaryView::from(&a);
    b_view.checked = Some(&b);
    assert!(!a_view.equivalent(&types, b_view, &types));
    assert!(!a_view.equivalent(&types, SummaryView::from(&a), &types));
}

#[test]
fn foreign_pools_never_compare_numeric_type_slots_without_their_rows() {
    let a = TypeInterner::new(None);
    let b = TypeInterner::new(None);
    let unit = a.intern(Type::unit());
    let byte = b.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
    assert_eq!(
        unit, byte,
        "same slot in different pools is deliberately misleading"
    );
    let a_summary = summary(unit);
    let byte_summary = summary(byte);
    assert!(!SummaryView::from(&a_summary).equivalent(&a, SummaryView::from(&byte_summary), &b));
    let other_unit = b.intern(Type::unit());
    assert_ne!(unit, other_unit);
    assert!(SummaryView::from(&a_summary).equivalent(
        &a,
        SummaryView::from(&summary(other_unit)),
        &b
    ));
}

#[test]
fn recursive_component_edge_split_and_merge_changes_the_exact_summary() {
    let a = TypeInterner::new(None);
    let b = TypeInterner::new(None);
    let cycle = a.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))]);
    let same = b.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))]);
    let split = b.intern_graph(2, |ids| {
        vec![
            Type::with_kind(TypeKind::Box(ids[1])),
            Type::with_kind(TypeKind::Box(ids[0])),
        ]
    });
    let a_summary = summary(cycle[0]);
    assert!(SummaryView::from(&a_summary).equivalent(&a, SummaryView::from(&summary(same[0])), &b));
    assert!(!SummaryView::from(&a_summary).equivalent(
        &a,
        SummaryView::from(&summary(split[0])),
        &b
    ));
    assert!(SummaryView::from(&a_summary).equivalent(&a, SummaryView::from(&summary(same[0])), &b));
}

#[test]
fn explicit_portable_summary_projection_preserves_the_existing_byte_contract() {
    let types = TypeInterner::new(None);
    let unit = types.intern(Type::unit());
    let mut checked = summary(unit);
    checked.summary.calls[0].site = Span {
        start: 3,
        end: 7,
        ..Span::default()
    };
    let mut normalized = checked.clone();
    normalized.summary.calls[0].site = Span::default();
    let view = SummaryView {
        summary: &checked.summary,
        growth: &checked.growth,
        checked: Some(&checked),
    };
    let old = (
        (&normalized.summary, &normalized.growth),
        Some((&normalized.summary, &normalized.growth)),
    );
    assert_eq!(
        hirsel_product_codec::canonical_bytes(types.clone(), &view, |_| None),
        hirsel_product_codec::canonical_bytes(types, &old, |_| None),
    );
}

#[test]
fn same_pool_handle_shortcuts_cannot_hide_shared_recursive_root_split() {
    let types = TypeInterner::new(None);
    let a = types.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))])[0];
    let b = types.intern_graph(1, |ids| vec![Type::with_kind(TypeKind::Box(ids[0]))])[0];
    assert_ne!(a, b);
    assert!(SummaryView::from(&summary(a)).equivalent(
        &types,
        SummaryView::from(&summary(b)),
        &types
    ));
    let mut shared = summary(a);
    shared.summary.calls[0].arguments =
        vec![GenericArgument::Type(a), GenericArgument::Type(a)].into();
    let mut split = summary(a);
    split.summary.calls[0].arguments =
        vec![GenericArgument::Type(a), GenericArgument::Type(b)].into();
    let shared_view = SummaryView::from(&shared);
    let split_view = SummaryView::from(&split);
    assert!(!shared_view.equivalent(&types, split_view, &types));
    assert_ne!(
        hirsel_product_codec::canonical_bytes(types.clone(), &shared_view, |_| None),
        hirsel_product_codec::canonical_bytes(types, &split_view, |_| None),
    );
}
