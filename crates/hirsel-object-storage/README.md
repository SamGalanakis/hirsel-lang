# hirsel-object-storage

The object storage seam shared by host providers that keep immutable objects:
the code store and the object-store-backed storage providers. `ObjectStorage`
has two operations, `get(key)` and `put_new(key, bytes)`, a conditional PUT
(`If-None-Match: *`): it creates the object whole when the key is absent,
durably before it returns `Created`, and otherwise returns `Exists` and leaves
the object unchanged. A failed put may or may not have created the object. Keys
are `/`-separated segments of lowercase ASCII letters, digits, `.`, `_` and
`-`, none empty or starting with `.` (`check_key`), so a key never escapes a
directory or collides with a temporary file.

Implementations:

- `DirObjects` (`objects.dir@1`): files under a directory. A put writes a
  temporary file, syncs it, hard-links it to the key (refused when the key
  exists, so creation is atomic and never replaces), removes the temporary
  name and syncs the directory.
- `MemoryObjects` (`objects.sim@1`): in memory with a bucket's
  conditional-PUT semantics. `with_faults(seed, one_in)` fails each operation
  with probability `1/one_in` from a SplitMix64 stream; a faulted put fails
  either before its write or after it (a lost reply). `overwrite` replaces an
  object as a hostile medium would, for tests.

`open(entry)` opens a run-configuration entry: `{"provider": "objects.dir@1",
"path": "<absolute directory>"}` or `{"provider": "objects.sim@1", "seed": n,
"fail_one_in": n}` (both default 0, never failing). Unknown members and
providers are refused. An S3-compatible client (`objects.s3@1`) implements the
same trait in its own lane.

Dependencies: std and serde_json only.
