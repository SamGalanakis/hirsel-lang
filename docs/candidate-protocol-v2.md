# Candidate protocol 2: serial closed inputs

Candidate connections select version 1 or 2 with their first supported decoded
frame and keep that version until disconnect. Use a separate connection for each
version. A later version change returns UnsupportedVersion without dispatch.
Version 1 retains its existing envelopes, source replacements and refusal cases.
Manifests in both protocols use schema 1.

V2 captures a finite namespace. It has complete directory tables and owns source,
manifest and image bytes. Inspection, submission, checking, result rendering and
promotion use only those captured inputs. Promotion changes the resident head;
it writes no files. Each candidate reloads and checks the entire selected graph
with Verify On, Comptime, an owned engine and NoneStore. File-set and manifest
changes never use the experimental fork backend.

## Capture and inspect

The server workspace is an absolute physical directory. Capture names explicit
roots; relative paths are relative to that workspace. Editable roots must be
inside it. Read-only roots can explicitly admit external dependencies. Roots
must be disjoint, case-sensitive UTF-8 paths without symlink ancestors; excluded
subtrees are unavailable, not absent. No automatic root expansion or package
fetch occurs. Supply the toolchain std manifest and its root explicitly if a
standalone source may select std, including after sidecar deletion.

```json
{"version":2,"op":"capture","entry":"hirsel.json","roots":[{"id":"app","path":"","editable":true}],"std_manifest":null,"images":[],"excluded":[]}
{"version":2,"op":"inspect","base":1,"paths":[{"root":"app","path":"main.hir"},{"root":"app","path":"mods/new.hir"},{"root":"app","path":"hirsel.json"}]}
```

Capture returns a base handle, `{workspace,generation,inputs}` token, namespace
digest, selected packages and usage. Inspection returns that same token and
ordered node facts. Present regular files include canonical `path`, `editable`
and `expected:{present,bytes}`. A missing leaf of a captured closed parent has
`expected:{absent}` and a canonical parent. MissingParent and DanglingSymlink
carry no absence proof. Opaque/uncaptured paths return InputUnavailable. Tokens
bind the full base namespace; copy them exactly. Digests are 64 lowercase hex
digits. Inspection is bounded to the existing per-request file allowance.

## Simultaneous edits

Use the exact base token and inspection expectations. These illustrative
uppercase strings stand for values from the preceding replies:

```json
{"version":2,"op":"submit","id":12,"base":1,"token":{"workspace":1,"generation":1,"inputs":"INPUTS_HEX"},"priority":"selected","edits":[{"kind":"create_source","path":{"root":"app","path":"mods/new.hir"},"expected":{"absent":"ABSENCE_HEX"},"text":"pub fn value()->u64{2}\n"},{"kind":"replace_source","path":{"root":"app","path":"main.hir"},"expected":{"present":"PRESENT_HEX","bytes":"BYTES_HEX"},"text":"pub fn main()->u64{new.value()}\n"},{"kind":"manifest","action":"replace","path":{"root":"app","path":"hirsel.json"},"expected":{"present":"MANIFEST_PRESENT_HEX","bytes":"MANIFEST_BYTES_HEX"},"text":"{\"schema\":1,\"package\":{\"name\":\"app\",\"version\":\"1\"},\"sources\":[{\"path\":\"main.hir\",\"module\":[]},{\"path\":\"mods/new.hir\",\"module\":[\"new\"]}],\"dependencies\":[],\"entry\":{\"module\":[],\"name\":\"main\"}}"}]}
```

Supported tags:

| Tag | Fields beyond kind/path/expected |
| --- | --- |
| create_source | absent expectation, text |
| replace_source | present and bytes expectations, text |
| delete_source | present and bytes expectations |
| edit_source | present and bytes expectations, byte_edits |
| manifest | action create/replace/delete; analogous expectations and whole text for create/replace |

`byte_edits` are `{start,end,replacement}` using u32 offsets into the same old
UTF-8 bytes. No newline or Unicode normalization occurs. Overlap, ambiguous
same-position inserts and non-boundary offsets refuse. Empty text is a file,
never absence. Edits are a simultaneous final-state set: two aliases or
delete+create at the same target return DuplicateTarget. Use replace instead.
Creation needs an existing editable parent; it never makes directories.

Source role means a selected source or eligible captured `.hir` file; manifests
are selected manifests, exact source sidecars or eligible `.json` files. This
editing boundary does not change language filename rules. Request fields are
strict: extra/missing/duplicate fields, bad tags/actions/priorities, wrong JSON
types, invalid UTF-8/surrogates and malformed digests refuse. Omitted priority
means background. Priority affects queue selection, never request identity or
compiler ordering. Request IDs are idempotent for a canonical request; changed
arguments conflict, and an expired ID never reruns.

## Results and adoption

`result`, `cancel`, `release`, `head`, `promote` and `counters` retain the v1
operation shapes under version 2. Result adds `scope_complete`, `entry_path`
and `selection` with the actual selected source contexts, entry, unsafe-FFI
metadata and image identity. A loader failure has no complete scope or checked
products. LoadManifest and LoadIo are Incomplete(ToolFailure)/Undecided;
uncaptured inputs are Incomplete(InputUnavailable)/Unavailable. Source diagnostics
are Complete/Reject. Only Complete/Pass is eligible for promotion.

Checked-read deltas keep the existing hirsel.checked-read-delta.v1 schema and
exact canonical owner/key/value semantics when both complete endpoints are
available. Missing compatible evidence stays Unknown; filenames imply no test
impact. Absolute products may coalesce, but each waiter retains its own parent,
cancellation and delta. Live old receipts retain deleted-source maps and bytes.
Namespace tables/source bytes, result maps and request records remain charged;
decoded image metadata and in-flight compiler allocations have the separately
reported/existing host limits. Live leases are never evicted to admit a request.

Promotion requires the candidate's exact parent token to equal the expected
current head. It adopts the namespace and recomputed selection together and
increments generation. Same-content ABA still conflicts by generation. An
operation-ID retry returns its original outcome; it never repeats adoption.

Explicit disk observation is independent:

```json
{"version":2,"op":"observe_disk","target":"base","handle":1}
{"version":2,"op":"observe_disk","target":"candidate","handle":2}
```

It returns inputs, observation_id, head_generation_at_observation, status
(MatchesObserved, Changed or Incomplete), differences, and an error for incomplete
observations. Created/deleted candidate nodes naturally differ from disk.
Observation changes no receipt identity or promotion eligibility.

## Refusal remedies

| Refusal | Remedy |
| --- | --- |
| UnknownBase/UnknownLease | Use a live handle from this version's connection. |
| PreconditionFailed/StaleBytes | Inspect the intended base again; use its exact token and expectations. |
| DuplicateTarget | Submit one final mutation per canonical target. |
| UnsupportedEdit: MissingParent | Create directories externally, then explicitly recapture. |
| InputUnavailable | Recapture with explicit adequate roots/std context; no lazy fallback is permitted. |
| Unsupported role, read-only/hardlink/symlink topology | Use an eligible regular target or edit externally and recapture. |
| LoadManifest/LoadIo | Repair the complete manifest graph and selected source set in a new sibling request. |
| ResourceLimit | Release unneeded handles or start a smaller explicit capture. |
| Conflict | Obtain the current head and submit a new candidate against it. |

Image-backed source edits require captured editable source closure. A changed
image key uses another exact preheld image or frozen source; images are never
reopened/rebuilt while checking. New roots, symlink topology, entry path,
environment and image-set replacement require a new explicit capture.
