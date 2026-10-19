# Capabilities and authority

All authority arrives as typed parameters. A program's entry receives root capabilities
(`Io`, `Args`, `Env`, `Unsafe`); everything else is projected from them, narrowed, or
created by a scope-forming library function. Holding a capability is necessary to use the
thing it names, and a value's **authority summary** says which capabilities it holds.
Whether a capability is global or scoped is fixed by
[MODE-6](ownership.md#mode-6-scoped-capabilities).

### CAP-1: Authority

Directories, network, processes, clocks, randomness, telemetry capture, tasks, standard
output, environment variables, runners, allocation placement and `Unsafe` arrive only as typed parameters. `Io` is projected only
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

**Directories.** `Dir` is a directory capability with an access, read or read-write: paths
are relative, cannot escape it, and it can only be narrowed, to a subdirectory
(`dir.narrow`) or to read-only access (`dir.read_only`).

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
is a `Proc` that runs nothing. A `Proc` can only be narrowed (`proc.narrow`), to rules each
naming one of its programs with a subset of that rule's environment names. `proc.run`,
`proc.pipeline`, `proc.spawn` and `proc.spawn_pipeline` check each
program against the rules of the `Proc` they are given before any host effect; a failed
check returns `NotGranted()` without spawning, and a pipeline checks every stage first. A child's environment holds only variables its rule
names: those the launcher has set, or exactly the program's own values for them, never
another name. Its working directory is set only by an explicit `Dir`. A
streaming `Child of p` or `ChildGroup of p` carries `p`'s authority, and every operation on it takes `p` as
well ([CAP-2](#cap-2-charged-at-use)). A run's or a wait's deadline names a `MonotonicClock` the
program already holds; it grants nothing, and no implicit clock is read. The child's own
file system, network and process access is outside Hirsel's model: granting a program is
trusting it with whatever the host lets it do ([processes](core-library.md#processes)).

**Signals.** `Signals` is a global capability over the host signals a run observes, a
nonempty subset of `SignalKind` (`Interrupt()`, `Terminate()`, `Hangup()`, for SIGINT,
SIGTERM and SIGHUP), which the run configuration's `io.signals` grant fixes. Absent, the
grant is denied: `io.signals` returns `NotGranted()` and every signal the process receives
keeps its default action, so a program that never asked for signals dies by one as any
process does. From the run's first projection, an arrival of an observed kind is pending
instead of fatal, and only `signal.wait` consumes it ([signals](core-library.md#effects-and-cancellation)):
no program code runs in a handler. A wait naming a kind outside the grant returns
`NotGranted()` before any effect. At most one arrival of a kind is pending; a second
Interrupt while one is pending keeps the default action, so a program that stops waiting
can still be interrupted. An arrival before the projection or of an unobserved kind takes
the default action as well.

**Runners and stores.** A `Runner` (planned) designates a place functions can run together
with a grant set the run configuration fixes; it can only be narrowed, a call through it
carries data and a definition's identity, never authority, and the callee's capabilities
come only from the place ([runners](runners.md)). A store is opened over a `Dir`, and its
authority is exactly that directory's; a transaction on it gives its body only a scoped
`Tx` ([storage](storage.md)).

Libraries define further capabilities and declare each global or scoped.
A package type that holds authority is opaque or sealed, and offers attenuated facets.

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

**Erased carriers.** A type whose representation holds erased state (an `Iter`, callbacks,
or other carriers) can carry that state's authority per value, as an `Iter` does. It names
those fields with `@captures(field, ..)`, either fields of a record or the field of a
`distinct` type:

```hirsel
@captures(inner)
pub opaque type Flow[T, E]: () uses {} { inner: Iter[Result[T, E]] }
```

A carrier's value-authority summary is its declared summary, specialised to its type
arguments, joined with the value's own **erased evidence**. A written carrier type has
unknown evidence, as a written `Iter[T]` does. Constructing a carrier gives it the joined
authority of the values it is built from. A function result or a callable result declared
with `@captures` ([CLO-4](closures.md#clo-4-escaping)) has the joined evidence of the
selected arguments, and the same call's crossing evidence, as an `Iter` result has. The
declaration's summary must cover the authority of the unselected fields; selected fields
are exempt because each value carries their authority. Multiplicity, cleanup and brands
stay the declaration's. A carrier cannot export `inert` (CAP-2) and cannot be a sealed
capability, a resource, `flat`, `serializable` or refined (CLO-4). A selector that names
no representation field is CLO-4.

### CAP-3: Restriction

A scope-forming library function (a transaction, [STORE-3](storage.md#store-3-transaction-scope);
a sandbox; a read-only query) gives its
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

**Narrowing.** A narrowing operation mints a new capability of its argument's type whose
authority is within its argument's, served by the same provider: `dir.narrow` to a
subdirectory, `dir.read_only` to read-only access, `net.narrow` and `proc.narrow` to rules
within their argument's, and `env.narrow` to a subset of names. A request outside the
argument's authority mints nothing: it returns `NotGranted()`, or `FileFault` for a path
that escapes a `Dir`. Each use checks the narrowed capability's own authority before any
host effect, so code holding only it cannot reach its parent's. Narrowings are run inputs,
recorded and replayed like the operations that use them.

**Grant subset.** One run configuration's grants are *within* another's when they hold no
authority the other lacks: `Unsafe` only if the other grants it; each Io projection (stdin,
stderr, stdout, tasks, each clock, random) only under the other's exact provider contract;
each mount only beneath one of the other's, its virtual path and its host root extending
that mount's by the same components, its access no wider (`read` is within `read_write`);
a network or process grant only under the same provider, each of its rules within one of
the other's rules by `net.narrow` or `proc.narrow` coverage; and each Env name only when the
other grants it with the same snapshot, set or unset. Absent Io or Env is within any
grants, and a present one only within a present one. The relation is total and lexical over
validated records and reads no host state; program, entry, inputs, Args, admission and
scripted provider data (peers, listeners, runs, children) are not authority and take no
part. A signals grant is within another's only under the same provider contract, its kinds a
subset of the other's; a script (`signals.script@1`) is provider data and takes no part.
It is reflexive and transitive, removing a grant or narrowing one stays within the
original, and adding one never does.

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
exists on the host. An `Env` can only be narrowed (`env.narrow`), to a subset of its names. Granted but unset names return `None`; the Env object
in the run configuration records both the name grants and their snapshots.

### CAP-6: Safe code

A definition is **safe** when its dependency closure reaches no **escape** from capability
checking. The closure is the one program identity covers
([run configuration](determinism.md#run-configuration)): every definition the checked
signature, body, private representations, implicit finalizers, compile-time execution and
selected generated instances read, transitively. The escapes are:

- an `extern "c"` function ([PROG-4](programs.md#prog-4-interop)), whose foreign code acts
  outside every grant;
- a core operation that requires `Unsafe` ([MEM-9](memory.md#mem-9-unsafe)): unchecked
  arithmetic (`int.unchecked_add`) and any later raw-pointer, manual-memory or
  weaker-atomic operation, each identified by its core role, never by its spelling.

Holding or passing `Unsafe`, `Ptr` or `FnPtr` values is not an escape: those values act
only through an escape. Selecting an `unsafe ffi` package is not one unless one of its
externs is reached. Core operations with safe signatures are not escapes: their native
implementations belong to the runtime, whose revision every program identity binds, and
are trusted as the runtime is. Std is checked, not trusted: it uses no `Unsafe`
([standard library](std-library.md)), so its operations are safe on their own closure, and
a std definition reaching an escape would be refused like any other. No package is exempt
by name or hash.

The **verdict** for a definition is either safe or the shortest dependency path from it to
an escape, both included, found breadth-first in definition-path order, so equal programs
report equal paths. `hirsel check --safe` prints the verdict for the program's entry.

A host requires safe code through the run configuration's code admission policy,
`admission.code` ([run configuration](determinism.md#run-configuration)), a seam whose
choice changes which programs launch and never what an admitted program means:
`admit.any@1`, the default, admits any checked code; `admit.safe@1` refuses a launch
whose entry is not safe when the run binds to its code, before any host is consulted,
with a refusal naming the path. Every launcher
applies it: the interpreter and the native driver from the checked program, and an
executable from the verdict compiled into it, which carries no path. A policy is part of
the record and so of run identity; the default, written or not, is absent from the
record's canonical bytes. Replay re-checks the policy against the code it replays. A
runner place applies the same verdict to a callee definition on arrival.
