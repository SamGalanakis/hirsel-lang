# Storage

**Status: partly shipped.** `std@1/store` ships the engine and its storage seam
(STORE-1, STORE-11), transactions and the output gate (STORE-2, STORE-3), the key encoding
(STORE-6, `std@1/keys`), recording (STORE-10), the store's side of call streams
(STORE-12, `store.call`) and the failure types (STORE-13); the [standard
library](std-library.md#stores) lists each shipped operation. Until typed tables ship, a
body reads and writes byte keys with `store.tx_get`, `store.tx_put`, `store.tx_delete` and
`store.tx_range`. Typed tables, rows, snapshot views, schema history and migrations
(STORE-5, STORE-7 to STORE-9), read-only opening and `store.for_test` are planned: a
program that names them does not resolve yet. Items marked **Reserved** are designed here
but have no implementation owner yet.

The S3 backend and its grants are implemented in `packages/s3@1`.

A **store** is an embedded, transactional, ordered key-value database kept in a directory.
It is ordinary Hirsel code in `std@1/store`: a log-structured engine that writes immutable,
checksummed objects through a backend function dictionary and keeps current state in an
in-memory ordered index. The local backend owns a `Dir`; the S3 package backend owns
explicit Net and clock grants and credential data. The host selects those grants in
the run configuration, and effects retain ordinary file or network recording and replay. Programs change a store
only inside a transaction, whose body holds nothing but a scoped `Tx` and data. Rows live in
typed tables: keys in an order-preserving encoding, values as canonical codec payloads,
each table with a recorded schema history. A transaction's outcome is released only once
every commit it observed is durable.

No core operation, TIR or LIR construct is added for stores. `Tx` is the scoped capability
of [MODE-6](ownership.md#mode-6-scoped-capabilities); `std@1/store` declares it, together
with `Store`, `Snapshot`, `Table`, `TxFailure` and the other types below.

### STORE-1: Stores

```text
fn store.open(d: Dir, config: StoreConfig) -> Result[Store, StoreError]
fn store.close(take s: Store) -> Result[(), StoreError]
```

`store.open` opens or creates the store kept in `d` and claims it as its writer
([STORE-11](#store-11-engine-and-storage-seam)): it recovers the newest intact state,
publishes the next **writer epoch**, and returns a `Store` holding `d` and the in-memory
state. `Store[S]` is an ordinary affine opaque type with owned backend state and
explicitly captured dictionary authority; it exports no copying or sending bound, and every
operation that changes it takes it `mut`. One `Store` value is therefore the store's only
writer in its run, and a second opener of the same directory, in this process or any
other, fences the first: the first's next write fails and it closes with `Fenced()`. Other
runs and hosts reach a store only through its holder.

A `Dir` granted `read` opens a store read-only (planned): recovery runs, no epoch is
claimed, and every write returns `ReadOnly()`; until then such an open fails with the
`Dir`'s own `Storage(..)` error. Holding a `Store` grants nothing outside its directory.
`store.close` waits until every commit is durable and releases the in-memory state; a
store that is dropped without `close` loses only commits that were not yet durable.

`StoreConfig` is the configuration of the store provider `db.local@1`, the engine of
`std@1/store`; its members and defaults are that provider's own
([STORE-11](#store-11-engine-and-storage-seam)).

### STORE-2: Transactions

```text
fn store.transact[R: inert, E: inert](mut s: Store, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
```

`store.transact` runs `body` once against `s` and commits its writes atomically when the
body returns `Ok`:

1. The body runs with a fresh `Tx` at read version `r`, the latest commit version
   ([visibility](#visibility-and-the-output-gate)), and sees every commit up to `r` and its
   own earlier writes.
2. `Ok(v)`: the writes commit at the next commit version. `Error(e)`: the writes are
   discarded and the outcome is `Error(Aborted(e))`. A language failure in the body
   discards the writes and propagates ([FAIL-2](failure.md#fail-2-boundary)); nothing is
   committed.
3. The outcome is released to the caller by the output gate below.

Because the `Store` is taken `mut`, transactions on one store are serial: there are no
conflicts and no retries. Concurrent tasks share a store by sending requests to the task
that holds it, or, across runs, through a runner ([STORE-12](#store-12-transactions-at-a-runner)).
`store.transact` performs no cancellation check
([CONC-3](concurrency.md#conc-3-cancellation)): a transaction started from a cancellable
scope is checked by its caller. Only the runner form returns `Canceled(cause)`, before its
body starts and with no effect ([STORE-12](#store-12-transactions-at-a-runner)). A commit that exceeds a
configured limit returns `Limit(..)` with no effect. A commit whose durability cannot be
decided ([STORE-11](#store-11-engine-and-storage-seam)) returns `Unknown(..)`: the store
closes, and whether the commit survived is decided when the store is next opened, never
guessed.

#### Visibility and the output gate

Commits on one store are numbered by consecutive **commit versions**. The store's
**visible version** `V` is the highest commit applied to the in-memory index; its
**durable version** `D` is the highest commit version such that it and every lower one are
durable. `D` never decreases and never exceeds `V`.

- **Visibility.** A transaction begins at read version `r = V` and observes every commit
  up to `r`, durable or not. A transaction may therefore read committed state that is not
  yet durable (group commit, [STORE-11](#store-11-engine-and-storage-seam)).
- **Output gate.** A transaction's **gate version** `g` is its own commit version when it
  committed, and `r` otherwise (read-only, aborted or limit-refused). Its outcome, of any
  kind, reaches the caller only when `D >= g`.
- **Crash.** A crash loses exactly the commits above `D`. Every transaction that observed
  a lost commit has `g` above `D`, so its outcome was never released: no released outcome
  depends on lost state.

### STORE-3: Transaction scope

`Tx` is a scoped capability ([MODE-6](ownership.md#mode-6-scoped-capabilities)): it cannot
be returned, stored, captured by an escaping closure or sent to another task. The body
receives it `mut`: reads borrow a `Tx` and writes take it `mut`, because a borrowed value
cannot collect a write set without a core primitive. The body is
`uses {}` and its result `inert` ([CAP-3](capabilities.md#cap-3-restriction)), so the body
holds only its `Tx` and data, keeps nothing, returns no authority, performs no effect
outside the store and cannot open a nested transaction. A capture the body's `uses {}`
excludes is CAP-3; a `Tx` that escapes is MODE-6. Because a body has no other effect, a
store may run it again, against the same state, to verify or replay it.

Table operations are not cancellation points and do not suspend; once a body starts, the
transaction ends only by its own outcome. A long body therefore delays every later
transaction on its store, which the store's limits bound.

### STORE-4: Call information

**Reserved**, with the transact form of runner calls
([STORE-12](#store-12-transactions-at-a-runner)).

```text
CallInfo   // opaque: copyable, data, key, inert, sendable; minted only by a store's holder
fn store.info(tx: Tx) -> CallInfo
fn call_info.caller(c: CallInfo) -> String
fn call_info.at(c: CallInfo) -> i64
fn call_info.seed(c: CallInfo) -> u64
fn call_info.for_test(t: Test, caller: String, at: i64, seed: u64) -> CallInfo
```

A store's holder mints one `CallInfo` for each transaction it runs, records it, and hands
it to the body: as the `call` argument of a transact callee, and through `store.info(tx)`
to any body, so one top-level body serves both. Because only a store's holder or a test
constructs one, a body can trust it as the place's statement of:

- `caller`: for a runner caller, the identity the place admitted: `run:` followed by the
  32 lowercase hexadecimal digits of the run instance ([RUN-11](runners.md#run-11-linked-tapes))
  for an in-process or child-process caller, or the caller name the runner host's
  configuration assigns to an authenticated remote client; the empty string for a
  transaction the holder runs itself;
- `at`: the commit-order time, signed Unix nanoseconds sampled from the holder's clock
  when the transaction begins and raised to the previous transaction's `at` when it is
  lower, so it never decreases on one store; 0 when the holder has no clock;
- `seed`: eight bytes, little-endian, by default the first eight bytes of
  `H("hirsel.store.seed.v1", store identity, read version)`; the default is predictable
  and never protects a secret, and a holder may draw it from `random.system@1` instead.

`CallInfo` is the only way time, randomness or caller identity enter a body, and it does
so as recorded data.

### STORE-5: Tables

```text
Table[K, V]   // opaque: copyable, data, key, inert, sendable; holds a table name
RangeBound[K] = Unbounded() | Included(K) | Excluded(K)
ScanOrder     = Ascending() | Descending()
```

A `Table[K, V]` is inert data naming a table and its expected key and value types; it
carries no authority, and every operation on it takes a `Tx`. Every operation requires
`K: key + serializable` and `V: serializable`.

```text
fn table.named[K: key + serializable, V: serializable](name: String) -> Table[K, V]
fn table.create[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V]) -> Result[(), TxError]
fn table.drop[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V]) -> Result[(), TxError]
fn table.get[K: key + serializable, V: serializable](tx: Tx, t: Table[K, V], key: K) -> Result[Option[V], TxError]
fn table.put[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V], key: K, value: V) -> Result[(), TxError]
fn table.delete[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V], key: K) -> Result[bool, TxError]
fn table.range[K: key + serializable, V: serializable](tx: Tx, t: Table[K, V], from: RangeBound[K], to: RangeBound[K], limit: u64, order: ScanOrder) -> Result[List[(K, V)], TxError]
fn table.history(tx: Tx, name: String) -> Result[List[SchemaEntry], TxError]
```

- `table.named` checks nothing; a table name is valid when it is nonempty UTF-8 of at most
  255 bytes, contains no NUL, and does not begin with `hirsel.`, the namespace reserved
  for the store's own tables.
- `create` makes an empty table whose schema history holds one entry for `K` and `V`;
  `drop` removes the table, its rows and its history. There is no implicit creation.
- `get` returns a fresh owned copy of the value stored under `key`, or `None`. `put`
  inserts or replaces. `delete` removes the row and returns whether it existed.
- `range` returns the rows whose keys lie between `from` and `to` in key order
  ([STORE-6](#store-6-key-encoding)), ascending or descending, at most `limit` of them; a
  caller pages by passing `Excluded(last key)`. Rows are fresh owned data.
- `history` returns the named table's schema history ([STORE-9](#store-9-schema-history-renames-and-migrations)).

Each operation checks, in order: argument validity (a valid name; `limit` of at least 1),
else `InvalidInput()`; access (a write in a read-only transaction or store), else
`ReadOnly()`; the table exists (or, for `create`, does not), else `NoTable()` or
`TableExists()`; the table's current key and value schemas match `K` and `V`
([STORE-7](#store-7-rows-and-storage-identity)), else `Schema(..)`; a write to a table
under migration, else `Migrating()`; and the configured limits, else `Limit(..)`. A failed
operation changes nothing, and the transaction continues; the body decides whether to
abort. There are no per-table grants: the authority over a store is its `Dir`, and
per-table footprints in signatures are reserved for a later version.

```hirsel
@serializable
distinct type Sku(v: u64)

@serializable
type Reservation { sku: Sku, count: u32 }

@serializable
type StockError = OutOfStock(Sku) | Db(TxError)

fn stock() -> Table[Sku, u32] {
  table.named("stock")
}

fn reserved() -> Table[Sku, u32] {
  table.named("reserved")
}

pub fn take_stock(mut tx: Tx, req: Reservation) -> Result[Reservation, StockError] {
  let stocked = table.get(tx, stock(), req.sku) ? Db
  let have = match stocked { Some(v) => v, None => 0 }
  if have < req.count { return Error(OutOfStock(req.sku)) }
  table.put(mut tx, stock(), req.sku, have - req.count) ? Db
  let held = table.get(tx, reserved(), req.sku) ? Db
  let prior = match held { Some(v) => v, None => 0 }
  table.put(mut tx, reserved(), req.sku, prior + req.count) ? Db
  Ok(req)
}

pub fn reserve(mut s: Store, req: Reservation) -> Result[Reservation, TxFailure[StockError]] {
  store.transact(mut s, fn(mut tx: Tx) { take_stock(mut tx, req) })
}
```

### STORE-6: Key encoding

Table keys are stored in the **key encoding**, `hirsel.key.v1`, a schema-directed encoding
whose unsigned byte-lexicographic order is the key order. It is separate from the
canonical codec, whose little-endian integers and length prefixes do not sort
([SER-2](serialization.md#ser-2-canonical-codec)). `std@1/keys` implements it
(`keys.encode`, `keys.decode`), and libraries use it for keys they build themselves, such
as secondary indexes kept as tables.

| Schema constructor | Encoding |
|---|---|
| `bool` | `00` for false, `01` for true |
| `u8` to `u128` | big-endian, exactly the type's width |
| `i8` to `i128` | big-endian two's complement with the sign bit inverted, exactly the type's width |
| `f32`, `f64` | the IEEE bits, big-endian, after the total-order transform: a value whose sign bit is set has every bit inverted, any other has only its sign bit flipped; the NaN is the canonical one ([SER-2](serialization.md#ser-2-canonical-codec)) |
| `char` | the scalar value as a big-endian `u32` |
| `String`, `Bytes` | each byte in order, with `00` written as `00 FF`; then `00 01` |
| unit | nothing |
| tuple, record | the fields in declaration order |
| variant, including core `Option` and `Result` and `Packed[T]` | the arm's declaration ordinal as a big-endian `u32`, then its fields in order |
| `box T`; an opted-in transparent nominal or distinct type | the payload's encoding |
| `Array[T]`, `List[T]` | for each element, `01` then the element; then `00` |
| `Inline[T, n]` | the `n` elements in order |

Every encoding is self-delimiting given the schema, so a tuple's encoding is the
concatenation of its fields'. **Key order** is defined by the same structure: integers and
`char` numerically; floats in IEEE total order, so `-0` precedes `+0` and the canonical NaN
follows every other value; `false` before `true`; `String` and `Bytes`
byte-lexicographically with a proper prefix first (the order of the
[SEM-5](semantics.md#sem-5-maps) operators); tuples and records lexicographically by field;
variants by arm ordinal and then payload (so `None` precedes every `Some`); and sequences
lexicographically with a proper prefix first. For all values `a` and `b` of one schema, `a`
precedes `b` in key order exactly when the encoding of `a` precedes the encoding of `b`
bytewise. A table key satisfies `key`, which excludes floats
([GEN-1](generics.md#gen-1-bounds)), so floats occur only in keys a library encodes itself.

`Map`, `Set` and `Simd` have no key encoding: the codec's `Set` order is not the key order,
so encoding them as sorted sequences would mislead. A key schema containing one is
STORE-6 at the use that instantiates the table operation. Decoding a stored key validates
it completely (escapes, terminators, `bool`, `char` and UTF-8 validity, float canonicity,
tags and trailing bytes); a stored key that does not decode is `Storage(Corrupt())`.

### STORE-7: Rows and storage identity

A row's value is stored as a `u32` little-endian schema epoch followed by the canonical
codec payload of the value, with no envelope; the table's schema history holds the
descriptors once ([STORE-9](#store-9-schema-history-renames-and-migrations)).

A table's schema is identified by **names and shape**: the package, module, type, field
and variant names of every nominal type it reaches, and its structure, with every package
version text excluded. The **storage descriptor** of a type is its concrete schema
descriptor ([SER-2](serialization.md#concrete-schema-descriptor)) with the exact version
text of every Nominal node replaced by the empty text atom. A schema matches a stored one
when their storage descriptors are equal after record fields are matched **by name**:

- a package version change that keeps names and shape needs nothing;
- reordering a record's fields needs nothing: fields are matched by name, and a stored row
  is read in its stored field order;
- a name that differs (a package, module, type, field or variant renamed) refuses with
  `Schema(Renamed(..))`, which names the stored and the expected spelling as a suggested
  rename, until the table's history declares that rename
  ([STORE-9](#store-9-schema-history-renames-and-migrations));
- any other difference (a field added, removed or retyped, an arm added or removed or
  reordered) refuses with `Schema(KeyMismatch())` or `Schema(ValueMismatch())` until a
  migration changes the table's schema.

Storage identity, like a descriptor digest, establishes no nominal type equality and
grants nothing. Reading a row checks that its epoch is one the table's history maps to the
current schema and decodes the payload against the operation's `V` with every SER-2
validation, without loading a type, a constructor or a decoder from the store. A row that
fails either check is `Storage(Corrupt())`.

### STORE-8: Snapshots

```text
fn store.snapshot(s: Store) -> Snapshot
fn store.view[R: inert, E: inert](snap: Snapshot, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
```

`Snapshot` is a persistent, immutable view of a store's tables at its durable version `D`
when taken; it is `copyable`, `inert` and `sendable`, and holds no authority. Taking one
costs O(1): the in-memory index is a persistent ordered map shared structurally between
versions. `store.view` runs `body` against the snapshot with a read-only `Tx`; writes
return `ReadOnly()`, and the outcome is `Ok`, `Aborted` or `Canceled`. A view never waits
for the store's writer, never touches the store's directory, and needs no output gate,
because a snapshot holds only durable state. Read-only work therefore runs on any task, in
parallel with transactions on the store.

### STORE-9: Schema history, renames and migrations

```text
SchemaEntry     = { epoch: u32, key_schema: String, value_schema: String, change: SchemaChange }
SchemaChange    = Created() | Renamed(from: String, to: String) | Migrated(definition: String, code: String)
MigrateError[E] = Failed(E) | Collision() | Table(TxError) | Canceled(CancelCause)

fn table.rename(mut tx: Tx, name: String, from: String, to: String) -> Result[(), TxError]
fn table.migrate[J: key + serializable, A: serializable, K: key + serializable, B: serializable, E: serializable](mut s: Store, from: Table[J, A], to: Table[K, B], f: Entry[fn(key: J, old: A) -> Result[(K, B), E]], batch: u64) -> Result[u64, MigrateError[E]]
```

Every table keeps its **schema history**: one entry per schema it has had, numbered from
epoch 0, each with the SHA-256 (64 lowercase hexadecimal digits) of the key and value
storage descriptors and the change that produced it. The last entry is current. Entries are
never edited or removed while the table exists.

**Renames.** `table.rename` declares, once, that the spelling `from` (a full nominal path,
or a field or variant name qualified by its type's path) is now spelled `to` in the named
table's schemas. It appends an entry whose descriptors are the current ones with the
rename applied, and rewrites no row: rows keep their epoch, and reads map the old
spelling through the history. A rename that does not occur in the current schema is
`InvalidInput()`.

**Migrations.** Any other change of a table's key or value schema is made only by
`table.migrate`, whose `f` is a pure top-level function ([RUN-3](runners.md#run-3-entries))
from an old row to a new one. `from` and `to` must name the same table, and `batch` must be
between 1 and the store's `range_rows` limit, else `Table(InvalidInput())`. Then:

1. A table whose current schema already matches `K` and `B` returns `Ok(0)`. Otherwise its
   current schema must match `J` and `A`, else `Table(Schema(..))`.
2. One transaction marks the table as migrating, with the target descriptors, the identity
   of `f` (its definition path and code hash, [RUN-8](runners.md#run-8-identity-and-arrival))
   and a cursor. From then on writes to the table return `Migrating()`; reads through
   `from` continue.
3. Successive transactions each read up to `batch` rows after the cursor in key order,
   apply `f` to each in order, write the results into a shadow table under the new
   schema, and advance the cursor. Cancellation is checked between these transactions.
4. One final transaction replaces the table's rows with the shadow's, appends the new
   schema entry, clears the mark, and the call returns `Ok(n)` with the number of rows
   migrated.

If `f` returns `Error(e)`, two rows map to the same new key, or the scope is cancelled,
one transaction discards the shadow and the mark, and the call returns `Failed(e)`,
`Collision()` or `Canceled(cause)`; the table is unchanged. A failure in `f` propagates and
leaves the migration pending; a later `table.migrate` with the same `from`, `to` and `f`
identity resumes it from its cursor, any other one returns `Table(Migrating())`, and
opening the store discards every pending migration. Key migrations are allowed, since `f`
maps the key too. Tooling may propose migrations for additive changes; the store applies
none on its own.

### STORE-10: Recording

A store is Hirsel code over a `Dir`, so its effects are recorded as the file effects it
performs ([determinism](determinism.md#recording-and-resuming)): each object it creates,
reads, lists or removes, with the exact bytes read. A recording therefore holds every byte
the store loaded, including the rows any transaction read, and is as secret-bearing as the
store. Replay re-runs the engine and every body against the recorded completions; it never
opens a real directory, and it refuses a request that differs from the recorded one in any
byte. A recording never copies the mounted tree into its header. The recording level never
changes what a transaction does.

At the `durable` level ([RUN-14](runners.md#run-14-recording-levels)) a commit's
durability point is its log object's create, journaled like any file effect. A crash
between that create's `Prepared` and `Completed` records leaves it in doubt for recovery,
which resolves it by reading the object back: the create is whole or nothing and its bytes
carry the writer's nonce, so an object with equal bytes proves the commit durable and an
absent one proves it never happened. That is a provider recovery procedure
[recording and resuming](determinism.md#recording-and-resuming) allows.

### STORE-11: Engine and storage seam

The engine is `db.local@1`, the provider of `std@1/store`. Its **storage seam** is an
explicit `Backend[S]` record of functions over owned state `S`; `store.open(d, config)`
selects `local_backend()` with state `Dir`. The local implementation uses these operations:

- `file.put_new(d, path, bytes, mode)`: create an immutable object only if no object has
  that path, whole or nothing; an existing object with equal bytes is success, any other is
  `AlreadyExists`. `Durable()` returns once the object and its directory entry are durable;
  `Volatile()` returns without a barrier.
- reading a whole object, listing a prefix in byte order, and removing an object.

There is no append, no rewrite in place, no rename, no lock and no read at an offset. A
host chooses the medium by the mount it grants ([RUN-13](runners.md#run-13-provider-seams)):
a host directory by default; a simulated directory for tests, whose crash drops every
unsynced write, tears the last write at a seeded byte and loses unsynced directory entries;
or an **object-storage backend**, which presents the same operations over explicit
network grants, mapping `put_new` to a conditional create (`If-None-Match: *`) and a
lost response to a read-back. The pure Hirsel package `s3@1` supplies the S3 backend; std never depends on it.
The host `ObjectStorage` trait is reserved for code fetching and is not this seam.
The run-config `object_stores` declaration below supplies S3 grants.
The S3 backend probes conditional-create support before opening the store. An
endpoint that ignores it refuses open with `ConditionalUnsupported(endpoint)`;
raw S3 GET and LIST remain usable directly.

`Backend[S]` has callbacks `read`, `put_new`, `list`, `remove` and `mkdir`, each
taking `mut state: S` and a relative name. `list` returns full relative names sorted
by bytes. `open_backend` owns state and dictionary. `Store[S]` is affine ordinary
opaque state with captured backend authority, rather than a resource destructor:
only explicit `close`, `durable` and `into_state` flush. The local callback maps
to Dir operations; the S3 callback maps to HTTP, and the simulated callback injects
pre-write failures, lost responses, racing conditional creates and virtual latency.
They run exactly the same engine.

**Layout.** Under its directory the store keeps `manifest/`, `log/` and `snap/`, each a
sequence of immutable objects named by fixed-width, zero-padded decimal ids, so byte order
is numeric order. Every object begins with a magic, a format version and the writer epoch,
ends with a CRC-32C checksum of its contents, and carries the writer's nonce.

- A **log object** holds one commit group: for each commit, its version and its ordered
  write set of puts and deletes on byte keys and values. Writing it with `put_new(Durable)`
  is the commit point of every commit in the group.
- A **snapshot object** holds a complete state at a commit version.
- A **manifest** names the writer epoch, the newest snapshot and the first log id after
  it. A new manifest is written only when a writer claims the store and at compaction,
  always with `put_new` at the next manifest id, never by overwriting.

**Claim and fencing.** Opening as writer recovers, then claims the next writer epoch by
`put_new` of a fence log object at the next log id and then of the next manifest naming
the new epoch. A writer whose next `put_new` finds its id taken by different bytes has been
fenced by a later claim: it fails with `Fenced()` and closes, releasing nothing further.

**Group commit and the durable version.** Commits apply to the in-memory index at once
(raising `V`) and queue; a queue is written as one log object when an outcome awaits its
durability or a group bound in the configuration is reached, never on a timer. `D` is the
highest commit version whose log object, and every earlier one, has been created
durably. A `put_new` whose outcome is lost is resolved by reading the object back; if that
read fails too, durability cannot be decided, the waiting transaction returns `Unknown(..)`
and the store closes.

**Recovery.** Opening reads the newest manifest whose checksum verifies, loads its
snapshot, and applies the log objects after it in id order. A last log object that fails
its checksum is a torn tail that was never acknowledged: it is ignored, and the next claim
writes past it. A corrupt object before the tail refuses the open with
`Storage(Corrupt())`, and when no intact state exists the open refuses rather than guess.
Recovery never re-executes a body.

**Compaction.** `store.compact` writes a snapshot object of the durable state, then the
next manifest naming it, and only after that manifest is durable removes the objects no
manifest at or after it reaches. A crash at any point recovers.

`db.local@1`'s configuration, `StoreConfig`, is its own: `durability` (`Durable()` by
default, or `Volatile()`, under which `D = V` at all times, outcomes are released without a
barrier and a crash may lose any suffix of commits), the group bound, and `limits`
(`key_bytes` 4096, `value_bytes` 1048576, `transaction_bytes` 16777216, `range_rows`
10000). There is no general durability setting: another store provider defines its own
settings and defaults. A store must survive, by detecting and refusing, never by returning
wrong data: the loss of anything not yet durable, torn and garbage writes, lost or
ambiguous creates, and corruption on read. These are file-effect outcomes, recorded like
any other.

The engine-level operations below serve libraries that build their own transaction
discipline on byte keys; `store.transact`, views and tables are built on them.

```text
fn store.begin(s: Store) -> Txn
fn store.begin_with(s: Store, isolation: Isolation) -> Txn
fn store.rollback(mut s: Store, take t: Txn) -> ()
fn txn.get(mut t: Txn, key: Bytes) -> Option[Bytes]
fn txn.mark_read(mut t: Txn, key: Bytes) -> ()
fn txn.put(mut t: Txn, key: Bytes, value: Bytes) -> ()
fn txn.delete(mut t: Txn, key: Bytes) -> ()
fn store.commit(mut s: Store, take t: Txn) -> Result[Ticket, StoreError]
fn store.durable(mut s: Store, ticket: Ticket) -> Result[(), StoreError]
fn store.compact(mut s: Store) -> Result[(), StoreError]
```

`store.commit` returns `Conflict()` when a commit after `t` began wrote a key `t`
conflicts on: one it writes or marked read under `SnapshotIsolation`, and also one it
read or one inside a range it read under `SerializableSnapshot`, `store.begin`'s level
(per-key detection after SlateDB's transaction manager). Otherwise it applies `t` and returns a `Ticket` for its commit version; `store.durable`
returns once `D` reaches the ticket's version, writing the queued group if needed. A
library that releases an outcome before `durable` returns breaks the output gate.

### STORE-12: Transactions at a runner

The store's side of a call stream ships as `store.call`; the runner form is **Reserved**.

```text
CallRequest    = { stream: Bytes, epoch: u64, position: u64, digest: Bytes, acknowledged: u64 }
CallFailure[E] = Tx(TxFailure[E]) | Denied() | Rejected()
fn store.call[R: inert + serializable, E: inert + serializable](mut s: Store, req: CallRequest, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, CallFailure[E]]
```

`store.call` is `store.transact` as one request on a call stream, decided as the contract
below says. The stream's row is kept under the reserved key `hirsel.streams/` followed by
the stream identity, as the canonical codec payload of its epoch, `applied` and
`acknowledged` positions and, for each retained position, its request digest and the codec
bytes of its outcome (`Ok`, `Aborted` or `Limit`). A request whose epoch exceeds the row's
raises it in the commit that decides the request, including a duplicate's, which then
commits the row alone. `Denied()` and `Rejected()` are the runner's `NotRun(Denied())` and
`NotRun(Rejected())`; `Tx(..)` is the transaction's own outcome. Every key beginning with
`hirsel.` is the store's: `store.tx_put` and `store.tx_delete` require a key outside it
([FACT-1](contracts.md#fact-1-contracts)). Command records and the verifier below remain
Reserved.

A transaction body can run at the place that holds a store, as the transact form of a
runner call ([RUN-4](runners.md#run-4-calls)):

```text
fn runner.transact[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(tx: Tx, call: CallInfo, a: A) -> Result[R, E]], a: A) -> Result[R, RunError[TxFailure[E]]]
```

`runner.transact` names `Tx`, `CallInfo` and `TxFailure`, which `std@1/store` declares, so
it is declared beside them. It needs a core serving primitive, so that the run holding a
`Store` can run an arrived entry against a `Tx` of its own transaction; that primitive and
the `db@1` provider built on it are not yet designed. The contract they must meet:

- **Call streams.** Each caller sends its transact calls to a place on an ordered call
  stream, so the state a store keeps grows with the number of callers, not of calls. A
  stream belongs to one caller run at one store; its identity is `H("hirsel.stream.v1",
  caller name, run instance)`. The store keeps, in its reserved table `hirsel.streams`,
  one row per stream: its `epoch`, its `applied` position, its `acknowledged` position, and
  the request digest and outcome bytes of each position above `acknowledged` and at most
  `applied`.
- A request carries the stream identity, the caller's epoch, a position, the request
  digest and the highest position whose outcome the caller has made durable. Positions
  start at 1 and are consecutive, and a caller has at most one request in flight per
  stream; a request that ends in `NotRun` leaves its position to the caller's next
  request. The store decides each request in the transaction that would run it: an epoch
  below the row's is `NotRun(Denied())`; a position at or below `acknowledged`, or above
  `applied + 1`, is `NotRun(Rejected())`; a position above `acknowledged` and at most
  `applied` returns its recorded outcome when the digest is equal and is
  `NotRun(Rejected())` when it differs; position `applied + 1` runs the body, and the same
  commit records the outcome, sets `applied`, raises `acknowledged` and drops the outcomes
  at or below it. An outcome that commits no writes is recorded in a transaction of its own
  that writes only the stream row.
- **Fencing.** A durable caller's executor holds an epoch for each stream, recorded in its
  journal header. A live continuation first commits, on each store the run used, a fence
  request that raises the stream's epoch and returns its `applied` position and retained
  outcomes. A `Prepared` position at or below `applied` then has its recorded outcome, and
  one above it proves the transaction never committed, so a transact is never in doubt. A
  caller ends its stream by acknowledging its last position; an administrator may purge
  abandoned streams, which the store records, and a purged stream's requests are
  unresolvable thereafter rather than "never run".
- **Commands.** Every commit records the SHA-256 of its physical write set, and every
  commit made by a transact call also records its command, `(definition path, code hash,
  stream, position, call info, argument bytes)`, in the reserved table `hirsel.commands`,
  retained as the holder's configuration says. Recovery stays physical (STORE-11).
  Commands serve two tools, never recovery: a verifier, which re-executes the recorded
  commands in commit order from a snapshot, each under its recorded code, and compares
  every write-set digest; and an upgrade trial, which does the same under a candidate
  revision and reports the first divergence. Code hashes are the definition hashes of
  [LANG-6](language.md#lang-6-signatures).
- A store that is fenced or whose durability cannot be decided ends every unreleased call
  with `InDoubt(Abandoned())`, which the caller's stream resolves.

### STORE-13: Failures

```text
TxFailure[E] = Aborted(E) | Canceled(CancelCause) | Storage(StoreError) | Limit(TxLimit)
             | Unknown(StoreError)
TxError      = NoTable() | TableExists() | Schema(SchemaError) | Migrating() | ReadOnly()
             | InvalidInput() | Limit(TxLimit) | Storage(StoreError)
SchemaError  = KeyMismatch() | ValueMismatch() | Renamed(stored: String, expected: String)
StoreError   = Storage(FileError) | Corrupt(String) | Fenced() | Conflict() | Injected() | Closed()
TxLimit      = KeyBytes() | ValueBytes() | TransactionBytes() | RangeRows()
```

Variant ordinals in the order written, `serializable` (`TxFailure[E]` when `E` is).
`TxFailure` is the outcome of a transaction: `Aborted(e)` when the body returned
`Error(e)`; `Canceled(cause)` before the body started; `Storage(..)` and `Limit(..)` when
the commit could not be made, with no effect; and `Unknown(..)` when the commit was
attempted and its durability could not be decided (STORE-11), which never claims an
effect or its absence: a create that failed with a file error, which may have happened,
is `Unknown(Storage(e))`, and the store's other errors prove the commit did not happen.
`TxError` is the outcome of one table operation inside a body. `Storage(e)` is a file error
from the medium, a full one included; `Corrupt(name)` that the named object failed a
checksum, decoding or structural check; `Fenced()` that a later writer claimed the store;
`Injected()` that the `StopAfterWrites` test seam refused a storage write; `Closed()` that
the store closed earlier. Every one but `Conflict()` closes the store. Conflicts and retry
exhaustion do not occur in `store.transact`, whose writer is serial; `Conflict()` is the
engine-level `store.commit`'s alone.

## Ledger

```text
// std@1/store; STORE-1..13, CAP-3/5, MODE-6, CONC-3, DET-1
fn store.open(d: Dir, config: StoreConfig) -> Result[Store, StoreError]
fn store.close(take s: Store) -> Result[(), StoreError]
fn store.transact[R: inert, E: inert](mut s: Store, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
fn store.call[R: inert + serializable, E: inert + serializable](mut s: Store, req: CallRequest, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, CallFailure[E]]
fn store.tx_get(tx: Tx, key: Bytes) -> Option[Bytes] in tx
fn store.tx_put(mut tx: Tx, take key: Bytes, take value: Bytes) -> ()
fn store.tx_delete(mut tx: Tx, take key: Bytes) -> ()
fn store.tx_range(tx: Tx, from: Bound[Bytes], to: Bound[Bytes], direction: Direction, limit: u64) -> List[(Bytes, Bytes)]
fn store.snapshot(s: Store) -> Snapshot
fn store.view[R: inert, E: inert](snap: Snapshot, body: fn(mut tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
fn store.begin(s: Store) -> Txn
fn store.begin_with(s: Store, isolation: Isolation) -> Txn
fn store.rollback(mut s: Store, take t: Txn) -> ()
fn txn.get(mut t: Txn, key: Bytes) -> Option[Bytes]
fn txn.mark_read(mut t: Txn, key: Bytes) -> ()
fn txn.put(mut t: Txn, key: Bytes, value: Bytes) -> ()
fn txn.delete(mut t: Txn, key: Bytes) -> ()
fn store.commit(mut s: Store, take t: Txn) -> Result[Ticket, StoreError]
fn store.durable(mut s: Store, ticket: Ticket) -> Result[(), StoreError]
fn store.compact(mut s: Store) -> Result[(), StoreError]
fn table.named[K: key + serializable, V: serializable](name: String) -> Table[K, V]
fn table.create[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V]) -> Result[(), TxError]
fn table.drop[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V]) -> Result[(), TxError]
fn table.get[K: key + serializable, V: serializable](tx: Tx, t: Table[K, V], key: K) -> Result[Option[V], TxError]
fn table.put[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V], key: K, value: V) -> Result[(), TxError]
fn table.delete[K: key + serializable, V: serializable](mut tx: Tx, t: Table[K, V], key: K) -> Result[bool, TxError]
fn table.range[K: key + serializable, V: serializable](tx: Tx, t: Table[K, V], from: RangeBound[K], to: RangeBound[K], limit: u64, order: ScanOrder) -> Result[List[(K, V)], TxError]
fn table.history(tx: Tx, name: String) -> Result[List[SchemaEntry], TxError]
fn table.rename(mut tx: Tx, name: String, from: String, to: String) -> Result[(), TxError]
fn table.migrate[J: key + serializable, A: serializable, K: key + serializable, B: serializable, E: serializable](mut s: Store, from: Table[J, A], to: Table[K, B], f: Entry[fn(key: J, old: A) -> Result[(K, B), E]], batch: u64) -> Result[u64, MigrateError[E]]
fn store.for_test(t: Test) -> Store
// Reserved (STORE-4, STORE-12)
fn store.info(tx: Tx) -> CallInfo
fn call_info.caller(c: CallInfo) -> String
fn call_info.at(c: CallInfo) -> i64
fn call_info.seed(c: CallInfo) -> u64
fn call_info.for_test(t: Test, caller: String, at: i64, seed: u64) -> CallInfo
fn runner.transact[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(tx: Tx, call: CallInfo, a: A) -> Result[R, E]], a: A) -> Result[R, RunError[TxFailure[E]]]
```

`store.for_test` returns a `Store` for a fresh empty store over a simulated directory without
faults, owned by the test and discarded when it ends. Crash tests use the simulated
directory's crash ([STORE-11](#store-11-engine-and-storage-seam)) inside one program.
