# Retry and repeat schedules: primary-source adaptation notes

Research date: 2026-10-20. Prefer `std@1/schedule`: a schedule is reusable policy for both retrying failures and repeating successes, matching [Effect's model](https://github.com/Effect-TS/effect/blob/5a2d19f60e6608396389a66586848acbb3f98f12/packages/effect/src/Schedule.ts#L1-L8). This note records source behavior and proposed Hirsel adaptations separately; it does not import implementation code.

## Sources and local references

All repositories were shallow-cloned under `/tmp/ref-*-std-retry`:

| Project | Revision studied | Primary source |
| --- | --- | --- |
| Effect v3 | `e95695057b2446d5bc4de81bac0a77ef49d7a4c0` (`effect@3.19.4`) | [Schedule API](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/Schedule.ts), [implementation](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/internal/schedule.ts), [tests](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/test/Schedule.test.ts) |
| Effect HEAD | `5a2d19f60e6608396389a66586848acbb3f98f12` | [Schedule API and implementation](https://github.com/Effect-TS/effect/blob/5a2d19f60e6608396389a66586848acbb3f98f12/packages/effect/src/Schedule.ts) |
| Go backoff | `6f228bfdb51209aa375869feb27dcede4fa0db06` | [exponential](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/exponential.go), [retry](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/retry.go) |
| Rust backoff | `587e2da8fb2dcfc65ca544cb9249022c51f1406e` | [exponential](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/src/exponential.rs), [tests](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/tests/exponential.rs) |
| tokio-retry | `69a8766aaead1190c2a305937d45740d6c2d9b33` | [strategies and tests](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs) |
| AWS | Article plus simulator `66cb169277051eea207dbef8c7f71767fe6af144` | [Exponential Backoff and Jitter](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/), [simulator](https://github.com/awslabs/aws-arch-backoff-simulator/blob/66cb169277051eea207dbef8c7f71767fe6af144/src/backoff_simulator.py) |

Rust `backoff` declares `https://github.com/ihrwein/backoff` in its Cargo manifest. The initially suggested `iqlusioninc/backoff` repository returns 404. Effect HEAD is a later API generation without the v3 `union`/`intersect` surface, so the v3 release above is the combinator reference.

## Constructors and composition

Effect exponential delays are `base * factor^n`, beginning with `base`, and Fibonacci delays begin `base, base, 2*base, 3*base`. Effect distinguishes `spaced`, which waits after an operation completes, from `fixed`, which aligns recurrences to a cadence. A slow fixed operation may cause an immediate next recurrence; missed intervals are not replayed. See [v3 constructor contracts](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/Schedule.ts#L989-L1049) and [HEAD fixed algorithm](https://github.com/Effect-TS/effect/blob/5a2d19f60e6608396389a66586848acbb3f98f12/packages/effect/src/Schedule.ts#L926-L984).

Effect v3 defines [intersect](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/Schedule.ts#L1150-L1179) as continuing only when both continue and using the longer delay; [union](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/Schedule.ts#L1845-L1876) continues while either continues and uses the shorter live delay. A completed branch does not contribute a zero delay to union: the [implementation](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/src/internal/schedule.ts#L1483-L1521) chooses the surviving branch's intervals.

For Hirsel's simpler delay/stop decisions, derive commutativity, associativity, and idempotence from `min`/OR for union and `max`/AND for intersect. Also test absorption and terminal behavior. These are proposed laws for observable delays and continuation, not assertions that arbitrary stateful, random-effectful schedules are structurally equal. Evaluate child schedules in a documented deterministic order; drawing independent randomness twice can invalidate naive `union(s,s) == s` trace equality. Keep algebraic tests on deterministic schedules, or share a sampled decision explicitly.

A stopped schedule is union's identity and intersect's absorbing element. A forever-zero schedule is intersect's identity. Both branches need independent state. Budgets attached outside a composition apply to the combined driver; branch-local budgets should stay branch-local. These are design consequences of the decision algebra, rather than additional Effect API guarantees.

## Jitter and limits

The AWS [simulator](https://github.com/awslabs/aws-arch-backoff-simulator/blob/66cb169277051eea207dbef8c7f71767fe6af144/src/backoff_simulator.py#L19-L53) gives the precise formulas, with `B = min(cap, base * 2^n)`:

- Full jitter: sample uniformly between `0` and `B`.
- Equal jitter: `B/2 + uniform(0, B/2)`.
- Decorrelated jitter: initialize `previous = base`; then `previous = min(cap, uniform(base, 3*previous))`.

Decorrelated jitter samples first and caps afterward. Sampling from `base..min(cap, 3*previous)` changes its distribution, eliminating the probability mass at the cap. Document integer endpoint and odd-number rounding choices in Hirsel. A generic decorrelated modifier must retain a stable base and previous sampled sleep; it cannot be implemented as an independent random multiplier of the current exponential delay.

Go and Rust backoff use symmetric jitter around the current interval, rather than AWS full jitter: `(1-r)*interval .. (1+r)*interval`. Go explicitly says its MaxInterval caps the unrandomized interval; randomized sleeps can exceed it ([source](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/exponential.go#L11-L30)). Hirsel `capped(max_delay)` should instead bound actual emitted sleeps, with modifier ordering documented.

The Rust [elapsed-time test](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/tests/exponential.rs#L54-L81) guards the important case: elapsed 900 ms plus a next delay of at least 500 ms cannot fit a 1000 ms budget. Go similarly checks the candidate delay against remaining time, includes time inside the operation, and does not interrupt an already-running operation for an elapsed limit ([retry](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/retry.go)). Cancellation is a separate concern: Go selects between its timer and the context and checks context again before another attempt.

Recommended Hirsel choices: delays and elapsed time are nonnegative integer nanoseconds; addition and multiplication saturate at the integer maximum before conversion to a duration. tokio-retry's [strategy implementation and overflow tests](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs#L45-L64) explicitly support saturating growth. Use subtraction rather than adding elapsed and delay to avoid overflow. Hirsel chooses a strict budget: reject `next_delay >= budget - elapsed` after checking `elapsed >= budget`, and recheck after waking so no further operation starts at or beyond the limit. Read time only through Clock and randomness only through Random. Reset execution state for every retry/repeat invocation. Define max_attempts as total operation calls, so `max_attempts(1)` makes one immediate call and no sleep, matching Go's [WithMaxTries contract](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/retry.go#L71-L80); specify the zero case explicitly.

## Applicable source vectors

These are numerical vectors restated from source tests; units can be scaled exactly to integer ns. Their source framework code is not copied.

| Vector | Expected result | Source |
| --- | --- | --- |
| Exponential base 500 ms, factor 2, cap 5000 ms | `500,1000,2000,4000,5000,5000,5000,5000,5000,5000` ms | [Go TestBackOff](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/exponential_test.go#L9-L38), [Rust backoff](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/tests/exponential.rs#L84-L105) |
| Fibonacci base 10, cap absent | `10,10,20,30,50,80` | [tokio-retry](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs#L235-L244) |
| Fibonacci base 10, cap 50 | `10,10,20,30,50,50` | [tokio-retry](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs#L255-L264) |
| Base greater than cap (20 vs 10) | `10,10` | [tokio-retry](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs#L229-L233) |
| Exponential near integer max | `MAX-1,MAX,MAX` | [tokio-retry](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/src/strategy.rs#L199-L206); adapt base/factor to Hirsel's `base*factor^n` because tokio-retry's `from_millis(base)` uses powers of base |
| Exponential base 100 ms, factor 2 | attempt timestamps `0,100,300,700,1500` ms | [Effect v3](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/test/Schedule.test.ts#L521-L527) |
| Exponential base 100 ms, factor 3 | attempt timestamps `0,100,400,1300,4000` ms | [Effect v3](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/test/Schedule.test.ts#L528-L534) |
| Fibonacci base 100 ms | attempt timestamps `0,100,200,400,700` ms | [Effect v3](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/test/Schedule.test.ts#L469-L475) |
| Fixed period 5 s, decisions at 0 and 6.5 s | next deadlines 5 s and 10 s | [Effect v3](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/packages/effect/test/Schedule.test.ts#L115-L123) |
| Elapsed 900 ms, next delay >=500 ms, budget 1000 ms | stop without sleeping or invoking again | [Rust backoff regression](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/tests/exponential.rs#L63-L81) |

Additional Hirsel tests should cover seeded exact full/equal/decorrelated traces, their integer endpoints, cancellation during a nonzero sleep, operation runtime consuming the elapsed budget, cap after jitter, and reuse of a Schedule value without sharing execution state. These are project-specific requirements; no upstream exact random vector applies because upstreams use different random generators or floating point sampling.

## Attribution and reuse

Effect, cenkalti/backoff, and tokio-retry are MIT licensed ([Effect](https://github.com/Effect-TS/effect/blob/e95695057b2446d5bc4de81bac0a77ef49d7a4c0/LICENSE), [Go](https://github.com/cenkalti/backoff/blob/6f228bfdb51209aa375869feb27dcede4fa0db06/LICENSE), [tokio-retry](https://github.com/srijs/rust-tokio-retry/blob/69a8766aaead1190c2a305937d45740d6c2d9b33/LICENSE)). Rust backoff offers MIT or Apache-2.0 ([MIT](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/LICENSE-MIT), [Apache](https://github.com/ihrwein/backoff/blob/587e2da8fb2dcfc65ca544cb9249022c51f1406e/LICENSE-APACHE)). AWS simulator is [Apache-2.0](https://github.com/awslabs/aws-arch-backoff-simulator/blob/66cb169277051eea207dbef8c7f71767fe6af144/LICENSE). No root NOTICE files were present in these clones.

If implementation or substantial test code is copied, preserve the selected source's copyright and license notice in the repository's third-party notices, and for Apache reuse follow its change-notice requirements. The Hirsel implementation should independently express these documented algorithms and numeric vectors; this note uses citations instead of importing upstream source or article images.
