target triple = "x86_64-unknown-linux-gnu"
declare void @hirsel_rt_v2_ahead_handoff(ptr, i32)
define i64 @reserved_ahead(ptr %state, ptr %rows, i64 %n, i64 %stop) {
entry:
 %quota = load i64, ptr %state, align 8
 br label %header
header:
 %left = phi i64 [%quota, %entry], [%remaining2, %write]
 %i = phi i64 [0, %entry], [%next, %write]
 %sum = phi i64 [0, %entry], [%total, %write]
 %dec = add i64 %left, -1
 %expired = icmp eq i64 %dec, 0
 br i1 %expired, label %handoff, label %test
handoff:
 store i64 0, ptr %state, align 8
 call void @hirsel_rt_v2_ahead_handoff(ptr %state, i32 1)
 %refill = load i64, ptr %state, align 8
 br label %test
test:
 %remaining = phi i64 [%dec, %header], [%refill, %handoff]
 %done = icmp eq i64 %i, %n
 br i1 %done, label %exit, label %body
body:
 %dec2 = add i64 %remaining, -1
 %expired2 = icmp eq i64 %dec2, 0
 br i1 %expired2, label %handoff2, label %check
handoff2:
 store i64 0, ptr %state, align 8
 call void @hirsel_rt_v2_ahead_handoff(ptr %state, i32 1)
 %refill2 = load i64, ptr %state, align 8
 br label %check
check:
 %remaining2 = phi i64 [%dec2, %body], [%refill2, %handoff2]
 %failed = icmp eq i64 %i, %stop
 br i1 %failed, label %exit, label %write
write:
 %at = getelementptr i64, ptr %rows, i64 %i
 %old = load i64, ptr %at, align 8
 %value = add i64 %old, %i
 store i64 %value, ptr %at, align 8
 %total = add i64 %sum, %value
 %next = add i64 %i, 1
 br label %header
exit:
 %result = phi i64 [%sum, %test], [%sum, %check]
 %rest = phi i64 [%remaining, %test], [%remaining2, %check]
 store i64 %rest, ptr %state, align 8
 ret i64 %result
}
