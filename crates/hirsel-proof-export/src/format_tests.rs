//! Encoding identity controls only; no source or execution cells are counted.
use super::*;

#[test]
fn sealed_regions_33_combined_format_refuses_each_previous_identity() {
    assert_eq!(TIR_VERSION, 16);
    assert_eq!(FORMAT, "hirsel.tir.export.v16");
    let environment = Environment {
        interface_version: 1,
        compiler_version: "combined-depth-grant-format-control".into(),
        spec_hash: ContentHash::default(),
    };
    let input = ExportInput {
        types: TypeInterner::new(None),
        header: Header {
            format: FORMAT.into(),
            tir_version: TIR_VERSION,
            environment: environment.clone(),
            entry: DefPath::core("format_control"),
            program_revision: None,
        },
        definitions: BTreeMap::new(),
        signatures: BTreeMap::new(),
        constants: BTreeMap::new(),
        skipped: vec![],
        annotations: vec![],
        locator: Rc::new(|_| Location::unknown()),
    };
    let text = export(&input);
    let accepted = import(&text, &environment, &input.types).unwrap();
    assert_eq!(export(&accepted.to_input()), text);
    let header = format!("(Header \"{FORMAT}\" {TIR_VERSION}\n");
    assert_eq!(text.matches(&header).count(), 1);
    for previous in [11, 12, 13, 14, 15] {
        for (format, version, at) in [
            (
                format!("hirsel.tir.export.v{previous}"),
                TIR_VERSION,
                "header.format",
            ),
            (FORMAT.into(), previous, "header.tir_version"),
            (
                format!("hirsel.tir.export.v{previous}"),
                previous,
                "header.format",
            ),
        ] {
            let changed = text.replacen(&header, &format!("(Header \"{format}\" {version}\n"), 1);
            let refused = import(&changed, &environment, &input.types).unwrap_err();
            assert_eq!(refused.at, at, "{refused}");
        }
    }
}

#[test]
fn sealed_regions_33_combined_format_preserves_the_optional_grant_encoding() {
    let types = TypeInterner::new(None);
    let locator: Locator = Rc::new(|_| Location::unknown());
    for grant in [
        None,
        Some(hirsel_tir::ArenaScopeGrant {
            intrinsic: DefPath::core("arena.bounded"),
            callback_parameter: hirsel_types::ParamId(1),
            arena_parameter: hirsel_types::ParamId(0),
        }),
    ] {
        let mut writer = Writer::with_context(Cx::new(locator.clone(), types.clone()));
        grant.write(&mut writer);
        let (text, _) = writer.finish();
        let mut reader = Reader::with_context(&text, Cx::new(locator.clone(), types.clone()));
        let decoded = Option::<hirsel_tir::ArenaScopeGrant>::read(&mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(decoded, grant);
    }
}

#[test]
fn bounded_export_output_includes_digest_and_refuses_partial_text() {
    let types = TypeInterner::new(None);
    let locator: Locator = Rc::new(|_| Location::unknown());
    for name in ["plain", "quoted\"\\\nλ🐑"] {
        let input = ExportInput {
            types: types.clone(),
            header: Header {
                format: FORMAT.into(),
                tir_version: TIR_VERSION,
                environment: Environment {
                    interface_version: 1,
                    compiler_version: name.into(),
                    spec_hash: ContentHash::default(),
                },
                entry: DefPath::core(name),
                program_revision: None,
            },
            definitions: BTreeMap::new(),
            signatures: BTreeMap::new(),
            constants: BTreeMap::from([(
                DefPath::core("unit"),
                Materialized {
                    root: hirsel_tir::ValueNode {
                        ty: types.intern(hirsel_types::Type::unit()),
                        kind: hirsel_tir::ValueKind::Construct {
                            constructor: hirsel_tir::Constructor::Tuple,
                            fields: vec![],
                        },
                    },
                },
            )]),
            skipped: vec![],
            annotations: vec![],
            locator: locator.clone(),
        };
        let built = build(&input);
        let expected = encode(&built, &locator);
        assert_eq!(encoded_bytes(&built, &locator), Some(expected.len()));
        assert_eq!(
            encode_with_output_limit(&built, &locator, expected.len()),
            Some(expected.clone())
        );
        for limit in [0, encode_body(&built, &locator).len(), expected.len() - 1] {
            assert!(encode_with_output_limit(&built, &locator, limit).is_none());
        }
        assert_eq!(
            encode_with_output_limit(&built, &locator, expected.len() + 1),
            Some(expected)
        );
    }
}

#[test]
fn private_uniqueness_rows_round_trip_without_entering_callable_types() {
    use hirsel_types::{ParamId, UniqueResultRow, UniqueSummary};
    let types = TypeInterner::new(None);
    let locator: Locator = Rc::new(|_| Location::unknown());
    let summary = UniqueSummary {
        nonretaining: vec![ParamId(1)],
        preserves_unique: vec![ParamId(0), ParamId(2)],
        results: vec![
            UniqueResultRow::Root,
            UniqueResultRow::Tuple(1),
            UniqueResultRow::Payload {
                variant: 0,
                field: 0,
            },
            UniqueResultRow::Payload {
                variant: 1,
                field: 0,
            },
        ],
    };
    let mut writer = Writer::with_context(Cx::new(locator.clone(), types.clone()));
    summary.write(&mut writer);
    let (text, _) = writer.finish();
    let mut reader = Reader::with_context(&text, Cx::new(locator, types));
    assert_eq!(UniqueSummary::read(&mut reader).unwrap(), summary);
    reader.finish().unwrap();
}
