The framing structure is adapted from Git `object-file.c:102–111,459–474`,
commit `47fdf2208c5b94c8cc01c106b8f7c85efb91c926`; its whole-file SHA256 is
`53fc4fa2245c6885604da369ddc836c66252f570a7b51d64dfb0185b625a0035`.
Vector shapes follow `t/t1007-hash-object.sh:44–111` at the same revision
(SHA256 `9636dc2b8fa7cf0c9e38f195ef8cf67aa27711ad50e9358f5f844605115751ff`).
Git's GPL v2 terms are retained in COPYING. The framing adaptation and test
oracle are distributed under GPL-2.0-only. All hashes are independently
computed with Python hashlib, never copied SHA1 expectations.

Deviations: uncompressed SHA256-only objects; canonical UTF8 JSON trees rather
than Git binary trees; explicit expected-ID manifests instead of working-tree
or stdin discovery; directory capabilities and typed errors; immutable named
roots rather than mutable refs; Durable put_new and verified duplicate winners;
bounded task batches and root-based traversal. This is not Git interoperability.
Hirsel std.sha256/std.json own hashing, UTF8 and JSON token/escape algorithms.
The bounded task set and component inspection follow packtree/jobs.hir and
catalog.hir; no archive codec, directory scanner or runtime adapter is copied.
