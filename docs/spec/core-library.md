# Core library

The core library, `core@1`, is supplied by the compiler. It holds the capability types, the
root data types and the operations that need the runtime, the host or a privileged
representation. Its root types sit directly under `core@1`, and its operations are
`core@1/module.item`, written `module.item` in source. Everything else is ordinary Hirsel
source in the [standard library](std-library.md).

## Notation

The ledger states each operation's signature under the rules its comment line cites.
Qualified names identify library definitions; they are not dotted source declarations.
`I` and `J` range over sized integers, `F` over `f32` and `f64`, and `A` over integer,
float and `bool` scalars; these are intrinsic schema variables, not bounds or overloads.
Other bracketed variables are ordinary generics. Omitted effects and modes take the
[LANG-6](language.md#lang-6-signatures) defaults. `N` marks an explicit `@noalloc`
signature; unmarked operations may allocate. Scoped callback authority and allocation join
under [CLO-2](closures.md#clo-2-transparent), even for an `N` operation. Result ownership
follows the passing modes on every exit unless stated otherwise. Ordinary field and index
syntax needs no library call.

## The ledger

```text
// SER-1/2
fn codec.encode[T: serializable](value: T) -> Bytes
fn codec.decode[T: serializable](data: Bytes) -> Result[T, DecodeError]

// CAP-1/2/4, LANG-9, CONC-3
N fn io.stdin(rt: Io) -> Result[Stdin, GrantError]
N fn io.stderr(rt: Io) -> Result[Stderr, GrantError]
N fn io.stdout(rt: Io) -> Result[Stdout, GrantError]
N fn io.tasks(rt: Io) -> Result[Tasks, GrantError]
N fn io.monotonic_clock(rt: Io) -> Result[MonotonicClock, GrantError]
N fn io.wall_clock(rt: Io) -> Result[WallClock, GrantError]
N fn clock.monotonic(clock: MonotonicClock) -> Result[Instant of clock, ClockError]
N fn clock.wall(clock: WallClock) -> Result[i64, ClockError]
N fn instant.ticks(value: Instant) -> u64 uses {}
N fn io.net(rt: Io) -> Result[Net, GrantError]
N fn io.random(rt: Io) -> Result[Random, GrantError]
N fn io.process(rt: Io) -> Result[Proc, GrantError]
fn io.dir(rt: Io, p: Path) -> Result[Dir, FileError]
fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]
N fn arg.count(args: Args) -> u64
fn arg.text(args: Args, index: u64) -> Result[String, ArgError]
fn arg.path(args: Args, index: u64) -> Result[Path, ArgError]
fn path.of(text: String) -> Path
fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]
N fn err.print(s: Stderr, text: String) -> Result[(), OutputError]
N fn out.print(s: Stdout, text: String) -> Result[(), OutputError]
fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]
N fn test.check(t: Test, condition: bool) -> ()
N fn test.check_case(t: Test, ok: bool, case: u64) -> ()
N fn test.fail(t: Test) -> Never
fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir
fn test.net(t: Test, peers: List[NetPeer]) -> Net
fn test.process(t: Test, runs: List[ProcessScript]) -> Proc

// CAP-1/2, PROV-1/2, DROP-2/4, DET-1
fn file.open(d: Dir, p: Path) -> Result[File of d, FileError]
fn file.read(d: Dir, mut f: File of d, mut buffer: List[u8], max: u64) -> Result[u64, FileError]
fn file.seek(d: Dir, mut f: File of d, offset: u64) -> Result[(), FileError]
fn file.create(d: Dir, p: Path) -> Result[File of d, FileError]
fn file.write(d: Dir, mut f: File of d, data: Bytes) -> Result[u64, FileError]
fn file.append(d: Dir, mut f: File of d, data: Bytes) -> Result[(), FileError]
fn file.narrow(d: Dir, take f: File) -> Option[File of d]
fn file.walk(d: Dir) -> Result[Iter[Result[Path, FileError]], FileError]
fn file.size(d: Dir, p: Path) -> Result[u64, FileError]
fn file.read_bytes(d: Dir, p: Path) -> Result[Bytes, FileError] in d
fn file.write_bytes(d: Dir, p: Path, take data: Bytes) -> Result[(), FileError]
N fn file.remove(d: Dir, p: Path) -> Result[(), FileError]
fn file.create_atomic(d: Dir, p: Path) -> Result[AtomicFile of d, FileError]
fn file.write_all(d: Dir, mut f: AtomicFile of d, data: Bytes) -> Result[(), FileError]
fn file.commit(d: Dir, take f: AtomicFile of d) -> Result[(), FileError]

// CAP-1/2/3, PROV-1/3, DROP-2/3/4, CONC-3/8, DET-1
fn net.narrow(n: Net, rules: List[NetRule]) -> Result[Net, NetError]
fn net.resolve(n: Net, host: String, port: u16) -> Result[List[SocketAddr], NetError]
fn net.connect(n: Net, host: String, port: u16, timeout: NetTimeout) -> Result[Stream of n, NetError]
fn net.connect_addr(n: Net, addr: SocketAddr, timeout: NetTimeout) -> Result[Stream of n, NetError]
fn net.read(n: Net, mut s: Stream of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]
N fn net.read_into[k: size](n: Net, mut s: Stream of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]
N fn net.write(n: Net, mut s: Stream of n, data: Bytes) -> Result[u64, NetError]
N fn net.write_all(n: Net, mut s: Stream of n, data: Bytes) -> Result[(), NetError]
N fn net.shutdown(n: Net, mut s: Stream of n, how: Shutdown) -> Result[(), NetError]
N fn net.set_read_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]
N fn net.set_write_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]
N fn net.peer_addr(n: Net, s: Stream of n) -> Result[SocketAddr, NetError]
N fn net.close(n: Net, take s: Stream of n) -> ()
fn tls.connect(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots) -> Result[Stream of n, NetError]
N fn tls.close(n: Net, take s: Stream of n) -> Result[(), NetError]
fn net.listen(n: Net, addr: SocketAddr) -> Result[Listener of n, NetError]
fn net.accept(n: Net, mut l: Listener of n) -> Result[(Stream of n, SocketAddr), NetError]
N fn net.local_addr(n: Net, l: Listener of n) -> Result[SocketAddr, NetError]

// CAP-1/2, DET-1
fn proc.run(p: Proc, program: Path, args: List[String], stdin: Bytes, dir: Option[Dir], max_output: u64) -> Result[ProcessOutput, ProcessError]

// CONC-2/3/5/6/7, CLO-3, FAIL-2
fn task.scope[R](tasks: Tasks, body: once fn(s: Scope) -> R) -> R
fn task.async[R: sendable](s: Scope, take f: once fn() -> R) -> Task[R] of s
fn task.await[R](s: Scope, take h: Task[R] of s) -> R
N fn task.cancel[R](s: Scope, h: Task[R] of s) -> ()
fn task.detach[R](s: Scope, take h: Task[R] of s) -> ()
N fn task.check_canceled(s: Scope) -> Result[(), CancelError]
fn task.isolate[R: sendable](tasks: Tasks, take body: once fn() -> R) -> Result[R, Failure]
fn mutex.create[T: sendable](tasks: Tasks, take v: unique T) -> Mutex[T]
fn mutex.with[T, R: sendable](tasks: Tasks, m: Mutex[T], body: fn(mut v: unique T) -> R uses {}) -> R
fn channel.open[T: sendable](tasks: Tasks) -> (Sender[T], Receiver[T])
fn sender.dup[T](tasks: Tasks, s: Sender[T]) -> Sender[T]
fn channel.send[T: sendable](tasks: Tasks, mut s: Sender[T], take v: T) -> Result[(), (T, ChannelError)]
fn channel.receive[T](tasks: Tasks, mut r: Receiver[T]) -> Result[T, ChannelError]
fn atomic.create[A](tasks: Tasks, value: A) -> Atomic[A]
N fn atomic.load[A](tasks: Tasks, a: Atomic[A]) -> A
N fn atomic.store[A](tasks: Tasks, a: Atomic[A], value: A) -> ()

// MEM-4/5/11/14, PROV-1/4, SEM-4
N fn arena.with[R](body: fn(a: Arena) -> R) -> R
N fn arena.bounded[R](cap: u64, body: fn(a: Arena) -> R) -> Result[R, OutOfSpace]
fn arena.seal[T](take a: Arena in a, take root: T in root) -> Region[T]
N fn region.with[T, R](mut r: Region[T], body: fn(root: T) -> R) -> R
fn copy_to[T: copyable](a: Arena, x: T) -> T in a
fn shared.freeze[T: inert](take x: unique T) -> Shared[T]
fn shared.thaw[T](take s: Shared[T]) -> Result[unique T, Shared[T]]
fn slots.empty[T]() -> Slots[T]
fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s
N fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s
fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]
N fn list.len[T](xs: List[T]) -> u64
N fn array.len[T](xs: Array[T]) -> u64
fn list.push[T](mut xs: List[T], take value: T) -> ()
N fn list.pop[T](mut xs: unique List[T]) -> Option[T]
N fn list.remove[T](mut xs: unique List[T], index: u64) -> T
N fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()
fn list.reserve[T](mut xs: List[T], capacity: u64) -> ()
fn iter.from[S, T](take state: S, take next: fn(mut S) -> Option[T]) -> Iter[T] uses {state, next}
fn iter.map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f}
fn iter.next[T](mut it: Iter[T]) -> Option[T]

// SEM-1/2/3/5, GEN-2, MEM-8/9
N fn int.widen[I, J](value: I) -> J
N fn int.try_narrow[I, J](value: I) -> Option[J]
N fn int.narrow[I, J](value: I) -> J
N fn int.truncate[I, J](value: I) -> J
N fn int.wrapping_add[I](a: I, b: I) -> I
N fn int.saturating_add[I](a: I, b: I) -> I
N fn int.checked_add[I](a: I, b: I) -> Option[I]
N fn int.div_floor[I](a: I, b: I) -> I
N fn int.mod[I](a: I, b: I) -> I
N fn int.div_exact[I](a: I, b: I) -> I
N fn int.unchecked_add[I](permit: Unsafe, a: I, b: I) -> I
N fn float.min[F](a: F, b: F) -> F
N fn float.max[F](a: F, b: F) -> F
N fn float.min_number[F](a: F, b: F) -> F
N fn float.max_number[F](a: F, b: F) -> F
N fn float.mul_add[F](a: F, b: F, c: F) -> F
N fn float.total_order[F](a: F, b: F) -> i32
N fn float.to_i32[F](value: F) -> Result[i32, FloatIntError]
N fn float.from_int[I, F](value: I) -> F
N fn f32.bits(value: f32) -> u32
N fn f32.from_bits(value: u32) -> f32
N fn f64.bits(value: f64) -> u64
N fn f64.from_bits(value: u64) -> f64
N fn string.byte_len(s: String) -> u64
N fn string.scalars(s: String) -> ScalarView in s
N fn string.positions(s: String) -> PositionView of s in s
N fn string.slice(s: String, start: StrPos of s, end: StrPos of s) -> String in s
fn string.from_bytes(take bytes: Bytes) -> Result[String, Bytes]
N fn bytes.len(src: Bytes) -> u64
fn bytes.from_list(take src: List[u8]) -> Bytes
N fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src
N fn string.as_bytes(s: String) -> Bytes in s
N fn string.position(s: String, offset: u64) -> Option[StrPos of s]
N fn string.byte_offset(s: String, pos: StrPos of s) -> u64
fn path.text(p: Path) -> String
N fn failure.check(condition: bool, rule: CheckRule) -> () uses {}
fn map.empty[K: key, V]() -> Map[K, V]
fn map.insert[K: key, V](mut m: Map[K, V], k: K, take v: V) -> ()
N fn map.get[K: key, V](m: Map[K, V], k: K) -> Option[V] in m
fn set.empty[T: key]() -> Set[T]
fn set.insert[T: key](mut s: Set[T], take value: T) -> ()
N fn set.contains[T: key](s: Set[T], value: T) -> bool
fn sorted_map.empty[K, V](by: Order[K]) -> SortedMap[K, V] of by
fn sorted_map.insert[K, V](by: Order[K], mut m: SortedMap[K, V] of by, k: K, take v: V) -> ()
N fn simd.splat[A, n: size](value: A) -> Simd[A, n]
N fn simd.add[I, n: size](a: Simd[I, n], b: Simd[I, n]) -> Simd[I, n]
```

A core operation that only advances an erased state it takes `mut` (`iter.next`) is
declared at the argument's own captures, so `iter.next(mut it)` and `for x in mut it` over
an inferred iterator need no written type. An annotated `let mut` binding of an erased state
holds its written unknown captures.

## Authority and errors

`Io`, `Args`, `Env`, `Unsafe` and the global projections are plain capabilities; `Path` is
an inert, plain, immutable path value. `File` and `AtomicFile` carry `Dir` authority, task
handles `Scope`, and `Mutex`, `Atomic` and channel handles `Tasks`. The latter use run-time
scheduling-domain affinity, not an implicit static `of tasks` brand; an operation with
another domain fails [CAP-2](capabilities.md#cap-2-charged-at-use) before any effect. The
declaration of a fake capability fixes its kind too.

Errors are data:

```text
CancelCause   = Requested | Parent | ChildFailed
CancelError   = Canceled(CancelCause)
ChannelError  = Closed | Canceled(CancelCause)
GrantError    = NotGranted()
type FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory()
InputError    = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()
OutputError   = OutputFault(u32) | Canceled(CancelCause)
EnvError      = NotGranted() | InvalidUtf8()
ArgError      = MissingArgument() | InvalidUtf8()
FloatIntError = NotFinite | OutOfRange
OutOfSpace    = OutOfSpace()
```

A missing configured projection returns `GrantError::NotGranted`; a missing mount, or an
operation outside the granted file rights, returns `FileError::NotGranted` before any host
effect. File providers report portable `NotFound`, `PermissionDenied`, `AlreadyExists`,
`IsDirectory` and `NotDirectory` errors for those conditions. The host maps ENOENT,
EACCES/EPERM, EEXIST, EISDIR and ENOTDIR respectively; the simulated directory produces
the corresponding path/type errors. Permission and existence faults can also be explicit
provider inputs. Path-confinement refusals retain `FileFault` and never masquerade as OS
permission errors. Other file errors retain `FileFault(u32)`; file and output fault codes
are capability inputs, not inferred OS error numbers. Every variant is recorded and
replayed as a run input; injected non-canceled errors can follow partial write effects.

## Network types

`Stream` and `Listener` are noncopyable, sendable resources carrying `Net` authority,
branded by the `Net` that produced them (`Stream of n`, `Listener of n`). Their finalizers
are `@noalloc`, close the socket, discard close errors and send no TLS `close_notify`. The
network data types are root core types, with variant ordinals in the order written:

```text
IpAddr       = V4(Inline[u8, 4]) | V6(Inline[u8, 16])          // network byte order
SocketAddr   = { ip: IpAddr, port: u16 }
NetOperation = Connect() | ConnectTls() | Listen()
NetRule      = { address: String, port: u16, operation: NetOperation }
NetTimeout   = Unbounded() | Nanos(u64)
Shutdown     = Read() | Write() | Both()
TlsRoots     = WebPki() | Pem(Bytes)
TlsFailure   = UnknownIssuer() | Expired() | NotYetValid() | NameMismatch() | Revoked()
             | BadCertificate() | AlertReceived(u8) | HandshakeFailure() | PeerMisbehaved()
             | Truncated()
NetError     = NotGranted() | Canceled(CancelCause) | InvalidInput() | UnknownHost()
             | ResolverFailure() | ConnectionRefused() | ConnectionReset()
             | ConnectionAborted() | NotConnected() | AddrInUse() | AddrNotAvailable()
             | HostUnreachable() | NetworkUnreachable() | NetworkDown() | BrokenPipe()
             | TimedOut() | Tls(TlsFailure) | Fault(u32)
```

`Fault` codes, like `FileFault` codes, are capability inputs, not OS error numbers. One error
type serves the whole network stack, so `connect`, `tls.connect`, `read` and `write` compose
with plain `?`. Interrupted and would-block conditions never surface: the provider retries
interrupted calls, and every operation blocks.

The scripted-peer types used by `test.net` and the `net.script@2` provider:

```text
NetPeer       = { address: String, port: u16, resolves_to: List[IpAddr],
                  tls: PeerTls, connections: List[NetConnection] }
PeerTls       = Plain() | Certificate(List[String]) | Fails(TlsFailure)
NetConnection = Refused() | Unanswered() | Open(List[NetStep])
NetStep       = Send(Bytes) | Expect(Bytes) | Stall() | Close() | Reset() | Truncate()
```

`arg.count` counts the ordered launch arguments, excluding the executable and tool options.
`arg.text` returns the unchanged UTF-8 argument at a zero-based index, or
`MissingArgument()` outside the snapshot and `InvalidUtf8()` for a non-UTF-8 argument.
Run descriptions contain UTF-8 snapshots; host admission may reject an invalid snapshot
before entry. Neither operation consults ambient process arguments.

## Effects and cancellation

`stdin.read` appends at most `max` bytes to `buf` and returns the appended count,
zero only at end of input. A positive read returns as soon as bytes are available;
it does not wait to fill `max`. Zero `max` returns `InvalidInput()` without reading.
Interrupted reads retry inside the provider. An error appends nothing. All handles
of the run's `Stdin` share one input cursor. `InputFault` codes are capability
inputs, like `OutputFault` codes. The runtime byte operation accepts arbitrary bytes;
`InvalidUtf8()` is available to the source text helpers.

`env.get` reads only an exact name in the run configuration's Env object. An
absent name returns `NotGranted()`; a name mapped to null returns `Ok(None())`;
a string returns `Ok(Some(value))`. No enumeration or ambient process lookup is
provided. Invalid UTF-8 from a host provider returns `InvalidUtf8()`; run-description
admission may reject an invalid snapshot before entry. Io presence alone grants
neither input nor output: stdin and stderr require their own configured projections.

`err.print` and `out.print` write exactly the supplied UTF-8 bytes, with no newline. `Canceled` means no
output or effect happened; another output or file error may follow partial effects.
Output success records that the provider accepted all supplied bytes at its stated effect
boundary; an unflushed private buffer is not that boundary. Neither an ordinary successful
write nor its recording promises durability across a crash.

Borrowed and `mut` inputs survive every `Result`; taken data is consumed on both outcomes
unless it is explicitly returned. A successful `file.write_all` writes all bytes; a
successful `file.write_bytes` replaces the file's contents. `file.narrow` consumes its
handle: success rebrands it, failure closes it. Narrowing requires the opened inode to be
currently reachable through the target `Dir`, the original `Dir` included; unlinking or
replacing that directory entry makes `file.narrow` return `None`. A `File` closes on drop;
an `AtomicFile` aborts and removes its temporary on drop, and `file.commit` consumes it on
either outcome. Directory paths are relative; absolute paths, traversal outside the root
(resolved links included) return `FileFault` without granting
authority. `file.walk` yields relative paths in UTF-8 byte order and ends after its first
`Error`.

`file.open` opens a read-only `File` at position 0; `file.create` opens an
append-only `File`, creating a missing file and truncating an existing file to zero
bytes. Both use the same `File` type, with no source mode flags. Providers return
`FileFault` for reads on a created handle or writes on an opened handle. A read-only
`Dir` denies create and writes with `NotGranted`.

`file.read` appends up to `max` bytes to `buffer` and returns the appended count;
0 means end of file when `max` is positive. With `max` 0 it succeeds without changing
the buffer or position. Reads may return a short prefix. `file.seek` sets an absolute
byte position, including beyond the end (a subsequent read returns 0), without
changing file contents. An unrepresentable position returns `FileFault`. Writes
always append regardless of the seek position. `file.write` returns the accepted
byte count, possibly a prefix; `file.append` accepts all bytes on success.
`file.write_all` remains the all-bytes operation for `AtomicFile`. Empty writes
succeed with no effect. Interrupted provider calls retry internally. Reads, seek
outcomes and handle acquisitions are run inputs recorded for DET-1 replay; create
and writes are recorded effects, like the whole-file operations. Read errors leave
the buffer and position unchanged; non-canceled write errors may follow a prefix.
Narrowing preserves a File's mode and position. An opened handle continues to refer
to its file after unlink or replacement; it does not reopen the stored path.

`test.check(t, false)` fails LANG-9. `test.check_case` behaves like `test.check`; on false
it first reports `test failure: seed=S case=C` and a newline through Test reporting, using
the run's Test seed and the supplied zero-based case index. `test.fail` records a LANG-9
failure through Test reporting, then ends the test along its failure edge after ordinary
failure cleanup; it never returns. `test.dir` creates a confined fake file system from its
entries; `test.net` creates a fake `Net` served by scripted peers ([below](#network));
`test.process` creates a fake `Proc` served by scripted runs ([below](#processes)).

## Concurrency

`channel.open` creates an unbounded FIFO. `send` never waits for buffer space; success
transfers the value into the buffer, while `Closed` returns it unchanged. `receive` waits
only while the buffer is empty and senders are live; cancellation returns `Canceled` without
dequeuing. A nonblocking send does not check cancellation (its `ChannelError` type is shared
with `receive`).

Each nested `task.scope` inherits cancellation from its containing task. A child uses a
captured global `Tasks` value to open its own scope and obtain its own `Scope` for
`check_canceled`; it never captures its parent's `Scope`. `cancel` targets the named
child's cancellation subtree, not its siblings. An explicit cancellation's cause is
`Requested`, an inherited one `Parent`, and a cancellation caused by a failed child
`ChildFailed`. Joins deliver owned child results without a cancellation check. Closures
passed to `task.async` and `task.isolate` obey [CLO-3](closures.md#clo-3-task-closures),
the Receiver exception included; this constraint is part of those intrinsic signatures.
Receiver captures are taken from immutable outer bindings and may then be rebound mutable
inside the child.

`Failure` is the data record
`{ kind: FailureKind, rule: String, site: SourceSite, spawn: List[u64], cause: Option[box Failure] }`,
where `SourceSite` is `{ definition: String, line: u64, column: u64 }`, using a fully
qualified definition path and one-based source coordinates, and `FailureKind` is
`Check | Contract | Depth | OutOfSpace | DeadMutex`. The site belongs to the failing
operation, not to the isolation boundary.

## Network

`net.narrow` returns a `Net` with exactly the requested rules when each equals a rule of
`n`, or is `(a, p, ConnectTls())` under `(a, p, Connect())`; otherwise it returns
`NotGranted()` and mints no `Net`. An empty rule list is allowed and permits nothing.

| Operation | Rule required in the `Net` passed |
|---|---|
| `net.resolve(n, host, port)` | a rule naming `(host, port)`, any operation |
| `net.connect(n, host, port, t)` | `(host, port, Connect())` or `(host, port, ConnectTls())` |
| `net.connect_addr(n, addr, t)` | a rule whose address is the canonical literal of `addr.ip`, with `addr.port`; a host-name rule never authorizes a raw address |
| stream operations | the stream's own grant |
| `tls.connect(n, take s, server_name, roots)` | on a `ConnectTls` stream, `server_name` equal to the rule's address |
| `net.listen(n, addr)` | `(literal, port, Listen())` for `addr`; port 0 grants an ephemeral port |
| `net.accept(n, mut l)` | the listener's grant; the peer is not checked |

`net.resolve` returns the provider's addresses in resolver order, each with `port`; a name
that does not exist is `UnknownHost()`, and a resolver that cannot answer
`ResolverFailure()`. `net.connect` resolves inside the provider and tries the addresses in
order, one at a time, returning the first stream or the last error; its timeout bounds the
whole call. Host names must be valid ASCII names and literals canonical, otherwise
`InvalidInput()` before any check. `Nanos(0)` and `max == 0` are `InvalidInput()`.

`net.read` appends between 1 and `max` bytes to `buffer` and returns their count as soon as
any are available, or returns 0 at end of stream; `net.read_into` writes a prefix of
`buffer` and returns its length, 0 at end of stream. Neither waits to fill its limit.
`net.write` returns the number of bytes accepted, at least 1 for nonempty `data`;
`net.write_all` succeeds only when every byte is accepted, and an error may follow a
partial write. A new stream has `Unbounded()` read and write timeouts; an expired timeout
returns `TimedOut()` and leaves the stream usable. `net.shutdown` follows the host's
half-close semantics. `net.close` and dropping a stream are the same effect.

`tls.connect` performs a TLS 1.2 or 1.3 client handshake on the stream, with `server_name`
as both SNI and verification name, verifying the peer's chain against `roots`: `WebPki()` is
the root set compiled into the provider, and `Pem(b)` trusts only the certificates in `b`,
where an empty or malformed bundle is `InvalidInput()`. Verification uses the provider's
time and cannot be disabled. On success it returns the same stream, after which every `net`
stream operation moves plaintext; on failure the stream is closed. A second `tls.connect` on
an upgraded stream is `InvalidInput()` and consumes and closes it. Reading a TLS stream whose
peer closes without `close_notify` returns `Tls(Truncated())`. `tls.close` sends
`close_notify`, then closes; on a plain stream it closes, with nothing to send. On a stream
opened under a `ConnectTls` rule, plaintext reads, writes and shutdown before a successful
`tls.connect`, and a `tls.connect` whose `server_name` differs from the rule's address,
return `NotGranted()` with no effect; timeout setters and `peer_addr` are allowed before the
upgrade, so a read timeout can bound the handshake.

`net.listen` binds and listens with a backlog of 128 and no address reuse; `net.accept`
returns a connected stream and its peer address.

The blocking network operations are `resolve`, `connect`, `connect_addr`, `read`,
`read_into`, `write`, `write_all`, `accept`, `tls.connect` and `tls.close`. Each is a
cancellation point only before it has an effect: a read before any byte is received, a
write before any byte is accepted, a connect or handshake before any packet is sent. The
others never return `Canceled`.

**Scripted peers.** `test.net` returns a fake `Net` served by its peers, whose rules are
`(address, port, ConnectTls())` for a `Certificate` or `Fails` peer and
`(address, port, Connect())` for a `Plain` peer. Duplicate `(address, port)` peers are
invalid input.

- Resolving a peer's address returns `resolves_to` with the peer's port; other names are
  `UnknownHost()`. `peer_addr` is a literal-address peer's literal, or a host-name peer's
  first `resolves_to` address; with an empty list it is `UnknownHost()`.
- Each connect to a peer takes its next connection entry: `Refused()` is
  `ConnectionRefused()`; `Unanswered()` is `TimedOut()` with a timeout and otherwise never
  completes ([CONC-8](concurrency.md#conc-8-global-quiescence)); an exhausted list refuses.
- Within `Open(steps)`, a read returns at most `max` bytes of the current `Send` and never
  crosses into the next step. `Close()` ends the peer's data, so reads return 0.
  `Truncate()` ends it without `close_notify`: every later read of a stream after a
  successful `tls.connect` returns `Tls(Truncated())`, and a read of any other stream
  returns 0. `Reset()` makes every later operation `ConnectionReset()`. `Stall()` delivers
  nothing, so a read times out or never completes. The end of the steps is `Close()`.
- Written bytes accumulate and are compared with the next `Expect(b)` once `b`'s length of
  them exist; a mismatch is `Reset()` at that point. Writes are otherwise accepted.
- `tls.connect` to a `Certificate(names)` peer succeeds exactly when `server_name` is in
  `names`, otherwise `Tls(NameMismatch())`; to a `Plain()` peer it is
  `Tls(HandshakeFailure())`; to a `Fails(f)` peer it is `Tls(f)`, after the same grant checks
  and with the same consumed stream and recorded events as any handshake failure. `roots`
  is not consulted, and script bytes are plaintext.

Every operation against a scripted peer is recorded as the same request and completion a
real network produces, so a simulated run yields a recording that replays like a real one
([network recording](determinism.md#network-recording)).

## Processes

The process data types are root core types, with variant ordinals in the order written:

```text
ExitStatus    = Exited(i32) | Signaled(i32)
ProcessOutput = { status: ExitStatus, stdout: Bytes, stderr: Bytes }
ProcessError  = NotGranted() | InvalidInput() | NotFound() | PermissionDenied()
              | OutputLimit() | Fault(u32)
ProcessScript = { program: String, args: List[String],
                  outcome: Result[ProcessOutput, ProcessError] }
```

`proc.run(p, program, args, stdin, dir, max_output)` runs one child to completion and
returns its exit status and everything it wrote to standard output and standard error.
The checks run in this order, each before any host effect:

1. `program` must be a canonical absolute path: it starts with `/`, has no empty, `.` or
   `..` component and no trailing `/`, and no argument contains a NUL byte; otherwise
   `InvalidInput()`.
2. `program` must equal a rule of `p` byte for byte; otherwise `NotGranted()`. No `PATH`
   search, link resolution or prefix match takes part in the check.
3. With `Some(d)`, the child's working directory is `d`'s directory; a `Dir` with no host
   directory (one from `test.dir`) is `InvalidInput()` for a host provider. With `None()`
   the working directory is `/`. A `Dir` grants the child nothing; it only names where it
   starts.

The child's argument vector is `program` followed by `args`. Its environment holds exactly
the variables its rule lists that are set in the launcher's environment, and nothing else.
`stdin` is written to the child's standard input, which is then closed; standard output and
standard error are captured separately and concurrently, so neither pipe can stall the
other. When either stream exceeds `max_output` bytes the child is killed and the result is
`OutputLimit()`; `max_output` 0 accepts only a child that writes nothing. A child that
exits returns `Exited(code)`, one ended by a signal `Signaled(number)`; a nonzero status is
an ordinary `Ok`, not an error. A missing executable is `NotFound()`, one the host refuses
to execute `PermissionDenied()`, and any other spawn or wait failure `Fault(code)`, a
capability input like other fault codes. `proc.run` blocks its task until the child exits;
it is not a cancellation point. Interactive and streaming children are not provided.

**Scripted runs.** `test.process` returns a fake `Proc` whose rules are the distinct
`program` values of its runs, with no environment names. A `proc.run` on it makes the same
checks, `InvalidInput()` and `NotGranted()` first, then takes the first unused script
entry whose `program` and `args` equal its own and returns its `outcome`, with an output
longer than `max_output` on either stream returning `OutputLimit()`. With no such entry the
result is `NotFound()`. `stdin` and `dir` are not consulted beyond their checks. A scripted
run is recorded as the same request and completion a host run produces, so a simulated run
replays like a real one ([process recording](determinism.md#process-recording)).

## Memory and iteration

Seal and freeze reject roots containing resources ([MEM-4](memory.md#mem-4-arenas),
[MEM-11](memory.md#mem-11-shared)). `region.with` lends a read-only root, and neither it nor
`arena.with` or `arena.bounded` may return a value placed in the storage it lent.
`slots.remove` transfers the stored value on success and returns `None` for a stale
generation; insertion and removal preserve container identity. `List`, `Array` and
`Inline` literals use the expected type.

All builtin values satisfy `noalloc_drop` exactly when their owned components do; `File`
and `AtomicFile` have noalloc finalizers, and task, channel, mutex and region cleanup
composes the payloads' effects. An erased closure's or iterator's destruction carries the
cleanup summary of its construction site in its type, independently of its call
allocation effect. `iter.from` accepts a global `next` function or closure, owns it and its
state, and computes sendability and authority from both; `next` need not be noalloc.
`iter.map` owns its inputs; advancing it preserves the iterator and moves out each `Some`
item. The `file.walk` iterator stops on `Error`; user `iter.from` callbacks decide their own
termination.

| List operation | Type | Bounds and failure |
|---|---|---|
| `list.len(xs)` | `u64` | Number of elements in `xs: List[T]`. |
| `xs[i]` | `T` | Zero-based `i: u64`; requires `i < list.len(xs)`, otherwise traps with diagnostic MEM-14 (a language failure, not a `Result` error). |
| `xs[i] = v` | `()` | Requires mutable `xs: List[T]` and `v: T`; the same MEM-14 bounds check happens before replacement. |
| `list.push(mut xs, v)` | `()` | Appends `v: T` to a mutable `List[T]`. |

**List storage primitives.** `list.pop` returns `None` when empty; otherwise it moves the
last element out before publishing `Some`. `list.remove` checks `index < len` before any
mutation, failing MEM-14 otherwise; it moves the selected element out and shifts the suffix
stably. `list.swap` checks `a`, then `b`, against `len` before any mutation, failing MEM-14
otherwise; equal valid indices are a no-op. `list.reserve` ensures a total capacity of at
least `capacity`, preserving every element and its order. Capacity is unobservable except
through allocation. Ordinary `mut` copy-on-write applies to `reserve`; `pop`, `remove` and
`swap` require unique storage and allocate nothing. Removed elements belong to the caller
and are never finalized by the operation.

**The failure primitive.** `type CheckRule = Arithmetic() | TextInvariant() | Bounds()`, and
`failure.check(condition, rule)` evaluates its arguments left to right. True has no effect;
false raises SEM-1, SEM-3 or MEM-14 respectively at that ordinary source call site and
enters the ordinary failure and cleanup path with its complete cause chain. It takes no
arbitrary rule or string, inspects no stack, grants no capability, reads no Test input,
allocates nothing and forwards no caller site. This is how source libraries, the standard
library included, raise language failures; `bytes.replace`'s empty-pattern SEM-3, for
example, is reported at its check site in the std source. A failure during cleanup keeps
FAIL-3 shielding.

**Owner-sharing slices.** `string.slice(s, start, end)` takes positions relative to this
exact source slice, a slice with a nonzero origin included; both brands are checked
statically. The interval is half-open; a reversed interval fails SEM-3 before a result is
published, and equal positions produce an empty slice. The result keeps the original owner,
byte origin and source loan, acquiring exactly the ordinary sharing credit of an owned
counted result; it allocates no bytes, descriptor or block and cannot mint uniqueness.
Releasing parent and child slices in any permitted order releases each credit once.
`string.from_bytes` consumes valid UTF-8 into a `String`, which may reuse the same owner,
start and length, or returns exactly the original invalid `Bytes`.

**Byte and position bridges.** `Bytes` supports read-only indexing `b[i] -> u8`, failing
MEM-14 when `i` is out of range; mutable indexing is rejected. `bytes.len` returns the
slice's byte count. Byte and string slices share the exact owner and interval and expose no
mutable alias or uniqueness. `bytes.slice` accepts `0 <= start <= end <= len` and otherwise
fails SEM-3 before creating a result. `bytes.from_list` consumes the list once, reusing its
storage when the layouts are compatible and copying otherwise. `string.as_bytes` views a
string's bytes. `string.position` returns `Some` exactly at a UTF-8 scalar boundary within
the source, zero and the terminal offset included, and `None` for an offset inside a scalar
or out of range. The brand belongs only to the `StrPos` inside `Some`, never to the `Option`
owner, and `None` carries no source identity. Matching preserves the payload's identity,
constructors cannot fabricate a source brand, and joining `Some` values from different
source owners cannot satisfy one exact owner. `string.byte_offset` extracts the relative
offset and grants no new position. `Path` is an opaque plain core handle; `path.text`
returns ordinary owned UTF-8 text and grants no host authority. Source libraries cannot emit
reinterpretation or other privileged storage operations.

## Numeric and string details

The expected type selects a conversion's destination `J`. `int.widen` requires all of
`I`'s range to fit in `J`; `int.try_narrow` tests representability; `int.narrow` fails
SEM-1 when the value is unrepresentable; and `int.truncate` requires a destination no wider
than the source and keeps the low bits, interpreted with `J`'s signedness. `int.div_floor`
rounds the quotient toward negative infinity, `int.mod` is its remainder with the divisor's
sign, and `int.div_exact` fails SEM-1 for a nonzero remainder; zero divisors and signed
`MIN / -1` fail. `int.unchecked_add`'s written aliasing contract is "no memory access; the
mathematical result must fit `I`"; violating it is outside DET-1. `simd.splat` requires
`n > 0`; `simd.add` checks integer lanes in ascending order and cites the call on overflow.

`float.from_int` converts any sized integer to `f32` or `f64`, rounding to nearest, ties to
even, with no error case. `float.to_i32` truncates toward zero, returning `NotFinite` for NaN
and infinities and `OutOfRange` when the truncated result is outside `i32`.
`float.total_order` returns -1, 0 or 1 for IEEE totalOrder after NaN canonicalization; the
NaN is the positive quiet canonical NaN. `f32.bits` and `f64.bits` expose the IEEE
binary32 and binary64 representation, and `from_bits` reinterprets an unsigned word of the
same width. These allocate nothing, preserve signed zero, infinities and every finite bit
pattern, and canonicalize NaNs as [SEM-2](semantics.md#sem-2-floats) requires.

The library reductions `f32.sum` and `f64.sum` repeatedly add adjacent pairs
left to right, carry an odd tail, and repeat until one value is left; an empty sum is +0.
`sum_exact` rounds the exact real sum once; NaN or opposing infinities give the canonical
NaN, a single infinity sign is preserved, and an exact zero is +0 except for an input of
only negative zeros.

`ScalarView` yields each `u32` Unicode scalar value; `PositionView` yields each scalar's
start and the terminal position, even for an empty string; `GraphemeView` is a standard
library type ([Unicode](std-library.md#unicode)). These are nonallocating borrowed loop
sources with declared loop-source associations, not erased `Iter` constructors.

## Dictionaries

`Order[T] { compare: fn(T, T) -> i32 uses {} }`, `Eq[T] { equal: fn(T, T) -> bool uses {} }`
and `Hasher[T] { hash: fn(T, u64) -> u64 uses {} }` are ordinary pure records. There is no
implicit dictionary derivation; builtin `data` and `key` operations are structural, and
scalar `u32.order` (likewise for the other integer types) is a named `Order`. Map
iteration yields borrowed `(K, V)` pairs and Set iteration borrowed `T`; consuming iteration
transfers them. `SortedMap` uses the supplied comparator (negative, zero or positive) and
keeps its brand; combining two sorted maps is an ordinary function that requires the same
brand. Dictionary callbacks may allocate unless individually `@noalloc`, so a containing
operation's allocation joins them.

## Example application definitions

The `reserve` example of [the first look](index.md#a-first-look) uses these signature-only
fixtures: `Db` (a global capability), `Tx` (a scoped capability), an inert plain `Sku`,
`Reservation { sku: Sku, count: u32 }`, `StockError = OutOfStock(Sku)` and
`TxFailure[E] = Aborted(E) | Canceled(CancelCause)`, with

```text
store.transact[R: inert, E: inert](db: Db, body: fn(tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]
stock.get(tx: Tx, sku: Sku) -> Result[u32, StockError]
stock.put(tx: Tx, sku: Sku, n: u32) -> Result[(), StockError]
reservations.insert(tx: Tx, value: Reservation) -> Result[Reservation, StockError]
```

They grant no ambient database authority. The overview's `CliError` is
`Usage(ArgError) | Files(FileError) | Output(OutputError)`.

`codec.encode` and `codec.decode` use the [canonical codec](serialization.md#ser-2-canonical-codec).
`DecodeError` is the nullary variant `Version | SchemaMismatch | Truncated |
Trailing | Scalar | Utf8 | Tag | Length | Duplicate | Order`; these are expected
validation failures with no partially decoded value. Operational refusal is separate.
