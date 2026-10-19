# hirsel-code-store

Content-addressed code for runners and hosts (the RUN-13 code seam): code
objects keyed by LANG-6 definition hash, a store whose one write is
all-or-nothing `add(bundle) -> Ok | Missing(hashes)`, fetch by hash, an
optional signature policy, and a place's code check on arrival.

**Code objects** (`object.rs`). A definition's object is its hash preimage:
its recursive group's content texts (`hirsel-proof-export` content mode, every
outside reference written as its hash), each member's group references and the
member it is, as `hirsel.code.object.v1` codec text. Decoding requires
canonical form, counts each member's group markers against its references,
recomputes the group's hashes with `group_hashes` and refuses an object whose
member hash differs from the claimed key (`Tampered`). Dependencies are the
`(Ref "<hash>")` nodes parsed from the texts, so nothing in an object is
trusted for what it claims. An object holds its whole group, so `group()`
yields every member's object; a store holds groups whole.

**Store** (`store.rs`). `CodeStore` keeps `code/<hash>` and, per signature,
`sig/<hash>/<public key>` in an `ObjectStorage`, all written create-if-absent.
`add` verifies every bundle object, checks the signature policy, then
computes the dependencies neither offered nor stored: when any is missing it
writes nothing and returns exactly that set. Otherwise it writes signatures
first, then code objects dependencies first, so an interrupted add never
leaves an object without its closure. An object already stored is success, so
re-adding is idempotent. `get` and `closure` re-hash every object they read
(`Corrupt` when a medium changed it) and, under a signing policy, check a
trusted signature again (`Unsigned`).

**Signatures** (`sign.rs`). `sign.none@1` (default) checks nothing beyond the
hash. `sign.ed25519@1` with a sorted, unique, non-empty key list requires each
object to carry an Ed25519 signature by a listed key over
`hirsel.code.signature.v1\0 || hash`, from the bundle or already stored; the
hash binds the whole group. `Signer` signs (from a seed or PKCS#8), via
`ring`, which the workspace already uses through rustls.

**Arrival** (`arrival.rs`). `resolve(identity, local, store)` is RUN-8 step 3:
`Local` when the place's revision defines the path with the request's hash,
`Fetched(objects)` when the store holds the hash's whole closure, verified, and
otherwise `VersionMismatch { missing, cause }`, the hashes the place lacks and
any store failure. Loading fetched objects into a runnable program belongs to
the runner providers.

**Configuration** (`config.rs`). The `code` entry of a runner or host:

```json
{"provider": "code.none@1"}
{"provider": "code.dir@1", "path": "/srv/hirsel/code",
 "signatures": {"provider": "sign.ed25519@1", "keys": ["<64 hex digits>"]}}
{"provider": "code.object@1", "objects": {"provider": "objects.dir@1", "path": "/srv/objects"}}
{"provider": "code.sim@1", "seed": 7, "fail_one_in": 8}
```

`code.none@1` (default) holds only the place's own revision and refuses adds.
`code.dir@1` stores under a directory, `code.object@1` over any object storage
entry (`hirsel_object_storage::open`), and `code.sim@1` in memory with seeded
faults. Each but `code.none@1` takes an optional `signatures` entry (default
`sign.none@1`). No choice changes what code means: a store changes only which
code a place can find and admit.

Dependencies: `hirsel-codec`, `hirsel-object-storage`, `hirsel-proof-export`,
`hirsel-syntax`, `ring`, `serde_json`. The tests over compiled programs live
in `hirsel-pipeline-tests` (`code_store_programs`).
