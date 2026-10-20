# Small core prior-art vectors

References are pinned in third-party/NOTICES.md. Source comments cite the
upstream file and line; reference checkouts were read before writing vectors.
No runtime, public API, serialized format or context-switch instruction changes.

| Item | Before → after | Corpus coverage |
| --- | --- | --- |
| iter | 2 → 3 runnable cases | Rust filter count/order, all rejected/empty; take shorter/longer/zero/max/partially consumed; enumerate offset fold/empty; zip unequal in both orders/empty/repeated EOF |
| stream | 1 → 2 runnable behavioral cases | from_results/next/to_iter, try_map, try_filter, try_fold, try_for_each, take_n; source and callback errors, short circuit, empty fold/sink, limit before an error |
| actor | 3 → 3 runnable cases, stronger assertions | cast then call observes 5 before the second producer; call to a stopped actor returns Closed; reply after receiver destruction preserves 73; drain and abandoned reply retained |
| test | 2 audit minima + 1 already-landed drawn-length minimum → 6 reference minima | drawn-length [1001] retained; Hypothesis 1 [0,93,0]; Hypothesis 2 [2]; workout tree [7,0,0] |
| task/fiber | uncited → pinned source comments | errgroup/TaskGroup and corosensei/Boost.Context; no behavioral changes |

## Adaptations and deviations

- Rust's forward next/fold/collect values are retained. nth(0/1) is expressed
  through next calls; Hirsel has no nth, next_back, advance_by, rfold, size_hint,
  ExactSizeIterator or TrustedRandomAccess. Those API-only assertions are omitted.
  Count uses fold. Test cursors own copies of reused input slices because an
  escaping Hirsel cursor owns its state. Hirsel fuses every adapter, including a non-fused source;
  the existing fused case verifies no further source pulls.
- futures-rs's seven corresponding surfaces are synchronous owned pulls here:
  from_results/next, try_map (and_then), try_filter, try_fold, try_for_each,
  take_n (StreamExt::take), and to_iter (result-stream observation). Raw Rust
  streams may continue after an error; Flow ends there. A Hirsel predicate can
  itself fail. Existing terminal tests cover these stronger guarantees and
  forbidden callbacks. Async Pending/wakers and unsupported flatten/skip/any/all
  combinators are outside this API. The fold/for_each examples actually live in
  StreamExt, not TryStreamExt; their Result inputs are propagated by the callback
  upstream and by Flow here.
- Actors own FIFO channels instead of Erlang process IDs. Scope cancellation
  supplies stop; Closed replaces noproc exits. Reply owns its one-use sender;
  a closed caller returns the payload instead of silently discarding it. The
  existing roundtrip/closed cases are extended rather than duplicated.
- Minithesis vector facts were re-expressed independently; no MPL source was
  copied. Hirsel uses a constant false or bounded zero choice for weighted(0.0), and
  treats INVALID branches as passing laws because Source has no rejection API.
  Output lists include the generated values, not just the consumed choice tape.
  Hirsel's own PRNG determines the first failing case, not Python Random.
- Upstream test_give_minithesis_a_workout (test_minithesis.py:444-502) has no
  fixed expected minimum: Hypothesis generates decision trees, checks execution
  bounds, and emits regressions. Its bounded choice/weighted/interesting branch
  structure is represented by an explicit tree with two failure branches; the
  inverted boolean makes seed 0 enter the longer branch first; the short
  failure at 7 competes with a longer branch at 9. A deterministic expected
  minimum fits the corpus and checks deletion, lowering and replay without
  adding a second meta-fuzzer or target/reject APIs.
- Task.all returns the first replay-ordered error, rather than ExceptionGroup;
  successful values retain add order. The scheduler owns each context save slot
  and passes two entry words rather than coroutine parent linkage/transfer_t.
  Optional cross-stack CFI is deferred: this change adds citations only, and
  task failures already settle through the scheduler. No unwind claim is added.

## Validation

The focused set contains 46 corpus programs: the added cases, actor/task
controls, iterator/stream ownership and authority twins, and existing shrink
minima. All pass after fixture corrections; only failed cases were rerun.
All eight changed executable programs agree across the four native smoke
configurations, real-I/O recording/replay, runner/executable checks and
Cranelift parity. Runtime tests pass: 36 unit tests and 319 integration tests
(three existing integration tests are ignored). Build, Clippy and Rust
formatting are checked by tools/land.
