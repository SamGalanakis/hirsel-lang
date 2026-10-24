//! An append producer must finish before the caller's list changes, even
//! when a source call inside the producer fails at the exact depth boundary.
use hirsel_conformance::native;
use hirsel_driver::native::{Configuration, Engine, Options, Outcome, TargetSpec, Toolchain};

#[test]
fn append_depth_failure_preserves_the_callers_cleanup_observation() {
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
            &std::env::temp_dir().join("hirsel-append-depth-47"),
        )
        .unwrap();
        for (name, depth, rules, output) in [
            ("exact", 1019u64, vec![], b"full\n".as_slice()),
            ("over", 1020u64, vec!["LANG-10"], b"empty\n".as_slice()),
        ] {
            let source = format!(
                r#"
resource type Trace {{stdout:Stdout,data:List[u8]}}
drop(x) {{
 let text=if list.len(x.data)==0u64 {{"empty\n"}} else {{"full\n"}}
 let _=out.print(x.stdout,text)
}}
fn byte()->u8 {{41u8}}
fn leaf()->u8 {{byte()}}
fn make()->Bytes {{
 let mut out:List[u8]=[]
 list.push(mut out,40u8)
 list.push(mut out,leaf())
 bytes.from_list(take out)
}}
fn append(mut out:List[u8],data:Bytes) {{
 for i in 0u64..bytes.len(data) {{list.push(mut out,data[i])}}
}}
fn dive(n:u64,mut out:List[u8])->u64 {{
 if n==0u64 {{append(mut out,make());0u64}}
 else {{1u64+dive(n-1u64,mut out)}}
}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let mut trace=Trace(stdout:io.stdout(rt)?,data:[])
 let _=dive({depth}u64,mut trace.data)
 Ok(())
}}
"#
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
                    .map(|f| f.rule.as_str())
                    .collect::<Vec<_>>(),
                rules
            );
            assert_eq!(reference.output, output);
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
