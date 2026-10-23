# Shardsearch friction

Original failed drafts, exact diagnostics, timestamps and execution receipts are
retained outside the repository at `hirsel-briefs/shardsearch-27/`.

| Witness | Exact diagnostic / behavior | Disposition, cost, owner | Elapsed |
| --- | --- | --- | --- |
| `fn invalid(kind: String) -> SearchError { SearchError(kind: copy kind, file: 0) }` | MODE-1: `this operation requires a global value: kind is local to the caller, but the result must be global` | Intended ownership rule. `unique kind` copies the short error tag into the owned report. Application owner shardsearch-27. | ~3 min, shared with next row |
| `let name = unique names[i]; task.async(s, fn() { search_file(tasks, d, name, query, limit, budget) })` with borrowed `name` parameter | CLO-3: `task captures must copy or take sendable values; Receivers must be taken: name is not proved sendable, but the closure crosses into a task` | Intended rule. Worker parameter is `take name: unique String`; call uses `take name`. One independent path copy per child, no extra copy on transfer. Application owner shardsearch-27. | ~3 min, shared with previous row |
| Native test child changed cwd with a relative CLI path. | Python `FileNotFoundError: ... cargo-bin-exe/hirsel-cli-test-cli-harness/hirsel` | Harness setup error. Resolve the CLI before changing cwd. No language workaround or application cost. | ~2 min |
| Standalone test supplied Args only in a grants template, then put recording options after `--hirsel-grants`. | App stderr `jobs:0`; minimized Args control returned `Arg(MissingArgument)`. | Harness invocation error. Standalone argv supplies Args after the grants operand; recording options precede it. CLI `run` keeps template Args unless explicitly overridden. No runtime repair. | ~4 min |
| `pub fn main(args: Args) -> Result[(), ArgError] { let _ = arg.text(args, 0)?; Ok(()) }` with argument `x`, native Ordered width 1 and trace export. | `hirsel-rt: launch refused: incomplete canonical event evidence`, exit 3 after the app ran. Recording without trace export succeeds. | Open production trace defect, owner e1-trace-record-27. The tool and permanent semantic tests use ordinary recording; traced qualification reds remain external evidence. No second runtime repair. | ~10 min including bounded controls |

The frozen query Shared root, bounded batches, input-order `await_at`, owned
per-file results and delayed output are ordinary application plumbing. They do
not require std/task or runtime changes. Known fixed linkcheck endpoint
inference diagnostics are controls, not new defects here.

The deterministic public file simulator has no pending/stall file-read provider.
A long earlier file exercises additional read scheduling points, but is not a
provider-stall witness. Cancellation during a genuinely pending file-provider
operation must be accounted separately; no model-only pass is claimed for it.

Reference/checked-H workflow observations passed with the full public observation
compared. Native workflow tests and a separate joined-cutoff control read tapes
through the public runtime adapter; all opened files must close before result
output and no worker file I/O can follow it. This is a continuation invariant,
not a claim that a host disk operation was deliberately stalled.
