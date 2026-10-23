//! Valid/invalid generic neighbors adapt rust-analyzer's parser tests.rs:61–130,
//! top_entries.rs:1–47, prefix_entries.rs:1–89 and generic_args.rs:1–110
//! (3131751e). Hirsel's GEN-1, rather than Rust trait syntax, defines the repairs.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str) -> (QueryEngine, CheckOutput) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("generic.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    (engine, output)
}

fn reject(source: &str, token: &str, message: &str) -> CheckOutput {
    let (_, output) = check(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{source}: {:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "GEN-1");
    assert_eq!(d.message, message);
    assert_eq!(output.sources.slice(d.primary), Some(token));
    let at = output.sources.resolve(d.primary);
    let declaration = source.split("\npub").next().unwrap();
    let start = declaration.rfind(token).unwrap() as u32;
    assert_eq!((at.start, at.end), (start, start + token.len() as u32));
    let json = d.to_json(&output.sources);
    let human = d.render_human(&output.sources);
    for note in json["notes"].as_array().unwrap() {
        assert!(human.contains(note.as_str().unwrap()), "{human}");
    }
    assert!(!human.contains("before copying `T`"), "{human}");
    output
}

#[test]
fn generic_kind_22_bound_spelling_and_prefix_neighbors() {
    with_compiler_stack(|| {
        for (bound, suggestion) in [
            ("nonsense", None),
            // Equally close to plain and flat: list the language names, do not guess.
            ("fplait", None),
            ("plian", Some("plain")),
            ("copyble", Some("copyable")),
            ("sendabel", Some("sendable")),
        ] {
            for prefix in ["", "// café 🐑\ntype Padding { value: u8 }\n"] {
                let source =
                    format!("{prefix}fn probe[T: {bound}](x: T) {{}}\npub fn main() {{}}\n");
                let output = reject(&source, bound, "expected structural bound");
                let d = &output.diagnostics[0];
                assert!(d.notes.iter().any(|n| n == "help: use a supported structural bound: `plain`, `flat`, `data`, `key`, `sendable`, `inert`, `copyable`, `noalloc_drop`, `serializable`; or use `[T]` if no bound is required"));
                assert_eq!(d.fix.as_ref().map(|f| f.replacement.as_str()), suggestion);
                assert_eq!(
                    d.notes
                        .iter()
                        .filter(|n| n.starts_with("help: did you mean"))
                        .count(),
                    usize::from(suggestion.is_some())
                );
                if let Some(fix) = &d.fix {
                    assert_eq!(fix.span, d.primary);
                    let repaired = source.replacen(bound, &fix.replacement, 1);
                    assert!(check(&repaired).1.diagnostics.is_empty(), "{repaired}");
                }
            }
        }
        for bound in [
            "plain",
            "flat",
            "data",
            "key",
            "sendable",
            "inert",
            "copyable",
            "noalloc_drop",
            "serializable",
        ] {
            let source = format!("fn probe[T: {bound}](x: T) {{}}\npub fn main() {{}}");
            assert!(check(&source).1.diagnostics.is_empty(), "{source}");
        }
    });
}

#[test]
fn generic_kind_22_size_in_type_keeps_exact_span_and_kind_help() {
    with_compiler_stack(|| {
        for source in [
            "fn probe[n:size](x:n) {}\npub fn main() {}\n",
            "// café 🐑\nfn probe[n:size](x:(u8, n)) {}\npub fn main() {}\n",
            "fn probe[length:size](x:(u8, length)) {}\npub fn main() {}\n",
        ] {
            let name = if source.contains("length") {
                "length"
            } else {
                "n"
            };
            let output = reject(source, name, "size parameter cannot be used as a type");
            let d = &output.diagnostics[0];
            assert!(d.fix.is_none());
            assert_eq!(
                d.notes,
                [format!(
                    "help: `{name}` is a size parameter; use it in a size argument, for example `Inline[u8, {name}]`. A type position needs a type parameter: declare a separate `T` with `[T, {name}: size]` and use `T` here, or change the binder to `[{name}]` if it was intended as a type parameter"
                )]
            );
        }
        // A direct generic argument is classified before builtin arity/kind checking.
        // This distinct existing producer cites the application, not the size token.
        let source = "fn probe[length:size](x:List[length]) {}\npub fn main() {}\n";
        let (_, output) = check(source);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "GEN-1");
        assert_eq!(d.message, "wrong builtin type/size arguments");
        assert_eq!(output.sources.slice(d.primary), Some("List[length]"));
    });
}

#[test]
fn generic_kind_22_applied_repairs_check_and_validate_reachable_helpers() {
    with_compiler_stack(|| {
        for source in [
            "fn probe[T: plain](x:T) {}\npub fn main() { probe(7u8) }",
            "fn probe[n:size](x:Inline[u8,n]) {}\npub fn main() { let xs: Inline[u8,2] = [1u8,2u8]; probe(xs) }",
            "fn probe[T,n:size](x:T, bytes:Inline[u8,n]) {}\npub fn main() { let xs: Inline[u8,2] = [1u8,2u8]; probe(7u8,xs) }",
            "fn probe[n](x:n) {}\npub fn main() { probe(7u8) }",
            "fn probe[T](x:T) {}\npub fn main() { probe(7u8) }",
        ] {
            let (mut engine, output) = check(source);
            assert!(
                output.diagnostics.is_empty(),
                "{source}: {:?}",
                output.diagnostics
            );
            engine
                .lower_to_tir(&entry_path(&default_context()))
                .unwrap();
        }
    });
}

#[test]
fn generic_kind_22_real_copy_and_key_obligations_keep_existing_advice() {
    with_compiler_stack(|| {
        for (source, required) in [
            ("fn probe[T](x:T) -> T { copy x }", "copyable"),
            (
                "fn need[T:key](x:T) {}\nfn probe[T](x:T) { need(x) }",
                "key",
            ),
        ] {
            let (_, output) = check(source);
            let [d] = &output.diagnostics[..] else {
                panic!("{:?}", output.diagnostics)
            };
            assert_eq!(d.rule.as_str(), "GEN-1");
            let json = d.to_json(&output.sources);
            assert!(json["notes"].to_string().contains(required), "{json}");
            assert!(
                !json["notes"]
                    .to_string()
                    .contains("supported structural bound")
            );
            assert!(!json["notes"].to_string().contains("is a size parameter"));
            for note in json["notes"].as_array().unwrap() {
                assert!(
                    d.render_human(&output.sources)
                        .contains(note.as_str().unwrap())
                );
            }
        }
    });
}
