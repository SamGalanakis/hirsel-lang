# Scoped CSV driver port

The driver reuses Hirsel's existing bounded, byte-at-a-time CSV parser. Its
state transitions derive from Go encoding/csv, pinned at
`ebe2feefc400f813aefdc8a036e66d2619da6649`:
`src/encoding/csv/reader.go:197-205` returns one record per Read, and
`:296-435` implements the quoting/field transitions. The inspected clone is
`/tmp/ref-go`. `CONC-3-stream-scoped-bounds-run.hir` ports Simple, CRLF,
RFC4180 quoting and NoEOLTest from `reader_test.go:45-86`, repeats them at
refill sizes 1–11, and adds Hirsel UTF-8, budget and typed-error cases.

Tokio `ce685ee6916549ed7e52dbfd5430858bbdc734d3`,
`tokio/src/task/coop/mod.rs:8-31` explains why an always-ready source can starve
other tasks; `:80-91` discusses checkpoint placement. The inspected clone is
`/tmp/ref-tokio`. The rejecting-source fixture adapts that always-ready loop:
its only notification happens on the first refill, every record is rejected,
and cancellation must stop it before a second 16-byte refill. A resource drop
checks the byte bound and witnesses source destruction before return.

Deviations are deliberate: Hirsel uses explicit Scope checks before each raw
record and bounded refill instead of a hidden scheduler budget; the calls are
deterministic scheduling points. Input/record/output limits are caller data,
source/parser/callback/check errors are separate typed variants, and owned
source cleanup replaces a separate stop/finally API. Existing CSV semantics
(blank rows and bare CR preserved, quoted CRLF preserved, no partial row on
error) remain unchanged. No new CSV dialect, runtime ABI or tape format is
introduced. The existing opaque stream carrier is named Flow because Stream
is the core socket handle; scoped parsing consumes S directly rather than
putting a Scope into that carrier.

The sketches call the configuration CsvStreamConfig; the compiled API calls it
csv.StreamConfig and places DriverError in csv so the parser implementation can
own its private Reader without exposing parser storage. stream.csv_fold_scoped
forwards to csv.fold_scoped with the same scoped callback/authority contracts.
No aliasing or new language feature is needed.
