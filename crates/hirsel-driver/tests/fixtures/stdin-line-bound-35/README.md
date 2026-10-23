The delimiter-before-full structure and short/long/remainder vectors adapt Go
`src/bufio/bufio.go:338–386` and `bufio_test.go:918–1021` at
`ebe2feefc400f813aefdc8a036e66d2619da6649`. Copyright 2009 The Go Authors.
BSD-3-Clause (Go), full license in `third-party/NOTICES.md`.
The owned loop and error wrapper follow Hirsel's MIT-licensed `input.hir`,
`file.hir` and Sheet `main.hir` at `a66f928db35513624c09dddb637feaa4d3e7fe22`.
Hirsel's MIT notice remains in the repository LICENSE.

Adaptations: owned String replaces a recyclable byte view; LF is retained and
CR counts as content; EOF without data is None; UTF-8 is checked only after
delimiting; overflow returns no partial data and consumes exactly the first
rejected non-LF byte; no buffering, prefix return, unread or drain is added.
The direct read checks the remaining suffix, rather than assuming recovery at
the next line. One-byte requests exercise Go's stride-one vector structure.
The MAX limit uses tiny input, so no allocation is proportional to the cap.

The Rust harness retains actual native recordings before scratch cleanup when
`STDIN_LINE_BOUND_35_EVIDENCE` is set. Trace instrumentation uses the production
source and emits each probe length and pre-append buffer length/byte; an
independent prefix oracle checks the logical bound. This is not an RSS bound:
List capacity growth and allocator slack remain implementation-dependent.

The native stream-error rows use a real raw PTY master: after queued bytes,
closing its slave produces EIO, which the existing host provider translates to
InputFault(1). This tests errors after accepted/invalid prefixes without
inventing completion events. Pending cancellation uses a real open stdin pipe
and the existing scripted-signal grant; the live root reports the canceled
child's result because output from the child would itself be canceled.

Qualification can pass `STDIN_LINE_BOUND_35_RESUME` as JSON `[passed, tapes]`:
exact cell names already accepted, and hex-encoded original recordings for
their native recorders. Reused cells are logged separately; skipped native
recordings require the original tape, whose exact requests are still checked.
The default run selects every cell. This is a recovery aid, not a replacement
for retaining the original logs and recordings.
