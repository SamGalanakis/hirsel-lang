//! An iterator primitive and its user callback consume separate activations;
//! the compiler's iterator thunk consumes none.
use hirsel_conformance::native;
use hirsel_driver::native::{Configuration, Engine, Options, Outcome, TargetSpec, Toolchain};

#[test]
fn iterator_dispatch_preserves_the_exact_activation_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
        let mut matrix = native::cranelift::matrix(options.target.clone());
        matrix.insert(
            0,
            Configuration {
                options,
                engine: Engine::Heap { campaign: false },
            },
        );
        let tools = Toolchain::for_matrix(&native::runtime_cache().unwrap(), &matrix).unwrap();
        let scratch = native::scratch::RunRoot::create_in(
            &std::env::temp_dir().join("hirsel-iterator-depth-47"),
        )
        .unwrap();
        for (name, depth, rules) in [
            ("exact", 1020u64, vec![]),
            ("over", 1021u64, vec!["LANG-10"]),
        ] {
            let source = format!(
                r#"
fn step(mut state:u64)->Option[u64] {{state=state+1u64;Some(state)}}
fn dive(n:u64,mut it:Iter[u64])->u64 {{
 if n==0u64 {{match iter.next(mut it) {{Some(value)=>value,None()=>0u64}}}}
 else {{1u64+dive(n-1u64,mut it)}}
}}
pub fn main()->Result[(),()] {{
 let mut it:Iter[u64]=iter.from(0u64,step)
 let value=dive({depth}u64,mut it)
 failure.check(value=={expected}u64,TextInvariant())
 Ok(())
}}
"#,
                expected = depth + 1,
            );
            let result =
                native::run_source(name, &source, &matrix, &tools, &scratch.join(name).unwrap());
            assert!(result.snapshot.is_none(), "{name}: {:?}", result.snapshot);
            let reference = result
                .reference
                .as_ref()
                .expect("reference execution is required");
            assert_eq!(
                reference
                    .failures
                    .iter()
                    .map(|failure| failure.rule.as_str())
                    .collect::<Vec<_>>(),
                rules
            );
            assert_eq!(result.outcomes.len(), 3);
            for (configuration, _, outcome) in &result.outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}/{configuration}: {outcome:?}"
                );
            }
        }
    });
}
