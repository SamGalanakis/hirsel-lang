target triple = "x86_64-unknown-linux-gnu"
declare void @hirsel_rt_v2_ahead_handoff(ptr, i32)
define i64 @recurrence_ahead(ptr %state, i64 %seed, i64 %n) {
entry:
 %quota = load i64, ptr %state, align 8
 br label %header
header:
 %left = phi i64 [%quota, %entry], [%remaining, %body]
 %i = phi i64 [0, %entry], [%next, %body]
 %x = phi i64 [%seed, %entry], [%value, %body]
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
 %shift = lshr i64 %x, 7
 %xor = xor i64 %x, %shift
 %mul = mul i64 %xor, -7046029254386353131
 %value = add i64 %mul, %i
 %next = add i64 %i, 1
 br label %header
exit:
 store i64 %remaining, ptr %state, align 8
 ret i64 %x
}
