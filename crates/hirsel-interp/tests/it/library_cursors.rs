use crate::support;
use hirsel_interp::{
    library::{collection, *},
    *,
};
use hirsel_tir::*;
use hirsel_types::*;
use support::ty;
struct Harness {
    library: Library,
    run: RunConfiguration,
    types: TypeStore,
    sources: hirsel_span::SourceMap,
    stdout: Vec<u8>,
    trace: Vec<TraceEvent>,
}
impl Harness {
    fn new() -> Self {
        let run = RunConfiguration {
            entry: support::path("main"),
            hash_seed: 42,
            inputs: vec![],
        };
        Self {
            library: Library::new(&run, Default::default()).unwrap(),
            run,
            types: TypeStore::default(),
            sources: support::sources(),
            stdout: vec![],
            trace: vec![],
        }
    }
    fn call(&mut self, name: &str, args: &[Value]) -> Result<OperationResult, Error> {
        let result_type = self.types.push(ty(TypeKind::Tuple(vec![].into())));
        self.library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &hirsel_syntax::DefPath::core(name),
                    arguments: &mut args.to_vec(),
                    generics: &[],
                    result_type,
                    types: &self.types,
                    site: support::site(7),
                },
                &mut OperationContext {
                    trace_mode: Default::default(),
                    anchor: None,
                    configuration: &self.run,
                    sources: &self.sources,
                    stdout: &mut self.stdout,
                    trace: &mut self.trace,
                },
            )?
            .ok_or_else(|| Error::InvalidTir("unhandled fixture".into()))
    }
    fn value(&mut self, name: &str, args: &[Value]) -> Value {
        let OperationResult::Complete { value: v, .. } = self.call(name, args).unwrap() else {
            panic!()
        };
        v
    }
    fn values(&mut self, protocol: &str, source: Value) -> Vec<Value> {
        let mut cursor = self.value(&format!("$intrinsic.{protocol}_open"), &[source]);
        let mut out = vec![];
        for _ in 0..100 {
            let OperationResult::Complete {
                value, arguments, ..
            } = self
                .call(&format!("$intrinsic.{protocol}_next"), &[cursor])
                .unwrap()
            else {
                panic!()
            };
            cursor = arguments[0].1.clone();
            match value {
                Value::Variant { variant: 0, fields } => {
                    assert!(fields.is_empty());
                    let again = self
                        .call(&format!("$intrinsic.{protocol}_next"), &[cursor.clone()])
                        .unwrap();
                    assert_eq!(
                        again,
                        OperationResult::Complete {
                            consumed: vec![],
                            released: vec![],
                            value: none(),
                            arguments: vec![(0, cursor)]
                        }
                    );
                    return out;
                }
                Value::Variant {
                    variant: 1,
                    mut fields,
                } => out.push(fields.pop().unwrap().unwrap()),
                _ => panic!(),
            }
        }
        panic!("cursor did not end")
    }
}
#[test]
fn unicode_view_protocols_keep_scalars_and_positions() {
    let mut h = Harness::new();
    let source = "a\u{301}👩\u{200d}🚀🇩🇪";
    let input = [Value::String(source.into())];
    let scalars = h.value("string.scalars", &input);
    assert_eq!(
        h.values("UnicodeScalars", scalars),
        source.chars().map(Value::Char).collect::<Vec<_>>()
    );
    let positions = h.value("string.positions", &input);
    let values = h.values("ScalarPositions", positions);
    let indices = values
        .iter()
        .map(|v| {
            let Value::Record { definition, fields } = v else {
                panic!()
            };
            assert_eq!(*definition, hirsel_syntax::DefPath::core("StrPos"));
            assert_eq!(fields[0], Some(Value::String(source.into())));
            fields[1].clone().unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        indices,
        source
            .char_indices()
            .map(|(i, _)| u64_value(i as u64))
            .chain([u64_value(source.len() as u64)])
            .collect::<Vec<_>>()
    );
    for (function, protocol, expected) in [
        ("string.scalars", "UnicodeScalars", 0),
        ("string.positions", "ScalarPositions", 1),
    ] {
        let view = h.value(function, &[Value::String(String::new())]);
        assert_eq!(h.values(protocol, view).len(), expected);
    }
}
#[test]
fn mismatched_cursor_protocols_and_malformed_sources_are_tool_errors() {
    let mut h = Harness::new();
    let cursor = h.value("$intrinsic.MapInsertionOrder_open", &[Value::map(vec![])]);
    assert!(matches!(
        h.call("$intrinsic.SortedMapOrder_next", &[cursor]),
        Err(Error::InvalidTir(_))
    ));
    assert!(matches!(
        h.call(
            "$intrinsic.UnicodeScalars_open",
            &[collection::sequence(
                BuiltinType::Array,
                [u32_value(0xd800)]
            )]
        ),
        Err(Error::InvalidTir(_))
    ));
    assert!(matches!(
        h.call(
            "$intrinsic.MapInsertionOrder_open",
            &[Value::map(vec![None])]
        ),
        Err(Error::InvalidTir(_))
    ));
}
