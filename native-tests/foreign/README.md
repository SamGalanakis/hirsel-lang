# Foreign pointer boundary

`null.c` is linked to `conformance/PROG/PROG-4-ffi-null-api-ok.hir` by the
hirsel-driver `foreign::native_null_pointer_boundaries_match_on_both_backends` test.
It checks NULL and stable non-NULL pointers, C-origin function pointers, nullable
callback arguments/results and writable pointer arguments. Separate process launches
assert that non-null contract violations terminate before the Hirsel callback body
runs, with PROG-4 and the active extern call's source location. Both native backends
run the same programs on x86-64 and AArch64. A callback making a nested C call
checks that the outer extern site is restored before its next null argument.
