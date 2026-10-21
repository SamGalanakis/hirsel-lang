# Captured candidate namespace (internal)

`hirsel_engine::loader_inputs::namespace` supplies a closed immutable input
adapter. It is an internal prerequisite for future candidate input editing;
there is no version2 protocol dispatch. The existing v1 observed-operation
`FrozenInputs` adapter and its unsupported edits remain unchanged.

A host explicitly admits disjoint UTF-8, case-sensitive roots, exclusions,
entry, toolchain std path, environment identity, owned image containers and
capture limits. `CaptureSource` is the capture-only no-follow/readlink seam.
The Linux `DiskCapture` backend traverses descriptors without following
ancestor links; unsupported identity policies refuse. It never creates
folders or expands roots. Two complete bounded scans must agree before
publication. This is an interval recheck, not proof of simultaneous filesystem
state against an uncooperative writer. Unreadable included roots, unsupported
names, changed inputs and resource limits publish no partial namespace.

`NamespaceLease::capture_program` additionally validates the selected package
graph through the ordinary loader before returning a program base. Source
syntax remains repairable. `capture` alone is a lower-level topology primitive.
A lease contains no disk adapter. It implements all four `LoaderInputs`
operations from one node graph: regular bytes, complete sorted directories
(including empty directories), symlink text, opaque nodes and explicit
ancestor traversal edges. Ancestor edges are not sibling directory coverage.
Fixed failed probes outside roots answer only their exact captured operation.
Unknown/opaque paths return `InputUnavailable`; known missing leaves/parents
and non-directory traversal retain their ordinary I/O categories. Resolution
follows symlinks before subsequent `..`. The explicit std hook prevents
`current_exe` discovery from frozen loads.

`inspect(root, relative)` uses strict edit paths and returns canonical keys
and namespace-bound present/byte or absence tokens. Missing parents, dangling
symlinks and unavailable coverage do not receive creation tokens. Tokens use
existing `ContentHash::of_parts` framing. `overlay` validates one final-state
set of regular-file create/replace/delete operations against the same base,
then publishes private tables. Canonical aliases identify one target;
duplicates refuse. Delete requires canonical target spelling. Symlink and
directory topology edits are unsupported; multiply linked regular files are
read-only. Empty directories survive last-file deletion. Old leases retain
old bytes/tables/aliases. `NamespaceDelta` is a conservative invalidation hint,
not proof of unchanged compiler products.

The caller owns source/manifest role admission, existing UTF-8 byte-patch
validation, request limits/identity, recomputed selection/scope, whole-closure
checking, receipts and adoption. The namespace's `load` helper forces an owned
engine, explicit config, Verify::On, Comptime, NoneStore and preheld immutable
images with image rebuild disabled. Explicit engine construction avoids even
transient process-default policy lookup. No disk observation, host execution,
foreign linking, source writing, result store discovery or protocol negotiation
is part of the namespace interface.

`Usage` reports roots/nodes/directory tables and entries/aliases/file bytes/
image bytes/fixed probes. `LeaseBudget` charges each distinct live namespace's
full tables/bytes, conservatively charging shared source bytes and both image
containers again per namespace; lease clones are free. Publication reserves
before image decoding, and descendant overlays share decoded immutable
handles. There is no parent chain and no eviction of live leases. Decoded
image indexes/type rows/lazy products are explicitly external allocations
reported as `external_image_handles`. This is a namespace table/byte allowance,
not a total heap bound. The host must separately account compiler/results and
independently retained source/image allocations and keep leases with receipts.

Prior art: TypeScript-Go `89d5d5b2849a0db0957065889ca58536fa6d2e4a`,
`internal/project/snapshotfs.go:20-30,56-117,160-180,199` and
`snapshotfs_test.go:24-257,1113-1144,1377-1432`. The port uses immutable overlay
and alias isolation; deliberate deviations are complete directory tables, no
disk fallback, no implicit mkdir or empty-directory pruning, and no mutable
watcher/alias topology. Descriptor traversal follows Rust std
`a7c37f9f0ee5e93396106e9de20f4a86b4154e63`,
`library/std/src/sys/fs/unix/dir.rs:112-133` and `unix.rs:1936-2010,2501`,
with no-follow collection separated from captured symlink interpretation.
