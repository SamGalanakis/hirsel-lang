# relquery

A bounded relational CLI. Run `relquery run data plan.json 1` or
`relquery explain data plan.json 1`; jobs is an integer from 1 through 4.
Append `--interruptible` to race the entire workflow against Interrupt.
Grant the `data` directory, stdout, stderr and tasks; interruptible runs also
need signals. CSV destinations are `<query-name>.csv` inside that directory.
Success exits 0 and prints `published N\n` (explain prints the typed plan).
Domain, denial and delivered cancellation exit 2 with `relquery: <reason>\n`.
Missing stdout/stderr exits 3. No SQL parser, NULL or implicit coercion exists.

The version 1 document has exactly `version`, `tables`, `queries`. Tables have
`name`, `path`, `columns`; each column has `name`, `type` (`integer` or `text`).
Names use ASCII letters, digits and underscore, 1–48 bytes; paths are a name
followed by `.csv`, matching the table name. Headers must exactly equal the declared column names in
order. Text is UTF-8, including empty text. Integers use optional minus and one
or more decimal digits in the i64 domain; no plus or whitespace.

Queries have `name`, `nodes`. Nodes form a consuming linear pipeline:

| op | other required fields | meaning |
| --- | --- | --- |
| scan | table | first node; columns become `table.column` |
| join | table, left, right | inner equi-join; right names belong to the new table |
| select | column, value | equality to a JSON integer or string of the same type |
| project | columns | ordered list of `{column, name}`; aliases become current names |
| group | keys, sum, count, total | keys are current names; sum must be integer; count/total name integer outputs |
| order | columns, descending | lexicographic stable typed ordering |

Unknown fields, duplicate names/aliases/keys, unknown columns, unsupported types
and repeated joins are errors. Projection/order lists contain 1–8 names; group
keys contain 0–6 names. Intermediate schemas contain at most 8 columns.
Groups appear in first encounter order and use checked i64 count/sum; an empty
ungrouped aggregate is `(0,0)`, an empty keyed aggregate has no rows. Order ties
use the incoming row's ordinal; joins enumerate left-major/right-minor pairs.
Projection and filtering preserve ordinals, groups retain first encounter.

Bounds: 4 tables, 64 data rows per table, 8 columns, 4096 candidate join pairs
checked before materialization, 32 nodes total, 8 queries, 65536 total input
bytes (JSON plus each declared CSV once), 65536 total encoded output bytes,
4 jobs. A 65th row is rejected even if filtered away. The 4097-pair boundary
cannot be admitted with two <=64-row inputs: its original vector therefore
has 65x64 rows and is rejected at row admission. This does not prove a separate
4097-pair execution cell. No file metadata is trusted for input bounds.

All schemas and nodes are resolved before reading CSV. Explain validates and
prints resolved operator indexes, schemas and literals without opening tables
or publishing. Run preloads tables once, maps independent owned query work in
batches of jobs, joins results in query order, checks the aggregate output cap,
then publishes in that order. Each destination is atomic; the collection of
CSV files is not a filesystem transaction. A domain error publishes no file;
a later publication I/O error can leave earlier complete query files.

`Row[String]` retains decoded CSV fields, `Row[Scalar]` holds resolved execution
rows; generic selection/project helpers own their cursor and borrow callbacks.
Aggregate state is uniquely owned. Text and integer equality and ordering use
their actual domains, including text `"01"` versus integer `1`.

Prior art: SQLite commit `014644d8f25501d0526c65f1024154ba182f6899`,
`src/select.c:1193–1250` (projection/destination order),
`test/select1.test:20–85` (missing table and positional projection vectors).
SQLite is public domain; no license restriction applies to those structures.
This port uses typed operators instead of VDBE, rejects duplicate output names,
omits SQL parsing/NULL/coercions, checks overflow, and exposes bounded atomic
CSV destinations rather than SQLite transactions. The independent SQLite translator and exhaustive nested-loop oracle remain
in the lane evidence directory; permanent fixtures contain their frozen output
bytes and pinned source identity. Original outputs precede implementation.

Qualification uses `relation_query_33` in the CLI harness. The original29
preimplementation vectors and six supplemental cases live in its named support
directory. Original interpreter cells completed102 workflows at jobs1/2/4;
eight cap cells stopped at the unchanged Steps1000000 limit. Those remain
incomplete. Physical Ordered widths1/2/4 are individually incomplete pending
exact-CPU deployment owned by @agentlang-24; jobs arguments prove no capacity.
Native-only cap results are compared to independently frozen expected results,
not called four-engine agreement. See FRICTION.md for witnesses and costs.

Native qualification completed35 scenarios on LLVM and Cranelift:70 retained
recordings,140 native cross-replays,124 completed interpreter replays and16
step-limited interpreter replays. Six supplemental scenarios passed36 ordinary
interpreter cells. The original returned-record diagnostic obligation for a late
suffix refusal remains owned by native-file-faults-33; its exact raw artifact
is retained, while status and stdout-prefix checks passed.

To use the checked-in sample, run from this directory:
`hirsel run hirsel.json --grants grants.json`. The grant maps virtual `data`
to `fixtures`; successful CSVs are published there. Build a standalone binary
with `hirsel build hirsel.json -o relquery` and use the same grants.
The independent 32/33 total-node controls were added separately when final
review tightened the limit across all queries, as required by the task bound.

Final missing-only checks add jobs1/4 workflows and jobs1 cap recordings, for118
native recordings overall,236 native cross-replays and204 completed host
interpreter replays.32 host replay cells and8 ordinary interpreter cells remain
at the unchanged step limit;18 physical capacity cells remain deployment-owned.
The sample also passed real-host recording and offline replay with output
sentinels unchanged. Historical tapes retain their own source/tool identities.
