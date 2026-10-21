//! The Ready-construct probes (`support::ready`). The heap checker runs every case for its
//! observation, and the native program, linked with a generated C table and the runtime,
//! must print exactly those observations on every target and leave no block live. Native
//! runs need the pinned toolchain, so `tools/check-native.sh` runs them (`--include-ignored`);
//! the plain test run covers emission and the heap checker's audit.

use crate::support;

use std::fmt::Write as _;

use hirsel_heapcheck::{Arg, Config, NoOperations, Observed};
use hirsel_lir::emission::{EmitInputs, OptLevel, site_code};
use hirsel_lir::ir::SiteRange;
use hirsel_lir::layout_types::{ScalarKind, TargetSpec};
use hirsel_lir::stage::Ready;
use hirsel_llvm::emit_object;
use support::native::*;
use support::programs::Program;
use support::ready::*;
use support::*;

const OPTS: [OptLevel; 2] = [OptLevel::None, OptLevel::Speed];

/// Rounds of every case after the first, which must return the live count to its start.
const ROUNDS: u32 = 1000;

fn heap_program(program: &Program) -> hirsel_heapcheck::Program<Ready> {
    hirsel_heapcheck::Program {
        target: program.table.target.clone(),
        layouts: program.table.table.clone(),
        units: program.units.iter().map(|u| u.unit().clone()).collect(),
        statics: program.statics.clone(),
    }
}

fn bits(value: i64) -> Arg {
    Arg::Bits {
        scalar: ScalarKind::I64,
        bits: u128::from(value as u64),
    }
}

/// The heap checker's line for one case, as the harness prints it: `ok <i64>`, or the
/// primary failure's kind, rule code and site code.
fn checked(program: &hirsel_heapcheck::Program<Ready>, name: &str, a: i64, b: i64) -> String {
    let run = hirsel_heapcheck::run(
        program,
        &path(name),
        &[bits(a), bits(b)],
        &mut NoOperations,
        &Config::default(),
    );
    let audit = run
        .result
        .unwrap_or_else(|v| panic!("{name} {a} {b}: the heap checker rejects the run: {v:?}"));
    let observation = audit.observation;
    if let Some(failure) = observation.failures.first() {
        let codes = failure_codes();
        let code = codes.codes[&failure.rule];
        let site = site_code(
            &failure.definition,
            &revision(&failure.definition.0.name),
            SiteRange {
                source: failure.source,
                start: failure.site.0,
                end: failure.site.1,
            },
        );
        return format!("fail {} {} {site:016x}", code.kind, code.rule);
    }
    match observation.returned {
        Some(Observed::Integer { bits, .. }) => format!("ok {}", bits as u64 as i64),
        other => panic!("{name}: an unexpected observation {other:?}"),
    }
}

fn expected(program: &Program) -> String {
    let heap = heap_program(program);
    let mut out = String::new();
    for (name, pairs) in READY_CASES {
        for &(a, b) in *pairs {
            let _ = writeln!(out, "{name} {a} {b} {}", checked(&heap, name, a, b));
        }
    }
    // C passes `&m` for `bump(mut x)`: the callee writes through it; an overflow fails
    // before the write.
    let sem1 = failure_codes().codes["SEM-1"];
    let site = site_code(
        &path("bump"),
        &revision("bump"),
        SiteRange {
            source: 0,
            start: 10,
            end: 20,
        },
    );
    let _ = writeln!(out, "c bump 41 ok 42");
    let _ = writeln!(
        out,
        "c bump max fail {} {} {site:016x} m {}",
        sem1.kind,
        sem1.rule,
        i64::MAX
    );
    // C passes handles 12 and 13 (a taken handle goes by address) to `close_held(h, 5)`; the harness's Io adapter fails 13,
    // recording LANG-9 at the close's anchor, and the box is released all the same.
    let lang9 = failure_codes().codes["LANG-9"];
    let site = site_code(
        &path("close_held"),
        &revision("close_held"),
        SiteRange {
            source: 0,
            start: 80,
            end: 90,
        },
    );
    let _ = writeln!(out, "c close 12 ok 5");
    let _ = writeln!(
        out,
        "c close 13 fail {} {} {site:016x}",
        lang9.kind, lang9.rule
    );
    let _ = writeln!(out, "closes 2");
    let _ = writeln!(out, "live after one round 0");
    let _ = writeln!(out, "rounds {ROUNDS} live 0");
    // The harness runs every case under the runtime's diagnostic mode and again in
    // production mode: the lines are the same.
    format!("mode diagnostic\n{out}mode production\n{out}")
}

/// The C side: a table of every case, run once with its observations printed, then
/// `ROUNDS` more times, the runtime's live block count checked after each pass.
fn harness(program: &Program) -> String {
    let mut c = String::from(
        "#include <inttypes.h>\n#include <stdint.h>\n#include <stdio.h>\n#include <string.h>\n\
         #include \"hirsel_rt_v2.h\"\n\
         typedef hirsel_rt_v2_ctx ctx_t;\n\
         typedef uint8_t (*entry_t)(ctx_t *, void *, int64_t, int64_t);\n",
    );
    // Each entry through a wrapper of its ABI v1 signature, the result pointer first.
    for name in READY_CASES
        .iter()
        .map(|(name, _)| *name)
        .chain(["bump", "close_held"])
    {
        c.push_str(&program.c_call(name, &format!("call_{name}")));
    }
    let lang9 = failure_codes().codes["LANG-9"];
    let _ = write!(
        c,
        "static unsigned closes;\n\
         static uint8_t close_probe(ctx_t *c, uint64_t handle, uint64_t anchor, uint64_t operation_site) {{\n\
           closes++;\n\
           if (handle != 13) return 0;\n\
           hirsel_rt_v2_failure_begin(c, {}, {}, anchor, operation_site);\n\
           return 1;\n\
         }}\n",
        lang9.kind, lang9.rule
    );
    c.push_str(
        "struct ready_case { const char *name; entry_t entry; int64_t a, b; };\n\
         static const struct ready_case cases[] = {\n",
    );
    for (name, pairs) in READY_CASES {
        for &(a, b) in *pairs {
            // INT64_MIN has no literal: spell every argument as an unsigned bit pattern.
            let _ = writeln!(
                c,
                "  {{\"{name}\", call_{name}, (int64_t){}ULL, (int64_t){}ULL}},",
                a as u64, b as u64
            );
        }
    }
    let _ = write!(
        c,
        "}};\n\
         static ctx_t *ctx;\n\
         static uint64_t live(void) {{\n\
           hirsel_rt_v2_stats stats;\n\
           hirsel_rt_v2_ctx_stats(ctx, &stats);\n\
           return stats.live_blocks;\n\
         }}\n\
         static int run_mode(const char *mode, uint32_t flags) {{\n\
           printf(\"mode %s\\n\", mode);\n\
           closes = 0;\n\
           hirsel_rt_v2_config config;\n\
           memset(&config, 0, sizeof config);\n\
           config.abi_version = HIRSEL_RT_V2_ABI_VERSION;\n\
           config.flags = flags;\n\
           config.close_handle = close_probe;\n\
           if (hirsel_rt_v2_ctx_create(&config, &ctx) != 0) {{ printf(\"no context\\n\"); return 1; }}\n\
           uint64_t base = live();\n\
           size_t n = sizeof cases / sizeof cases[0];\n\
           for (size_t i = 0; i < n; i++) {{\n\
             int64_t r = 0;\n\
             uint8_t s = cases[i].entry(ctx, &r, cases[i].a, cases[i].b);\n\
             printf(\"%s %\" PRId64 \" %\" PRId64 \" \", cases[i].name, cases[i].a, cases[i].b);\n\
             hirsel_rt_v2_failure f;\n\
             if (s == 0) printf(\"ok %\" PRId64 \"\\n\", r);\n\
             else if (s == 1 && hirsel_rt_v2_failure_primary(ctx, &f)) {{\n\
               printf(\"fail %u %u %016\" PRIx64 \"\\n\", f.kind, f.rule, f.site);\n\
               hirsel_rt_v2_failure_clear(ctx);\n\
             }} else printf(\"bad status %u\\n\", (unsigned)s);\n\
           }}\n\
           int64_t m = 41;\n\
           if (call_bump(ctx, &m) == 0) printf(\"c bump 41 ok %\" PRId64 \"\\n\", m);\n\
           m = INT64_MAX;\n\
           hirsel_rt_v2_failure bf;\n\
           if (call_bump(ctx, &m) == 1 && hirsel_rt_v2_failure_primary(ctx, &bf)) {{\n\
             printf(\"c bump max fail %u %u %016\" PRIx64 \" m %\" PRId64 \"\\n\", bf.kind, bf.rule, bf.site, m);\n\
             hirsel_rt_v2_failure_clear(ctx);\n\
           }}\n\
           uint64_t open = 12, failing = 13;\n\
           int64_t k = 0;\n\
           if (call_close_held(ctx, &k, &open, 5) == 0) printf(\"c close 12 ok %\" PRId64 \"\\n\", k);\n\
           hirsel_rt_v2_failure cf;\n\
           if (call_close_held(ctx, &k, &failing, 5) == 1 && hirsel_rt_v2_failure_primary(ctx, &cf)) {{\n\
             printf(\"c close 13 fail %u %u %016\" PRIx64 \"\\n\", cf.kind, cf.rule, cf.site);\n\
             hirsel_rt_v2_failure_clear(ctx);\n\
           }}\n\
           printf(\"closes %u\\n\", closes);\n\
           printf(\"live after one round %\" PRIu64 \"\\n\", live() - base);\n\
           for (unsigned round = 0; round < {ROUNDS}; round++) {{\n\
             for (size_t i = 0; i < n; i++) {{\n\
               int64_t r = 0;\n\
               if (cases[i].entry(ctx, &r, cases[i].a, cases[i].b) != 0) hirsel_rt_v2_failure_clear(ctx);\n\
             }}\n\
           }}\n\
           printf(\"rounds {ROUNDS} live %\" PRIu64 \"\\n\", live() - base);\n\
           hirsel_rt_v2_ctx_destroy(ctx);\n\
           return 0;\n\
         }}\n\
         int main(void) {{\n\
           if (run_mode(\"diagnostic\", HIRSEL_RT_V2_CONFIG_DIAGNOSTIC) != 0) return 1;\n\
           return run_mode(\"production\", 0);\n\
         }}\n"
    );
    c
}

fn emit(program: &Program, opt: OptLevel) -> Vec<u8> {
    let input = input();
    let inputs = EmitInputs {
        index: None,
        verify_backend: true,
        input: &input,
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &program.statics,
        inline_instances: &std::collections::BTreeSet::new(),
        available_externally: &std::collections::BTreeSet::new(),
        callbacks: false,
        tracking: false,
        opt,
        measurement: false,
        fuel: false,
    };
    emit_object(&program.units(), &inputs)
        .unwrap_or_else(|e| panic!("{}: {e}", program.table.target.triple))
        .bytes
}

#[test]
fn the_ready_probes_emit_and_the_heap_checker_audits_them() {
    for target in TargetSpec::executed() {
        let program = ready_program(&target);
        for opt in OPTS {
            emit(&program, opt);
        }
        // Every case runs to a clean audit.
        let expected = expected(&program);
        // The revision 7 probes observe what they wrote: every bit set, or MEM-14 past
        // the flat list's end.
        let mem14 = failure_codes().codes["MEM-14"];
        for line in [
            "seq_push 3 4 ok 31",
            "seq_push 5 5 ok 31",
            "seq_push -1 9223372036854775807 ok 31",
            "flat_walk 1 0 ok 3",
        ] {
            assert!(
                expected.lines().any(|l| l == line),
                "no line `{line}` in\n{expected}"
            );
        }
        let past = format!("flat_walk 2 0 fail {} {} ", mem14.kind, mem14.rule);
        assert!(expected.lines().any(|l| l.starts_with(&past)), "{expected}");
    }
}

#[test]
#[ignore = "native: run by tools/check-native.sh"]
fn every_target_executes_the_ready_probes_as_the_heap_checker_does() {
    for target in TargetSpec::executed() {
        let program = ready_program(&target);
        let expected = expected(&program);
        for opt in OPTS {
            let dir = scratch(&format!("ready-{}-{opt:?}", target.triple));
            let bytes = emit(&program, opt);
            let source = dir.join("ready_harness.c");
            std::fs::write(&source, harness(&program)).unwrap();
            let executable = link(
                &target,
                &dir,
                &[("ready.o", &bytes), ("program-glue.o", &program.glue())],
                &[source],
            );
            assert_eq!(
                run(&target, &executable),
                expected,
                "{} {opt:?}",
                target.triple
            );
        }
    }
}
