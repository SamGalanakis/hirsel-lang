use super::*;

fn row(path: &DefPath, anchor: Anchor) -> RowImage {
    RowImage {
        definition: DefId::intern(path),
        types: true,
        visibility: Visibility::PublicOpaque,
        span: Span {
            anchor,
            start: 1,
            end: 2,
        },
        metadata: DefinitionMetadata {
            public: true,
            alias: false,
        },
        site_spans: vec![
            Span {
                anchor,
                start: 1,
                end: 2,
            },
            Span {
                anchor,
                start: 7,
                end: 8,
            },
        ],
        headers: vec![
            ContentHash::of_bytes(b"first"),
            ContentHash::of_bytes(b"second"),
        ],
        candidates: ContentHash::of_bytes(b"ordered candidates"),
        identity: [1, 2, 3, 4],
    }
}

#[test]
fn image_rows_share_ownership_and_materialize_sites_only_on_demand() {
    crate::with_compiler_stack(|| {
        let path = DefPath::core("compact_image_declaration");
        let id = DefId::intern(&path);
        let anchor = Anchor::fresh();
        let expected = row(&path, anchor);
        let mut image = DeclarationIndex::default();
        image.adopt_row(&path, expected.clone());
        let mut session = DeclarationIndex::default();
        session.adopt_rows(&image);
        assert!(Arc::ptr_eq(&image.rows[&id], &session.rows[&id]));
        assert!(image.rows[&id].sites.get().is_none());
        let frozen = session.frozen();
        assert!(
            image.rows[&id].sites.get().is_none(),
            "freezing must keep compact rows"
        );
        assert_eq!(session.identity(), image.identity());
        assert_eq!(session.candidates(&path), Some(expected.candidates));
        assert_eq!(session.headers(&path), Some(expected.headers.as_slice()));
        assert_eq!(session.paths().collect::<Vec<_>>(), [&path]);
        let sites = frozen.sites(&path);
        assert_eq!(
            sites.iter().map(|s| s.name_span).collect::<Vec<_>>(),
            expected.site_spans
        );
        assert!(sites.iter().all(|s| s.definition == path));
        assert!(Arc::ptr_eq(&sites, image.rows[&id].sites()));
        assert!(Arc::ptr_eq(&sites, &frozen.sites(&path)));
        let missing = DefPath::core("absent_compact_image_declaration");
        assert!(session.candidates(&missing).is_none());
        assert!(frozen.sites(&missing).is_empty());
        let replacement = row(&path, Anchor::fresh());
        session.adopt_row(&path, replacement.clone());
        assert!(!Arc::ptr_eq(&image.rows[&id], &session.rows[&id]));
        assert_eq!(frozen.sites(&path)[0].name_span.anchor, anchor);
        assert_eq!(
            session.frozen().sites(&path)[0].name_span,
            replacement.site_spans[0]
        );
    });
}

#[test]
fn compact_row_roundtrip_preserves_order_visibility_and_exact_anchors() {
    crate::with_compiler_stack(|| {
        let path = DefPath::core("compact_image_roundtrip");
        let id = DefId::intern(&path);
        let anchor = Anchor::fresh();
        let expected = row(&path, anchor);
        let mut writer = hirsel_image::ImageWriter::new(
            ContentHash::of_bytes(b"compact"),
            hirsel_types::TypeInterner::new(None),
        );
        writer.anchors([(anchor, 0, 23)]);
        writer.entry(
            crate::image::ROWS,
            id,
            Box::new(crate::image::Whole(expected.clone())),
        );
        let image = hirsel_image::Image::from_bytes(writer.finish().unwrap().bytes).unwrap();
        let actual: RowImage = image.decode(crate::image::ROWS, 0).unwrap();
        assert_eq!(actual.definition, id);
        assert_eq!(actual.visibility, expected.visibility);
        assert_eq!(actual.headers, expected.headers);
        assert_eq!(actual.candidates, expected.candidates);
        assert_eq!(actual.identity, expected.identity);
        let decoded_anchor = image.anchors()[0].0;
        assert_eq!((image.anchors()[0].1, image.anchors()[0].2), (0, 23));
        for (actual, expected) in actual.site_spans.iter().zip(&expected.site_spans) {
            assert_eq!(actual.anchor, decoded_anchor);
            assert_eq!((actual.start, actual.end), (expected.start, expected.end));
        }
        let actual = Row::from(actual);
        assert!(actual.sites.get().is_none());
        assert_eq!(actual.entry().definition, path);
        assert_eq!(actual.entry().namespace, Namespace::Type);
        assert_eq!(actual.sites().len(), 2);
    });
}
