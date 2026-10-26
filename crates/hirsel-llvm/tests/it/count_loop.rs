//! Guarded counts agree with an independent scalar oracle at overflow boundaries.
//! Trip vectors 1/3/20 adapt LLVM 23.1.4 LoopVersioning's single-iteration.ll,
//! add-phi-update-users.ll and basic.ll to checked count reductions; extra
//! vectors straddle the SIMD width and u64 overflow boundary.
use crate::support::native::*;
use hirsel_lir::{emission::OptLevel, layout_types::TargetSpec};
use hirsel_llvm::compile_ir;

const COUNT: &str = r#"target triple = "x86_64-unknown-linux-gnu"
declare {i64,i1} @llvm.uadd.with.overflow.i64(i64,i64)
define i64 @count(ptr %tags, i64 %n, i64 %initial) {
entry:
 %empty=icmp eq i64 %n,0
 br i1 %empty,label %exit,label %ph
ph:
 br label %loop
loop:
 %i=phi i64 [0,%ph],[%next,%latch]
 %c=phi i64 [%initial,%ph],[%carry,%latch]
 %at=getelementptr i8,ptr %tags,i64 %i
 %tag=load i8,ptr %at,align 1
 %pair=icmp eq i8 %tag,1
 br i1 %pair,label %add,label %latch
add:
 %a=call {i64,i1} @llvm.uadd.with.overflow.i64(i64 %c,i64 1)
 %sum=extractvalue {i64,i1} %a,0
 %overflow=extractvalue {i64,i1} %a,1
 br i1 %overflow,label %fail,label %latch
latch:
 %carry=phi i64 [%c,%loop],[%sum,%add]
 %next=add nuw i64 %i,1
 %done=icmp eq i64 %next,%n
 br i1 %done,label %exit,label %loop
exit:
 %answer=phi i64 [%initial,%entry],[%carry,%latch]
 ret i64 %answer
fail:
 ret i64 42
}
"#;

#[test]
fn count_loop_fast_and_checked_paths_preserve_boundary_results() {
    for target in TargetSpec::executed() {
        for opt in [OptLevel::None, OptLevel::Default] {
            let dir = scratch(&format!("count-loop-{}-{opt:?}", target.triple));
            let source = COUNT.replace("x86_64-unknown-linux-gnu", &target.triple);
            let object = compile_ir(&source, &target, opt).unwrap();
            let harness = dir.join("count.c");
            std::fs::write(
                &harness,
                r#"
#include <stdint.h>
#include <stdio.h>
extern uint64_t count(const uint8_t *, uint64_t, uint64_t);
int main(void) {
  uint8_t tags[257];
  const unsigned sizes[] = {0,1,2,3,4,17,20,257};
  for (unsigned seed=0; seed<4; seed++) {
    for (unsigned i=0; i<257; i++) tags[i]=(i*7+seed)%3;
    for (unsigned cell=0; cell<sizeof sizes/sizeof *sizes; cell++) {
      unsigned n=sizes[cell], matches=0;
      for (unsigned i=0; i<n; i++) matches += tags[i]==1;
      const uint64_t starts[]={0, UINT64_MAX, UINT64_MAX-n,
        UINT64_MAX-matches, UINT64_MAX-matches+(matches!=0)};
      for (unsigned s=0; s<sizeof starts/sizeof *starts; s++) {
        uint64_t initial=starts[s];
        uint64_t expected=initial > UINT64_MAX-matches ? 42 : initial+matches;
        if (count(tags,n,initial)!=expected) return 1;
      }
    }
  }
  puts("160 count boundaries passed");
  return 0;
}
"#,
            )
            .unwrap();
            let executable = link(&target, &dir, &[("count.o", &object.bytes)], &[harness]);
            assert_eq!(run(&target, &executable), "160 count boundaries passed\n");
        }
    }
}
