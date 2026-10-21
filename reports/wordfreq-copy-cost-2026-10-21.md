# Word-frequency token copy cost

The application variant `bench/native/programs/wordfreq-owned-bulk.hir` replaces
the reserved element loop in `owned` with `bytes.append`, followed by the same
`bytes.from_list`. It saves 0.41–0.56% of total instructions in the matched
ten-round sessions. Cycles and task-clock increase. The **≤1.2× Rust target
remains INCOMPLETE/NO-GO**: the variant costs 1.304–1.519× Rust arena instructions.
This is an optional application variant, not a general compiler improvement or
a replacement for the frozen wordfreq benchmark.

Both token copies remain: every token is owned before lookup and every new node
receives its own key. Bucket chains, insertion order, FNV checksum, count updates,
output bytes, validation, cleanup and Rust allocator participants are unchanged.
The original `wordfreq.hir` and benchmark manifest are retained exactly.

## Current reproduction and disposition

Baseline main is `39eb05462794baf0034619d0a3d37fe5a80aec76`, including
map-string-cost `f97fd1128635c22c51d6dcba9ae5a1e74d4d51d0`. All variants use the
same frozen compiler, std image/source and runtime; no architecture checkout is
used. The retained 2026-10-21 `native/wordfreq/main.hir` is byte-identical to
current `wordfreq.hir` plus `common.hir`.

The historical profile is reproduced: a current large-input instruction profile
attributes 27.13% to `seq_grow`, 9.66% to `owned`, 3.93% to `dup` and 3.76% to
`bytes_slice`. Caller stacks put the sampled `dup` under `bytes_slice`, not boxed
key reads. The reserved copy loop already has a vectorized byte copy and no
per-byte grow call in its steady loop. Its allocation still uses `seq_grow`.
Accordingly, no new capacity proof or runtime copy rewrite is justified here.

Current borrowed boxed projections lower to `Read Borrow` for keys and
`Read Bits` for chain links. A regression test validates Value LIR, then checks
the final Ready units with regions on/off for absence of both `Dup` and
`Release`. Its aliased-container execution control copies the outer list,
changes the live boxed key/count, and verifies the other container's original
key/count. No borrowed-box optimization is claimed: this repro is already
resolved on current main and receives coverage only.

The existing compact unique-copy path is also retained: `copy.rs:644–703`
precharges the destination before copying one canonical String/Bytes slice
without a graph walk. Replacing the application helper with `unique data`
increased instructions by 0.64–1.04%, so that candidate is declined. Replacing
both token and output append helpers with bulk appends regressed small-input
instructions by 5.65% and was nearly neutral at larger sizes. Keeping the output
copy loops lets the existing append-into pass recognize the decimal builder's
consumer; only the token helper is changed in the retained application variant.

## Prior art and exact vectors

Rust `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`:

- `library/alloc/src/vec/spec_extend.rs:53–61` selects a slice specialization.
- `library/alloc/src/vec/mod.rs:2925–2955` reserves once, copies the initialized
  interval with `copy_nonoverlapping`, then publishes the new length.
- `library/alloctests/tests/vec.rs:170–178` appends `[6,7,8,9,0]` to
  `[1,2,3,4,5]`. The corpus control ports exactly these values as bytes.

Hirsel already provides that mechanism in `hirsel-rt/src/io.rs:4902–4970`.
This lane uses the existing public operation. Hirsel-specific behavior is its
ordinary copy-on-write detachment, tracked-arena precharge and explicit slice
offset; the old holders remain live. No Rust allocator behavior is ported.

Additional controls cover empty input, all six ASCII whitespace bytes, the
exact `a b a b c` output, UTF-8 words with a final unterminated word, embedded
zero/255 bytes, nonzero slice offsets, an empty subslice and an aliased byte
builder. The original independent expected output files remain the oracle for
all three workload sizes; no checksum is replaced by a looser validator.

## Measurement contract and identities

Evidence and replay scripts: `/tmp/wordfreq-copy-cost-evidence`.
`measurement-sha256.json`, `candidate-measurement-sha256.json`, `env.json`,
`plan.json`, per-binary build logs and the retained sources freeze all inputs.
Hashes were checked again after the accepted captures. The production code
after `type Word` in the retained variant matches its measured source exactly;
only its introductory explanatory comment differs.

| Artifact | SHA-256 |
|---|---|
| compiler | `db4105e8aefc55206cfc1167923f575b6a59627a3edb7ae72c07bb68fef3e0cd` |
| std image | `fec0c231fcf6d41272b6ecdf10a68e25e865f6d9f5f961d5b8691ba79d49bae7` |
| runtime, x86-64/default | `8f3572fa7b177a84487856eb64643583e0651ecf9504a04cdd61131030cc20c3` |
| before source | `73ec83d40700901e0f8331dafaf36ed7e575fb0a943cb97e9f4320d2ce8ada11` |
| token-bulk source | `73deb749f47936953c981b484147188a995fdbb51a66c260339217e4bd719698` |
| before executable | `d1f72ea1e7aaa71a6266d2db35e1573fc9328669d3de7483025625e381e9a645` |
| token-bulk executable | `e3758e01ba8f3eda2d89b0b7fe753cbbc48a51201b1d1c3d3edcced7929922b0` |

The copied unchanged Rust system/arena ELF hashes are
`41dc5a68be772340ee59f502504dcf4cd8e7db8863706adaeb8188eee177777c`;
mimalloc is `92a970d845ec4f543ec862f92bb97a2a506467d84d38625966b8c76e527c7a0a`.
Their runtime variant arguments are retained separately even where the ELF is
the same. Inputs small/medium/large and their oracle hashes are recorded in
the identity inventories and copied unchanged into `inputs/`.

Every measured process runs through the unchanged `tools/capped` budget
(2560 MiB, two CPUs), pinned to CPU31, under the existing CPU31 timing lock.
This is a local file workload, so there is no server or open-loop client.
Seed 20261021 shuffles ten complete rounds per size. Events are total/user
instructions and cycles plus task-clock. Startup, file loading, token/key
construction, validation and full teardown are included, as are descendants.
Epochs remain small=10000, medium=6047, large=800.

There are two separate complete campaigns: 150 samples compare before, both-
helper bulk and the three Rust variants; 180 compare before, token-only bulk,
unique-copy and the three Rust variants. They are not pooled. No sample was
restarted, excluded or replaced. All 330 outputs match their original epoch
checksums. Six initial exact-byte before/both-helper controls and 15 later
candidate/Rust controls match the independent output files. The initial six
were executed directly; subsequent local native runs use `tools/capped`.

## Accepted token-only session results

Ratios are medians from `candidates-session-{small,medium,large}.json`.
Rust arena is the lowest-instruction unchanged Rust participant in every size.

| Size | Before / Rust, total | After / Rust, total / user | After / before, total / user | Cycles after / before | Task-clock after / before |
|---|---:|---:|---:|---:|---:|
| small | 1.309512 | 1.304122 / 1.300993 | 0.995884 / 0.996071 | 1.041101 | 1.044101 |
| medium | 1.462309 | 1.454555 / 1.454291 | 0.994697 / 0.994657 | 1.031240 | 1.026417 |
| large | 1.527565 | 1.519072 / 1.501517 | 0.994440 / 0.994362 | 1.019600 | 1.017164 |

Only within-session timing ratios are meaningful. This instruction saving is
small and is not a latency improvement. `results.json` contains medians for
each event and every Rust allocator variant; the raw sessions retain samples,
seeded order, stdout hashes, CPU sibling observations and counter running
percentages. Profiles explain attribution only; their sampled event totals
are not a second performance verdict.

## Validation and open work

The affected RC byte-view/list/freeze/append families ran 46 passing tests and
four pre-existing native-only ignores. The added final-stage box assertion and
full-workload byte-vector test both pass. In total this covers 45 existing and
three new RC tests. The runtime graph-copy integration selection executes all
18 tests successfully. The selected five ownership/bounds/cleanup corpus cases
pass native smoke (four configurations, real-Io record/replay and Cranelift)
and LLVM/Cranelift parity. The new alias control also executes through the heap
checker's complete injected-failure campaign. Corpus admission and canonical
formatting are checked independently.

The required Python benchmark harness check ran 41 tests: 38 passed, three
existing `test_c_arena` tests failed because the frozen manifest's
`hirsel-native-build/src/main.rs` hash predates `a5a14ba9`. Both that helper and
the manifest are unchanged from this lane's baseline. The failure is recorded
in `tools/known-failures.tsv`; no historical source seal was refreshed to obtain
a pass. Repro: `python3 -m unittest discover -s bench/native/tests -v`.
Current corpus discovery has already retired admission through `passing.txt`,
so the new fixture needs no entry in that old file.

Setup failures remain in `/tmp/wordfreq-copy-cost-*.log`: missing unprepared
Buck executable, unsupported literal `root//:*`, unneeded std-object bundle
build canceled on its own client, wrong test-runner argument syntax, a zero-
test unit selection instead of the integration copy tests, missing generated
include inventory, an upload size mismatch while the fixture changed, and
initial noncanonical corpus formatting. None supplies execution credit.

Smallest ownership repro, already green:

```hir
type Word { key: Bytes, count: u64, next: u64 }
fn equal(nodes: List[box Word], i: u64, token: Bytes) -> bool {
  nodes[i].key == token
}
```

The remaining performance trigger is `owned(bytes.slice(data, start, end))`
inside the unchanged wordfreq token loop. The current profile still pays one
token allocation/grow plus the slice owner's dup/release, rather than a
per-byte grow or boxed-key retain. `before/main.hir` is the exact retained
full repro, and `owned-bulk/main.hir` is the optional application variant.
Replay any accepted cell with the retained `snapshot/bench/micro/run`, its
`candidates-SIZE.json` participant list and a fresh output filename, `--rounds 10
--seed 20261021`. The smallest measured size already misses 1.2×; the large
1.519× row remains the largest gap. No threshold, allocator or resource cap
was changed, and no general-path or adoption gate is declared passed.
