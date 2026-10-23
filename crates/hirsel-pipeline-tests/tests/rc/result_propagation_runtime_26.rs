//! Single evaluation and LANG-8 selection, separate from failure_cleanup_21's
//! owner-stack model. Short-circuit shapes adapted from Rust Result::branch
//! (core/src/result.rs:2170-2200) and and_then/collect controls
//! (coretests/tests/result.rs:11-77), f6c1107f8be13e48465e12c0fd26e6a39fe18e4f,
//! MIT OR Apache-2.0. Hirsel's nominal injection and mapper rules take precedence.
use crate::rc_support::history::{Category, Runner, persist};
use std::fmt::Write;

const SEED: u64 = 20261026;

// 24 if/helper histories, 16 match/closure histories, and four literal/parameter
// pairs (8 histories). Each mask independently selects Error at three operands.
fn cases(part: usize) -> Vec<(usize, usize, bool)> {
    if part < 5 {
        (0..8)
            .map(|mask| (if part == 4 { 2 } else { part % 3 }, mask, false))
            .collect()
    } else {
        [0, 1, 2, 4]
            .into_iter()
            .flat_map(|mask| [true, false].map(move |literal| (1, mask, literal)))
            .collect()
    }
}

/// Explicit match desugaring: no compiler plan, syntax walker or engine output.
/// Every reached operand increments the caller's count; only its Ok continuation
/// emits an uppercase event. A named mapper is called solely on the first Error.
fn expected(mode: usize, mask: usize) -> String {
    let mut events = String::new();
    let mut count = 0;
    let mut result = Ok(0);
    for layer in 0..3 {
        count += 1;
        events.push(char::from(b'a' + layer));
        let operand = if mask & (1 << layer) == 0 {
            Ok(10 * (usize::from(layer) + 1) + count)
        } else {
            Err(10 * (usize::from(layer) + 1) + count)
        };
        match operand {
            Ok(value) => {
                events.push(char::from(b'A' + layer));
                *result.as_mut().unwrap() += value;
            }
            Err(payload) => {
                result = Err(payload);
                break;
            }
        }
    }
    let (arm, payload) = match result {
        Ok(value) => ('O', value),
        Err(value) => match mode {
            0 => ('I', value),
            1 => ('S', value),
            2 => {
                events.push_str("m1");
                ('N', value + 1000)
            }
            _ => unreachable!(),
        },
    };
    write!(events, "{arm}{payload:04}{count};").unwrap();
    events
}

fn fixture(part: usize) -> (String, String) {
    let mut source = String::from(
        r#"
type Fault {n:u64,stdout:Stdout}
type App = Input(Fault) | Selected(Fault) | Named(Fault) | Noise(u64)
fn digit(stdout:Stdout,n:u64) {
 let _=out.print(stdout,match n {0u64=>"0",1u64=>"1",2u64=>"2",3u64=>"3",4u64=>"4",5u64=>"5",6u64=>"6",7u64=>"7",8u64=>"8",9u64=>"9",_=>"!"})
}
fn number(stdout:Stdout,n:u64) {
 digit(stdout,n/1000u64);digit(stdout,(n/100u64)%10u64)
 digit(stdout,(n/10u64)%10u64);digit(stdout,n%10u64)
}
fn remap(e:Fault)->App {
 let mut calls=0u64
 calls+=1u64
 let _=out.print(e.stdout,"m");digit(e.stdout,calls)
 Named(Fault(n:e.n+1000u64,stdout:e.stdout))
}
fn probe(stdout:Stdout,mut count:u64,bad:bool,layer:u64,poison:bool)->Result[u64,Fault] {
 if poison {let zero=0u64;let _=1u64/zero}
 count+=1u64
 let _=out.print(stdout,match layer {1u64=>"a",2u64=>"b",_=>"c"})
 let n=10u64*layer+count
 if bad {return Error(Fault(n:n,stdout:stdout))}
 Ok(n)
}
"#,
    );
    let mut expected_output = String::new();
    for (index, (mode, mask, literal)) in cases(part).into_iter().enumerate() {
        // Plain injection has exactly one matching arm; explicit selectors and
        // named mappers retain two same-payload arms as wrong-arm controls.
        let error = if mode == 0 { "Plain" } else { "App" };
        if mode == 0 && index == 0 {
            source.push_str("type Plain = Input(Fault) | Noise(u64)\n");
        }
        let mapper = ["", " Selected", " remap"][mode];
        let parameters = "stdout:Stdout,mut count:u64,a:bool,b:bool,c:bool";
        let run_parameters = if part == 3 || part == 4 {
            "outer:Stdout,mut total:u64,x:bool,y:bool,z:bool"
        } else {
            parameters
        };
        writeln!(
            source,
            "fn run{index}({run_parameters})->Result[u64,{error}] {{"
        )
        .unwrap();
        if part == 3 || part == 4 {
            writeln!(
                source,
                "let action=fn({parameters})->Result[u64,{error}] {{"
            )
            .unwrap();
        }
        for (layer, flag) in ["a", "b", "c"].into_iter().enumerate() {
            let bad = if literal {
                ((mask & (1 << layer)) != 0).to_string()
            } else {
                flag.into()
            };
            // If a preceding operand errors, this division must never execute.
            let poison = (mask & ((1 << layer) - 1)) != 0;
            let call =
                |bad: &str| format!("probe(stdout,mut count,{bad},{}u64,{poison})", layer + 1);
            let operand = if part == 4 {
                format!(
                    "match {bad} {{true=>if {bad} {{{}}} else {{{}}},false=>if {bad} {{{}}} else {{{}}}}}",
                    call("true"),
                    call("false"),
                    call("true"),
                    call("false")
                )
            } else if part == 3 {
                format!(
                    "match {bad} {{true=>{},false=>{}}}",
                    call("true"),
                    call("false")
                )
            } else {
                format!("if {bad} {{{}}} else {{{}}}", call("true"), call("false"))
            };
            writeln!(
                source,
                "let v{layer}=({operand})?{mapper}\nlet _=out.print(stdout,\"{}\")",
                char::from(b'A' + layer as u8)
            )
            .unwrap();
        }
        if mask != 0 {
            source.push_str("let sentinel=0u64;let _=1u64/sentinel\n");
        }
        source.push_str("Ok(v0+v1+v2)\n");
        if part == 3 || part == 4 {
            source.push_str("}\nOk(action(outer,mut total,x,y,z)?)\n");
        }
        source.push_str("}\n");
        // An additional Result-returning helper exercises propagation across a
        // helper return, including the already-mapped nominal payload.
        writeln!(source,"fn relay{index}({parameters})->Result[u64,{error}] {{Ok(run{index}(stdout,mut count,a,b,c)?)}}").unwrap();
        expected_output.push_str(&format!("[{part}:{index}]{}", expected(mode, mask)));
    }
    source.push_str("pub fn main(rt:Io)->Result[(),GrantError] {let stdout=io.stdout(rt)?\n");
    for (index, (_, mask, _)) in cases(part).into_iter().enumerate() {
        writeln!(source,"let _=out.print(stdout,\"[{part}:{index}]\")\nlet mut count{index}=0u64\nlet r{index}=relay{index}(stdout,mut count{index},{},{},{})",mask&1!=0,mask&2!=0,mask&4!=0).unwrap();
        let arms = if part == 0 || part == 3 {
            format!(
                "Input(input{index})=>{{let _=out.print(stdout,\"I\");number(stdout,input{index}.n)}},Noise(noise{index})=>{{number(stdout,noise{index})}}"
            )
        } else {
            format!(
                "Input(input{index})=>{{let _=out.print(stdout,\"I\");number(stdout,input{index}.n)}},Selected(selected{index})=>{{let _=out.print(stdout,\"S\");number(stdout,selected{index}.n)}},Named(named{index})=>{{let _=out.print(stdout,\"N\");number(stdout,named{index}.n)}},Noise(noise{index})=>{{number(stdout,noise{index})}}"
            )
        };
        writeln!(source,"match r{index} {{Ok(ok{index})=>{{let _=out.print(stdout,\"O\");number(stdout,ok{index})}},Error(error{index})=>{{match error{index} {{{arms}}}}}}}\ndigit(stdout,count{index});let _=out.print(stdout,\";\")").unwrap();
    }
    source.push_str("Ok(())}\n");
    (source, expected_output)
}

fn run(part: usize) {
    let artifacts = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("result-propagation-runtime-26")
        .join(format!("partition-{part}"));
    let (source, expected) = fixture(part);
    let expected_hash = hirsel_syntax::ContentHash::of_bytes(expected.as_bytes());
    persist(
        &artifacts,
        "input",
        SEED,
        &source,
        &format!(
            "seed_suffix=result-propagation-runtime-26 selected={:?} expected={expected:?} expected_hash={expected_hash:?}",
            cases(part)
        ),
    );
    let runner = Runner::new(&artifacts.join("runs")).expect("missing tools are incomplete");
    eprintln!(
        "result_propagation_runtime_26 started partition={part} histories=8 engines=reference,H,LLVM,Cranelift"
    );
    let report = runner.run(&format!("partition-{part}"), &source);
    persist(
        &artifacts,
        "result",
        SEED,
        &source,
        &format!("expected={expected:?} expected_hash={expected_hash:?}\n{report:#?}"),
    );
    assert!(report.validated, "partition={part}: {report:#?}");
    assert_eq!(report.cells.len(), 4, "missing cells: {report:#?}");
    for cell in &report.cells {
        assert_eq!(cell.category, Category::Observation, "{cell:#?}");
        let observed = cell.observation.as_ref().unwrap();
        assert_eq!(
            observed.output,
            expected.as_bytes(),
            "partition={part} engine={}",
            cell.engine
        );
        assert_eq!(observed.exit_code, Some(0), "{cell:#?}");
        assert!(
            observed.failures.is_empty()
                && !observed.blocked
                && observed.budget.is_none()
                && observed.signaled.is_none(),
            "{cell:#?}"
        );
        eprintln!(
            "result_propagation_runtime_26 completed pass partition={part} engine={} IDs={:?} expected_hash={expected_hash:?} observed={observed:?}",
            cell.engine,
            (0..8).map(|i| format!("{part}:{i}")).collect::<Vec<_>>()
        );
    }
}

#[test]
fn if_plain() {
    run(0);
}
#[test]
fn if_selected() {
    run(1);
}
#[test]
fn if_named() {
    run(2);
}
#[test]
fn closure_match_plain() {
    run(3);
}
#[test]
fn closure_match_named() {
    run(4);
}
#[test]
fn literal_parameter_twins() {
    run(5);
}
