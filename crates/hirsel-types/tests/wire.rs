hirsel_guard::install!();

use hirsel_run_config::hex;
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use hirsel_types::serialization::{
    SchemaArgument as A, SchemaId as S, SchemaNode as N, SerializableSchema,
};
use hirsel_types::wire::{Codec, Error, Limits, Value, ValueId as V, ValueNode as D};
use hirsel_types::{BuiltinType as B, Primitive as P, Size};
use serde_json::{Value as Json, json};
use std::process::Command;

struct Case {
    name: &'static str,
    schema: SerializableSchema,
    value: Value,
    payload: &'static str,
}
fn schema(nodes: Vec<N>) -> SerializableSchema {
    SerializableSchema {
        version: 1,
        root: S(0),
        nodes,
    }
}
fn value(nodes: Vec<D>) -> Value {
    Value {
        root: V(nodes.len() - 1),
        nodes,
    }
}
fn one(name: &'static str, p: P, d: D, bytes: &'static str) -> Case {
    Case {
        name,
        schema: schema(vec![N::Scalar(p)]),
        value: value(vec![d]),
        payload: bytes,
    }
}
fn builtin(kind: B, args: Vec<A>) -> N {
    N::Builtin {
        kind,
        arguments: args,
    }
}
fn nominal(name: &str, representation: u32) -> N {
    N::Nominal {
        definition: DefPath(ItemPath {
            package: PackageId {
                name: "core".into(),
                version: "1".into(),
            },
            modules: vec![],
            name: name.into(),
        }),
        representation: S(representation),
    }
}
fn cases() -> Vec<Case> {
    let mut result = vec![
        one("bool-false", P::Bool, D::Bool(false), "00"),
        one("bool-true", P::Bool, D::Bool(true), "01"),
        one("u8", P::U8, D::Unsigned(255), "ff"),
        one("u16", P::U16, D::Unsigned(0x1234), "3412"),
        one("u32", P::U32, D::Unsigned(0x12345678), "78563412"),
        one(
            "u64",
            P::U64,
            D::Unsigned(0x123456789abcdef0),
            "f0debc9a78563412",
        ),
        one(
            "u128",
            P::U128,
            D::Unsigned(u128::MAX),
            "ffffffffffffffffffffffffffffffff",
        ),
        one("i8", P::I8, D::Signed(-128), "80"),
        one("i16", P::I16, D::Signed(-32768), "0080"),
        one("i32", P::I32, D::Signed(-2147483648), "00000080"),
        one(
            "i64",
            P::I64,
            D::Signed(i64::MIN as i128),
            "0000000000000080",
        ),
        one(
            "i128",
            P::I128,
            D::Signed(i128::MIN),
            "00000000000000000000000000000080",
        ),
        one("f32", P::F32, D::F32(1.5f32.to_bits()), "0000c03f"),
        one("f64", P::F64, D::F64(1.5f64.to_bits()), "000000000000f83f"),
        one("f32-nan", P::F32, D::F32(0xff800001), "0000c07f"),
        one(
            "f64-nan",
            P::F64,
            D::F64(0xfff0000000000001),
            "000000000000f87f",
        ),
        one("f32-negative-zero", P::F32, D::F32(0x80000000), "00000080"),
        one(
            "f64-negative-zero",
            P::F64,
            D::F64(0x8000000000000000),
            "0000000000000080",
        ),
        one("f32-infinity", P::F32, D::F32(0x7f800000), "0000807f"),
        one(
            "f64-negative-infinity",
            P::F64,
            D::F64(0xfff0000000000000),
            "000000000000f0ff",
        ),
        one(
            "utf8",
            P::String,
            D::String("é\0😀".into()),
            "0700000000000000c3a900f09f9880",
        ),
        one(
            "bytes",
            P::Bytes,
            D::Bytes(vec![0, 255, 128]),
            "030000000000000000ff80",
        ),
        one(
            "empty-string",
            P::String,
            D::String(String::new()),
            "0000000000000000",
        ),
        Case {
            name: "unit",
            schema: schema(vec![N::Tuple(vec![])]),
            value: value(vec![D::Fields(vec![])]),
            payload: "",
        },
        Case {
            name: "record",
            schema: schema(vec![
                N::Record(vec![
                    ("flag".into(), S(1)),
                    ("word".into(), S(2)),
                    ("text".into(), S(3)),
                ]),
                N::Scalar(P::Bool),
                N::Scalar(P::U16),
                N::Scalar(P::String),
            ]),
            value: value(vec![
                D::Bool(true),
                D::Unsigned(0x1234),
                D::String("x".into()),
                D::Fields(vec![V(0), V(1), V(2)]),
            ]),
            payload: "013412010000000000000078",
        },
        Case {
            name: "tuple",
            schema: schema(vec![
                N::Tuple(vec![S(1), S(2)]),
                N::Scalar(P::I8),
                N::Tuple(vec![]),
            ]),
            value: value(vec![
                D::Signed(-1),
                D::Fields(vec![]),
                D::Fields(vec![V(0), V(1)]),
            ]),
            payload: "ff",
        },
        Case {
            name: "variant",
            schema: schema(vec![
                N::Variant(vec![
                    ("None".into(), vec![]),
                    ("Some".into(), vec![(None, S(1))]),
                ]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![
                D::Unsigned(9),
                D::Variant {
                    ordinal: 1,
                    fields: vec![V(0)],
                },
            ]),
            payload: "0100000009",
        },
        Case {
            name: "box",
            schema: schema(vec![N::Box(S(1)), N::Scalar(P::I8)]),
            value: value(vec![D::Signed(-1), D::Wrapper(V(0))]),
            payload: "ff",
        },
    ];
    for (name, kind) in [
        ("array", B::Array),
        ("list", B::List),
        ("inline", B::Inline),
        ("simd", B::Simd),
    ] {
        let fixed = matches!(kind, B::Inline | B::Simd);
        let mut args = vec![A::Type(S(1))];
        if fixed {
            args.push(A::Size(Size::Constant(2)));
        }
        result.push(Case {
            name,
            schema: schema(vec![builtin(kind, args), N::Scalar(P::U16)]),
            value: value(vec![
                D::Unsigned(1),
                D::Unsigned(258),
                D::Sequence(vec![V(0), V(1)]),
            ]),
            payload: if fixed {
                "01000201"
            } else {
                "020000000000000001000201"
            },
        });
    }
    result.extend([
        Case {
            name: "inline-zero",
            schema: schema(vec![
                builtin(B::Inline, vec![A::Type(S(1)), A::Size(Size::Constant(0))]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![D::Sequence(vec![])]),
            payload: "",
        },
        Case {
            name: "list-unit",
            schema: schema(vec![
                builtin(B::List, vec![A::Type(S(1))]),
                N::Tuple(vec![]),
            ]),
            value: value(vec![D::Fields(vec![]), D::Sequence(vec![V(0), V(0), V(0)])]),
            payload: "0300000000000000",
        },
        Case {
            name: "nested-list",
            schema: schema(vec![
                builtin(B::List, vec![A::Type(S(1))]),
                builtin(B::List, vec![A::Type(S(2))]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![
                D::Unsigned(7),
                D::Sequence(vec![V(0)]),
                D::Sequence(vec![]),
                D::Sequence(vec![V(1), V(2)]),
            ]),
            payload: "02000000000000000100000000000000070000000000000000",
        },
        Case {
            name: "list-string",
            schema: schema(vec![
                builtin(B::List, vec![A::Type(S(1))]),
                N::Scalar(P::String),
            ]),
            value: value(vec![
                D::String("x".into()),
                D::String("yz".into()),
                D::Sequence(vec![V(0), V(1)]),
            ]),
            payload: "02000000000000000100000000000000780200000000000000797a",
        },
        Case {
            name: "packed",
            schema: schema(vec![
                builtin(B::Packed, vec![A::Type(S(1))]),
                N::Variant(vec![
                    ("Empty".into(), vec![]),
                    ("Byte".into(), vec![(None, S(2))]),
                ]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![
                D::Unsigned(4),
                D::Variant {
                    ordinal: 1,
                    fields: vec![V(0)],
                },
                D::Wrapper(V(1)),
            ]),
            payload: "0100000004",
        },
        Case {
            name: "option-none",
            schema: schema(vec![
                nominal("Option", 1),
                N::Variant(vec![
                    ("None".into(), vec![]),
                    ("Some".into(), vec![(None, S(2))]),
                ]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![
                D::Variant {
                    ordinal: 0,
                    fields: vec![],
                },
                D::Wrapper(V(0)),
            ]),
            payload: "00000000",
        },
        Case {
            name: "option-some",
            schema: schema(vec![
                nominal("Option", 1),
                N::Variant(vec![
                    ("None".into(), vec![]),
                    ("Some".into(), vec![(None, S(2))]),
                ]),
                N::Scalar(P::U8),
            ]),
            value: value(vec![
                D::Unsigned(5),
                D::Variant {
                    ordinal: 1,
                    fields: vec![V(0)],
                },
                D::Wrapper(V(1)),
            ]),
            payload: "0100000005",
        },
        Case {
            name: "result-error",
            schema: schema(vec![
                nominal("Result", 1),
                N::Variant(vec![
                    ("Ok".into(), vec![(None, S(2))]),
                    ("Error".into(), vec![(None, S(3))]),
                ]),
                N::Scalar(P::U8),
                N::Scalar(P::String),
            ]),
            value: value(vec![
                D::String("bad".into()),
                D::Variant {
                    ordinal: 1,
                    fields: vec![V(0)],
                },
                D::Wrapper(V(1)),
            ]),
            payload: "010000000300000000000000626164",
        },
        Case {
            name: "result-ok",
            schema: schema(vec![
                nominal("Result", 1),
                N::Variant(vec![
                    ("Ok".into(), vec![(None, S(2))]),
                    ("Error".into(), vec![(None, S(3))]),
                ]),
                N::Scalar(P::U8),
                N::Scalar(P::String),
            ]),
            value: value(vec![
                D::Unsigned(12),
                D::Variant {
                    ordinal: 0,
                    fields: vec![V(0)],
                },
                D::Wrapper(V(1)),
            ]),
            payload: "000000000c",
        },
        Case {
            name: "map-insertion-order",
            schema: schema(vec![
                builtin(B::Map, vec![A::Type(S(1)), A::Type(S(2))]),
                N::Scalar(P::U8),
                N::Scalar(P::Bool),
            ]),
            value: value(vec![
                D::Unsigned(2),
                D::Bool(true),
                D::Unsigned(1),
                D::Bool(false),
                D::Map(vec![(V(0), V(1)), (V(2), V(3))]),
            ]),
            payload: "020000000000000002010100",
        },
        Case {
            name: "set-byte-order",
            schema: schema(vec![
                builtin(B::Set, vec![A::Type(S(1))]),
                N::Scalar(P::U16),
            ]),
            value: value(vec![
                D::Unsigned(1),
                D::Unsigned(256),
                D::Set(vec![V(0), V(1)]),
            ]),
            payload: "020000000000000000010100",
        },
        Case {
            name: "recursive-tree",
            schema: schema(vec![
                N::Variant(vec![
                    ("Leaf".into(), vec![]),
                    ("Node".into(), vec![(None, S(1))]),
                ]),
                N::Box(S(0)),
            ]),
            value: value(vec![
                D::Variant {
                    ordinal: 0,
                    fields: vec![],
                },
                D::Wrapper(V(0)),
                D::Variant {
                    ordinal: 1,
                    fields: vec![V(1)],
                },
            ]),
            payload: "0100000000000000",
        },
    ]);
    result
}
fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn observe(v: &Value, id: V) -> Json {
    match &v.nodes[id.0] {
        D::Bool(b) => json!(["bool", b]),
        D::Unsigned(n) => json!(["u", n.to_string()]),
        D::Signed(n) => json!(["i", n.to_string()]),
        D::F32(b) => json!(["f32", format!("{b:08x}")]),
        D::F64(b) => json!(["f64", format!("{b:016x}")]),
        D::String(s) => json!(["string", s]),
        D::Bytes(b) => json!(["bytes", hex(b)]),
        D::Fields(fs) => json!([
            "fields",
            fs.iter().map(|t| observe(v, *t)).collect::<Vec<_>>()
        ]),
        D::Variant { ordinal, fields } => json!([
            "variant",
            ordinal,
            fields.iter().map(|t| observe(v, *t)).collect::<Vec<_>>()
        ]),
        D::Sequence(fs) => json!(["seq", fs.iter().map(|t| observe(v, *t)).collect::<Vec<_>>()]),
        D::Set(fs) => json!(["set", fs.iter().map(|t| observe(v, *t)).collect::<Vec<_>>()]),
        D::Map(fs) => json!([
            "map",
            fs.iter()
                .map(|(k, t)| json!([observe(v, *k), observe(v, *t)]))
                .collect::<Vec<_>>()
        ]),
        D::Wrapper(t) => json!(["wrap", observe(v, *t)]),
    }
}
fn golden_text() -> String {
    let mut text =
        "# SER-2 v1: name<TAB>descriptor hex<TAB>payload hex<TAB>decoded data JSON\n".to_string();
    for case in cases() {
        let codec = Codec::new(&case.schema).unwrap();
        let bytes = codec
            .encode_payload(&case.value, Limits::default())
            .unwrap();
        assert_eq!(hex(&bytes), case.payload, "{}", case.name);
        let decoded = codec.decode_payload(&bytes, Limits::default()).unwrap();
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            case.name,
            hex(codec.descriptor()),
            case.payload,
            observe(&decoded, decoded.root)
        ));
    }
    text
}
#[test]
fn recursive_canonical_golden_vectors() {
    // New SER-2 goldens: `tools/bless.sh wire`.
    if std::env::var_os("HIRSEL_BLESS").is_some() {
        let golden = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wire/golden.tsv");
        std::fs::write(golden, golden_text()).unwrap();
        return;
    }
    assert_eq!(golden_text(), include_str!("wire/golden.tsv"));
    for case in cases() {
        let codec = Codec::new(&case.schema).unwrap();
        let bytes = codec.encode(&case.value, Limits::default()).unwrap();
        let mut expected = b"HSC1".to_vec();
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&(codec.descriptor().len() as u64).to_le_bytes());
        expected.extend_from_slice(codec.descriptor());
        let payload_bytes = unhex(case.payload);
        expected.extend_from_slice(&(payload_bytes.len() as u64).to_le_bytes());
        expected.extend_from_slice(&payload_bytes);
        assert_eq!(bytes, expected, "{} envelope", case.name);
        let decoded = codec.decode(&bytes, Limits::default()).unwrap();
        assert_eq!(
            codec.encode(&decoded, Limits::default()).unwrap(),
            bytes,
            "{}",
            case.name
        );
        let payload = codec.validated_payload(&bytes, Limits::default()).unwrap();
        assert_eq!(hex(payload), case.payload);
        for at in 0..bytes.len() {
            assert!(
                codec.decode(&bytes[..at], Limits::default()).is_err(),
                "{} prefix {at}",
                case.name
            );
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            codec.decode(&trailing, Limits::default()),
            Err(Error::Trailing)
        );
    }
}
#[test]
fn independent_python_decoder_checks_golden_and_malformed_corpus() {
    let status = Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/wire/oracle.py"))
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
fn malformed_payloads_fail_before_publishing_values() {
    for line in include_str!("wire/malformed.tsv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fs: Vec<_> = line.split('\t').collect();
        let case = cases().into_iter().find(|c| c.name == fs[1]).unwrap();
        let codec = Codec::new(&case.schema).unwrap();
        let bytes = unhex(fs[2]);
        let result = codec.decode_payload(&bytes, Limits::default());
        assert_eq!(format!("{:?}", result.unwrap_err()), fs[3], "{}", fs[0]);
        let mut envelope = codec.encode(&case.value, Limits::default()).unwrap();
        let payload_at = 24 + codec.descriptor().len();
        envelope.truncate(payload_at - 8);
        envelope.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        envelope.extend_from_slice(&bytes);
        assert!(
            codec
                .validated_payload(&envelope, Limits::default())
                .is_err(),
            "{}",
            fs[0]
        );
    }
}
#[test]
fn schema_mismatch_and_budgets_are_distinct_refusals() {
    let a = Codec::new(&schema(vec![N::Scalar(P::U8)])).unwrap();
    let b = Codec::new(&schema(vec![N::Scalar(P::Bool)])).unwrap();
    let bytes = a
        .encode(&value(vec![D::Unsigned(1)]), Limits::default())
        .unwrap();
    assert_eq!(
        b.decode(&bytes, Limits::default()),
        Err(Error::SchemaMismatch)
    );
    let mut version = bytes.clone();
    version[4] = 2;
    assert_eq!(a.decode(&version, Limits::default()), Err(Error::Version));
    let small = Limits {
        bytes: bytes.len() - 1,
        ..Limits::default()
    };
    assert_eq!(a.decode(&bytes, small), Err(Error::Limit));
    let unit_list = Codec::new(&schema(vec![
        builtin(B::List, vec![A::Type(S(1))]),
        N::Tuple(vec![]),
    ]))
    .unwrap();
    assert_eq!(
        unit_list.decode_payload(
            &100u64.to_le_bytes(),
            Limits {
                values: 5,
                ..Limits::default()
            }
        ),
        Err(Error::Limit)
    );
    // Cyclic in-memory values and zero-byte schema recursion cannot hang.
    let cycle = Codec::new(&schema(vec![N::Box(S(0))])).unwrap();
    let limits = Limits {
        values: 10,
        work: 100,
        ..Limits::default()
    };
    assert_eq!(
        cycle.encode_payload(&value(vec![D::Wrapper(V(0))]), limits),
        Err(Error::Limit)
    );
    assert_eq!(cycle.decode_payload(&[], limits), Err(Error::Limit));
}
#[test]
fn checked_flat_fast_path_is_exact_and_conservative() {
    for case in cases() {
        let codec = Codec::new(&case.schema).unwrap();
        if let Some(width) = codec.fixed_flat_extent() {
            assert_eq!(width, unhex(case.payload).len());
            assert_eq!(
                codec
                    .encode_fixed_flat(&unhex(case.payload), Limits::default())
                    .unwrap(),
                codec.encode(&case.value, Limits::default()).unwrap()
            );
        } else {
            assert_eq!(
                codec.encode_fixed_flat(&unhex(case.payload), Limits::default()),
                Err(Error::Schema)
            );
        }
    }
    let bool_ = Codec::new(&schema(vec![N::Scalar(P::Bool)])).unwrap();
    assert_eq!(
        bool_.encode_fixed_flat(&[2], Limits::default()),
        Err(Error::Scalar)
    );
    let f = Codec::new(&schema(vec![N::Scalar(P::F32)])).unwrap();
    assert_eq!(
        f.encode_fixed_flat(&0xff800001u32.to_le_bytes(), Limits::default()),
        Err(Error::Scalar)
    );
}
#[test]
fn forged_and_symbolic_descriptors_have_no_executable_decoder() {
    let bad = [
        schema(vec![N::Box(S(100))]),
        schema(vec![N::Scalar(P::Never)]),
        schema(vec![
            builtin(B::Shared, vec![A::Type(S(1))]),
            N::Scalar(P::U8),
        ]),
        schema(vec![
            builtin(B::Packed, vec![A::Type(S(1))]),
            N::Scalar(P::U8),
        ]),
        schema(vec![
            builtin(B::Set, vec![A::Type(S(1))]),
            N::Scalar(P::F32),
        ]),
        schema(vec![
            builtin(B::Map, vec![A::Type(S(1)), A::Type(S(2))]),
            N::Box(S(2)),
            N::Scalar(P::F64),
        ]),
        schema(vec![
            builtin(B::Simd, vec![A::Type(S(1)), A::Size(Size::Constant(2))]),
            N::Scalar(P::String),
        ]),
    ];
    for s in bad {
        assert!(matches!(Codec::new(&s), Err(Error::Schema)));
    }
}
#[test]
fn schema_numbering_sharing_and_unreachable_rows_do_not_change_identity() {
    let a = schema(vec![N::Tuple(vec![S(1), S(1)]), N::Scalar(P::U8)]);
    let b = schema(vec![
        N::Tuple(vec![S(2), S(3)]),
        N::Scalar(P::U64),
        N::Scalar(P::U8),
        N::Scalar(P::U8),
    ]);
    assert_eq!(
        Codec::new(&a).unwrap().descriptor(),
        Codec::new(&b).unwrap().descriptor()
    );
}
#[test]
fn deep_recursive_values_walk_and_drop_without_host_recursion() {
    let codec = Codec::new(&schema(vec![
        N::Variant(vec![
            ("End".into(), vec![]),
            ("Next".into(), vec![(None, S(1))]),
        ]),
        N::Box(S(0)),
    ]))
    .unwrap();
    let mut nodes = vec![D::Variant {
        ordinal: 0,
        fields: vec![],
    }];
    for _ in 0..10_000 {
        let t = V(nodes.len() - 1);
        nodes.push(D::Wrapper(t));
        let t = V(nodes.len() - 1);
        nodes.push(D::Variant {
            ordinal: 1,
            fields: vec![t],
        });
    }
    let bytes = codec
        .encode_payload(&value(nodes), Limits::default())
        .unwrap();
    assert_eq!(bytes.len(), 40_004);
    let decoded = codec.decode_payload(&bytes, Limits::default()).unwrap();
    assert_eq!(
        codec.encode_payload(&decoded, Limits::default()).unwrap(),
        bytes
    );
}

#[test]
fn placement_seeds_and_set_insertion_orders_preserve_bytes_but_map_order_matters() {
    use std::hash::{BuildHasher, Hasher};
    struct Seed(u64);
    impl BuildHasher for Seed {
        type Hasher = std::collections::hash_map::DefaultHasher;
        fn build_hasher(&self) -> Self::Hasher {
            let mut h = Self::Hasher::new();
            h.write_u64(self.0);
            h
        }
    }
    let codec = Codec::new(&schema(vec![
        builtin(B::Set, vec![A::Type(S(1))]),
        N::Scalar(P::U16),
    ]))
    .unwrap();
    let mut baseline = None;
    for seed in [0, 1, 7, 19, 0x0123456789abcdef, u64::MAX] {
        let mut table = std::collections::HashMap::with_hasher(Seed(seed));
        for key in [1u16, 256, 2, 255, 65535] {
            table.insert(key, ());
        }
        let mut nodes: Vec<_> = table
            .into_keys()
            .map(|n| D::Unsigned(u128::from(n)))
            .collect();
        let children = (0..nodes.len()).map(V).collect();
        nodes.push(D::Set(children));
        let encoded = codec.encode(&value(nodes), Limits::default()).unwrap();
        if let Some(expected) = &baseline {
            assert_eq!(&encoded, expected);
        } else {
            baseline = Some(encoded);
        }
    }
    let mut case = cases()
        .into_iter()
        .find(|c| c.name == "map-insertion-order")
        .unwrap();
    let codec = Codec::new(&case.schema).unwrap();
    let baseline = codec.encode(&case.value, Limits::default()).unwrap();
    let D::Map(entries) = &mut case.value.nodes[case.value.root.0] else {
        panic!("map")
    };
    entries.reverse();
    assert_ne!(
        codec.encode(&case.value, Limits::default()).unwrap(),
        baseline
    );
}

#[test]
fn arbitrary_bytes_never_publish_noncanonical_values() {
    let schemas = [
        schema(vec![N::Scalar(P::Bool)]),
        schema(vec![N::Scalar(P::I16)]),
        schema(vec![N::Scalar(P::F32)]),
        schema(vec![N::Scalar(P::String)]),
        schema(vec![
            N::Variant(vec![
                ("Empty".into(), vec![]),
                ("Byte".into(), vec![(None, S(1))]),
            ]),
            N::Scalar(P::U8),
        ]),
        schema(vec![
            builtin(B::List, vec![A::Type(S(1))]),
            N::Tuple(vec![]),
        ]),
    ];
    let limits = Limits {
        values: 32,
        work: 256,
        bytes: 256,
    };
    let mut rng = 0x3141592653589793u64;
    for s in schemas {
        let codec = Codec::new(&s).unwrap();
        for n in 0..512 {
            let mut bytes = vec![0; (n % 24) as usize];
            for byte in &mut bytes {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                *byte = rng as u8;
            }
            if let Ok(v) = codec.decode_payload(&bytes, limits) {
                assert_eq!(codec.encode_payload(&v, limits).unwrap(), bytes);
            }
        }
    }
}
