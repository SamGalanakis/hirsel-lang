# sheet

A bounded line-oriented spreadsheet. Expressions use checked signed 64-bit + - * /,
parentheses, unary signs, A1–P16 references and ascending SUM(A1:B2) rectangles.
Division truncates toward zero. Blank cells read as zero. Errors remain in cells.

```
hirsel run examples/sheet/hirsel.json --grants examples/sheet/grants.json
hirsel run examples/sheet/hirsel.json --grants examples/sheet/grants.json -- --jobs 4 -
```

Commands, one per line: `set A1 = 2+3*4`, `clear A1`, `show`, `show A1`,
`recalc`, `save book.json`, `load book.json`, `quit`. No terminal control sequences
or prompts. Show recalculates; recalc reports the number of invalidated cells.
Output rows follow row-major order. Save persists formula text only and atomically
replaces a file; load validates into a separate workbook before recomputation and
replacement. Duplicate cell entries reject the entire load.

The `work` directory grant contains both scripts and workbooks. Copy the template
to a private location and give its `host` an absolute writable directory. Tapes and
control sockets must be outside that directory. `--interruptible` additionally
requires a Signals grant for interrupt. Logical jobs (1–4) limit each ready batch;
requested native cores are a separate placement choice.

```
hirsel build examples/sheet/hirsel.json --backend llvm -o /tmp/sheet-llvm
/tmp/sheet-llvm --hirsel-record /tmp/sheet.tape --hirsel-grants examples/sheet/grants.json --jobs 4 demo.sheet
/tmp/sheet-llvm --hirsel-replay /tmp/sheet.tape
```

Limits: 256 cells, 256 formula characters (the accepted grammar is ASCII), AST height/parenthesis nesting 16,
4096 unique dependency edges, 128 command lines, 512 bytes per command, 64 KiB
per workbook/script, and four jobs. Limit refusals preserve the prior cell/workbook.
Domain/input errors return 1, resource/provider/output refusals 3, success 0.
Session exit remembers errors even if later commands repair a cell. Malformed
formulas are persisted as #SYNTAX, overflow as #OVERFLOW, division by zero as
#DIV_ZERO. Cycles and downstream cells show #CYCLE; other error dependencies show
#DEPENDENCY.

Workbook schema 1: `{"version":1,"cells":[{"name":"A1","formula":"2+3"}]}`.
Names are uppercase canonical coordinates. Unknown fields, duplicate names and
versions are rejected. This is an application format with no Excel compatibility.

The six explicit manifest modules separate sessions, parser/AST, generic Cell
state, dependency graph, jobs/evaluation, and bounded persistence/rendering.
See FRICTION.md for validation dispositions and REFERENCES.md for ports/licenses.

Focused verification uses `sheet_eval_31` in the CLI harness. The stock full-entry
matrix, independent edit oracle, native recording/replay, Pending cancellation,
AfterWrite provider host and Ordered tapes are separate named selections.
Production width qualification is reproducible with
`crates/hirsel-cli/tests/support/sheet_eval_31/production.py --app /absolute/sheet-llvm --backend llvm --root /absolute/new-evidence --capped /absolute/repo/tools/capped`.
The ordinary grant needs only work, std I/O and Tasks; interruptible sessions also
need `"signals":{"provider":"signals.host@1","kinds":["interrupt"]}` in io.

Qualification is **incomplete** for the original4096/4097-edge reference/checked
cells at their fixed16M tooling bound and for12 production host-signal cancellation
replays owned by reference-cancel-replay-31. The reduced512-edge twin is a distinct
passing workflow. See FRICTION.md for the original errors and exact dispositions.

The original capture source/V defect is fixed by86ddbca1; this app keeps its
scoped callback arrangement. Native06's original remote raw-tape archive is
incomplete; later representative and width recordings are retained. This is a
useful application landing with precisely recorded incomplete qualification cells.

After interpreter repair6993069c, six fresh cancellation recordings and twelve
native replays pass. Their twelve reference/checked host replays still report
a full-application tooling residual; that exact fresh witness was routed to
@agentlang-24. It remains separate from the incompatible historical tapes.
