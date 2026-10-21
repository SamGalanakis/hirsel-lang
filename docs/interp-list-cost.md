# Synchronous list completion cost

`list.push` constructs its `OperationResult::Complete` directly. Its result is
still Unit, its consumed argument list is `[1]`, and its replacement and release
lists are empty. The mutable owner is updated in place. Argument extraction,
collection validation and mutation, interpreter completion, cleanup, tracing,
step limits, fuel and logical memory charges keep their existing paths.

The previous `returned(Unit).transferring([1])` first constructed an empty
completion and extended its consumed vector. In the measured profiling build,
that emitted a vector reserve call and a 208-byte result copy. Direct construction
removes those calls and reduces the handler's stack reservation from 264 to 120
bytes. The consumed vector still allocates; no allocation-call reduction is
claimed.

The container prior art is Rust
`a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`library/alloc/src/macros.rs:43-62` (known-length vector construction),
`library/alloc/src/vec/spec_extend.rs:22-28` and
`library/alloc/src/vec/mod.rs:4234` (trusted iterator extension).
List storage continues to use safe standard `Vec::push`
(`library/alloc/src/vec/mod.rs:959-961`). No container algorithm or unsafe code
is introduced, and there are no semantic deviations from these standard APIs.

## Controls and measurement

The lane's interpreter tests cover exact transfer fields, owned and nested
values, shared snapshots, extra arguments, malformed-call ordering and state,
and every step cut around a real mutation. Existing completion tests compare
immediate and scheduled execution across traces, checked execution, step limits,
fuel and memory budgets. Selected MEM-6 and DROP-2 cases cover bounded-arena
exhaustion, shared/nested writes, resources and cleanup on the interpreter and
both native backends with the diagnostic runtime.

The frozen controls sum one million integers, perform 8,000 list pushes, and run
the tasks package tests. Scalar and list counts subtract their zero-iteration
controls and divide by 58,000,000 and 416,000 additional steps respectively.
Ten seeded, shuffled interleaved rounds use matched compiler/std inputs on
CPU31, with separate user and total instruction counters. All samples, including
first-use std setup outliers, remain in the session; the table reports medians.

| Workload | User instructions/step before | After | Total instructions/step before | After |
| --- | ---: | ---: | ---: | ---: |
| Scalar | 77.40971 | 77.39297 | 77.47645 | 77.45637 |
| List pushes | 160.74378 | 157.68171 | 174.19862 | 171.54266 |

The list reduction is 1.90% user and 1.52% total. Scalar changes are below 0.03%.
The operation/plain ratios are **2.037416x user and 2.214700x total**; the twofold
target remains unmet. Remaining argument preparation and completion costs lie
outside this local constructor change. These instruction results make no time
speedup claim.

The tasks control passed 25 tests in each of ten processes per implementation.
Median full-process instructions changed from 50,380,955,294 to 50,307,026,986
user and 50,810,893,268.5 to 50,750,294,812 total. The paired 95% ratio intervals
are [0.993629, 1.005393] user and [0.994069, 1.005149] total. The task-clock ratio
is 1.021 with interval [0.988, 1.119]; no tasks time improvement is claimed.
The coordinator reported that the tasks session overlapped another lane's
potentially unlocked CPU31 measurements. All samples remain retained. This
limits cycle/time attribution; the recorded instruction result and unmet
twofold target stand separately.
