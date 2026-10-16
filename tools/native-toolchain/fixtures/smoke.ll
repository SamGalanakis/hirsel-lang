; Toolchain smoke fixture: hand-written IR in the shapes native code relies on, compiled by
; hirsel-llvm for every target and linked against smoke_harness.c.
;
; It sets no triple or DataLayout; the compiler sets the target's. Each `smoke_*` function
; follows the native status convention: an i8 status return (0 success,
; 1 failure) and the result through an out pointer. Arithmetic is checked with overflow
; intrinsics, division and shifts are guarded before the instruction, so no path reaches
; poison or undefined behaviour. No instruction carries fast-math flags.

%packed = type <{ i8, i64, i16 }>

; The C side of the object-to-C direction (defined in smoke_harness.c).
declare i64 @smoke_host_scale(i64)

declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)
declare { i128, i1 } @llvm.smul.with.overflow.i128(i128, i128)
declare double @llvm.fma.f64(double, double, double)

define i8 @smoke_add_i64(ptr %out, i64 %a, i64 %b) {
entry:
  %r = call { i64, i1 } @llvm.sadd.with.overflow.i64(i64 %a, i64 %b)
  %overflow = extractvalue { i64, i1 } %r, 1
  br i1 %overflow, label %fail, label %ok
ok:
  %sum = extractvalue { i64, i1 } %r, 0
  store i64 %sum, ptr %out
  ret i8 0
fail:
  ret i8 1
}

define i8 @smoke_div_i64(ptr %out, i64 %a, i64 %b) {
entry:
  %zero = icmp eq i64 %b, 0
  %minus_one = icmp eq i64 %b, -1
  %min = icmp eq i64 %a, -9223372036854775808
  %overflow = and i1 %minus_one, %min
  %bad = or i1 %zero, %overflow
  br i1 %bad, label %fail, label %ok
ok:
  %q = sdiv i64 %a, %b
  store i64 %q, ptr %out
  ret i8 0
fail:
  ret i8 1
}

define i8 @smoke_shl_u64(ptr %out, i64 %a, i64 %n) {
entry:
  %bad = icmp uge i64 %n, 64
  br i1 %bad, label %fail, label %ok
ok:
  %v = shl i64 %a, %n
  store i64 %v, ptr %out
  ret i8 0
fail:
  ret i8 1
}

define i8 @smoke_mul_i128(ptr %out, ptr %a, ptr %b) {
entry:
  %x = load i128, ptr %a, align 16
  %y = load i128, ptr %b, align 16
  %r = call { i128, i1 } @llvm.smul.with.overflow.i128(i128 %x, i128 %y)
  %overflow = extractvalue { i128, i1 } %r, 1
  br i1 %overflow, label %fail, label %ok
ok:
  %product = extractvalue { i128, i1 } %r, 0
  store i128 %product, ptr %out, align 16
  ret i8 0
fail:
  ret i8 1
}

; tag + value + extra of a packed (unaligned) struct read through its address.
define i8 @smoke_packed_sum(ptr %out, ptr %p) {
entry:
  %tag_at = getelementptr inbounds %packed, ptr %p, i32 0, i32 0
  %value_at = getelementptr inbounds %packed, ptr %p, i32 0, i32 1
  %extra_at = getelementptr inbounds %packed, ptr %p, i32 0, i32 2
  %tag = load i8, ptr %tag_at, align 1
  %value = load i64, ptr %value_at, align 1
  %extra = load i16, ptr %extra_at, align 1
  %tag64 = zext i8 %tag to i64
  %extra64 = zext i16 %extra to i64
  %small = add nuw nsw i64 %tag64, %extra64
  %r = call { i64, i1 } @llvm.sadd.with.overflow.i64(i64 %value, i64 %small)
  %overflow = extractvalue { i64, i1 } %r, 1
  br i1 %overflow, label %fail, label %ok
ok:
  %sum = extractvalue { i64, i1 } %r, 0
  store i64 %sum, ptr %out
  ret i8 0
fail:
  ret i8 1
}

; smoke_host_scale(a) + 1, checked: the call into C and the status edge after it.
define i8 @smoke_call_host(ptr %out, i64 %a) {
entry:
  %h = call i64 @smoke_host_scale(i64 %a)
  %r = call { i64, i1 } @llvm.sadd.with.overflow.i64(i64 %h, i64 1)
  %overflow = extractvalue { i64, i1 } %r, 1
  br i1 %overflow, label %fail, label %ok
ok:
  %sum = extractvalue { i64, i1 } %r, 0
  store i64 %sum, ptr %out
  ret i8 0
fail:
  ret i8 1
}

; mul_add is the one fused operation.
define double @smoke_mul_add(double %a, double %b, double %c) {
entry:
  %r = call double @llvm.fma.f64(double %a, double %b, double %c)
  ret double %r
}

; A multiply then an add stay two roundings: the optimizer must not contract them.
define double @smoke_mul_then_add(double %a, double %b, double %c) {
entry:
  %p = fmul double %a, %b
  %r = fadd double %p, %c
  ret double %r
}

; Subnormal results survive (no flush to zero).
define double @smoke_div_f64(double %a, double %b) {
entry:
  %r = fdiv double %a, %b
  ret double %r
}
