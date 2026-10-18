# Capabilities and authority

All authority arrives as typed parameters. A program's entry receives root capabilities
(`Io`, `Args`, `Env`, `Unsafe`); everything else is projected from them, narrowed, or
created by a scope-forming library function. Holding a capability is necessary to use the
thing it names, and a value's **authority summary** says which capabilities it holds.
Whether a capability is global or scoped is fixed by
[MODE-6](ownership.md#mode-6-scoped-capabilities).

### CAP-1: Authority

Directories, network, processes, clocks, randomness, tasks, standard output, environment
variables, allocation placement and `Unsafe` arrive only as typed parameters. `Io` is projected only
into the grants the run configuration selects ([PROG-3](programs.md#prog-3-entry)); each
projection requires an actual grant and cannot widen its resource restrictions. A path
designates something within a grant and does not itself grant host access. An operation
called without its explicit capability argument is CAP-1.

```hirsel
pub fn main(rt: Io, args: Args) -> Result[(), OutputError] {
  let out = match io.stdout(rt) { Ok(s) => s, Error(_) => return Ok(()) }
  out.print(out, "hello\n")
}
```

**Directories.** `Dir` is a directory capability: paths are relative, cannot escape it, and
it can only be narrowed.

**Network.** `Net` is a network capability over a finite set of rules, each a triple
`(address, port, operation)`, the operation being `Connect()`, `ConnectTls()`, `Listen()`,
`UdpBind()`, `UdpPeer()`, `ListenTls()`, `UnixConnect()`, `UnixListen()` or
`UnixListenRemoveStale()`.

- An address is a canonical host name (lowercase ASCII, no trailing dot, a valid host
  name) or a canonical IP literal (dotted IPv4; RFC 5952 IPv6 without brackets). A host
  whose final label is all ASCII digits or an IPv4 number (including a `0x` hexadecimal
  form), after ignoring one trailing root dot, must be canonical dotted-quad IPv4; this
  rejects bare numbers, short forms, leading zeros, out-of-range octets, `0x7f.0.0.1`,
  `example.123` and `1.2.3.4.5`, so a resolver cannot reinterpret a granted name.
- A port is one exact port; `0` in `Listen` or `UdpBind` permits an ephemeral bind port.
- A Unix rule's address is one canonical absolute socket path: `/`, then nonempty
  components other than `.` and `..`, with no trailing `/`, backslash or NUL, at most 107
  bytes; its port is `0`. Abstract-namespace sockets cannot be named.
- There are no wildcard, subnet or range rules. No rule means no network, not "all", and
  an empty rule set is a `Net` that permits nothing.

Each network operation is checked against the rules of the `Net` it is given before any
host effect; a failed check returns `NotGranted()` without resolving, connecting or
sending. A host-name rule authorizes that name, never the addresses it resolves to. A
`Net` can only be narrowed, to a subset of its rules, where a `ConnectTls` rule is the
narrowing of the `Connect` rule for the same address and port, and a `ListenTls` rule the
narrowing of the `Listen` rule. A stream opened under a `ConnectTls` rule carries no
plaintext until TLS is established with the rule's address as server name, and a stream
accepted on a listener opened under a `ListenTls` rule none until a server handshake
(`tls.accept`) succeeds ([network operations](core-library.md#network)).

UDP bind and peer authority are separate exact IP-address/port rules. A bound
`UdpSocket of n` can only be operated through its originating `n`. Sending requires
an exact `UdpPeer` destination rule. Receive silently discards ungranted sources
and keeps waiting without changing the output buffer; outside senders cannot
force `NotGranted` or another language error. TCP and UDP grants never imply one
another. Argument and authority checks precede cancellation; admitted effects
follow it (CONC-3).

Unix-domain stream authority is a rule per exact socket path. `UnixConnect` connects to
the path and `UnixListen` binds and listens on it; `UnixListenRemoveStale` also lets the
listen remove a stale socket file left at the path, and `UnixListen` is its narrowing.
A path rule authorizes that spelling only, never a link to it or another path naming the
same socket. TCP, UDP and Unix rules never imply one another.

**Processes.** `Proc` is a process capability over a finite set of rules, each naming one
canonical absolute executable path and the exact environment variable names passed through
to it. There is no `PATH` search, no wildcard and no directory rule, and an empty rule set
is a `Proc` that runs nothing. `proc.run` and `proc.spawn` check the program against the
rules of the `Proc` they are given before any host effect; a failed check returns
`NotGranted()` without spawning. A child's environment holds only variables its rule
names: those the launcher has set, or exactly the program's own values for them, never
another name. Its working directory is set only by an explicit `Dir`. A
streaming `Child of p` carries `p`'s authority, and every operation on it takes `p` as
well ([CAP-2](#cap-2-charged-at-use)). A run's or a wait's deadline names a `MonotonicClock` the
program already holds; it grants nothing, and no implicit clock is read. The child's own
file system, network and process access is outside Hirsel's model: granting a program is
trusting it with whatever the host lets it do ([processes](core-library.md#processes)).

Libraries define further capabilities and declare each global or scoped.

### CAP-2: Charged at use

Resources and handles do not replace the governing capability their operations require:
`file.write_all(dir, mut f, data)` takes the `Dir` as well as the file. An explicitly
sealed resource capability is itself a capability under [CAP-5](#cap-5-capability-sealing).
Handles of tasks, mutexes, atomics and channels carry run-time scheduling-domain affinity;
an operation with another domain's capability fails CAP-2 before any effect. Mutex,
Atomic and Sender handles live in run-owned tables, so they are sendable within their
domain and may be captured by the children of a scope in the same run.

Every type has a value-authority summary. A capability contributes its own nominal type;
aggregates join their parts, resources include the authority their declared destruction
requires, and closures include their captures. A builtin handle holds the authority of the
capability that governs it: a `Task` holds its `Scope`, and a `Sender`, `Receiver`, `Mutex`
or `Atomic` holds `Tasks`, so none of them is inert. Held authority never stands in for the
capability operand an operation requires. An ordinary opaque type exports an explicit
summary checked against its representation in its opacity owner. An explicit summary
cannot erase field or destruction authority, except by the nominal sealing of CAP-5, and
sealing never produces an empty summary. `inert` means the summary is empty. A function
whose parameters are all inert is **pure**: deterministic in its arguments. Kinds and the
incomplete flag follow from signatures, never from bodies.

A nonopaque resource's authority is the union of its declared field types' summaries and
any explicit type-level `uses {...}` summary, never inferred from its drop body. The drop
inherits that summary and may use only stored capabilities. Opaque exports declare both
bounds and authority: `pub opaque type Marker: () uses {}` explicitly exports no bounds and
no authority, while an omitted opaque bound clause is incomplete and rejected under
[LANG-6](language.md#lang-6-signatures).

### CAP-3: Restriction

A scope-forming library function (a transaction, a sandbox, a read-only query) gives its
body a scoped capability, declares the body `uses {}`, and bounds the body's result
`inert`. Together these mean the body holds only what it is passed, cannot keep it, and
cannot return authority. Authority (`uses`), locality (`in`) and identity (`of`) are
separate judgments. A closure whose own `uses` clause excludes authority it captures is
CAP-3 at the first such capture.

```hirsel
fn probe(d: Dir, p: Path) -> u64 {
  let measure = fn() -> u64 uses {Dir} {     // with uses {} the capture of d is CAP-3
    match file.size(d, p) { Ok(n) => n, Error(_) => 0 }
  }
  measure()
}
```

### CAP-4: Synchronisation

Every atomic operation and every mutex acquisition takes the `Tasks` capability, so code
that uses them is never pure and their order is a scheduling decision made through `Io`
([DET-1](determinism.md#det-1-contract)).

### CAP-5: Capability sealing

An opaque declaration marked `@capability(global)` or `@capability(local)` may have a
representation, including a resource representation. Its whole-value authority summary is
exactly its own nominal capability term. Its written type-level `uses` clause declares a
private implementation ceiling, which must include the authority of every represented
field, retained environment and implicit or explicit destruction. Only its opacity owner
can construct it or inspect its representation. Obtaining a represented value keeps that
value's own authority; there is no implicit conversion or authority-subset relation
between the seal and its representation. Ordinary functions that construct or inspect a
seal are checked from their declared parameters and results and the actual values they
use. A capability cannot satisfy `inert`, `data`, `key`, `flat` or `serializable`, even
when its representation is empty.

Nominal permissions use the declaration's fully qualified identity, never generic policy
arguments. Whole seals keep their summary inside their owner; projected values regain
their own authority through value derivation. Unknown generic or erased authority cannot
fit a finite ceiling without the relevant declared bound or capture contract. Primitive
root grants stay plain and sendable; represented seals prove every exported bound and
retained scope independently.

Missing implementation authority is CAP-5 at the declaration; access to the representation
outside the exact package and version that own it is [PROG-2](programs.md#prog-2-visibility).
Outside the owner, authority diagnostics name the seal and its declaration, never private
representation paths or a hidden grant to add.

The planned [read-only directory and environment-key
facets](std-library.md#capability-facets), not in `std@1`, are sealed capabilities.

`io.stdin` and `io.stderr` project `Stdin` and `Stderr` only when their exact
run grants are present. `Env` grants a set of exact names: `env.get` returns
`EnvError.NotGranted()` for every other name, independently of whether that name
exists on the host. Granted but unset names return `None`; the Env object
in the run configuration records both the name grants and their snapshots.
