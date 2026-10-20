//! Cross-engine boundary: the corpus cannot invoke the Rust codec directly.
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    DefinitionQueries, QueryEngine, RunConfiguration, check_progress, default_context,
    default_environment, entry_path, with_compiler_stack,
};
use hirsel_interp::{Outcome, Value, library::Library};
use hirsel_types::{
    GenericDeclarations, SignatureKind,
    serialization::serializable_schema,
    wire::{Codec, Limits},
};

#[test]
fn rust_vectors_cross_source_in_both_directions() {
    with_compiler_stack(|| {
        for (ty, expr, payload) in [
            ("bool", "true", vec![1]),
            ("i32", "-2147483648i32", i32::MIN.to_le_bytes().to_vec()),
            (
                "u128",
                "340282366920938463463374607431768211455u128",
                u128::MAX.to_le_bytes().to_vec(),
            ),
            (
                "String",
                "\"a\\0é\"",
                [4u64.to_le_bytes().as_slice(), b"a\0\xc3\xa9"].concat(),
            ),
            ("(u8, bool)", "(42u8, true)", vec![42, 1]),
            ("Row", "Row(x:42, y:true)", vec![42, 1]),
            (
                "Choice",
                "Item(42)",
                [1u32.to_le_bytes().as_slice(), &[42]].concat(),
            ),
            (
                "List[u16]",
                "[1u16, 4660u16]",
                [2u64.to_le_bytes().as_slice(), &[1, 0, 0x34, 0x12]].concat(),
            ),
        ] {
            let source = format!(
                "@serializable\npub type Row {{x:u8,y:bool}}\n@serializable\npub type Choice=Empty()|Item(u8)\npub fn value()->{ty}{{{expr}}}\nfn encode_value[T:serializable](x:T)->Bytes{{let thunk=fn(){{codec.encode(x)}};thunk()}}\npub fn encode()->Bytes{{encode_value(value())}}\npub fn decode(data:Bytes)->Result[Bytes,DecodeError]{{let decoded:{ty}=codec.decode(data)?;Ok(codec.encode(decoded))}}\n"
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("codec.hir", source.clone()).unwrap();
            let checked = check_progress(&mut engine, file, &context).unwrap();
            assert!(
                checked.diagnostics.is_empty(),
                "{source}\n{:?}",
                checked.diagnostics
            );
            let mut path = entry_path(&context);
            path.0.name = "value".into();
            let signature = engine.item_signature(&path).unwrap().value;
            let SignatureKind::Function(function) = &signature.kind else {
                unreachable!()
            };
            let schema = serializable_schema(
                &engine.types().clone(),
                function.result,
                &GenericDeclarations::default(),
                &mut |d| engine.item_signature(d.path()).map(|s| Some(s.value)),
            )
            .unwrap()
            .unwrap();
            let codec = Codec::new(&schema).unwrap();
            let runtime = engine.runtime_types().unwrap();
            let sources = engine.sources.clone();
            let mut execute = |name: &str, args| {
                path.0.name = name.into();
                let run = RunConfiguration {
                    entry: path.clone(),
                    hash_seed: 17,
                    inputs: vec![],
                };
                let mut library = Library::new(&run, Default::default()).unwrap();
                let result = hirsel_interp::run_with_runtime(
                    &mut engine,
                    &sources,
                    &run,
                    args,
                    Default::default(),
                    &mut library,
                    &runtime,
                )
                .unwrap();
                match result.outcome {
                    Outcome::Returned { value, .. } => value,
                    other => panic!("{ty}: {other:?}"),
                }
            };
            let Value::Bytes(encoded) = execute("encode", vec![]) else {
                panic!("Bytes expected")
            };
            assert_eq!(
                codec
                    .validated_payload(&encoded, Limits::default())
                    .unwrap(),
                payload,
                "{ty}"
            );
            let decoded = codec.decode(&encoded, Limits::default()).unwrap();
            let rust_encoded = codec.encode(&decoded, Limits::default()).unwrap();
            assert_eq!(encoded, rust_encoded);
            assert_eq!(
                execute("decode", vec![Value::Bytes(rust_encoded)]),
                hirsel_interp::library::ok(Value::Bytes(encoded)),
                "{ty}"
            );
        }
    });
}
