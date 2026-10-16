use crate::source_std;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Integer, Outcome, Value, library::Library};

fn engine(source: &str) -> QueryEngine {
    let mut engine = source_std::engine();
    let file = engine.sources.add("clocks.hir", source.to_owned()).unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}

const ARITHMETIC: &str = r#"
pub fn span(c: MonotonicClock, first: Instant of c, last: Instant of c) -> i128 {
  std@1/time.between(c, first, last).nanoseconds
}
pub fn order(c: MonotonicClock, first: Instant of c, last: Instant of c) -> i32 {
  std@1/time.compare(c, first, last)
}
pub fn main(rt: Io, args: Args) -> Result[(), ClockError] {
  match io.monotonic_clock(rt) {
    Ok(c) => {
      let first = clock.monotonic(c)?
      let last = clock.monotonic(c)?
      failure.check(span(c, first, last) == 0, Arithmetic())
      failure.check(order(c, first, last) == 0, Arithmetic())
      failure.check(std@1/time.elapsed(c, first)?.nanoseconds == 0, Arithmetic())
      Ok(())
    },
    Error(NotGranted()) => Ok(()),
  }
}
"#;

#[test]
fn ordinary_source_time_operations_execute_with_extreme_ticks_and_replay() {
    with_compiler_stack(|| {
        let mut engine = engine(ARITHMETIC);
        let run = RunConfiguration {
            entry: entry_path(&default_context()),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut config = SimulationConfig {
            monotonic: u64::MAX,
            ..Default::default()
        };
        config.grants.insert("MonotonicClock".into());
        let execution = run_observed_snapshot(&mut engine, &run, config.clone()).unwrap();
        assert!(matches!(execution.outcome, Ok(RunOutcome::Exit(0))));
        let events: Vec<_> = execution
            .observation
            .as_ref()
            .unwrap()
            .effects
            .iter()
            .filter_map(|e| match &e.effect {
                hirsel_interp::observation::Effect::CapabilityConsumed(input) => {
                    Some(input.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.operation == "clock.monotonic")
                .count(),
            3
        );
        let replay = run_observed_snapshot(
            &mut engine,
            &RunConfiguration {
                inputs: events,
                ..run
            },
            config,
        )
        .unwrap();
        assert_eq!(replay.observation, execution.observation);
    });
}

#[test]
fn signed_differences_and_comparisons_execute_source_units_without_wrapper_allocation() {
    with_compiler_stack(|| {
        let mut engine = engine(ARITHMETIC);
        let runtime = engine.runtime_types().unwrap();
        let sources = engine.sources.clone();
        for (first, last) in [(0, u64::MAX), (u64::MAX, 0), (u64::MAX, u64::MAX)] {
            for (name, expected, kind) in [
                (
                    "span",
                    i128::from(last) - i128::from(first),
                    hirsel_types::Primitive::I128,
                ),
                (
                    "order",
                    if first < last {
                        -1
                    } else if first > last {
                        1
                    } else {
                        0
                    },
                    hirsel_types::Primitive::I32,
                ),
            ] {
                let mut entry = entry_path(&default_context());
                entry.0.name = name.into();
                let run = RunConfiguration {
                    entry,
                    hash_seed: 0,
                    inputs: vec![],
                };
                let mut config = SimulationConfig::default();
                config.grants.insert("MonotonicClock".into());
                let mut operations = Library::new(&run, config).unwrap();
                let clock = Value::Handle {
                    kind: "MonotonicClock".into(),
                    id: 1,
                };
                // The fixture supplies the enumerated primitive's exact opaque word
                // representation; clients still have no constructor or decoder.
                let args = vec![
                    clock,
                    Value::Handle {
                        kind: "Instant".into(),
                        id: first,
                    },
                    Value::Handle {
                        kind: "Instant".into(),
                        id: last,
                    },
                ];
                let execution = hirsel_interp::run_with_runtime(
                    &mut engine,
                    &sources,
                    &run,
                    args,
                    hirsel_interp::Limits::default(),
                    &mut operations,
                    &runtime,
                )
                .unwrap();
                let bits = if kind == hirsel_types::Primitive::I32 {
                    expected as u32 as u128
                } else {
                    expected as u128
                };
                assert!(
                    matches!(execution.outcome, Outcome::Returned { value: Value::Integer(n), .. } if n == Integer::new(kind, bits).unwrap())
                );
            }
        }
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = native::build(
            &validated,
            &native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        for unit in &ready.units {
            if unit.unit().header.definition.0.package.name == "std"
                && unit.unit().header.definition.0.modules == ["time"]
            {
                assert_eq!(
                    unit.unit().header.origin,
                    hirsel_lir::ir::UnitOrigin::Source
                );
            }
        }
        native::check_runtime_calls(&ready).unwrap();
    });
}

#[test]
#[ignore = "full gate"]
fn full_gate_native_instant_arithmetic_matches_direct_code() {
    with_compiler_stack(|| {
        use std::process::Command;
        let mut engine = engine(ARITHMETIC);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let target = native::TargetSpec::x86_64_unknown_linux_gnu();
        let ready = native::build(&validated, &native::Options::new(target.clone())).unwrap();
        let artifact = native::emit(&ready, native::OptLevel::Speed, |_| true).unwrap();
        let symbol = &artifact
            .exports
            .iter()
            .find(|s| s.definition.as_ref().is_some_and(|d| d.0.name == "span"))
            .unwrap()
            .name;
        let scratch = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("instant-arithmetic");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("time.o"), &artifact.bytes).unwrap();
        let tools = native::Toolchain::in_tree(&scratch.join("runtime"), &[&target]).unwrap();
        let harness = format!(
            r#"
#include <stdint.h>
#include <stdio.h>
#include <time.h>
#include <stdlib.h>
extern uint8_t span(void*, __int128*, const uint64_t*, const uint64_t*, const uint64_t*) __asm__("{symbol}");
__attribute__((noinline)) uint8_t direct(void* ctx, __int128* out, const uint64_t* c, const uint64_t* a, const uint64_t* b) {{
  *out = (__int128)*b - (__int128)*a; return 0;
}}
static uint64_t now(void) {{ struct timespec t; if(clock_gettime(CLOCK_MONOTONIC, &t)) abort(); return (uint64_t)t.tv_sec * 1000000000 + t.tv_nsec; }}
int main(void) {{
  uint64_t c=1, a=0, b=UINT64_MAX; __int128 out=0;
  if(span(NULL,&out,&c,&a,&b) || out != (__int128)UINT64_MAX) return 1;
  if(span(NULL,&out,&c,&b,&a) || out != -(__int128)UINT64_MAX) return 2;
  if(span(NULL,&out,&c,&b,&b) || out != 0) return 3;
  volatile uint64_t checksum=0;
  for (int round=0; round<7; ++round) {{
    uint64_t begin=now();
    for(uint64_t i=0; i<1000000; ++i) {{ a=i; b=i+1; if(direct(NULL,&out,&c,&a,&b)) return 4; checksum += (uint64_t)out; }}
    uint64_t direct_ns=now()-begin; begin=now();
    for(uint64_t i=0; i<1000000; ++i) {{ a=i; b=i+1; if(span(NULL,&out,&c,&a,&b)) return 5; checksum += (uint64_t)out; }}
    printf("%d %llu %llu\n", round, (unsigned long long)direct_ns, (unsigned long long)(now()-begin));
  }}
  return checksum != 14000000;
}}
"#
        );
        std::fs::write(scratch.join("harness.c"), harness).unwrap();
        let compilation = Command::new(&tools.clang)
            .args(["-O3", "-fno-lto"])
            .arg(scratch.join("harness.c"))
            .arg(scratch.join("time.o"))
            .arg(tools.archive(&target.triple).unwrap())
            .args(["-lm", "-ldl", "-lpthread"])
            .arg("-o")
            .arg(scratch.join("bench"))
            .output()
            .unwrap();
        assert!(
            compilation.status.success(),
            "{}",
            String::from_utf8_lossy(&compilation.stderr)
        );
        let output = Command::new(scratch.join("bench")).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        std::fs::write(scratch.join("samples.txt"), &output.stdout).unwrap();
        eprintln!(
            "Instant arithmetic samples (round direct_ns source_ns):\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let llvm_prefix = std::env::var_os("LLVM_SYS_211_PREFIX")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "/usr/lib/llvm-21".into());
        let disassembly = Command::new(llvm_prefix.join("bin/llvm-objdump"))
            .args(["-d", "--no-show-raw-insn"])
            .arg(scratch.join("time.o"))
            .output()
            .unwrap();
        std::fs::write(scratch.join("disassembly.txt"), &disassembly.stdout).unwrap();
        eprintln!("Clock artifact directory: {}", scratch.display());
    });
}
