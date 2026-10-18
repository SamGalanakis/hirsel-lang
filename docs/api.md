# Full signature index

Generated from the core registry and std manifest declarations. Do not edit by hand.
Regenerate from the repository root: `hirsel doc --index > docs/api.md`.

Short module names are the spellings programs use; core and std share module heads.
Signatures retain declared modes, brands, attributes and `uses` clauses; whitespace is folded.
Opaque representations stay hidden. Descriptions quote the first sentence of an attached
comment or catalogue row when one exists. Constructor rows also appear in their type.
Core types show their registry bounds and authority; intrinsic storage is not a recursive alias.
Core schema variables `I`/`J` mean sized integers, `F` floats, and `A` integer/float/bool scalars.

812 exported definitions; 254 variants.


## Types and roots

- `@capability(local) opaque type Arena: noalloc_drop uses {Arena}` <!-- core@1/Arena -->
- `type ArgError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = MissingArgument() | InvalidUtf8()` <!-- core@1/ArgError -->
- `InvalidUtf8() -> ArgError` — Variant of ArgError. <!-- core@1/ArgError#InvalidUtf8 -->
- `MissingArgument() -> ArgError` — Variant of ArgError. <!-- core@1/ArgError#MissingArgument -->
- `@capability(global) opaque type Args: plain + copyable + sendable + noalloc_drop uses {Args}` <!-- core@1/Args -->
- `type Array[T] uses {T}` <!-- core@1/Array -->
- `type Atomic[A] uses {Tasks, A}` <!-- core@1/Atomic -->
- `resource type AtomicFile: noalloc_drop uses {Dir} = { dir: Dir, tmp: Path, dst: Path }` <!-- core@1/AtomicFile -->
- `type Bytes: copyable + data + inert + noalloc_drop + key uses {}` <!-- core@1/Bytes -->
- `type CancelCause: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Requested() | Parent() | ChildFailed()` <!-- core@1/CancelCause -->
- `ChildFailed() -> CancelCause` — Variant of CancelCause. <!-- core@1/CancelCause#ChildFailed -->
- `Parent() -> CancelCause` — Variant of CancelCause. <!-- core@1/CancelCause#Parent -->
- `Requested() -> CancelCause` — Variant of CancelCause. <!-- core@1/CancelCause#Requested -->
- `type CancelError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Canceled(CancelCause)` <!-- core@1/CancelError -->
- `Canceled(CancelCause) -> CancelError` — Variant of CancelError. <!-- core@1/CancelError#Canceled -->
- `type ChannelError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Closed() | Canceled(CancelCause)` <!-- core@1/ChannelError -->
- `Canceled(CancelCause) -> ChannelError` — Variant of ChannelError. <!-- core@1/ChannelError#Canceled -->
- `Closed() -> ChannelError` — Variant of ChannelError. <!-- core@1/ChannelError#Closed -->
- `type CheckRule: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Arithmetic() | TextInvariant() | Bounds()` <!-- core@1/CheckRule -->
- `Arithmetic() -> CheckRule` — Variant of CheckRule. <!-- core@1/CheckRule#Arithmetic -->
- `Bounds() -> CheckRule` — Variant of CheckRule. <!-- core@1/CheckRule#Bounds -->
- `TextInvariant() -> CheckRule` — Variant of CheckRule. <!-- core@1/CheckRule#TextInvariant -->
- `opaque resource type Child: sendable + noalloc_drop uses {Proc}` <!-- core@1/Child -->
- `type ChildScript uses {} = { program: String, args: List[String], steps: List[ChildStep] }` <!-- core@1/ChildScript -->
- `type ChildStep uses {} = Print(Bytes) | PrintErr(Bytes) | Expect(Bytes) | Exit(ExitStatus)` <!-- core@1/ChildStep -->
- `Exit(ExitStatus) -> ChildStep` — Variant of ChildStep. <!-- core@1/ChildStep#Exit -->
- `Expect(Bytes) -> ChildStep` — Variant of ChildStep. <!-- core@1/ChildStep#Expect -->
- `Print(Bytes) -> ChildStep` — Variant of ChildStep. <!-- core@1/ChildStep#Print -->
- `PrintErr(Bytes) -> ChildStep` — Variant of ChildStep. <!-- core@1/ChildStep#PrintErr -->
- `type CliError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Usage(ArgError) | Files(FileError) | Output(OutputError)` <!-- core@1/CliError -->
- `Files(FileError) -> CliError` — Variant of CliError. <!-- core@1/CliError#Files -->
- `Output(OutputError) -> CliError` — Variant of CliError. <!-- core@1/CliError#Output -->
- `Usage(ArgError) -> CliError` — Variant of CliError. <!-- core@1/CliError#Usage -->
- `type ClockError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unavailable() | OutOfRange()` <!-- core@1/ClockError -->
- `OutOfRange() -> ClockError` — Variant of ClockError. <!-- core@1/ClockError#OutOfRange -->
- `Unavailable() -> ClockError` — Variant of ClockError. <!-- core@1/ClockError#Unavailable -->
- `@capability(global) opaque type Db: plain + copyable + sendable + noalloc_drop uses {Db}` <!-- core@1/Db -->
- `type Deadline: plain + copyable + sendable + noalloc_drop uses {MonotonicClock} = { clock: MonotonicClock, at: Instant }` <!-- core@1/Deadline -->
- `type DecodeError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Version() | SchemaMismatch() | Truncated() | Trailing() | Scalar() | Utf8() | Tag() | Length() | Duplicate() | Order()` <!-- core@1/DecodeError -->
- `Duplicate() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Duplicate -->
- `Length() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Length -->
- `Order() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Order -->
- `Scalar() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Scalar -->
- `SchemaMismatch() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#SchemaMismatch -->
- `Tag() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Tag -->
- `Trailing() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Trailing -->
- `Truncated() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Truncated -->
- `Utf8() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Utf8 -->
- `Version() -> DecodeError` — Variant of DecodeError. <!-- core@1/DecodeError#Version -->
- `@capability(global) opaque type Dir: plain + copyable + sendable + noalloc_drop uses {Dir}` <!-- core@1/Dir -->
- `type DirEntry uses {} = { path: Path, kind: FileKind }` <!-- core@1/DirEntry -->
- `@capability(global) opaque type Env: plain + copyable + sendable + noalloc_drop uses {Env}` <!-- core@1/Env -->
- `type EnvError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | InvalidUtf8()` <!-- core@1/EnvError -->
- `InvalidUtf8() -> EnvError` — Variant of EnvError. <!-- core@1/EnvError#InvalidUtf8 -->
- `NotGranted() -> EnvError` — Variant of EnvError. <!-- core@1/EnvError#NotGranted -->
- `type Eq[T] uses {} = { equal: fn(T, T) -> bool uses {} }` <!-- core@1/Eq -->
- `type ExitStatus: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Exited(i32) | Signaled(i32)` <!-- core@1/ExitStatus -->
- `Exited(i32) -> ExitStatus` — Variant of ExitStatus. <!-- core@1/ExitStatus#Exited -->
- `Signaled(i32) -> ExitStatus` — Variant of ExitStatus. <!-- core@1/ExitStatus#Signaled -->
- `type Failure uses {} = { kind: FailureKind, rule: String, site: SourceSite, spawn: List[u64], cause: Option[box Failure] }` <!-- core@1/Failure -->
- `type FailureKind: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Check() | Contract() | Depth() | OutOfSpace() | DeadMutex()` <!-- core@1/FailureKind -->
- `Check() -> FailureKind` — Variant of FailureKind. <!-- core@1/FailureKind#Check -->
- `Contract() -> FailureKind` — Variant of FailureKind. <!-- core@1/FailureKind#Contract -->
- `DeadMutex() -> FailureKind` — Variant of FailureKind. <!-- core@1/FailureKind#DeadMutex -->
- `Depth() -> FailureKind` — Variant of FailureKind. <!-- core@1/FailureKind#Depth -->
- `OutOfSpace() -> FailureKind` — Variant of FailureKind. <!-- core@1/FailureKind#OutOfSpace -->
- `opaque resource type File: noalloc_drop uses {Dir}` <!-- core@1/File -->
- `type FileError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory() | DirectoryNotEmpty() | CrossDevice()` <!-- core@1/FileError -->
- `AlreadyExists() -> FileError` — Variant of FileError. <!-- core@1/FileError#AlreadyExists -->
- `Canceled(CancelCause) -> FileError` — Variant of FileError. <!-- core@1/FileError#Canceled -->
- `CrossDevice() -> FileError` — Variant of FileError. <!-- core@1/FileError#CrossDevice -->
- `DirectoryNotEmpty() -> FileError` — Variant of FileError. <!-- core@1/FileError#DirectoryNotEmpty -->
- `FileFault(u32) -> FileError` — Variant of FileError. <!-- core@1/FileError#FileFault -->
- `IsDirectory() -> FileError` — Variant of FileError. <!-- core@1/FileError#IsDirectory -->
- `NotDirectory() -> FileError` — Variant of FileError. <!-- core@1/FileError#NotDirectory -->
- `NotFound() -> FileError` — Variant of FileError. <!-- core@1/FileError#NotFound -->
- `NotGranted() -> FileError` — Variant of FileError. <!-- core@1/FileError#NotGranted -->
- `PermissionDenied() -> FileError` — Variant of FileError. <!-- core@1/FileError#PermissionDenied -->
- `type FileKind: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Regular() | Directory() | Symlink() | Other()` <!-- core@1/FileKind -->
- `Directory() -> FileKind` — Variant of FileKind. <!-- core@1/FileKind#Directory -->
- `Other() -> FileKind` — Variant of FileKind. <!-- core@1/FileKind#Other -->
- `Regular() -> FileKind` — Variant of FileKind. <!-- core@1/FileKind#Regular -->
- `Symlink() -> FileKind` — Variant of FileKind. <!-- core@1/FileKind#Symlink -->
- `type FloatIntError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotFinite() | OutOfRange()` <!-- core@1/FloatIntError -->
- `NotFinite() -> FloatIntError` — Variant of FloatIntError. <!-- core@1/FloatIntError#NotFinite -->
- `OutOfRange() -> FloatIntError` — Variant of FloatIntError. <!-- core@1/FloatIntError#OutOfRange -->
- `type GrantError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted()` <!-- core@1/GrantError -->
- `NotGranted() -> GrantError` — Variant of GrantError. <!-- core@1/GrantError#NotGranted -->
- `type Hasher[T] uses {} = { hash: fn(T, u64) -> u64 uses {} }` <!-- core@1/Hasher -->
- `type Inline[T, n: size] uses {T}` <!-- core@1/Inline -->
- `type InputError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()` <!-- core@1/InputError -->
- `Canceled(CancelCause) -> InputError` — Variant of InputError. <!-- core@1/InputError#Canceled -->
- `InputFault(u32) -> InputError` — Variant of InputError. <!-- core@1/InputError#InputFault -->
- `InvalidInput() -> InputError` — Variant of InputError. <!-- core@1/InputError#InvalidInput -->
- `InvalidUtf8() -> InputError` — Variant of InputError. <!-- core@1/InputError#InvalidUtf8 -->
- `opaque type Instant: copyable + inert + noalloc_drop + plain + sendable uses {}` <!-- core@1/Instant -->
- `@capability(global) opaque type Io: plain + copyable + sendable + noalloc_drop uses {Io}` <!-- core@1/Io -->
- `type IpAddr: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = V4(Inline[u8, 4]) | V6(Inline[u8, 16])` <!-- core@1/IpAddr -->
- `V4(Inline[u8, 4]) -> IpAddr` — Variant of IpAddr. <!-- core@1/IpAddr#V4 -->
- `V6(Inline[u8, 16]) -> IpAddr` — Variant of IpAddr. <!-- core@1/IpAddr#V6 -->
- `type Iter[T]` <!-- core@1/Iter -->
- `type List[T] uses {T}` <!-- core@1/List -->
- `opaque resource type Listener: sendable + noalloc_drop uses {Net}` <!-- core@1/Listener -->
- `type Map[K: key, V] uses {K, V}` <!-- core@1/Map -->
- `type Metadata uses {} = { kind: FileKind, size: u64, modified_unix_ns: Option[i128] }` <!-- core@1/Metadata -->
- `@capability(global) opaque type MonotonicClock: plain + copyable + sendable + noalloc_drop uses {MonotonicClock}` <!-- core@1/MonotonicClock -->
- `type Mutex[T] uses {Tasks, T}` <!-- core@1/Mutex -->
- `@capability(global) opaque type Net: plain + copyable + sendable + noalloc_drop uses {Net}` <!-- core@1/Net -->
- `type NetClient uses {} = { from: SocketAddr, steps: List[NetStep] }` <!-- core@1/NetClient -->
- `type NetConnection uses {} = Refused() | Unanswered() | Open(List[NetStep])` <!-- core@1/NetConnection -->
- `Open(List[NetStep]) -> NetConnection` — Variant of NetConnection. <!-- core@1/NetConnection#Open -->
- `Refused() -> NetConnection` — Variant of NetConnection. <!-- core@1/NetConnection#Refused -->
- `Unanswered() -> NetConnection` — Variant of NetConnection. <!-- core@1/NetConnection#Unanswered -->
- `type NetError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | Canceled(CancelCause) | InvalidInput() | UnknownHost() | ResolverFailure() | ConnectionRefused() | ConnectionReset() | ConnectionAborted() | NotConnected() | AddrInUse() | AddrNotAvailable() | HostUnreachable() | NetworkUnreachable() | NetworkDown() | BrokenPipe() | TimedOut() | Tls(TlsFailure) | Fault(u32)` <!-- core@1/NetError -->
- `AddrInUse() -> NetError` — Variant of NetError. <!-- core@1/NetError#AddrInUse -->
- `AddrNotAvailable() -> NetError` — Variant of NetError. <!-- core@1/NetError#AddrNotAvailable -->
- `BrokenPipe() -> NetError` — Variant of NetError. <!-- core@1/NetError#BrokenPipe -->
- `Canceled(CancelCause) -> NetError` — Variant of NetError. <!-- core@1/NetError#Canceled -->
- `ConnectionAborted() -> NetError` — Variant of NetError. <!-- core@1/NetError#ConnectionAborted -->
- `ConnectionRefused() -> NetError` — Variant of NetError. <!-- core@1/NetError#ConnectionRefused -->
- `ConnectionReset() -> NetError` — Variant of NetError. <!-- core@1/NetError#ConnectionReset -->
- `Fault(u32) -> NetError` — Variant of NetError. <!-- core@1/NetError#Fault -->
- `HostUnreachable() -> NetError` — Variant of NetError. <!-- core@1/NetError#HostUnreachable -->
- `InvalidInput() -> NetError` — Variant of NetError. <!-- core@1/NetError#InvalidInput -->
- `NetworkDown() -> NetError` — Variant of NetError. <!-- core@1/NetError#NetworkDown -->
- `NetworkUnreachable() -> NetError` — Variant of NetError. <!-- core@1/NetError#NetworkUnreachable -->
- `NotConnected() -> NetError` — Variant of NetError. <!-- core@1/NetError#NotConnected -->
- `NotGranted() -> NetError` — Variant of NetError. <!-- core@1/NetError#NotGranted -->
- `ResolverFailure() -> NetError` — Variant of NetError. <!-- core@1/NetError#ResolverFailure -->
- `TimedOut() -> NetError` — Variant of NetError. <!-- core@1/NetError#TimedOut -->
- `Tls(TlsFailure) -> NetError` — Variant of NetError. <!-- core@1/NetError#Tls -->
- `UnknownHost() -> NetError` — Variant of NetError. <!-- core@1/NetError#UnknownHost -->
- `type NetListener uses {} = { address: SocketAddr, clients: List[NetClient] }` <!-- core@1/NetListener -->
- `type NetOperation: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Connect() | ConnectTls() | Listen()` <!-- core@1/NetOperation -->
- `Connect() -> NetOperation` — Variant of NetOperation. <!-- core@1/NetOperation#Connect -->
- `ConnectTls() -> NetOperation` — Variant of NetOperation. <!-- core@1/NetOperation#ConnectTls -->
- `Listen() -> NetOperation` — Variant of NetOperation. <!-- core@1/NetOperation#Listen -->
- `type NetPeer uses {} = { address: String, port: u16, resolves_to: List[IpAddr], tls: PeerTls, connections: List[NetConnection] }` <!-- core@1/NetPeer -->
- `type NetRule uses {} = { address: String, port: u16, operation: NetOperation }` <!-- core@1/NetRule -->
- `type NetStep uses {} = Send(Bytes) | Expect(Bytes) | Stall() | Close() | Reset() | Truncate()` <!-- core@1/NetStep -->
- `Close() -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Close -->
- `Expect(Bytes) -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Expect -->
- `Reset() -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Reset -->
- `Send(Bytes) -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Send -->
- `Stall() -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Stall -->
- `Truncate() -> NetStep` — Variant of NetStep. <!-- core@1/NetStep#Truncate -->
- `type NetTimeout: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unbounded() | Nanos(u64)` <!-- core@1/NetTimeout -->
- `Nanos(u64) -> NetTimeout` — Variant of NetTimeout. <!-- core@1/NetTimeout#Nanos -->
- `Unbounded() -> NetTimeout` — Variant of NetTimeout. <!-- core@1/NetTimeout#Unbounded -->
- `type Option[T] uses {T} = None() | Some(T)` <!-- core@1/Option -->
- `None() -> Option[T]` — Variant of Option. <!-- core@1/Option#None -->
- `Some(T) -> Option[T]` — Variant of Option. <!-- core@1/Option#Some -->
- `type Order[T] uses {} = { compare: fn(T, T) -> i32 uses {} }` <!-- core@1/Order -->
- `type OutOfSpace: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutOfSpace()` <!-- core@1/OutOfSpace -->
- `OutOfSpace() -> OutOfSpace` — Variant of OutOfSpace. <!-- core@1/OutOfSpace#OutOfSpace -->
- `type OutputError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutputFault(u32) | Canceled(CancelCause)` <!-- core@1/OutputError -->
- `Canceled(CancelCause) -> OutputError` — Variant of OutputError. <!-- core@1/OutputError#Canceled -->
- `OutputFault(u32) -> OutputError` — Variant of OutputError. <!-- core@1/OutputError#OutputFault -->
- `type Packed[T] uses {T}` <!-- core@1/Packed -->
- `opaque type Path: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}` <!-- core@1/Path -->
- `type PeerTls uses {} = Plain() | Certificate(List[String]) | Fails(TlsFailure)` <!-- core@1/PeerTls -->
- `Certificate(List[String]) -> PeerTls` — Variant of PeerTls. <!-- core@1/PeerTls#Certificate -->
- `Fails(TlsFailure) -> PeerTls` — Variant of PeerTls. <!-- core@1/PeerTls#Fails -->
- `Plain() -> PeerTls` — Variant of PeerTls. <!-- core@1/PeerTls#Plain -->
- `opaque type PositionView: copyable + inert + noalloc_drop uses {}` <!-- core@1/PositionView -->
- `@capability(global) opaque type Proc: plain + copyable + sendable + noalloc_drop uses {Proc}` <!-- core@1/Proc -->
- `type ProcessError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | InvalidInput() | NotFound() | PermissionDenied() | OutputLimit() | Fault(u32) | TimedOut() | Canceled(CancelCause) | InputClosed()` <!-- core@1/ProcessError -->
- `Canceled(CancelCause) -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#Canceled -->
- `Fault(u32) -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#Fault -->
- `InputClosed() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#InputClosed -->
- `InvalidInput() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#InvalidInput -->
- `NotFound() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#NotFound -->
- `NotGranted() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#NotGranted -->
- `OutputLimit() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#OutputLimit -->
- `PermissionDenied() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#PermissionDenied -->
- `TimedOut() -> ProcessError` — Variant of ProcessError. <!-- core@1/ProcessError#TimedOut -->
- `type ProcessOutput uses {} = { status: ExitStatus, stdout: Bytes, stderr: Bytes }` <!-- core@1/ProcessOutput -->
- `type ProcessScript uses {} = { program: String, args: List[String], outcome: Result[ProcessOutput, ProcessError] }` <!-- core@1/ProcessScript -->
- `type Ptr[T] uses {}` <!-- core@1/Ptr -->
- `@capability(global) opaque type Random: plain + copyable + sendable + noalloc_drop uses {Random}` <!-- core@1/Random -->
- `type RandomError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unavailable()` <!-- core@1/RandomError -->
- `Unavailable() -> RandomError` — Variant of RandomError. <!-- core@1/RandomError#Unavailable -->
- `type Receiver[T] uses {Tasks, T}` <!-- core@1/Receiver -->
- `type Region[T] uses {T}` <!-- core@1/Region -->
- `type Reservation uses {} = { sku: Sku, count: u32 }` <!-- core@1/Reservation -->
- `type Result[T, E] uses {T, E} = Ok(T) | Error(E)` <!-- core@1/Result -->
- `Error(E) -> Result[T, E]` — Variant of Result. <!-- core@1/Result#Error -->
- `Ok(T) -> Result[T, E]` — Variant of Result. <!-- core@1/Result#Ok -->
- `opaque type ScalarView: copyable + inert + noalloc_drop uses {}` <!-- core@1/ScalarView -->
- `@capability(local) opaque type Scope: noalloc_drop uses {Scope}` <!-- core@1/Scope -->
- `type Sender[T] uses {Tasks, T}` <!-- core@1/Sender -->
- `type Set[T: key] uses {T}` <!-- core@1/Set -->
- `type Shared[T] uses {T}` <!-- core@1/Shared -->
- `type Shutdown: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Read() | Write() | Both()` <!-- core@1/Shutdown -->
- `Both() -> Shutdown` — Variant of Shutdown. <!-- core@1/Shutdown#Both -->
- `Read() -> Shutdown` — Variant of Shutdown. <!-- core@1/Shutdown#Read -->
- `Write() -> Shutdown` — Variant of Shutdown. <!-- core@1/Shutdown#Write -->
- `type Simd[A, n: size] uses {A}` <!-- core@1/Simd -->
- `opaque type Sku: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}` <!-- core@1/Sku -->
- `type SlotId[T] uses {}` <!-- core@1/SlotId -->
- `type Slots[T] uses {T}` <!-- core@1/Slots -->
- `type SocketAddr uses {} = { ip: IpAddr, port: u16 }` <!-- core@1/SocketAddr -->
- `type SortedMap[K, V] uses {K, V}` <!-- core@1/SortedMap -->
- `type SourceSite uses {} = { definition: String, line: u64, column: u64 }` <!-- core@1/SourceSite -->
- `@capability(global) opaque type Stderr: plain + copyable + sendable + noalloc_drop uses {Stderr}` <!-- core@1/Stderr -->
- `@capability(global) opaque type Stdin: plain + copyable + sendable + noalloc_drop uses {Stdin}` <!-- core@1/Stdin -->
- `@capability(global) opaque type Stdout: plain + copyable + sendable + noalloc_drop uses {Stdout}` <!-- core@1/Stdout -->
- `type StockError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutOfStock(Sku)` <!-- core@1/StockError -->
- `OutOfStock(Sku) -> StockError` — Variant of StockError. <!-- core@1/StockError#OutOfStock -->
- `opaque type StrPos: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}` <!-- core@1/StrPos -->
- `opaque resource type Stream: sendable + noalloc_drop uses {Net}` <!-- core@1/Stream -->
- `type String: copyable + data + inert + noalloc_drop + key uses {}` <!-- core@1/String -->
- `type Task[T] uses {Scope, T}` <!-- core@1/Task -->
- `@capability(global) opaque type Tasks: plain + copyable + sendable + noalloc_drop uses {Tasks}` <!-- core@1/Tasks -->
- `@capability(local) opaque type Test: noalloc_drop uses {Test}` <!-- core@1/Test -->
- `type TlsFailure: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = UnknownIssuer() | Expired() | NotYetValid() | NameMismatch() | Revoked() | BadCertificate() | AlertReceived(u8) | HandshakeFailure() | PeerMisbehaved() | Truncated()` <!-- core@1/TlsFailure -->
- `AlertReceived(u8) -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#AlertReceived -->
- `BadCertificate() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#BadCertificate -->
- `Expired() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#Expired -->
- `HandshakeFailure() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#HandshakeFailure -->
- `NameMismatch() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#NameMismatch -->
- `NotYetValid() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#NotYetValid -->
- `PeerMisbehaved() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#PeerMisbehaved -->
- `Revoked() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#Revoked -->
- `Truncated() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#Truncated -->
- `UnknownIssuer() -> TlsFailure` — Variant of TlsFailure. <!-- core@1/TlsFailure#UnknownIssuer -->
- `type TlsRoots uses {} = WebPki() | Pem(Bytes)` <!-- core@1/TlsRoots -->
- `Pem(Bytes) -> TlsRoots` — Variant of TlsRoots. <!-- core@1/TlsRoots#Pem -->
- `WebPki() -> TlsRoots` — Variant of TlsRoots. <!-- core@1/TlsRoots#WebPki -->
- `@capability(local) opaque type Tx: noalloc_drop uses {Tx}` <!-- core@1/Tx -->
- `type TxFailure[E] uses {E} = Aborted(E) | Canceled(CancelCause)` <!-- core@1/TxFailure -->
- `Aborted(E) -> TxFailure[E]` — Variant of TxFailure. <!-- core@1/TxFailure#Aborted -->
- `Canceled(CancelCause) -> TxFailure[E]` — Variant of TxFailure. <!-- core@1/TxFailure#Canceled -->
- `@capability(global) opaque type Unsafe: plain + copyable + sendable + noalloc_drop uses {Unsafe}` <!-- core@1/Unsafe -->
- `type WaitError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Clock(ClockError) | Canceled(CancelCause)` <!-- core@1/WaitError -->
- `Canceled(CancelCause) -> WaitError` — Variant of WaitError. <!-- core@1/WaitError#Canceled -->
- `Clock(ClockError) -> WaitError` — Variant of WaitError. <!-- core@1/WaitError#Clock -->
- `@capability(global) opaque type WallClock: plain + copyable + sendable + noalloc_drop uses {WallClock}` <!-- core@1/WallClock -->
- `type bool: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/bool -->
- `type char: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/char -->
- `fn copy_to[T: copyable](a: Arena, x: T) -> T in a` <!-- core@1/copy_to -->
- `type f32: copyable + data + inert + noalloc_drop + plain + sendable uses {}` <!-- core@1/f32 -->
- `type f64: copyable + data + inert + noalloc_drop + plain + sendable uses {}` <!-- core@1/f64 -->
- `@noalloc fn forget[T](take x: T) -> ()` <!-- core@1/forget -->
- `type i128: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/i128 -->
- `type i16: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/i16 -->
- `type i32: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/i32 -->
- `type i64: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/i64 -->
- `type i8: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/i8 -->
- `type u128: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/u128 -->
- `type u16: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/u16 -->
- `type u32: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/u32 -->
- `type u64: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/u64 -->
- `type u8: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}` <!-- core@1/u8 -->

## arena

- `@noalloc fn arena.bounded[R](cap: u64, body: fn(a: Arena) -> R) -> Result[R, OutOfSpace]` <!-- core@1/arena/bounded -->
- `fn arena.seal[T](take a: Arena in a, take root: T in root) -> Region[T]` <!-- core@1/arena/seal -->
- `@noalloc fn arena.with[R](body: fn(a: Arena) -> R) -> R` <!-- core@1/arena/with -->

## arg

- `@noalloc fn arg.count(args: Args) -> u64` <!-- core@1/arg/count -->
- `fn arg.path(args: Args, index: u64) -> Result[Path, ArgError]` <!-- core@1/arg/path -->
- `fn arg.text(args: Args, index: u64) -> Result[String, ArgError]` <!-- core@1/arg/text -->

## array

- `@noalloc fn array.len[T](xs: Array[T]) -> u64` <!-- core@1/array/len -->

## ascii

- `@noalloc pub fn ascii.digit_value(c: u8) -> Option[u8] uses {}` <!-- std@1/ascii/digit_value -->
- `@noalloc pub fn ascii.digit_value_radix(c: u8, radix: u8) -> Option[u8] uses {}` — Zig ascii.charToDigit, extended to an explicit radix. <!-- std@1/ascii/digit_value_radix -->
- `@noalloc pub fn ascii.is_alnum(c: u8) -> bool uses {}` <!-- std@1/ascii/is_alnum -->
- `@noalloc pub fn ascii.is_alpha(c: u8) -> bool uses {}` <!-- std@1/ascii/is_alpha -->
- `@noalloc pub fn ascii.is_digit(c: u8) -> bool uses {}` — Adapted from Zig lib/std/ascii.zig; see third-party/NOTICES.md. <!-- std@1/ascii/is_digit -->
- `@noalloc pub fn ascii.is_lower(c: u8) -> bool uses {}` <!-- std@1/ascii/is_lower -->
- `@noalloc pub fn ascii.is_space(c: u8) -> bool uses {}` <!-- std@1/ascii/is_space -->
- `@noalloc pub fn ascii.is_upper(c: u8) -> bool uses {}` <!-- std@1/ascii/is_upper -->
- `@noalloc pub fn ascii.to_lower(c: u8) -> u8 uses {}` <!-- std@1/ascii/to_lower -->
- `@noalloc pub fn ascii.to_upper(c: u8) -> u8 uses {}` <!-- std@1/ascii/to_upper -->

## atomic

- `fn atomic.create[A](tasks: Tasks, value: A) -> Atomic[A]` <!-- core@1/atomic/create -->
- `@noalloc fn atomic.load[A](tasks: Tasks, a: Atomic[A]) -> A` <!-- core@1/atomic/load -->
- `@noalloc fn atomic.store[A](tasks: Tasks, a: Atomic[A], value: A) -> ()` <!-- core@1/atomic/store -->

## base64

- `pub type base64.Alphabet = Standard | UrlSafe` — Go encoding/base64 Encode/decodeQuantum, with strict RFC4648 input policy. <!-- std@1/base64/Alphabet -->
- `Standard -> base64.Alphabet` — Variant of base64.Alphabet. <!-- std@1/base64/Alphabet#Standard -->
- `UrlSafe -> base64.Alphabet` — Variant of base64.Alphabet. <!-- std@1/base64/Alphabet#UrlSafe -->
- `pub type base64.DecodeError = InvalidByte(u64) | InvalidLength | InvalidPadding(u64) | NonCanonical( u64 )` <!-- std@1/base64/DecodeError -->
- `InvalidByte(u64) -> base64.DecodeError` — Variant of base64.DecodeError. <!-- std@1/base64/DecodeError#InvalidByte -->
- `InvalidLength -> base64.DecodeError` — Variant of base64.DecodeError. <!-- std@1/base64/DecodeError#InvalidLength -->
- `InvalidPadding(u64) -> base64.DecodeError` — Variant of base64.DecodeError. <!-- std@1/base64/DecodeError#InvalidPadding -->
- `NonCanonical( u64 ) -> base64.DecodeError` — Variant of base64.DecodeError. <!-- std@1/base64/DecodeError#NonCanonical -->
- `pub type base64.Padding = Required | Omitted` <!-- std@1/base64/Padding -->
- `Omitted -> base64.Padding` — Variant of base64.Padding. <!-- std@1/base64/Padding#Omitted -->
- `Required -> base64.Padding` — Variant of base64.Padding. <!-- std@1/base64/Padding#Required -->
- `pub fn base64.decode(text: String, alphabet: Alphabet, padding: Padding) -> Result[ unique Bytes, DecodeError ] uses {}` <!-- std@1/base64/decode -->
- `pub fn base64.encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String uses {}` <!-- std@1/base64/encode -->

## bool

- `@noalloc pub fn bool.format[n: size](value: bool, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` <!-- std@1/bool/format -->
- `@preserves_unique(out) pub fn bool.format_into(value: bool, mut out: List[u8]) -> () uses {}` — Appends 'true' or 'false' to 'out'; to_string is that text. <!-- std@1/bool/format_into -->
- `@preserves_unique(out) pub fn bool.format_into_spec( value: bool, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/bool/format_into_spec -->
- `@noalloc pub fn bool.parse(text: String) -> Result[bool, scalar.ParseError] uses {}` <!-- std@1/bool/parse -->
- `pub fn bool.to_string(value: bool) -> unique String uses {}` <!-- std@1/bool/to_string -->

## bufio

- `pub type bufio.ReadError[E] = Source(E) | Limit | UnexpectedEof | InvalidUtf8 | InvalidRead` — Rust alloc/src/io/buffered/bufreader.rs fill_buf/consume and alloctests/tests/io/buffered.rs delimiter/line vectors, f6c1107f. <!-- std@1/bufio/ReadError -->
- `InvalidRead -> bufio.ReadError[E]` — Variant of bufio.ReadError. <!-- std@1/bufio/ReadError#InvalidRead -->
- `InvalidUtf8 -> bufio.ReadError[E]` — Variant of bufio.ReadError. <!-- std@1/bufio/ReadError#InvalidUtf8 -->
- `Limit -> bufio.ReadError[E]` — Variant of bufio.ReadError. <!-- std@1/bufio/ReadError#Limit -->
- `Source(E) -> bufio.ReadError[E]` — Variant of bufio.ReadError. <!-- std@1/bufio/ReadError#Source -->
- `UnexpectedEof -> bufio.ReadError[E]` — Variant of bufio.ReadError. <!-- std@1/bufio/ReadError#UnexpectedEof -->
- `pub opaque type bufio.Reader: copyable + inert + noalloc_drop uses {}` <!-- std@1/bufio/Reader -->
- `pub fn bufio.buffered(r: Reader) -> u64 uses {}` <!-- std@1/bufio/buffered -->
- `pub fn bufio.consume(mut r: Reader, count: u64) -> () uses {}` <!-- std@1/bufio/consume -->
- `pub fn bufio.contents(r: Reader) -> unique Bytes uses {}` <!-- std@1/bufio/contents -->
- `@preserves_unique(out) pub fn bufio.file_read(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], max: u64) -> Result[ u64, ReadError[FileError] ]` <!-- std@1/bufio/file_read -->
- `@preserves_unique(out) pub fn bufio.file_read_exact( d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], count: u64 ) -> Result[(), ReadError[FileError]]` <!-- std@1/bufio/file_read_exact -->
- `pub fn bufio.file_read_line(d: Dir, mut f: File of d, mut r: Reader, limit: u64) -> Result[ unique String, ReadError[FileError] ]` <!-- std@1/bufio/file_read_line -->
- `pub fn bufio.file_read_more(d: Dir, mut f: File of d, mut r: Reader, max: u64) -> Result[ u64, ReadError[FileError] ]` <!-- std@1/bufio/file_read_more -->
- `@preserves_unique(out) pub fn bufio.file_read_until( d: Dir, mut f: File of d, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[FileError]]` <!-- std@1/bufio/file_read_until -->
- `pub fn bufio.new(capacity: u64) -> Reader uses {}` <!-- std@1/bufio/new -->
- `@preserves_unique(out) pub fn bufio.read[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}` <!-- std@1/bufio/read -->
- `@preserves_unique(out) pub fn bufio.read_exact[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], count: u64 ) -> Result[(), ReadError[E]] uses {refill}` <!-- std@1/bufio/read_exact -->
- `pub fn bufio.read_line[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], limit: u64 ) -> Result[unique String, ReadError[E]] uses {refill}` <!-- std@1/bufio/read_line -->
- `pub fn bufio.read_more[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}` <!-- std@1/bufio/read_more -->
- `@preserves_unique(out) pub fn bufio.read_until[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[E]] uses {refill}` <!-- std@1/bufio/read_until -->
- `@preserves_unique(out) pub fn bufio.stream_read( n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], max: u64 ) -> Result[u64, ReadError[NetError]]` <!-- std@1/bufio/stream_read -->
- `@preserves_unique(out) pub fn bufio.stream_read_exact( n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], count: u64 ) -> Result[(), ReadError[NetError]]` <!-- std@1/bufio/stream_read_exact -->
- `pub fn bufio.stream_read_line(n: Net, mut s: Stream of n, mut r: Reader, limit: u64) -> Result[ unique String, ReadError[NetError] ]` <!-- std@1/bufio/stream_read_line -->
- `pub fn bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[ u64, ReadError[NetError] ]` <!-- std@1/bufio/stream_read_more -->
- `@preserves_unique(out) pub fn bufio.stream_read_until( n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[NetError]]` <!-- std@1/bufio/stream_read_until -->

## bytes

- `@preserves_unique(src) fn bytes.from_list(take src: List[u8]) -> Bytes` <!-- core@1/bytes/from_list -->
- `@noalloc fn bytes.len(src: Bytes) -> u64` <!-- core@1/bytes/len -->
- `pub fn bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> unique Bytes in src` — Left-to-right nonoverlapping byte replacement, including arbitrary UTF-8 errors. <!-- std@1/bytes/replace -->
- `@noalloc fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src` <!-- core@1/bytes/slice -->

## channel

- `fn channel.open[T: sendable](tasks: Tasks) -> (Sender[T], Receiver[T])` <!-- core@1/channel/open -->
- `fn channel.receive[T](tasks: Tasks, mut r: Receiver[T]) -> Result[T, ChannelError]` <!-- core@1/channel/receive -->
- `fn channel.send[T: sendable](tasks: Tasks, mut s: Sender[T], take v: T) -> Result[(), (T, ChannelError)]` <!-- core@1/channel/send -->

## char

- `@preserves_unique(out) pub fn char.format_into(value: char, mut out: List[u8]) -> () uses {}` — Appends the UTF-8 encoding of 'value', one to four bytes; to_string is that text. <!-- std@1/char/format_into -->
- `@noalloc pub fn char.from_ascii(byte: u8) -> Option[char] uses {}` — The char of an ASCII byte; None for a byte above 0x7F. <!-- std@1/char/from_ascii -->
- `@noalloc fn char.from_u32(value: u32) -> Option[char]` <!-- core@1/char/from_u32 -->
- `@noalloc pub fn char.from_u8(byte: u8) -> char uses {}` — The char whose scalar value is 'byte', U+0000 to U+00FF, as Rust's char::from(u8). <!-- std@1/char/from_u8 -->
- `pub fn char.to_string(value: char) -> unique String uses {}` <!-- std@1/char/to_string -->
- `@noalloc fn char.to_u32(c: char) -> u32` <!-- core@1/char/to_u32 -->

## child

- `@noalloc fn child.close_stdin(p: Proc, mut c: Child of p) -> ()` <!-- core@1/child/close_stdin -->
- `@noalloc fn child.kill(p: Proc, mut c: Child of p) -> ()` <!-- core@1/child/kill -->
- `fn child.read_stderr(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]` <!-- core@1/child/read_stderr -->
- `fn child.read_stdout(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]` <!-- core@1/child/read_stdout -->
- `@noalloc fn child.wait(p: Proc, take c: Child of p) -> Result[ExitStatus, ProcessError]` <!-- core@1/child/wait -->
- `@noalloc fn child.write(p: Proc, mut c: Child of p, data: Bytes) -> Result[(), ProcessError]` <!-- core@1/child/write -->

## cli

- `pub type cli.Flag { name: String, short: Option[u8], kind: Kind, help: String, }` <!-- std@1/cli/Flag -->
- `pub type cli.Kind = Switch | Value` — Go flag.FlagSet.parseOne token handling, with immutable explicit specs. <!-- std@1/cli/Kind -->
- `Switch -> cli.Kind` — Variant of cli.Kind. <!-- std@1/cli/Kind#Switch -->
- `Value -> cli.Kind` — Variant of cli.Kind. <!-- std@1/cli/Kind#Value -->
- `pub type cli.ParseError = UnknownFlag(u64) | MissingValue(u64) | UnexpectedValue(u64) | InvalidSpec` <!-- std@1/cli/ParseError -->
- `InvalidSpec -> cli.ParseError` — Variant of cli.ParseError. <!-- std@1/cli/ParseError#InvalidSpec -->
- `MissingValue(u64) -> cli.ParseError` — Variant of cli.ParseError. <!-- std@1/cli/ParseError#MissingValue -->
- `UnexpectedValue(u64) -> cli.ParseError` — Variant of cli.ParseError. <!-- std@1/cli/ParseError#UnexpectedValue -->
- `UnknownFlag(u64) -> cli.ParseError` — Variant of cli.ParseError. <!-- std@1/cli/ParseError#UnknownFlag -->
- `pub type cli.Parsed { flags: Map[String, List[String]], positional: List[String], }` <!-- std@1/cli/Parsed -->
- `pub fn cli.parse(take args: List[String], specs: List[Flag]) -> Result[Parsed, ParseError] uses {}` <!-- std@1/cli/parse -->
- `pub fn cli.usage(program: String, specs: List[Flag]) -> String uses {}` <!-- std@1/cli/usage -->

## clock

- `@noalloc fn clock.monotonic(clock: MonotonicClock) -> Result[Instant of clock, ClockError]` <!-- core@1/clock/monotonic -->
- `@noalloc fn clock.sleep_until(clock: MonotonicClock, deadline: Instant of clock) -> Result[(), WaitError]` <!-- core@1/clock/sleep_until -->
- `@noalloc fn clock.wall(clock: WallClock) -> Result[i64, ClockError]` <!-- core@1/clock/wall -->

## codec

- `fn codec.decode[T: serializable](data: Bytes) -> Result[T, DecodeError]` <!-- core@1/codec/decode -->
- `fn codec.encode[T: serializable](value: T) -> Bytes` <!-- core@1/codec/encode -->

## csv

- `pub type csv.Config { delimiter: u8, fields_per_record: Option[u64], max_record_bytes: u64, }` — Go encoding/csv reader/writer state transitions, adapted to bounded chunks. <!-- std@1/csv/Config -->
- `pub type csv.CsvError = Syntax(u64) | UnequalFields(u64) | RecordTooLarge | InvalidUtf8 | InvalidDelimiter` <!-- std@1/csv/CsvError -->
- `InvalidDelimiter -> csv.CsvError` — Variant of csv.CsvError. <!-- std@1/csv/CsvError#InvalidDelimiter -->
- `InvalidUtf8 -> csv.CsvError` — Variant of csv.CsvError. <!-- std@1/csv/CsvError#InvalidUtf8 -->
- `RecordTooLarge -> csv.CsvError` — Variant of csv.CsvError. <!-- std@1/csv/CsvError#RecordTooLarge -->
- `Syntax(u64) -> csv.CsvError` — Variant of csv.CsvError. <!-- std@1/csv/CsvError#Syntax -->
- `UnequalFields(u64) -> csv.CsvError` — Variant of csv.CsvError. <!-- std@1/csv/CsvError#UnequalFields -->
- `pub type csv.ReadError[E] = Source(E) | Csv(CsvError) | InvalidRead` <!-- std@1/csv/ReadError -->
- `Csv(CsvError) -> csv.ReadError[E]` — Variant of csv.ReadError. <!-- std@1/csv/ReadError#Csv -->
- `InvalidRead -> csv.ReadError[E]` — Variant of csv.ReadError. <!-- std@1/csv/ReadError#InvalidRead -->
- `Source(E) -> csv.ReadError[E]` — Variant of csv.ReadError. <!-- std@1/csv/ReadError#Source -->
- `pub opaque type csv.Reader: copyable + inert + noalloc_drop uses {}` <!-- std@1/csv/Reader -->
- `pub fn csv.default_config() -> Config uses {}` <!-- std@1/csv/default_config -->
- `pub fn csv.feed(mut r: Reader, chunk: Bytes, eof: bool) -> Result[ List[List[String]], CsvError ] uses {}` <!-- std@1/csv/feed -->
- `pub fn csv.parse(take text: String, config: Config) -> Result[List[List[String]], CsvError] uses {}` <!-- std@1/csv/parse -->
- `pub fn csv.read_record[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E] ) -> Result[Option[List[String]], ReadError[E]]` <!-- std@1/csv/read_record -->
- `pub fn csv.reader(config: Config) -> Result[Reader, CsvError] uses {}` <!-- std@1/csv/reader -->
- `pub fn csv.write_record(fields: List[String], delimiter: u8, mut out: List[u8]) -> Result[ (), CsvError ] uses {}` <!-- std@1/csv/write_record -->

## deque

- `pub opaque type deque.Deque[T]: () uses {T}` — Endpoint semantics and owned-item vectors from Rust VecDeque and Zig deque. <!-- std@1/deque/Deque -->
- `pub fn deque.empty[T]() -> unique Deque[T] uses {}` <!-- std@1/deque/empty -->
- `@noalloc pub fn deque.len[T](q: Deque[T]) -> u64 uses {}` <!-- std@1/deque/len -->
- `pub fn deque.pop_back[T](mut q: unique Deque[T]) -> unique Option[T] uses {q}` <!-- std@1/deque/pop_back -->
- `pub fn deque.pop_front[T](mut q: unique Deque[T]) -> unique Option[T] uses {q}` <!-- std@1/deque/pop_front -->
- `pub fn deque.push_back[T](mut q: unique Deque[T], take value: unique T) -> () uses {q, value}` <!-- std@1/deque/push_back -->
- `pub fn deque.push_front[T](mut q: unique Deque[T], take value: unique T) -> () uses {q, value}` <!-- std@1/deque/push_front -->

## dir

- `fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]` <!-- core@1/dir/narrow -->

## duration

- `pub fn duration.format(value: time.Duration) -> unique String uses {}` — Adapted decimal fraction emission from Rust library/core/src/time.rs Debug::fmt_decimal. <!-- std@1/duration/format -->
- `@noalloc pub fn duration.from_parts(seconds: i64, nanoseconds: i32) -> time.Duration uses {}` — Adapted unit composition from Rust library/core/src/time.rs Duration::new/as_nanos. <!-- std@1/duration/from_parts -->
- `@noalloc pub fn duration.parse(text: String) -> Result[time.Duration, scalar.ParseError] uses {}` — Contract-specific parser: neither referenced Zig nor Rust std has signed seconds-text parsing. <!-- std@1/duration/parse -->

## env

- `fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]` <!-- core@1/env/get -->

## err

- `@noalloc fn err.print(s: Stderr, text: String) -> Result[(), OutputError]` <!-- core@1/err/print -->

## f32

- `@noalloc fn f32.bits(value: f32) -> u32` <!-- core@1/f32/bits -->
- `@noalloc pub fn f32.ceil(value: f32) -> f32 uses {}` <!-- std@1/f32/ceil -->
- `@noalloc pub fn f32.floor(value: f32) -> f32 uses {}` <!-- std@1/f32/floor -->
- `@noalloc pub fn f32.format[n: size](value: f32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Floating text is an ordinary source algorithm over the four IEEE primitives. <!-- std@1/f32/format -->
- `@preserves_unique(out) pub fn f32.format_into(value: f32, mut out: List[u8]) -> () uses {}` — Appends the shortest round-trip text of 'value' to 'out'; to_string is that text. <!-- std@1/f32/format_into -->
- `@preserves_unique(out) pub fn f32.format_into_spec( value: f32, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/f32/format_into_spec -->
- `@noalloc fn f32.from_bits(value: u32) -> f32` <!-- core@1/f32/from_bits -->
- `@noalloc pub fn f32.parse(text: String) -> Result[f32, scalar.ParseError] uses {}` <!-- std@1/f32/parse -->
- `@noalloc pub fn f32.round(value: f32) -> f32 uses {}` <!-- std@1/f32/round -->
- `@noalloc pub fn f32.sqrt(value: f32) -> f32 uses {}` <!-- std@1/f32/sqrt -->
- `@noalloc pub fn f32.sum(xs: Array[f32]) -> f32 uses {}` <!-- std@1/f32/sum -->
- `pub fn f32.sum_exact(xs: Array[f32]) -> f32 uses {}` <!-- std@1/f32/sum_exact -->
- `pub fn f32.to_string(value: f32) -> unique String uses {}` <!-- std@1/f32/to_string -->

## f64

- `@noalloc fn f64.bits(value: f64) -> u64` <!-- core@1/f64/bits -->
- `@noalloc pub fn f64.ceil(value: f64) -> f64 uses {}` <!-- std@1/f64/ceil -->
- `@noalloc pub fn f64.floor(value: f64) -> f64 uses {}` <!-- std@1/f64/floor -->
- `@noalloc pub fn f64.format[n: size](value: f64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Floating text is an ordinary source algorithm over the four IEEE primitives. <!-- std@1/f64/format -->
- `@preserves_unique(out) pub fn f64.format_into(value: f64, mut out: List[u8]) -> () uses {}` — Appends the shortest round-trip text of 'value' to 'out'; to_string is that text. <!-- std@1/f64/format_into -->
- `@preserves_unique(out) pub fn f64.format_into_spec( value: f64, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/f64/format_into_spec -->
- `@noalloc fn f64.from_bits(value: u64) -> f64` <!-- core@1/f64/from_bits -->
- `@noalloc pub fn f64.parse(text: String) -> Result[f64, scalar.ParseError] uses {}` <!-- std@1/f64/parse -->
- `@noalloc pub fn f64.round(value: f64) -> f64 uses {}` <!-- std@1/f64/round -->
- `@noalloc pub fn f64.sqrt(value: f64) -> f64 uses {}` <!-- std@1/f64/sqrt -->
- `@noalloc pub fn f64.sum(xs: Array[f64]) -> f64 uses {}` <!-- std@1/f64/sum -->
- `pub fn f64.sum_exact(xs: Array[f64]) -> f64 uses {}` <!-- std@1/f64/sum_exact -->
- `pub fn f64.to_string(value: f64) -> unique String uses {}` <!-- std@1/f64/to_string -->

## failure

- `@noalloc fn failure.check(condition: bool, rule: CheckRule) -> () uses {}` <!-- core@1/failure/check -->

## file

- `fn file.append(d: Dir, mut f: File of d, data: Bytes) -> Result[(), FileError]` <!-- core@1/file/append -->
- `fn file.commit(d: Dir, take f: AtomicFile of d) -> Result[(), FileError]` <!-- core@1/file/commit -->
- `fn file.create(d: Dir, p: Path) -> Result[File of d, FileError]` <!-- core@1/file/create -->
- `fn file.create_atomic(d: Dir, p: Path) -> Result[AtomicFile of d, FileError]` <!-- core@1/file/create_atomic -->
- `fn file.metadata(d: Dir, p: Path) -> Result[Metadata, FileError]` <!-- core@1/file/metadata -->
- `fn file.mkdir(d: Dir, p: Path) -> Result[(), FileError]` <!-- core@1/file/mkdir -->
- `pub fn file.mkdir_all(d: Dir, p: Path) -> Result[(), FileError]` — Adapted from Zig Io.Dir.createDirPath and Rust fs::create_dir_all (MIT option; third-party/NOTICES.md). <!-- std@1/file/mkdir_all -->
- `fn file.narrow(d: Dir, take f: File) -> Option[File of d]` <!-- core@1/file/narrow -->
- `fn file.open(d: Dir, p: Path) -> Result[File of d, FileError]` <!-- core@1/file/open -->
- `fn file.read(d: Dir, mut f: File of d, mut buffer: List[u8], max: u64) -> Result[u64, FileError]` <!-- core@1/file/read -->
- `fn file.read_bytes(d: Dir, p: Path) -> Result[Bytes, FileError] in d` <!-- core@1/file/read_bytes -->
- `fn file.read_dir(d: Dir, p: Path) -> Result[List[DirEntry], FileError]` <!-- core@1/file/read_dir -->
- `@noalloc fn file.remove(d: Dir, p: Path) -> Result[(), FileError]` <!-- core@1/file/remove -->
- `fn file.remove_dir(d: Dir, p: Path) -> Result[(), FileError]` <!-- core@1/file/remove_dir -->
- `fn file.rename(d: Dir, from: Path, to: Path) -> Result[(), FileError]` <!-- core@1/file/rename -->
- `fn file.seek(d: Dir, mut f: File of d, offset: u64) -> Result[(), FileError]` <!-- core@1/file/seek -->
- `fn file.size(d: Dir, p: Path) -> Result[u64, FileError]` <!-- core@1/file/size -->
- `fn file.walk(d: Dir) -> Result[Iter[Result[Path, FileError]], FileError]` <!-- core@1/file/walk -->
- `fn file.write(d: Dir, mut f: File of d, data: Bytes) -> Result[u64, FileError]` <!-- core@1/file/write -->
- `fn file.write_all(d: Dir, mut f: AtomicFile of d, data: Bytes) -> Result[(), FileError]` <!-- core@1/file/write_all -->
- `fn file.write_bytes(d: Dir, p: Path, take data: Bytes) -> Result[(), FileError]` <!-- core@1/file/write_bytes -->

## float

- `@noalloc fn float.from_int[I, F](value: I) -> F` <!-- core@1/float/from_int -->
- `@noalloc fn float.max[F](a: F, b: F) -> F` <!-- core@1/float/max -->
- `@noalloc fn float.max_number[F](a: F, b: F) -> F` <!-- core@1/float/max_number -->
- `@noalloc fn float.min[F](a: F, b: F) -> F` <!-- core@1/float/min -->
- `@noalloc fn float.min_number[F](a: F, b: F) -> F` <!-- core@1/float/min_number -->
- `@noalloc fn float.mul_add[F](a: F, b: F, c: F) -> F` <!-- core@1/float/mul_add -->
- `@noalloc fn float.to_i32[F](value: F) -> Result[i32, FloatIntError]` <!-- core@1/float/to_i32 -->
- `@noalloc fn float.total_order[F](a: F, b: F) -> i32` <!-- core@1/float/total_order -->

## glob

- `pub type glob.ExpandError = InvalidPattern(PatternError) | Files(FileError)` <!-- std@1/glob/ExpandError -->
- `Files(FileError) -> glob.ExpandError` — Variant of glob.ExpandError. <!-- std@1/glob/ExpandError#Files -->
- `InvalidPattern(PatternError) -> glob.ExpandError` — Variant of glob.ExpandError. <!-- std@1/glob/ExpandError#InvalidPattern -->
- `pub opaque type glob.Pattern: copyable + inert + noalloc_drop uses {}` <!-- std@1/glob/Pattern -->
- `pub type glob.PatternError = Syntax(u64)` — Slash grammar and class parsing adapted from Go path/match.go. <!-- std@1/glob/PatternError -->
- `Syntax(u64) -> glob.PatternError` — Variant of glob.PatternError. <!-- std@1/glob/PatternError#Syntax -->
- `pub fn glob.compile(pattern: String) -> Result[Pattern, PatternError] uses {}` <!-- std@1/glob/compile -->
- `pub fn glob.files(d: Dir, pattern: String) -> Result[unique List[Path], ExpandError]` <!-- std@1/glob/files -->
- `pub fn glob.matches(pattern: Pattern, path: String) -> bool uses {}` — Two DP rows: O(pattern scalars * path scalars), O(pattern scalars) memory. <!-- std@1/glob/matches -->

## hex

- `pub type hex.DecodeError = InvalidDigit(u64) | OddLength` — Go encoding/hex Encode/Decode, adapted under BSD-3-Clause; see NOTICES. <!-- std@1/hex/DecodeError -->
- `InvalidDigit(u64) -> hex.DecodeError` — Variant of hex.DecodeError. <!-- std@1/hex/DecodeError#InvalidDigit -->
- `OddLength -> hex.DecodeError` — Variant of hex.DecodeError. <!-- std@1/hex/DecodeError#OddLength -->
- `pub fn hex.decode(text: String) -> Result[unique Bytes, DecodeError] uses {}` <!-- std@1/hex/decode -->
- `pub fn hex.encode(data: Bytes) -> unique String uses {}` <!-- std@1/hex/encode -->

## http

- `pub type http.Config { max_redirects: u32, connect_timeout: Option[time.Duration], read_timeout: Option[time.Duration], write_timeout: Option[time.Duration], max_head_bytes: u64, max_body_bytes: u64, idle_timeout: Option[time.Duration], max_headers: u64, user_agent: String, roots: TlsRoots, status_as_error: bool, }` <!-- std@1/http/Config -->
- `pub type http.Header { name: String, value: String, }` <!-- std@1/http/Header -->
- `pub type http.HttpError = Net(NetError) | Url(uri.UriError) | Malformed(http_wire.HeadError) | Chunk | HeadTooLarge | BodyTooLarge | Truncated | TooManyRedirects | RedirectRequiresResend | RedirectLocationInvalid | UnsupportedScheme | UnsupportedTransferEncoding | Status( u16 ) | OutOfOrder | Closed | RequestTimeout | Clock(ClockError)` <!-- std@1/http/HttpError -->
- `BodyTooLarge -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#BodyTooLarge -->
- `Chunk -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Chunk -->
- `Clock(ClockError) -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Clock -->
- `Closed -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Closed -->
- `HeadTooLarge -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#HeadTooLarge -->
- `Malformed(http_wire.HeadError) -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Malformed -->
- `Net(NetError) -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Net -->
- `OutOfOrder -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#OutOfOrder -->
- `RedirectLocationInvalid -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#RedirectLocationInvalid -->
- `RedirectRequiresResend -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#RedirectRequiresResend -->
- `RequestTimeout -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#RequestTimeout -->
- `Status( u16 ) -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Status -->
- `TooManyRedirects -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#TooManyRedirects -->
- `Truncated -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Truncated -->
- `UnsupportedScheme -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#UnsupportedScheme -->
- `UnsupportedTransferEncoding -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#UnsupportedTransferEncoding -->
- `Url(uri.UriError) -> http.HttpError` — Variant of http.HttpError. <!-- std@1/http/HttpError#Url -->
- `pub type http.Method = Get | Head | Post | Put | Delete | Patch | Options` — Port of Zig lib/std/http/Client.zig request/sendBodiless/sendBodyComplete, Request.receiveHead/redirect/fetch and lib/std/http.zig Reader.State and BodyWriter.chunkedDrain/endChunked, bff490a23889750de7096d9057c0a432efc929a8. <!-- std@1/http/Method -->
- `Delete -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Delete -->
- `Get -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Get -->
- `Head -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Head -->
- `Options -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Options -->
- `Patch -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Patch -->
- `Post -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Post -->
- `Put -> http.Method` — Variant of http.Method. <!-- std@1/http/Method#Put -->
- `pub opaque resource type http.Request: noalloc_drop uses {Net}` — ready=0, streaming request=1, sent=2, body=3, done=4, failed=5. <!-- std@1/http/Request -->
- `pub type http.Response { status: u16, headers: List[Header], body: Bytes, url: String, }` <!-- std@1/http/Response -->
- `pub type http.ResponseHead { version: Version, status: u16, reason: String, headers: List[Header], content_length: Option[u64], chunked: bool, keep_alive: bool, location: Option[String], content_type: Option[String], }` <!-- std@1/http/ResponseHead -->
- `pub opaque resource type http.ServerConnection: noalloc_drop uses {Net}` — A sequential server connection owns its stream and unread bytes. <!-- std@1/http/ServerConnection -->
- `pub type http.ServerRequest { method: String, target: String, version: Version, headers: List[Header], body: Bytes, trailers: List[Header], keep_alive: bool, }` <!-- std@1/http/ServerRequest -->
- `pub type http.Version = Http10 | Http11` <!-- std@1/http/Version -->
- `Http10 -> http.Version` — Variant of http.Version. <!-- std@1/http/Version#Http10 -->
- `Http11 -> http.Version` — Variant of http.Version. <!-- std@1/http/Version#Http11 -->
- `pub fn http.default_config() -> Config uses {}` <!-- std@1/http/default_config -->
- `pub fn http.end_body(mut r: Request, request_trailers: List[Header]) -> Result[(), HttpError]` <!-- std@1/http/end_body -->
- `pub fn http.fetch( n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes] ) -> Result[Response, HttpError]` <!-- std@1/http/fetch -->
- `pub fn http.header(headers: List[Header], name: String) -> Option[String] uses {}` — Zig HeaderIterator lookup; the first case-insensitive match wins. <!-- std@1/http/header -->
- `pub fn http.parse_server_request(buf: Bytes, max_head_bytes: u64, max_body_bytes: u64) -> Result[ Option[(ServerRequest, u64)], HttpError ] uses {}` — Complete includes decoded body and trailers; subsequent bytes belong to the next request. <!-- std@1/http/parse_server_request -->
- `pub fn http.read_all(mut r: Request, limit: u64) -> Result[unique Bytes, HttpError]` <!-- std@1/http/read_all -->
- `@preserves_unique(out) pub fn http.read_body(mut r: Request, mut out: List[u8], max: u64) -> Result[u64, HttpError]` <!-- std@1/http/read_body -->
- `pub fn http.receive_head(mut r: Request) -> Result[ResponseHead, HttpError]` <!-- std@1/http/receive_head -->
- `pub fn http.receive_request(mut c: ServerConnection, clock: Option[MonotonicClock]) -> Result[ ServerRequest, HttpError ]` — Clean EOF and the keep-alive idle limit return Closed. <!-- std@1/http/receive_request -->
- `pub fn http.request( n: Net, config: Config, method: Method, url: String, headers: List[Header] ) -> Result[Request, HttpError]` <!-- std@1/http/request -->
- `pub fn http.respond( mut c: ServerConnection, status: u16, headers: List[Header], body: Bytes, close: bool ) -> Result[(), HttpError]` — The writer owns framing: callers cannot inject a second length or encoding. <!-- std@1/http/respond -->
- `pub fn http.send_bodiless(mut r: Request) -> Result[(), HttpError]` <!-- std@1/http/send_bodiless -->
- `pub fn http.send_body(mut r: Request, body: Bytes) -> Result[(), HttpError]` <!-- std@1/http/send_body -->
- `pub fn http.send_chunk(mut r: Request, data: Bytes) -> Result[(), HttpError]` <!-- std@1/http/send_chunk -->
- `pub fn http.server_connection(n: Net, take stream: Stream of n, config: Config) -> ServerConnection` <!-- std@1/http/server_connection -->
- `pub fn http.trailers(r: Request) -> unique List[Header]` <!-- std@1/http/trailers -->

## http_wire

- `pub type http_wire.ChunkError = InvalidChunkSize()` <!-- std@1/http_wire/ChunkError -->
- `InvalidChunkSize() -> http_wire.ChunkError` — Variant of http_wire.ChunkError. <!-- std@1/http_wire/ChunkError#InvalidChunkSize -->
- `pub type http_wire.ChunkSize = Complete(u64, u64) | Partial` <!-- std@1/http_wire/ChunkSize -->
- `Complete(u64, u64) -> http_wire.ChunkSize` — Variant of http_wire.ChunkSize. <!-- std@1/http_wire/ChunkSize#Complete -->
- `Partial -> http_wire.ChunkSize` — Variant of http_wire.ChunkSize. <!-- std@1/http_wire/ChunkSize#Partial -->
- `pub type http_wire.Framing { content_length: Option[u64], chunked: bool, keep_alive: bool, }` <!-- std@1/http_wire/Framing -->
- `pub type http_wire.HeadError = HeaderName | HeaderValue | NewLine | Status | Token | TooManyHeaders | Version | ContentLength | TransferEncoding | Continuation` <!-- std@1/http_wire/HeadError -->
- `ContentLength -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#ContentLength -->
- `Continuation -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#Continuation -->
- `HeaderName -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#HeaderName -->
- `HeaderValue -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#HeaderValue -->
- `NewLine -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#NewLine -->
- `Status -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#Status -->
- `Token -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#Token -->
- `TooManyHeaders -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#TooManyHeaders -->
- `TransferEncoding -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#TransferEncoding -->
- `Version -> http_wire.HeadError` — Variant of http_wire.HeadError. <!-- std@1/http_wire/HeadError#Version -->
- `pub type http_wire.HeaderIterator { index: u64, is_trailer: bool, done: bool, }` — Zig lib/std/http/HeaderIterator.zig init/next and test next, bff490a2. <!-- std@1/http_wire/HeaderIterator -->
- `pub type http_wire.ParseStatus = Complete(u64) | Partial` <!-- std@1/http_wire/ParseStatus -->
- `Complete(u64) -> http_wire.ParseStatus` — Variant of http_wire.ParseStatus. <!-- std@1/http_wire/ParseStatus#Complete -->
- `Partial -> http_wire.ParseStatus` — Variant of http_wire.ParseStatus. <!-- std@1/http_wire/ParseStatus#Partial -->
- `pub type http_wire.RawHeader { name_start: u64, name_end: u64, value_start: u64, value_end: u64, }` — Scalar response/header/chunk parsers ported from httparse src/lib.rs, a0fa552e4e0fa9a5914a736ef3a556748fef709b (MIT; third-party/NOTICES.md). <!-- std@1/http_wire/RawHeader -->
- `pub type http_wire.RequestLine { method_start: u64, method_end: u64, target_start: u64, target_end: u64, version: u8, }` — httparse Request::parse / parse_method / parse_uri, strict delimiters. <!-- std@1/http_wire/RequestLine -->
- `pub type http_wire.StatusLine { version: u8, code: u16, reason_start: u64, reason_end: u64, }` <!-- std@1/http_wire/StatusLine -->
- `@noalloc pub fn http_wire.find_header[k: size]( buf: Bytes, headers: Inline[RawHeader, k], count: u64, name: Bytes ) -> Option[RawHeader] uses {}` — Case-insensitive lookup over httparse's validated header offsets. <!-- std@1/http_wire/find_header -->
- `@noalloc pub fn http_wire.framing[k: size]( buf: Bytes, version: u8, headers: Inline[RawHeader, k], count: u64 ) -> Result[Framing, HeadError] uses {}` — Zig lib/std/http/Client.zig Response.Head.parse, bff490a2 (MIT). <!-- std@1/http_wire/framing -->
- `@noalloc pub fn http_wire.header_iterator(buf: Bytes) -> HeaderIterator uses {}` <!-- std@1/http_wire/header_iterator -->
- `@noalloc pub fn http_wire.next_header(buf: Bytes, mut it: HeaderIterator) -> Option[RawHeader] uses {}` <!-- std@1/http_wire/next_header -->
- `@noalloc pub fn http_wire.parse_chunk_size(buf: Bytes, start: u64) -> Result[ChunkSize, ChunkError] uses {}` — httparse parse_chunk_size: <=16 hex digits, implied LWS before ';', opaque extensions and CRLF. <!-- std@1/http_wire/parse_chunk_size -->
- `@noalloc pub fn http_wire.parse_headers[k: size]( buf: Bytes, start: u64, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}` — httparse parse_headers_iter_uninit (strict config). <!-- std@1/http_wire/parse_headers -->
- `@noalloc pub fn http_wire.parse_request[k: size]( buf: Bytes, mut line: RequestLine, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}` <!-- std@1/http_wire/parse_request -->
- `@noalloc pub fn http_wire.parse_response[k: size]( buf: Bytes, mut line: StatusLine, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}` — httparse Response::parse, skip_empty_lines, parse_version/code/reason. <!-- std@1/http_wire/parse_response -->

## i128

- `@noalloc pub fn i128.checked_add(a: i128, b: i128) -> Option[i128] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/i128/checked_add -->
- `@noalloc pub fn i128.checked_mul(a: i128, b: i128) -> Option[i128] uses {}` <!-- std@1/i128/checked_mul -->
- `@noalloc pub fn i128.checked_sub(a: i128, b: i128) -> Option[i128] uses {}` <!-- std@1/i128/checked_sub -->
- `@noalloc pub fn i128.format[n: size](value: i128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/i128/format -->
- `@preserves_unique(out) pub fn i128.format_into(value: i128, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/i128/format_into -->
- `@preserves_unique(out) pub fn i128.format_into_spec( value: i128, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/i128/format_into_spec -->
- `pub fn i128.format_radix(value: i128, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/i128/format_radix -->
- `const i128.order: Order[i128]` <!-- core@1/i128/order -->
- `@noalloc pub fn i128.parse(text: String) -> Result[i128, scalar.ParseError] uses {}` <!-- std@1/i128/parse -->
- `@noalloc pub fn i128.parse_radix(text: String, radix: u8) -> Result[i128, scalar.ParseError] uses {}` <!-- std@1/i128/parse_radix -->
- `@noalloc pub fn i128.saturating_add(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/saturating_add -->
- `@noalloc pub fn i128.saturating_mul(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/saturating_mul -->
- `@noalloc pub fn i128.saturating_sub(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/saturating_sub -->
- `pub fn i128.to_string(value: i128) -> unique String uses {}` <!-- std@1/i128/to_string -->
- `@noalloc pub fn i128.wrapping_add(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/wrapping_add -->
- `@noalloc pub fn i128.wrapping_mul(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/wrapping_mul -->
- `@noalloc pub fn i128.wrapping_sub(a: i128, b: i128) -> i128 uses {}` <!-- std@1/i128/wrapping_sub -->

## i16

- `@noalloc pub fn i16.checked_add(a: i16, b: i16) -> Option[i16] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/i16/checked_add -->
- `@noalloc pub fn i16.checked_mul(a: i16, b: i16) -> Option[i16] uses {}` <!-- std@1/i16/checked_mul -->
- `@noalloc pub fn i16.checked_sub(a: i16, b: i16) -> Option[i16] uses {}` <!-- std@1/i16/checked_sub -->
- `@noalloc pub fn i16.format[n: size](value: i16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/i16/format -->
- `@preserves_unique(out) pub fn i16.format_into(value: i16, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/i16/format_into -->
- `@preserves_unique(out) pub fn i16.format_into_spec( value: i16, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/i16/format_into_spec -->
- `pub fn i16.format_radix(value: i16, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/i16/format_radix -->
- `const i16.order: Order[i16]` <!-- core@1/i16/order -->
- `@noalloc pub fn i16.parse(text: String) -> Result[i16, scalar.ParseError] uses {}` <!-- std@1/i16/parse -->
- `@noalloc pub fn i16.parse_radix(text: String, radix: u8) -> Result[i16, scalar.ParseError] uses {}` <!-- std@1/i16/parse_radix -->
- `@noalloc pub fn i16.saturating_add(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/saturating_add -->
- `@noalloc pub fn i16.saturating_mul(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/saturating_mul -->
- `@noalloc pub fn i16.saturating_sub(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/saturating_sub -->
- `pub fn i16.to_string(value: i16) -> unique String uses {}` <!-- std@1/i16/to_string -->
- `@noalloc pub fn i16.wrapping_add(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/wrapping_add -->
- `@noalloc pub fn i16.wrapping_mul(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/wrapping_mul -->
- `@noalloc pub fn i16.wrapping_sub(a: i16, b: i16) -> i16 uses {}` <!-- std@1/i16/wrapping_sub -->

## i32

- `@noalloc pub fn i32.checked_add(a: i32, b: i32) -> Option[i32] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/i32/checked_add -->
- `@noalloc pub fn i32.checked_mul(a: i32, b: i32) -> Option[i32] uses {}` <!-- std@1/i32/checked_mul -->
- `@noalloc pub fn i32.checked_sub(a: i32, b: i32) -> Option[i32] uses {}` <!-- std@1/i32/checked_sub -->
- `@noalloc pub fn i32.format[n: size](value: i32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/i32/format -->
- `@preserves_unique(out) pub fn i32.format_into(value: i32, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/i32/format_into -->
- `@preserves_unique(out) pub fn i32.format_into_spec( value: i32, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/i32/format_into_spec -->
- `pub fn i32.format_radix(value: i32, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/i32/format_radix -->
- `const i32.order: Order[i32]` <!-- core@1/i32/order -->
- `@noalloc pub fn i32.parse(text: String) -> Result[i32, scalar.ParseError] uses {}` <!-- std@1/i32/parse -->
- `@noalloc pub fn i32.parse_radix(text: String, radix: u8) -> Result[i32, scalar.ParseError] uses {}` <!-- std@1/i32/parse_radix -->
- `@noalloc pub fn i32.saturating_add(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/saturating_add -->
- `@noalloc pub fn i32.saturating_mul(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/saturating_mul -->
- `@noalloc pub fn i32.saturating_sub(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/saturating_sub -->
- `pub fn i32.to_string(value: i32) -> unique String uses {}` <!-- std@1/i32/to_string -->
- `@noalloc pub fn i32.wrapping_add(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/wrapping_add -->
- `@noalloc pub fn i32.wrapping_mul(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/wrapping_mul -->
- `@noalloc pub fn i32.wrapping_sub(a: i32, b: i32) -> i32 uses {}` <!-- std@1/i32/wrapping_sub -->

## i64

- `@noalloc pub fn i64.checked_add(a: i64, b: i64) -> Option[i64] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/i64/checked_add -->
- `@noalloc pub fn i64.checked_mul(a: i64, b: i64) -> Option[i64] uses {}` <!-- std@1/i64/checked_mul -->
- `@noalloc pub fn i64.checked_sub(a: i64, b: i64) -> Option[i64] uses {}` <!-- std@1/i64/checked_sub -->
- `@noalloc pub fn i64.format[n: size](value: i64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/i64/format -->
- `@preserves_unique(out) pub fn i64.format_into(value: i64, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/i64/format_into -->
- `@preserves_unique(out) pub fn i64.format_into_spec( value: i64, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/i64/format_into_spec -->
- `pub fn i64.format_radix(value: i64, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/i64/format_radix -->
- `const i64.order: Order[i64]` <!-- core@1/i64/order -->
- `@noalloc pub fn i64.parse(text: String) -> Result[i64, scalar.ParseError] uses {}` <!-- std@1/i64/parse -->
- `@noalloc pub fn i64.parse_radix(text: String, radix: u8) -> Result[i64, scalar.ParseError] uses {}` <!-- std@1/i64/parse_radix -->
- `@noalloc pub fn i64.saturating_add(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/saturating_add -->
- `@noalloc pub fn i64.saturating_mul(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/saturating_mul -->
- `@noalloc pub fn i64.saturating_sub(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/saturating_sub -->
- `pub fn i64.to_string(value: i64) -> unique String uses {}` <!-- std@1/i64/to_string -->
- `@noalloc pub fn i64.wrapping_add(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/wrapping_add -->
- `@noalloc pub fn i64.wrapping_mul(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/wrapping_mul -->
- `@noalloc pub fn i64.wrapping_sub(a: i64, b: i64) -> i64 uses {}` <!-- std@1/i64/wrapping_sub -->

## i8

- `@noalloc pub fn i8.checked_add(a: i8, b: i8) -> Option[i8] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/i8/checked_add -->
- `@noalloc pub fn i8.checked_mul(a: i8, b: i8) -> Option[i8] uses {}` <!-- std@1/i8/checked_mul -->
- `@noalloc pub fn i8.checked_sub(a: i8, b: i8) -> Option[i8] uses {}` <!-- std@1/i8/checked_sub -->
- `@noalloc pub fn i8.format[n: size](value: i8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/i8/format -->
- `@preserves_unique(out) pub fn i8.format_into(value: i8, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/i8/format_into -->
- `@preserves_unique(out) pub fn i8.format_into_spec( value: i8, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/i8/format_into_spec -->
- `pub fn i8.format_radix(value: i8, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/i8/format_radix -->
- `const i8.order: Order[i8]` <!-- core@1/i8/order -->
- `@noalloc pub fn i8.parse(text: String) -> Result[i8, scalar.ParseError] uses {}` <!-- std@1/i8/parse -->
- `@noalloc pub fn i8.parse_radix(text: String, radix: u8) -> Result[i8, scalar.ParseError] uses {}` <!-- std@1/i8/parse_radix -->
- `@noalloc pub fn i8.saturating_add(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/saturating_add -->
- `@noalloc pub fn i8.saturating_mul(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/saturating_mul -->
- `@noalloc pub fn i8.saturating_sub(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/saturating_sub -->
- `pub fn i8.to_string(value: i8) -> unique String uses {}` <!-- std@1/i8/to_string -->
- `@noalloc pub fn i8.wrapping_add(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/wrapping_add -->
- `@noalloc pub fn i8.wrapping_mul(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/wrapping_mul -->
- `@noalloc pub fn i8.wrapping_sub(a: i8, b: i8) -> i8 uses {}` <!-- std@1/i8/wrapping_sub -->

## input

- `pub fn input.read_all(s: Stdin) -> Result[unique Bytes, InputError]` — Adapted from Rust alloc::io read_to_end/read_until and their tests (MIT option; third-party/NOTICES.md). <!-- std@1/input/read_all -->
- `pub fn input.read_line(s: Stdin) -> Result[Option[unique String], InputError]` <!-- std@1/input/read_line -->

## instant

- `@noalloc fn instant.checked_add(clock: MonotonicClock, start: Instant of clock, nanoseconds: u64) -> Option[Instant of clock]` <!-- core@1/instant/checked_add -->
- `@noalloc fn instant.ticks(value: Instant) -> u64 uses {}` <!-- core@1/instant/ticks -->

## int

- `@noalloc fn int.checked_add[I](a: I, b: I) -> Option[I]` <!-- core@1/int/checked_add -->
- `@noalloc fn int.div_exact[I](a: I, b: I) -> I` <!-- core@1/int/div_exact -->
- `@noalloc fn int.div_floor[I](a: I, b: I) -> I` <!-- core@1/int/div_floor -->
- `@noalloc fn int.mod[I](a: I, b: I) -> I` <!-- core@1/int/mod -->
- `@noalloc fn int.narrow[I, J](value: I) -> J` <!-- core@1/int/narrow -->
- `@noalloc fn int.saturating_add[I](a: I, b: I) -> I` <!-- core@1/int/saturating_add -->
- `@noalloc fn int.truncate[I, J](value: I) -> J` <!-- core@1/int/truncate -->
- `@noalloc fn int.try_narrow[I, J](value: I) -> Option[J]` <!-- core@1/int/try_narrow -->
- `@noalloc fn int.unchecked_add[I](permit: Unsafe, a: I, b: I) -> I` <!-- core@1/int/unchecked_add -->
- `@noalloc fn int.widen[I, J](value: I) -> J` <!-- core@1/int/widen -->
- `@noalloc fn int.wrapping_add[I](a: I, b: I) -> I` <!-- core@1/int/wrapping_add -->

## internal

- `pub type internal.big.Big { limbs: Inline[u64, 128], used: u64, first: u64, }` — Internal fixed decimal integer: 128 base-10^9 limbs, 1152 decimal digits. <!-- std@1/internal/big/Big -->
- `@noalloc pub fn internal.big.big_add_small(mut a: Big, value: u64) -> () uses {}` <!-- std@1/internal/big/big_add_small -->
- `@noalloc pub fn internal.big.big_compare(a: Big, b: Big, bump: bool) -> i32 uses {}` — Compare a + bump with b without constructing another 1 KiB value. <!-- std@1/internal/big/big_compare -->
- `@noalloc pub fn internal.big.big_digit(a: Big, i: u64) -> u64 uses {}` <!-- std@1/internal/big/big_digit -->
- `@noalloc pub fn internal.big.big_divide(mut a: Big, places: u64) -> u64 uses {}` <!-- std@1/internal/big/big_divide -->
- `@noalloc pub fn internal.big.big_from_word(value: u64) -> Big uses {}` <!-- std@1/internal/big/big_from_word -->
- `@noalloc pub fn internal.big.big_mul(mut a: Big, factor: u64) -> () uses {}` <!-- std@1/internal/big/big_mul -->
- `@noalloc pub fn internal.big.big_mul_word(mut a: Big, factor: u64) -> () uses {}` <!-- std@1/internal/big/big_mul_word -->
- `@noalloc pub fn internal.big.big_power(mut a: Big, base: u64, exponent: u64) -> () uses {}` <!-- std@1/internal/big/big_power -->
- `@noalloc pub fn internal.big.big_subtract(mut a: Big, b: Big) -> () uses {}` <!-- std@1/internal/big/big_subtract -->
- `@noalloc pub fn internal.big.big_trim(mut a: Big) -> () uses {}` <!-- std@1/internal/big/big_trim -->
- `@noalloc pub fn internal.big.big_word(a: Big) -> u64 uses {}` <!-- std@1/internal/big/big_word -->
- `@preserves_unique(out) pub fn internal.decimal.append(mut out: List[u8], magnitude: u128, negative: bool) -> () uses {}` — Appends the decimal text of 'magnitude' to 'out', after a minus sign when 'negative', after one reservation of its exact count. <!-- std@1/internal/decimal/append -->
- `@preserves_unique(out) pub fn internal.decimal.append_prefix(mut out: List[u8], buffer: Inline[u8, 40], count: u64) -> () uses {}` — Appends the first 'count' bytes of a 'format' buffer. <!-- std@1/internal/decimal/append_prefix -->
- `@noalloc pub fn internal.decimal.write_unsigned[n: size](value: u128, negative: bool, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Count first so capacity failure leaves every byte unchanged. <!-- std@1/internal/decimal/write_unsigned -->
- `@noalloc pub fn internal.float_decimal.parse(text: String, fraction: u64, bias: i32) -> Result[u64, scalar.ParseError] uses {}` — At most 800 significant decimal digits are retained. <!-- std@1/internal/float_decimal/parse -->
- `@noalloc pub fn internal.float_decimal.render[n: size]( significand: u64, scale: i32, negative: bool, mut buffer: Inline[u8, n] ) -> Result[u64, scalar.FormatError] uses {}` — Number::toString fixed/scientific thresholds: https://tc39.es/ecma262/#sec-numeric-types-number-tostring <!-- std@1/internal/float_decimal/render -->
- `@noalloc pub fn internal.float_decimal.shortest(m: u64, e: i32, asymmetric: bool) -> (u64, i32) uses {}` <!-- std@1/internal/float_decimal/shortest -->
- `@noalloc pub fn internal.float_decimal.special[n: size](text: String, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` <!-- std@1/internal/float_decimal/special -->
- `@preserves_unique(out) pub fn internal.format.begin( length: u64, negative: bool, prefix: String, fill: u64, align: u64, width: u64, flags: u64, mut out: List[u8] ) -> u64 uses {}` — Emits left padding, then the sign and radix prefix. <!-- std@1/internal/format/begin -->
- `@preserves_unique(out) pub fn internal.format.fixed( mantissa: u64, exponent: i32, negative: bool, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` — Exact dyadic-to-decimal conversion reuses the same fixed bigint as shortest. <!-- std@1/internal/format/fixed -->
- `@preserves_unique(out) pub fn internal.format.integer( value: u128, negative: bool, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/internal/format/integer -->
- `@preserves_unique(out) pub fn internal.format.pad_from( start: u64, fill: u64, align: u64, width: u64, numeric: bool, mut out: List[u8] ) -> () uses {}` — Pads the text appended since byte 'start' to 'width' scalars. <!-- std@1/internal/format/pad_from -->
- `@noalloc pub fn internal.format.padding(width: u64, length: u64, align: u64, numeric: bool) -> (u64, u64) uses {}` <!-- std@1/internal/format/padding -->
- `@preserves_unique(out) pub fn internal.format.repeat(mut out: List[u8], fill: u64, count: u64) -> () uses {}` — Padding and sign/prefix placement follow Rust core::fmt::Formatter. <!-- std@1/internal/format/repeat -->
- `@noalloc pub fn internal.heap_ops.sift[T]( mut xs: unique List[T], begin: u64, end: u64, by: @noalloc fn(T, T) -> i32 uses {} ) -> () uses {}` — Shared sift kernels adapted from Rust/Zig heap algorithms. <!-- std@1/internal/heap_ops/sift -->
- `@noalloc pub fn internal.heap_ops.sift_up[T]( mut xs: unique List[T], begin: u64, by: @noalloc fn(T, T) -> i32 uses {} ) -> () uses {}` <!-- std@1/internal/heap_ops/sift_up -->
- `pub fn internal.integer.format_radix(value: u128, negative: bool, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/internal/integer/format_radix -->
- `@noalloc pub fn internal.integer.read_radix(text: String, radix: u8, signed: bool, positive_limit: u128, negative_limit: u128) -> Result[(u128, bool), scalar.ParseError] uses {}` — Go internal/strconv/{atoi,itoa}.go radix loops (ebe2feef). <!-- std@1/internal/integer/read_radix -->
- `@noalloc pub fn internal.sample.below(draw: u64, upper: u64) -> Option[u64] uses {}` — Adapted from Zig lib/std/Random.zig uintLessThan (Lemire's multiply-shift with O'Neill's threshold), commit bff490a23889750de7096d9057c0a432efc929a8; copyright Zig contributors (third-party/NOTICES.md). <!-- std@1/internal/sample/below -->
- `pub type internal.superacc.Accumulator { plus: Inline[u64, 70], minus: Inline[u64, 70], }` — Exact binary superaccumulator (Kulisch): 2240 bits, separate signs. <!-- std@1/internal/superacc/Accumulator -->
- `@noalloc pub fn internal.superacc.acc_add(mut accumulator: Accumulator, mantissa: u64, shift: u64, negative: bool) -> () uses {}` <!-- std@1/internal/superacc/acc_add -->
- `@noalloc pub fn internal.superacc.acc_finish( accumulator: Accumulator, fraction: u64, exponent_limit: u64, sign_bit: u64, all_negative_zero: bool ) -> u64 uses {}` <!-- std@1/internal/superacc/acc_finish -->
- `@noalloc pub fn internal.superacc.acc_zero() -> Accumulator uses {}` <!-- std@1/internal/superacc/acc_zero -->

## io

- `fn io.dir(rt: Io, p: Path) -> Result[Dir, FileError]` <!-- core@1/io/dir -->
- `@noalloc fn io.monotonic_clock(rt: Io) -> Result[MonotonicClock, GrantError]` <!-- core@1/io/monotonic_clock -->
- `@noalloc fn io.net(rt: Io) -> Result[Net, GrantError]` <!-- core@1/io/net -->
- `@noalloc fn io.process(rt: Io) -> Result[Proc, GrantError]` <!-- core@1/io/process -->
- `@noalloc fn io.random(rt: Io) -> Result[Random, GrantError]` <!-- core@1/io/random -->
- `@noalloc fn io.stderr(rt: Io) -> Result[Stderr, GrantError]` <!-- core@1/io/stderr -->
- `@noalloc fn io.stdin(rt: Io) -> Result[Stdin, GrantError]` <!-- core@1/io/stdin -->
- `@noalloc fn io.stdout(rt: Io) -> Result[Stdout, GrantError]` <!-- core@1/io/stdout -->
- `@noalloc fn io.tasks(rt: Io) -> Result[Tasks, GrantError]` <!-- core@1/io/tasks -->
- `@noalloc fn io.wall_clock(rt: Io) -> Result[WallClock, GrantError]` <!-- core@1/io/wall_clock -->

## iter

- `pub fn iter.collect[T](take it: Iter[T]) -> List[T] uses {it}` <!-- std@1/iter/collect -->
- `@captures(it) pub fn iter.enumerate[T](take it: Iter[T]) -> Iter[(u64, T)] uses {it}` <!-- std@1/iter/enumerate -->
- `@captures(it, predicate) pub fn iter.filter[T](take it: Iter[T], take predicate: fn(T) -> bool) -> Iter[T] uses {it, predicate}` <!-- std@1/iter/filter -->
- `pub fn iter.fold[T, A](take it: Iter[T], take initial: A, take f: fn(take A, take T) -> A) -> A uses {it, initial, f}` <!-- std@1/iter/fold -->
- `fn iter.from[S, T](take state: S, take next: fn(mut S) -> Option[T]) -> Iter[T] uses {state, next}` <!-- core@1/iter/from -->
- `fn iter.map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f}` <!-- core@1/iter/map -->
- `fn iter.next[T](mut it: Iter[T]) -> Option[T]` <!-- core@1/iter/next -->
- `@captures(it) pub fn iter.take_n[T](take it: Iter[T], count: u64) -> Iter[T] uses {it}` <!-- std@1/iter/take_n -->
- `@captures(a, b) pub fn iter.zip[T, U](take a: Iter[T], take b: Iter[U]) -> Iter[(T, U)] uses {a, b}` <!-- std@1/iter/zip -->

## json

- `pub type json.JsonError = Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber` <!-- std@1/json/JsonError -->
- `DuplicateKey(u64) -> json.JsonError` — Variant of json.JsonError. <!-- std@1/json/JsonError#DuplicateKey -->
- `InvalidNumber -> json.JsonError` — Variant of json.JsonError. <!-- std@1/json/JsonError#InvalidNumber -->
- `Nesting(u64) -> json.JsonError` — Variant of json.JsonError. <!-- std@1/json/JsonError#Nesting -->
- `Syntax(u64) -> json.JsonError` — Variant of json.JsonError. <!-- std@1/json/JsonError#Syntax -->
- `pub const json.MAX_NESTING: u64` — Policy counts open arrays/objects; scalars do not consume a level. <!-- std@1/json/MAX_NESTING -->
- `pub type json.Value = Null | Bool(bool) | Number(String) | Text(String) | Array( List[box Value] ) | Object(Map[String, box Value])` — JSON DOM contracts: STD-OP-json.* and TYPE-ROW-Value/JsonError. <!-- std@1/json/Value -->
- `Array( List[box Value] ) -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Array -->
- `Bool(bool) -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Bool -->
- `Null -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Null -->
- `Number(String) -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Number -->
- `Object(Map[String, box Value]) -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Object -->
- `Text(String) -> json.Value` — Variant of json.Value. <!-- std@1/json/Value#Text -->
- `pub fn json.parse(take text: String) -> Result[Value, JsonError] uses {}` — Adapted serde_json src/de.rs from_str/end, commit 20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8. <!-- std@1/json/parse -->
- `pub fn json.stringify(value: Value) -> Result[unique String, JsonError] uses {}` — Adapted serde_json src/ser.rs to_string, commit 20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8: one writer path. <!-- std@1/json/stringify -->
- `@preserves_unique(builder) pub fn json.write(value: Value, mut builder: List[u8]) -> Result[(), JsonError] uses {}` — Adapted serde_json src/ser.rs compact Serializer dispatch and sequence/map emission, commit 20572a6d066958542d79cdd6cf1a7cd9f0c8e1f8. <!-- std@1/json/write -->

## list

- `@noalloc pub fn list.all[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}` <!-- std@1/list/all -->
- `@noalloc pub fn list.any[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}` <!-- std@1/list/any -->
- `@noalloc pub fn list.binary_search[T]( xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {} ) -> Option[u64] uses {}` — First equal element; callers provide input sorted by the same comparator. <!-- std@1/list/binary_search -->
- `pub fn list.concat[T](take a: List[T], take b: List[T]) -> List[T] uses {a, b}` <!-- std@1/list/concat -->
- `pub fn list.dedup[T: data](take xs: List[T]) -> List[T] uses {xs}` <!-- std@1/list/dedup -->
- `pub fn list.filter[T](take xs: List[T], predicate: fn(T) -> bool) -> List[T] uses {xs, predicate}` <!-- std@1/list/filter -->
- `@noalloc pub fn list.find[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> Option[u64] uses {xs, predicate}` <!-- std@1/list/find -->
- `pub fn list.fold[T, A](xs: List[T], take initial: A, f: fn(take A, T) -> A) -> A uses {xs, initial, f}` <!-- std@1/list/fold -->
- `@noalloc fn list.len[T](xs: List[T]) -> u64` — Number of elements in 'xs: List[T]'. <!-- core@1/list/len -->
- `pub fn list.map[T, U](xs: List[T], f: fn(T) -> U) -> List[U] uses {xs, f}` <!-- std@1/list/map -->
- `pub fn list.partition[T](take xs: List[T], predicate: fn(T) -> bool) -> (List[T], List[T]) uses {xs, predicate}` <!-- std@1/list/partition -->
- `@noalloc fn list.pop[T](mut xs: unique List[T]) -> Option[T]` <!-- core@1/list/pop -->
- `@preserves_unique(xs) fn list.push[T](mut xs: List[T], take value: T) -> ()` — Appends 'v: T' to a mutable 'List[T]'. <!-- core@1/list/push -->
- `@noalloc fn list.remove[T](mut xs: unique List[T], index: u64) -> T` <!-- core@1/list/remove -->
- `@preserves_unique(xs) fn list.reserve[T](mut xs: List[T], capacity: u64) -> ()` <!-- core@1/list/reserve -->
- `@noalloc pub fn list.reverse[T](mut xs: unique List[T]) -> ()` — Collection algorithms use ordinary source calls and ownership. <!-- std@1/list/reverse -->
- `@noalloc pub fn list.rotate[T](mut xs: unique List[T], amount: u64) -> ()` — Left rotation uses three reversals, each preserving element ownership. <!-- std@1/list/rotate -->
- `@noalloc pub fn list.sort[T](mut xs: unique List[T], by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {}` — In-place, unstable heapsort, O(n log n), including move-only elements. <!-- std@1/list/sort -->
- `@noalloc fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()` <!-- core@1/list/swap -->

## log

- `pub type log.Entry { level: Level, message: String, fields: List[Field], timestamp: Option[time.Timestamp], }` <!-- std@1/log/Entry -->
- `pub type log.Field { name: String, value: String, }` <!-- std@1/log/Field -->
- `pub type log.Level = Debug | Info | Warn | Error` — Go log/slog JSON records/levels, using Hirsel's existing JSON escaping. <!-- std@1/log/Level -->
- `Debug -> log.Level` — Variant of log.Level. <!-- std@1/log/Level#Debug -->
- `Error -> log.Level` — Variant of log.Level. <!-- std@1/log/Level#Error -->
- `Info -> log.Level` — Variant of log.Level. <!-- std@1/log/Level#Info -->
- `Warn -> log.Level` — Variant of log.Level. <!-- std@1/log/Level#Warn -->
- `pub fn log.format(entry: Entry) -> String uses {}` <!-- std@1/log/format -->
- `pub fn log.write(sink: Stderr, minimum: Level, entry: Entry) -> Result[(), OutputError]` <!-- std@1/log/write -->

## map

- `@noalloc fn map.contains[K: key, V](m: Map[K, V], k: K) -> bool` <!-- core@1/map/contains -->
- `fn map.empty[K: key, V]() -> unique Map[K, V]` <!-- core@1/map/empty -->
- `@noalloc fn map.get[K: key, V](m: Map[K, V], k: K) -> Option[V] in m` <!-- core@1/map/get -->
- `@preserves_unique(m) fn map.insert[K: key, V](mut m: Map[K, V], take k: K, take v: V) -> ()` <!-- core@1/map/insert -->
- `@noalloc fn map.len[K: key, V](m: Map[K, V]) -> u64` <!-- core@1/map/len -->
- `fn map.remove[K: key, V](mut m: unique Map[K, V], k: K) -> unique Option[V]` <!-- core@1/map/remove -->
- `@noalloc pub fn map.same_entries[K: key, V: data](a: Map[K, V], b: Map[K, V]) -> bool` — Equality ignores insertion order and borrows all entries, including opaque data. <!-- std@1/map/same_entries -->

## mutex

- `fn mutex.create[T: sendable](tasks: Tasks, take v: unique T) -> Mutex[T]` <!-- core@1/mutex/create -->
- `fn mutex.with[T, R: sendable](tasks: Tasks, m: Mutex[T], body: fn(mut v: unique T) -> R uses {}) -> R` <!-- core@1/mutex/with -->

## net

- `pub type net.AddrParseError = Invalid()` — Port of Rust library/core/src/net/parser.rs and Display in ip_addr.rs/socket_addr.rs, f6c1107f8be13e48465e12c0fd26e6a39fe18e4f (MIT option; third-party/NOTICES.md). <!-- std@1/net/AddrParseError -->
- `Invalid() -> net.AddrParseError` — Variant of net.AddrParseError. <!-- std@1/net/AddrParseError#Invalid -->
- `pub type net.HostError = NameTooLong | InvalidHostName` <!-- std@1/net/HostError -->
- `InvalidHostName -> net.HostError` — Variant of net.HostError. <!-- std@1/net/HostError#InvalidHostName -->
- `NameTooLong -> net.HostError` — Variant of net.HostError. <!-- std@1/net/HostError#NameTooLong -->
- `fn net.accept(n: Net, mut l: Listener of n) -> Result[(Stream of n, SocketAddr), NetError]` — the listener's grant; the peer is not checked <!-- core@1/net/accept -->
- `@noalloc fn net.close(n: Net, take s: Stream of n) -> ()` <!-- core@1/net/close -->
- `fn net.connect(n: Net, host: String, port: u16, timeout: NetTimeout) -> Result[Stream of n, NetError]` — '(host, port, Connect())' or '(host, port, ConnectTls())' <!-- core@1/net/connect -->
- `fn net.connect_addr(n: Net, addr: SocketAddr, timeout: NetTimeout) -> Result[Stream of n, NetError]` — a rule whose address is the canonical literal of 'addr.ip', with 'addr.port'; a host-name rule never authorizes a raw address <!-- core@1/net/connect_addr -->
- `@noalloc pub fn net.ends_in_number(text: String) -> bool uses {}` — WHATWG URL ends-in-a-number checker; shared by host validation and URI parsing. <!-- std@1/net/ends_in_number -->
- `pub fn net.format_ip(ip: IpAddr) -> unique String uses {}` — Rust Display: mapped IPv4, otherwise the first longest zero run (length > 1). <!-- std@1/net/format_ip -->
- `pub fn net.format_socket_addr(addr: SocketAddr) -> unique String uses {}` <!-- std@1/net/format_socket_addr -->
- `fn net.listen(n: Net, addr: SocketAddr) -> Result[Listener of n, NetError]` — '(literal, port, Listen())' for 'addr'; port 0 grants an ephemeral port <!-- core@1/net/listen -->
- `@noalloc fn net.local_addr(n: Net, l: Listener of n) -> Result[SocketAddr, NetError]` <!-- core@1/net/local_addr -->
- `fn net.narrow(n: Net, rules: List[NetRule]) -> Result[Net, NetError]` <!-- core@1/net/narrow -->
- `@noalloc pub fn net.parse_ip(text: String) -> Result[IpAddr, AddrParseError] uses {}` <!-- std@1/net/parse_ip -->
- `@noalloc pub fn net.parse_ipv4(text: String) -> Result[Inline[u8, 4], AddrParseError] uses {}` <!-- std@1/net/parse_ipv4 -->
- `@noalloc pub fn net.parse_ipv6(text: String) -> Result[Inline[u8, 16], AddrParseError] uses {}` <!-- std@1/net/parse_ipv6 -->
- `@noalloc pub fn net.parse_socket_addr(text: String) -> Result[SocketAddr, AddrParseError] uses {}` <!-- std@1/net/parse_socket_addr -->
- `@noalloc fn net.peer_addr(n: Net, s: Stream of n) -> Result[SocketAddr, NetError]` <!-- core@1/net/peer_addr -->
- `fn net.read(n: Net, mut s: Stream of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]` <!-- core@1/net/read -->
- `@noalloc fn net.read_into[k: size](n: Net, mut s: Stream of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]` <!-- core@1/net/read_into -->
- `fn net.resolve(n: Net, host: String, port: u16) -> Result[List[SocketAddr], NetError]` — a rule naming '(host, port)', any operation <!-- core@1/net/resolve -->
- `@noalloc fn net.set_read_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]` <!-- core@1/net/set_read_timeout -->
- `@noalloc fn net.set_write_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]` <!-- core@1/net/set_write_timeout -->
- `@noalloc fn net.shutdown(n: Net, mut s: Stream of n, how: Shutdown) -> Result[(), NetError]` <!-- core@1/net/shutdown -->
- `@noalloc pub fn net.validate_host(text: String) -> Result[(), HostError] uses {}` — Zig lib/std/Io/net/HostName.zig validate, bff490a2 (MIT). <!-- std@1/net/validate_host -->
- `@noalloc fn net.write(n: Net, mut s: Stream of n, data: Bytes) -> Result[u64, NetError]` <!-- core@1/net/write -->
- `@noalloc fn net.write_all(n: Net, mut s: Stream of n, data: Bytes) -> Result[(), NetError]` <!-- core@1/net/write_all -->

## option

- `pub fn option.and_then[T, U](take value: Option[T], take f: once fn(take T) -> Option[U]) -> Option[ U ] uses {value, f}` — Chains a transformation that may itself return None. <!-- std@1/option/and_then -->
- `pub fn option.filter[T](take value: Option[T], take predicate: once fn(T) -> bool) -> Option[T] uses {value, predicate}` — Borrows the payload for the predicate and retains it only on success. <!-- std@1/option/filter -->
- `@preserves_unique(value) pub fn option.flatten[T](take value: Option[Option[T]]) -> Option[T] uses {value}` — Removes exactly one level of nesting. <!-- std@1/option/flatten -->
- `@noalloc pub fn option.is_none[T](value: Option[T]) -> bool uses {}` — Tests whether a value is absent without consuming it. <!-- std@1/option/is_none -->
- `@noalloc pub fn option.is_some[T](value: Option[T]) -> bool uses {}` — Adapted from Rust core::option, f6c1107f8be13e48465e12c0fd26e6a39fe18e4f. <!-- std@1/option/is_some -->
- `pub fn option.map[T, U](take value: Option[T], take f: once fn(take T) -> U) -> Option[U] uses {value, f}` — Transforms a present payload, consuming it. <!-- std@1/option/map -->
- `@preserves_unique(value) pub fn option.ok_or[T, E](take value: Option[T], take error: E) -> Result[T, E] uses {value, error}` — Converts absence to the eagerly evaluated error. <!-- std@1/option/ok_or -->
- `pub fn option.ok_or_else[T, E](take value: Option[T], take error: once fn() -> E) -> Result[ T, E ] uses {value, error}` — Computes an error only when no payload is present. <!-- std@1/option/ok_or_else -->
- `pub fn option.or_else[T](take value: Option[T], take fallback: once fn() -> Option[T]) -> Option[ T ] uses {value, fallback}` — Computes another option only when no payload is present. <!-- std@1/option/or_else -->
- `@preserves_unique(value) pub fn option.unwrap_or[T](take value: Option[T], take fallback: T) -> T uses {value, fallback}` — Returns the payload or the eagerly evaluated fallback. <!-- std@1/option/unwrap_or -->
- `pub fn option.unwrap_or_else[T](take value: Option[T], take fallback: once fn() -> T) -> T uses {value, fallback}` — Computes the fallback only when no payload is present. <!-- std@1/option/unwrap_or_else -->
- `@preserves_unique(left) pub fn option.zip[T, U](take left: Option[T], take right: Option[U]) -> Option[(T, U)] uses {left, right}` — Pairs two present payloads; otherwise discards both inputs. <!-- std@1/option/zip -->

## out

- `@noalloc fn out.print(s: Stdout, text: String) -> Result[(), OutputError]` <!-- core@1/out/print -->

## path

- `pub type path.PathError = Invalid(u64)` — Slash-only lexical policy. <!-- std@1/path/PathError -->
- `Invalid(u64) -> path.PathError` — Variant of path.PathError. <!-- std@1/path/PathError#Invalid -->
- `pub fn path.components(p: Path) -> unique List[String] uses {}` — Adapted from Zig lib/std/fs/path.zig mem.tokenizeScalar used by resolveAppendPosix. <!-- std@1/path/components -->
- `pub fn path.extension(p: Path) -> unique String uses {}` — Empty means both absent extension and an empty suffix ("a."). <!-- std@1/path/extension -->
- `pub fn path.filename(p: Path) -> unique String uses {}` — Adapted from Zig lib/std/fs/path.zig basenameInner (last component; owned UTF-8 result). <!-- std@1/path/filename -->
- `pub fn path.join(base: Path, child: Path) -> Result[Path, PathError] uses {}` — Adapted from Zig lib/std/fs/path.zig joinSepMaybeZ/resolveAppendPosix (relative child contract). <!-- std@1/path/join -->
- `pub fn path.normalize(p: Path) -> Result[Path, PathError] uses {}` — Adapted from Zig lib/std/fs/path.zig resolveAppendPosix (active prefix instead of raw buffer truncation). <!-- std@1/path/normalize -->
- `fn path.of(text: String) -> Path` <!-- core@1/path/of -->
- `pub fn path.parent(p: Path) -> Option[Path] uses {}` — Adapted from Zig lib/std/fs/path.zig dirnameInner (raw component prefix; singleton parent is dot). <!-- std@1/path/parent -->
- `fn path.text(p: Path) -> String` <!-- core@1/path/text -->

## priority_queue

- `pub opaque type priority_queue.Heap[T]: () uses {T}` — Min-heap endpoint semantics from Zig PriorityQueue and Rust BinaryHeap. <!-- std@1/priority_queue/Heap -->
- `pub fn priority_queue.empty[T]() -> unique Heap[T] uses {}` <!-- std@1/priority_queue/empty -->
- `@noalloc pub fn priority_queue.len[T](h: Heap[T]) -> u64 uses {}` <!-- std@1/priority_queue/len -->
- `pub fn priority_queue.pop[T](mut h: unique Heap[T], by: @noalloc fn(T, T) -> i32 uses {}) -> unique Option[ T ] uses {h}` <!-- std@1/priority_queue/pop -->
- `pub fn priority_queue.push[T]( mut h: unique Heap[T], take value: unique T, by: @noalloc fn(T, T) -> i32 uses {} ) -> () uses {h, value}` <!-- std@1/priority_queue/push -->

## prng

- `pub opaque type prng.Generator: inert + noalloc_drop uses {}` <!-- std@1/prng/Generator -->
- `pub type prng.SampleError = EmptyRange()` — Adapted from Zig lib/std/Random/Xoshiro256.zig (Xoshiro256++) and lib/std/Random/SplitMix64.zig, commit bff490a23889750de7096d9057c0a432efc929a8; copyright Zig contributors (third-party/NOTICES.md). <!-- std@1/prng/SampleError -->
- `EmptyRange() -> prng.SampleError` — Variant of prng.SampleError. <!-- std@1/prng/SampleError#EmptyRange -->
- `@noalloc pub fn prng.below(mut generator: Generator, upper_exclusive: u64) -> Result[u64, SampleError] uses {}` — Unbiased: rejected draws are discarded, never reduced by modulo. <!-- std@1/prng/below -->
- `pub fn prng.fill(mut generator: Generator, mut out: List[u8], count: u64) -> () uses {}` — Appends count bytes: each draw little-endian, the last draw cut short. <!-- std@1/prng/fill -->
- `@noalloc pub fn prng.next_u64(mut generator: Generator) -> u64 uses {}` <!-- std@1/prng/next_u64 -->
- `@noalloc pub fn prng.seed(value: u64) -> Generator uses {}` <!-- std@1/prng/seed -->

## probe

- `@noalloc pub fn probe.identity(value: u64) -> u64 uses {}` <!-- std@1/probe/identity -->

## proc

- `fn proc.run(p: Proc, program: Path, args: List[String], stdin: Bytes, dir: Option[Dir], max_output: u64, deadline: Option[Deadline]) -> Result[ProcessOutput, ProcessError]` <!-- core@1/proc/run -->
- `fn proc.spawn(p: Proc, program: Path, args: List[String], dir: Option[Dir]) -> Result[Child of p, ProcessError]` <!-- core@1/proc/spawn -->

## random

- `pub type random.SampleError = EmptyRange | Source(RandomError)` — Adapted from Zig lib/std/Random.zig int (little-endian draw) and uintLessThan, commit bff490a23889750de7096d9057c0a432efc929a8; copyright Zig contributors (third-party/NOTICES.md). <!-- std@1/random/SampleError -->
- `EmptyRange -> random.SampleError` — Variant of random.SampleError. <!-- std@1/random/SampleError#EmptyRange -->
- `Source(RandomError) -> random.SampleError` — Variant of random.SampleError. <!-- std@1/random/SampleError#Source -->
- `pub fn random.below(r: Random, upper_exclusive: u64) -> Result[u64, SampleError]` — Unbiased: rejected draws are discarded, never reduced by modulo. <!-- std@1/random/below -->
- `fn random.bytes(r: Random, count: u64) -> Result[Bytes, RandomError]` <!-- core@1/random/bytes -->
- `pub fn random.u64(r: Random) -> Result[u64, RandomError]` <!-- std@1/random/u64 -->

## regex

- `pub type regex.CompileError = Syntax(u64) | ProgramTooLarge` — Thompson compiler and ordered Pike queues adapted from Go regexp/syntax {parse,compile,prog}.go and regexp/exec.go. <!-- std@1/regex/CompileError -->
- `ProgramTooLarge -> regex.CompileError` — Variant of regex.CompileError. <!-- std@1/regex/CompileError#ProgramTooLarge -->
- `Syntax(u64) -> regex.CompileError` — Variant of regex.CompileError. <!-- std@1/regex/CompileError#Syntax -->
- `pub type regex.MatchError = WorkLimit()` <!-- std@1/regex/MatchError -->
- `WorkLimit() -> regex.MatchError` — Variant of regex.MatchError. <!-- std@1/regex/MatchError#WorkLimit -->
- `pub opaque type regex.Regex: copyable + inert + noalloc_drop uses {}` <!-- std@1/regex/Regex -->
- `pub type regex.Span { start: u64, end: u64, }` <!-- std@1/regex/Span -->
- `pub fn regex.compile(pattern: String, max_instructions: u64) -> Result[Regex, CompileError] uses {}` <!-- std@1/regex/compile -->
- `pub fn regex.find(re: Regex, text: String, work_limit: u64) -> Result[ Option[Span], MatchError ] uses {}` <!-- std@1/regex/find -->
- `pub fn regex.is_match(re: Regex, text: String, work_limit: u64) -> Result[bool, MatchError] uses {}` <!-- std@1/regex/is_match -->

## region

- `@noalloc fn region.with[T, R](mut r: Region[T], body: fn(root: T) -> R) -> R` <!-- core@1/region/with -->

## reservations

- `fn reservations.insert(tx: Tx, value: Reservation) -> Result[Reservation, StockError]` <!-- core@1/reservations/insert -->

## results

- `pub fn results.and_then[T, U, E]( take value: Result[T, E], take f: once fn(take T) -> Result[U, E] ) -> Result[U, E] uses {value, f}` — Chains a fallible success transformation. <!-- std@1/results/and_then -->
- `@preserves_unique(value) pub fn results.error[T, E](take value: Result[T, E]) -> Option[E] uses {value}` — Keeps the error payload and discards any success. <!-- std@1/results/error -->
- `@preserves_unique(value) pub fn results.flatten[T, E](take value: Result[Result[T, E], E]) -> Result[T, E] uses {value}` — Removes exactly one level of nesting, retaining either error. <!-- std@1/results/flatten -->
- `@noalloc pub fn results.is_error[T, E](value: Result[T, E]) -> bool uses {}` — Tests for an error without consuming the result. <!-- std@1/results/is_error -->
- `@noalloc pub fn results.is_ok[T, E](value: Result[T, E]) -> bool uses {}` — Rust's Result helpers, adapted from core::result, f6c1107f8be13e48465e12c0fd26e6a39fe18e4f. <!-- std@1/results/is_ok -->
- `pub fn results.map[T, U, E](take value: Result[T, E], take f: once fn(take T) -> U) -> Result[ U, E ] uses {value, f}` — Transforms the success payload, passing errors through unchanged. <!-- std@1/results/map -->
- `pub fn results.map_error[T, E, F](take value: Result[T, E], take f: once fn(take E) -> F) -> Result[ T, F ] uses {value, f}` — Transforms the error payload, passing successes through unchanged. <!-- std@1/results/map_error -->
- `@preserves_unique(value) pub fn results.ok[T, E](take value: Result[T, E]) -> Option[T] uses {value}` — Keeps the success payload and discards any error. <!-- std@1/results/ok -->
- `pub fn results.or_else[T, E, F]( take value: Result[T, E], take f: once fn(take E) -> Result[T, F] ) -> Result[T, F] uses {value, f}` — Recovers from an error with another result. <!-- std@1/results/or_else -->
- `@preserves_unique(value) pub fn results.unwrap_or[T, E](take value: Result[T, E], take fallback: T) -> T uses {value, fallback}` — Returns the success payload or the eagerly evaluated fallback. <!-- std@1/results/unwrap_or -->
- `pub fn results.unwrap_or_else[T, E]( take value: Result[T, E], take fallback: once fn(take E) -> T ) -> T uses {value, fallback}` — Computes the fallback from the error only on failure. <!-- std@1/results/unwrap_or_else -->

## scalar

- `pub type scalar.FormatError = BufferTooSmall()` — Shared ordinary std error types. <!-- std@1/scalar/FormatError -->
- `BufferTooSmall() -> scalar.FormatError` — Variant of scalar.FormatError. <!-- std@1/scalar/FormatError#BufferTooSmall -->
- `pub type scalar.ParseError = Invalid | OutOfRange` <!-- std@1/scalar/ParseError -->
- `Invalid -> scalar.ParseError` — Variant of scalar.ParseError. <!-- std@1/scalar/ParseError#Invalid -->
- `OutOfRange -> scalar.ParseError` — Variant of scalar.ParseError. <!-- std@1/scalar/ParseError#OutOfRange -->

## sender

- `fn sender.dup[T](tasks: Tasks, s: Sender[T]) -> Sender[T]` <!-- core@1/sender/dup -->

## set

- `@noalloc fn set.contains[T: key](s: Set[T], value: T) -> bool` <!-- core@1/set/contains -->
- `fn set.empty[T: key]() -> unique Set[T]` <!-- core@1/set/empty -->
- `@preserves_unique(s) fn set.insert[T: key](mut s: Set[T], take value: T) -> ()` <!-- core@1/set/insert -->
- `@noalloc fn set.len[T: key](s: Set[T]) -> u64` <!-- core@1/set/len -->
- `fn set.remove[T: key](mut s: unique Set[T], value: T) -> bool` <!-- core@1/set/remove -->

## sha256

- `pub opaque type sha256.State: plain + inert uses {}` — Zig crypto/sha2.zig portable SHA-256 rounds/update/final (MIT). <!-- std@1/sha256/State -->
- `@noalloc pub fn sha256.digest(data: Bytes) -> Inline[u8, 32] uses {}` <!-- std@1/sha256/digest -->
- `@noalloc pub fn sha256.finish(take state: State) -> Inline[u8, 32] uses {}` <!-- std@1/sha256/finish -->
- `@noalloc pub fn sha256.new() -> State uses {}` <!-- std@1/sha256/new -->
- `@noalloc pub fn sha256.update(mut state: State, data: Bytes) -> () uses {}` <!-- std@1/sha256/update -->

## shared

- `fn shared.freeze[T: inert](take x: unique T) -> Shared[T]` <!-- core@1/shared/freeze -->
- `fn shared.thaw[T](take s: Shared[T]) -> Result[unique T, Shared[T]]` <!-- core@1/shared/thaw -->

## simd

- `@noalloc fn simd.add[I, n: size](a: Simd[I, n], b: Simd[I, n]) -> Simd[I, n]` <!-- core@1/simd/add -->
- `@noalloc fn simd.splat[A, n: size](value: A) -> Simd[A, n]` <!-- core@1/simd/splat -->

## slots

- `fn slots.empty[T]() -> Slots[T]` <!-- core@1/slots/empty -->
- `@noalloc fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s` <!-- core@1/slots/get -->
- `@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s` <!-- core@1/slots/insert -->
- `fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]` <!-- core@1/slots/remove -->

## sorted_map

- `fn sorted_map.empty[K, V](by: Order[K]) -> unique SortedMap[K, V] of by` <!-- core@1/sorted_map/empty -->
- `fn sorted_map.get[K, V: copyable](by: Order[K], m: SortedMap[K, V] of by, key: K) -> Option[V] in m` <!-- core@1/sorted_map/get -->
- `@preserves_unique(m) @unretained(by) fn sorted_map.insert[K, V](by: Order[K], mut m: SortedMap[K, V] of by, take k: K, take v: V) -> ()` <!-- core@1/sorted_map/insert -->
- `@noalloc fn sorted_map.len[K, V](by: Order[K], m: SortedMap[K, V] of by) -> u64` <!-- core@1/sorted_map/len -->
- `fn sorted_map.remove[K, V](by: Order[K], mut m: unique SortedMap[K, V] of by, key: K) -> unique Option[V]` <!-- core@1/sorted_map/remove -->

## stdin

- `@preserves_unique(buf) @unretained(s) fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]` <!-- core@1/stdin/read -->

## stock

- `fn stock.get(tx: Tx, sku: Sku) -> Result[u32, StockError]` <!-- core@1/stock/get -->
- `fn stock.put(tx: Tx, sku: Sku, n: u32) -> Result[(), StockError]` <!-- core@1/stock/put -->

## store

- `fn store.transact[R: inert, E: inert](db: Db, body: fn(tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]` <!-- core@1/store/transact -->

## string

- `@noalloc fn string.as_bytes(s: String) -> Bytes in s` <!-- core@1/string/as_bytes -->
- `pub fn string.ascii_lower(s: String) -> unique String uses {}` <!-- std@1/string/ascii_lower -->
- `pub fn string.ascii_upper(s: String) -> unique String uses {}` <!-- std@1/string/ascii_upper -->
- `@noalloc fn string.byte_len(s: String) -> u64` <!-- core@1/string/byte_len -->
- `@noalloc fn string.byte_offset(s: String, pos: StrPos of s) -> u64` <!-- core@1/string/byte_offset -->
- `@noalloc pub fn string.compare(a: String, b: String) -> i32 uses {}` — Rust str Ord: byte-lexicographic, a proper prefix first, as '<' orders String. <!-- std@1/string/compare -->
- `pub fn string.concat(a: String, b: String) -> unique String uses {}` <!-- std@1/string/concat -->
- `@noalloc pub fn string.contains(s: String, pattern: String) -> bool uses {}` <!-- std@1/string/contains -->
- `@noalloc pub fn string.ends_with(s: String, pattern: String) -> bool uses {}` <!-- std@1/string/ends_with -->
- `@noalloc pub fn string.find(s: String, pattern: String) -> Option[StrPos of s] uses {}` <!-- std@1/string/find -->
- `@preserves_unique(bytes) fn string.from_bytes(take bytes: Bytes) -> Result[String, Bytes]` <!-- core@1/string/from_bytes -->
- `pub fn string.join(parts: List[String], separator: String) -> unique String uses {}` <!-- std@1/string/join -->
- `pub fn string.lines(take s: String) -> List[String] uses {}` — Rust str::lines: only LF and CRLF terminate lines; omit a final empty line. <!-- std@1/string/lines -->
- `@noalloc fn string.position(s: String, offset: u64) -> Option[StrPos of s]` <!-- core@1/string/position -->
- `@noalloc fn string.positions(s: String) -> PositionView of s in s` <!-- core@1/string/positions -->
- `pub fn string.replace(s: String, old: String, new: String) -> unique String uses {}` <!-- std@1/string/replace -->
- `@noalloc fn string.scalars(s: String) -> ScalarView in s` <!-- core@1/string/scalars -->
- `@noalloc fn string.slice(s: String, start: StrPos of s, end: StrPos of s) -> String in s` <!-- core@1/string/slice -->
- `pub fn string.split(take s: String, separator: String) -> List[String] uses {}` — Nonoverlapping substring delimiter; preserve leading, trailing and empty fields. <!-- std@1/string/split -->
- `pub fn string.split_ascii_whitespace(take s: String) -> List[String] uses {}` <!-- std@1/string/split_ascii_whitespace -->
- `@noalloc pub fn string.split_once(s: String, separator: String) -> Option[(String, String)] in s uses {}` — Rust str::split_once: the text before and after the first separator match. <!-- std@1/string/split_once -->
- `@noalloc pub fn string.starts_with(s: String, pattern: String) -> bool uses {}` <!-- std@1/string/starts_with -->
- `@noalloc pub fn string.trim(s: String) -> String in s uses {}` <!-- std@1/string/trim -->

## task

- `fn task.async[R: sendable](s: Scope, take f: once fn() -> R) -> Task[R] of s` <!-- core@1/task/async -->
- `fn task.await[R](s: Scope, take h: Task[R] of s) -> R` <!-- core@1/task/await -->
- `@noalloc fn task.cancel[R](s: Scope, h: Task[R] of s) -> ()` <!-- core@1/task/cancel -->
- `@noalloc fn task.check_canceled(s: Scope) -> Result[(), CancelError]` <!-- core@1/task/check_canceled -->
- `fn task.detach[R](s: Scope, take h: Task[R] of s) -> ()` <!-- core@1/task/detach -->
- `fn task.isolate[R: sendable](tasks: Tasks, take body: once fn() -> R) -> Result[R, Failure]` <!-- core@1/task/isolate -->
- `fn task.scope[R](tasks: Tasks, body: once fn(s: Scope) -> R) -> R` <!-- core@1/task/scope -->

## test

- `@noalloc pub fn test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()` — Adapted from Zig lib/std/math.zig approxEqAbs/approxEqRel and their tests, commit bff490a23889750de7096d9057c0a432efc929a8. <!-- std@1/test/approx -->
- `@noalloc fn test.check(t: Test, condition: bool) -> ()` <!-- core@1/test/check -->
- `@noalloc fn test.check_case(t: Test, ok: bool, case: u64) -> ()` <!-- core@1/test/check_case -->
- `fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir` <!-- core@1/test/dir -->
- `@noalloc pub fn test.equal[T: data](t: Test, actual: T, expected: T) -> ()` — Adapted from Zig lib/std/testing.zig expectEqualInner, commit bff490a23889750de7096d9057c0a432efc929a8. <!-- std@1/test/equal -->
- `pub fn test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E` — Adapted from Zig lib/std/testing.zig expectError, commit bff490a23889750de7096d9057c0a432efc929a8. <!-- std@1/test/expect_error -->
- `pub fn test.expect_ok[T, E](t: Test, take value: Result[T, E]) -> T` — Zig testing.zig assertion cleanup, mirrored for an unexpected Error payload. <!-- std@1/test/expect_ok -->
- `@noalloc fn test.fail(t: Test) -> Never` <!-- core@1/test/fail -->
- `pub fn test.for_all[T](t: Test, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()` — Preserve the original hundred-case entry point over the shared kernel. <!-- std@1/test/for_all -->
- `pub fn test.for_all_cases[T](t: Test, cases: u64, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()` — Adapted from Go testing/quick: an explicit count bounds the property loop. <!-- std@1/test/for_all_cases -->
- `fn test.net(t: Test, peers: List[NetPeer], listeners: List[NetListener]) -> Net` <!-- core@1/test/net -->
- `fn test.process(t: Test, runs: List[ProcessScript], children: List[ChildScript]) -> Proc` <!-- core@1/test/process -->

## text

- `@preserves_unique(builder) pub fn text.append(mut builder: List[u8], text: String) -> () uses {}` — An ordinary List[u8] is the UTF-8 builder. <!-- std@1/text/append -->
- `@preserves_unique(builder) pub fn text.finish(take builder: List[u8]) -> Result[String, Bytes] uses {}` <!-- std@1/text/finish -->
- `@preserves_unique(builder) pub fn text.finish_valid(take builder: List[u8]) -> String uses {}` — The text that appends through the formatting protocol left in an empty builder: 'format_into' and 'append' only add valid UTF-8, so every 'to_string' and every f-string finishes here; invalid bytes fail TextInvariant. <!-- std@1/text/finish_valid -->
- `@preserves_unique(out) pub fn text.format_into_spec( value: String, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` — Width and precision count Unicode scalars, following Rust Formatter::pad. <!-- std@1/text/format_into_spec -->
- `@preserves_unique(out) pub fn text.pad_from(start: u64, fill: u64, align: u64, width: u64, mut out: List[u8]) -> () uses {}` — Pads the text appended to 'out' since byte 'start' to 'width' Unicode scalars, left-aligned by default: an f-string hole of String, bool or a type with @format whose only options are fill, alignment and width. <!-- std@1/text/pad_from -->
- `@preserves_unique(out) pub fn text.pad_number_from( start: u64, fill: u64, align: u64, width: u64, mut out: List[u8] ) -> () uses {}` — The same, right-aligned by default: an integer or float hole. <!-- std@1/text/pad_number_from -->

## time

- `@serializable pub type time.Duration { nanoseconds: i128, }` — Widen ticks before subtracting so every u64 interval fits exactly. <!-- std@1/time/Duration -->
- `pub type time.SleepError = NegativeDuration | OutOfRange | Wait(WaitError)` <!-- std@1/time/SleepError -->
- `NegativeDuration -> time.SleepError` — Variant of time.SleepError. <!-- std@1/time/SleepError#NegativeDuration -->
- `OutOfRange -> time.SleepError` — Variant of time.SleepError. <!-- std@1/time/SleepError#OutOfRange -->
- `Wait(WaitError) -> time.SleepError` — Variant of time.SleepError. <!-- std@1/time/SleepError#Wait -->
- `pub type time.TimeError = OutOfRange()` <!-- std@1/time/TimeError -->
- `OutOfRange() -> time.TimeError` — Variant of time.TimeError. <!-- std@1/time/TimeError#OutOfRange -->
- `@serializable pub type time.Timestamp { unix_nanoseconds: i128, }` <!-- std@1/time/Timestamp -->
- `@noalloc pub fn time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> Duration` <!-- std@1/time/between -->
- `@noalloc pub fn time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32` <!-- std@1/time/compare -->
- `@noalloc pub fn time.deadline(clock: MonotonicClock, interval: Duration) -> Result[ Instant of clock, SleepError ]` — Relative-to-absolute conversion adapted from Zig lib/std/Io.zig Timeout.toDeadline, Clock.Timestamp.fromNow and Timeout.sleep, commit bff490a23889750de7096d9057c0a432efc929a8; copyright Zig contributors (third-party/NOTICES.md). <!-- std@1/time/deadline -->
- `@noalloc pub fn time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[ Duration, ClockError ]` <!-- std@1/time/elapsed -->
- `pub fn time.format(value: Timestamp) -> Result[unique String, TimeError] uses {}` — Adapted from Zig lib/std/time/epoch.zig EpochDay.calculateYearDay, YearAndDay.calculateMonthDay and DaySeconds, commit bff490a23889750de7096d9057c0a432efc929a8. <!-- std@1/time/format -->
- `@noalloc pub fn time.parse(text: String) -> Result[Timestamp, scalar.ParseError] uses {}` — Contract-specific RFC3339 scanner; neither Zig epoch nor Rust std exposes one. <!-- std@1/time/parse -->
- `@noalloc pub fn time.sleep(clock: MonotonicClock, interval: Duration) -> Result[(), SleepError]` <!-- std@1/time/sleep -->

## tls

- `@noalloc fn tls.close(n: Net, take s: Stream of n) -> Result[(), NetError]` <!-- core@1/tls/close -->
- `fn tls.connect(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots) -> Result[Stream of n, NetError]` — on a 'ConnectTls' stream, 'server_name' equal to the rule's address <!-- core@1/tls/connect -->

## u128

- `@noalloc pub fn u128.checked_add(a: u128, b: u128) -> Option[u128] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/u128/checked_add -->
- `@noalloc pub fn u128.checked_mul(a: u128, b: u128) -> Option[u128] uses {}` <!-- std@1/u128/checked_mul -->
- `@noalloc pub fn u128.checked_sub(a: u128, b: u128) -> Option[u128] uses {}` <!-- std@1/u128/checked_sub -->
- `@noalloc pub fn u128.format[n: size](value: u128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/u128/format -->
- `@preserves_unique(out) pub fn u128.format_into(value: u128, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/u128/format_into -->
- `@preserves_unique(out) pub fn u128.format_into_spec( value: u128, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/u128/format_into_spec -->
- `pub fn u128.format_radix(value: u128, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/u128/format_radix -->
- `const u128.order: Order[u128]` <!-- core@1/u128/order -->
- `@noalloc pub fn u128.parse(text: String) -> Result[u128, scalar.ParseError] uses {}` <!-- std@1/u128/parse -->
- `@noalloc pub fn u128.parse_radix(text: String, radix: u8) -> Result[u128, scalar.ParseError] uses {}` <!-- std@1/u128/parse_radix -->
- `@noalloc pub fn u128.saturating_add(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/saturating_add -->
- `@noalloc pub fn u128.saturating_mul(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/saturating_mul -->
- `@noalloc pub fn u128.saturating_sub(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/saturating_sub -->
- `pub fn u128.to_string(value: u128) -> unique String uses {}` <!-- std@1/u128/to_string -->
- `@noalloc pub fn u128.wrapping_add(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/wrapping_add -->
- `@noalloc pub fn u128.wrapping_mul(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/wrapping_mul -->
- `@noalloc pub fn u128.wrapping_sub(a: u128, b: u128) -> u128 uses {}` <!-- std@1/u128/wrapping_sub -->

## u16

- `@noalloc pub fn u16.checked_add(a: u16, b: u16) -> Option[u16] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/u16/checked_add -->
- `@noalloc pub fn u16.checked_mul(a: u16, b: u16) -> Option[u16] uses {}` <!-- std@1/u16/checked_mul -->
- `@noalloc pub fn u16.checked_sub(a: u16, b: u16) -> Option[u16] uses {}` <!-- std@1/u16/checked_sub -->
- `@noalloc pub fn u16.format[n: size](value: u16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/u16/format -->
- `@preserves_unique(out) pub fn u16.format_into(value: u16, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/u16/format_into -->
- `@preserves_unique(out) pub fn u16.format_into_spec( value: u16, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/u16/format_into_spec -->
- `pub fn u16.format_radix(value: u16, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/u16/format_radix -->
- `const u16.order: Order[u16]` <!-- core@1/u16/order -->
- `@noalloc pub fn u16.parse(text: String) -> Result[u16, scalar.ParseError] uses {}` <!-- std@1/u16/parse -->
- `@noalloc pub fn u16.parse_radix(text: String, radix: u8) -> Result[u16, scalar.ParseError] uses {}` <!-- std@1/u16/parse_radix -->
- `@noalloc pub fn u16.saturating_add(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/saturating_add -->
- `@noalloc pub fn u16.saturating_mul(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/saturating_mul -->
- `@noalloc pub fn u16.saturating_sub(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/saturating_sub -->
- `pub fn u16.to_string(value: u16) -> unique String uses {}` <!-- std@1/u16/to_string -->
- `@noalloc pub fn u16.wrapping_add(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/wrapping_add -->
- `@noalloc pub fn u16.wrapping_mul(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/wrapping_mul -->
- `@noalloc pub fn u16.wrapping_sub(a: u16, b: u16) -> u16 uses {}` <!-- std@1/u16/wrapping_sub -->

## u32

- `@noalloc pub fn u32.checked_add(a: u32, b: u32) -> Option[u32] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/u32/checked_add -->
- `@noalloc pub fn u32.checked_mul(a: u32, b: u32) -> Option[u32] uses {}` <!-- std@1/u32/checked_mul -->
- `@noalloc pub fn u32.checked_sub(a: u32, b: u32) -> Option[u32] uses {}` <!-- std@1/u32/checked_sub -->
- `@noalloc pub fn u32.format[n: size](value: u32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/u32/format -->
- `@preserves_unique(out) pub fn u32.format_into(value: u32, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/u32/format_into -->
- `@preserves_unique(out) pub fn u32.format_into_spec( value: u32, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/u32/format_into_spec -->
- `pub fn u32.format_radix(value: u32, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/u32/format_radix -->
- `const u32.order: Order[u32]` <!-- core@1/u32/order -->
- `@noalloc pub fn u32.parse(text: String) -> Result[u32, scalar.ParseError] uses {}` <!-- std@1/u32/parse -->
- `@noalloc pub fn u32.parse_radix(text: String, radix: u8) -> Result[u32, scalar.ParseError] uses {}` <!-- std@1/u32/parse_radix -->
- `@noalloc pub fn u32.saturating_add(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/saturating_add -->
- `@noalloc pub fn u32.saturating_mul(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/saturating_mul -->
- `@noalloc pub fn u32.saturating_sub(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/saturating_sub -->
- `pub fn u32.to_string(value: u32) -> unique String uses {}` <!-- std@1/u32/to_string -->
- `@noalloc pub fn u32.wrapping_add(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/wrapping_add -->
- `@noalloc pub fn u32.wrapping_mul(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/wrapping_mul -->
- `@noalloc pub fn u32.wrapping_sub(a: u32, b: u32) -> u32 uses {}` <!-- std@1/u32/wrapping_sub -->

## u64

- `@noalloc pub fn u64.checked_add(a: u64, b: u64) -> Option[u64] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/u64/checked_add -->
- `@noalloc pub fn u64.checked_mul(a: u64, b: u64) -> Option[u64] uses {}` <!-- std@1/u64/checked_mul -->
- `@noalloc pub fn u64.checked_sub(a: u64, b: u64) -> Option[u64] uses {}` <!-- std@1/u64/checked_sub -->
- `@noalloc pub fn u64.format[n: size](value: u64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/u64/format -->
- `@preserves_unique(out) pub fn u64.format_into(value: u64, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/u64/format_into -->
- `@preserves_unique(out) pub fn u64.format_into_spec( value: u64, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/u64/format_into_spec -->
- `pub fn u64.format_radix(value: u64, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/u64/format_radix -->
- `const u64.order: Order[u64]` <!-- core@1/u64/order -->
- `@noalloc pub fn u64.parse(text: String) -> Result[u64, scalar.ParseError] uses {}` <!-- std@1/u64/parse -->
- `@noalloc pub fn u64.parse_radix(text: String, radix: u8) -> Result[u64, scalar.ParseError] uses {}` <!-- std@1/u64/parse_radix -->
- `@noalloc pub fn u64.saturating_add(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/saturating_add -->
- `@noalloc pub fn u64.saturating_mul(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/saturating_mul -->
- `@noalloc pub fn u64.saturating_sub(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/saturating_sub -->
- `pub fn u64.to_string(value: u64) -> unique String uses {}` <!-- std@1/u64/to_string -->
- `@noalloc pub fn u64.wrapping_add(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/wrapping_add -->
- `@noalloc pub fn u64.wrapping_mul(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/wrapping_mul -->
- `@noalloc pub fn u64.wrapping_sub(a: u64, b: u64) -> u64 uses {}` <!-- std@1/u64/wrapping_sub -->

## u8

- `@noalloc pub fn u8.checked_add(a: u8, b: u8) -> Option[u8] uses {}` — Generated by tools/generate-integer-ops.py; edit the template there. <!-- std@1/u8/checked_add -->
- `@noalloc pub fn u8.checked_mul(a: u8, b: u8) -> Option[u8] uses {}` <!-- std@1/u8/checked_mul -->
- `@noalloc pub fn u8.checked_sub(a: u8, b: u8) -> Option[u8] uses {}` <!-- std@1/u8/checked_sub -->
- `@noalloc pub fn u8.format[n: size](value: u8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}` — Decimal conversion is ordinary source; no numeric schema or hidden allocator. <!-- std@1/u8/format -->
- `@preserves_unique(out) pub fn u8.format_into(value: u8, mut out: List[u8]) -> () uses {}` — Appends the decimal text of 'value' to 'out'; to_string is that text, built from the same digits in a list reserved to its exact length. <!-- std@1/u8/format_into -->
- `@preserves_unique(out) pub fn u8.format_into_spec( value: u8, fill: u64, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}` <!-- std@1/u8/format_into_spec -->
- `pub fn u8.format_radix(value: u8, radix: u8) -> Result[unique String, scalar.ParseError] uses {}` <!-- std@1/u8/format_radix -->
- `const u8.order: Order[u8]` <!-- core@1/u8/order -->
- `@noalloc pub fn u8.parse(text: String) -> Result[u8, scalar.ParseError] uses {}` <!-- std@1/u8/parse -->
- `@noalloc pub fn u8.parse_radix(text: String, radix: u8) -> Result[u8, scalar.ParseError] uses {}` <!-- std@1/u8/parse_radix -->
- `@noalloc pub fn u8.saturating_add(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/saturating_add -->
- `@noalloc pub fn u8.saturating_mul(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/saturating_mul -->
- `@noalloc pub fn u8.saturating_sub(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/saturating_sub -->
- `pub fn u8.to_string(value: u8) -> unique String uses {}` <!-- std@1/u8/to_string -->
- `@noalloc pub fn u8.wrapping_add(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/wrapping_add -->
- `@noalloc pub fn u8.wrapping_mul(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/wrapping_mul -->
- `@noalloc pub fn u8.wrapping_sub(a: u8, b: u8) -> u8 uses {}` <!-- std@1/u8/wrapping_sub -->

## unicode

- `pub opaque type unicode.GraphemeCursor: copyable + inert + noalloc_drop uses {}` — Public signature type; representation and associated operations stay private. <!-- std@1/unicode/GraphemeCursor -->
- `@loop_source(graphemes_open, graphemes_next) pub opaque type unicode.GraphemeView: copyable + inert + noalloc_drop uses {}` — Default extended grapheme clusters, Unicode16 UAX #29 revision 45. <!-- std@1/unicode/GraphemeView -->
- `@noalloc pub fn unicode.graphemes(s: String) -> GraphemeView in s` — Adapted from src/grapheme.rs::new_graphemes at unicode-segmentation ff47b3187adddb14813e6fb2cfc1a079f58cfddc. <!-- std@1/unicode/graphemes -->
- `@noalloc pub fn unicode.is_digit(c: char) -> bool uses {}` <!-- std@1/unicode/is_digit -->
- `@noalloc pub fn unicode.is_letter(c: char) -> bool uses {}` <!-- std@1/unicode/is_letter -->
- `@noalloc pub fn unicode.is_whitespace(c: char) -> bool uses {}` <!-- std@1/unicode/is_whitespace -->
- `pub fn unicode.normalize_nfc(s: String) -> unique String uses {}` — Explicit NFC: compatibility mappings never enter canonical_pair. <!-- std@1/unicode/normalize_nfc -->
- `@noalloc pub fn unicode.simple_lower(c: char) -> char uses {}` <!-- std@1/unicode/simple_lower -->
- `@noalloc pub fn unicode.simple_upper(c: char) -> char uses {}` <!-- std@1/unicode/simple_upper -->

## uri

- `pub type uri.Component = Raw(String) | Encoded(String)` — Port of Zig lib/std/Uri.zig, bff490a23889750de7096d9057c0a432efc929a8 (MIT; third-party/NOTICES.md): parse[AfterScheme], resolveInPlace, remove_dot_segments, Component.percentEncode/formatRaw and writeToStream. <!-- std@1/uri/Component -->
- `Encoded(String) -> uri.Component` — Variant of uri.Component. <!-- std@1/uri/Component#Encoded -->
- `Raw(String) -> uri.Component` — Variant of uri.Component. <!-- std@1/uri/Component#Raw -->
- `pub type uri.Part = Unreserved | User | Password | Host | Path | Query | Fragment` <!-- std@1/uri/Part -->
- `Fragment -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Fragment -->
- `Host -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Host -->
- `Password -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Password -->
- `Path -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Path -->
- `Query -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Query -->
- `Unreserved -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#Unreserved -->
- `User -> uri.Part` — Variant of uri.Part. <!-- std@1/uri/Part#User -->
- `pub type uri.QueryError = InvalidEscape(u64) | InvalidSeparator(u64) | InvalidUtf8(u64)` — Go net/url parseQuery and QueryUnescape, adapted to ordered, owned pairs. <!-- std@1/uri/QueryError -->
- `InvalidEscape(u64) -> uri.QueryError` — Variant of uri.QueryError. <!-- std@1/uri/QueryError#InvalidEscape -->
- `InvalidSeparator(u64) -> uri.QueryError` — Variant of uri.QueryError. <!-- std@1/uri/QueryError#InvalidSeparator -->
- `InvalidUtf8(u64) -> uri.QueryError` — Variant of uri.QueryError. <!-- std@1/uri/QueryError#InvalidUtf8 -->
- `pub type uri.Uri { scheme: String, user: Option[Component], password: Option[Component], host: Option[Component], port: Option[u16], path: Component, query: Option[Component], fragment: Option[Component], }` <!-- std@1/uri/Uri -->
- `pub type uri.UriError = UnexpectedCharacter | InvalidFormat | InvalidPort | InvalidHostName` <!-- std@1/uri/UriError -->
- `InvalidFormat -> uri.UriError` — Variant of uri.UriError. <!-- std@1/uri/UriError#InvalidFormat -->
- `InvalidHostName -> uri.UriError` — Variant of uri.UriError. <!-- std@1/uri/UriError#InvalidHostName -->
- `InvalidPort -> uri.UriError` — Variant of uri.UriError. <!-- std@1/uri/UriError#InvalidPort -->
- `UnexpectedCharacter -> uri.UriError` — Variant of uri.UriError. <!-- std@1/uri/UriError#UnexpectedCharacter -->
- `pub fn uri.component_encoded(component: Component, part: Part) -> unique String uses {}` <!-- std@1/uri/component_encoded -->
- `pub fn uri.component_raw(component: Component) -> unique Bytes uses {}` <!-- std@1/uri/component_raw -->
- `pub fn uri.component_text(component: Component) -> Result[unique String, unique Bytes] uses {}` <!-- std@1/uri/component_text -->
- `pub fn uri.encode_query(pairs: List[(String, String)]) -> unique String uses {}` — Go QueryEscape's alphabet; pair order and repeated keys are preserved. <!-- std@1/uri/encode_query -->
- `pub fn uri.format(parsed_uri: Uri) -> unique String uses {}` — Zig writeToStream(.all): an empty path is rendered as '/'. <!-- std@1/uri/format -->
- `pub fn uri.parse(text: String) -> Result[Uri, UriError] uses {}` <!-- std@1/uri/parse -->
- `pub fn uri.percent_decode(text: String) -> unique Bytes uses {}` — Zig formatRaw: invalid or incomplete escapes stay literal; '+' stays '+'. <!-- std@1/uri/percent_decode -->
- `pub fn uri.percent_encode(data: Bytes) -> unique String uses {}` <!-- std@1/uri/percent_encode -->
- `pub fn uri.query_pairs(query: String) -> Result[unique List[(unique String, unique String)], QueryError] uses {}` — The input is the query without '?', or an application/x-www-form-urlencoded body. <!-- std@1/uri/query_pairs -->
- `pub fn uri.remove_dot_segments(path: String) -> unique String uses {}` — Zig remove_dot_segments, RFC3986 §5.2.4. <!-- std@1/uri/remove_dot_segments -->
- `pub fn uri.resolve(base: Uri, reference: String) -> Result[Uri, UriError] uses {}` — Zig resolveInPlace and merge_paths; the result owns its copied base components. <!-- std@1/uri/resolve -->

## utf8

- `pub fn utf8.decode_lossy(data: Bytes) -> unique String uses {}` — Rust str validation/lossy error-span semantics. <!-- std@1/utf8/decode_lossy -->
