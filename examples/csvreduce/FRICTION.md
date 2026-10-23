# First-contact friction

Elapsed investigation times are approximate and exclude unrelated build waits.
Diagnostics below came from retained failed drafts; no runtime/std repair was made.

| Minimal source | Exact diagnostic or behavior | Resolution / cost | Minutes | Status / owner |
| --- | --- | --- | ---: | --- |
| `let mut result = empty()` | LANG-14: `` `result` is reserved for the returned value in a postcondition; choose another name `` | Rename the local to `summary`; no runtime cost. | 1 | intended rule / application |
| In a loop: `list.push(mut chunks, Chunk(rows: input, ...))` | OWN-2: the value is moved or not initialised on every path: `` `input` is not initialised on every path, but it is read here `` | `Chunk(rows: copy input, ...)` explicitly retains one Shared root per descriptor; at most 16 root copies, no table copy per worker. | 2 | intended rule / application |

The parsed table now comes from `csv.parse_owned` with deep ownership evidence.
Header and width validation borrow it, then `shared.freeze(take rows)` transfers
it without a completed-table deep copy. The previous bounded workaround,
`csv.parse` followed by `shared.freeze(unique rows)`, copied the whole table
within the 256 KiB input bound; it has been removed. The constructor reuses the
strict SliceReader transitions, materializes each final field into fresh storage
once, and grows fresh row/table lists. Escape/CRLF decoding still uses per-record
scratch, and allocator capacity slack remains. These construction costs are not
zero-copy parsing. `Shared` avoids copying the full table per chunk; descriptors
still retain at most 16 Shared roots. The pure callback takes only an inert
descriptor and returns a plain summary or a payload-free error.

The CLI argument conversion, explicit CSV width checks, byte/record/cell limits
and JSON output are ordinary application plumbing. The fixed four-element
`map_all` control and prior counted-error/Receiver diagnostic controls are
already accepted or intended/fixed controls, not new runtime failures. The old
csvstats compiler thread-limit documentation issue is not a csvreduce defect.

Production trace export initially returned exit 3 after printing the correct
summary: `hirsel-rt: launch refused: incomplete canonical event evidence`.
Minimal source: `pub fn main(args: Args) -> Result[(), ()] {
failure.check(arg.len(args) == 0, Arithmetic()); Ok(()) }`, launched with
`--hirsel-trace-events`, cooperative FIFO, width 1. Output-only and map_all square
controls passed. Approximately 10 minutes to isolate; owner
`e1-trace-record-27`. This is a canonical observer defect, separate from the
application's aggregation. Production recordings without trace export passed at
live-observed widths 1/2/4 in both backends; trace-enabled receipts are tracked
separately. No duplicate runtime repair is made here.
