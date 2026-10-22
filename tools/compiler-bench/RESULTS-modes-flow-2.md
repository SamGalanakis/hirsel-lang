# Source flow: retain unchanged value payloads

The bounded change reduces instruction work, but **F6 remains INCOMPLETE**.
There is no wall-time or cycle improvement claim. The remaining trigger is the
standard six-family syn10k check, especially ownership transfers and authority
call traversal; this change does not approach the modes CPU goal.

## Mechanism and source

An ownership state join previously entered `ValueData::join` through
`Value::deref_mut`. Even two branch snapshots retaining the same `Rc` allocation
copied its payload before merging it. `Value::join` now checks retained identity
before copy-on-write. It preserves the original operation when the first join
must initialize `uniqueness_site` or normalize borrowed/parked-share bookkeeping.
Distinct payloads always use the existing merge, including earliest loan sites,
complete field-path witnesses and negative evidence. The work-counter meaning,
solver schedule, signatures, query reads and independent V are unchanged.

Prior art: Rust commit `a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`library/alloc/src/rcs/rc.rs:2782-2794` checks allocation identity before payload
equality; lines 2173-2226 document and implement copy-on-write snapshot isolation.
The tests adapt those retained-snapshot/mutation vectors. Hirsel's deviation is
the explicit first-join bookkeeping guard: arbitrary identical values cannot
skip that normalization. The new reference test compares 256 bookkeeping
combinations, plus differing loan paths and sites, with the original operation.
Another test observes 128 payload allocations in the original operation and
zero in the retained path, with identical values. This is a bounded payload
allocation count, not an end-to-end allocator-total claim.

The current accepted baseline profile has 10,693 instruction samples, zero lost.
Inclusive attribution: ownership 13.25%, ownership flow 8.74%, solver 4.81%,
`Value::deref_mut` 0.59%, value merge 0.42%; authority walk 10.85%, authority calls
7.60%, contextual proof memo 4.29%. These overlap and are not additive savings.

## Frozen comparison

Base: `ab44bbb731be91d79289eb3df88aeb139e3dfb75`, after architecture integration,
loan-lowering `2b0e271fc5e7` and owned-JSON implementation `9f0b1acabadc`.
Both CLIs have their matching std images and the same frozen standard-library,
tasks, and `corpus.synthetic(..., 10000)` inputs. Check executes no native runtime
or linker. Source, tool, binary and image hashes and patches are retained with
the session evidence in `hirsel-briefs/modes-flow-2/` outside this repository.

Ten seeded interleaved rounds, seed 20261022, 40 accepted checks; one CPU, jobs1,
4 GiB, cache and daemon off. All five perf events ran 100%. Measurements include
process startup, memo validation, cleanup and descendants through process exit.
CPU31's exclusive timing lock was held by the long LangArena campaign. Under the
2026-10-22 host rule and shepherd correction, the completed session uses ordinary
CPU5 and records affinity per row. An earlier interrupted 39-row CPU31 capture
is preserved separately and provides no acceptance credit. No locked campaign
was restarted or modified. Cycles and phase CPU are recorded observations on a
shared CPU; wall and cycle gains are not inferred.

| Accepted program | Total instructions before/after G | Median paired change | User instructions before/after G | Median paired change |
| --- | ---: | ---: | ---: | ---: |
| tasks | 5.458289 / 5.454029 | -0.0727% | 5.265170 / 5.262251 | -0.0610% |
| standard syn10k | 11.007979 / 10.996629 | -0.1081% | 10.695613 / 10.683652 | -0.1121% |

Recorded cycle medians, with no gain inference: tasks total3.955618/3.925858G and user3.600338/3.578992G; syn10k total8.698256/8.753119G and user8.073665/8.119876G (before/after). Paired cycle ranges are broad under shared-CPU load.

All ten user-instruction pairs improve for each program. Total instructions
improve in ten tasks pairs and nine syn10k pairs. The bounded candidate is
retained on that evidence; it is a small reduction, not the missing F6 solution.

| Syn10k phase CPU, seconds | Before | After | F6 goal |
| --- | ---: | ---: | ---: |
| modes | 0.733677 | 0.751013 | <=0.35, unmet |
| remaining signatures | 0.048842 | 0.050650 | <=0.05, incomplete |
| signature normalization, separately timed | 0.027655 | 0.028714 | no reinterpretation of the signature gate |
| check + resolve | 0.314141 | 0.324532 | <=0.35, within observed limit |
| independent V | 0.405493 | 0.418224 | separate work |
| parse self | 0.014599 | 0.015132 | separate work |

Remaining signatures plus normalization also exceed 0.05 seconds. None of these
unlocked observations certifies a wall-time gate. V-run counts remain 156 for
syn10k and 63 for tasks on both sides; no source proof replaces validation.

## Executed checks and failures retained

- Ownership: baseline 34 pass/1 historical corpus failure; candidate 36 pass/1
  identical historical failure. Both new reference/allocation tests pass.
- Authority: 27 pass/1 identical historical corpus failure before and after.
  Modes: 13 pass before and after. Existing failure ledger entries are retained.
- New diagnostic/read oracle: one test before and after, three subprocess salts
  each, 12 source programs with cold/warm queries. Complete diagnostic JSON,
  signature/resolve/check/modes dependency detail and canonical typed read bytes
  agree, including negative reads, generic mutable contracts, loop feedback and
  signature-only callees. Both transcripts are 353,835 bytes with SHA-256
  `b06dc94aa41a4c530c0b0bc386b403da2fd0f6b3c420ae5c276142e27f284ba4`.
- Focused driver selection: six executed tests pass, including that oracle and
  persistent modes/read-invalidation controls. Front: 30 pass, including flow
  solver, both scaling tests, ownership, authority, loop and mutable-state
  controls; nine explanation snapshots fail. Only those nine were replayed on
  the pristine production path: zero pass/nine fail. Their diff is absolute
  loaded source paths versus expected `../../conformance` paths. They are
  ledgered without blessing or changing path/diagnostic behavior.
- Exact corpus comparison: 1,434 valid pairs, 775 accepted and 659 rejected with
  JSON diagnostics, all exit/stdout/stderr triples identical. The first capture
  had 1,331 equal ENOENT setup failures because a relocated CLI lacked adjacent
  stdlib. Those are not credited as rejections: only those setup failures were
  retried after exposing the frozen stdlib beside both binaries. The valid 103
  earlier comparisons were preserved. Final results contain no ENOENT or
  non-JSON rejection. Selected MODE/OWN/PROV accepted/rejected twins execute
  six conformance cases: six pass, zero fail or unimplemented.
- Setup failures are retained separately: the first oracle's cross-crate
  `include_str!` inputs were not declared to Buck; the first added ownership
  test had a borrow-checking error. Neither failed build counts as executed
  tests. Corrected tests use self-contained sources and a prior loan snapshot.

Scaling work remains bounded: persistent32/128 forward steps271/751 and copied
slot rows1976/6104; deep32/128 forward steps554/2186 and copied slot rows1920/7680.
Sendable generic calls32/128 add18/18 type rows. Existing bounds are unchanged.

## Remaining reproductions

F6: freeze tasks and standard six-example syn10k, then run the matching compiler
with `HIRSEL_DAEMON=off HIRSEL_RESULTS=none@1 HIRSEL_JOBS=1`,
`check --timings=json <syn10k/hirsel.json>`. Modes still needs approximately
0.401 CPU seconds removed in this session. The original source-flow transfer
and authority-call work remain the dominant bounded next investigation.

Historical ratchets: `PROV-1-field-index-container-ok.hir:21`; MODE-2 copied-part
type-argument line11, inferred-box and inferred-tuple line9, push-loop-aliased
line11 and unique-record-part line12. Run their exact `.hir` paths with the
matched CLI. Explanation snapshots: filter `modes_explanations::case_1_` in
`hirsel-pipeline-tests-test-front-shard-1` for the smallest path-format repro.
Wall-clock acceptance needs a future exclusive timing session; caps and goals
were not increased. No TIR/LIR, public API, ABI or format change was made.
