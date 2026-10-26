//! Guarded reductions retain a checked fallback and refuse unproved recurrences.
use super::*;

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
 ret i64 %c
}
"#;

fn transform(source: &str, target: TargetSpec) -> (bool, String) {
    let context = Context::new();
    let module = Module::parse(&context, "count-control", source).unwrap();
    let machine = TargetMachine::new(&target, true).unwrap();
    module.set_target(&machine);
    module.verify().unwrap();
    module.run_passes("default<O2>", &machine).unwrap();
    let changed = module.text().contains("count.headroom");
    module.verify().unwrap();
    (changed, module.text())
}

#[test]
fn a_guarded_conditional_count_vectorizes_and_keeps_its_checked_twin() {
    for target in TargetSpec::executed() {
        let source = COUNT.replace("x86_64-unknown-linux-gnu", &target.triple);
        let (changed, ir) = transform(&source, target);
        assert!(changed, "{ir}");
        assert!(ir.contains("llvm.vector.reduce.add"), "{ir}");
        assert!(ir.contains("count.checked"), "{ir}");
        assert!(
            ir.contains("call { i64, i1 } @llvm.uadd.with.overflow.i64"),
            "{ir}"
        );
    }
}

#[test]
fn unproved_count_updates_and_effectful_loops_keep_their_checks() {
    for source in [
        COUNT.replace("i64 %c,i64 1", "i64 %c,i64 2"),
        COUNT.replace(
            "%carry=phi i64 [%c,%loop],[%sum,%add]",
            "%carry=phi i64 [%initial,%loop],[%sum,%add]",
        ),
        COUNT.replace(
            "%tag=load i8,ptr %at,align 1",
            "%tag=load i8,ptr %at,align 1\n store volatile i8 %tag,ptr %at",
        ),
        COUNT.replace("%done=icmp eq i64 %next,%n", "%done=icmp eq i8 %tag,0"),
    ] {
        let (changed, ir) = transform(&source, TargetSpec::x86_64_unknown_linux_gnu());
        assert!(!changed, "{source}\n{ir}");
        assert!(!ir.contains("count.proven"), "{ir}");
    }
}
