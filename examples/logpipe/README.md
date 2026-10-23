# logpipe

Filter one bounded UTF-8 log without collecting its lines. The producer reads
incrementally, the transform decodes and filters, and the calling task writes
numbered matches to its granted stdout. Channels are private, unbounded FIFOs;
1–8 circulating credits bound admitted work across all three stages.

Build a matching CLI and std image from the repository root:

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 build //:hirsel-cli-bin-hirsel //:hirsel-std-image --show-output
```

Set `HIRSEL` to the printed CLI path and `HIRSEL_IMAGE` to the printed image
path. Instantiate the example grant in an owned directory; host roots must be
absolute. This command grants read access only to the small fixture directory:

```sh
export HIRSEL HIRSEL_IMAGE
python3 - <<'PY'
import json
from pathlib import Path
p = Path('examples/logpipe')
g = json.loads((p / 'grants.json').read_text())
g['io']['dirs'][0]['host'] = str((p / 'fixtures').resolve())
Path('/tmp/logpipe-grants.json').write_text(json.dumps(g))
PY
"$HIRSEL" run examples/logpipe/hirsel.json --grants /tmp/logpipe-grants.json
```

Expected stdout:

```text
2: ERROR first
4: ERROR second
```

The argument order is `file query [credits [stop-after [timeout-ms]]]`.
Defaults are 2 credits, no match-count stop, and a 60,000 ms deadline. For
example, append `-- sample.log ERROR 1 1 5000` to emit only the first match.
Zero stop-after pulls no source records. Credits must be 1–8 and the deadline
must be 1–60,000 ms. The input path is relative to the granted `data` mount;
stdout is the granted output. Exit 0 means completion or the requested match
count; an error exits 1 and leaves any already accepted stdout prefix intact.

Use `run --native --backend llvm` or `run --native --backend cranelift` for the
native edit loop. Standalone builds avoid compilation in repeated runs:

```sh
"$HIRSEL" build --backend llvm examples/logpipe/hirsel.json -o /tmp/logpipe
/tmp/logpipe --hirsel-grants /tmp/logpipe-grants.json sample.log ERROR 2
```

Standalone executables snapshot their trailing arguments, replacing a grants
template's `args`; supply them as above. CLI `run --grants` uses the template's
arguments unless explicitly replaced after `--`.

## Bounds and text policy

The file's initial size and total consumed bytes may not exceed 256 KiB. Each
line may contain at most 4 KiB of content; LF and the CR in a CRLF delimiter
are excluded. A final unterminated line is included. Strip CR only immediately
before LF, so a terminal bare CR remains content. Decode every admitted line
as UTF-8, even a line the query would reject. Matching uses a case-sensitive
literal substring, including an empty query, and numbers original source
lines from 1. Output uses LF and may not exceed 256 KiB; a write that would
exceed it is rejected before that write. Queries are at most 4 KiB.

One reservation is acquired before each source-record pull and travels as a
credit ID with the unique row. Transform dequeue does not release it. Only a
filter discard or a completed sink call returns credit. The producer-held,
queued, transform-held and sink-held rows share this budget. EOF consumes an
admission reservation and retires it without creating a row.

The source retains a 256-byte refill buffer, one bounded line scratch, and
channel-held rows bounded by credits. UTF-8 conversion and rendering may
briefly coexist with a row's original bytes; their scratch is bounded by the
4 KiB line and query limits. The production ledger stores scalar counters and
no per-line history. Fixture-only audit logs have a hard 128-event bound.
Runtime recording and provider bookkeeping have their own retention costs;
this is an application data bound, not a bound on the recording file or on
runtime metadata. A concurrent file replacement is not a snapshot; the open
handle and its observed reads determine the run.

On stop, error or deadline, the sink closes its receiver and credit sender,
cancels both children and explicitly awaits both. The transform's receiver
and return-credit sender close as that child exits. No child or source pull
survives return. A send racing shutdown handles `Closed` and drops its owned
payload; send is never assumed cancellable.

## Verification and precedent

The focused Rust tests in `crates/hirsel-cli/tests/cli/logpipe_27.rs` execute the
same pipeline with deterministic file/clock providers and an injected sink.
They compare literal expected bytes and a separate sequential model, and
validate unsorted reservation/transfer/consume/discard/join traces with an
independent state machine. Text, credit, failure/cancellation, line limits,
input cap and output cap are separate bounded partitions. Native tests use
the existing public differential adapter. Checked H runs the full pipeline.
A CLI test separately covers the shipped entry and grants interface.

The reservation and shutdown structure adapts Tokio at commit
`ce685ee6916549ed7e52dbfd5430858bbdc734d3`:

- `tokio/src/sync/mpsc/bounded.rs:1265–1311`: reserve before producing.
- `tokio/src/task/join_set.rs:381–384`: cancel, then join every child.
- `tokio/tests/sync_mpsc.rs:48–109`: FIFO drain and reserve/disarm vectors.
- `tokio/tests/task_join_set.rs:267–295`: pending/ready shutdown shape.

The MIT notice is in `fixtures/LICENSE-Tokio`. Hirsel uses explicit lexical
handles, unbounded data channels and application credits. Credit remains
charged through sink consumption, longer than Tokio's dequeue lifetime.
There are two children plus the calling sink; joins use explicit handles,
not completion-ordered `JoinSet::join_all`. No std prefetch API or language
rule is added. See [FRICTION.md](FRICTION.md) for the recorded production
launch limitation and ownership recipes.
