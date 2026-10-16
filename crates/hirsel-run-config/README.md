# hirsel-run-config

Closed tooling data for PROG-3/6 and DET-1. Allowed dependencies: std, serde,
serde_json and sha2. No compiler or execution implementation dependency.

`RunConfig::validate(bytes)` is the sole parser, normalizer and schema validator.
It returns an opaque `ValidatedRun`; serialization never supplies a validated tag.
Canonical JSON has fixed member order, explicit nulls, sorted grant sets, exact
ordered Args, lowercase control escapes and length-framed SHA-256 identity.
Absent Io/Args/Env differs from an explicitly empty grant/snapshot.

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
without leading zeros, and operations are `connect`, `connect_tls` or `listen`.

`snapshot_args` and `snapshot_env` convert only explicitly supplied OS strings;
byte-invalid inputs refuse with the argument index, key index or selected Env key.
They never read ambient argv/env. Empty snapshots remain explicit and granted
unset keys remain null. Package versions are canonical decimal u64 majors.

Checks: `cargo test -p hirsel-run-config`.

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
`stdout.bytes@1`, `random.splitmix64@1`, `clock.monotonic.ns@1` and
`clock.wall.unix_ns@1`, `net.stream@1` and `net.script@2`. The validator checks the exact projection and version.
These names are identical for real hosts and simulation/replay; execution origin
belongs to tooling provenance. Unknown contracts and versions, Tasks and
scheduler policy refuse as Unsupported until adapters can enforce them.
Revocation references remain external host evidence selectors, not provider names.

`net.stream@1` takes `rules`; `net.script@2` additionally requires `peers`
(possibly empty). `peers` is forbidden under every other provider, including an
explicit null. A peer has fields `address`, `port` (u16), `resolves_to` (canonical
IP strings in resolver order), `tls` and `connections`. Variants use external
snake_case tags: `"plain"`, `{"certificate":["example.com"]}` or
`{"fails":f}` for TLS, where `f` is a TlsFailure tag such as `"unknown_issuer"`
or `{"alert_received":40}`; `"refused"`, `"unanswered"` or `{"open":[steps]}`
for connections; and `{"send":[0,255]}`, `{"expect":[71,69,84]}`, `"stall"`,
`"close"`, `"reset"` or `"truncate"` for steps. `net.script@2` replaced
`net.script@1`, which lacked `fails` and `truncate` and now refuses as an
unknown version. Bytes are arrays of u8. Peer, address, certificate-name,
connection and step order is preserved in run identity. Peer data supplies no
authority beyond the explicit rules, including when those rules are empty.

`RunConfig::read_path` accepts only an explicit absolute record path.
`ValidatedRun::io_projections`, `clock_grants` and `net_grant` derive adapter policy
from the validated grants, with no projections for absent or empty Io. An explicit
Net with empty rules grants the `net` projection while permitting no operations;
`net_grant` returns its exact validated provider, rules and peers.

`select_run_record` accepts zero tooling arguments or exactly one
`--hirsel-run <absolute path>`. It binds the result to semantic code, entry and
launch kind. Zero arguments select denial defaults; tooling argv never becomes
an Args snapshot.

`encode_net_peers` / `decode_net_peers` transport the ordered peer-data schema for
scripted network tape requests. They neither admit a RunConfig nor grant network
authority; the serving adapter validates endpoints and duplicates before minting
a world. JSON unknown fields and invalid variant/byte representations refuse.
