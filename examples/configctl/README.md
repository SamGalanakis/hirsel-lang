# configctl

Validate versioned JSON, migrate v1/v2 to canonical v2, and compare semantic
values through real `validate`, `migrate` and `diff` subcommands. See the
[executable agent guide](GUIDE.md) for signatures, grants, statuses and replay.
Inputs are read-only; successful validation and equal diffs are silent.

TOML service migrations are separate commands:

```sh
hirsel run examples/configctl/hirsel.json --grants /tmp/configctl-grants.json -- toml-plan --to 3 --jobs 4 service-v1.toml service-v2.toml
hirsel build examples/configctl/hirsel.json --backend llvm -o /tmp/configctl-llvm
/tmp/configctl-llvm --hirsel-record /tmp/configctl.tape --hirsel-grants /tmp/configctl-grants.json toml-migrate --to 3 --jobs 4 service-v1.toml service-v2.toml
/tmp/configctl-llvm --hirsel-replay /tmp/configctl.tape
hirsel run examples/configctl/hirsel.json --replay /tmp/configctl.tape
```

Copy `toml-grants.template.json` to `/tmp/configctl-grants.json`, replace both
host directories with distinct existing absolute directories, and put the example
`fixtures/service-v*.toml` files in the input directory. Keep recordings outside
both directories. The destination grant must resolve to a separate directory;
virtual mount names cannot prove that two host directories do not alias.

`toml-plan --to 2|3 [--jobs 1..4] FILE...` reports ordered JSON lines containing
source, status, semantic changes and a unified text diff. `toml-migrate` writes
`0.toml`, `1.toml`, etc. atomically under `migrated`, then atomically replaces
`manifest.json` with schema/target/success and actual per-file statuses. Existing
destinations are replaced. This is per-file publication; a failed peer does not
roll back successful files. A failed manifest publication returns an error.
Already-current source bytes are preserved, and migrating published documents to
the same version again leaves their TOML bytes identical. The manifest describes
each run's changes and can differ between first and repeat migration.

The service schema is explicit:

| Version | Required service fields |
| --- | --- |
| 1 | `version=1`, `endpoint`, `timeout_ms`, `retries`, `log_level` |
| 2 | `version=2`, `service.endpoint`, `timeouts.connect_ms`, `timeouts.request_ms`, `retries`, `log_level` |
| 3 | `version=3`, same endpoint/timeouts, `retry.attempts`, `retry.policy`, `log_level` |

v1→v2 uses the old timeout as request timeout and caps connect timeout at 1000 ms.
v2→v3 uses policy `none` for zero attempts, otherwise `fixed`. Direct v1→v3 runs
both transitions. Timeouts must be positive, connect ≤ request ≤ 3600000 ms;
attempts are 0..10; levels are debug/info/warn/error. Downgrades, old/new key
conflicts, duplicate keys and invalid types are rejected. Unknown application keys
remain intact. No includes, discovery or environment expansion takes place.

Inputs are limited to 16 distinct canonical relative filenames of at most 240
UTF-8 bytes, 65536 bytes per file and 524288 bytes total. Jobs defaults to 1 and
ranges through 4. File sizes are admitted before reads, and changed sizes refuse
that file. Each plan has at most 64 edits; std's TOML nesting bound stays 128.
Resource refusals are typed application errors, separate from language failures.
The entry returns exit 0 on success/help and 1 on invalid input, failed peers,
grants or publication errors; launcher/tooling statuses remain the host contract.
The legacy JSON commands retain their original behavior and exact help output.

Unchanged text, quoted/dotted keys and arrays of tables are preserved by
`std.toml.Document`. Updating an existing scalar retains its key spelling and
inline comment. Removing a key also removes comments owned by that entry; new
keys use std's formatting. See [FRICTION.md](FRICTION.md) for ownership costs and
explicit unsupported validation cells.

Prior art: toml-rs `ceac6e4412420d5a5c19e4f7293ad005cf1a31b2`,
`crates/toml_edit/src/document.rs:113–161` and `tests/testsuite/edit.rs:61–90`
(MIT/Apache-2.0); Similar `96993d92851eff9a484ea157614de30f80dfdc73`,
`src/udiff.rs:280–330,339–364` and its newline-hint snapshot (Apache-2.0).
The orchestration reuses std's implementations. Deviations: typed explicit
transitions replace indexing an untyped document; std controls trivia ownership;
std Myers supplies diff edits rather than importing Similar's Rust iterator.

The new application modules are schema (typed service versions and validation),
migration (ordered generic transitions), document (owned edits and independent
reparse), batch (size admission and task-owned plans), and publish (serial atomic
outputs and manifest). Existing JSON policy stays in model. The focused harness
is `config_migrate_29`; legacy identities stay in `configctl_8`.

The useful application is qualified with explicit remaining limits: reference
and checked H each pass 32 of 39 workflow/boundary identities; seven exceed the
unchanged 1M-step budget. Native adapters pass 23 supported workflows and 12
bounds each, with 162 offline replays. Native cancellation recording, checked-H
host-tape replay and effective physical width remain INCOMPLETE. Exact identities
and evidence limitations are recorded in FRICTION.md; this is not an all-engine
completion claim.
