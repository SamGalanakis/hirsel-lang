# Core library

Browse the [full signature index](../api.md) for every public core and std declaration.

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
signature; unmarked operations may allocate. `@preserves_unique(p)` is the source attribute
([closures](closures.md)); `@unretained(q)` after it is ledger notation, not source syntax,
marking a borrowed parameter the operation reads and stores nothing derived from, which the
promise's borrowed-argument clause exempts ([MODE-2](ownership.md#mode-2-uniqueness)). Scoped callback authority and allocation join
under [CLO-2](closures.md#clo-2-transparent), even for an `N` operation. Result ownership
follows the passing modes on every exit unless stated otherwise. Ordinary field and index
syntax needs no library call.

## Value replacement

`replace(mut place, take new)` exchanges the complete value in mutable storage
with `new` and returns the displaced value. Neither value is finalized by the
exchange. Argument evaluation and address checks precede the exchange; if either
fails, no exchange occurs. It is a lowered intrinsic: its call becomes
TIR `Replace`, with no runtime operation. Lowered intrinsics have checked core
declarations but are exempt from runtime dispatch and operation manifests.

## The ledger

```text
// OWN-3, MODE-2
@preserves_unique(place) fn replace[T](mut place: T, take new: T) -> T

// SER-1/2
fn codec.encode[T: serializable](value: T) -> Bytes
fn codec.decode[T: serializable](data: Bytes) -> Result[T, DecodeError]

// CT-3
fn type_shape.info[T](s: TypeShape[T]) -> ShapeInfo
fn codegen.body[F](text: String) -> Body[F]

// CAP-1/2/4, LANG-9, CONC-3
N fn io.stdin(rt: Io) -> Result[Stdin, GrantError]
N fn io.stderr(rt: Io) -> Result[Stderr, GrantError]
N fn io.stdout(rt: Io) -> Result[Stdout, GrantError]
N fn io.tasks(rt: Io) -> Result[Tasks, GrantError]
N fn io.monotonic_clock(rt: Io) -> Result[MonotonicClock, GrantError]
N fn io.wall_clock(rt: Io) -> Result[WallClock, GrantError]
N fn clock.monotonic(clock: MonotonicClock) -> Result[Instant of clock, ClockError]
N fn clock.wall(clock: WallClock) -> Result[i64, ClockError]
N fn clock.sleep_until(clock: MonotonicClock, deadline: Instant of clock) -> Result[(), WaitError]
N fn instant.ticks(value: Instant) -> u64 uses {}
N fn instant.checked_add(clock: MonotonicClock, start: Instant of clock, nanoseconds: u64) -> Option[Instant of clock]
N fn io.net(rt: Io) -> Result[Net, GrantError]
N fn io.random(rt: Io) -> Result[Random, GrantError]
fn random.bytes(r: Random, count: u64) -> Result[Bytes, RandomError]
N fn io.process(rt: Io) -> Result[Proc, GrantError]
fn io.dir(rt: Io, p: Path) -> Result[Dir, FileError]
fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]
N fn arg.len(args: Args) -> u64
fn arg.text(args: Args, index: u64) -> Result[String, ArgError]
fn arg.path(args: Args, index: u64) -> Result[Path, ArgError]
fn path.of(text: String) -> Path
@preserves_unique(buf) @unretained(s) fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]
N fn err.print(s: Stderr, text: String) -> Result[(), OutputError]
N fn out.print(s: Stdout, text: String) -> Result[(), OutputError]
fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]
N fn test.check(t: Test, condition: bool) -> ()
N fn test.check_case(t: Test, ok: bool, case: u64) -> ()
N fn test.fail(t: Test) -> Never
N fn test.seed(t: Test) -> u64
N fn test.log(t: Test, text: String) -> ()
N fn test.expect_snapshot(t: Test, name: String, text: String) -> ()
fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir
fn test.net(t: Test, peers: List[NetPeer], listeners: List[NetListener]) -> Net
fn test.process(t: Test, runs: List[ProcessScript], children: List[ChildScript]) -> Proc

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
fn file.read_dir(d: Dir, p: Path) -> Result[List[DirEntry], FileError]
fn file.metadata(d: Dir, p: Path) -> Result[Metadata, FileError]
fn file.mkdir(d: Dir, p: Path) -> Result[(), FileError]
fn file.rename(d: Dir, from: Path, to: Path) -> Result[(), FileError]
fn file.remove_dir(d: Dir, p: Path) -> Result[(), FileError]
fn file.set_permissions(d: Dir, p: Path, permissions: u32) -> Result[(), FileError]
fn dir.remove_all(d: Dir, p: Path) -> Result[(), FileError]
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
fn tls.connect_as(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots, identity: TlsIdentity) -> Result[Stream of n, NetError]
fn tls.accept(n: Net, take s: Stream of n, server: TlsServer) -> Result[Stream of n, NetError]
fn tls.peer(n: Net, s: Stream of n) -> Result[Option[Bytes], NetError]
N fn tls.close(n: Net, take s: Stream of n) -> Result[(), NetError]
fn net.listen(n: Net, addr: SocketAddr) -> Result[Listener of n, NetError]
fn net.accept(n: Net, mut l: Listener of n) -> Result[(Stream of n, SocketAddr), NetError]
N fn net.local_addr(n: Net, l: Listener of n) -> Result[SocketAddr, NetError]
fn net.unix_connect(n: Net, path: Path) -> Result[Stream of n, NetError]
fn net.unix_listen(n: Net, path: Path) -> Result[Listener of n, NetError]

fn net.udp_bind(n: Net, addr: SocketAddr) -> Result[unique UdpSocket of n, NetError]
N fn udp.send_to(n: Net, mut s: UdpSocket of n, data: Bytes, addr: SocketAddr) -> Result[u64, NetError]
fn udp.recv_from(n: Net, mut s: UdpSocket of n, mut buffer: List[u8], max: u64) -> Result[UdpReceived, NetError]
N fn udp.close(n: Net, take s: UdpSocket of n) -> ()

// CAP-1/2, PROV-1/3, DROP-2/3, CONC-1/3, DET-1
fn proc.run(p: Proc, cmd: Command, stdin: Bytes, max_output: u64, deadline: Option[Deadline]) -> Result[ProcessOutput, ProcessError]
fn proc.pipeline(p: Proc, stages: List[Command], stdin: Bytes, max_output: u64, deadline: Option[Deadline]) -> Result[PipelineOutput, ProcessError]
fn proc.spawn(p: Proc, cmd: Command) -> Result[Child of p, ProcessError]
N fn child.write(p: Proc, mut c: Child of p, data: Bytes) -> Result[(), ProcessError]
N fn child.close_stdin(p: Proc, mut c: Child of p) -> ()
fn child.read_stdout(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]
fn child.read_stderr(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]
N fn child.wait(p: Proc, take c: Child of p, deadline: Option[Deadline]) -> Result[ExitStatus, ProcessError]
N fn child.kill(p: Proc, mut c: Child of p, signal: Signal) -> ()
fn proc.spawn_pipeline(p: Proc, stages: List[Command], max_stderr: u64) -> Result[ChildGroup of p, ProcessError]
N fn group.write(p: Proc, mut g: ChildGroup of p, data: Bytes) -> Result[(), ProcessError]
N fn group.close_stdin(p: Proc, mut g: ChildGroup of p) -> ()
fn group.read_stdout(p: Proc, mut g: ChildGroup of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]
fn proc.wait_group(p: Proc, take g: ChildGroup of p, deadline: Option[Deadline]) -> Result[GroupOutput, ProcessError]
N fn group.kill(p: Proc, mut g: ChildGroup of p, signal: Signal) -> ()

// CONC-2/3/5/6/7, CLO-3, FAIL-2
fn task.scope[R](tasks: Tasks, body: once fn(s: Scope) -> R) -> R
fn task.scope_until[R](tasks: Tasks, deadline: Deadline, body: once fn(s: Scope) -> R) -> Bounded[R]
fn task.async[R: sendable](s: Scope, take f: once fn() -> R) -> Task[R] of s
fn task.await[R](s: Scope, take h: Task[R] of s) -> R
N fn task.cancel[R](s: Scope, h: Task[R] of s) -> ()
fn task.detach[R](s: Scope, take h: Task[R] of s) -> ()
fn task.set[R](s: Scope) -> TaskSet[R] of s
N fn task.add[R](s: Scope, mut set: TaskSet[R] of s, take h: Task[R] of s) -> ()
fn task.await_any[R](s: Scope, set: TaskSet[R] of s) -> Option[u64]
fn task.await_at[R](s: Scope, mut set: TaskSet[R] of s, index: u64) -> R
fn task.cancel_all[R](s: Scope, take set: TaskSet[R] of s) -> ()
fn task.detach_all[R](s: Scope, take set: TaskSet[R] of s) -> ()
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
N fn shared.with[T, R](s: Shared[T], body: fn(v: T) -> R) -> R
fn slots.empty[T]() -> Slots[T]
@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s
N fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s
fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]
N fn list.len[T](xs: List[T]) -> u64
N fn array.len[T](xs: Array[T]) -> u64
@preserves_unique(xs) fn list.push[T](mut xs: List[T], take value: T) -> ()
N fn list.pop[T](mut xs: unique List[T]) -> Option[T]
N fn list.remove[T](mut xs: unique List[T], index: u64) -> T
N fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()
@preserves_unique(xs) fn list.reserve[T](mut xs: List[T], capacity: u64) -> ()
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
N fn f32.from_f64(value: f64) -> f32
N fn f64.from_f32(value: f32) -> f64
N fn char.to_u32(c: char) -> u32
N fn char.from_u32(value: u32) -> Option[char]
N fn string.byte_len(s: String) -> u64
N fn string.scalars(s: String) -> ScalarView in s
N fn string.positions(s: String) -> PositionView of s in s
N fn string.slice(s: String, start: StrPos of s, end: StrPos of s) -> String in s
@preserves_unique(bytes) fn string.from_bytes(take bytes: Bytes) -> Result[String, Bytes]
N fn bytes.len(src: Bytes) -> u64
@preserves_unique(src) fn bytes.from_list(take src: List[u8]) -> Bytes
N fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src
N fn string.as_bytes(s: String) -> Bytes in s
N fn string.position(s: String, offset: u64) -> Option[StrPos of s]
N fn string.byte_offset(s: String, pos: StrPos of s) -> u64
fn path.text(p: Path) -> String
N fn failure.check(condition: bool, rule: CheckRule) -> () uses {}
fn map.empty[K: key, V]() -> unique Map[K, V]
@preserves_unique(m) fn map.insert[K: key, V](mut m: Map[K, V], take k: K, take v: V) -> ()
N fn map.get[K: key, V](m: Map[K, V], k: K) -> Option[V] in m
N fn map.contains[K: key, V](m: Map[K, V], k: K) -> bool
N fn map.len[K: key, V](m: Map[K, V]) -> u64
fn map.remove[K: key, V](mut m: unique Map[K, V], k: K) -> unique Option[V]
fn set.empty[T: key]() -> unique Set[T]
@preserves_unique(s) fn set.insert[T: key](mut s: Set[T], take value: T) -> ()
N fn set.contains[T: key](s: Set[T], value: T) -> bool
N fn set.len[T: key](s: Set[T]) -> u64
fn set.remove[T: key](mut s: unique Set[T], value: T) -> bool
fn sorted_map.empty[K, V](by: Order[K]) -> unique SortedMap[K, V] of by
@preserves_unique(m) @unretained(by) fn sorted_map.insert[K, V](by: Order[K], mut m: SortedMap[K, V] of by, take k: K, take v: V) -> ()
N fn sorted_map.len[K, V](by: Order[K], m: SortedMap[K, V] of by) -> u64
fn sorted_map.get[K, V: copyable](by: Order[K], m: SortedMap[K, V] of by, key: K) -> Option[V] in m
fn sorted_map.remove[K, V](by: Order[K], mut m: unique SortedMap[K, V] of by, key: K) -> unique Option[V]
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
CancelCause   = Requested | Parent | ChildFailed | DeadlineExceeded
CancelError   = Canceled(CancelCause)
ChannelError  = Closed | Canceled(CancelCause)
GrantError    = NotGranted()
type FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory() | DirectoryNotEmpty() | CrossDevice()
InputError    = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()
OutputError   = OutputFault(u32) | Canceled(CancelCause)
ClockError    = Unavailable() | OutOfRange()
WaitError     = Clock(ClockError) | Canceled(CancelCause)
RandomError   = Unavailable()
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

## Filesystem lifecycle

```text
type FileKind = Regular | Directory | Symlink | Other
type Metadata { kind: FileKind, size: u64, modified_unix_ns: Option[i128], permissions: u32 }
type DirEntry { path: Path, kind: FileKind }
```

`file.read_dir` lists direct children, including empty directories and symlinks,
sorted by UTF-8 path bytes. Entry paths are relative to the supplied `Dir`, including
the requested directory prefix. Invalid UTF-8 entry names return `FileFault`.
`file.metadata` describes the final entry without following its symlink; parent
symlinks resolve beneath the granted root. `size` is the host entry's byte length
(and the UTF-8 target length for simulated symlinks); non-regular sizes are
provider inputs. `modified_unix_ns` is an optional signed nanosecond count since
the Unix epoch, absent when unavailable. Metadata is a filesystem input and needs
no `WallClock`. A simulated new entry has no timestamp; recorded host metadata
and its changes remain inputs during replay.

`file.mkdir` creates one directory and returns `AlreadyExists` for any existing
entry, including a symlink. Missing parents return `NotFound`. `file.rename` moves
a file, symlink or directory between two paths within the same `Dir`; final
symlinks are moved or replaced as entries. Replacement follows POSIX semantics:
files and symlinks replace files or symlinks, directories replace only empty
directories, and a same-entry rename succeeds. Directory/non-directory mismatches
return `IsDirectory` or `NotDirectory`; a nonempty destination directory returns
`DirectoryNotEmpty`. Moving a directory into its descendant returns `FileFault`.
Cross-device rename returns `CrossDevice`, with no copy fallback.
`file.remove_dir` removes only an empty directory, never a final symlink; a
nonempty directory returns `DirectoryNotEmpty`. Rename and directory removal
refuse either spelling of the grant root with `FileFault`. Read-only grants deny
all three mutations with `NotGranted` before any storage call. Paths retain the
existing descriptor confinement and traversal refusal. Portable host errors also
map ENOTEMPTY to `DirectoryNotEmpty` and EXDEV to `CrossDevice`; other errors use
the existing `FileError` contract. Successful operations do not promise durability.

`Metadata.permissions` reports the low nine POSIX permission bits (`0o777`),
without file-type, set-id or sticky bits. It is a filesystem input, not a clock
read. Fresh simulated regular files use `0o666`, directories and symlinks `0o777`;
host modes reflect the host (including its creation mask) and are recorded.
`file.set_permissions` replaces those nine bits on a regular file or directory
inside its Dir, including the grant root. Values above `0o777`, final symlinks
and other entry kinds return `FileFault`; it never changes a symlink's target.
A read-only grant returns `NotGranted` before any storage call. Parent resolution
retains ordinary descriptor confinement. Permission bits do not grant authority;
host ownership and permission failures remain filesystem inputs.

`dir.remove_all` removes a directory and its descendants, or unlinks a final
symlink as an entry even when its target lies outside the grant. It never descends
symlinks. A non-directory, non-symlink starting entry returns `NotDirectory`;
a missing entry returns `NotFound`. An empty/dot or normalized grant-root path
returns `FileFault`, and a read-only grant returns `NotGranted` before storage
activation. Parent components retain existing confined resolution; all recursive
descents are descriptor-relative and use no-follow directory opens. The first
error stops removal and may leave a partially removed tree. No durability or
transactional removal is promised. Concurrent replacement can cause an error;
it cannot make recursive descent follow a link or widen the grant. Already
issued Dir and File handles retain their pinned identities across removal.

## Network types

`Stream`, `Listener` and `UdpSocket` are noncopyable, sendable resources carrying `Net` authority,
branded by the `Net` that produced them (`Stream of n`, `Listener of n`, `UdpSocket of n`). Their finalizers
are `@noalloc`, close the socket, discard close errors and send no TLS `close_notify`. The
network data types are root core types, with variant ordinals in the order written:

```text
IpAddr       = V4(Inline[u8, 4]) | V6(Inline[u8, 16])          // network byte order
SocketAddr   = { ip: IpAddr, port: u16 }
NetOperation = Connect() | ConnectTls() | Listen() | UdpBind() | UdpPeer() | ListenTls()
             | UnixConnect() | UnixListen() | UnixListenRemoveStale()
UdpReceived  = { count: u64, peer: SocketAddr, truncated: bool }
NetRule      = { address: String, port: u16, operation: NetOperation }
NetTimeout   = Unbounded() | Nanos(u64)
Shutdown     = Read() | Write() | Both()
TlsRoots     = WebPki() | Pem(Bytes)
TlsIdentity  = { chain: Bytes, key: Bytes }                      // PEM, leaf certificate first
TlsServer    = { identities: List[TlsIdentity], clients: TlsClientAuth }
TlsClientAuth = Anonymous() | Request(Bytes) | Require(Bytes)    // PEM trust set
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
type serves the whole network stack, so `connect`, `tls.connect`, `tls.accept`, `read` and `write` compose
with plain `?`. Interrupted and would-block conditions never surface: the provider retries
interrupted calls, and every operation blocks.

The scripted-peer types used by `test.net` and the `net.script@2` provider:

```text
NetPeer       = { address: String, port: u16, resolves_to: List[IpAddr],
                  tls: PeerTls, connections: List[NetConnection] }
PeerTls       = Plain() | Certificate(List[String]) | Fails(TlsFailure)
              | Presents(List[String], Bytes)
NetConnection = Refused() | Unanswered() | Open(List[NetStep])
NetStep       = Send(Bytes) | Expect(Bytes) | Stall() | Close() | Reset() | Truncate()
NetListener   = { address: String, port: u16, clients: List[NetClient] }
NetClient     = { from: SocketAddr, tls: PeerTls, steps: List[NetStep] }
```

`arg.len` is the number of ordered launch arguments, excluding the executable and tool options.
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

`clock.monotonic` samples an `Instant` of its clock. Its `instant.ticks` are nanoseconds
since an origin fixed for the run (`clock.monotonic.ns@1`), and later samples never
decrease. `instant.checked_add(clock, start, n)` advances an `Instant of clock` by `n`
nanoseconds, or returns `None()` when the ticks would pass `u64` MAX; it reads no clock
and records nothing. `clock.wall` returns signed Unix nanoseconds. A failed sample returns
`Unavailable()`, or `OutOfRange()` when the reading does not fit.

`clock.sleep_until(clock, deadline)` is a blocking `Io` call. In a cancelled scope it
returns `Canceled(cause)` first ([CONC-3](concurrency.md#conc-3-cancellation)); it returns
`Ok(())` at once when the deadline has passed, and otherwise suspends its task, without
polling, until the clock reaches the deadline. After `Ok(())`, a sample of the same clock is
at least the deadline. Cancelling a suspended wait removes its timer and returns
`Canceled(cause)`: waiting has no effect, so that is always allowed. `Clock(e)` reports a
provider that cannot read its clock. A host waits on its monotonic clock. A deterministic
simulation never waits: its monotonic clock is a virtual reading that starts at the
configured value, and when no task is runnable the simulator advances it to the earliest
pending deadline and resumes the waits due then, in the order they suspended. Pending
waits are an outstanding event source, so they never make a run Blocked
([CONC-8](concurrency.md#conc-8-global-quiescence)), and a run whose only live tasks are
waiting still terminates. A wait is recorded when it completes, with its outcome; replay
returns the recorded outcome without waiting.

`random.bytes(r, count)` returns exactly `count` bytes from the run's Random provider,
or `Unavailable()` with no bytes; it never publishes a partial draw. A zero count returns
empty `Bytes` without consulting the provider. A draw is nonblocking, like clock sampling,
so it is not a cancellation point ([CONC-3](concurrency.md#conc-3-cancellation)), and each
draw is a recorded capability input. The provider is chosen at launch
([providers](determinism.md#run-configuration)): `random.splitmix64@1` is a deterministic
simulation, predictable from the run's Random seed, and never protects a secret;
`random.system@1` reads the host's cryptographically secure generator.
`std@1/random` builds integer and bounded draws on this one operation.

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

The ordinary `std@1/test` assertion helpers (`equal`, `approx`, `assert_eq`,
`expect_error`, `expect_ok`, `property` and `property_cases`) and `std@1/text.finish_valid` are
`@track_caller`: a failing check identifies their application call, while reports retain
the helper's original operation as secondary context.

`test.check(t, false)` fails LANG-9. `test.check_case` behaves like `test.check`; on false
it first reports `test failure: seed=S case=C` and a newline through Test reporting, using
the run's Test seed and the supplied zero-based case index. `test.fail` records a LANG-9
failure through Test reporting, then ends the test along its failure edge after ordinary
failure cleanup; it never returns. `test.seed` returns the run's Test seed, the same value
`test.check_case` reports, so a generator seeded from it is replayed by the runner's
`--seed`. Test reporting is written one record per line, with each user text escaped:
`\` as `\\`, line feed, carriage return and tab as `\n`, `\r` and `\t`, and every other
C0 control and DEL as `\u{XX}` in two lowercase hex digits; other scalars are unchanged.
`test.log` reports `test log: T` and a newline, where `T` is the escaped text.
`test.expect_snapshot` reports `test snapshot N: T` and a newline. Its name `N` is 1 to 64
ASCII letters, digits, `_` or `-`; any other name fails LANG-9 without a report. The
snapshot is only recorded here: the test runner compares it with the committed file and
decides the test's outcome (`hirsel test` in the building guide).
`test.dir` creates a confined fake file system from its
entries; `test.net` creates a fake `Net` served by scripted peers ([below](#network));
`test.process` creates a fake `Proc` served by scripted runs and children
([below](#processes)).

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
`Requested`, an inherited one `Parent`, a cancellation caused by a failed child
`ChildFailed`, and one caused by its scope's deadline `DeadlineExceeded`. Joins deliver
owned child results without a cancellation check. Closures passed to `task.async` and
`task.isolate` obey [CLO-3](closures.md#clo-3-task-closures), the Receiver exception
included; this constraint is part of those intrinsic signatures. Receiver captures are
taken from immutable outer bindings and may then be rebound mutable inside the child.

`task.scope_until` returns the root core type `Bounded[R] = Finished(R) | Expired(R)`,
variant ordinals in the order written ([CONC-3](concurrency.md#conc-3-cancellation)). Its
`Deadline` is the one `proc.run` takes, so `deadline.at` is an `Instant` of
`deadline.clock` ([PROV-5](brands.md#prov-5-sibling-brands)). Whether the deadline fired
is decided when the scope exits: a deadline that passes while the exit waits for children
still cancels them and makes the result `Expired`. A scope cancelled for another cause
first keeps that cause, and its deadline still decides `Expired`. In deterministic
simulation the virtual clock advances to the earliest pending timer wait or scope deadline
when no task is runnable, and a scope deadline due at a reading expires before the timer
waits due at it resume.

`Failure` is the data record
`{ kind: FailureKind, rule: String, site: SourceSite, spawn: List[u64], cause: Option[box Failure] }`,
where `SourceSite` is `{ definition: String, line: u64, column: u64 }`, using a fully
qualified definition path and one-based source coordinates, and `FailureKind` is
`Check | Contract | Depth | OutOfSpace | DeadMutex`. The site follows the failing
operation and [`@track_caller`](failure.md#fail-1-kinds), rather than the isolation
boundary. Tooling additionally retains the original operation and the complete file
identity for human and JSON reports.

## Network

`net.narrow` returns a `Net` with exactly the requested rules when each equals a rule of
`n`, or is `(a, p, ConnectTls())` under `(a, p, Connect())`; otherwise it returns
`NotGranted()` and mints no `Net`; `(a, p, ListenTls())` is likewise covered by
`(a, p, Listen())`, and `(a, p, UnixListen())` by `(a, p, UnixListenRemoveStale())`. An
empty rule list is allowed and permits nothing.

| Operation | Rule required in the `Net` passed |
|---|---|
| `net.resolve(n, host, port)` | a rule naming `(host, port)`, any operation |
| `net.connect(n, host, port, t)` | `(host, port, Connect())` or `(host, port, ConnectTls())` |
| `net.connect_addr(n, addr, t)` | a rule whose address is the canonical literal of `addr.ip`, with `addr.port`; a host-name rule never authorizes a raw address |
| stream operations | the stream's own grant |
| `tls.connect(n, take s, server_name, roots)`, `tls.connect_as(n, take s, server_name, roots, identity)` | on a `ConnectTls` stream, `server_name` equal to the rule's address |
| `tls.accept(n, take s, server)` | on a stream accepted on a listener of `n` |
| `tls.peer(n, s)` | the stream's own grant |
| `net.listen(n, addr)` | `(literal, port, Listen())` or `(literal, port, ListenTls())` for `addr`; port 0 grants an ephemeral port |
| `net.accept(n, mut l)` | the listener's grant; the peer is not checked |
| `net.udp_bind(n, addr)` | `(literal, port, UdpBind())` for `addr`; port 0 permits an ephemeral port |
| `udp.send_to(n, mut s, data, addr)` | the socket's originating `Net`, and `(literal, port, UdpPeer())` for `addr` |
| `udp.recv_from(n, mut s, mut buffer, max)` | the socket's originating `Net`; only peers with an exact `UdpPeer()` rule deliver input |
| `udp.close(n, take s)` | the socket's originating `Net` |
| `net.unix_connect(n, path)` | `(path, 0, UnixConnect())` |
| `net.unix_listen(n, path)` | `(path, 0, UnixListen())` or `(path, 0, UnixListenRemoveStale())` |

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
where an empty or malformed bundle is `InvalidInput()`. An invalid verification name is
also `InvalidInput()`. Verification uses the provider's
time and cannot be disabled. On success it returns the same stream, after which every `net`
stream operation moves plaintext; on failure the stream is closed. A second `tls.connect` on
an upgraded stream is `InvalidInput()` and consumes and closes it. Reading a TLS stream whose
peer closes without `close_notify` returns `Tls(Truncated())`. `tls.close` sends
`close_notify`, then closes; on a plain stream it closes, with nothing to send. On a stream
opened under a `ConnectTls` rule, plaintext reads, writes and shutdown before a successful
`tls.connect`, and a `tls.connect` whose `server_name` differs from the rule's address,
return `NotGranted()` with no effect; timeout setters and `peer_addr` are allowed before the
upgrade, so a read timeout can bound the handshake.

`tls.connect_as` is `tls.connect` that presents `identity` as its client certificate when the
server requests one; a server that requests none never sees it. `identity` is checked as
`tls.accept` checks an identity, after `roots` and `server_name`: an invalid one is
`InvalidInput()`. A server refusing the certificate, or requiring one the client does not
send, ends the handshake with its alert, such as `Tls(AlertReceived(116))`
(`certificate_required`) or `Tls(AlertReceived(48))` (`unknown_ca`). The grant checks, the
consumed stream and every other rule are those of `tls.connect`; wherever a rule names
`tls.connect`, it covers `tls.connect_as`.

`tls.accept` performs a TLS 1.2 or 1.3 server handshake on an accepted stream with `server`.
Each identity's `chain` holds the PEM certificates sent to the client, leaf first, and its
`key` the PEM private key (PKCS#8, PKCS#1 or SEC1; the first key in the bytes) whose public
key is the leaf's. The server presents the first identity whose leaf certificate is valid
for the client's SNI name (a DNS subject alternative name, as a verifying client checks it),
or the first identity when the client sends no SNI or none matches. `clients` decides client
certificates: `Anonymous()` requests none; `Request(trust)` requests one and accepts a
client that sends none; `Require(trust)` requests one and fails a client that sends none
with `Tls(BadCertificate())`. A certificate the client sends is verified against `trust`
alone (the PEM certificates there are the trust anchors) for client authentication, at the
provider's time, as `tls.connect` verifies a server: an unknown issuer is
`Tls(UnknownIssuer())`, and the other verification failures are the `TlsFailure`s
`tls.connect` reports. An empty identity list, an empty or malformed chain or key, a key
type the provider cannot sign with, a key that does not match its leaf, and an empty or
malformed `trust` are `InvalidInput()`. The server offers ALPN `http/1.1` only (a client
offering ALPN without it fails the handshake), and issues no session tickets. On success
it returns the same stream, after which every `net` stream operation moves plaintext; on
failure the stream is closed. `tls.accept`
or `tls.connect` on an upgraded stream is `InvalidInput()` and consumes and closes it. A
stream accepted on a listener opened under a `ListenTls` rule (and no `Listen` rule, which
`net.listen` prefers) carries no plaintext before a successful `tls.accept`: reads, writes,
shutdown and `tls.connect` return `NotGranted()` with no effect, while timeout setters,
`peer_addr` and `tls.peer` are allowed. `tls.accept` on a stream from `connect` or
`connect_addr` is `NotGranted()`. Truncation and `tls.close` behave as for `tls.connect`
streams.

`tls.peer` returns the client certificate chain a successful `tls.accept` verified on the
stream, as the DER certificates the client sent, leaf first, concatenated (each DER
certificate delimits itself), so a program reads the peer's identity as data, and the
chain's SHA-256 identifies it; [`std@1/x509`](std-library.md#x509-certificate-fields) reads its subject and
alternative names. It returns `None()` on every other stream: plain
streams, streams upgraded by `tls.connect`, and `tls.accept` streams whose client sent no
certificate. It reads handshake state only: it never blocks, is never a cancellation point,
and answers the same after the stream's peer closes or resets.

`net.listen` binds and listens with a backlog of 128 and no address reuse; `net.accept`
returns a connected stream and its peer address.

The blocking network operations are `resolve`, `connect`, `connect_addr`, `unix_connect`,
`read`, `read_into`, `write`, `write_all`, `accept`, `tls.connect`, `tls.connect_as`,
`tls.accept` and `tls.close`. Each is a
cancellation point only before it has an effect: a read before any byte is received, a
write before any byte is accepted, a connect or handshake before any packet is sent. The
others never return `Canceled`. A canceled `tls.connect`, `tls.connect_as`, `tls.accept` or `tls.close` still consumes its
stream and performs the same cleanup as dropping it in that world, including scripted-client
`Expect` checks and their failure report.

**Datagrams.** `net.udp_bind` creates a fresh unique UDP socket, without address
reuse. `UdpBind` and `UdpPeer` are independent rules; neither implies TCP authority,
and TCP rules imply neither. All socket operations require the exact originating
`Net` (PROV-1), including narrowed Nets sharing a world. UDP uses IP literals;
name rules do not authorize their resolved addresses. `udp.send_to` sends one
datagram and returns its full byte count. Empty datagrams are valid. Payloads over
65507 bytes are `InvalidInput()` before any effect; no partial datagram is sent.
`udp.recv_from` appends at most `max` bytes from one datagram and returns
`UdpReceived(count, peer, truncated)`. `max == 0` is `InvalidInput()`.
`truncated` is true exactly when the received datagram was longer than `max`;
the excess is discarded and never delivered by a later receive. A count of zero
is an empty datagram, never EOF. A receive never combines datagrams. Datagrams
from peers outside the `UdpPeer` rules are silently discarded, and receive keeps
waiting; they neither cause an error nor change `buffer`. Errors and cancellation
leave `buffer` unchanged. UDP provides no delivery, ordering or duplication guarantee.
`udp.close` consumes the socket; implicit drop closes it with the same effect,
discarding close errors (DROP-3).

`net.udp_bind`, `udp.send_to` and `udp.recv_from` are cancellation points.
Their order is argument and authority checks, then `Canceled(cause)`, then
binding, sending or delivering an admitted datagram (CONC-3). A waiting receive
remains cancelable after discarding outside input. `udp.close` and implicit drop
are shielded from cancellation. Native schedulers park only the waiting task.

**Unix domain sockets.** `net.unix_connect` connects a stream socket to the socket at
`path`, and `net.unix_listen` creates a socket file at `path`, binds and listens with a
backlog of 128. They return the same `Stream` and `Listener` a TCP connect or listen does, so
every stream operation, `net.accept`, `tls.connect` and `tls.close` apply unchanged; the
listener's grant authorizes accepting, and accepted streams carry its authority. `path` must
be canonical as a Unix rule's address ([CAP-1](capabilities.md#cap-1-authority)), otherwise
`InvalidInput()` before the grant check. Unix peers are unnamed and have no IP address: the
address `net.accept` returns, and `net.peer_addr` and `net.local_addr` of a Unix stream or
listener, are `0.0.0.0` with port 0.

A connect to a path with no socket accepting on it, including a missing path, is
`ConnectionRefused()`. `net.unix_connect` has no timeout: it waits only while the
listener's backlog is full. A listen on a path that already exists is `AddrInUse()`, except
under a `UnixListenRemoveStale()` rule when the existing file is a socket on which a connect
is refused: the listen then removes that file and binds once more. A file other than a socket
is never removed. A listen whose directory does not exist is `AddrNotAvailable()`; other
host permission failures are `Fault` inputs. Closing or dropping a Unix listener, on every
path including cancellation cleanup, removes its socket file when the path still names the
socket it created (the same device and inode) and otherwise leaves the path alone; closing a
stream removes nothing.

`net.unix_connect` is a cancellation point before it connects: its order is the path check,
the grant check, then `Canceled(cause)`, then the connect (CONC-3). `net.unix_listen`, like
`net.listen`, never returns `Canceled`. A canceled operation on a Unix stream or listener
behaves as on a TCP one, and the cleanup of a stream or listener a cancellation abandons is
its drop.

**Scripted UDP.** `test.net` additionally grants `UdpBind` for each listener address
and `UdpPeer` for each `Plain` peer whose address is an IP literal. Scripted bind
addresses and peer data supply no authority in `net.script@2`: its explicit rules
remain authoritative. Each bound socket receives independent copies of the first
connection of each plain IP peer: `Open(steps)` supplies its steps, `Refused()`
acts as `Reset()`, and `Unanswered()` as `Stall()`. Each `Expect` matches one
complete outgoing datagram and each `Send` delivers one complete incoming datagram,
including empty datagrams; bytes are never merged across steps. A contradicted
`Expect` returns `ConnectionReset()`. Receive checks peers in declaration order;
`Expect`, `Stall`, `Close` and exhausted scripts have no deliverable input, and
`Reset` returns `ConnectionReset()`. `Truncate` likewise has no deliverable input;
UDP truncation is determined by payload size and `max`. A send to another bound
socket in the same world queues one datagram there; these queues precede scripted
input, in send order. A send to an address with no scripted receiver succeeds
without delivery. Narrowing away a peer's rule still leaves its scripted input,
which receive discards before looking for allowed input. TCP and UDP have separate
socket namespaces and script cursors. Port-zero UDP binds choose an unused UDP
port from 49152 upward in world order; exhaustion is `AddrNotAvailable()`.

**Scripted peers.** `test.net` returns a fake `Net` served by its peers, whose rules are
`(address, port, ConnectTls())` for a `Certificate` or `Fails` peer and
`(address, port, Connect())` for a `Plain` peer, plus `(literal, port, Listen())` for each
listener's address. Duplicate `(address, port)` peers and duplicate listener addresses
are invalid input. A `NetListener` with port 0 binds its clients to the first port-0
listen on that IP; clients target that listener's assigned address. Later port-0
listens on that IP have no scripted clients.

- A peer whose address is a canonical socket path is a Unix peer: it must be `Plain()`,
  with port 0 and no `resolves_to`, and `test.net` grants `(path, 0, UnixConnect())` for it
  in place of `Connect` and `UdpPeer` rules. Each `net.unix_connect` to it takes its next
  connection entry as a connect does; `Unanswered()` never completes. Its streams' `peer_addr`
  is `0.0.0.0` with port 0. A Unix peer is reached only by `net.unix_connect`.
- A listener's `address` is a canonical IP literal or a canonical socket path. A listener
  whose address is a path is a Unix listener: its port is 0, its clients come from `0.0.0.0`
  with port 0, since Unix peers are unnamed, and `test.net` grants `(path, 0, UnixListen())`
  for it in place of `Listen` and `UdpBind` rules. `net.unix_listen` on that path takes its
  clients as `net.listen` does, and Test reports name the listener by its path.
- Resolving a peer's address returns `resolves_to` with the peer's port; other names are
  `UnknownHost()`. `peer_addr` is a literal-address peer's literal, or a host-name peer's
  first `resolves_to` address; with an empty list it is `UnknownHost()`.
- Each connect to a peer takes its next connection entry: `Refused()` is
  `ConnectionRefused()`; `Unanswered()` is `TimedOut()` with a timeout and otherwise never
  completes ([CONC-8](concurrency.md#conc-8-global-quiescence)); an exhausted list refuses.
- Within `Open(steps)`, a read returns at most `max` bytes of the current `Send` and never
  crosses into the next step. `Close()` ends the peer's data, so reads return 0.
  `Truncate()` ends it without `close_notify`: every later read of a stream after a
  successful `tls.connect` or `tls.accept` returns `Tls(Truncated())`, and a read of any
  other stream returns 0. `Reset()` makes every later operation `ConnectionReset()`. `Stall()` delivers
  nothing, so a read times out or never completes. The end of the steps is `Close()`.
- Written bytes accumulate and are compared with the next `Expect(b)` once `b`'s length of
  them exist; a mismatch is `Reset()` at that point. Writes are otherwise accepted.
- `net.listen` on a scripted listener's address succeeds. A second listen on the same
  nonzero address while the first listener is open is `AddrInUse()`. A granted listen
  on an address with no scripted listener has no clients. `net.local_addr` is the assigned listen address, including
  the deterministic ephemeral port for a port-0 listen ([simulation](determinism.md#det-1-contract)).
- `net.accept` takes the address's next client in list order, across every listener opened
  on it. The stream follows the client's steps as `Open(steps)` does, and the accepted
  address and `peer_addr` are `from`. After the last client, accept returns
  `ConnectionAborted()`. An accepted stream has no TLS: `tls.connect` on it is
  `Tls(HandshakeFailure())`.
- A client's `Expect` is an assertion about the program: a mismatch fails LANG-9 at the
  write that completes it, after reporting `test failure: net client C of A expected E but
  the program wrote W` and a newline through Test reporting, where `C` is the zero-based
  client index, `A` the listen address and `E` and `W` byte-string literals of the expected
  bytes and of the same number of written bytes. Closing an accepted stream, by `net.close`,
  `tls.close` or drop, while its client has an unmet `Expect` fails LANG-9 the same way,
  reporting `test failure: net client C of A expected E but the program closed after
  writing W`, with `W` the bytes written toward it. Closing a listener while its address
  has an unaccepted client with an `Expect` fails LANG-9 after reporting
  `test failure: net client C of A was never accepted`. The assertion runs at close;
  re-listening cannot defer or undo it. For example, opening a listener whose first
  client has `Expect(b"pong")`, then leaving its scope without accepting that client,
  fails LANG-9 immediately, even if the next statement would listen on the same address.
- `tls.connect` to a `Certificate(names)` peer succeeds exactly when `server_name` is in
  `names`, otherwise `Tls(NameMismatch())`; to a `Plain()` peer it is
  `Tls(HandshakeFailure())`; to a `Fails(f)` peer it is `Tls(f)`, after the same grant checks
  and with the same consumed stream and recorded events as any handshake failure. `roots`
  is not consulted, and script bytes are plaintext. Scripted peers request no client
  certificate, so `tls.connect_as` checks its identity and otherwise behaves as
  `tls.connect`. `Presents` describes a client and is invalid input for a peer.
- `tls.accept` validates `server` as the host does, then handshakes with the stream's
  client by its `tls`. A `Certificate(names)` or `Presents(names, chain)` client sends the
  first of `names` as SNI when it is a DNS name, and none otherwise; the server selects its
  identity from that SNI as the host does. The client accepts the selected identity exactly
  when its leaf certificate is valid for a name in `names` (a DNS or IP subject alternative
  name; time and chain are not checked), otherwise the handshake fails with
  `Tls(AlertReceived(42))`, the `bad_certificate` alert a verifying client sends. Then the
  server's `clients` applies: a `Presents` client sends the PEM certificates of `chain`,
  leaf first, which `Request` and `Require` verify against their trust set as the host does
  but at the latest `notBefore` of those certificates, so time is not checked, and
  `Anonymous()` never sees; a `Certificate` client sends none. A `Plain()` client is
  `Tls(HandshakeFailure())`; a `Fails(f)` client is `Tls(f)`. A `Presents` chain that holds
  no well-formed PEM certificate is invalid input to `test.net`. A `Reset()` or `Stall()`
  step at the handshake behaves as for `tls.connect`. Script bytes are plaintext.

Every operation against a scripted peer or listener is recorded as the same request and completion a
real network produces, so a simulated run yields a recording that replays like a real one
([network recording](determinism.md#network-recording)).

## Processes

The process data types are root core types, with variant ordinals in the order written:

```text
ExitStatus    = Exited(i32) | Signaled(i32)
Signal        = Interrupt() | Terminate() | Kill()
ProcessOutput = { status: ExitStatus, stdout: Bytes, stderr: Bytes }
PipelineOutput = { statuses: List[ExitStatus], stdout: Bytes, stderr: Bytes }
GroupOutput   = { statuses: List[ExitStatus], stderr: List[Bytes] }
ProcessError  = NotGranted() | InvalidInput() | NotFound() | PermissionDenied()
              | OutputLimit() | Fault(u32) | TimedOut() | Canceled(CancelCause)
              | InputClosed()
Deadline      = { clock: MonotonicClock, at: Instant of .clock }
Command       = { program: Path, args: List[String], dir: Option[Dir],
                  env: Option[List[(String, String)]] }
ProcessScript = { program: String, args: List[String],
                  outcome: Result[ProcessOutput, ProcessError] }
ChildScript   = { program: String, args: List[String], steps: List[ChildStep] }
ChildStep     = Print(Bytes) | PrintErr(Bytes) | Expect(Bytes) | Exit(ExitStatus)
```

A `Command` describes one child: the program to run, its arguments, its working directory
and its environment. `proc.run(p, cmd, stdin, max_output, deadline)` runs one child to
completion and returns its exit status and everything it wrote to standard output and
standard error. `proc.pipeline(p, stages, stdin, max_output, deadline)` runs commands
connected as a shell pipeline. `proc.spawn(p, cmd)` starts a streaming child that the
program talks to while it runs, and `proc.spawn_pipeline(p, stages, max_stderr)` a streaming
pipeline. Each makes these checks of each command in this order, each before any host
effect; a pipeline checks every stage in stage order before anything is spawned, and an
empty `stages` is `InvalidInput()`:

1. `program` must be a canonical absolute path: it starts with `/`, has no empty, `.` or
   `..` component and no trailing `/`, and no argument contains a NUL byte. With `env`
   `Some(vars)`, every name must be nonempty without `=` or NUL, no value may contain NUL,
   and no name may appear twice. Otherwise `InvalidInput()`.
2. `program` must equal a rule of `p` byte for byte, and with `env` `Some(vars)` every
   name in `vars` must be one of that rule's environment names; otherwise `NotGranted()`.
   No `PATH` search, link resolution or prefix match takes part in the check.
3. With `dir` `Some(d)`, the child's working directory is `d`'s directory; a `Dir` with no
   host directory (one from `test.dir`) is `InvalidInput()` for a host provider. With
   `None()` the working directory is `/`. A `Dir` grants the child nothing; it only names
   where it starts.
Cancellation follows these program checks as specified by
[CONC-3](concurrency.md#conc-3-cancellation). After that, for `proc.run` and
`proc.pipeline` with `Some(d)`, a deadline `d.at` that `d.clock` has already reached is
`TimedOut()`, and nothing is spawned.

The child's argument vector is `program` followed by `args`. With `env` `None()` its
environment holds exactly the variables its rule lists that are set in the launcher's
environment; with `Some(vars)` it holds exactly `vars`, and nothing is inherited. A rule's
environment names bound what a child can see from either source, as an `Env` grant bounds
`env.get`.
A missing executable is `NotFound()`, one the host refuses to execute `PermissionDenied()`,
and any other spawn, stream or wait failure `Fault(code)`, a capability input like other
fault codes. A child that exits has status `Exited(code)`, one ended by a signal
`Signaled(number)`; a nonzero status is an ordinary `Ok`, not an error. A child starts with
the default disposition of SIGINT and SIGTERM, whatever the launcher's. The host provider
signals only the child itself, never processes it started.

**One-shot runs.** `proc.run` writes `stdin` to the child's standard input, which is then
closed; standard output and standard error are captured separately and concurrently, so
neither pipe can stall the other. When either stream exceeds `max_output` bytes the child is
killed and the result is `OutputLimit()`; `max_output` 0 accepts only a child that writes
nothing. With `deadline` `Some(d)` the run must end by `d.at`: when `d.clock` reaches it
first, the child is sent SIGTERM, then SIGKILL if it has not exited one second later, and
once it is reaped the result is `TimedOut()`, with its output discarded. A run that ends
first returns as if it had no deadline, and `None()` sets none. A deadline grants nothing:
its clock is a `MonotonicClock` the program already holds, and no other clock is read. A run's deadline ends
only its child, gracefully, and returns `TimedOut()`; a scope deadline that passes during
a run cancels it like any other cancellation.

**Pipelines.** `proc.pipeline` runs its stages as one run: `stdin` goes to the first
stage, each stage's standard output is connected to the next stage's standard input by a
host pipe the program never touches, and the last stage's standard output is captured as
`stdout`. Every stage writes standard error to one shared captured pipe, as a shell's
stages share a terminal, so `stderr` interleaves them as the host delivers it.
`statuses` holds every stage's `ExitStatus` in stage order (a shell's `PIPESTATUS`); a
stage that exits early can end an earlier one with SIGPIPE, reported as `Signaled(13)`.
`max_output` bounds `stdout` and `stderr`, `OutputLimit()` killing every stage, and a
deadline or cancellation ends every stage as it ends a `proc.run` child. A stage that
cannot start (`NotFound()`, `PermissionDenied()`, `Fault(_)`) kills and reaps the stages
started before it and is the result. `std@1/process.pipefail` reduces the statuses as a
shell's `pipefail` does.

`proc.run` and `proc.pipeline` park their task while the children run, so other tasks keep running
([CONC-1](concurrency.md#conc-1-runtime)). It is a cancellation point throughout: cancelling
its task sends the child SIGKILL at once, reaps it, discards its output and returns
`Canceled(cause)`. The child's own effects are outside Hirsel's model
([CAP-1](capabilities.md#cap-1-authority)), and none of its output reaches the program, so
the run establishes no effect. Without a Tasks provider the run blocks the thread and
nothing can cancel it.

**Streaming children.** `proc.spawn` starts the child with all three standard streams as
pipes and returns it as a `Child of p`: a noncopyable, sendable resource carrying `Proc`
authority, whose operations take the same `Proc`
([CAP-2](capabilities.md#cap-2-charged-at-use)).

- `child.write(p, mut c, data)` writes every byte of `data` to the child's standard input
  and succeeds once all are accepted; empty `data` succeeds with no effect. After
  `child.close_stdin` it is `InvalidInput()`. When the child has closed its end, or exited,
  it is `InputClosed()`, possibly after a prefix was accepted.
- `child.close_stdin(p, mut c)` closes the child's standard input, so the child reads end of
  input; closing again has no effect.
- `child.read_stdout(p, mut c, mut buffer, max)` appends between 1 and `max` bytes of the
  child's standard output to `buffer` and returns their count as soon as any are available,
  or returns 0 at end of stream; it does not wait to fill `max`. `max` 0 is
  `InvalidInput()`, and an error appends nothing. `child.read_stderr` is the same for
  standard error. A stream is a pipe of bounded host capacity: a child that fills one
  nobody reads waits until it is read.
- `child.wait(p, take c, deadline)` closes standard input if it is open, reads and
  discards whatever the child still writes to either stream until the child exits, reaps
  it and returns its `ExitStatus`. With `deadline` `Some(d)` the wait must end by `d.at`,
  as a `proc.run` deadline: when `d.clock` reaches it first (or has already reached it),
  the child is sent SIGTERM, then SIGKILL if it has not exited one second later, and once
  it is reaped the result is `TimedOut()`. It consumes the Child on every outcome.
- `child.kill(p, mut c, signal)` sends the child `signal`: SIGINT for `Interrupt()`,
  SIGTERM for `Terminate()`, SIGKILL for `Kill()`. A later `wait` returns `Signaled(n)`
  with the signal's number (2, 15 or 9) unless the child handled or ignored it or had
  already exited. Signalling an exited child has no effect. Stopping a child gracefully
  within a grace period `g` is `child.kill(p, c, Terminate())` followed by a `wait` whose
  deadline is `g` away.

`child.write`, the reads and `child.wait` park their task until they can proceed. A write or
read is a cancellation point only before it has an effect: a write before any byte is
accepted, a read before any byte is received. A cancelled `child.wait` returns
`Canceled(cause)` and drops the Child it took, which kills the child. `close_stdin` and
`kill` never wait and are not cancellation points.

Dropping a `Child` (one never waited, since `child.wait` takes it) closes its standard
input, sends the child SIGKILL, reaps it, then closes its output pipes, discarding unread
bytes. The finalizer is `@noalloc`, and it never waits on the child except to reap it after
SIGKILL. A program that wants the child's exit status calls `child.wait`.

**Streaming pipelines.** `proc.spawn_pipeline` starts its stages connected as
`proc.pipeline` connects them and returns them as a `ChildGroup of p`, a noncopyable,
sendable resource carrying `Proc` authority like a `Child`. The program streams to the first
stage's standard input and from the last stage's standard output: `group.write`,
`group.close_stdin` and `group.read_stdout` are `child.write`, `child.close_stdin` and
`child.read_stdout` on those ends, parking, cancelling and failing as they do. Each stage
writes standard error to a pipe of its own, which the provider drains while the group runs,
so no stage waits on it however the program streams; a stage that writes more than
`max_stderr` bytes there makes the provider send every stage SIGKILL, and the group's wait is
then `OutputLimit()`. A stage that cannot start (`NotFound()`, `PermissionDenied()`,
`Fault(_)`) kills and reaps the stages started before it and is the result.

- `proc.wait_group(p, take g, deadline)` closes standard input if it is open, reads and
  discards whatever the last stage still writes until every stage has closed its output and
  standard error, reaps every stage and returns `GroupOutput`: `statuses` every stage's
  `ExitStatus` in stage order (a stage that exits early can end an earlier one with
  SIGPIPE, `Signaled(13)`) and `stderr` every stage's standard error, each its own `Bytes`,
  in stage order. With `deadline` `Some(d)` the wait must end by `d.at` as a `child.wait`
  does: when `d.clock` reaches it first, every stage is sent SIGTERM, then SIGKILL if it has
  not exited one second later, and once all are reaped the result is `TimedOut()`. It
  consumes the group on every outcome; a cancelled wait returns `Canceled(cause)` and drops
  the group it took.
- `group.kill(p, mut g, signal)` sends `signal` to every stage, as `child.kill` does to a
  child.

Dropping a `ChildGroup` closes its standard input, sends every stage SIGKILL, reaps them all,
then closes its pipes, discarding unread output and captured standard error. The finalizer
is `@noalloc`.

**Scripted runs.** `test.process(t, runs, children)` returns a fake `Proc` whose rules are
the distinct `program` values of its runs and children, with no environment names. A
script outcome or step that is not valid for a host child (a noncanonical `program`, or an
outcome `Error(Canceled(_))`) is invalid input. A `proc.run` on it makes the same checks,
then takes the first unused run entry whose `program` and `args` equal its own and returns
its `outcome`, with an output longer than `max_output` on either stream returning
`OutputLimit()`. With no such entry the result is `NotFound()`. `stdin`, `dir` and `env` are not
consulted beyond their checks. A scripted run takes no time: its deadline matters only to
the check before spawning, and an outcome `Error(TimedOut())` scripts a timeout. A
`proc.pipeline` on it makes every stage's checks, then each stage takes the first unused
run entry whose `program` and `args` equal its own, all or none: with a stage that has
none the result is `NotFound()` and no entry is taken. The first error outcome in stage
order is the result; otherwise `statuses` are the outcomes' statuses, `stdout` the last
stage's output and `stderr` every stage's standard error concatenated in stage order,
bounded by `max_output` as a run's streams are. Scripted runs do not read their input,
so no stage's output reaches the next.

A `proc.spawn` on it takes the first unused child entry whose `program` and `args` equal
its own (with none, `NotFound()`) and plays its steps in order:

- `Print(b)` and `PrintErr(b)` make `b` available on standard output and standard error,
  and the child moves on at once. A read returns at most `max` bytes of one step and never
  crosses into the next.
- `Expect(b)` waits until the program has written `b`'s length of bytes to standard input,
  which must equal `b`. Written bytes accumulate across writes and are compared at the
  write that completes them; a mismatch fails LANG-9 after reporting
  `test failure: child P expected E but the program wrote W` and a newline through Test
  reporting, where `P` is the program path and `E` and `W` are byte-string literals of the
  expected bytes and of the same number of written bytes. Closing standard input (by
  `close_stdin`, `wait` or drop) at an unmet `Expect` fails LANG-9 the same way, reporting
  `test failure: child P expected E but the program closed its input after writing W`.
- `Exit(status)` ends the child: each stream returns 0 once its available bytes are read, a
  write is `InputClosed()`, and `wait` returns `status`. The end of the steps is
  `Exit(Exited(0))`.

A read of a stream with no available bytes waits while the child is at an `Expect`, as a
host read does, and so can leave a run Blocked ([CONC-8](concurrency.md#conc-8-global-quiescence)).
Writes never wait. `child.kill` with any signal ends the script at once without checking an
`Expect`, and a later `wait` returns `Signaled(n)` with that signal's number. A scripted
child takes no time: a `wait` deadline matters only when `d.clock` has already reached it,
which ends the script the same way and returns `TimedOut()`.

A `proc.spawn_pipeline` on it makes every stage's checks, then each stage takes the first
unused child entry whose `program` and `args` equal its own, all or none: with a stage that
has none the result is `NotFound()` and no entry is taken. The stages play their steps as
spawned children do, connected: the program's writes are the first stage's input, the bytes
every other stage's `Print` steps make available are written to the next stage's input at
once, the end of a stage closes the next stage's input, and `group.read_stdout` reads the
last stage's `Print` steps as `child.read_stdout` does. Each stage's `PrintErr` bytes are
its captured standard error, bounded by `max_stderr`: a stage past it ends every stage as
`group.kill(p, g, Kill())` does, and the wait returns `OutputLimit()`. An `Expect` of a later
stage that its input stage contradicts fails LANG-9 after reporting
`test failure: child P expected E but its input stage Q wrote W`, and one left unmet when its
input stage ends after reporting
`test failure: child P expected E but its input stage Q closed its output after writing W`,
where `Q` is the previous stage's program; the first stage reports as a child does. Such a
failure occurs at the operation that moved the bytes or ended the stage, `proc.spawn_pipeline`
included. `group.kill` with any signal ends every stage at once without checking an `Expect`,
and each stage's status is then `Signaled(n)` unless it had already ended; a wait whose
deadline `d.clock` has already reached ends every stage the same way and returns `TimedOut()`.
Every scripted operation is recorded as the same request
and completion a host child produces, so a simulated run replays like a real one
([process recording](determinism.md#process-recording)).

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
`f64.from_f32` converts an `f32` to the `f64` of the same value, exactly; `f32.from_f64`
rounds an `f64` to nearest `f32`, ties to even, so a finite value beyond `f32`'s range
becomes an infinity and a tiny one a subnormal or signed zero. Both allocate nothing,
preserve signed zero and infinities, have no error case, and give the result type's one
NaN for a NaN.

`char.to_u32` returns a `char`'s Unicode scalar value. `char.from_u32` returns `Some` of
the `char` whose scalar value is its argument, and `None` for a surrogate (0xD800 to
0xDFFF) or a value above 0x10FFFF. Neither allocates or fails. These are the only
conversions between `char` and the integers; the standard library's `char` module builds
the rest on them ([scalars](std-library.md#scalars)).

The library reductions `f32.sum` and `f64.sum` repeatedly add adjacent pairs
left to right, carry an odd tail, and repeat until one value is left; an empty sum is +0.
`sum_exact` rounds the exact real sum once; NaN or opposing infinities give the canonical
NaN, a single infinity sign is preserved, and an exact zero is +0 except for an input of
only negative zeros.

`ScalarView` yields each Unicode scalar value as a `char`; `PositionView` yields each scalar's
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

`map.empty`, `set.empty` and `sorted_map.empty` each return a fresh unique
container. Two constructor calls produce independent owners; SortedMap retains
its existing `of by` brand.

## Map and Set lifecycle

`map.empty` and `set.empty` return fresh unique containers. Length is O(1).
`map.contains` probes keys without copying, retaining or destroying a value,
including move-only resource values. Map and Set removal require unique mutation and
leave missing keys unchanged (`None` or `false`). Map removal returns a unique option, transfers the stored
value with its deep uniqueness and drops its stored key exactly once; Set removal drops its removed value
exactly once. Remaining Map entries keep insertion order, and reinserting a removed
key appends a new position. Removal shifts dense entries and repairs only the
private index, taking expected O(n + index capacity) time without copying payloads.

## SortedMap access

Length is O(1). Get requires copyable V and returns an owned copy `in m`.
Removal requires a unique map and returns a unique option, transferring V with
its deep uniqueness and dropping the stored K exactly once. Missing keys return `None`; remaining entries
stay in comparator order. Lookup and removal use the same fixed binary-search probes
as insertion, completing comparisons before mutation. Comparator failure leaves the
map unchanged. The existing sequence representation gives O(log n) comparisons and
O(n) movement on successful removal; no second ordered container is introduced.

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

`type_shape.info` reads a compile-time `TypeShape[T]` as the `ShapeInfo` data of
[CT-3](compile-time.md#ct-3-shapes); its data types `ShapeInfo`, `ShapeVisibility`,
`ShapeBody`, `FieldShape`, `VariantShape` and `PayloadShape` are root core types with the
`copyable`, `data`, `key`, `inert` and `noalloc_drop` bounds. It runs only during
compile-time evaluation and has no run-time implementation. `codegen.body` builds the
opaque root core type `Body[F]` (bounds `copyable`, `inert` and `noalloc_drop`, no
authority) that [generated bodies](compile-time.md#ct-5-generation) return; like a
`TypeShape` it is compile-time only ([CT-3](compile-time.md#ct-3-shapes)) and has no
run-time implementation.

`codec.encode` and `codec.decode` use the [canonical codec](serialization.md#ser-2-canonical-codec).
`DecodeError` is the nullary variant `Version | SchemaMismatch | Truncated |
Trailing | Scalar | Utf8 | Tag | Length | Duplicate | Order`; these are expected
validation failures with no partially decoded value. Operational refusal is separate.
