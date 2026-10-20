# hirsel-free

The free-mode protocol of `tasks.free@1` (docs/spec/determinism.md, S-12): what an engine
running tasks freely records, how a cut is taken while tasks run, and how a tape is replayed.
It is backend-neutral: the runtime and the interpreter drive the same recorder and replayer,
and F1's threaded runtime uses the same gate, logs and claim words. Allowed dependencies: `std`
only (`LeafUtility`, enforced by `hirsel-lir/tests/it/dependencies.rs`).

- `phase`: op phases (entered, claimed, attempt i, ended i, cleanup i, done) and a task's
  published progress (k, phase, i), packed into one word, ordered within an op.
- `section`: op sections and the cutter. A section is a bounded run that acts, records and
  publishes; the cutter's SC handshake (`active`/`cut`) closes the gate so every cut it reads is
  causally closed; a stop's cut stays closed. Logs are chained fixed segments.
- `claim`: issue ownership. A request's state word arbitrates the issuer's
  `Prepared -> ClaimedForIssue` against the stop's `Prepared -> Withdrawn`; a claimed request at
  the stop is in doubt; a later completion is quarantined; the record owns the buffers. The home
  bridge uses the same records.
- `record`, `tape`: the compressed records (nonzero object gaps only, load values, `await_any`
  indices, cancellation changes, expiries, thaw bits, `Prepared`, host-key epoch gaps,
  observations and completions, `blocked`/`ready`) and the tape of CRC-framed flush and stop
  chunks; a torn final frame reads as the last flush's cut (a truncated tape).
- `keys`: mode-aware host-key epochs.
- `pending`: a task's pending `Shared` releases, coalesced per root. A walk that ends a phase
  applies its nested releases at once (see the module's Walks note: deferring them to the
  walker strands them in some replays).
- `projection`: the normalised projection a tape is checked against, built from stamped events
  filtered by a cut.
- `recorder`, `replay`: the two sides an engine calls at each op: admission, enablement (object
  version = p + gap; key epochs), substitution and validation.

Invariants:

- A record is written in the section of the phase it belongs to, so a cut that admits a phase
  holds its records, and an absent gap record of an admitted sub-event is gap 0.
- Nothing is published outside a section, and no section opens after the stop's cut.
- Exactly one of claim and withdraw wins per request.

Tests: `tests/it` runs a reference cooperative executor (width 1, one permit, a seeded choice
of the next task's phase at every step, injected flushes and stops) over the F0 traces: the
gap boundary on a fresh object, the finite release with an infinite suffix, the mutex cycle,
the parked receive at a durable cut, the nested root at task end, acquire / wait / acquire /
reply, and stops at every step with requests in flight. Each live run's tape replays under
other seeds to the live projection, terminal and durable, and every truncated or torn prefix
replays to the live projection at its cut. Two mutations (releases applied at once; phase
walks deferred) must fail. `proofs/free/check.sh` model-checks `sync`, `phase`, `section` and
`claim` with loom (from this source) and the handshake and claim protocol with GenMC.
