//! Native checks of chunk, physical-quota and source-exit boundaries.
use crate::support::native::*;
use hirsel_lir::{emission::OptLevel, layout_types::TargetSpec};
use hirsel_llvm::compile_ir;

#[test]
fn scalar_chunks_preserve_results_and_every_physical_quota_boundary() {
    let source = include_str!("../fixtures/ahead_loop.ll");
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    for opt in [OptLevel::None, OptLevel::Default, OptLevel::Speed] {
        let dir = scratch(&format!("ahead-chunks-{opt:?}"));
        let object = compile_ir(source, &target, opt).unwrap();
        let harness = dir.join("ahead.c");
        std::fs::write(
            &harness,
            r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
extern uint64_t recurrence_ahead(uint64_t *, uint64_t, uint64_t);
static uint64_t calls;
static uint64_t grant(uint64_t call) {
  static const uint64_t words[] = {1,33,1024,7,32,2};
  return words[call % 6];
}
void hirsel_rt_v2_ahead_handoff(uint64_t *state, uint32_t reason) {
  if (reason != 1 || *state != 0) abort();
  *state = grant(++calls);
}
int main(void) {
  const uint64_t sizes[] = {0,1,31,32,33,34,63,64,65,1023,1024,1025,4097};
  const uint64_t quotas[] = {1,2,31,32,33,34,1023,1024,INT64_MAX};
  unsigned cells = 0;
  for (unsigned seed = 1; seed <= 4; ++seed)
    for (unsigned n = 0; n < sizeof sizes / sizeof *sizes; ++n)
      for (unsigned q = 0; q < sizeof quotas / sizeof *quotas; ++q) {
        uint64_t x = seed, expected = quotas[q], handoffs = 0;
        for (uint64_t i = 0; i <= sizes[n]; ++i) {
          if (--expected == 0) expected = grant(++handoffs);
          if (i != sizes[n]) x = (x ^ (x >> 7)) * UINT64_C(0x9e3779b97f4a7c15) + i;
        }
        uint64_t state = quotas[q];
        calls = 0;
        uint64_t got = recurrence_ahead(&state, seed, sizes[n]);
        if (got != x || state != expected || calls != handoffs) {
          fprintf(stderr, "seed=%u n=%llu quota=%llu calls=%llu/%llu left=%llu/%llu\n",
            seed, (unsigned long long)sizes[n], (unsigned long long)quotas[q],
            (unsigned long long)calls, (unsigned long long)handoffs,
            (unsigned long long)state, (unsigned long long)expected);
          return 1;
        }
        ++cells;
      }
  printf("%u chunk boundaries passed\n", cells);
  return 0;
}
"#,
        )
        .unwrap();
        let object_path = dir.join("ahead.o");
        std::fs::write(&object_path, &object.bytes).unwrap();
        let executable = dir.join("program");
        output(
            std::process::Command::new(llvm_prefix().join("bin/clang"))
                .arg(format!(
                    "--ld-path={}",
                    llvm_prefix().join("bin/ld.lld").display()
                ))
                .args(["-std=c11", "-O1", "-Wall", "-Werror"])
                .arg(harness)
                .arg(object_path)
                .arg("-o")
                .arg(&executable),
        );
        assert_eq!(run(&target, &executable), "468 chunk boundaries passed\n");
    }
}

#[test]
fn private_memory_chunks_preserve_stores_partial_exits_and_checkpoint_positions() {
    let source = include_str!("../fixtures/ahead_memory_loop.ll");
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    for opt in [OptLevel::None, OptLevel::Default, OptLevel::Speed] {
        let dir = scratch(&format!("ahead-memory-chunks-{opt:?}"));
        let object = compile_ir(source, &target, opt).unwrap();
        let harness = dir.join("ahead.c");
        std::fs::write(
            &harness,
            r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
extern uint64_t reserved_ahead(uint64_t *, uint64_t *, uint64_t, uint64_t);
static uint64_t rows[4097], expected_rows[4097], calls, positions[8196], expected_calls;
static uint64_t grant(uint64_t call) {
  static const uint64_t words[] = {1,33,1024,7,32,2};
  return words[call % 6];
}
static uint64_t prefix(const uint64_t *data) {
  uint64_t result = 0;
  for (unsigned i = 0; i < 4097; ++i) result += data[i];
  return result;
}
void hirsel_rt_v2_ahead_handoff(uint64_t *state, uint32_t reason) {
  if (reason != 1 || *state != 0 || calls >= expected_calls ||
      prefix(rows) != positions[calls]) abort();
  *state = grant(++calls);
}
int main(void) {
  const uint64_t sizes[] = {0,1,31,32,33,34,63,64,65,1023,1024,1025,4097};
  const uint64_t quotas[] = {1,2,31,32,33,34,63,64,65,1023,1024,INT64_MAX};
  const uint64_t stops[] = {0,1,31,32,33,63,64,65,1023,1024,UINT64_MAX};
  const uint64_t initial[] = {7,UINT64_MAX / 64};
  unsigned cells = 0;
  for (unsigned v = 0; v < sizeof initial / sizeof *initial; ++v)
  for (unsigned n = 0; n < sizeof sizes / sizeof *sizes; ++n)
    for (unsigned q = 0; q < sizeof quotas / sizeof *quotas; ++q)
      for (unsigned s = 0; s < sizeof stops / sizeof *stops; ++s) {
        for (unsigned i = 0; i < 4097; ++i) rows[i] = expected_rows[i] = initial[v];
        uint64_t expected = quotas[q], total = 0;
        expected_calls = 0;
        for (uint64_t i = 0;; ++i) {
          if (--expected == 0) {
            positions[expected_calls] = prefix(expected_rows);
            expected = grant(++expected_calls);
          }
          if (i == sizes[n]) break;
          if (--expected == 0) {
            positions[expected_calls] = prefix(expected_rows);
            expected = grant(++expected_calls);
          }
          if (i == stops[s]) break;
          uint64_t value = expected_rows[i] += i;
          if (UINT64_MAX - total < value) break;
          total += value;
        }
        uint64_t state = quotas[q];
        calls = 0;
        uint64_t got = reserved_ahead(&state, rows, sizes[n], stops[s]);
        if (got != total || state != expected || calls != expected_calls) abort();
        for (unsigned i = 0; i < 4097; ++i) if (rows[i] != expected_rows[i]) abort();
        ++cells;
      }
  printf("%u memory chunk boundaries passed\n", cells);
  return 0;
}
"#,
        )
        .unwrap();
        let object_path = dir.join("ahead.o");
        std::fs::write(&object_path, &object.bytes).unwrap();
        let executable = dir.join("program");
        output(
            std::process::Command::new(llvm_prefix().join("bin/clang"))
                .arg(format!(
                    "--ld-path={}",
                    llvm_prefix().join("bin/ld.lld").display()
                ))
                .args(["-std=c11", "-O1", "-Wall", "-Werror"])
                .arg(harness)
                .arg(object_path)
                .arg("-o")
                .arg(&executable),
        );
        assert_eq!(
            run(&target, &executable),
            "3432 memory chunk boundaries passed\n"
        );
    }
}
