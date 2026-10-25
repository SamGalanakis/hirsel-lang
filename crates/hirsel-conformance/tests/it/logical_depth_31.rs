//! Deterministic invocation depth through the public four-engine adapter, V on.
use hirsel_conformance::native::{self, Class};
use hirsel_driver::native::{Configuration, Engine, Options, Outcome, TargetSpec, Toolchain};

fn matrix() -> Vec<Configuration> {
    let options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    let mut matrix = native::cranelift::matrix(options.target.clone());
    matrix.insert(
        0,
        Configuration {
            options,
            engine: Engine::Heap { campaign: false },
        },
    );
    matrix
}

#[test]
fn p1_recursion_and_self_restart_preserve_the_invocation_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        let matrix = matrix();
        let toolchain = Toolchain::for_matrix(&native::runtime_cache().unwrap(), &matrix).unwrap();
        let scratch = native::scratch::RunRoot::create_in(
            &std::env::temp_dir().join("hirsel-logical-depth-31"),
        )
        .unwrap();
        for (name, source, failure) in [
            (
                "p1-small",
                "fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}\npub fn main()->Result[(), ()] {let _ = f(2u64); Ok(())}",
                false,
            ),
            (
                "p1-exact",
                "fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}\npub fn main()->Result[(), ()] {let _ = f(1022u64); Ok(())}",
                false,
            ),
            (
                "p1-over",
                "fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}\npub fn main()->Result[(), ()] {let _ = f(1023u64); Ok(())}",
                true,
            ),
            (
                "p1-tail",
                "fn f(n:u64)->u64 {if n==0u64 {0u64}else {f(n-1u64)}}\npub fn main()->Result[(), ()] {let _ = f(2048u64); Ok(())}",
                false,
            ),
        ] {
            let result = native::run_source(
                name,
                source,
                &matrix,
                &toolchain,
                &scratch.join(name).unwrap(),
            );
            assert!(result.snapshot.is_none(), "{name}: {:?}", result.snapshot);
            let observation = result
                .reference
                .as_ref()
                .expect("tooling is incomplete, never agreement");
            assert_eq!(
                observation.failures.len(),
                usize::from(failure),
                "{name}: {observation:?}"
            );
            if failure {
                assert_eq!(observation.failures[0].rule, "LANG-10");
            }
            assert_eq!(result.outcomes.len(), 3, "{name}");
            for (configuration, class, outcome) in &result.outcomes {
                assert!(matches!(
                    class,
                    Class::Heap | Class::Llvm | Class::Cranelift
                ));
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}/{configuration}: {outcome:?}"
                );
            }
        }
    });
}

fn agreement(name: &str, source: &str, rules: &[&str]) {
    agreement_output(name, source, rules, None)
}

fn agreement_output(name: &str, source: &str, rules: &[&str], output: Option<&[u8]>) {
    let matrix = matrix();
    let toolchain = Toolchain::for_matrix(&native::runtime_cache().unwrap(), &matrix).unwrap();
    let scratch =
        native::scratch::RunRoot::create_in(&std::env::temp_dir().join("hirsel-logical-depth-31"))
            .unwrap();
    let result = native::run_source(
        name,
        source,
        &matrix,
        &toolchain,
        &scratch.join(name).unwrap(),
    );
    assert!(result.snapshot.is_none(), "{name}: {:?}", result.snapshot);
    let observation = result
        .reference
        .as_ref()
        .expect("tooling is incomplete, never agreement");
    assert_eq!(
        observation
            .failures
            .iter()
            .map(|f| f.rule.as_str())
            .collect::<Vec<_>>(),
        rules,
        "{name}: {observation:?}"
    );
    if let Some(output) = output {
        assert_eq!(observation.output, output, "{name}: cleanup effects");
    }
    assert_eq!(result.outcomes.len(), 3, "{name}");
    for (configuration, _, outcome) in &result.outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{name}/{configuration}: {outcome:?}"
        );
    }
    eprintln!("logical-depth case {name}: all four engines agree");
}

#[test]
fn p2_arena_primitive_and_callback_each_keep_their_invocation() {
    hirsel_engine::with_compiler_stack(|| {
        for (n, rules) in [(1020, &[][..]), (1021, &["LANG-10"][..])] {
            let source = format!(
                "fn f(n:u64)->u64 {{if n==0u64 {{0u64}}else {{1u64+f(n-1u64)}}}}\npub fn main()->Result[(), ()] {{let _ = arena.with(fn(a:Arena)->u64 {{f({n}u64)}}); Ok(())}}"
            );
            agreement(&format!("p2-arena-{n}"), &source, rules);
        }
    });
}

#[test]
fn p2_generic_payload_does_not_change_invocation_cost() {
    hirsel_engine::with_compiler_stack(|| {
        for (n, rules) in [(1022, &[][..]), (1023, &["LANG-10"][..])] {
            let source = format!(
                "type Four {{a:u64,b:u64,c:u64,d:u64}}\nfn f[T](n:u64,x:T)->u64 {{if n==0u64 {{0u64}}else {{1u64+f(n-1u64,x)}}}}\npub fn main()->Result[(), ()] {{let _ = f({n}u64,Four(a:1u64,b:2u64,c:3u64,d:4u64)); Ok(())}}"
            );
            agreement(&format!("p2-generic-{n}"), &source, rules);
        }
    });
}

#[test]
fn p2_indirect_closure_invocations_remain_live() {
    hirsel_engine::with_compiler_stack(|| {
        for (n, rules) in [(511, &[][..]), (512, &["LANG-10"][..])] {
            let source = format!(
                "fn f(n:u64)->u64 {{if n==0u64 {{0u64}}else {{let g=fn()->u64 {{f(n-1u64)}}; 1u64+g()}}}}\npub fn main()->Result[(), ()] {{let _ = f({n}u64); Ok(())}}"
            );
            agreement(&format!("p2-indirect-{n}"), &source, rules);
        }
    });
}

const P4_HEADER: &str = r#"
fn f(n:u64)->u64 { if n==0u64 {0u64} else {1u64+f(n-1u64)} }
resource type R {n:u64}
drop(x) {let _ = f(x.n)}
resource type Outer {inner:R}
drop(x) {()}
"#;

#[test]
fn p4_finalizer_has_an_independent_exact_cleanup_allowance() {
    hirsel_engine::with_compiler_stack(|| {
        for (n, rules) in [(62, &[][..]), (63, &["LANG-10"][..])] {
            let source =
                format!("{P4_HEADER}\npub fn main()->Result[(), ()] {{let r=R(n:{n}u64);Ok(())}}");
            agreement(&format!("p4-cleanup-{n}"), &source, rules);
        }
    });
}

#[test]
fn p4_outer_finalizer_stays_live_through_nested_inline_fields() {
    hirsel_engine::with_compiler_stack(|| {
        let source = format!(
            "{P4_HEADER}\npub fn main()->Result[(), ()] {{let r=Outer(inner:R(n:62u64));Ok(())}}"
        );
        agreement("p4-nested-fields", &source, &["LANG-10"]);
    });
}

#[test]
fn p4_rejected_take_keeps_primary_failure_and_runs_sibling_cleanup() {
    hirsel_engine::with_compiler_stack(|| {
        let source = format!(
            r#"{P4_HEADER}
fn sink(take r:R)->() {{ () }}
fn dive(n:u64)->u64 {{
    if n==0u64 {{ let sibling=R(n:0u64); let r=R(n:63u64); sink(take r); 0u64 }}
    else {{1u64+dive(n-1u64)}}
}}
pub fn main()->Result[(), ()] {{let _=dive(1022u64);Ok(())}}
"#
        );
        agreement("p4-rejected-take", &source, &["LANG-10", "LANG-10"]);
    });
}

#[test]
fn p4_defused_shell_does_not_reenter_the_resource_finalizer() {
    hirsel_engine::with_compiler_stack(|| {
        let source = format!(
            "{P4_HEADER}\npub fn main()->Result[(), ()] {{let r=Outer(inner:R(n:0u64));let Outer(inner:inner)=take r;let R(n:n)=take inner;Ok(())}}"
        );
        agreement("p4-defused-shell", &source, &[]);
    });
}

#[test]
fn p4_rejected_take_preserves_effect_order_and_finalizes_each_owner_once() {
    hirsel_engine::with_compiler_stack(|| {
        let source = r#"
resource type Trace {stdout:Stdout, text:String, n:u64}
drop(x) {let _=out.print(x.stdout,x.text); let _=f(x.n)}
fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}
fn sink(take r:Trace)->() {let _=out.print(r.stdout,"entered\n")}
fn dive(n:u64,stdout:Stdout)->u64 {
  if n==0u64 {
    let sibling=Trace(stdout:stdout,text:"sibling\n",n:0u64)
    let r=Trace(stdout:stdout,text:"argument\n",n:63u64)
    sink(take r)
    0u64
  }else{1u64+dive(n-1u64,stdout)}
}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?
 let _=dive(1022u64,stdout)
 Ok(())
}
"#;
        // Full engine observations include output; no sink effect may precede
        // the argument finalizer, and sibling cleanup follows its failure.
        agreement_output(
            "p4-take-effects",
            source,
            &["LANG-10", "LANG-10"],
            Some(b"argument\nsibling\n"),
        );
    });
}

#[test]
fn p5_mem6_charge_remains_independent_of_invocation_depth() {
    hirsel_engine::with_compiler_stack(|| {
        for cap in [15, 16] {
            let source = format!(
                r#"
pub fn main(rt:Io)->Result[(),GrantError] {{
  let stdout=io.stdout(rt)?
  let outcome:Result[u64,OutOfSpace]=arena.bounded({cap}u64,fn(a:Arena)->u64 {{
    let xs:List[u64] in a=[1u64,2u64]
    list.len(xs)
  }})
  let _=out.print(stdout,match outcome {{
    Ok(n)=>if n==2u64 {{"two\n"}} else {{"wrong count\n"}},
    Error(e)=>"out of space\n",
  }})
  Ok(())
}}
"#
            );
            agreement_output(
                &format!("p5-charge-{cap}"),
                &source,
                &[],
                Some(if cap == 15 {
                    b"out of space\n"
                } else {
                    b"two\n"
                }),
            );
        }
        let source = r#"
fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}
pub fn main()->Result[(), ()] {
 let _=arena.bounded(0u64,fn(a:Arena)->u64{f(1021u64)})
 Ok(())
}
"#;
        agreement("p5-depth-never-outofspace", source, &["LANG-10"]);
    });
}
/// LANG-10 leaf operations add no activation, so a frame at the limit still runs
/// them, and recursion still stops at exactly 1,024. A callback primitive, an
/// adapter-backed primitive and a leaf called through a function value still count.
#[test]
fn p6_leaf_operations_add_no_activation_at_the_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        // At f(0), main and the f frames hold n + 2 activations.
        let leaf = "fn f(n:u64)->u64 {if n==0u64 {let mut xs:List[u64]=[]; list.reserve(mut xs,1u64); list.push(mut xs,int.wrapping_add(n,1u64)); failure.check(list.len(xs)==1u64,TextInvariant()); f64.bits(f64.from_bits(xs[0u64]))} else {1u64+f(n-1u64)}}";
        let callback = "fn f(n:u64)->u64 {if n==0u64 {arena.with(fn(a:Arena)->u64 {0u64})} else {1u64+f(n-1u64)}}";
        let indirect = "fn f(n:u64)->u64 {if n==0u64 {let g:fn(u64,u64)->u64=int.wrapping_add; g(n,1u64)} else {1u64+f(n-1u64)}}";
        for (name, header, n, rules) in [
            ("p6-leaf-exact", leaf, 1022, &[][..]),
            ("p6-leaf-over", leaf, 1023, &["LANG-10"][..]),
            ("p6-callback-exact", callback, 1020, &[][..]),
            ("p6-callback-over", callback, 1021, &["LANG-10"][..]),
            ("p6-indirect-exact", indirect, 1021, &[][..]),
            ("p6-indirect-over", indirect, 1022, &["LANG-10"][..]),
        ] {
            let source =
                format!("{header}\npub fn main()->Result[(), ()] {{let _ = f({n}u64); Ok(())}}");
            agreement(name, &source, rules);
        }
        for (n, rules, output) in [
            (1021, &[][..], b"deep\n".as_slice()),
            (1022, &["LANG-10"][..], b"".as_slice()),
        ] {
            let source = format!(
                r#"fn f(n:u64,s:Stdout)->u64 {{if n==0u64 {{let _=out.print(s,"deep\n"); 0u64}} else {{1u64+f(n-1u64,s)}}}}
pub fn main(rt:Io)->Result[(),GrantError] {{let s=io.stdout(rt)?; let _=f({n}u64,s); Ok(())}}"#
            );
            agreement_output(&format!("p6-adapter-{n}"), &source, rules, Some(output));
        }
    });
}
const P3_CHAIN: &str = r#"
type Chain = End(value:Option[u64]) | Link(next:box Chain)
type Alias = Chain
distinct type Wrapped(v:Chain)
fn chain(payload:bool)->Chain {
 let mut c:Chain=End(value:if payload {Some(0u64)} else {None})
 for i in 0u64..543u64 {c=Link(next:box take c)}
 take c
}
"#;

fn p3_logical_edges_admit_s_minus_one_and_s_but_reject_distinct_and_tuple_s_plus_one(
    selected: &str,
) {
    hirsel_engine::with_compiler_stack(|| {
        let mut ran = false;
        for (name, setup, rules, output) in [
            (
                "p3-minus-one",
                "let x=chain(false)",
                &[][..],
                b"equal\n".as_slice(),
            ),
            (
                "p3-exact",
                "let x=chain(true)",
                &[][..],
                b"equal\n".as_slice(),
            ),
            (
                "p3-alias-exact",
                "let x:Alias=chain(true)",
                &[][..],
                b"equal\n".as_slice(),
            ),
            (
                "p3-distinct-over",
                "let x=Wrapped(chain(true))",
                &["MEM-3"][..],
                b"".as_slice(),
            ),
            (
                "p3-tuple-over",
                "let x=(chain(true),)",
                &["MEM-3"][..],
                b"".as_slice(),
            ),
        ] {
            if name != selected {
                continue;
            }
            ran = true;
            let source = format!(
                "{P3_CHAIN}\npub fn main(rt:Io)->Result[(),GrantError] {{let stdout=io.stdout(rt)?\n{setup}\nlet same=x==x\nlet _=out.print(stdout,if same {{\"equal\\n\"}} else {{\"unequal\\n\"}})\nOk(())}}"
            );
            agreement_output(name, &source, rules, Some(output));
        }
        assert!(ran, "unknown P3 case {selected}");
    });
}

fn p3_preflight_precedes_mismatch_tag_length_and_repeated_shared_path_shortcuts(selected: &str) {
    hirsel_engine::with_compiler_stack(|| {
        let mut ran = false;
        for (name, setup) in [
            (
                "p3-early-mismatch",
                "let x=(0u64,chain(true));let y=(1u64,chain(true))",
            ),
            (
                "p3-tag-mismatch",
                "let x:Option[Chain]=Some(chain(true));let y:Option[Chain]=None",
            ),
            (
                "p3-length-mismatch",
                "let x:List[Chain]=[chain(true)];let y:List[Chain]=[]",
            ),
            (
                "p3-shared-deeper-path",
                "let c=chain(false);let x=(copy c,(take c,));let y=copy x",
            ),
        ] {
            if name != selected {
                continue;
            }
            ran = true;
            let source =
                format!("{P3_CHAIN}\npub fn main()->Result[(),()] {{{setup}\nlet _=x==y\nOk(())}}");
            agreement_output(name, &source, &["MEM-3"], Some(b""));
        }
        assert!(ran, "unknown P3 case {selected}");
    });
}

fn p3_key_admission_precedes_empty_container_and_whole_map_counts_no_pair_edge(selected: &str) {
    hirsel_engine::with_compiler_stack(|| {
        let mut ran = false;
        for (name, setup, rules) in [
            (
                "p3-empty-map-probe",
                "let m:Map[(Chain,),u64]=map.empty();let key=(chain(true),);let _=map.get(m,key)",
                &["MEM-3"][..],
            ),
            (
                "p3-empty-set-probe",
                "let s:Set[(Chain,)]=set.empty();let key=(chain(true),);let _=set.contains(s,key)",
                &["MEM-3"][..],
            ),
            (
                "p3-whole-map-exact",
                "let key=chain(false);let mut m:Map[Chain,u64]=map.empty();map.insert(mut m,take key,1u64);let _=m==m",
                &[][..],
            ),
            (
                "p3-whole-map-over",
                "let key=chain(true);let mut m:Map[Chain,u64]=map.empty();map.insert(mut m,take key,1u64);let _=m==m",
                &["MEM-3"][..],
            ),
        ] {
            if name != selected {
                continue;
            }
            ran = true;
            let source = format!("{P3_CHAIN}\npub fn main()->Result[(),()] {{{setup}\nOk(())}}");
            agreement_output(name, &source, rules, Some(b""));
        }
        assert!(ran, "unknown P3 case {selected}");
    });
}

#[test]
fn p3_packed_and_flat_representation_wrappers_add_no_edges_at_the_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        for (name, declaration, leaf) in [
            (
                "p3-packed-exact",
                "type Leaf=Small(v:u64)|Wide(v:u64,extra:u64)\ntype Chain=End(value:Packed[Leaf])|Link(next:box Chain)",
                "Small(v:0u64)",
            ),
            (
                "p3-flat-exact",
                "flat type Leaf {n:u64}\ntype Chain=End(value:Leaf)|Link(next:box Chain)",
                "Leaf(n:0u64)",
            ),
        ] {
            let source = format!(
                r#"{declaration}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let mut c:Chain=End(value:{leaf})
 for i in 0u64..543u64 {{c=Link(next:box take c)}}
 let same=c==c
 let _=out.print(stdout,if same {{"equal\n"}} else {{"unequal\n"}})
 Ok(())
}}"#
            );
            agreement_output(name, &source, &[], Some(b"equal\n"));
        }
    });
}

#[test]
fn p3_shallow_width_and_byte_lengths_do_not_spend_logical_depth() {
    hirsel_engine::with_compiler_stack(|| {
        let source = r#"
flat type Row {xs:List[u64]}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?
 let mut xs:List[u64]=[]
 for i in 0u64..256u64 {list.push(mut xs,i)}
 let row=Row(xs:[1u64,2u64,3u64])
 let same=xs==xs && row==row && b"wide bytes"==b"wide bytes"
 let _=out.print(stdout,if same {"equal\n"} else {"unequal\n"})
 Ok(())
}
"#;
        agreement_output("p3-width", source, &[], Some(b"equal\n"));
    });
}

fn p3_value_operations_preserve_flat_packed_sequential_and_column_equality_and_hash(
    selected: &str,
) {
    hirsel_engine::with_compiler_stack(|| {
        let mut ran = false;
        for (name, declaration, checks) in [
            (
                "p3-flat-values",
                "flat type Row {label:Bytes,n:i64}",
                r#"
 let a=Row(label:b"abc",n:-1i64)
 let b=Row(label:b"abc",n:1i64)
 let c=Row(label:b"abcd",n:-2i64)
 let mut m:Map[Row,u64]=map.empty()
 map.insert(mut m,copy a,7u64)
 map.insert(mut m,Row(label:b"abc",n:-1i64),9u64)
 let good=a!=b && b!=c && a==a && map.get(m,a)==Some(9u64) && map.len(m)==1u64
"#,
            ),
            (
                "p3-packed-values",
                "type Choice=Empty|Number(n:i64)|Pair(n:i64,extra:u8)",
                r#"
 let a:Packed[Choice]=Number(n:-1i64)
 let b:Packed[Choice]=Number(n:1i64)
 let c:Packed[Choice]=Pair(n:-2i64,extra:7u8)
 let mut m:Map[Packed[Choice],u64]=map.empty()
 map.insert(mut m,copy a,7u64)
 map.insert(mut m,Number(n:-1i64),9u64)
 let good=a!=b && b!=c && a==a && map.get(m,a)==Some(9u64) && map.len(m)==1u64
"#,
            ),
            (
                "p3-sequential-values",
                "type Choice=Empty|Number(n:i64)|Pair(n:i64,extra:u8)",
                r#"
 let a:List[Packed[Choice]]=[Empty,Number(n:-1i64),Pair(n:2i64,extra:7u8)]
 let b:List[Packed[Choice]]=[Empty,Number(n:1i64),Pair(n:2i64,extra:7u8)]
 let mut m:Map[List[Packed[Choice]],u64]=map.empty()
 map.insert(mut m,copy a,7u64)
 map.insert(mut m,[Empty,Number(n:-1i64),Pair(n:2i64,extra:7u8)],9u64)
 let good=a!=b && a==a && map.get(m,a)==Some(9u64) && map.len(m)==1u64
"#,
            ),
            (
                "p3-column-values",
                "@soa\ntype Row {n:i64,label:String}",
                r#"
 let a:List[Row]=[Row(n:-1i64,label:"a"),Row(n:3i64,label:"z")]
 let b:List[Row]=[Row(n:1i64,label:"a"),Row(n:2i64,label:"z")]
 let mut m:Map[List[Row],u64]=map.empty()
 map.insert(mut m,copy a,7u64)
 map.insert(mut m,[Row(n:-1i64,label:"a"),Row(n:3i64,label:"z")],9u64)
 let good=a!=b && a==a && map.get(m,a)==Some(9u64) && map.len(m)==1u64
"#,
            ),
        ] {
            if name != selected {
                continue;
            }
            ran = true;
            let source = format!(
                r#"{declaration}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 {checks}
 let _=out.print(stdout,if good {{"values\n"}} else {{"wrong\n"}})
 Ok(())
}}"#
            );
            agreement_output(name, &source, &[], Some(b"values\n"));
        }
        assert!(ran, "unknown P3 case {selected}");
    });
}

// Draft outside frozen source; compile/run before accepting any identity.
#[test]
fn task_entry_has_fresh_count_and_parent_suspension_retains_its_count() {
    hirsel_engine::with_compiler_stack(|| {
        for (name, child, rules) in [
            ("task-fresh-exact", 1022, &[][..]),
            ("task-fresh-over", 1023, &["LANG-10"][..]),
        ] {
            let source = format!(
                r#"
fn f(n:u64)->u64 {{if n==0u64 {{0u64}} else {{1u64+f(n-1u64)}}}}
fn launch(tasks:Tasks,n:u64)->u64 {{
 if n==0u64 {{task.scope(tasks,fn(s:Scope)->u64 {{
   let h=task.async(s,fn()->u64 {{f({child}u64)}})
   task.await(s,take h)
 }})}} else {{1u64+launch(tasks,n-1u64)}}
}}
pub fn main(rt:Io)->Result[(),GrantError] {{let tasks=io.tasks(rt)?;let _=launch(tasks,1018u64);Ok(())}}
"#
            );
            agreement(name, &source, rules);
        }
    });
}

#[test]
fn isolate_unwinds_live_invocations_without_resetting_parent() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "isolate-depth-recovery",
            r#"
fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let stdout=io.stdout(rt)?
 let mut good=true
 for i in 0u64..2u64 {
   let status:Result[u64,Failure]=task.isolate(tasks,fn()->u64 {f(1021u64)})
   good=good && match status {Error(caught)=>caught.rule=="LANG-10",Ok(value)=>false}
 }
 let value=f(1022u64)
 let _=out.print(stdout,if good && value==1022u64 {"restored\n"}else{"wrong\n"})
 Ok(())
}
"#,
            &[],
            Some(b"restored\n"),
        );
    });
}

#[test]
fn cancellation_drains_cleanup_at_full_foreground_depth_and_retires_for_later_calls() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "cancel-depth-cleanup",
            r#"
fn f(n:u64)->u64 {if n==0u64 {0u64}else {1u64+f(n-1u64)}}
resource type Trace {stdout:Stdout}
drop(x) {let _=f(62u64);let _=out.print(x.stdout,"released\n")}
fn waiting(tasks:Tasks,take receiver:Receiver[u8],stdout:Stdout,n:u64)->u64 {
 if n==0u64 {
   let trace=Trace(stdout:stdout)
   let mut r=take receiver
   match channel.receive(tasks,mut r) {Error(Canceled(Requested))=>1u64,_=>0u64}
 }else{1u64+waiting(tasks,take receiver,stdout,n-1u64)}
}
pub fn main(rt:Io)->Result[(),GrantError] {
 let tasks=io.tasks(rt)?
 let stdout=io.stdout(rt)?
 let received=task.scope(tasks,fn(s:Scope)->u64 {
   let (sender,receiver):(Sender[u8],Receiver[u8])=channel.open(tasks)
   let h=task.async(s,fn()->u64 {waiting(tasks,take receiver,stdout,1021u64)})
   task.cancel(s,h)
   task.await(s,take h)
 })
 let value=f(1022u64)
 let _=out.print(stdout,if received==1022u64 && value==1022u64 {"restored\n"}else{"wrong\n"})
 Ok(())
}
"#,
            &[],
            Some(b"released\nrestored\n"),
        );
    });
}

macro_rules! p3_case {
    ($test:ident, $helper:ident, $case:literal) => {
        #[test]
        fn $test() {
            $helper($case);
        }
    };
}
p3_case!(
    p3_exact,
    p3_logical_edges_admit_s_minus_one_and_s_but_reject_distinct_and_tuple_s_plus_one,
    "p3-exact"
);
p3_case!(
    p3_alias_exact,
    p3_logical_edges_admit_s_minus_one_and_s_but_reject_distinct_and_tuple_s_plus_one,
    "p3-alias-exact"
);
p3_case!(
    p3_distinct_over,
    p3_logical_edges_admit_s_minus_one_and_s_but_reject_distinct_and_tuple_s_plus_one,
    "p3-distinct-over"
);
p3_case!(
    p3_tuple_over,
    p3_logical_edges_admit_s_minus_one_and_s_but_reject_distinct_and_tuple_s_plus_one,
    "p3-tuple-over"
);
p3_case!(
    p3_early_mismatch,
    p3_preflight_precedes_mismatch_tag_length_and_repeated_shared_path_shortcuts,
    "p3-early-mismatch"
);
p3_case!(
    p3_tag_mismatch,
    p3_preflight_precedes_mismatch_tag_length_and_repeated_shared_path_shortcuts,
    "p3-tag-mismatch"
);
p3_case!(
    p3_length_mismatch,
    p3_preflight_precedes_mismatch_tag_length_and_repeated_shared_path_shortcuts,
    "p3-length-mismatch"
);
p3_case!(
    p3_shared_deeper_path,
    p3_preflight_precedes_mismatch_tag_length_and_repeated_shared_path_shortcuts,
    "p3-shared-deeper-path"
);
p3_case!(
    p3_empty_map_probe,
    p3_key_admission_precedes_empty_container_and_whole_map_counts_no_pair_edge,
    "p3-empty-map-probe"
);
p3_case!(
    p3_empty_set_probe,
    p3_key_admission_precedes_empty_container_and_whole_map_counts_no_pair_edge,
    "p3-empty-set-probe"
);
p3_case!(
    p3_whole_map_exact,
    p3_key_admission_precedes_empty_container_and_whole_map_counts_no_pair_edge,
    "p3-whole-map-exact"
);
p3_case!(
    p3_whole_map_over,
    p3_key_admission_precedes_empty_container_and_whole_map_counts_no_pair_edge,
    "p3-whole-map-over"
);
p3_case!(
    p3_flat_values,
    p3_value_operations_preserve_flat_packed_sequential_and_column_equality_and_hash,
    "p3-flat-values"
);
p3_case!(
    p3_packed_values,
    p3_value_operations_preserve_flat_packed_sequential_and_column_equality_and_hash,
    "p3-packed-values"
);
p3_case!(
    p3_sequential_values,
    p3_value_operations_preserve_flat_packed_sequential_and_column_equality_and_hash,
    "p3-sequential-values"
);
p3_case!(
    p3_column_values,
    p3_value_operations_preserve_flat_packed_sequential_and_column_equality_and_hash,
    "p3-column-values"
);

// Additional required direct Take rejection controls; not yet applied/executed.
fn keyed_take_source(set: bool, over: bool) -> String {
    let key = if over { "chain(true)" } else { "chain(false)" };
    let operation = if set {
        "let mut target:Set[(Chain,)]=set.empty();set.insert(mut target,take key)"
    } else {
        "let mut target:Map[(Chain,),u64]=map.empty();map.insert(mut target,take key,1u64)"
    };
    format!(
        r#"{P3_CHAIN}
resource type Sibling {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"sibling\n")}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let sibling=Sibling(stdout:stdout)
 let key=({key},)
 {operation}
 let _=out.print(stdout,"entered\n")
 Ok(())
}}"#
    )
}

#[test]
fn p3_take_map_exact() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "p3-take-map-exact",
            &keyed_take_source(false, false),
            &[],
            Some(b"entered\nsibling\n"),
        );
    });
}

#[test]
fn p3_take_map_over() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "p3-take-map-over",
            &keyed_take_source(false, true),
            &["MEM-3"],
            Some(b"sibling\n"),
        );
    });
}

#[test]
fn p3_take_set_exact() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "p3-take-set-exact",
            &keyed_take_source(true, false),
            &[],
            Some(b"entered\nsibling\n"),
        );
    });
}

#[test]
fn p3_take_set_over() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "p3-take-set-over",
            &keyed_take_source(true, true),
            &["MEM-3"],
            Some(b"sibling\n"),
        );
    });
}

#[test]
fn optimized_inlining_keeps_the_leaf_invocation_at_full_foreground_depth() {
    hirsel_engine::with_compiler_stack(|| {
        let mut matrix = matrix();
        for configuration in &mut matrix {
            if let Engine::Native { opt, .. } = &mut configuration.engine {
                *opt = hirsel_driver::native::OptLevel::Speed;
            }
        }
        let toolchain = Toolchain::for_matrix(&native::runtime_cache().unwrap(), &matrix).unwrap();
        let scratch = native::scratch::RunRoot::create_in(
            &std::env::temp_dir().join("hirsel-logical-depth-31"),
        )
        .unwrap();
        for (name, n, rules) in [
            ("inline-leaf-exact", 1021, &[][..]),
            ("inline-leaf-over", 1022, &["LANG-10"][..]),
        ] {
            let source = format!(
                r#"
fn leaf()->u64 {{7u64}}
fn f(n:u64)->u64 {{if n==0u64 {{leaf()}}else{{1u64+f(n-1u64)}}}}
pub fn main()->Result[(),()] {{let _=f({n}u64);Ok(())}}
"#
            );
            let result = native::run_source(
                name,
                &source,
                &matrix,
                &toolchain,
                &scratch.join(name).unwrap(),
            );
            assert!(result.snapshot.is_none(), "{name}: {:?}", result.snapshot);
            let observation = result.reference.as_ref().expect("incomplete reference");
            assert_eq!(
                observation
                    .failures
                    .iter()
                    .map(|f| f.rule.as_str())
                    .collect::<Vec<_>>(),
                rules,
                "{name}: {observation:?}"
            );
            assert_eq!(result.outcomes.len(), 3, "{name}");
            for (configuration, _, outcome) in &result.outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{name}/{configuration}: {outcome:?}"
                );
            }
            eprintln!("logical-depth case {name}: all four engines agree (LLVM Speed)");
        }
    });
}

#[test]
fn independent_body_and_callback_proofs_cover_generic_closures_and_contracts() {
    hirsel_engine::with_compiler_stack(|| {
        agreement_output(
            "body-proof-generic-callback-contract",
            r#"
fn checked(x:u64)->u64
 requires x==7u64
 ensures result==7u64
 {x}
fn apply[T](f:fn(T)->T,x:T)->T {f(x)}
pub fn main(rt:Io)->Result[(),GrantError] {
 let stdout=io.stdout(rt)?
 let x=apply(fn(n:u64)->u64 {checked(n)},7u64)
 let bounded:Result[u64,OutOfSpace]=arena.bounded(0u64,fn(a:Arena)->u64 {checked(x)})
 let good=bounded==Ok(7u64)
 let _=out.print(stdout,if good {"body proof\n"}else{"wrong\n"})
 Ok(())
}
"#,
            &[],
            Some(b"body proof\n"),
        );
    });
}

fn indirect_keyed_take(set: bool, over: bool) {
    let source = keyed_take_source(set, over);
    let source = if set {
        source.replace("set.insert(mut target,take key)","let insert:fn(mut Set[(Chain,)],take (Chain,))->()=set.insert;insert(mut target,take key)")
    } else {
        source.replace("map.insert(mut target,take key,1u64)","let insert:fn(mut Map[(Chain,),u64],take (Chain,),take u64)->()=map.insert;insert(mut target,take key,1u64)")
    };
    agreement_output(
        &format!(
            "indirect-keyed-{}-{}",
            if set { "set" } else { "map" },
            if over { "over" } else { "exact" }
        ),
        &source,
        if over { &["MEM-3"] } else { &[] },
        Some(if over {
            b"sibling\n"
        } else {
            b"entered\nsibling\n"
        }),
    );
}
#[test]
fn indirect_keyed_map_exact() {
    hirsel_engine::with_compiler_stack(|| indirect_keyed_take(false, false));
}
#[test]
fn indirect_keyed_map_rejection() {
    hirsel_engine::with_compiler_stack(|| indirect_keyed_take(false, true));
}
#[test]
fn indirect_keyed_set_exact() {
    hirsel_engine::with_compiler_stack(|| indirect_keyed_take(true, false));
}
#[test]
fn indirect_keyed_set_rejection() {
    hirsel_engine::with_compiler_stack(|| indirect_keyed_take(true, true));
}

fn indirect_borrowed_key(set: bool, over: bool) {
    let source = keyed_take_source(set, over);
    let source = if set {
        source.replace(
            "set.insert(mut target,take key)",
            "let probe:fn(Set[(Chain,)],(Chain,))->bool=set.contains;let _=probe(target,key)",
        )
    } else {
        source.replace(
            "map.insert(mut target,take key,1u64)",
            "let probe:fn(Map[(Chain,),u64],(Chain,))->Option[u64]=map.get;let _=probe(target,key)",
        )
    };
    agreement_output(
        &format!(
            "indirect-borrowed-{}-{}",
            if set { "set" } else { "map" },
            if over { "over" } else { "exact" }
        ),
        &source,
        if over { &["MEM-3"] } else { &[] },
        Some(if over {
            b"sibling\n"
        } else {
            b"entered\nsibling\n"
        }),
    );
}
#[test]
fn indirect_borrowed_map_exact() {
    hirsel_engine::with_compiler_stack(|| indirect_borrowed_key(false, false));
}
#[test]
fn indirect_borrowed_map_rejection() {
    hirsel_engine::with_compiler_stack(|| indirect_borrowed_key(false, true));
}
#[test]
fn indirect_borrowed_set_exact() {
    hirsel_engine::with_compiler_stack(|| indirect_borrowed_key(true, false));
}
#[test]
fn indirect_borrowed_set_rejection() {
    hirsel_engine::with_compiler_stack(|| indirect_borrowed_key(true, true));
}

fn mixed_keyed_target(primitive: bool, over: bool) {
    let source = format!(
        r#"{P3_CHAIN}
fn ignore(mut target:Set[(Chain,)],take key:(Chain,))->() {{ () }}
fn select(primitive:bool)->fn(mut Set[(Chain,)],take (Chain,))->() {{
 let insert:fn(mut Set[(Chain,)],take (Chain,))->()=set.insert
 let ignored:fn(mut Set[(Chain,)],take (Chain,))->()=ignore
 if primitive {{insert}} else {{ignored}}
}}
resource type Sibling {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"sibling\n")}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let sibling=Sibling(stdout:stdout)
 let mut target:Set[(Chain,)]=set.empty()
 let key=(chain({over}),)
 let action=select({primitive})
 action(mut target,take key)
 let _=out.print(stdout,"entered\n")
 Ok(())
}}"#
    );
    let rejects = primitive && over;
    agreement_output(
        &format!("mixed-keyed-{primitive}-{over}"),
        &source,
        if rejects { &["MEM-3"] } else { &[] },
        Some(if rejects {
            b"sibling\n"
        } else {
            b"entered\nsibling\n"
        }),
    );
}
#[test]
fn indirect_mixed_primitive_exact() {
    hirsel_engine::with_compiler_stack(|| mixed_keyed_target(true, false));
}
#[test]
fn indirect_mixed_primitive_rejection() {
    hirsel_engine::with_compiler_stack(|| mixed_keyed_target(true, true));
}
#[test]
fn indirect_mixed_ordinary_deep_key_has_no_structural_read() {
    hirsel_engine::with_compiler_stack(|| mixed_keyed_target(false, true));
}

fn indirect_resource_actual(over: bool) {
    let source = format!(
        r#"{P3_CHAIN}
resource type Sibling {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"sibling\n")}}
resource type Payload {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"payload\n")}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let sibling=Sibling(stdout:stdout)
 let mut target:Map[(Chain,),Payload]=map.empty()
 let key=(chain({over}),)
 let value=Payload(stdout:stdout)
 let insert:fn(mut Map[(Chain,),Payload],take (Chain,),take Payload)->()=map.insert
 insert(mut target,take key,take value)
 let _=out.print(stdout,"entered\n")
 Ok(())
}}"#
    );
    agreement_output(
        &format!("indirect-resource-{over}"),
        &source,
        if over { &["MEM-3"] } else { &[] },
        Some(if over {
            b"payload\nsibling\n"
        } else {
            b"entered\npayload\nsibling\n"
        }),
    );
}
#[test]
fn indirect_resource_actual_exact() {
    hirsel_engine::with_compiler_stack(|| indirect_resource_actual(false));
}
#[test]
fn indirect_resource_actual_rejection() {
    hirsel_engine::with_compiler_stack(|| indirect_resource_actual(true));
}

fn indirect_key_before_arena_charge(over: bool) {
    let source = format!(
        r#"{P3_CHAIN}
resource type Sibling {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"sibling\n")}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let sibling=Sibling(stdout:stdout)
 let bounded=arena.bounded(0u64,fn(a:Arena)->u64 {{
   let key=(chain({over}),)
   let mut target:Set[(Chain,)] in a=set.empty()
   let insert:fn(mut Set[(Chain,)],take (Chain,))->()=set.insert
   insert(mut target,take key)
   1u64
 }})
 let _=out.print(stdout,match bounded {{Ok(_)=>"entered\n",Error(_)=>"space\n"}})
 Ok(())
}}"#
    );
    agreement_output(
        &format!("indirect-key-before-charge-{over}"),
        &source,
        if over { &["MEM-3"] } else { &[] },
        Some(if over {
            b"sibling\n"
        } else {
            b"space\nsibling\n"
        }),
    );
}
#[test]
fn indirect_arena_charge_after_admitted_key() {
    hirsel_engine::with_compiler_stack(|| indirect_key_before_arena_charge(false));
}
#[test]
fn indirect_arena_rejection_precedes_charge() {
    hirsel_engine::with_compiler_stack(|| indirect_key_before_arena_charge(true));
}

fn sorted_callback_boundary(over: bool) {
    let n = if over { 1021 } else { 1020 };
    let source = format!(
        r#"
fn f(n:u64)->u64 {{
 if n==0u64 {{
  let by=u64.order
  let mut m:SortedMap[u64,u64] of by=sorted_map.empty(by)
  sorted_map.insert(by,mut m,1u64,10u64)
  sorted_map.insert(by,mut m,2u64,20u64)
  0u64
 }} else {{1u64+f(n-1u64)}}
}}
pub fn main()->Result[(),()] {{let _=f({n}u64);Ok(())}}
"#
    );
    agreement(
        &format!("sorted-callback-{over}"),
        &source,
        if over { &["LANG-10"] } else { &[] },
    );
}
#[test]
fn sorted_callback_exact() {
    hirsel_engine::with_compiler_stack(|| sorted_callback_boundary(false));
}
#[test]
fn sorted_callback_rejection() {
    hirsel_engine::with_compiler_stack(|| sorted_callback_boundary(true));
}

fn implicit_codec_check(over: bool) {
    let n = if over { 1021 } else { 1020 };
    let source = format!(
        r#"
@serializable(check=valid)
pub opaque type Month: plain + key + sendable + inert + noalloc_drop uses {{}} {{number:u8}}
@noalloc
fn valid(value:Month)->bool uses {{}} {{value.number>=1 && value.number<=12}}
resource type Trace {{stdout:Stdout}}
drop(x) {{let _=out.print(x.stdout,"sibling\n")}}
fn descend(n:u64,bytes:Bytes)->u64 uses {{}} {{
 if n==0u64 {{
  let decoded:Result[Month,DecodeError]=codec.decode(bytes)
  match decoded {{Ok(_)=>7u64,_=>0u64}}
 }} else {{1u64+descend(n-1u64,bytes)}}
}}
pub fn main(rt:Io)->Result[(),GrantError] {{
 let stdout=io.stdout(rt)?
 let sibling=Trace(stdout:stdout)
 let bytes=codec.encode(Month(number:3))
 let _=descend({n}u64,bytes)
 Ok(())
}}
"#
    );
    agreement_output(
        &format!("implicit-codec-check-{over}"),
        &source,
        if over { &["LANG-10"] } else { &[] },
        Some(if over { b"" } else { b"sibling\n" }),
    );
}

#[test]
fn implicit_codec_check_exact() {
    hirsel_engine::with_compiler_stack(|| implicit_codec_check(false));
}
#[test]
fn implicit_codec_check_rejection_is_fatal_before_sibling_cleanup() {
    hirsel_engine::with_compiler_stack(|| implicit_codec_check(true));
}

#[test]
fn short_circuit_self_restarts_preserve_the_invocation_boundary() {
    hirsel_engine::with_compiler_stack(|| {
        for (name, expression, expected, iterations, rules) in [
            ("tail-or", "n == 0u64 || f(n - 1u64)", true, 2048, &[][..]),
            ("tail-and", "n != 0u64 && f(n - 1u64)", false, 2048, &[][..]),
            (
                "left-or",
                "if n == 0u64 {true} else {f(n - 1u64) || false}",
                true,
                1023,
                &["LANG-10"][..],
            ),
            (
                "left-and",
                "if n == 0u64 {true} else {f(n - 1u64) && true}",
                true,
                1023,
                &["LANG-10"][..],
            ),
        ] {
            let source = format!(
                "fn f(n:u64)->bool {{ {expression} }}\npub fn main()->Result[(), ()] {{let actual = f({iterations}u64); let _ = 1u64 / (if actual == {expected} {{1u64}} else {{0u64}}); Ok(())}}"
            );
            agreement(name, &source, rules);
        }
    });
}
