//! Nominal alias cleanup equality preserves the actual's declaring binder.
use hirsel_driver::*;
use hirsel_engine::*;

const SHAPES: [(&str, &str, &str); 5] = [
    ("direct", "", "R"),
    ("transparent", "type Alias[T]=T", "Alias[R]"),
    (
        "nominal",
        "type Wrapped[T] { payload:(T,) }\ntype Alias[T]=Wrapped[T]",
        "Alias[R]",
    ),
    (
        "nested",
        "type Wrapped[T] { payload:(T,) }\ntype Alias[T]=Wrapped[Wrapped[T]]",
        "Alias[R]",
    ),
    (
        "swapped",
        "type Pair[A,B] { first:A, second:B }\ntype Alias2[A,B]=Pair[B,A]",
        "Alias2[R,bool]",
    ),
];

fn check(source: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = crate::source_std::engine();
    let file = engine
        .sources
        .add("alias-cleanup.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    (engine, output)
}

#[test]
fn alias_cleanup_equality_24_bounded_callback() {
    with_compiler_stack(|| {
        let mut count = 0;
        for (name, types, result) in SHAPES {
            for bound in ["noalloc_drop", "plain"] {
                let source = format!(
                    "{types}\n@noalloc fn relay[R:{bound}](read:@noalloc fn()->{result} uses {{}}, divisor:u64)->{result} uses {{}}{{let output=read();let _=1u64/divisor;take output}}\npub fn main(){{}}"
                );
                let (mut engine, output) = check(&source);
                assert!(
                    output.diagnostics.is_empty(),
                    "{name}/{bound}: {:?}",
                    output.diagnostics
                );
                // Reach relay explicitly: an empty main alone cannot exercise its V receipt.
                let mut relay = entry_path(&default_context());
                relay.0.name = "relay".into();
                engine.native_input_for(&[relay]).unwrap();
                count += 1;
            }
        }
        assert_eq!(count, 10);
        eprintln!("alias_cleanup_equality_24: {count} source/V bounded controls");
    });
}

#[test]
fn alias_cleanup_equality_24_unbounded_and_unrelated_bound_reject() {
    with_compiler_stack(|| {
        let mut count = 0;
        for (name, types, result) in SHAPES {
            for parameters in ["R", "R,S:noalloc_drop"] {
                let source = format!(
                    "{types}\n@noalloc fn relay[{parameters}](read:@noalloc fn()->{result} uses {{}}, divisor:u64)->{result} uses {{}}{{let output=read();let _=1u64/divisor;take output}}\npub fn main(){{}}"
                );
                let (engine, output) = check(&source);
                let [diagnostic] = &output.diagnostics[..] else {
                    panic!("{name}/{parameters}: {:?}", output.diagnostics);
                };
                assert_eq!(diagnostic.rule.as_str(), "MEM-5");
                assert!(diagnostic.message.contains("owned value"), "{diagnostic:?}");
                assert_eq!(engine.sources.slice(diagnostic.primary), Some("output"));
                count += 1;
            }
        }
        assert_eq!(count, 10);
        eprintln!("alias_cleanup_equality_24: {count} unbounded/unrelated MEM-5 rejections");
    });
}
