# Free-mode protocol models

Model checks of the free-mode protocol in `crates/hirsel-free` (design §5 Op sections and the
cut, Claim versus withdraw). `check.sh` runs them all and fails unless every run has its
expected result; the gate runs it when these models or the kernel source they compile change.

- `loom/` compiles the kernel's `sync`, `phase`, `section` and `claim` modules from their own
  source against loom (`loom/src/sync.rs`), so the models check the code the runtime runs.
  Loom's `UnsafeCell` checks that no log, dirty list or scan position is touched concurrently,
  which is what lets a cutter read them while workers run. Models: every cut is causally
  closed (versions recorded are exactly 0..n, at a flush and at the stop, and nothing opens
  after it); an old receive against a new send; registration and end during a cut; the claim
  matrix with and without recording (a stop at every point of entered, claimed, the host call
  and done; exactly one of claim and withdraw; in doubt; quarantine; buffers freed once); and
  the home bridge's claim and close. Loom models a SeqCst access as AcqRel, so the shim pairs
  each with an SC fence; the `weak` feature drops the fences and must fail. Runs use one
  preemption: a second lets loom keep a spin wait reading a stale value forever, a liveness
  artefact of its unbounded-path limit.
- `genmc/` holds C11 models with the SC accesses exactly as the kernel writes them, checked by
  GenMC v0.19.0 under RC11 (and IMM for the gate), exhaustively: `gate.c` (two workers, a flush
  and the stop; GenMC's race detector checks the cutter's plain reads), `claim.c` (one request
  against a stop at every point, with and without recording, the request table's closed flag
  included; without recording it is also the home bridge), and `events.c` (old receive and new
  send, and a spawn's registration and the child's end on another thread, against two cuts).
  Mutations that must fail: `-DWEAK_ENTER` (a release store for `active := 1`) and
  `-DWITHDRAW_BLIND` (a plain store instead of the withdraw CAS), with and without recording.

GenMC is built once by `check.sh`, pinned by commit, into
`${XDG_CACHE_HOME:-~/.cache}/genmc-<commit>`; it needs cmake, clang and LLVM 19-22 with their
development files, and libffi. `GENMC=<path>` uses another build.
