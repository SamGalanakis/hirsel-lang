# Words: streaming tokens and deterministic maxima

The selected change reduces full-workload instructions by **28.47%**
and Hirsel runtime allocation requests by **25.50%**.
It streams tokens and scans insertion-ordered distinct entries, preserving the
first inserted winner on ties. The full checksum remains **1998586832**;
the exact test.js checksum remains **580539813** on all three Hirsel engines.

The faster upsert candidate is **rejected**: it decreases instructions further,
but allocations increase **25,414 → 8,064,502**, deep copies **432 → 8,045,136**,
and copied bytes **140,679,936 → 173,061,792**. Those copies are one per distinct
key per step (18,623 × 432), required by the current unique-key upsert path.
The selected change retains the original shared-key get+insert update path;
no borrowed-map API or global hash/ownership change is included.

## Instruction evidence

Full run.js Words: 50,000 ASCII words, word_len 4, 72 warmup + 360 timed steps.
Rust and both Hirsel variants have the same generated input and exact checksum.
Both the quick and full generated workloads have a unique maximum, so the
unmodified Rust checksum is usable; adversarial ties use Crystal's deterministic
policy, never randomized Rust HashMap iteration.

| Session | Participant | Median instructions | Min | Max |
| --- | --- | ---: | ---: | ---: |
| A | frozen baseline | 34,151,370,443.0 | 34,110,722,123 | 34,463,345,372 |
| A | rejected upsert | 20,164,229,565.5 | 20,152,532,985 | 20,189,141,742 |
| A | unchanged Rust | 12,197,877,605.5 | 12,193,828,246 | 12,206,057,570 |
| B | rejected upsert | 20,157,052,829.0 | 20,141,592,732 | 20,196,657,078 |
| B | selected streamed | 24,428,152,485.5 | 24,411,754,920 | 24,436,355,357 |

Session A is the single before/upsert/Rust comparison (30 accepted processes).
After its allocation regression was identified, session B compares only the
corrected candidate against the same frozen upsert binary (20 processes).
The original baseline/Rust campaign and baseline vector run were reused.
The upsert median differs by 0.036% across sessions. Instruction
comparison to the original baseline is 2.800× → 2.003× Rust;
these are instruction ratios, not runtime claims.

Each session uses ten seeded interleaved rounds (seed 20261022), CPU30 affinity,
and perf stat task-clock/cycles/instructions/cycles:u/instructions:u. All 50
accepted processes pass their full checksum; all five counters have **100%**
running coverage. Scope is the isolated benchmark process tree, including
startup, preparation, warmup, timed steps and destruction. /usr/bin/time launches
one benchmark; perf hooks capture SMT sibling activity around each process.
No CPU lock is held. CPU31 timing lock availability was checked without waiting
(available and immediately released); CPU31 was never selected for these runs.
Session A's raw wall_timing explanation contains stale static text saying the lock
was occupied; its timing_lock field records the actual available/released result.
No wall-clock speedup, cycle ratio, published-host projection, or singleton
speedup is claimed. Follow-up timing requires its own exclusive ten-round session.

All consumed source snapshots, std sources, reference source/lockfile, generated
manifests, grants, measurement scripts, compiler/helper executables, Rust compiler,
native toolchain/runtime archives, and benchmark executables were hashed before
each session and verified afterward. Baseline starts at 8bedce64; main moving
never triggered another baseline campaign. Source/binary identities and all
sample counts are retained in the lane evidence directory. Both accepted
sessions have `frozen_identity_verified=true`.

## Allocation evidence and attribution

| Participant | Allocation requests | Frees | Deep copies | Copied bytes |
| --- | ---: | ---: | ---: | ---: |
| baseline | 25,414 | 25,414 | 432 | 140,679,936 |
| rejected upsert | 8,064,502 | 8,064,502 | 8,045,136 | 173,061,792 |
| selected streamed | 18,934 | 18,934 | 432 | 140,679,936 |

Counts use separately linked standalone measurement runtime archives on the full
workload, including setup and warmup; they count physical Hirsel runtime blocks
and growth, not process-wide malloc. The ordinary launcher counts only its own
allocations and was excluded. All selected allocations are freed; live bytes at
exit are zero. Rust heaptrack separately records 7,109 process-wide malloc calls,
including setup and warmup; its coverage differs, so no cross-engine allocation
ratio is inferred.

Removing the token list saves **6,480 = 15 growth allocations × 432 steps**.
The original one-source-copy-per-step policy remains: 432 copies and 140,679,936
copied bytes. Every source copy, map, and its key slices is released within the
same step's timed lifetime. Keys retain the owned source until the map is dropped.
Final lookup work falls from 50,000 token probes per step to an ordered traversal
of 18,623 distinct entries; updates retain the original get+insert probes.

Baseline instruction sampling attributes 26.86% to map.get, 19.17% to SipHash,
11.35% to the split piece helper, 8.48% to map.insert, and 6.09% to the splitter.
The selected profile removes splitter/piece work and all final-pass map probes.
Profiles are attribution-only singleton samples; the interleaved counts determine
the verdict. The separate wordfreq-copy-cost app/workload's findings are untouched.

## Correctness, failed attempts and references

Target discovered with `.buck2/bin/buck2 targets 'root//:'`:

```
.buck2/bin/buck2 test root//:gate-langarena-words-9 -- --test-arg=WordsVectors.test_vectors_all_engines
```

Baseline: **1 unittest passed**, 12 vectors × two steps × three engines = **72
vector executions**, plus exact test.js checksum on interpreter, Cranelift and
LLVM (**3 checksum checks**). Selected candidate: the same counts, **1/1 passed**
through Buck2 with nonzero intended selection. Each execution independently checks
the entire frequency map, exact winner, maximum and accumulated checksum. Vectors
cover empty/whitespace-only, single, repeated, distinct, `a b b a`, reversed ties,
a late third-way tie, eventual overtaking, ASCII whitespace and retained vertical
tab. Full native checksums pass in every accepted perf/allocation process.

Preserved failures/setup attempts: initial vector entry returned () instead of
Result (3 engine failures); corrected entry omitted required grants.dirs (3);
two remote launch attempts failed executable permission (0 vector executions);
launcher allocation orchestration had a Python syntax error before any child;
non-standalone launcher coverage run failed host I/O and was excluded; initial
measurement preflight supplied an absolute capability-relative config path (0
rounds). `confined_relative`, crates/hirsel-run-schema/src/path.rs:5–7, rejects
absolute paths; the single corrected session uses the already validated relative
argv and grants. Shared-key update trial fails MODE-2 uniqueness and MODE-1
caller-local key checks (three engines, zero executed vectors), retained separately.
The corrected streamed build and scoped control pass without weakening assertions.
All setup profiles, builds, preflights and failed attempts have separate logs;
there are no unreported zero-selection greens. `attempts.json` indexes them.

Upstream LangArena **3c9f84ddaeec834c0631747cb789e8cfd602f993**, MIT license,
copyright 2026 Konstantin Makarchev. Existing LICENSE and UPSTREAM are retained.
Port authorities: rust/src/benchmarks/words.rs:31–73 (token traversal, frequencies,
checksum); crystal/main.cr:1442–1473 (first insertion wins equal maxima);
test.js:122–129 (580539813). Hirsel adaptations: retain the existing ASCII
whitespace policy (VT excluded), checked UTF-8 positions, one owned source per
step required by retained key ownership, insertion-ordered Map traversal and
strict `>` ties. Rust's Unicode expansion and randomized max iteration are not
ported. Empty input retains the existing empty winner/count behavior.

No language-rule, TIR/LIR, runtime ABI, stdlib, reference license/notice, README,
or existing test-harness registration is changed. A dedicated Python target is
registered in the build generator; generated BUCK files are regenerated, not
committed. No Rust harness main.rs change is needed.

## Identities and handoff

- before executable SHA-256: `32fc2d9a43f3de406013c0e3b30bf1e97ca0ad7b0ff047dd8c03bf3a2208ffdb`.
- final executable SHA-256: `75d5df0117bef67080e85133e4dfa67b0d5e119e9c8ad7e0bb1054e6d50cfb16`.
- rust executable SHA-256: `468817173ea176c7c80ee1ddd3e4b0233eecf53a209419de392ec080e0f39f4f`.
- streamed executable SHA-256: `f535501349185b1b48e50d229a6a569c03fc4ab148ca2e2ce0bc68cc85294ca8`.
- Selected words.hir SHA-256: `4b0ae2a18e7215e359f62477a726e0cbb31e99af735dce8446a9180da7ffe0e4`.

Machine-readable summary: `RESULTS-la-words-9.json`.
Retained evidence: `/workspace/code/hirsel-briefs/la-words-9/`:
`measurements-final.json`, `measurements-streamed.json`, `frozen-inputs.json`,
`streamed-frozen-inputs.json`, `attempts.json`, source snapshots, standalone
allocation reports, perf counters/profiles, independent generated input oracle,
and copied build/test/landing logs. `REPORT.md` adds the final landing disposition.

Optional follow-ups: a map value update API that can safely operate on maps of
shared source-slice keys (minimal expected MODE-2 repro and diagnostics in
`borrowed-map-repro.hir` / `.log`), and exclusive-session wall timing of
`bench/langarena/run.js Etc::Words`. Neither blocks the selected bounded change.
