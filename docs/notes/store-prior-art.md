# std@1/store: transactions and compaction against SlateDB

Reference: slatedb/slatedb `a5c4fd22b36c836a89d8c4f27bf0b7b045e62e32` (Apache-2.0).

## Per-key conflict detection

`std@1/txn_manager` ports SlateDB's `TransactionManager`
(`slatedb/src/transaction_manager.rs`):

| Hirsel | SlateDB |
|---|---|
| `Isolation = SnapshotIsolation \| SerializableSnapshot` | `IsolationLevel` (`:13`) |
| `begin`, `begin_at` | `new_transaction` (`:112`), `new_txn_with_id` (`:130`) |
| `end` | `drop_txn` (`:151`) |
| `has_conflict` | `check_has_conflict`, `has_write_write_conflict`, `has_read_write_conflict` (`:190`, `:324`, `:350`) |
| `track_committed`, `track_batch` | `track_recent_committed_txn` (`:219`), `track_recent_committed_write_batch` (`:241`) |
| `min_active`, `recycle` | `min_active_seq` (`:259`), `recycle_recent_committed_txns` (`:296`) |
| `txn.get` / `txn.range` record a key / the asked range under `SerializableSnapshot` | `DbTransaction::get_key_value_with_options` (`db_transaction.rs:150`), `scan_inner` (`:299`) |
| `txn.mark_read` | `DbTransaction::mark_read` (`:410`) |
| `store.rollback` | `DbTransaction::rollback` (`:625`) |

`store.commit` checks the transaction's write keys, read keys and read ranges
against the write sets of commits made after it began, as `commit_with_options`
(`db_transaction.rs:572`) and the batch writer do; disjoint transactions now
both commit. `store.transact` and `store.call` hold the store from begin to
commit, so nothing can land in between: they skip the check, and when no
transaction is open the store keeps and builds no write sets.

### Deviations

- **`store.begin` is serializable.** SlateDB's default is `Snapshot`. Before
  this change every commit since `begin` conflicted, which is serializable;
  defaulting to snapshot isolation would have silently admitted write skew.
  `begin_with(SnapshotIsolation())` gives SlateDB's default.
- **Bounded retention and a horizon.** SlateDB frees an active transaction's
  state when it is dropped. A `Txn` is a copyable value with no drop hook, so
  an abandoned one never ends. The manager keeps at most 4096 write sets or
  1 MiB of their keys (`Limits`); past a limit it drops the older half, raises
  the horizon past them and forgets the transactions that began before it,
  and such a transaction conflicts whenever it writes or read anything. A copy
  of a committed transaction shares its id; the store therefore records every
  commit as a batch after ending the id, so a diverged copy's write set is
  never lost.
- **Key sets travel with the transaction.** `has_conflict` and
  `track_committed` take ascending key lists from the `Txn` instead of
  accumulating them in the manager (`track_write_keys`, `track_read_keys`).
  So `merges_conflict_keys` has no counterpart, and the committed record
  keeps only its sequence and keys (no `started_seq`).
- **Recycling drops a prefix.** Sequences are tracked in ascending order, so
  `recycle` drops the leading write sets older than the oldest active
  transaction (SlateDB filters with `retain`); the kept boundary matches
  (`committed_seq >= min`). `committed_seq: None` cannot be represented, so
  `test_recycle_recent_committed_txns_handles_none_committed_seq` is not
  carried.
- **Not carried:** `unmark_write` and merge operators (Hirsel has no merge
  writes), TTLs, `commit_returns_before_durable` (the output gate is
  `store.durable`, tested by SEM-5 group commit), the proptest invariants
  beyond what the table cases and flows pin, and SlateDB's empty-batch commit
  returning no handle (a Hirsel commit without writes still takes a sequence).

### Vectors

- `conformance/STORE/STORE-2-txn-manager-run.hir`: the 8 conflict-table
  cases, the 3 `min_active_seq` cases, `new_transaction`, the 3 `drop_txn`
  tests, 3 of the 4 `track_recent_committed_txn` cases, 4 recycle tests, the
  lifecycle, concurrent and GC-timing flows, the SSI phantom range test, and
  the retention limits.
- `conformance/STORE/STORE-2-txn-isolation-run.hir`: all 20
  `test_txn_table_driven` cases, run through the store with plain writes as
  `store.transact` commits (about 1.9 million steps, so the case raises its
  budget to 3 million).
- `conformance/STORE/STORE-2-txn-conflicts-run.hir`: the 9 conflict tests from
  `test_txn_si_commit_conflict` to
  `test_txn_ssi_scan_prefix_subrange_tracks_composed_range`, and Hirsel's own:
  serializable `begin` refuses write skew, disjoint transactions commit in any
  order, rollback.

## Fencing (`fence.rs`)

The safety properties match: a new writer leaves an empty fence log as the
newest log object, log ids stay contiguous, the fenced writer's next write
fails `Fenced()` and every later call `Closed()`, a writer that fenced
another is fenced in turn by a third, and a reopen holds every acknowledged
commit and no stale one (`DET-1-std-store-fence-chain-run`, after
`test_fence` and `test_fencer_fenced_after_claiming_epoch`). Two deviations,
both kept:

- The fence log is created before the manifest, and the fence log itself
  carries the new epoch (recovery takes the larger of the manifest and log
  epochs). SlateDB first bumps the epoch in the manifest by compare-and-swap
  (`FenceableManifest::init_writer`), then writes the fence WAL. A `Dir` and
  an S3 object store offer create-if-absent, not compare-and-swap on a mutable
  manifest, so the manifests here are immutable numbered objects.
- When the fence id is already taken, `open` fails with `Fenced()`. SlateDB
  refreshes the manifest and retries at the next WAL id
  (`wal/slatedb/writer_init.rs:107-150`) because its epoch is already
  exclusive. Here the taken id may belong to a concurrent opener at the same
  epoch, so the opener cannot retry safely; its caller can reopen. This costs
  liveness under racing opens, never safety.

## Memory and compaction cost

`bench/store/measure.hir`, built with `hirsel build --release`, loads `n` keys
(16-byte keys, 100-byte values) through `store.transact`, 100 keys per commit,
on a real directory, run under `perf stat -e instructions:u` and
`/usr/bin/time` on CPU 7. Instruction counts repeat to within 0.05%. The table
is the store before this change, which leaves compaction as it was.

| n | load (instr) | full compaction (instr) | compaction after 1 more key (instr) | snapshot bytes | max RSS |
|---|---|---|---|---|---|
| 1,000 | 18.85 M | 3.47 M | 3.35 M | 132,042 | 12.3 MB |
| 10,000 | 188.6 M | 34.2 M | 32.0 M | 1,320,042 | 12.5 MB |
| 100,000 | 1,955.3 M | 372.1 M | 317.9 M | 13,200,042 | 34.3 MB (60.2 MB while compacting) |

- **Memory.** The whole index is resident: about 220 B of RSS per entry for
  116 B of data at 100,000 entries, and compaction encodes the whole snapshot
  in memory, about doubling the peak. Linear, so 1M entries is ~220 MB.
- **Compaction.** Every `compact` encodes and writes every entry: 3.7k
  instructions per entry, and nearly as much after one new key as after
  100,000 (the "after 1 more key" column is all compaction but one commit). That is about 19% of the
  instructions it took to load the entries.
- **This change.** Plain `transact` loads cost +0.035% instructions (old
  1,955.32 M, new 1,956.00 M at 100,000). With one serializable transaction
  held open through the load, so every commit's write set is retained up to
  the limits, they cost +2.5% instructions and +4.3 MB RSS.

**Decision: SST-style incremental compaction is deferred.** Compaction is
explicit and costs O(n) in time and in peak memory, but at the sizes the store
serves (an embedded store for an agent's state, ~1e5 entries) one compaction
is 0.37 G instructions. Incremental compaction only pays once compaction time,
or the resident index, outgrows the commits it serves, and fixing memory needs
more than compaction: reads must come from on-disk blocks (block cache,
filters and merging iterators over sorted runs, SlateDB's engine).
Revisit when a workload passes ~1M entries (~220 MB resident) or compacts
often enough that compaction instructions exceed its commits'. The cheaper
first step is a delta snapshot (SlateDB's L0 SST) that writes only the keys
changed since the last snapshot.
