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
N fn io.signals(rt: Io) -> Result[Signals, GrantError]
N fn signal.wait(s: Signals, kinds: List[SignalKind]) -> Result[SignalKind, SignalError]
fn random.bytes(r: Random, count: u64) -> Result[Bytes, RandomError]
N fn io.process(rt: Io) -> Result[Proc, GrantError]
fn io.dir(rt: Io, p: Path) -> Result[Dir, FileError]
fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]
fn dir.read_only(d: Dir) -> Result[Dir, FileError]
N fn arg.len(args: Args) -> u64
fn arg.text(args: Args, index: u64) -> Result[String, ArgError]
fn arg.path(args: Args, index: u64) -> Result[Path, ArgError]
fn path.of(text: String) -> Path
@preserves_unique(buf) @unretained(s) fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]
N fn err.print(s: Stderr, text: String) -> Result[(), OutputError]
N fn err.write(s: Stderr, bytes: Bytes) -> Result[(), OutputError]
N fn out.print(s: Stdout, text: String) -> Result[(), OutputError]
N fn out.write(s: Stdout, bytes: Bytes) -> Result[(), OutputError]
fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]
fn env.narrow(e: Env, names: List[String]) -> Result[Env, EnvError]
N fn test.check(t: Test, condition: bool) -> ()
N fn test.check_case(t: Test, ok: bool, case: u64) -> ()
N fn test.fail(t: Test) -> Never
N fn test.seed(t: Test) -> u64
N fn test.tasks(t: Test) -> Tasks
N fn test.clock(t: Test) -> MonotonicClock
N fn test.log(t: Test, text: String) -> ()
N fn test.expect_snapshot(t: Test, name: String, text: String) -> ()
fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir
fn test.dir_event(t: Test, d: Dir, event: WatchEvent) -> ()
fn test.net(t: Test, peers: List[NetPeer], listeners: List[NetListener]) -> Net
fn test.process(t: Test, runs: List[ProcessScript], children: List[ChildScript]) -> Proc
fn test.crash(t: Test, d: Dir, seed: u64) -> ()

// CAP-1/2, DROP-2/3, CONC-1/3, DET-1
N fn io.telemetry(rt: Io) -> Result[Telemetry, GrantError]
fn capture.begin(t: Telemetry, parent: Bytes, spec: Bytes) -> Result[Span of t, TelemetryError]
fn capture.context(t: Telemetry, s: Span of t) -> Bytes
fn capture.event(t: Telemetry, s: Span of t, payload: Bytes) -> Result[(), TelemetryError]
fn capture.finish(t: Telemetry, take s: Span of t, status: Bytes) -> Result[(), TelemetryError]
fn capture.log(t: Telemetry, payload: Bytes) -> Result[(), TelemetryError]
fn capture.measure(t: Telemetry, payload: Bytes) -> Result[(), TelemetryError]

// CAP-1/2, PROV-1/2, DROP-2/4, DET-1
fn file.open(d: Dir, p: Path) -> Result[File of d, FileError]
@preserves_captures(f, buffer) fn file.read(d: Dir, mut f: File of d, mut buffer: List[u8], max: u64) -> Result[u64, FileError]
fn file.seek(d: Dir, mut f: File of d, offset: u64) -> Result[(), FileError]
fn file.create(d: Dir, p: Path) -> Result[File of d, FileError]
fn file.open_append(d: Dir, p: Path) -> Result[File of d, FileError]
fn file.write(d: Dir, mut f: File of d, data: Bytes) -> Result[u64, FileError]
fn file.append(d: Dir, mut f: File of d, data: Bytes) -> Result[(), FileError]
fn file.narrow(d: Dir, take f: File) -> Option[File of d]
fn file.walk(d: Dir) -> Result[Iter[Result[Path, FileError]], FileError]
fn file.size(d: Dir, p: Path) -> Result[u64, FileError]
fn file.read_dir(d: Dir, p: Path) -> Result[Iter[Result[DirEntry, FileError]], FileError]
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
fn file.sync(d: Dir, mut f: File of d) -> Result[(), FileError]
fn file.put_new(d: Dir, p: Path, take data: Bytes, mode: Durability) -> Result[(), FileError]
fn file.map(d: Dir, p: Path) -> Result[Mapped of d, FileError]
fn mapped.with[R](d: Dir, m: Mapped of d, body: fn(view: Bytes) -> R) -> Result[R, FileError]
fn file.watch(d: Dir, p: Path, recursive: bool) -> Result[Watcher of d, FileError]
fn watcher.next(d: Dir, mut w: Watcher of d) -> Result[WatchEvent, FileError]

// CAP-1/2/3, PROV-1/3, DROP-2/3/4, CONC-3/8, DET-1
fn net.narrow(n: Net, rules: List[NetRule]) -> Result[Net, NetError]
fn net.resolve(n: Net, host: String, port: u16) -> Result[List[SocketAddr], NetError]
fn net.connect(n: Net, host: String, port: u16, timeout: NetTimeout) -> Result[Stream of n, NetError]
fn net.connect_addr(n: Net, addr: SocketAddr, timeout: NetTimeout) -> Result[Stream of n, NetError]
@preserves_captures(s, buffer) fn net.read(n: Net, mut s: Stream of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]
@preserves_captures(s) N fn net.read_into[k: size](n: Net, mut s: Stream of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]
@preserves_captures(s) N fn net.write(n: Net, mut s: Stream of n, data: Bytes) -> Result[u64, NetError]
@preserves_captures(s) N fn net.write_all(n: Net, mut s: Stream of n, data: Bytes) -> Result[(), NetError]
@preserves_captures(s) N fn net.shutdown(n: Net, mut s: Stream of n, how: Shutdown) -> Result[(), NetError]
@preserves_captures(s) N fn net.set_read_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]
@preserves_captures(s) N fn net.set_write_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]
N fn net.peer_addr(n: Net, s: Stream of n) -> Result[SocketAddr, NetError]
N fn net.close(n: Net, take s: Stream of n) -> ()
fn net.split(n: Net, take s: Stream of n) -> (ReadHalf of n, WriteHalf of n)
fn net.join(n: Net, take r: ReadHalf of n, take w: WriteHalf of n) -> Result[Stream of n, NetError]
fn read_half.read(n: Net, mut r: ReadHalf of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]
N fn read_half.read_into[k: size](n: Net, mut r: ReadHalf of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]
N fn read_half.set_timeout(n: Net, mut r: ReadHalf of n, timeout: NetTimeout) -> Result[(), NetError]
N fn read_half.close(n: Net, take r: ReadHalf of n) -> ()
N fn write_half.write(n: Net, mut w: WriteHalf of n, data: Bytes) -> Result[u64, NetError]
N fn write_half.write_all(n: Net, mut w: WriteHalf of n, data: Bytes) -> Result[(), NetError]
N fn write_half.shutdown(n: Net, mut w: WriteHalf of n) -> Result[(), NetError]
N fn write_half.set_timeout(n: Net, mut w: WriteHalf of n, timeout: NetTimeout) -> Result[(), NetError]
N fn write_half.close(n: Net, take w: WriteHalf of n) -> ()
fn tls.connect(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots) -> Result[Stream of n, NetError]
fn tls.connect_as(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots, identity: TlsIdentity) -> Result[Stream of n, NetError]
fn tls.connect_with(n: Net, take s: Stream of n, server_name: String, client: TlsClient) -> Result[Stream of n, NetError]
fn tls.accept(n: Net, take s: Stream of n, server: TlsServer) -> Result[Stream of n, NetError]
fn tls.peer(n: Net, s: Stream of n) -> Result[Option[Bytes], NetError]
fn tls.negotiated_protocol(n: Net, s: Stream of n) -> Result[Option[String], NetError]
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
fn proc.narrow(p: Proc, rules: List[ProcessRule]) -> Result[Proc, ProcessError]
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
fn cpu.map_scoped[J: sendable + inert, R: sendable + inert](tasks: Tasks, width: u32, take prepare: sendable once fn() -> List[J] uses {}, take map: sendable fn(take job: J) -> R uses {}) -> Result[List[R], CpuPoolError]
// CPU callbacks require locally visible closures with closed calls. Captures,
// jobs and results contain no capabilities, resources, synchronization handles,
// pointers or opaque function values. Immutable Shared reads and thaw are allowed.
// Free requires io.cpu cpu.scoped@1, width 1/4/8/16 and at most 2*width jobs.
// Preparation runs on worker 0 after all fresh workers are ready. All workers
// join before results return in input order; language failures retain FAIL-2.
// InvalidWidth, NotGranted, Busy, WidthUnavailable, NestedPool, BatchTooLarge,
// WorkerStartFailed, CapacityLost, Canceled, NotSupported and ResourceLimit
// refuse a batch without returning a partial result. Ordered returns NotSupported.
// Physical and batch byte limits conservatively include run heap backing that
// predates entry and result heaps retained after worker exit. Exceeding a live
// allocation cap stops through the existing memory meter. Native replay starts
// no CPU workers and substitutes recorded admission and capacity outcomes.
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
// The exact core scope callback entry supplies a consumable arena grant; ordinary
// borrowed Arena parameters do not. A scope invokes its callback once, so explicit
// taken captures may transfer an outer grant; ordinary captures remain borrowed.
// Seal consumes the designated grant once, transfers retained
// semantic storage (including dead allocations), and preserves nested Shared roots.
// Region is an opaque affine resource with mechanical NoAlloc cleanup. With lends
// only a read-only invocation-bound root; callback/results retain their own effects.
N fn arena.with[R](body: once fn(a: Arena) -> R) -> R
N fn arena.bounded[R](cap: u64, body: once fn(a: Arena) -> R) -> Result[R, OutOfSpace]
fn arena.seal[T](take a: Arena in a, take root: T in root) -> Region[T]
N fn region.with[T, R](mut r: Region[T], body: fn(root: T) -> R) -> R
fn copy_to[T: copyable](a: Arena, x: T) -> T in a
fn shared.freeze[T: inert](take x: unique T) -> Shared[T]
fn shared.thaw[T](take s: Shared[T]) -> Result[unique T, Shared[T]]
// The callback borrows the frozen payload only for this call. Return plain values,
// deep unique copies or copied nested Shared roots. For mutation consume a root
// with thaw and handle both Ok(unique value) and Error(still-frozen root).
// See MEM-11 for checked read/copy/thaw examples; opaque visibility is unchanged.
N fn shared.with[T, R](s: Shared[T], body: fn(v: T) -> R) -> R
fn slots.empty[T]() -> Slots[T]
@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s
N fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s
fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]
N fn list.len[T](xs: List[T]) -> u64
N fn array.len[T](xs: Array[T]) -> u64
@preserves_unique(xs) fn list.push[T](mut xs: List[T], take value: T) -> ()
@preserves_unique(xs) fn list.truncate[T](mut xs: List[T], len: u64) -> () uses {xs}
N fn list.pop[T](mut xs: unique List[T]) -> Option[T]
N fn list.remove[T](mut xs: unique List[T], index: u64) -> T
N fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()
@preserves_unique(xs) fn list.reserve[T](mut xs: List[T], capacity: u64) -> ()
fn iter.from[S, T](take state: S, take next: fn(mut S) -> Option[T]) -> Iter[T] uses {state, next}
fn iter.next[T](mut it: Iter[T]) -> Option[T]

// SEM-1/2/3/5, GEN-2, MEM-8/9
N fn int.widen[I, J](value: I) -> J
N fn int.try_narrow[I, J](value: I) -> Option[J]
N fn int.narrow[I, J](value: I) -> J
N fn int.truncate[I, J](value: I) -> J
N fn int.wrapping_add[I](a: I, b: I) -> I
N fn int.saturating_add[I](a: I, b: I) -> I
N fn int.checked_add[I](a: I, b: I) -> Option[I]
N fn int.wrapping_sub[I](a: I, b: I) -> I
N fn int.saturating_sub[I](a: I, b: I) -> I
N fn int.checked_sub[I](a: I, b: I) -> Option[I]
N fn int.wrapping_mul[I](a: I, b: I) -> I
N fn int.checked_mul[I](a: I, b: I) -> Option[I]
N fn int.leading_zeros[I](value: I) -> I
N fn int.trailing_zeros[I](value: I) -> I
N fn int.count_ones[I](value: I) -> I
N fn int.swap_bytes[I](value: I) -> I
N fn int.reverse_bits[I](value: I) -> I
N fn int.div_floor[I](a: I, b: I) -> I
N fn int.mod[I](a: I, b: I) -> I
N fn int.div_exact[I](a: I, b: I) -> I
N fn int.unchecked_add[I](permit: Unsafe, a: I, b: I) -> I
N fn float.min[F](a: F, b: F) -> F
N fn float.max[F](a: F, b: F) -> F
N fn float.min_number[F](a: F, b: F) -> F
N fn float.max_number[F](a: F, b: F) -> F
N fn float.mul_add[F](a: F, b: F, c: F) -> F
N fn float.sqrt[F](value: F) -> F
N fn float.floor[F](value: F) -> F
N fn float.ceil[F](value: F) -> F
N fn float.trunc[F](value: F) -> F
N fn float.round[F](value: F) -> F
N fn float.abs[F](value: F) -> F
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
fn bytes.base64_encode(data: Bytes, alphabet: Base64Alphabet, padding: Base64Padding) -> unique Bytes uses {}
fn bytes.base64_decode(data: Bytes, alphabet: Base64Alphabet, padding: Base64Padding) -> unique Option[Bytes] uses {}
N fn bytes.sha1(data: Bytes) -> Inline[u8, 20] uses {}
N fn bytes.sha224(data: Bytes) -> Inline[u8, 28] uses {}
N fn bytes.sha256(data: Bytes) -> Inline[u8, 32] uses {}
N fn bytes.sha384(data: Bytes) -> Inline[u8, 48] uses {}
N fn bytes.sha512(data: Bytes) -> Inline[u8, 64] uses {}
N fn bytes.sha512_256(data: Bytes) -> Inline[u8, 32] uses {}
N fn bytes.len(src: Bytes) -> u64
N fn bytes.find_any(src: Bytes, needles: Bytes, start: u64) -> u64
N fn bytes.json_number_end(src: Bytes, start: u64) -> (u64, u64)
N fn bytes.integer(src: Bytes, radix: u8, signed: bool, positive_hi: u64, positive_lo: u64, negative_hi: u64, negative_lo: u64) -> (u64, u64, u64)
N fn bytes.decimal_word(value: u64, mut buffer: Inline[u8, 20]) -> u64
@preserves_unique(builder) @unretained(data) fn bytes.append(mut builder: List[u8], data: Bytes) -> ()
@preserves_unique(builder) @unretained(data) fn bytes.replace_range(mut builder: List[u8], start: u64, data: Bytes) -> ()
@preserves_unique(out) fn bytes.append_from_history(mut out: List[u8], distance: u64, count: u64) -> ()
@preserves_unique(src) fn bytes.from_list(take src: List[u8]) -> Bytes
N fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src
N fn string.as_bytes(s: String) -> Bytes in s
N fn string.position(s: String, offset: u64) -> Option[StrPos of s]
N fn string.byte_offset(s: String, pos: StrPos of s) -> u64
fn path.text(p: Path) -> String
N fn failure.check(condition: bool, rule: CheckRule) -> () uses {}
fn map.empty[K: key, V]() -> unique Map[K, V]
@preserves_unique(m) fn map.insert[K: key, V](mut m: Map[K, V], take k: K, take v: V) -> ()
@preserves_unique(m) @unretained(k, initial, update) fn map.upsert_with[K: key, V](mut m: unique Map[K, V], k: K, initial: fn() -> unique V uses {}, update: fn(mut value: unique V) -> () uses {}) -> ()
@preserves_unique(m) @unretained(k, update) fn map.update_with[K: key, V](mut m: unique Map[K, V], k: K, update: fn(mut value: unique V) -> () uses {}) -> bool
fn map.update_value_with[K: key, V: copyable](mut m: Map[K, V], k: K, change: fn(mut value: V) -> () uses {}) -> bool uses {}
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

// RUN-1..14, CAP-1/2, CONC-2/3/8, DET-1
N fn io.runner(rt: Io, name: String) -> Result[Runner, GrantError]
fn runner.grants(r: Runner) -> GrantSet
fn runner.narrow(r: Runner, keep: GrantSet) -> Result[Runner, GrantError]
fn runner.only(r: Runner, definitions: List[String]) -> Result[Runner, GrantError]
fn runner.entry_path[F](f: Entry[F]) -> String
fn runner.compute[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.call[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]
fn runner.spawn[A: serializable, R: serializable, E: serializable](s: Scope, r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> Task[Result[R, RunError[E]]] of s
fn test.runner(t: Test) -> Runner
```

A core operation that only advances an erased state it takes `mut` (`iter.next`) is
declared at the argument's own captures, so `iter.next(mut it)` and `for x in mut it` over
an inferred iterator need no written type. An annotated `let mut` binding of an erased state
holds its written unknown captures.

## Authority and errors

`Io`, `Args`, `Env`, `Unsafe` and the global projections, `Runner`
([runners](runners.md)) included, are plain capabilities; `Path` is
an inert, plain, immutable path value. `File`, `AtomicFile`, `Mapped` and `Watcher` carry `Dir` authority, `Span` carries `Telemetry`, task
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
type FileError = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory() | DirectoryNotEmpty() | CrossDevice() | Truncated()
InputError    = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()
OutputError   = OutputFault(u32) | Canceled(CancelCause)
ClockError    = Unavailable() | OutOfRange()
WaitError     = Clock(ClockError) | Canceled(CancelCause)
SignalKind    = Interrupt() | Terminate() | Hangup()
SignalError   = Canceled(CancelCause) | NotGranted() | InvalidInput()
RandomError   = Unavailable()
TelemetryError = TooLarge() | InvalidParent()
EnvError      = NotGranted() | InvalidUtf8()
ArgError      = MissingArgument() | InvalidUtf8()
FloatIntError = NotFinite | OutOfRange
OutOfSpace    = OutOfSpace()
```

Every error type in this block is `serializable`
([SER-1](serialization.md#ser-1-serializable-schemas)), as are `FileKind`, `Metadata` and
the network, process and failure errors and outcomes below, so a stored or remote result
can carry them.

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

`dir.narrow(d, p)` returns a `Dir` over `p` beneath `d` with `d`'s access.
`dir.read_only(d)` returns a `Dir` over `d`'s own pinned directory (never the path
re-resolved) whose access is read only, whatever `d`'s is; its narrowings stay read only. Every operation that creates, writes, appends to, commits, renames,
removes or changes the permissions of an entry returns `NotGranted()` for a read-only `Dir`
before cancellation or any host effect, as for a read-only mount. A writable `File` that `file.narrow` moves into a read-only `Dir` keeps its mode,
and writing it through that `Dir` is still `NotGranted()`.

## Filesystem lifecycle

```text
type FileKind = Regular | Directory | Symlink | Other
type Metadata { kind: FileKind, size: u64, modified_unix_ns: Option[i128], permissions: u32 }
type DirEntry { path: Path, kind: FileKind }
type WatchEvent = Created(Path) | Modified(Path) | Removed(Path) | Renamed(Path, Path) | Overflow
```

`DirEntry` is `serializable` like `FileKind` and `Metadata`: a listed path is data, and
reading the entry again still needs the `Dir`.

`file.read_dir` returns a lazy, fused iterator over direct children, including empty
directories and symlinks, sorted by UTF-8 path bytes. Entry paths are relative to
the supplied `Dir`, including the requested directory prefix. The constructor
reads the first bounded page; directory and first-page errors are its outer
`Error`. Later page errors are yielded once after buffered entries, then the
iterator ends permanently. Invalid UTF-8 entry names return `FileFault` on the
scan that encounters them. Dropping the iterator frees its unread pages.

Both `file.read_dir` and `file.walk` use the same fixed internal bound of 128
entries per page. A provider scans lazy directory entries directly into a
bounded sorted selector; it never collects an entire directory. Cursor memory
is O(128 × maximum entry path bytes) for `read_dir`, independent of directory
width, and O(depth × 128 × maximum entry path bytes) for `walk`. The selector,
page conversion and iterator state may hold a constant number of page copies;
one scan buffer, a candidate entry and a continuation name add bounded storage.
Native Path identities are interned for the run, and recordings retain consumed
pages. Those values, application-retained results and the pre-existing simulated
filesystem are outside this bound on cursor storage and unread work.

Each refill rescans the directory after its last name (after its virtual slash
work key for `walk` directories). For width W and C consumed entries in one
directory, this costs O(W × ceil(C/128)) entries examined, plus one terminal scan
when C is an exact multiple of 128; selection costs O(log 128) per candidate.
Pages observe directory contents at each scan, without a whole-directory
snapshot: buffered name/kind pairs keep their observed values. Names inserted
at or behind the last page cursor are skipped; names ahead may be observed on
a later scan. Direct names are yielded at most once and in byte order. Directory
mutation does not make earlier pages rewind. Old eager directory recordings,
including old walk recordings that used them, require re-recording.
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

## Durability

```text
type Durability = Durable() | Volatile()
```

`Durability` is plain data and `serializable`
([SER-1](serialization.md#ser-1-serializable-schemas)).

Other file operations promise nothing across a crash; these two do. `file.sync(d, f)` returns
once `f`'s bytes, and the listing of every directory from the parent of the path `f` was
opened or created at up to its mount's root, are durable: a crash keeps them. A rename is
durable once a sync covers both directories it changed. A read-only `Dir` may sync a file it
opened. `file.put_new(d, p, data, mode)` creates `p` holding exactly `data`, never partial
to a reader: an existing entry at `p` returns `AlreadyExists()`, unless it is a regular file
with equal bytes, which returns `Ok(())` with no other effect; missing parents return
`NotFound()`. With `Durable()` it returns once the file and its directory chain are durable,
as `file.sync` would leave them, the equal-bytes case included. With `Volatile()` a crash may
leave `p` absent, empty or holding a prefix of `data`. A host writes and, for `Durable()`,
synchronizes a hidden temporary in `p`'s directory, links it at `p` without replacement and
removes it; a crash can leave such a temporary behind, never a partial `p`. Both are
recorded effects: the request records the handles, `put_new` its path, every byte and the
mode, and the completion the result.

A simulated directory (`test.dir`) keeps a durable state beside its live tree. Its entries
start durable; writes, creations, renames, removals and directory changes are live only
until a sync covers them. `test.crash(t, d, seed)` crashes the whole tree `d` belongs to: it
becomes its durable state, so unsynced bytes, names, renames and removals are lost, except
that the most recent unsynced write of `n` bytes keeps its first `m mod (n + 1)` bytes,
where `m` is the first `random.splitmix64@1` output from state `seed`, when every earlier
byte of its file is durable. Directories a sync named but never
listed come back empty; permission bits survive with their entry. Every `File` and
`AtomicFile` opened in the tree before the crash is stale: reads, writes, `file.sync` and
`file.commit` return `FileFault(1)` and `file.narrow` returns `None()`; seeking and dropping
still succeed. A `Dir` outside every `test.dir` tree fails LANG-9 with no effect. The crash is
recorded like any other Test operation and replays to the same tree.

### Mapped files

`file.map` maps the regular file at a path read-only and returns a `Mapped of d`, a
noncopyable resource with `Dir` authority whose `@noalloc` finalizer unmaps it. It is a
read under the Dir's ordinary rights and confinement, a cancellation point like
`file.open`, and fails as `file.open` does (`NotFound`, `IsDirectory`, `FileFault` for a
path outside the grant). The mapping's length is the file's length when it is mapped.
Like an opened `File`, a mapping keeps the file it mapped: unlinking or replacing the
path (`file.write_bytes`, `file.commit`) does not change what it reads.

`mapped.with(d, m, body)` calls `body` once with a borrowed `Bytes` view of the mapping's
length, read in place with no copy. The view is the body's borrowed parameter, so MODE-1
keeps it, and any slice of it, inside the body: a body cannot return it, store it or
capture it in an escaping closure, and the view cannot outlive the mapping.
`unique view` copies the bytes out. After the body returns, `mapped.with` checks the file:
`Ok(r)` returns the body's result, and if the file is now shorter than the mapping, or was
found shorter at any earlier check of this mapping, the body's result is dropped and
`mapped.with` returns `Error(Truncated())`. Truncation is sticky: a later regrowth does not
clear it. `mapped.with` is not a cancellation point and returns no other error.

The view shows the file's current bytes. Bytes past the end of a file truncated while it
is mapped (by the program, through `file.create`, or by another process) read as zero,
never as a fault: on a host the runtime answers such a read with a zero page instead of
SIGBUS, and the enclosing `mapped.with` returns `Truncated()`. Bytes another process
changes in place while a body runs are outside DET-1, as other concurrent host
modifications are: the body may see old or new bytes. So are the program's own writes to
the mapped file while a body runs (from the body or another task): a host view shows
them in place and a simulated view keeps the bytes it lent; a truncation still makes
`mapped.with` return `Truncated()`. Writes made before `mapped.with` starts always show.
A `test.crash` makes a mapping of its tree stale, as it does a `File`: every later
`mapped.with` lends a view of zeros and returns `FileFault(1)`.

### Watching files

`file.watch(d, p, recursive)` returns a `Watcher of d`, a noncopyable resource with `Dir`
authority whose `@noalloc` finalizer stops it. It watches the directory or file at `p`:
for a directory, the directory itself and its entries, and with `recursive` every path
below it. It is a read under the Dir's ordinary rights and confinement and a cancellation
point; a missing path is `NotFound`, a path outside the grant `FileFault`.

`watcher.next(d, mut w)` returns the watcher's next change, its paths relative to `d`:
`Created`, `Modified` (a content write), `Removed`, `Renamed(from, to)` within the watch, and
`Overflow()`. A rename into the watch is `Created` there and one out of it `Removed`. An
event equal to the last one still queued is coalesced into it. A watcher queues at most
1024 events; past that the queue ends with one `Overflow()` and drops later events until
the program reads it, after which it should rescan what it watches. With no event queued,
`watcher.next` parks its task, as a stdin read does, until one arrives or the task is
canceled (`Canceled(cause)`); a parked watcher that nothing can deliver to is a CONC-8
wait. Other tasks run meanwhile.

The Dir decides where events come from. A host Dir's watcher reads inotify: the
program's own effects and other processes' alike, in kernel order, including the
temporary file a replacing write publishes by rename. A move is `Renamed` when the kernel
reports both halves in one read and otherwise a removal and a creation; a directory
created or moved into a recursive watch is watched from when its event is read, so
entries made in it before then are not reported. A kernel queue overflow is
`Overflow()`. A simulated tree (a `test.dir` tree or the interpreter's default tree) has
no change source of its own: its watchers receive only what `test.dir_event(t, d, event)`
delivers. That operation hands `event`, its paths relative to `d`, to every watcher of
`d`'s `test.dir` tree that sees it; an event whose path leaves `d`, or one on a Dir that
is not a `test.dir` tree, reaches no watcher. An injected `Overflow()` reaches every
watcher of the tree.

## Network types

`Stream`, `Listener` and `UdpSocket` are noncopyable, sendable resources carrying `Net` authority,
branded by the `Net` that produced them (`Stream of n`, `Listener of n`, `UdpSocket of n`). Their finalizers
are `@noalloc`, close the socket, discard close errors and send no TLS `close_notify`.
`ReadHalf` and `WriteHalf`, the halves `net.split` makes of a `Stream`, are noncopyable,
sendable resources branded like it (`ReadHalf of n`, `WriteHalf of n`); their `@noalloc`
finalizers close the socket only with the second half ([split streams](#network)). The
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
TlsServer    = { identities: List[TlsIdentity], clients: TlsClientAuth, alpn: List[String] }
TlsClient    = { roots: TlsRoots, identity: Option[TlsIdentity], alpn: List[String] }
TlsClientAuth = Anonymous() | Request(Bytes) | Require(Bytes)    // PEM trust set
TlsFailure   = UnknownIssuer() | Expired() | NotYetValid() | NameMismatch() | Revoked()
             | BadCertificate() | AlertReceived(u8) | HandshakeFailure() | PeerMisbehaved()
             | Truncated() | CertificateRequired()
NetError     = NotGranted() | Canceled(CancelCause) | InvalidInput() | UnknownHost()
             | ResolverFailure() | ConnectionRefused() | ConnectionReset()
             | ConnectionAborted() | NotConnected() | AddrInUse() | AddrNotAvailable()
             | HostUnreachable() | NetworkUnreachable() | NetworkDown() | BrokenPipe()
             | TimedOut() | Tls(TlsFailure) | Fault(u32)
```

`IpAddr`, `SocketAddr`, `UdpReceived`, `TlsRoots`, `TlsFailure` and `NetError` are
`serializable`;
the rule and scripted-peer types are not. `Fault` codes, like `FileFault` codes, are capability inputs, not OS error numbers. One error
type serves the whole network stack, so `connect`, `tls.connect`, `tls.accept`, `read` and `write` compose
with plain `?`. Interrupted and would-block conditions never surface: the provider retries
interrupted calls, and every operation blocks.

The scripted-peer types used by `test.net` and the `net.script@2` provider:

```text
NetPeer       = { address: String, port: u16, resolves_to: List[IpAddr],
                  tls: PeerTls, connections: List[NetConnection] }
PeerTls       = Plain() | Certificate(List[String]) | Fails(TlsFailure)
              | Presents(List[String], Bytes) | Serves(TlsServer)
              | Offers(List[String], List[String])
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

`env.get` reads only an exact name its `Env` grants: the entry's `Env` grants every name in
the run configuration's Env object, and `env.narrow(e, names)` returns an `Env` granting
exactly `names` when `e` grants each, and otherwise `NotGranted()`, minting nothing; an
empty `names` is allowed and grants nothing. A narrowed `Env` reads the same snapshot. An
ungranted name returns `NotGranted()`; a name mapped to null returns `Ok(None())`;
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

`io.signals(rt)` projects the run's `Signals` when the run configuration grants
`io.signals` ([CAP-1](capabilities.md#cap-1-authority)); the grant names the kinds the run
observes. `SignalKind` is closed: `Interrupt()` is SIGINT, `Terminate()` SIGTERM and
`Hangup()` SIGHUP; later versions may add kinds, and no other host signal is observable.
From the run's first successful projection until the run ends, an arrival of an observed
kind is **pending** rather than taking the host's default action. At most one arrival of a
kind is pending: a repeated `Terminate` or `Hangup` coalesces with the pending one, but a
second `Interrupt` while one is pending takes the default action, so the process ends by the
signal and a program that never reaches a wait can still be stopped. Arrivals before the
projection, of kinds the grant does not name, and every signal of a run without the grant
keep the default action. Pending arrivals left when the run ends are discarded.

`signal.wait(s, kinds)` returns the earliest pending arrival whose kind is in `kinds`,
which it consumes; arrivals of other kinds stay pending. An empty `kinds` returns
`InvalidInput()` and a kind the grant does not name `NotGranted()`, both before
cancellation; then, in a cancelled scope, it returns `Canceled(cause)`. Otherwise it returns
at once when such an arrival is pending, and suspends its task, without polling, until one
arrives. Waiting has no effect, so cancelling a suspended wait is always allowed. Under
`tasks.cooperative@1` only its task waits, so a program can race a wait against its other
work with `task.select` and let the first to finish cancel the rest. A host provider waits
on the process's signals and is an external event source of unknown completion, so a run
waiting on it is never Blocked ([CONC-8](concurrency.md#conc-8-global-quiescence)). A
scripted provider delivers its arrivals at the run's idle points
([providers](determinism.md#run-configuration)); a run waiting with none left to deliver is
Blocked. A completed wait is recorded with its kind; replay returns it without a signal.

### Telemetry capture

`io.telemetry(rt)` projects the run's `Telemetry` grant, a global plain capability whose
provider is chosen at launch (`telemetry.capture@1` or `telemetry.null@1`,
[providers](determinism.md#run-configuration)). A `Span of t` is a live span: a resource
retaining its Telemetry, with no other authority. Capture accepts an observation; it is not
delivery to any backend, and nothing a provider or exporter does changes what a program
observes. `std@1/telemetry` is the checked interface over these raw operations.

Each payload is at most 65536 bytes; a larger one returns `TooLarge()` before any request,
and so is no scheduling point. `capture.begin(t, parent, spec)` takes the parent as 25 bytes
(a 16-byte trace ID, an 8-byte span ID and a flags byte) followed by an opaque tail: all-zero
IDs begin a new root, two nonzero IDs a child, anything else (or a shorter parent) is
`InvalidParent()`. The provider supplies nonzero identities: a root's trace ID is new to the
run, a child keeps its parent's, and every span ID is new to the run; a provider retries a
zero or colliding draw before it delivers. `spec` is opaque. `capture.context(t, s)` returns
the span's own IDs followed by its parent's flags and tail, a valid parent for the next
span; it issues no request and is no scheduling point.

`capture.event(t, s, payload)`, `capture.log(t, payload)` and `capture.measure(t, payload)`
record an opaque observation. `capture.finish(t, take s, status)` ends the span with an
opaque status; a `TooLarge()` status abandons it. A dropped live span is abandoned: its
noalloc drop records the abandonment and claims no outcome, neither success nor failure
nor cancellation ([DROP-2](failure.md#drop-2-resources)). A parent's end does not end its
children. A span or Telemetry of another identity fails
[CAP-2](capabilities.md#cap-2-charged-at-use) before any effect.

Every accepted request is a scheduling point after its effect, as other capability
operations are ([CONC-1](concurrency.md#conc-1-runtime)). None is a cancellation point
([CONC-3](concurrency.md#conc-3-cancellation)): a cancelled or failing task's shielded
cleanup can still finish its spans and log why. No telemetry operation waits for an
exporter, reads a clock or touches the network.

`err.write` and `out.write` write exactly the supplied bytes, with no newline or
encoding conversion. `err.print` and `out.print` are writes of the String’s UTF-8 bytes
through the same output path. Both forms require the same Stderr or Stdout capability
and share cancellation, failure, recording and replay semantics: recordings retain the
exact bytes, replay re-emits them, and scripted worlds capture them. `Canceled` means no
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
authority. `file.walk` yields regular-file and symlink relative paths in global UTF-8
byte order. Directories are traversal work, including empty directories, and are
not yielded. Symlinks are yielded as entries and never descended; their targets
are not resolved for traversal. Confinement of directory reads is unchanged.
Only the root directory is read when constructing the iterator; further
directories are read as iteration reaches them. Its bounded pages, name-cursor
mutation semantics, memory bound and rescan cost are described under
[filesystem lifecycle](#filesystem-lifecycle). A root read error is the outer `Error`;
a descendant read error is yielded at discovery, after any earlier paths, and
the iterator then ends permanently. Unlike eager traversal, unvisited subtrees
cannot cause an error before their turn or be read after an early stop.

`file.open` opens a read-only `File` at position 0; `file.create` opens an
append-only `File`, creating a missing file and truncating an existing file to zero
bytes. `file.open_append` opens an append-only `File`, creating a missing file without
truncating an existing one (POSIX `O_WRONLY | O_APPEND | O_CREAT`; Go
`os.OpenFile` with the same flags). Every write uses the current end, regardless
of seek. It requires the same write grant as `file.create`, reports the same
`FileError` cases, and promises no durability until `file.sync`.
All three use the same `File` type, with no source mode flags. Providers return
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

The reference simulator records a read request as directory, file and maximum
count, and its completion as the newly delivered bytes (or error). The existing
contents of the caller's append buffer are not a capability input and are not
serialized into each request. This matches the host `FileRead` request. Appending
moves the mutable buffer into its writeback without copying accumulated bytes;
retained read events grow with delivered data, not its cumulative prefixes.

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
([below](#processes)). `test.tasks` returns a `Tasks` for the test's own tasks, channels, mutexes and
atomics, scheduled by the run's scheduler (`inputs.scheduler` in
[determinism.md](determinism.md#run-configuration)); it grants no host authority, and every
call returns an equivalent capability.

`test.clock(t)` returns a fake monotonic clock starting at zero nanoseconds. Repeated
projections share the test's clock; sampling it does not advance time. Only the simulated
scheduler advances it, to the earliest live timer when no task is runnable. It grants no
host clock authority. A blocked test fails with its outstanding waits in the runner report.

## Concurrency

`channel.open` creates an unbounded FIFO. `send` never waits for buffer space; success
transfers the value into the buffer, while `Closed` returns it unchanged. `receive` waits
only while the buffer is empty and senders are live; cancellation returns `Canceled`
without dequeuing. `send` never checks cancellation; `Canceled` is an outcome of `receive`
only (the error type is shared).

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
`Check | Contract | Depth | OutOfSpace | DeadMutex`; all three are `serializable`. The site follows the failing
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
| `net.split(n, take s)`, `net.join(n, take r, take w)` and half operations | the stream's own grant |
| `tls.connect(n, take s, server_name, roots)`, `tls.connect_as(n, take s, server_name, roots, identity)`, `tls.connect_with(n, take s, server_name, client)` | on a `ConnectTls` stream, `server_name` equal to the rule's address |
| `tls.accept(n, take s, server)` | on a stream accepted on a listener of `n` |
| `tls.peer(n, s)`, `tls.negotiated_protocol(n, s)` | the stream's own grant |
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

`net.split` divides a stream into its read half and its write half, each owned and sent on
its own, so one task can read while another writes; it never blocks or fails, and has no
network effect. `read_half.read`, `read_half.read_into` and `read_half.set_timeout` are
`net.read`, `net.read_into` and `net.set_read_timeout` on the stream; `write_half.write`,
`write_half.write_all` and `write_half.set_timeout` are `net.write`, `net.write_all` and
`net.set_write_timeout`, and `write_half.shutdown` is `net.shutdown` with `Write()`. Each
keeps that operation's rules, errors, timeouts and cancellation, including a TLS-only
stream's `NotGranted()` before its handshake; the halves of a TLS stream share its one
session. One half's operation never waits on the other's: a read blocked on the read half
is unaffected by writes, shutdown, timeouts or the drop of the write half, and a blocked
write by the read half. `read_half.close`, `write_half.close` and dropping a half have no
network effect while the other half lives; dropping the second half is dropping the stream,
the same effect as `net.close`, including a scripted client's `Expect` check. `net.join`
consumes both halves: when they are the halves of one stream it returns that stream, with
its timeouts and TLS state; otherwise it returns `InvalidInput()` and drops both, as if each
were dropped. TLS operations, `net.peer_addr` and `net.shutdown` of the read direction take
only a whole stream.

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
(`certificate_required`) or `Tls(AlertReceived(48))` (`unknown_ca`). A TLS 1.3 client
finishes its handshake before the server checks its certificate, so on a host the alert can
instead be the result of the stream's first read or write. The grant checks, the
consumed stream and every other rule are those of `tls.connect`; wherever a rule names
`tls.connect`, it covers `tls.connect_as`.

`tls.connect_with` is `tls.connect` with `client.roots` when `client.identity` is `None()`,
and `tls.connect_as` with `client.roots` and the identity otherwise, that also offers the
ALPN protocols `client.alpn` (RFC 7301), in order; `tls.connect` and `tls.connect_as` offer
none. An ALPN list holding an empty protocol, a protocol over 255 bytes or the same protocol
twice is `InvalidInput()`, checked with the other TLS arguments. The server selects the
first protocol of its own list that the client offers. When the client offers protocols and
the server lists protocols but shares none, the server refuses with `no_application_protocol`,
which ends the client's handshake with `Tls(AlertReceived(120))`; a server that lists none
selects none. Wherever a rule names `tls.connect`, it covers `tls.connect_with`.

`tls.accept` performs a TLS 1.2 or 1.3 server handshake on an accepted stream with `server`.
Each identity's `chain` holds the PEM certificates sent to the client, leaf first, and its
`key` the PEM private key (PKCS#8, PKCS#1 or SEC1; the first key in the bytes) whose public
key is the leaf's. The server presents the first identity whose leaf certificate is valid
for the client's SNI name (a DNS subject alternative name, as a verifying client checks it),
or the first identity when the client sends no SNI or none matches. `clients` decides client
certificates: `Anonymous()` requests none; `Request(trust)` requests one and accepts a
client that sends none; `Require(trust)` requests one and fails a client that sends none
with `Tls(CertificateRequired())`. A certificate the client sends is verified against `trust`
alone (the PEM certificates there are the trust anchors) for client authentication, at the
provider's time, as `tls.connect` verifies a server: an unknown issuer is
`Tls(UnknownIssuer())`, and the other verification failures are the `TlsFailure`s
`tls.connect` reports. An empty identity list, an empty or malformed chain or key, a key
type the provider cannot sign with, a key that does not match its leaf, and an empty or
malformed `trust` are `InvalidInput()`, as is an invalid `alpn` list, checked as
`tls.connect_with` checks one. The server selects the first protocol of `alpn` that the
client offers, and none when either list is empty; a client that offers protocols, none of
them in a nonempty `alpn`, fails the handshake with `Tls(HandshakeFailure())` after the
server sends `no_application_protocol`. The server issues no session tickets. On success
it returns the same stream, after which every `net` stream operation moves plaintext; on
failure the stream is closed. `tls.accept`
or `tls.connect` on an upgraded stream is `InvalidInput()` and consumes and closes it. A
stream accepted on a listener opened under a `ListenTls` rule (and no `Listen` rule, which
`net.listen` prefers) carries no plaintext before a successful `tls.accept`: reads, writes,
shutdown and `tls.connect` return `NotGranted()` with no effect, while timeout setters,
`peer_addr` and `tls.peer` are allowed. `tls.accept` on a stream from `connect` or
`connect_addr` is `NotGranted()`. Truncation and `tls.close` behave as for `tls.connect`
streams.

`tls.peer` returns the certificate chain the stream's peer presented and a successful
handshake verified, on either side: the server's chain on a stream upgraded by
`tls.connect` or `tls.connect_as`, and the client's on a `tls.accept` stream. It is the DER
certificates the peer sent, leaf first, concatenated (each DER certificate delimits
itself), so a program reads the peer's identity as data, and the chain's SHA-256
identifies it; [`std@1/x509`](std-library.md#x509-certificate-fields) reads its subject and
alternative names. It returns `None()` on plain streams and on `tls.accept` streams whose
client sent no certificate. Client-side chain access requires no client identity and also
works with `tls.connect_with`. It reads handshake state only: it never blocks, is never a
cancellation point, and answers the same after the stream's peer closes or resets.

`tls.negotiated_protocol` returns the ALPN protocol the stream's handshake negotiated, on
either side: `Some(p)` exactly when the server selected `p`, and `None()` on plain streams
and when no protocol was selected. Like `tls.peer` it reads handshake state only: it never
blocks, is never a cancellation point, and answers the same after the stream's peer closes
or resets.

`net.listen` binds and listens with a backlog of 128. Host TCP listeners set
`SO_REUSEADDR` before binding, so a stopped server can immediately listen on its port
again while its closed connections remain in `TIME_WAIT`. They do not set
`SO_REUSEPORT`: a second live listener on the same address and port still fails with
`AddrInUse()`. `net.accept` returns a connected stream and its peer address.

The blocking network operations are `resolve`, `connect`, `connect_addr`, `unix_connect`,
`read`, `read_into`, `write`, `write_all`, their half forms (`read_half.read`,
`read_half.read_into`, `write_half.write`, `write_half.write_all`), `accept`,
`tls.connect`, `tls.connect_as`, `tls.connect_with`, `tls.accept` and `tls.close`. Each is a
cancellation point only before it has an effect: a read before any byte is received, a
write before any byte is accepted, a connect or handshake before any packet is sent. The
others never return `Canceled`. A canceled `tls.connect`, `tls.connect_as`,
`tls.connect_with`, `tls.accept` or `tls.close` still consumes its
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
listener's backlog is full. A connect waiting for room in a backlog has queued nothing,
so it stays cancellable while it waits; cooperative schedulers park only the waiting task.
A listen on a path that already exists is `AddrInUse()`, except
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
`(address, port, ConnectTls())` for a `Certificate`, `Fails` or `Serves` peer and
`(address, port, Connect())` for a `Plain` peer, plus `(literal, port, Listen())` for each
listener's address. Duplicate `(address, port)` peers and duplicate listener addresses
are invalid input. A `NetListener` with port 0 binds its clients to the first port-0
listen on that IP; clients target that listener's assigned address. Later port-0
listens on that IP have no scripted clients.

- A peer whose address is a canonical socket path is a Unix peer, with port 0 and no
  `resolves_to`; its `tls` is any peer's, and `tls.connect` on its stream behaves as for a
  TCP peer of that `tls`. `test.net` grants
  `(path, 0, UnixConnect())` for it in place of `Connect`, `ConnectTls` and `UdpPeer` rules. Each `net.unix_connect` to it takes its next
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
- A read at an `Expect` waits for the program's own writes: a split stream's read half,
  read in one task while another task writes through the write half, completes once those
  writes meet the `Expect` and the peer's next `Send` is reached. A read that only the
  program's writes can complete, with no task left to write, never completes
  ([CONC-8](concurrency.md#conc-8-global-quiescence)).
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
  `names`, otherwise `Tls(NameMismatch())`, and the peer presents no certificate bytes and
  selects no ALPN protocol, so `tls.peer` and `tls.negotiated_protocol` are `None()`; to a `Plain()` peer it is `Tls(HandshakeFailure())`; to a
  `Fails(f)` peer it is `Tls(f)`, after the same grant checks and with the same consumed
  stream and recorded events as any handshake failure. `roots` is not consulted, and
  script bytes are plaintext. A `Certificate` peer requests no client certificate, so
  `tls.connect_as` checks its identity and otherwise behaves as `tls.connect`.
- A `Serves(server)` peer is a TLS server: `test.net` validates `server` as `tls.accept`
  does, and an invalid one is invalid input. It first selects an ALPN protocol from the
  client's offer as `tls.accept` does, refusing an offer it shares no protocol with by
  `Tls(AlertReceived(120))`. The client sends `server_name` as SNI when it
  is a DNS name, and none otherwise; the peer presents the identity that SNI selects, as
  `tls.accept` selects one. The client accepts it exactly when its leaf certificate is
  valid for `server_name` (a DNS or IP subject alternative name; time and chain are not
  checked, and `roots` is not consulted), otherwise `Tls(NameMismatch())`. Then the
  server's `clients` applies to the client: `Anonymous()` never sees an identity;
  `Request(trust)` and `Require(trust)` verify the chain of the `tls.connect_as` identity
  against `trust` as the host does but at the latest `notBefore` of its certificates, and
  refuse it with the alert a rustls server sends for that failure, such as
  `Tls(AlertReceived(48))` (`unknown_ca`) for an unknown issuer; `Require` refuses a
  `tls.connect`, which presents none, with `Tls(AlertReceived(116))`
  (`certificate_required`). Every refusal ends `tls.connect` itself, as a TLS 1.2 handshake
  does. On success `tls.peer` returns the presented identity's chain as DER.
- `Presents` and `Offers` describe clients and `Serves` a peer: each is invalid input in the
  other's place. An `Offers(names, alpn)` client is a `Certificate(names)` client that also
  offers the ALPN protocols `alpn`, in order; an invalid list is invalid input to `test.net`.
- `tls.accept` validates `server` as the host does, then handshakes with the stream's
  client by its `tls`. The server first selects an ALPN protocol from an `Offers` client's
  offer as the host does, failing with `Tls(HandshakeFailure())` when it shares none; the
  other clients offer none. A `Certificate(names)`, `Presents(names, chain)` or
  `Offers(names, alpn)` client sends the
  first of `names` as SNI when it is a DNS name, and none otherwise; the server selects its
  identity from that SNI as the host does. The client accepts the selected identity exactly
  when its leaf certificate is valid for a name in `names` (a DNS or IP subject alternative
  name; time and chain are not checked), otherwise the handshake fails with
  `Tls(AlertReceived(42))`, the `bad_certificate` alert a verifying client sends. Then the
  server's `clients` applies: a `Presents` client sends the PEM certificates of `chain`,
  leaf first, which `Request` and `Require` verify against their trust set as the host does
  but at the latest `notBefore` of those certificates, so time is not checked, and
  `Anonymous()` never sees; a `Certificate` or `Offers` client sends none. A `Plain()` client is
  `Tls(HandshakeFailure())`; a `Fails(f)` client is `Tls(f)`. A `Presents` chain that holds
  no well-formed PEM certificate is invalid input to `test.net`. A `Reset()` or `Stall()`
  step at the handshake behaves as for `tls.connect`. Script bytes are plaintext.

Every operation against a scripted peer or listener is recorded as the same request and completion a
real network produces, so a simulated run yields a recording that replays like a real one
([network recording](determinism.md#network-recording)).

## Processes

`std@1/process` also supplies the opaque `ExitCode` returned by a program entry:
`SUCCESS` (0), `FAILURE` (1), and `exit_code(n: u8) -> ExitCode uses {}`. All values
0 through 255 are preserved. It is a pure value; returning it or `Ok(code)` performs
ordinary cleanup. `Error(e)` reports e and exits 1. See [PROG-3](programs.md#prog-3-entry).


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
                  env: Option[List[(String, String)]], own_group: bool }
ProcessScript = { program: String, args: List[String],
                  outcome: Result[ProcessOutput, ProcessError] }
ChildScript   = { program: String, args: List[String], steps: List[ChildStep] }
ProcessRule   = { program: String, env: List[String] }
ChildStep     = Print(Bytes) | PrintErr(Bytes) | Expect(Bytes) | Exit(ExitStatus)
```

`ExitStatus`, `ProcessOutput`, `PipelineOutput` and `ProcessError` are `serializable`;
`Deadline` holds a clock and `Command` a `Dir`, so neither is. A `Command` describes one child: the program to run, its arguments, its working directory,
its environment and whether it runs in a process group of its own. `proc.run(p, cmd, stdin, max_output, deadline)` runs one child to
completion and returns its exit status and everything it wrote to standard output and
standard error. `proc.pipeline(p, stages, stdin, max_output, deadline)` runs commands
connected as a shell pipeline. `proc.spawn(p, cmd)` starts a streaming child that the
program talks to while it runs, and `proc.spawn_pipeline(p, stages, max_stderr)` a streaming
pipeline. Each makes these checks of each command in this order, each before any host
effect; a pipeline checks every stage in stage order before anything is spawned, and an
empty `stages`, or one whose stages' `own_group` differ, is `InvalidInput()` before any
stage's checks:

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

`proc.narrow(p, rules)` returns a `Proc` served by `p`'s provider and world whose rules
are exactly `rules`, merged per program (the union of a program's names), when each rule
names the program of a rule of `p` byte for byte and passes through only names that rule
lists; otherwise it returns `NotGranted()` and mints nothing. An empty `rules` is allowed
and runs nothing. It spawns nothing, is no cancellation point, and its children and their
operations take the narrowed `Proc`.

The child's argument vector is `program` followed by `args`. With `env` `None()` its
environment holds exactly the variables its rule lists that are set in the launcher's
environment; with `Some(vars)` it holds exactly `vars`, and nothing is inherited. A rule's
environment names bound what a child can see from either source, as an `Env` grant bounds
`env.get`.
A missing executable is `NotFound()`, one the host refuses to execute `PermissionDenied()`,
and any other spawn, stream or wait failure `Fault(code)`, a capability input like other
fault codes. A child that exits has status `Exited(code)`, one ended by a signal
`Signaled(number)`; a nonzero status is an ordinary `Ok`, not an error. A child starts with
the default disposition of SIGINT and SIGTERM, whatever the launcher's. With `own_group`
`false` the host provider signals only the child itself, never processes it started.

**Process groups.** With `own_group` `true` the child starts as the leader of a new host
process group, and every stage of a pipeline joins the first stage's group, as a shell's
job does. Every signal the provider sends then goes to each stage, the last first, and then
to the whole group, so it also reaches every process a stage started that is still in the
group: a kill, the SIGTERM and SIGKILL of a deadline, the SIGKILL of an output bound or a
cancellation, and the SIGKILL of a drop. A timed-out group gets its SIGKILL once every
stage has exited or the grace has passed, whichever comes first, so a process a stage
started that ignored SIGTERM ends with its stages. The group's leader is reaped after every
other stage, so the group signalled is always the child's own. A process that leaves the
group (by `setsid` or `setpgid`) is beyond its reach. A wait that ends normally signals
nothing: a process a stage started that closed its pipes may outlive the wait. A child in a
group of its own receives no signal its launcher's terminal sends the launcher's group.
Scripted worlds start no host processes, so there `own_group` has only its check and is
recorded with the command.

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
  child, the last stage first: every stage has its signal before an earlier stage can end,
  so a stage that would exit on end of input is still reported `Signaled(n)` unless it
  handles or ignores the signal. Every signal the provider sends a group's stages (the
  `max_stderr` bound, a deadline, a drop) goes in the same order, and with `own_group` to
  the process group after the stages.

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
Adapters such as `map` are std source over `iter.from`
([lazy iteration](std-library.md#lazy-iteration)). The `file.walk` iterator stops on `Error`; user `iter.from` callbacks decide their own
termination. For terminal-error composition, use the opaque
[`std@1/stream` Flow](std-library.md#terminal-streams). Its capture contracts retain
the inner iterator's complete environment; its mutable `next` operation preserves
that environment through the same advance-only promise as `iter.next`.

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
through allocation. An impossible capacity fails SEM-1 before allocation: logical
`capacity * element_size` must fit `2^62 - 65` bytes (the runtime extent limit
`2^62 - 1` with 64 bytes of sequence metadata/alignment headroom). Native code
also checks the minimum physical stride, including padding/encoding. Zero-sized
elements do not overflow this count check. Host exhaustion is environmental.
Ordinary `mut` copy-on-write applies to `reserve`; `pop`, `remove` and
`swap` require unique storage and allocate nothing. Removed elements belong to the caller
and are never finalized by the operation. `list.truncate` uses ordinary mutable copy-on-write and
keeps the prefix and capacity, and drops the tail in increasing original index
order, including the remaining tail after a failed finalizer, before propagating.
A requested length at or above the current length is a no-op. Truncation of unique storage allocates
no storage itself, but invokes the elements' finalizers with their own effects. Sequential Packed
lists are refused by native `list.truncate` under the existing MEM-13 walk-only
availability boundary.

**The failure primitive.** `type CheckRule = Arithmetic() | TextInvariant() | Bounds()`, and
`failure.check(condition, rule)` evaluates its arguments left to right. True has no effect;
false raises SEM-1, SEM-3 or MEM-14 respectively at that ordinary source call site and
enters the ordinary failure and cleanup path with its complete cause chain. It takes no
arbitrary rule or string, inspects no stack, grants no capability, reads no Test input,
allocates nothing and forwards no caller site. This is how source libraries, the standard
library included, raise language failures; `bytes.replace`'s empty-pattern SEM-3, for
example, is reported at its check site in the std source. A failure during cleanup keeps
FAIL-3 shielding.

**Low-level Base64 leaves.** `Base64Alphabet = Standard() | UrlSafe()` and
`Base64Padding = Required() | Omitted()` are serializable plain configuration
variants; `std@1/base64.Alphabet` and `Padding` alias them. Applications normally
use `std@1/base64.encode/decode`, which retain their String API and typed errors.
The six `bytes.sha*` primitives borrow exactly their byte extent and return
fixed, inline digest bytes without allocation or owner transfer. RustCrypto
`sha1`/`sha2` supplies the algorithm and hardware dispatch. SHA-1/224/256 reject
lengths above 2^61-1 bytes with SEM-1 before reading input. Work charges and
checkpoints are specified under [long pure primitives](determinism.md).

`bytes.base64_encode` borrows its exact byte extent and returns fresh unique
ASCII bytes with the selected RFC4648 alphabet and padding. `bytes.base64_decode`
returns `Some` of fresh unique bytes only for a complete strict success: no
whitespace, other-alphabet symbols, misplaced/missing padding or nonzero unused
bits. Empty input succeeds as `Some(b"")`. `None` exposes no partial output or
error: std reruns its existing quantum parser from offset zero to recover the
precise `DecodeError` and byte offset, including its canonical-bit/padding
precedence. Neither leaf retains the input or changes its owner credits. Runtime
ISA selection is lazy and cached, with a scalar fallback. These are low-level
codec leaves, parallel to `string.from_bytes`, rather than a second diagnostic API.

**Bulk byte append.** `bytes.append(mut builder, data)` appends the complete borrowed
byte interval to the `List[u8]` in order. Empty input is a complete no-op, including arena preparation and charges.
The destination follows ordinary copy-on-write; other holders keep their contents.
It preserves destination uniqueness and introduces no source ownership. Growth is
amortized and the copy is bulk. Tracked arena destinations charge the appended
bytes under MEM-6 before mutation, including detachment of shared storage.

**Initialized byte replacement.** `bytes.replace_range(mut builder, start, data)`
replaces an initialized interval without changing length or capacity. After argument
and mutable-place evaluation, it checks `start <= L`, then `N <= L - start`, where
`L` is the original list length and `N` is the source length. Either failure raises
MEM-14 before replacement, detachment or operation-owned arena charges. A valid
empty source returns without these effects, even when shared or in a full arena.
For nonempty input ordinary COW preserves every existing immutable alias and the
source snapshot, including same-owner overlapping intervals. The copy is overlap
safe; bytes outside the interval remain unchanged. Spare capacity is never an
initialized destination. A shared tracked arena charges exactly `L` bytes before
copying; a writable arena charges nothing. Exhaustion leaves destination and
aliases unchanged. This ordinary MayAllocate operation borrows but does not retain
source provenance, has no capability or new error arm, and uses normal completion,
cleanup and FAIL-3 shielding. See `docs/http-replace-range.md` for implementation
constraints and prior art.

**History byte append.** `bytes.append_from_history(mut out, distance, count)`
appends `count` bytes using forward LZ copying: each appended byte is the byte
`distance` positions before the current end, including bytes appended by this
call. This permits overlapping repetitions without a mutable source alias.
Before mutation it checks `1 <= distance <= list.len(out)` and that the resulting
byte count is at most `2^62 - 17` (the native extent bound minus its 16-byte
metadata prefix);
invalid arguments fail SEM-3, including an invalid distance with zero count.
A valid zero count does not detach or charge storage. Otherwise a bounded arena
charges the appended bytes and any shared-history copy before mutation (MEM-6),
as for `bytes.append`. Existing shared values retain their original contents.
The native implementation reserves once and copies disjoint initialized chunks,
doubling an initialized prefix for an overlapping match.

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
string's bytes. Native physical layout keys identify `String` and `Bytes` as the same
owner/start/length slice, also within containing types. Source type identity and semantic
instance keys still distinguish them. `string.position` returns `Some` exactly at a UTF-8 scalar boundary within
the source, zero and the terminal offset included, and `None` for an offset inside a scalar
or out of range. The brand belongs only to the `StrPos` inside `Some`, never to the `Option`
owner, and `None` carries no source identity. Matching preserves the payload's identity,
constructors cannot fabricate a source brand, and joining `Some` values from different
source owners cannot satisfy one exact owner. `string.byte_offset` extracts the relative
offset and grants no new position. `Path` is an opaque plain core handle; `path.text`
returns ordinary owned UTF-8 text and grants no host authority. `Path` is
[serializable](serialization.md#ser-1-serializable-schemas) as that text, and
decoding constructs it as `path.of` does. Source libraries cannot emit
reinterpretation or other privileged storage operations.

## Numeric and string details

The expected type selects a conversion's destination `J`. `int.widen` requires all of
`I`'s range to fit in `J`; `int.try_narrow` tests representability; `int.narrow` fails
SEM-1 when the value is unrepresentable; and `int.truncate` requires a destination no wider
than the source and keeps the low bits, interpreted with `J`'s signedness. `int.div_floor`
rounds the quotient toward negative infinity, `int.mod` is its remainder with the divisor's
sign, and `int.div_exact` fails SEM-1 for a nonzero remainder; zero divisors and signed
`MIN / -1` fail. `int.wrapping_sub` and `int.wrapping_mul` give the mathematical result
modulo 2^width, read with `I`'s signedness; `int.saturating_sub` gives the nearest bound of
`I` when the difference is outside it (0 for an unsigned type); `int.checked_sub` and
`int.checked_mul` give `None` exactly when the result is outside `I`. As for the `_add` forms,
none fails or allocates. `int.leading_zeros`, `int.trailing_zeros` and `int.count_ones`
count the zero bits above the highest one bit, the zero bits below the lowest one bit, and
the one bits, of the operand's two's complement representation at `I`'s width; a zero
operand has `I`'s width of leading and trailing zeros. `int.swap_bytes` reverses the order of
the operand's bytes (an 8-bit operand is unchanged) and `int.reverse_bits` the order of its
bits. Each result has the operand's type, so a count feeds a shift with no conversion, and
none fails. `int.unchecked_add`'s written aliasing contract is "no memory access; the
mathematical result must fit `I`"; violating it is outside DET-1. `simd.splat` requires
`n > 0`; `simd.add` checks integer lanes in ascending order and cites the call on overflow.

`float.sqrt` is IEEE 754 squareRoot, correctly rounded: `sqrt(-0)` is -0, `sqrt(+inf)` is
+inf, and a NaN or any value below -0 (including -inf) gives the NaN. `float.floor`,
`float.ceil`, `float.trunc` and `float.round` are the IEEE roundToIntegral operations toward
negative infinity, toward positive infinity, toward zero, and to nearest with ties away from
zero. Each result is an integer value of the operand's type, exactly; signed zeros and
infinities pass through, an integral operand is returned unchanged, a result rounded to zero
keeps the operand's sign (`ceil(-0.5)`, `trunc(-0.5)` and `round(-0.4)` are -0), and a NaN gives
the NaN. `float.abs` clears the sign: `abs(-0)` is +0 and `abs(-inf)` is +inf. None of these
allocates or fails, and every result is exactly defined, so each engine gives the same bits.

`float.from_int` converts any sized integer to `f32` or `f64`, rounding to nearest, ties to
even, with no error case. `float.to_i32` truncates toward zero, returning `NotFinite` for NaN
and infinities and `OutOfRange` when the truncated result is outside `i32`.
`f32.bits` and `f64.bits` expose the IEEE
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
key appends a new position. Set also iterates in insertion order: inserting an
existing element keeps its position, removal preserves the order of the remaining
elements, and reinserting a removed element moves it to the end. Removal shifts
dense entries and repairs only the private index, taking expected O(n + index
capacity) time without copying payloads.

## SortedMap access

Length is O(1). Get requires copyable V and returns an owned copy `in m`.
`map.update_value_with` probes before any mutation. A miss returns false without
calling the callback, detaching storage or charging an arena. A hit shallow-detaches
every shared backing on the path to the selected value, preserving keys, cached
hashes, tombstones, length and insertion order, then calls `change` once with
ordinary exclusive `mut V` access. Arena copies are charged before publication;
the value slot is resolved in the detached owner. Writes inside V use ordinary
COW. A saved map copy and aliased values remain unchanged. Successful unit return
yields true; failure keeps prior writes and unwinds with valid owners, without
rollback. V must be copyable. This operation promises neither uniqueness,
no allocation nor the `@unretained` ledger contract. Shared unqualified payloads
copy receipts only; nested qualified receiving contracts still require their
ordinary semantic copies. Lookup keys remain read-only, and OWN-3 still forbids
a live borrowed view of m across the call. `map.update_with` is unchanged.

Removal requires a unique map and returns a unique option, transferring V with
its deep uniqueness and dropping the stored K exactly once. Missing keys return `None`; remaining entries
stay in comparator order. Lookup and removal use the same fixed binary-search probes
as insertion, completing comparisons before mutation. Comparator failure leaves the
map unchanged. The existing sequence representation gives O(log n) comparisons and
O(n) movement on successful removal; no second ordered container is introduced.

## Overview definitions

The [first look](index.md#a-first-look) declares its own `Sku` and `StockError` and uses
the store operations of [storage](storage.md); there are no application fixtures. `Tx`
and `TxFailure` are `std@1/store`'s. The overview's `CliError` is
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
Trailing | Scalar | Utf8 | Tag | Length | Duplicate | Order | Invalid`, itself `serializable`; these are expected
validation failures with no partially decoded value. Operational refusal is separate.

`bytes.find_any(src, needles, start)` returns the first relative byte offset at or after
`start` whose byte occurs in `needles`, or `bytes.len(src)` if there is none. Empty
sets and starts at or beyond the extent have no match. It borrows both extents,
allocates nothing and treats all 256 byte values equally.


The numeric byte leaves back std's existing lossless JSON and scalar APIs.
`bytes.json_number_end` returns the cursor immediately after a strict JSON number
and status 1; on malformed grammar it returns the exact error cursor and status 0.
It never converts/rounds the lexeme. Out-of-extent starts return that start/status 0.
The extra deterministic Steps and fuel charge is 1 plus the distinct source bytes
examined, including a terminating lookahead byte when present; EOF adds no byte.

`bytes.integer` takes radix 2..36, a signed policy and positive/negative u128 limits
split into high/low u64 words. It returns magnitude high/low and status 0 positive,
1 negative, 2 Invalid or 3 OutOfRange (zero magnitude on error). ASCII digits and
letters are accepted up to radix; plus, underscore, whitespace and non-ASCII are
invalid. Minus requires signed=true and at least one digit. Validate through
magnitude overflow: a later invalid digit wins over OutOfRange. Leading zeros
and signed negative zero retain their spelling-independent integer semantics.
It charges 1 plus the full input extent length in both Steps and fuel, even on
invalid radix or malformed input.

`bytes.decimal_word` writes the unsigned u64's shortest decimal prefix into the
caller's twenty-byte inline array, returns its length and preserves the tail.
It allocates nothing and charges 9 extra Steps and fuel (1 plus its eight input
bytes). The std wrapper retains signs, arbitrary-capacity atomic BufferTooSmall,
and its existing public error types. All three work charges precede result
publication, including named runtime callback routing; budget refusal publishes
no primitive result. They add no semantic scheduling/Io events.
