# mergeaudit

Plan a three-way text merge, review its conflict regions, and publish an
explicitly resolved result. From this directory with `hirsel` on PATH:

```sh
hirsel run hirsel.json --grants grants.json -- plan manifest.json plan.json
# The shipped decisions select left for the shipped fixture.
# After editing inputs, use the new plan binding and review every conflict ID.
hirsel run hirsel.json --grants grants.json -- resolve manifest.json decisions.json output
```

The manifest has exact fields `schema:1` and `files`, an array of 1–4 records
with `name`, `base`, `left`, `right` strings. All four are leaf filenames
(no separators, controls, colon, `.` or `..`, at most 128 UTF-8 bytes).
Sources and decisions live under the `input` grant; `name` independently names
the result under the destination Dir grant. Files are sorted by ordinal name.
The report name is under `output`; resolve's last argument selects the granted
Dir. No path is inferred from diff content.

A plan has `schema`, `binding`, and `files`. Each file contains `name` and
`regions`. Each region reports `[start,length]` line spans for `base`, `left`,
`right`, their exact `*_text` strings, `kind`, and an `id`. Kinds are `base`,
`left`, `right`, `identical` or `conflict`; only conflicts have nonempty IDs,
`name:zero_based_conflict_ordinal`. Deleted regions are retained, even with an
empty selected result, so all three inputs are covered exactly once.

The binding is SHA256 of canonical std JSON containing `manifest` (SHA256 of
the original manifest bytes) and `sources` (base/left/right SHA256 strings in
sorted file order). Changes to paths, manifest formatting or source bytes
invalidate old decisions. A decision envelope has exact fields `schema:1`,
`binding`, `choices`. Each choice has exact fields `id`, `choice`, `text`.
Choices are `base`, `left`, `right`, `manual`; `text` must be empty except for
manual choices. Every conflict needs exactly one choice. Unknown, duplicate,
missing or stale decisions refuse publication, including for otherwise clean
files. A clean plan still requires a matching binding and an empty choice list.

Alignment uses the shipped `std.diff.edits(..., MyersMinimal())`, including its
repeated-line sliding policy. The bounded overlap sweep ports node-diff3
`diff3.mjs:208–319` and its identical-replacement rule at lines 347–367, pinned
at `8226c27e074909241d72276c21215505a931665f`; see NOTICE. Touching intervals,
insertions at either boundary, deletion/modification and transitive overlaps
join a region. Equal-start hunks explicitly order left before right, then by
side ordinal. A bilateral region is clean only when its complete replacement
line sequences are byte equal. Repeated-line alignment can differ from upstream;
common-hunk tests isolate the region policy from either LCS tie choice.

Limits are checked before splitting or growing app structures: 32 KiB and 256
LF-delimited lines per UTF-8 source, 128 KiB aggregate source bytes (counting each
manifest reference), four triples, 128 conflicts/file, 16 KiB decisions and
256 KiB aggregate resolved output. A manifest is capped at 16 KiB. At most 512
edit runs/pair, 512 combined hunks/file and 1025 coverage regions/file derive
from the line bounds. CRLF, Unicode and missing final LF remain exact. JSON
parsing and rendering use std.json; reports include three content vectors. Their
fully escaped JSON, including
metadata, must also fit 256 KiB. The planner counts with std JSON semantics
before constructing the final encoded report; excess refuses publication.

Each independent file triple runs as an owned task; results are awaited in
manifest order. Publication begins after every input, binding, decision and
output bound succeeds. `create_atomic`/`write_all`/`commit` replaces each target
individually. Commit refusal removes its temporary and preserves the old file.
There is no multi-file transaction: a later publication fault may leave earlier
files published. The app never inserts conflict markers. `--interruptible`
adds a granted Interrupt subscription, races work against signal delivery and
joins canceled work, including Pending file reads.

Exit 0 means a successful plan (possibly with conflicts) or complete resolution.
Exit 2 means invalid data, stale decisions, denied authority, file error or
cancellation. stdout reports `mergeaudit: planned N files` or `resolved N files`;
stderr reports one deterministic `mergeaudit: KIND` line when granted. See
FRICTION.md for implementation costs and qualification limitations.
