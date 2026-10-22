//! Authority regression reduced from the std-store compaction callers.
use hirsel_driver::*;
use hirsel_engine::*;

#[test]
fn store_authority_14_mutable_generic_receiver_keeps_its_authority() {
    with_compiler_stack(|| {
        for (state, value, body) in [
            ("Dir", "d", ""),
            ("Sim", "Sim(dir: d)", "type Sim { dir: Dir }"),
        ] {
            for prefix in ["", "touch(mut s); "] {
                let source = format!(
                    r#"
{body}
type Backend[S] {{ run: fn(mut state: S) -> () uses {{state}} }}
@captures(backend)
pub opaque type Store[S]: () uses {{S}} {{ state: S, backend: Backend[S] }}
fn access(mut state: {state}) -> () uses {{state}} {{}}
fn open[S](take state: S, take backend: Backend[S]) -> Store[S] uses {{state, backend}} {{
  Store(state: take state, backend: take backend)
}}
fn touch[S](mut s: Store[S]) -> () uses {{}} {{}}
fn compact[S](mut s: Store[S]) -> () uses {{s}} {{}}
fn example(d: Dir) -> () uses {{d}} {{
  let mut s = open({value}, Backend(run: access))
  {prefix}compact(mut s)
}}
"#
                );
                let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
                let file = engine.sources.add("reduced.hir", source.clone()).unwrap();
                engine.parse_file(file, &default_context()).unwrap();
                let mut path = entry_path(&default_context());
                path.0.name = "example".into();
                let output = engine
                    .mode_definition(&path)
                    .unwrap_or_else(|e| panic!("{source}: {e:?}"));
                assert!(
                    output.diagnostics.is_empty(),
                    "{source}: {:?}",
                    output.diagnostics
                );
            }
        }
    });
}

#[test]
fn store_authority_14_handback_receipt_includes_other_operands() {
    with_compiler_stack(|| {
        for (extra, row, accepted) in [("0u64", "d", true), ("n", "d", false), ("n", "d, n", true)]
        {
            let source = format!(
                r#"
type Backend[S] {{ run: fn(mut state: S) -> () uses {{state}} }}
@captures(backend)
pub opaque type Store[S]: () uses {{S}} {{ state: S, backend: Backend[S] }}
fn access(mut d: Dir) -> () uses {{d}} {{}}
fn open[S](take state: S, take backend: Backend[S]) -> Store[S] uses {{state, backend}} {{
  Store(state: take state, backend: take backend)
}}
fn touch[S, A](mut s: Store[S], extra: A) -> () uses {{}} {{}}
fn compact[S](mut s: Store[S]) -> () uses {{s}} {{}}
fn example(d: Dir, n: Net) -> () uses {{{row}}} {{
  let mut s = open(d, Backend(run: access))
  touch(mut s, {extra})
  compact(mut s)
}}
"#
            );
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("operands.hir", source.clone()).unwrap();
            engine.parse_file(file, &default_context()).unwrap();
            let mut path = entry_path(&default_context());
            path.0.name = "example".into();
            let result = engine.mode_definition(&path);
            if accepted {
                assert!(
                    result
                        .unwrap_or_else(|e| panic!("{source}: {e:?}"))
                        .diagnostics
                        .is_empty()
                );
            } else {
                let Err(QueryError::Diagnostics(diagnostics)) = result else {
                    panic!("{source}: expected rejection")
                };
                assert_eq!(diagnostics.len(), 1, "{source}: {diagnostics:?}");
                assert_eq!(diagnostics[0].rule.as_str(), "CAP-1");
                let at = engine.sources.resolve(diagnostics[0].primary);
                assert_eq!(
                    &source[at.start as usize..at.end as usize],
                    "compact(mut s)"
                );
            }
        }
    });
}

#[test]
fn store_authority_14_backend_cannot_capture_an_unlisted_directory() {
    with_compiler_stack(|| {
        let source = r#"
type Backend[S] { run: fn(mut state: S) -> () uses {state} }
fn access(d: Dir) -> () uses {d} {}
fn example(d: Dir) {
  let backend: Backend[()] = Backend(run: fn(mut state: ()) -> () uses {state} { access(d) })
}
"#;
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("capture.hir", source.into()).unwrap();
        engine.parse_file(file, &default_context()).unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "example".into();
        let Err(QueryError::Diagnostics(diagnostics)) = engine.mode_definition(&path) else {
            panic!("unlisted Dir capture was accepted")
        };
        assert!(
            diagnostics.iter().any(|d| d.rule.as_str() == "CLO-4"),
            "{diagnostics:?}"
        );
    });
}
