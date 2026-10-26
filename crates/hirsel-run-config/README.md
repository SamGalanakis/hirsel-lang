# hirsel-run-config

Record parsing, normalization, validation and admission for PROG-3/6 and DET-1.
Shared grants, calls, outcomes, script data, semantic program revisions, hashing
and TLS argument checks live in `hirsel-run-schema`; consumers import it directly.
No compiler or execution implementation dependency.

Host input read failures include the selected grants, run record or revocation path. A relative directory resolution failure includes its template-relative candidate.

`RunConfig::validate(bytes)` is the sole parser, normalizer and schema validator.
It returns an opaque `ValidatedRun`; serialization never supplies a validated tag.
Canonical JSON has fixed member order, explicit nulls, sorted grant sets, exact
ordered Args, lowercase control escapes and length-framed SHA-256 identity.
Absent Io/Args/Env differs from an explicitly empty grant/snapshot.

`ProgramFacts.code` is the `CodeReach` the launcher found in checked code (CAP-6).
`ValidatedRun::bind` checks it last: under `admission.code` `admit.safe@1` an `Escapes`
reach refuses as `unsafe_code` with its path, before any host call, for live launches
and replays alike; `admit.any@1` and an absent policy admit any code. Validation
normalizes `admit.any@1` to absence, so it never reaches canonical bytes; an unknown
policy refuses as Unsupported.

`ValidatedRun::admit` invokes explicit host authorization and evaluates time and
revocation evidence once. It returns an opaque `AdmittedRun` retaining the accepted
record and evidence. `check_replay` validates the recorded binding without reading
a clock, revocation source or host provider. Replayed admission issues no live
capabilities; resuming host execution must call admit again.

`Config` is an opaque C-compatible seed/identity projection produced only by a
ValidatedRun. It is data, not proof of host authorization. Host launch adapters
must acquire and pin resources and check recorder/grant physical overlap before
entry. Tool destinations and native artifact provenance stay outside semantic
run identity. Provider strings describe versioned contracts; an adapter must
refuse every unsupported provider/restriction, never widen it.

The schema-1 empty constructor denies every external root. Entry is an exact
versioned definition path. Directory access is `read` or `read_write`; mount keys
are canonical portable relative paths (the empty mount denotes the virtual root),
and host roots are canonical absolute designators. Env is a sorted object mapping
granted keys to UTF-8 strings or null (granted but unset). Network rules are
sorted unique address/service/operation triples, with no wildcard rule. Addresses
are lowercase ASCII host names without a root dot, canonical dotted IPv4 or RFC
5952 IPv6 without brackets or a zone. Hosts ending in an ASCII numeric or hexadecimal IPv4-number label must be
canonical dotted-quad IPv4; short forms and leading zeros refuse. Services are decimal ports 0–65535
without leading zeros, and operations are `connect`, `connect_tls`, `listen`, `udp_bind`,
`udp_peer` or `listen_tls`.
The Unix operations `unix_connect`, `unix_listen` and `unix_listen_remove_stale` name a
canonical absolute socket path of at most 107 bytes with service `0`; scripted peers may
name such a path (port 0, no `resolves_to`).

`snapshot_args` and `snapshot_env` convert only explicitly supplied OS strings;
byte-invalid inputs refuse with the argument index, key index or selected Env key.
They never read ambient argv/env. Empty snapshots remain explicit and granted
unset keys remain null. Package versions are canonical decimal u64 majors.

Checks: `//:hirsel-run-config-unittest` and `//:hirsel-run-config-test-it`.

`ProgramRevision::encode(entry, nodes)` is the sole semantic program encoder.
It follows the entry's transitive graph, sorts identities and dependency sets,
and hashes length-framed content fields and edges in `hirsel.semantic-program.v1`.
Cycles are graph edges; missing or conflicting components refuse. Unreachable
components, duplicate reads and input enumeration order do not affect revision.
Adapters supply private bodies, constant initializers, core and runtime code;
artifact/compiler/position/instance details never belong in those fields.
`RunConfig::for_program` selects this revision with explicit denial defaults;
`ValidatedRun::check_revision` checks its digest, entry and launch kind.

Known provider contracts have one closed enumeration, `ProviderContract`:
`tasks.cooperative@1`, `stdin.bytes@1`, `stderr.bytes@1`, `stdout.bytes@1`, `random.splitmix64@1`, `random.system@1`, `clock.monotonic.ns@1` and
`clock.wall.unix_ns@1`, `net.stream@1`, `net.script@2`, `process.spawn@1`,
`process.script@1`, `signals.host@1`, `signals.script@1`, the code admission policies `admit.any@1` and `admit.safe@1`, and `budgets.none@1` and `budgets.limits@2`. The validator checks the exact projection and version.
These names are identical for real hosts and simulation/replay; execution origin
belongs to tooling provenance. Unknown contracts and versions refuse as Unsupported.

`io.cpu` is optional finite `cpu.scoped@1` authority subordinate to `io.tasks`.
It requires explicit `max_workers` in 1..=16, `max_domains` equal to 1, and
positive `max_physical_bytes` and `max_batch_bytes`. There are no default limits.
Absent or null grants are omitted from canonical bytes. Present grants and every
limit contribute to semantic identity and the grant subset relation. The schema
does not itself start workers or certify physical width. Runner grant narrowing
does not convey this adjunct implicitly.
`inputs.scheduler` names `fifo`, `random` or `pct`, version 1 (docs/spec/determinism.md,
"Scheduler"); any other refuses as Unsupported. `Config::scheduler` projects its launch
word (`Scheduler::CONTRACTS`, shared with `hirsel_kernels::schedule::Algorithm::code`) and
seed, and `Scheduler::parse` reads the `name@1:seed` spelling of `hirsel test --scheduler`.
Revocation references remain external host evidence selectors, not provider names.

`net.stream@1` takes `rules`; `net.script@2` additionally requires `peers`
(possibly empty). `peers` is forbidden under every other provider, including an
explicit null. A peer has fields `address`, `port` (u16), `resolves_to` (canonical
IP strings in resolver order), `tls` and `connections`. Variants use external
snake_case tags: `"plain"`, `{"certificate":["example.com"]}`,
`{"fails":f}`, for a listener's client only `{"presents":[["example.com"],[45,45]]}`
(names, then the PEM chain it presents), or for a peer only
`{"serves":{"identities":[{"chain":[45],"key":[45]}],"clients":{"require":[45]}}}` (a
`TlsServer` that must be well formed, `tls::well_formed`: PEM chains and keys and a
parsable trust set; the scripted handshake loads it as `tls.accept` loads one, and an
unloadable one fails that handshake; `clients` is `"anonymous"`,
`{"request":..}` or `{"require":..}`, and canonical bytes omit it when anonymous) for TLS,
where `f` is a TlsFailure tag such as `"unknown_issuer"`
or `{"alert_received":40}`; `"refused"`, `"unanswered"` or `{"open":[steps]}`
for connections; and `{"send":[0,255]}`, `{"expect":[71,69,84]}`, `"stall"`,
`"close"`, `"reset"` or `"truncate"` for steps. `net.script@2` replaced
`net.script@1`, which lacked `fails` and `truncate` and now refuses as an
unknown version. Bytes are arrays of u8. Peer, address, certificate-name,
connection and step order is preserved in run identity. Peer data supplies no
authority beyond the explicit rules, including when those rules are empty.

`process.spawn@1` takes `rules`, sorted and unique by `program`: each names a
canonical absolute path (`canonical_program`: no empty, `.` or `..` component, no
trailing `/`, backslash or NUL) and an optional sorted, unique `env` list of
pass-through variable names. `process.script@1` additionally requires `runs`
(possibly empty), forbidden under every other provider: ordered `{program, args,
outcome}` entries, where an outcome is `{"output":{"status":{"exited":0},"stdout":[..],
"stderr":[..]}}` (or `{"signaled":n}`) or `{"error":e}` with a snake_case
ProcessError tag such as `"not_found"`, `"timed_out"` or `{"fault":7}`, never
`Canceled`. It may also take `children`, ordered `{program, args, steps}` streaming
child entries whose steps are `{"print":[..]}`, `{"print_err":[..]}`, `{"expect":[..]}` or
`{"exit":status}`. Runs and children grant nothing beyond the rules.

`io.signals` (`SignalsGrant`, `src/signals.rs`) takes `provider` and `kinds`, the
observed `SignalKind`s `interrupt`, `terminate` and `hangup`, nonempty and sorted by name
and unique after admission; `signals.script@1` additionally requires `script`, the arrivals
in delivery order (possibly empty), forbidden under every other provider. The member is
omitted from canonical bytes when absent, so records without it keep their identities, and
an explicit null is absent. `grants_within` holds a signals grant within another's under the
same provider with a subset of its kinds. `RunOutcome::Signaled(kind)` is a scripted
signal's default action: `exit_code` is `128 + kind.number()`, the status a launcher that
cannot raise the signal itself exits with. `validate_signals_grant` admits a standalone
grant as the field is admitted, for a harness launcher (`HIRSEL_RT_SIGNALS`).

`validate_env` validates a standalone Env snapshot object exactly as a record's `env`.

`grants_within(a, b)` is the CAP-3 grant subset relation over two validated records:
Unsafe, Io projections with exact providers, mounts (virtual path and host root extended
by the same components, access no wider), Net and process rules under the same provider
(`net_rule_within`, `process_rule_within`) and Env names with equal snapshots. Absent Io or
Env is within anything; scripted provider data and non-grant fields take no part.

`RunConfig::read_path` accepts only an explicit absolute record path.
`ValidatedRun::io_projections`, `clock_grants`, `net_grant` and `process_grant` derive adapter policy
from the validated grants, with no projections for absent or empty Io. An explicit
Net with empty rules grants the `net` projection while permitting no operations;
`net_grant` returns its exact validated provider, rules and peers.

`encode_net_peers` / `decode_net_peers` transport the ordered peer-data schema for
scripted network tape requests. They neither admit a RunConfig nor grant network
authority; the serving adapter validates endpoints and duplicates before minting
a world. JSON unknown fields and invalid variant/byte representations refuse.

`bind_grants` / `read_grants` fill a fresh schema-1 binding from the launcher's
program, entry and launch kind, then validate the resulting record. Templates
accept only inputs, Io, Args, Env, Unsafe, admission, budgets, recording and
object_stores fields; binding fields,
unknowns and duplicates refuse. Omitted fields use the same denial defaults as
`empty`. Relative template paths are explicit tooling inputs.

`Revocations::read_path` reads an explicit schema-1 host revocation list once. Its
SHA-256 revision covers the exact file bytes; `evidence(reference)` records that
revision and whether the named grant was listed. It supplies no authority itself.

`read_grants` resolves relative `io.dirs[].host` paths against the selected
template's directory and canonicalizes them before record validation. Absolute
hosts retain strict canonical designator validation. `bind_grants` accepts bytes
without a location and therefore requires absolute hosts, as do bound records.

Scripted port-0 listeners bind their client queue to the first ephemeral listen on
that IP. Each world allocates from 49152 upward, skips script-reserved and bound
ports, and shares the sequence through narrowed Nets; independent worlds restart
it. Outbound connects do not consume ports. Listen completions retain the assigned
address for replay.

`record_input` and `grants_input` select inline JSON when the explicit OS argument
starts with `{`; otherwise they use `read_path` and `read_grants`. Inline templates
have no file directory and require absolute directory hosts. Both forms use the
same schema validator and denial defaults; malformed inline input never falls
back to reading a file.
`grants_template` and `record_template` return the record either selects before
validation, so a launcher that completes it (a standalone executable's argument snapshot)
validates and hashes it once. `HostAuthority::authorize` receives the `ValidatedRun` that
admission checked.

`budgets` is the record's last member: absent for `budgets.none@1` (null and an explicit
`{"provider":"budgets.none@1"}` normalize to absence, and the canonical bytes omit it, so
records without budgets keep their pre-budget identity), or `budgets.limits@2` with `fuel` and
`memory_bytes`, each a positive u64 or null, at least one present. `ValidatedRun::budgets`
returns it; budgets are host policy, part of run identity, and grant nothing. Templates may
choose them. An adapter that cannot enforce one refuses the launch (`BudgetKind` names the
budget a stopped run exceeded).

`io.runners` (docs/spec/runners.md "Providers") lists the run's runners, sorted and
unique by lowercase `name`; the canonical bytes omit the member when the list is empty,
so records without runners keep their identity. `local.threads@1` takes `threads`
(default 1, nonzero) and `queue` (default 64); `sim@1` takes a `node`, the contract it
`models` (`local.threads@1`, `local.process@1` or `remote.tls@1`) and `faults`, rates in
parts per million at the sites that contract can produce (`sim_model_sites`). The seam
members `recording` (`none` or `record`; `durable` is unsupported), `retry` (`attempts`),
`cache` (only null) and `code` (a `hirsel-code-store` entry: `code.none@1`, `code.dir@1`,
`code.object@1` or `code.sim@1`) are run policy: each normalizes to absence at its
default. `grants` is the callee's grant set, the `io` schema without `stdin` or
`runners`, validated like `io`; `grants` and `definitions` (sorted, unique full
versioned paths) are authority, spelled null when absent. `inputs.fault_seed` seeds
`sim@1` fault decisions and is omitted when absent. `validate_runners` validates one
list on its own, as a corpus fixture names it. A runner entry's `admission` (`{"code":
"admit.safe@1"}`; `admit.any@1` normalizes to absence) is the place's code policy at arrival
(`admits_only_safe`).

A callee run's record (RUN-10) has `launch: call` and a `call` member, required exactly
then: a `CallRecord` with the form (`compute` or `call`), the cause (the caller's 128-bit run
instance, the request ordinal and the request digest, in lowercase hex) and the exact
argument bytes. `calls.rs` holds the RUN-11 derivations both engines and the tape check
share: `request_digest`, `call_key`, `callee_seed` and `callee_instance`, with the
`RunnerRequest` and `RunnerCompletion` a caller records and the `CalleeOutcome` a callee
records as it ends (`CallOutcome::carries` compares them). `ValidatedRun::instance` is the
derived instance of a call launch; admission (`AdmittedRun::instance`) fixes it, or draws
one from `HostAuthority::run_instance` for a run that names runners.

`RunOutcome` is the host result of either engine. It retains the returned Result
tag, the complete FAIL-1 chain, Blocked waits, budget/environmental stops,
refusal/replay causes and interpreter tooling diagnostics without depending on
compiler types. `RunOutcome::exit_code()` is the only standalone exit policy;
see the [PROG-3 table](../../docs/spec/programs.md#prog-3-entry). `Site::report`
formats complete human/JSON failures, and `Wait::encode`/`decode` carry native
parked-wait reports. A multi-run host keeps outcomes and never exits for a run.

`object_stores` maps lowercase names to S3 endpoint/bucket/region/prefix/cache_bytes
and a credentials reference `{mount, path}` inside an explicitly granted Dir. The
validator expands it to per-store `HIRSEL_S3_<NAME>_*` Env snapshots, the exact Net
endpoint rule and a signing WallClock. Secrets remain in a granted credential file.
Conflicting Env values, duplicate names, invalid origins or escaping references
refuse. An absent or empty map keeps existing canonical bytes unchanged. The client
and store backend are the pure Hirsel `packages/s3@1`, not host Rust code.

`managed_cores` resolves placement against launcher grants, captured admission
CPU affinity, effective cpusets and every visible ancestor quota. It also supplies
the one-second refresh seam and JSONL metrics writer for admitted destinations.
See [managed core resolution and metrics](../../docs/managed-cores.md) for the
live-executor integration contract and checks. These host diagnostics are never
canonical inputs or replay dependencies.
