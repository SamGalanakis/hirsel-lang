# Textindex

Build a bounded, persistent inverted index over explicit UTF8 documents, then
query its immutable snapshot. All six modules are assigned in `hirsel.json`.
No directory traversal, query language, scoring model, stemming, or Unicode
normalization is performed. Original document IDs are preserved and sorted by
UTF8 bytes. Empty queries return no matches; repeated query terms count once.

From the repository root, with a matching built `hirsel` on PATH:

```sh
mkdir -p /tmp/textindex-store
python3 - <<'PY'
import json
from pathlib import Path
root = Path('examples/textindex').resolve()
for access in ['read_write', 'read']:
    grants = json.loads((root / f'grants-{access}.template.json').read_text())
    grants['io']['dirs'][0]['host'] = str(root / 'fixtures')
    grants['io']['dirs'][1]['host'] = '/tmp/textindex-store'
    Path(f'/tmp/textindex-{access}.json').write_text(json.dumps(grants))
PY
hirsel run examples/textindex/hirsel.json --grants /tmp/textindex-read_write.json -- build manifest.json search.json 4
hirsel run examples/textindex/hirsel.json --grants /tmp/textindex-read.json -- query manifest.json search.json all 'hello tax'
hirsel run examples/textindex/hirsel.json --grants /tmp/textindex-read.json -- query manifest.json search.json any 'café tax'
hirsel run examples/textindex/hirsel.json --grants /tmp/textindex-read.json -- inspect manifest.json search.json
hirsel build examples/textindex/hirsel.json --backend llvm -o /tmp/textindex-llvm
/tmp/textindex-llvm --hirsel-record /tmp/textindex-query.tape --hirsel-grants /tmp/textindex-read.json query manifest.json search.json all 'hello tax'
/tmp/textindex-llvm --hirsel-replay /tmp/textindex-query.tape
```

`build` prints `built`. `query` emits one JSON object per matching ID, with a
`frequencies` object containing every distinct normalized query term, including
zero for absent terms in `any` mode. `inspect` emits the validated index JSON.

The input mount is read only. The index mount needs write access only for
`build`. Both are explicit virtual mount selectors; paths are confined relative
paths. Tapes must be outside both mounts. CLI arguments follow the grants
operand for standalone executables. Jobs is a logical admission bound (1..4),
independent of physical placement width.

The version 1 manifest has schema-order fields:

```json
{"version":1,"documents":[{"id":"original-ID","path":"relative.txt"}]}
```

The version 1 index has schema-order fields:

```json
{"version":1,"documents":[{"id":"original-ID","bytes":5,"tokens":1}],"terms":[{"term":"hello","postings":[{"id":"original-ID","spans":[{"start":0,"end":5,"position":0}]}]}]}
```

Field order is part of this small application format; unknown, repeated,
reordered and missing fields are refused. JSON whitespace and escaped text are
accepted. The streaming decoder checks admission before retaining each element.
Documents, terms, and each term's postings are strictly sorted and unique. Every
posting names a document. Spans are half-open original byte intervals with
strictly increasing token positions, complete position coverage per document,
nonoverlapping offsets, and counts reconciled against document metadata.
`query`/`inspect` require the same set of manifest IDs; they do not read current
document bodies or claim to authenticate the stored text. A changed document
requires a new build. A manifest path change alone does not invalidate an index.

Every build validates all documents before encoding and before acquiring the
atomic replacement. A failed build preserves the existing index. Each reader
opens and reads its own snapshot; an atomic replacement does not change an
already open reader's file. This is atomic publication, not a promise to survive
a storage-device crash: `file.commit` supplies the shipped atomic-file contract.
Concurrent writers and in-place external edits are outside this workflow.

| Resource | Inclusive cap |
| --- | ---: |
| Documents | 32 |
| Document bytes | 8192 |
| Total document bytes | 131072 |
| Distinct terms | 4096 |
| Stored occurrences (spans) | 16384 |
| Query tokens before deduplication | 32 |
| Raw token / lowercase term bytes | 256 / 768 |
| Persisted index bytes | 1048576 |
| Worker slots | 4 |
| Manifest bytes / ID bytes / path bytes | 16384 / 128 / 256 |
| Query text bytes | 8192 |

Reads use `file.read_all_bounded`, including its one-byte EOF probe. Batches
reserve byte capacity before admitting children, use `await_any` to notice an
error promptly, join canceled children, and merge successes in manifest order.
The immutable document table is a scoped `Shared` root; each task owns its path
and result. The generic postings accumulator is instantiated for document spans
and global postings. Normalization allocation is bounded before retention by
the 256-byte input token cap and Unicode16 full-lowercase expansion bound.
Output checks the index-byte budget before each buffer growth; JSON-escaped
IDs/terms use a bounded temporary.

Domain/resource refusal prints `kind:document_index` to stderr and exits 2;
indices are zero based and non-document errors use 0. Denied output projections
exit 3; an actual output write error is returned through the entry Result.
Language failures and evaluator Steps stops are distinct tooling/test outcomes.

## References and deviations

Token span scanning and the `Hello, happy tax payer!` vector are ported from
Tantivy `e988ecb7a11109d89f9f4582b386089fc3a2fec0`,
`src/tokenizer/simple_tokenizer.rs:1–91`. Monotone intersection and overlap
controls follow `src/query/intersection.rs:114–185,340–390`. The MIT notice is
retained in `LICENSE-TANTIVY`.

The shipped Unicode16 `unicode.is_letter || unicode.is_digit` predicates mean
Letter or Decimal_Number. Tantivy/Rust's broader alphanumeric predicate also
includes other numeric/alphabetic properties; this app deliberately follows
the available shipped predicates. Lowercase uses `string.to_lower` after
splitting, including dotted-I expansion and final sigma. Combining marks split
spans, so composed/decomposed spellings need not match. The Python oracle uses
Unicode categories over fixed vectors shared by its Unicode version and
Unicode16. There is no claim of complete Unicode-version equivalence.

Bounded vectors replace Tantivy DocSets/seek acceleration. The persistent JSON
schema replaces FST/compressed segments; there is no Tantivy-format or BM25
compatibility claim. Hirsel Tasks, owned buffers, and explicit Dir capabilities
replace ambient threads and filesystem access. Shipped std owns JSON, Unicode,
sorting and bounded file reads.

## Verification

The focused harness is `text_index_30` in CLI shard 1. Its handwritten Python
oracle scans complete document inputs independently, generates fixed expected
postings and query counts before execution, and includes cap/one-over vectors.
The Rust harness runs the actual entry, config, and modules through reference
and checked-H adapters. Native host-file workflows use ordinary Ordered
launches, record immutable tapes, remove original providers and replay using
reference, checked-H, LLVM and Cranelift adapters. Receipts and original failures
are retained externally under `hirsel-briefs/text-index-30/`.

```sh
python3 tools/buck2/prepare.py
python3 tools/buck2/sync.py --if-stale
.buck2/bin/buck2 test -c hirsel.tests=focus //:hirsel-cli-test-cli-shard-1 -- --test-arg=text_index_30 --test-arg=--test-threads=1
```

`TEXT_INDEX_30_CASE` selects comma-separated vector IDs for affected/unrun cells;
`TEXT_INDEX_30_EVIDENCE_DIR` retains per-run fixtures. Source/export acceptance is
not an engine pass. Native scripted Pending cancellation is INCOMPLETE because
the public native fixture cannot supply this main's Args/mounts/initial files;
the coordinator owns the follow-up. Requested 1/2/4 placement is only qualified
where receipts retain `worker_phase=running`, effective N and pending zero
before process completion. Short runs without that receipt remain INCOMPLETE.
See `FRICTION.md` for exact dispositions, including evaluator budget stops.

The recorded application handoff has 198 completed reference/checked-H stage
checks, 272 native host-file launches and 128 successful replay checks. The
large reference fixtures include 32 Steps stops and 24 dependent unrun stages;
these are INCOMPLETE, alongside the owned native scripted cancellation gap.
The four focused Rust identities exist independently of their per-engine cells.
Application source lines: main 84, manifest 188, tokenize 90, index 217,
store 311, query 132; total 1022, excluding tests and fixtures.
