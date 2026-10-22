# configctl agent guide

Public pure functions in `configctl@0/model`:

```hirsel
pub fn parse(data: Bytes) -> Result[unique Config, ConfigError] uses {}
pub fn validate(config: Config) -> Result[(), ConfigError] uses {}
pub fn migrate(take config: Config, target: u32) -> Result[unique V2, ConfigError] uses {}
pub fn migration(data: Bytes, target: u32) -> Result[unique Bytes, ConfigError] uses {}
pub fn diff(old: Bytes, new: Bytes) -> Result[unique Bytes, ConfigError] uses {}
```

`Config = Version1(V1) | Version2(V2)`. V1 is
`{version: u32, name: String, retries: u32}`. V2 is
`{version: u32, service: Service, retry: Retry}`, where Service has `name: String`
and Retry has `attempts: u32`. The version must match its DTO. Names contain at
least one UTF-8 byte; retry counts are inclusive 0..10. Whitespace names are
nonempty. Unknown/missing fields, duplicate decoded keys, wrong types and unknown
versions fail. Duplicate rejection happens in std JSON before strict DTO decoding.
Inputs are bounded at 65536 bytes, including an EOF probe at the exact cap.

Migration policy belongs to this app: only 1→2 and 2→2 succeed. Both preserve the
name and count. `schema_evolution` is inert tooling and is not an executor; no
schema discovery or patch API participates here. Migration writes compact UTF-8
JSON with declaration-order fields and one LF. v2 idempotence means canonical
semantic output, independent of input whitespace or member order.

Diff validates both documents and compares their v2 normalization. Its output is
zero or more JSON lines with fields `path`, `old`, `new`, ordered by UTF-8 path:
`retry.attempts`, then `service.name`. Paths are application field names, not JSON
Pointer. Values retain their JSON types. Equal normalized documents emit zero
bytes. Changes still exit 0. Validation succeeds silently.

From the repository root, with a matched `hirsel` and native tools on PATH:

```sh
hirsel test examples/configctl/hirsel.json
hirsel test examples/configctl/hirsel.json --native --backend llvm
hirsel test examples/configctl/hirsel.json --native --backend cranelift
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- --help
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- validate --help
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- migrate --help
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- diff --help
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- validate dogfood8-v1-zero.json
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- migrate dogfood8-v1-zero.json --to 2
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- diff dogfood8-v1-zero.json dogfood8-v2.json
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json -- validate -- -dogfood8-name.json
```

The read-only `configs` mount exposes only this fixture directory. Filenames are
relative to that mount. No network, environment, clock, stdin or writable mount
is requested. Missing command, unknown command, wrong arity, invalid input and
unsupported migration return nonzero (1); successful commands and help return 0.
Errors have empty stdout and `error: ...` on the launcher's stderr. `--help`/`-h`
is supported at root and before operands on each command. `migrate FILE --to 2`
and `migrate --to 2 FILE` both work. Place a leading-dash filename after `--`:
`migrate --to 2 -- -dogfood8-name.json`. Options are not scanned past that delimiter.
Repeated flags are usage errors. Input files are never modified.

Build each native executable once, then reuse it for every launch and replay:

```sh
hirsel build examples/configctl/hirsel.json --backend llvm -o /tmp/dogfood8-configctl-llvm
hirsel build examples/configctl/hirsel.json --backend cranelift -o /tmp/dogfood8-configctl-cranelift
/tmp/dogfood8-configctl-llvm --hirsel-grants examples/configctl/fixtures/dogfood8-grants.json migrate dogfood8-v1-zero.json --to 2
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json --record /tmp/dogfood8-migrate.tape -- migrate dogfood8-v1-zero.json --to 2
hirsel run examples/configctl/hirsel.json --grants examples/configctl/fixtures/dogfood8-grants.json --record /tmp/dogfood8-diff.tape -- diff dogfood8-v1-zero.json dogfood8-v2.json
hirsel run examples/configctl/hirsel.json --replay /tmp/dogfood8-migrate.tape
/tmp/dogfood8-configctl-llvm --hirsel-replay /tmp/dogfood8-migrate.tape
/tmp/dogfood8-configctl-cranelift --hirsel-replay /tmp/dogfood8-diff.tape
```

Keep source/toolchain frozen through all consumers. Tapes live outside granted
roots and contain input bytes. The focused Rust integration additionally records
both operations on every engine, replaces host inputs with different sentinels,
and cross-replays all six tapes on all three engines without live grants.
See [native verification](../../docs/native-verification.md) for toolchain setup.

Dispatch and help/missing-operand vectors adapt Clap `examples/git-derive.rs:8–61`
and `examples/git-derive.md:5–68`, commit
`ccc89b83875474399e9d9d70841163f6319d1a41` (MIT, Individual contributors).
Ordinary `std.cli.parse` calls replace derives. Explicit root dispatch replaces
a generated enum; per-command arity replaces derive attributes; stdout help
replaces Clap's missing-operand help on stderr. Unlike Clap's extensible git
example, unknown commands fail. A small app-local partition permits flags after
FILE while preserving std's stop-at-first-positional and `--` behavior. No global
CLI framework is introduced. Existing third-party notices retain the MIT text.

The single Rust target `root//:hirsel-cli-test-configctl_8` has three bounded
fresh-process groups: `configctl_8_cli_arguments` (69 observations),
`configctl_8_data_validation` (66) and `configctl_8_record_replay` (24, plus the
three package tests on all engines). Run each with its exact `--test-arg` filter
and `-c hirsel.tests=focus`. Each independent group builds once per backend and
reuses those paths throughout that group. All 159 assertions remain present;
`HIRSEL_DAEMON=off` preserves ordinary test isolation and the 600-second deadline.
The Python fixture verifier defaults to the full matrix; `--group` selects the
same bounded groups for standalone reproduction.
