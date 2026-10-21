# Incremental Base64 implementation notes

The public contract and runnable example are in the Base64 section of
[std-library.md](spec/std-library.md#base64-codec). Whole-buffer encode/decode
retain their implementations and signatures. No compiler, IR, validator, runtime
ABI, HTTP server or byte bridge changes are needed.

## Prior art and adaptations

Go `golang/go` at `ebe2feefc400f813aefdc8a036e66d2619da6649` was cloned to
`/tmp/base64-stream-5-go` and compared to the same pinned `/tmp/ref-go` checkout.
Adapted `src/encoding/base64/base64.go:217-286` encoder.Write's leading fringe,
full-quanta interior, trailing fringe and Close; `:467-540` decoder.Read's
incomplete quantum, terminal padding and whole-stream error offsets; and
`:677-680` NewDecoder's state construction. Existing Hirsel decodeQuantum
precedence remains the compatibility oracle (`:328-408` in Go).

Zig `ziglang/zig` at `bff490a23889750de7096d9057c0a432efc929a8` was read from
`/tmp/ref-zig`: `lib/std/base64.zig:261-280` supplies the scalar accumulator,
sextet count and unused-bit check structure. The accumulator is widened to
retain a complete quantum until Hirsel can return an atomic chunk.

Semantic deviations, with reasons:

- Pure borrowed Bytes feed and explicit finish replace Go Reader/Writer and Zig
  destination slices: Hirsel callers receive owned chunks without I/O capabilities.
- Fixed scalar carry replaces I/O buffers and output remainders: state is bounded
  independently of caller chunk size; the existing encoder packs the interior.
- CR/LF and all other non-alphabet bytes are rejected: existing strict RFC4648
  policy, without Go's newline filter or Zig's optional ignore wrapper.
- Typed error classes and consumed-quantum offsets follow existing Hirsel decode:
  Zig's up-front length check would change invalid-byte/padding precedence.
- A padded terminal quantum is withheld until EOF: later input must report the
  same tail-bit-before-trailing-padding error as whole-buffer decode.
- Each failed call returns no chunk and stores the original failure: already
  committed earlier chunks cannot be mistaken for whole-stream success.
- Successful finish closes the state; repeated calls return Closed: callers must
  finalize explicitly, with no implicit output in drop.
- Only Standard/UrlSafe and Required/Omitted are exposed: custom alphabets/pad
  characters and non-strict modes are outside the existing API contract.
- Checked u64 offsets and output-size arithmetic use TextInvariant on overflow:
  consistent with the existing encoder; Go/Zig use host-sized indices.
- No SIMD, assembly, speculative wide reads or architecture-specific tables:
  ordinary existing source features and the current runtime ABI suffice.

## Lane-private tests

- `conformance/SEM/SEM-3-base64-stream-5-run.hir` and its `.hir.json` manifest:
  lifecycle, empty feeds, split equals, committed output, atomic failing calls,
  sticky errors, invalid UTF-8, retained owned output after failure.
- `conformance/SEM/SEM-3-base64-stream-5-importers-run.hir` and manifest: RFC6455
  section 1.3 pure acceptance key, with no server paths.
- `crates/hirsel-driver/tests/it-stdlib/base64_stream_5.rs`: bounded interpreter
  and both native backends with every-stage validation.
- `crates/hirsel-driver/tests/fixtures/base64-stream-5/partitions.hir` and manifest:
  every partition of the seven RFC4648 section 10 vectors and a binary vector,
  all four policies; all 18 Go pairs (including repeated sure.) at every split;
  all 24 Go corrupt rows and strict whitespace/tail/alphabet/UTF-8 controls at
  every partition; all eight Go TestDecoderChunking rows at every split;
  Go bigtest at refill widths 1..12. Reference tests: `base64_test.go:25-52`,
  `:102-105`, `:125-158`, `:183-279`. Successful bytes and final error class/offset
  are compared against the unchanged whole-buffer APIs.
- `crates/hirsel-driver/tests/fixtures/base64-stream-5/bounded.hir` and manifest:
  fixed 6 KiB foo input, single-byte refills, all policies, outputs checked and
  released each call; compile-time copyable/inert/noalloc_drop state controls.
- `crates/hirsel-driver/tests/fixtures/base64-stream-5/bounded-long.hir` and
  manifest: the original fixed 24 KiB vector unchanged, run only by the bounded
  Rust harness on LLVM and Cranelift after the interpreter exhausted 50M steps.
- `stdlib_text_wire::base64_stream_5_is_ordinary_source` proves all six new
  public functions lower as source.

Corpus registration and Rust module registration are append-only. Exhaustive
and long cases run only under explicit bounded Rust interpreter budgets, leaving
the corpus's 1M-step limit unchanged.

## Original observations

Original failing observations are retained separately in `/tmp/base64-stream-5-*`:
`baseline.log` (unsupported postfix syntax in the first draft), `compat.log`
(helper needed preserves_unique), `tests-first.log` (missing Rust imports),
`fmt.log` (CLI takes one source per call), `tests-second.log` (remote compressed
CAS upload size mismatch). Setup/compile failures and zero-case selectors count
as no executed tests; no successful verification is claimed from those runs.

Fixture setup initially selected 3 Rust tests with 0 passing (unresolved enum
constructor paths and local shadowing). The next attempt selected 3 with 1
passing; explicit fixture types were still needed and the 24 KiB interpreter
control exhausted its existing 50M-step budget. `tests-second-executed.log`
preserves that red. `tests-third.log` records 1 passing / 1 failing: the 6 KiB
interpreter/LLVM/Cranelift control passed, and a reused binary literal needed
`copy` in the fixture's expected Result. These were fixture fixes. No production
implementation or validator change followed the first source-origin green.

## Executed verification

All source verification used compiler/interpreter/backend/runtime sources from
`2b0e271fc5e7de39bb38f60e5bb90efec3b1da96`, plus this lane's additive std source
and tests. Compiler sources, IR definitions and runtime ABI stayed unchanged.
Runtime ABI is 37 and LIR layout ABI is 3. Exact immutable source tree identities:

| Component | Git tree |
| --- | --- |
| engine | `0b8df88b890e695198a8c92a26b40ae7084c12dc` |
| interpreter | `2fd0ae19d67d0537de631f2d114595e8038a3251` |
| LIR/backend source | `41825376a0592dd6cb02a048825dcee98ec6b26c` |
| runtime | `2d037648cf3571a6af601cbacc49fee78c62205e` |

The tested `stdlib/std@1/base64.hir` SHA256 is
`0188a535960056bf749e3c7a59a4e31daa93782acbc41d0685d55f35b5f4fe08`.
Rust 1.99.0 is pinned by `tools/buck2/toolchain-lock.json` (SHA256
`8e68b0e2a44a6c7e078b2feff9d3261c3beac5bc5419bdf98cc8cfcebb96d0d3`);
LLVM 21.1.8 by `tools/native-toolchain/llvm-21.lock` (SHA256
`1313649eb2221c9415669bdbb7db03c356a44037ff9ef307f66b7f1acc2b04bb`).
Cargo.lock SHA256 is
`92340afd5c0ed419c264945aa54d4e25e7a03d84d9b0ea25ea9e7c357ed7feb2`.
The complete tested input inventory was frozen in
`/tmp/base64-stream-5-frozen-fourth-inputs.json`; the std/fixture/test hashes
in `freeze-fourth.sha256` were verified unchanged after the successful chain.

Targets were discovered with `.buck2/bin/buck2 targets 'root//:'`.
Actual checks, retaining completed greens across fixture-only changes:

| Check | Executed result | Log suffix under `/tmp/base64-stream-5-` |
| --- | --- | --- |
| Rust source origin/lifecycle | 1 passing, retained from second executed attempt | `tests-second-executed.log` |
| Rust 6 KiB stream control | 1 passing; interpreter + LLVM + Cranelift | `tests-third.log` |
| Rust every partition | 1 passing; interpreter + LLVM + Cranelift | `tests-partitions-fourth.log` |
| Rust original 24 KiB control | 1 passing; LLVM + Cranelift | `tests-long-native.log` |
| Selected corpus with canonical formatting | 6 validated, 6 passing, 0 failing | `conformance-final.log` |
| Exact native smoke selection | 6 cases, all agree; 66 runner-image runs total | `native-final.log` |

Each Rust case ran through `//:hirsel-driver-test-it-stdlib-shard-1` with
`-- --test-arg=<case substring>`; the final two used respectively
`base64_stream_5_every_partition` and `base64_stream_5_long_native`.
Interpreter budgets remain 100M for exhaustive partitions and 50M for the 6 KiB
control, inside the bounded Rust harness; corpus fuel is unchanged. Native
fixture builds use every-stage validation. All outputs/statuses are asserted.

Corpus command:

```sh
.buck2/bin/buck2 test -c hirsel.conformance_paths='conformance/SEM/SEM-3-base64-|conformance/SEM/SEM-3-jwt-hs256-run.hir|conformance/SEM/SEM-3-jwt-tampered-run.hir' //:gate-conformance-selected-shard-1
```

Native command:

```sh
.buck2/bin/buck2 test -c hirsel.native_filters='conformance/SEM/SEM-3-base64-corrupt-run.hir|conformance/SEM/SEM-3-base64-run.hir|conformance/SEM/SEM-3-base64-stream-5-run.hir|conformance/SEM/SEM-3-base64-stream-5-importers-run.hir|conformance/SEM/SEM-3-jwt-hs256-run.hir|conformance/SEM/SEM-3-jwt-tampered-run.hir' //:gate-native-changed
```

Those six IDs are the actual selected identities printed by the native runner.
Each passed four smoke configurations (including H with failure injection,
x86-64 LLVM diagnostic/default and AArch64 default), real I/O record/replay,
and Cranelift, with runner/executable byte agreement. H covers cleanup of the
invalid-tail/error controls. No HTTP server cases ran.

Final distinct lane Rust controls: 4 passing / 0 failing (initial fixture run:
0 passing / 3 failing). Corpus adds 2 cases to the 4 existing selected controls,
all 6 passing; no unmodified-baseline test execution is claimed. The 24 KiB
interpreter tooling limit is an expected harness limit, covered natively without
a fuel increase. No production bug or minimal reproducer remains open.
