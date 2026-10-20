# hirsel-run-schema

Shared run data for compiler and execution crates: launch inputs, grants, calls,
provider contracts, refusal categories, outcomes, scripted network and process
data, semantic program revisions and hashing. Engine helpers (script playback,
endpoint checks and TLS argument checks) stay with their data. Dependencies are
std, serde, serde_json, sha2 and the pinned TLS libraries; no compiler or execution
implementation dependency.

Record parsing, normalization, validation and admission live in `hirsel-run-config`.
This crate never constructs a validated or admitted run. Script-data decoding
and OS-string snapshots confer no authority. Serialization and hash framing are
unchanged by the split.

Checks: `//:hirsel-run-schema-unittest` and `//:hirsel-run-schema-test-it`;
program revision and grant coverage also runs in `//:hirsel-run-config-test-it`.
