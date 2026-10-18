# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

832 exported definitions; 266 variants.

## Types and roots

<a id="def-core_401_2fArena"></a>

- `@capability(local) opaque type Arena: noalloc_drop uses {Arena}`

<a id="def-core_401_2fArgError"></a>

- `type ArgError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = MissingArgument() | InvalidUtf8()`

<a id="def-core_401_2fArgError_23InvalidUtf8"></a>

- `InvalidUtf8() -> ArgError`

Variant of ArgError.

<a id="def-core_401_2fArgError_23MissingArgument"></a>

- `MissingArgument() -> ArgError`

Variant of ArgError.

<a id="def-core_401_2fArgs"></a>

- `@capability(global) opaque type Args: plain + copyable + sendable + noalloc_drop uses {Args}`

<a id="def-core_401_2fArray"></a>

- `type Array[T] uses {T}`

<a id="def-core_401_2fAtomic"></a>

- `type Atomic[A] uses {Tasks, A}`

<a id="def-core_401_2fAtomicFile"></a>

- `resource type AtomicFile: noalloc_drop uses {Dir} = { dir: Dir, tmp: Path, dst: Path }`

<a id="def-core_401_2fBody"></a>

- `opaque type Body[F]: copyable + inert + noalloc_drop uses {}`

<a id="def-core_401_2fBounded"></a>

- `type Bounded[R] uses {R} = Finished(R) | Expired(R)`

<a id="def-core_401_2fBounded_23Expired"></a>

- `Expired(R) -> Bounded[R]`

Variant of Bounded.

<a id="def-core_401_2fBounded_23Finished"></a>

- `Finished(R) -> Bounded[R]`

Variant of Bounded.

<a id="def-core_401_2fBytes"></a>

- `type Bytes: copyable + data + inert + noalloc_drop + key uses {}`

<a id="def-core_401_2fCancelCause"></a>

- `type CancelCause: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Requested() | Parent() | ChildFailed() | DeadlineExceeded()`

<a id="def-core_401_2fCancelCause_23ChildFailed"></a>

- `ChildFailed() -> CancelCause`

Variant of CancelCause.

<a id="def-core_401_2fCancelCause_23DeadlineExceeded"></a>

- `DeadlineExceeded() -> CancelCause`

Variant of CancelCause.

<a id="def-core_401_2fCancelCause_23Parent"></a>

- `Parent() -> CancelCause`

Variant of CancelCause.

<a id="def-core_401_2fCancelCause_23Requested"></a>

- `Requested() -> CancelCause`

Variant of CancelCause.

<a id="def-core_401_2fCancelError"></a>

- `type CancelError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Canceled(CancelCause)`

<a id="def-core_401_2fCancelError_23Canceled"></a>

- `Canceled(CancelCause) -> CancelError`

Variant of CancelError.

<a id="def-core_401_2fChannelError"></a>

- `type ChannelError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Closed() | Canceled(CancelCause)`

<a id="def-core_401_2fChannelError_23Canceled"></a>

- `Canceled(CancelCause) -> ChannelError`

Variant of ChannelError.

<a id="def-core_401_2fChannelError_23Closed"></a>

- `Closed() -> ChannelError`

Variant of ChannelError.

<a id="def-core_401_2fCheckRule"></a>

- `type CheckRule: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Arithmetic() | TextInvariant() | Bounds()`

<a id="def-core_401_2fCheckRule_23Arithmetic"></a>

- `Arithmetic() -> CheckRule`

Variant of CheckRule.

<a id="def-core_401_2fCheckRule_23Bounds"></a>

- `Bounds() -> CheckRule`

Variant of CheckRule.

<a id="def-core_401_2fCheckRule_23TextInvariant"></a>

- `TextInvariant() -> CheckRule`

Variant of CheckRule.

<a id="def-core_401_2fChild"></a>

- `opaque resource type Child: sendable + noalloc_drop uses {Proc}`

<a id="def-core_401_2fChildScript"></a>

- `type ChildScript uses {} = { program: String, args: List[String], steps: List[ChildStep] }`

<a id="def-core_401_2fChildStep"></a>

- `type ChildStep uses {} = Print(Bytes) | PrintErr(Bytes) | Expect(Bytes) | Exit(ExitStatus)`

<a id="def-core_401_2fChildStep_23Exit"></a>

- `Exit(ExitStatus) -> ChildStep`

Variant of ChildStep.

<a id="def-core_401_2fChildStep_23Expect"></a>

- `Expect(Bytes) -> ChildStep`

Variant of ChildStep.

<a id="def-core_401_2fChildStep_23Print"></a>

- `Print(Bytes) -> ChildStep`

Variant of ChildStep.

<a id="def-core_401_2fChildStep_23PrintErr"></a>

- `PrintErr(Bytes) -> ChildStep`

Variant of ChildStep.

<a id="def-core_401_2fCliError"></a>

- `type CliError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Usage(ArgError) | Files(FileError) | Output(OutputError)`

<a id="def-core_401_2fCliError_23Files"></a>

- `Files(FileError) -> CliError`

Variant of CliError.

<a id="def-core_401_2fCliError_23Output"></a>

- `Output(OutputError) -> CliError`

Variant of CliError.

<a id="def-core_401_2fCliError_23Usage"></a>

- `Usage(ArgError) -> CliError`

Variant of CliError.

<a id="def-core_401_2fClockError"></a>

- `type ClockError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unavailable() | OutOfRange()`

<a id="def-core_401_2fClockError_23OutOfRange"></a>

- `OutOfRange() -> ClockError`

Variant of ClockError.

<a id="def-core_401_2fClockError_23Unavailable"></a>

- `Unavailable() -> ClockError`

Variant of ClockError.

<a id="def-core_401_2fDb"></a>

- `@capability(global) opaque type Db: plain + copyable + sendable + noalloc_drop uses {Db}`

<a id="def-core_401_2fDeadline"></a>

- `type Deadline: plain + copyable + sendable + noalloc_drop uses {MonotonicClock} = { clock: MonotonicClock, at: Instant of .clock }`

<a id="def-core_401_2fDecodeError"></a>

- `type DecodeError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Version() | SchemaMismatch() | Truncated() | Trailing() | Scalar() | Utf8() | Tag() | Length() | Duplicate() | Order()`

<a id="def-core_401_2fDecodeError_23Duplicate"></a>

- `Duplicate() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Length"></a>

- `Length() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Order"></a>

- `Order() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Scalar"></a>

- `Scalar() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23SchemaMismatch"></a>

- `SchemaMismatch() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Tag"></a>

- `Tag() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Trailing"></a>

- `Trailing() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Truncated"></a>

- `Truncated() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Utf8"></a>

- `Utf8() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Version"></a>

- `Version() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDir"></a>

- `@capability(global) opaque type Dir: plain + copyable + sendable + noalloc_drop uses {Dir}`

<a id="def-core_401_2fDirEntry"></a>

- `type DirEntry uses {} = { path: Path, kind: FileKind }`

<a id="def-core_401_2fEnv"></a>

- `@capability(global) opaque type Env: plain + copyable + sendable + noalloc_drop uses {Env}`

<a id="def-core_401_2fEnvError"></a>

- `type EnvError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | InvalidUtf8()`

<a id="def-core_401_2fEnvError_23InvalidUtf8"></a>

- `InvalidUtf8() -> EnvError`

Variant of EnvError.

<a id="def-core_401_2fEnvError_23NotGranted"></a>

- `NotGranted() -> EnvError`

Variant of EnvError.

<a id="def-core_401_2fEq"></a>

- `type Eq[T] uses {} = { equal: fn(T, T) -> bool uses {} }`

<a id="def-core_401_2fExitStatus"></a>

- `type ExitStatus: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Exited(i32) | Signaled(i32)`

<a id="def-core_401_2fExitStatus_23Exited"></a>

- `Exited(i32) -> ExitStatus`

Variant of ExitStatus.

<a id="def-core_401_2fExitStatus_23Signaled"></a>

- `Signaled(i32) -> ExitStatus`

Variant of ExitStatus.

<a id="def-core_401_2fFailure"></a>

- `type Failure uses {} = { kind: FailureKind, rule: String, site: SourceSite, spawn: List[u64], cause: Option[box Failure] }`

<a id="def-core_401_2fFailureKind"></a>

- `type FailureKind: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Check() | Contract() | Depth() | OutOfSpace() | DeadMutex()`

<a id="def-core_401_2fFailureKind_23Check"></a>

- `Check() -> FailureKind`

Variant of FailureKind.

<a id="def-core_401_2fFailureKind_23Contract"></a>

- `Contract() -> FailureKind`

Variant of FailureKind.

<a id="def-core_401_2fFailureKind_23DeadMutex"></a>

- `DeadMutex() -> FailureKind`

Variant of FailureKind.

<a id="def-core_401_2fFailureKind_23Depth"></a>

- `Depth() -> FailureKind`

Variant of FailureKind.

<a id="def-core_401_2fFailureKind_23OutOfSpace"></a>

- `OutOfSpace() -> FailureKind`

Variant of FailureKind.

<a id="def-core_401_2fFieldShape"></a>

- `type FieldShape: copyable + data + key + inert + noalloc_drop uses {} = { name: String, ty: String }`

<a id="def-core_401_2fFile"></a>

- `opaque resource type File: noalloc_drop uses {Dir}`

<a id="def-core_401_2fFileError"></a>

- `type FileError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory() | DirectoryNotEmpty() | CrossDevice()`

<a id="def-core_401_2fFileError_23AlreadyExists"></a>

- `AlreadyExists() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23Canceled"></a>

- `Canceled(CancelCause) -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23CrossDevice"></a>

- `CrossDevice() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23DirectoryNotEmpty"></a>

- `DirectoryNotEmpty() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23FileFault"></a>

- `FileFault(u32) -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23IsDirectory"></a>

- `IsDirectory() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23NotDirectory"></a>

- `NotDirectory() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23NotFound"></a>

- `NotFound() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23NotGranted"></a>

- `NotGranted() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileError_23PermissionDenied"></a>

- `PermissionDenied() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileKind"></a>

- `type FileKind: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Regular() | Directory() | Symlink() | Other()`

<a id="def-core_401_2fFileKind_23Directory"></a>

- `Directory() -> FileKind`

Variant of FileKind.

<a id="def-core_401_2fFileKind_23Other"></a>

- `Other() -> FileKind`

Variant of FileKind.

<a id="def-core_401_2fFileKind_23Regular"></a>

- `Regular() -> FileKind`

Variant of FileKind.

<a id="def-core_401_2fFileKind_23Symlink"></a>

- `Symlink() -> FileKind`

Variant of FileKind.

<a id="def-core_401_2fFloatIntError"></a>

- `type FloatIntError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotFinite() | OutOfRange()`

<a id="def-core_401_2fFloatIntError_23NotFinite"></a>

- `NotFinite() -> FloatIntError`

Variant of FloatIntError.

<a id="def-core_401_2fFloatIntError_23OutOfRange"></a>

- `OutOfRange() -> FloatIntError`

Variant of FloatIntError.

<a id="def-core_401_2fGrantError"></a>

- `type GrantError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted()`

<a id="def-core_401_2fGrantError_23NotGranted"></a>

- `NotGranted() -> GrantError`

Variant of GrantError.

<a id="def-core_401_2fHasher"></a>

- `type Hasher[T] uses {} = { hash: fn(T, u64) -> u64 uses {} }`

<a id="def-core_401_2fInline"></a>

- `type Inline[T, n: size] uses {T}`

<a id="def-core_401_2fInputError"></a>

- `type InputError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()`

<a id="def-core_401_2fInputError_23Canceled"></a>

- `Canceled(CancelCause) -> InputError`

Variant of InputError.

<a id="def-core_401_2fInputError_23InputFault"></a>

- `InputFault(u32) -> InputError`

Variant of InputError.

<a id="def-core_401_2fInputError_23InvalidInput"></a>

- `InvalidInput() -> InputError`

Variant of InputError.

<a id="def-core_401_2fInputError_23InvalidUtf8"></a>

- `InvalidUtf8() -> InputError`

Variant of InputError.

<a id="def-core_401_2fInstant"></a>

- `opaque type Instant: copyable + inert + noalloc_drop + plain + sendable uses {}`

<a id="def-core_401_2fIo"></a>

- `@capability(global) opaque type Io: plain + copyable + sendable + noalloc_drop uses {Io}`

<a id="def-core_401_2fIpAddr"></a>

- `type IpAddr: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = V4(Inline[u8, 4]) | V6(Inline[u8, 16])`

<a id="def-core_401_2fIpAddr_23V4"></a>

- `V4(Inline[u8, 4]) -> IpAddr`

Variant of IpAddr.

<a id="def-core_401_2fIpAddr_23V6"></a>

- `V6(Inline[u8, 16]) -> IpAddr`

Variant of IpAddr.

<a id="def-core_401_2fIter"></a>

- `type Iter[T]`

<a id="def-core_401_2fList"></a>

- `type List[T] uses {T}`

<a id="def-core_401_2fListener"></a>

- `opaque resource type Listener: sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fMap"></a>

- `type Map[K: key, V] uses {K, V}`

<a id="def-core_401_2fMetadata"></a>

- `type Metadata uses {} = { kind: FileKind, size: u64, modified_unix_ns: Option[i128], permissions: u32 }`

<a id="def-core_401_2fMonotonicClock"></a>

- `@capability(global) opaque type MonotonicClock: plain + copyable + sendable + noalloc_drop uses {MonotonicClock}`

<a id="def-core_401_2fMutex"></a>

- `type Mutex[T] uses {Tasks, T}`

<a id="def-core_401_2fNet"></a>

- `@capability(global) opaque type Net: plain + copyable + sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fNetClient"></a>

- `type NetClient uses {} = { from: SocketAddr, steps: List[NetStep] }`

<a id="def-core_401_2fNetConnection"></a>

- `type NetConnection uses {} = Refused() | Unanswered() | Open(List[NetStep])`

<a id="def-core_401_2fNetConnection_23Open"></a>

- `Open(List[NetStep]) -> NetConnection`

Variant of NetConnection.

<a id="def-core_401_2fNetConnection_23Refused"></a>

- `Refused() -> NetConnection`

Variant of NetConnection.

<a id="def-core_401_2fNetConnection_23Unanswered"></a>

- `Unanswered() -> NetConnection`

Variant of NetConnection.

<a id="def-core_401_2fNetError"></a>

- `type NetError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | Canceled(CancelCause) | InvalidInput() | UnknownHost() | ResolverFailure() | ConnectionRefused() | ConnectionReset() | ConnectionAborted() | NotConnected() | AddrInUse() | AddrNotAvailable() | HostUnreachable() | NetworkUnreachable() | NetworkDown() | BrokenPipe() | TimedOut() | Tls(TlsFailure) | Fault(u32)`

<a id="def-core_401_2fNetError_23AddrInUse"></a>

- `AddrInUse() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23AddrNotAvailable"></a>

- `AddrNotAvailable() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23BrokenPipe"></a>

- `BrokenPipe() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23Canceled"></a>

- `Canceled(CancelCause) -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23ConnectionAborted"></a>

- `ConnectionAborted() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23ConnectionRefused"></a>

- `ConnectionRefused() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23ConnectionReset"></a>

- `ConnectionReset() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23Fault"></a>

- `Fault(u32) -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23HostUnreachable"></a>

- `HostUnreachable() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23InvalidInput"></a>

- `InvalidInput() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23NetworkDown"></a>

- `NetworkDown() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23NetworkUnreachable"></a>

- `NetworkUnreachable() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23NotConnected"></a>

- `NotConnected() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23NotGranted"></a>

- `NotGranted() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23ResolverFailure"></a>

- `ResolverFailure() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23TimedOut"></a>

- `TimedOut() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23Tls"></a>

- `Tls(TlsFailure) -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetError_23UnknownHost"></a>

- `UnknownHost() -> NetError`

Variant of NetError.

<a id="def-core_401_2fNetListener"></a>

- `type NetListener uses {} = { address: SocketAddr, clients: List[NetClient] }`

<a id="def-core_401_2fNetOperation"></a>

- `type NetOperation: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Connect() | ConnectTls() | Listen() | UdpBind() | UdpPeer()`

<a id="def-core_401_2fNetOperation_23Connect"></a>

- `Connect() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23ConnectTls"></a>

- `ConnectTls() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23Listen"></a>

- `Listen() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UdpBind"></a>

- `UdpBind() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UdpPeer"></a>

- `UdpPeer() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetPeer"></a>

- `type NetPeer uses {} = { address: String, port: u16, resolves_to: List[IpAddr], tls: PeerTls, connections: List[NetConnection] }`

<a id="def-core_401_2fNetRule"></a>

- `type NetRule uses {} = { address: String, port: u16, operation: NetOperation }`

<a id="def-core_401_2fNetStep"></a>

- `type NetStep uses {} = Send(Bytes) | Expect(Bytes) | Stall() | Close() | Reset() | Truncate()`

<a id="def-core_401_2fNetStep_23Close"></a>

- `Close() -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetStep_23Expect"></a>

- `Expect(Bytes) -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetStep_23Reset"></a>

- `Reset() -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetStep_23Send"></a>

- `Send(Bytes) -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetStep_23Stall"></a>

- `Stall() -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetStep_23Truncate"></a>

- `Truncate() -> NetStep`

Variant of NetStep.

<a id="def-core_401_2fNetTimeout"></a>

- `type NetTimeout: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unbounded() | Nanos(u64)`

<a id="def-core_401_2fNetTimeout_23Nanos"></a>

- `Nanos(u64) -> NetTimeout`

Variant of NetTimeout.

<a id="def-core_401_2fNetTimeout_23Unbounded"></a>

- `Unbounded() -> NetTimeout`

Variant of NetTimeout.

<a id="def-core_401_2fOption"></a>

- `type Option[T] uses {T} = None() | Some(T)`

<a id="def-core_401_2fOption_23None"></a>

- `None() -> Option[T]`

Variant of Option.

<a id="def-core_401_2fOption_23Some"></a>

- `Some(T) -> Option[T]`

Variant of Option.

<a id="def-core_401_2fOrder"></a>

- `type Order[T] uses {} = { compare: fn(T, T) -> i32 uses {} }`

<a id="def-core_401_2fOutOfSpace"></a>

- `type OutOfSpace: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutOfSpace()`

<a id="def-core_401_2fOutOfSpace_23OutOfSpace"></a>

- `OutOfSpace() -> OutOfSpace`

Variant of OutOfSpace.

<a id="def-core_401_2fOutputError"></a>

- `type OutputError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutputFault(u32) | Canceled(CancelCause)`

<a id="def-core_401_2fOutputError_23Canceled"></a>

- `Canceled(CancelCause) -> OutputError`

Variant of OutputError.

<a id="def-core_401_2fOutputError_23OutputFault"></a>

- `OutputFault(u32) -> OutputError`

Variant of OutputError.

<a id="def-core_401_2fPacked"></a>

- `type Packed[T] uses {T}`

<a id="def-core_401_2fPath"></a>

- `opaque type Path: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}`

<a id="def-core_401_2fPayloadShape"></a>

- `type PayloadShape: copyable + data + key + inert + noalloc_drop uses {} = { label: Option[String], ty: String }`

<a id="def-core_401_2fPeerTls"></a>

- `type PeerTls uses {} = Plain() | Certificate(List[String]) | Fails(TlsFailure)`

<a id="def-core_401_2fPeerTls_23Certificate"></a>

- `Certificate(List[String]) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Fails"></a>

- `Fails(TlsFailure) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Plain"></a>

- `Plain() -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPositionView"></a>

- `opaque type PositionView: copyable + inert + noalloc_drop uses {}`

<a id="def-core_401_2fProc"></a>

- `@capability(global) opaque type Proc: plain + copyable + sendable + noalloc_drop uses {Proc}`

<a id="def-core_401_2fProcessError"></a>

- `type ProcessError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = NotGranted() | InvalidInput() | NotFound() | PermissionDenied() | OutputLimit() | Fault(u32) | TimedOut() | Canceled(CancelCause) | InputClosed()`

<a id="def-core_401_2fProcessError_23Canceled"></a>

- `Canceled(CancelCause) -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23Fault"></a>

- `Fault(u32) -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23InputClosed"></a>

- `InputClosed() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23InvalidInput"></a>

- `InvalidInput() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23NotFound"></a>

- `NotFound() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23NotGranted"></a>

- `NotGranted() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23OutputLimit"></a>

- `OutputLimit() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23PermissionDenied"></a>

- `PermissionDenied() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessError_23TimedOut"></a>

- `TimedOut() -> ProcessError`

Variant of ProcessError.

<a id="def-core_401_2fProcessOutput"></a>

- `type ProcessOutput uses {} = { status: ExitStatus, stdout: Bytes, stderr: Bytes }`

<a id="def-core_401_2fProcessScript"></a>

- `type ProcessScript uses {} = { program: String, args: List[String], outcome: Result[ProcessOutput, ProcessError] }`

<a id="def-core_401_2fPtr"></a>

- `type Ptr[T] uses {}`

<a id="def-core_401_2fRandom"></a>

- `@capability(global) opaque type Random: plain + copyable + sendable + noalloc_drop uses {Random}`

<a id="def-core_401_2fRandomError"></a>

- `type RandomError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Unavailable()`

<a id="def-core_401_2fRandomError_23Unavailable"></a>

- `Unavailable() -> RandomError`

Variant of RandomError.

<a id="def-core_401_2fReceiver"></a>

- `type Receiver[T] uses {Tasks, T}`

<a id="def-core_401_2fRegion"></a>

- `type Region[T] uses {T}`

<a id="def-core_401_2fReservation"></a>

- `type Reservation uses {} = { sku: Sku, count: u32 }`

<a id="def-core_401_2fResult"></a>

- `type Result[T, E] uses {T, E} = Ok(T) | Error(E)`

<a id="def-core_401_2fResult_23Error"></a>

- `Error(E) -> Result[T, E]`

Variant of Result.

<a id="def-core_401_2fResult_23Ok"></a>

- `Ok(T) -> Result[T, E]`

Variant of Result.

<a id="def-core_401_2fScalarView"></a>

- `opaque type ScalarView: copyable + inert + noalloc_drop uses {}`

<a id="def-core_401_2fScope"></a>

- `@capability(local) opaque type Scope: noalloc_drop uses {Scope}`

<a id="def-core_401_2fSender"></a>

- `type Sender[T] uses {Tasks, T}`

<a id="def-core_401_2fSet"></a>

- `type Set[T: key] uses {T}`

<a id="def-core_401_2fShapeBody"></a>

- `type ShapeBody: copyable + data + key + inert + noalloc_drop uses {} = Record(List[FieldShape]) | Choice(List[VariantShape])`

<a id="def-core_401_2fShapeBody_23Choice"></a>

- `Choice(List[VariantShape]) -> ShapeBody`

Variant of ShapeBody.

<a id="def-core_401_2fShapeBody_23Record"></a>

- `Record(List[FieldShape]) -> ShapeBody`

Variant of ShapeBody.

<a id="def-core_401_2fShapeInfo"></a>

- `type ShapeInfo: copyable + data + key + inert + noalloc_drop uses {} = { name: String, visibility: ShapeVisibility, body: ShapeBody }`

<a id="def-core_401_2fShapeVisibility"></a>

- `type ShapeVisibility: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Private() | Public() | Opaque()`

<a id="def-core_401_2fShapeVisibility_23Opaque"></a>

- `Opaque() -> ShapeVisibility`

Variant of ShapeVisibility.

<a id="def-core_401_2fShapeVisibility_23Private"></a>

- `Private() -> ShapeVisibility`

Variant of ShapeVisibility.

<a id="def-core_401_2fShapeVisibility_23Public"></a>

- `Public() -> ShapeVisibility`

Variant of ShapeVisibility.

<a id="def-core_401_2fShared"></a>

- `type Shared[T] uses {T}`

<a id="def-core_401_2fShutdown"></a>

- `type Shutdown: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Read() | Write() | Both()`

<a id="def-core_401_2fShutdown_23Both"></a>

- `Both() -> Shutdown`

Variant of Shutdown.

<a id="def-core_401_2fShutdown_23Read"></a>

- `Read() -> Shutdown`

Variant of Shutdown.

<a id="def-core_401_2fShutdown_23Write"></a>

- `Write() -> Shutdown`

Variant of Shutdown.

<a id="def-core_401_2fSimd"></a>

- `type Simd[A, n: size] uses {A}`

<a id="def-core_401_2fSku"></a>

- `opaque type Sku: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}`

<a id="def-core_401_2fSlotId"></a>

- `type SlotId[T] uses {}`

<a id="def-core_401_2fSlots"></a>

- `type Slots[T] uses {T}`

<a id="def-core_401_2fSocketAddr"></a>

- `type SocketAddr uses {} = { ip: IpAddr, port: u16 }`

<a id="def-core_401_2fSortedMap"></a>

- `type SortedMap[K, V] uses {K, V}`

<a id="def-core_401_2fSourceSite"></a>

- `type SourceSite uses {} = { definition: String, line: u64, column: u64 }`

<a id="def-core_401_2fStderr"></a>

- `@capability(global) opaque type Stderr: plain + copyable + sendable + noalloc_drop uses {Stderr}`

<a id="def-core_401_2fStdin"></a>

- `@capability(global) opaque type Stdin: plain + copyable + sendable + noalloc_drop uses {Stdin}`

<a id="def-core_401_2fStdout"></a>

- `@capability(global) opaque type Stdout: plain + copyable + sendable + noalloc_drop uses {Stdout}`

<a id="def-core_401_2fStockError"></a>

- `type StockError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = OutOfStock(Sku)`

<a id="def-core_401_2fStockError_23OutOfStock"></a>

- `OutOfStock(Sku) -> StockError`

Variant of StockError.

<a id="def-core_401_2fStrPos"></a>

- `opaque type StrPos: copyable + inert + noalloc_drop + plain + data + key + sendable uses {}`

<a id="def-core_401_2fStream"></a>

- `opaque resource type Stream: sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fString"></a>

- `type String: copyable + data + inert + noalloc_drop + key uses {}`

<a id="def-core_401_2fTask"></a>

- `type Task[T] uses {Scope, T}`

<a id="def-core_401_2fTaskSet"></a>

- `opaque resource type TaskSet[R]: noalloc_drop uses {Scope, R}`

<a id="def-core_401_2fTasks"></a>

- `@capability(global) opaque type Tasks: plain + copyable + sendable + noalloc_drop uses {Tasks}`

<a id="def-core_401_2fTest"></a>

- `@capability(local) opaque type Test: noalloc_drop uses {Test}`

<a id="def-core_401_2fTlsFailure"></a>

- `type TlsFailure: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = UnknownIssuer() | Expired() | NotYetValid() | NameMismatch() | Revoked() | BadCertificate() | AlertReceived(u8) | HandshakeFailure() | PeerMisbehaved() | Truncated()`

<a id="def-core_401_2fTlsFailure_23AlertReceived"></a>

- `AlertReceived(u8) -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23BadCertificate"></a>

- `BadCertificate() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23Expired"></a>

- `Expired() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23HandshakeFailure"></a>

- `HandshakeFailure() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23NameMismatch"></a>

- `NameMismatch() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23NotYetValid"></a>

- `NotYetValid() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23PeerMisbehaved"></a>

- `PeerMisbehaved() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23Revoked"></a>

- `Revoked() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23Truncated"></a>

- `Truncated() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23UnknownIssuer"></a>

- `UnknownIssuer() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsRoots"></a>

- `type TlsRoots uses {} = WebPki() | Pem(Bytes)`

<a id="def-core_401_2fTlsRoots_23Pem"></a>

- `Pem(Bytes) -> TlsRoots`

Variant of TlsRoots.

<a id="def-core_401_2fTlsRoots_23WebPki"></a>

- `WebPki() -> TlsRoots`

Variant of TlsRoots.

<a id="def-core_401_2fTx"></a>

- `@capability(local) opaque type Tx: noalloc_drop uses {Tx}`

<a id="def-core_401_2fTxFailure"></a>

- `type TxFailure[E] uses {E} = Aborted(E) | Canceled(CancelCause)`

<a id="def-core_401_2fTxFailure_23Aborted"></a>

- `Aborted(E) -> TxFailure[E]`

Variant of TxFailure.

<a id="def-core_401_2fTxFailure_23Canceled"></a>

- `Canceled(CancelCause) -> TxFailure[E]`

Variant of TxFailure.

<a id="def-core_401_2fUdpReceived"></a>

- `type UdpReceived uses {} = { count: u64, peer: SocketAddr, truncated: bool }`

<a id="def-core_401_2fUdpSocket"></a>

- `opaque resource type UdpSocket: sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fUnsafe"></a>

- `@capability(global) opaque type Unsafe: plain + copyable + sendable + noalloc_drop uses {Unsafe}`

<a id="def-core_401_2fVariantShape"></a>

- `type VariantShape: copyable + data + key + inert + noalloc_drop uses {} = { name: String, payload: List[PayloadShape] }`

<a id="def-core_401_2fWaitError"></a>

- `type WaitError: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Clock(ClockError) | Canceled(CancelCause)`

<a id="def-core_401_2fWaitError_23Canceled"></a>

- `Canceled(CancelCause) -> WaitError`

Variant of WaitError.

<a id="def-core_401_2fWaitError_23Clock"></a>

- `Clock(ClockError) -> WaitError`

Variant of WaitError.

<a id="def-core_401_2fWallClock"></a>

- `@capability(global) opaque type WallClock: plain + copyable + sendable + noalloc_drop uses {WallClock}`

<a id="def-core_401_2fbool"></a>

- `type bool: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fchar"></a>

- `type char: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fcopy_5fto"></a>

- `fn copy_to[T: copyable](a: Arena, x: T) -> T in a`

<a id="def-core_401_2ff32"></a>

- `type f32: copyable + data + inert + noalloc_drop + plain + sendable uses {}`

<a id="def-core_401_2ff64"></a>

- `type f64: copyable + data + inert + noalloc_drop + plain + sendable uses {}`

<a id="def-core_401_2fforget"></a>

- `@noalloc fn forget[T](take x: T) -> ()`

<a id="def-core_401_2fi128"></a>

- `type i128: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fi16"></a>

- `type i16: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fi32"></a>

- `type i32: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fi64"></a>

- `type i64: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fi8"></a>

- `type i8: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2freplace"></a>

- `@preserves_unique(place) fn replace[T](mut place: T, take new: T) -> T`

<a id="def-core_401_2fu128"></a>

- `type u128: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fu16"></a>

- `type u16: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fu32"></a>

- `type u32: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fu64"></a>

- `type u64: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`

<a id="def-core_401_2fu8"></a>

- `type u8: copyable + data + inert + noalloc_drop + plain + sendable + key uses {}`


## arena

<a id="def-core_401_2farena_2fbounded"></a>

- `@noalloc fn arena.bounded[R](cap: u64, body: fn(a: Arena) -> R) -> Result[R, OutOfSpace]`

<a id="def-core_401_2farena_2fseal"></a>

- `fn arena.seal[T](take a: Arena in a, take root: T in root) -> Region[T]`

<a id="def-core_401_2farena_2fwith"></a>

- `@noalloc fn arena.with[R](body: fn(a: Arena) -> R) -> R`


## arg

<a id="def-core_401_2farg_2fcount"></a>

- `@noalloc fn arg.count(args: Args) -> u64`

<a id="def-core_401_2farg_2fpath"></a>

- `fn arg.path(args: Args, index: u64) -> Result[Path, ArgError]`

<a id="def-core_401_2farg_2ftext"></a>

- `fn arg.text(args: Args, index: u64) -> Result[String, ArgError]`


## array

<a id="def-core_401_2farray_2flen"></a>

- `@noalloc fn array.len[T](xs: Array[T]) -> u64`


## ascii

<a id="def-std_401_2fascii_2fdigit_5fvalue"></a>

- `@noalloc pub fn ascii.digit_value(c: u8) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fdigit_5fvalue_5fradix"></a>

- `@noalloc pub fn ascii.digit_value_radix(c: u8, radix: u8) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5falnum"></a>

- `@noalloc pub fn ascii.is_alnum(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5falpha"></a>

- `@noalloc pub fn ascii.is_alpha(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5fdigit"></a>

- `@noalloc pub fn ascii.is_digit(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5flower"></a>

- `@noalloc pub fn ascii.is_lower(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5fspace"></a>

- `@noalloc pub fn ascii.is_space(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fis_5fupper"></a>

- `@noalloc pub fn ascii.is_upper(c: u8) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fto_5flower"></a>

- `@noalloc pub fn ascii.to_lower(c: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fascii_2fto_5fupper"></a>

- `@noalloc pub fn ascii.to_upper(c: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)


## atomic

<a id="def-core_401_2fatomic_2fcreate"></a>

- `fn atomic.create[A](tasks: Tasks, value: A) -> Atomic[A]`

<a id="def-core_401_2fatomic_2fload"></a>

- `@noalloc fn atomic.load[A](tasks: Tasks, a: Atomic[A]) -> A`

<a id="def-core_401_2fatomic_2fstore"></a>

- `@noalloc fn atomic.store[A](tasks: Tasks, a: Atomic[A], value: A) -> ()`


## base64

<a id="def-std_401_2fbase64_2fAlphabet"></a>

- `pub type base64.Alphabet = Standard | UrlSafe`

<a id="def-std_401_2fbase64_2fAlphabet_23Standard"></a>

- `Standard -> base64.Alphabet`

Variant of base64.Alphabet.

<a id="def-std_401_2fbase64_2fAlphabet_23UrlSafe"></a>

- `UrlSafe -> base64.Alphabet`

Variant of base64.Alphabet.

<a id="def-std_401_2fbase64_2fDecodeError"></a>

- `pub type base64.DecodeError = InvalidByte(u64) | InvalidLength | InvalidPadding(u64) | NonCanonical( u64 )`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidByte"></a>

- `InvalidByte(u64) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidLength"></a>

- `InvalidLength -> base64.DecodeError`

Variant of base64.DecodeError.

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidPadding"></a>

- `InvalidPadding(u64) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23NonCanonical"></a>

- `NonCanonical( u64 ) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fPadding"></a>

- `pub type base64.Padding = Required | Omitted`

<a id="def-std_401_2fbase64_2fPadding_23Omitted"></a>

- `Omitted -> base64.Padding`

Variant of base64.Padding.

<a id="def-std_401_2fbase64_2fPadding_23Required"></a>

- `Required -> base64.Padding`

Variant of base64.Padding.

<a id="def-std_401_2fbase64_2fdecode"></a>

- `pub fn base64.decode(text: String, alphabet: Alphabet, padding: Padding) -> Result[ unique Bytes, DecodeError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/DecodeError`](#def-std_401_2fbase64_2fDecodeError), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)

<a id="def-std_401_2fbase64_2fencode"></a>

- `pub fn base64.encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)


## bool

<a id="def-std_401_2fbool_2fformat"></a>

- `@noalloc pub fn bool.format[n: size](value: bool, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fbool_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn bool.format_into(value: bool, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fbool_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn bool.format_into_spec( value: bool, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fbool_2fparse"></a>

- `@noalloc pub fn bool.parse(text: String) -> Result[bool, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fbool_2fto_5fstring"></a>

- `pub fn bool.to_string(value: bool) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)


## bufio

<a id="def-std_401_2fbufio_2fReadError"></a>

- `pub type bufio.ReadError[E] = Source(E) | Limit | UnexpectedEof | InvalidUtf8 | InvalidRead`

<a id="def-std_401_2fbufio_2fReadError_23InvalidRead"></a>

- `InvalidRead -> bufio.ReadError[E]`

Variant of bufio.ReadError.

<a id="def-std_401_2fbufio_2fReadError_23InvalidUtf8"></a>

- `InvalidUtf8 -> bufio.ReadError[E]`

Variant of bufio.ReadError.

<a id="def-std_401_2fbufio_2fReadError_23Limit"></a>

- `Limit -> bufio.ReadError[E]`

Variant of bufio.ReadError.

<a id="def-std_401_2fbufio_2fReadError_23Source"></a>

- `Source(E) -> bufio.ReadError[E]`

Variant of bufio.ReadError.

<a id="def-std_401_2fbufio_2fReadError_23UnexpectedEof"></a>

- `UnexpectedEof -> bufio.ReadError[E]`

Variant of bufio.ReadError.

<a id="def-std_401_2fbufio_2fReader"></a>

- `pub opaque type bufio.Reader: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fbufio_2fbuffered"></a>

- `pub fn bufio.buffered(r: Reader) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fconsume"></a>

- `pub fn bufio.consume(mut r: Reader, count: u64) -> () uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fcontents"></a>

- `pub fn bufio.contents(r: Reader) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2ffile_5fread"></a>

- `@preserves_unique(out) pub fn bufio.file_read(d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], max: u64) -> Result[ u64, ReadError[FileError] ]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2ffile_5fread_5fexact"></a>

- `@preserves_unique(out) pub fn bufio.file_read_exact( d: Dir, mut f: File of d, mut r: Reader, mut out: List[u8], count: u64 ) -> Result[(), ReadError[FileError]]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2ffile_5fread_5fline"></a>

- `pub fn bufio.file_read_line(d: Dir, mut f: File of d, mut r: Reader, limit: u64) -> Result[ Option[unique String], ReadError[FileError] ]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2ffile_5fread_5fmore"></a>

- `pub fn bufio.file_read_more(d: Dir, mut f: File of d, mut r: Reader, max: u64) -> Result[ u64, ReadError[FileError] ]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2ffile_5fread_5funtil"></a>

- `@preserves_unique(out) pub fn bufio.file_read_until( d: Dir, mut f: File of d, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[FileError]]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fnew"></a>

- `pub fn bufio.new(capacity: u64) -> Reader uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fread"></a>

- `@preserves_unique(out) pub fn bufio.read[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fread_5fexact"></a>

- `@preserves_unique(out) pub fn bufio.read_exact[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], count: u64 ) -> Result[(), ReadError[E]] uses {refill}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fread_5fline"></a>

- `pub fn bufio.read_line[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], limit: u64 ) -> Result[Option[unique String], ReadError[E]] uses {refill}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fread_5fmore"></a>

- `pub fn bufio.read_more[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fread_5funtil"></a>

- `@preserves_unique(out) pub fn bufio.read_until[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[E]] uses {refill}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstdin_5fread"></a>

- `@preserves_unique(out) pub fn bufio.stdin_read(s: Stdin, mut r: Reader, mut out: List[u8], max: u64) -> Result[ u64, ReadError[InputError] ]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstdin_5fread_5fexact"></a>

- `@preserves_unique(out) pub fn bufio.stdin_read_exact(s: Stdin, mut r: Reader, mut out: List[u8], count: u64) -> Result[ (), ReadError[InputError] ]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstdin_5fread_5fline"></a>

- `pub fn bufio.stdin_read_line(s: Stdin, mut r: Reader, limit: u64) -> Result[ Option[unique String], ReadError[InputError] ]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstdin_5fread_5fmore"></a>

- `pub fn bufio.stdin_read_more(s: Stdin, mut r: Reader, max: u64) -> Result[ u64, ReadError[InputError] ]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstdin_5fread_5funtil"></a>

- `@preserves_unique(out) pub fn bufio.stdin_read_until( s: Stdin, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[InputError]]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread"></a>

- `@preserves_unique(out) pub fn bufio.stream_read( n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], max: u64 ) -> Result[u64, ReadError[NetError]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread_5fexact"></a>

- `@preserves_unique(out) pub fn bufio.stream_read_exact( n: Net, mut s: Stream of n, mut r: Reader, mut out: List[u8], count: u64 ) -> Result[(), ReadError[NetError]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread_5fline"></a>

- `pub fn bufio.stream_read_line(n: Net, mut s: Stream of n, mut r: Reader, limit: u64) -> Result[ Option[unique String], ReadError[NetError] ]`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread_5fmore"></a>

- `pub fn bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[ u64, ReadError[NetError] ]`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread_5funtil"></a>

- `@preserves_unique(out) pub fn bufio.stream_read_until( n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[NetError]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)


## bytes

<a id="def-std_401_2fbytes_2ffrom_5finline"></a>

- `pub fn bytes.from_inline[n: size](xs: Inline[u8, n]) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-core_401_2fbytes_2ffrom_5flist"></a>

- `@preserves_unique(src) fn bytes.from_list(take src: List[u8]) -> Bytes`

<a id="def-core_401_2fbytes_2flen"></a>

- `@noalloc fn bytes.len(src: Bytes) -> u64`

<a id="def-std_401_2fbytes_2freplace"></a>

- `pub fn bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> unique Bytes in src`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-core_401_2fbytes_2fslice"></a>

- `@noalloc fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src`


## channel

<a id="def-core_401_2fchannel_2fopen"></a>

- `fn channel.open[T: sendable](tasks: Tasks) -> (Sender[T], Receiver[T])`

<a id="def-core_401_2fchannel_2freceive"></a>

- `fn channel.receive[T](tasks: Tasks, mut r: Receiver[T]) -> Result[T, ChannelError]`

<a id="def-core_401_2fchannel_2fsend"></a>

- `fn channel.send[T: sendable](tasks: Tasks, mut s: Sender[T], take v: T) -> Result[(), (T, ChannelError)]`


## char

<a id="def-std_401_2fchar_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn char.format_into(value: char, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fchar_2ffrom_5fascii"></a>

- `@noalloc pub fn char.from_ascii(byte: u8) -> Option[char] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/char`](#def-core_401_2fchar), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-core_401_2fchar_2ffrom_5fu32"></a>

- `@noalloc fn char.from_u32(value: u32) -> Option[char]`

<a id="def-std_401_2fchar_2ffrom_5fu8"></a>

- `@noalloc pub fn char.from_u8(byte: u8) -> char uses {}`

References: [`core@1/char`](#def-core_401_2fchar), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fchar_2fto_5fstring"></a>

- `pub fn char.to_string(value: char) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/char`](#def-core_401_2fchar)

<a id="def-core_401_2fchar_2fto_5fu32"></a>

- `@noalloc fn char.to_u32(c: char) -> u32`


## child

<a id="def-core_401_2fchild_2fclose_5fstdin"></a>

- `@noalloc fn child.close_stdin(p: Proc, mut c: Child of p) -> ()`

<a id="def-core_401_2fchild_2fkill"></a>

- `@noalloc fn child.kill(p: Proc, mut c: Child of p) -> ()`

<a id="def-core_401_2fchild_2fread_5fstderr"></a>

- `fn child.read_stderr(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]`

<a id="def-core_401_2fchild_2fread_5fstdout"></a>

- `fn child.read_stdout(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]`

<a id="def-core_401_2fchild_2fwait"></a>

- `@noalloc fn child.wait(p: Proc, take c: Child of p) -> Result[ExitStatus, ProcessError]`

<a id="def-core_401_2fchild_2fwrite"></a>

- `@noalloc fn child.write(p: Proc, mut c: Child of p, data: Bytes) -> Result[(), ProcessError]`


## cli

<a id="def-std_401_2fcli_2fFlag"></a>

- `pub type cli.Flag { name: String, short: Option[u8], kind: Kind, help: String, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/cli/Kind`](#def-std_401_2fcli_2fKind)

<a id="def-std_401_2fcli_2fKind"></a>

- `pub type cli.Kind = Switch | Value`

<a id="def-std_401_2fcli_2fKind_23Switch"></a>

- `Switch -> cli.Kind`

Variant of cli.Kind.

<a id="def-std_401_2fcli_2fKind_23Value"></a>

- `Value -> cli.Kind`

Variant of cli.Kind.

<a id="def-std_401_2fcli_2fParseError"></a>

- `pub type cli.ParseError = UnknownFlag(u64) | MissingValue(u64) | UnexpectedValue(u64) | InvalidSpec`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcli_2fParseError_23InvalidSpec"></a>

- `InvalidSpec -> cli.ParseError`

Variant of cli.ParseError.

<a id="def-std_401_2fcli_2fParseError_23MissingValue"></a>

- `MissingValue(u64) -> cli.ParseError`

Variant of cli.ParseError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcli_2fParseError_23UnexpectedValue"></a>

- `UnexpectedValue(u64) -> cli.ParseError`

Variant of cli.ParseError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcli_2fParseError_23UnknownFlag"></a>

- `UnknownFlag(u64) -> cli.ParseError`

Variant of cli.ParseError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcli_2fParsed"></a>

- `pub type cli.Parsed { flags: Map[String, List[String]], positional: List[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fcli_2fparse"></a>

- `pub fn cli.parse(take args: List[String], specs: List[Flag]) -> Result[Parsed, ParseError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/cli/Flag`](#def-std_401_2fcli_2fFlag), [`std@1/cli/ParseError`](#def-std_401_2fcli_2fParseError), [`std@1/cli/Parsed`](#def-std_401_2fcli_2fParsed)

<a id="def-std_401_2fcli_2fusage"></a>

- `pub fn cli.usage(program: String, specs: List[Flag]) -> String uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/cli/Flag`](#def-std_401_2fcli_2fFlag)


## clock

<a id="def-core_401_2fclock_2fmonotonic"></a>

- `@noalloc fn clock.monotonic(clock: MonotonicClock) -> Result[Instant of clock, ClockError]`

<a id="def-core_401_2fclock_2fsleep_5funtil"></a>

- `@noalloc fn clock.sleep_until(clock: MonotonicClock, deadline: Instant of clock) -> Result[(), WaitError]`

<a id="def-core_401_2fclock_2fwall"></a>

- `@noalloc fn clock.wall(clock: WallClock) -> Result[i64, ClockError]`


## codec

<a id="def-core_401_2fcodec_2fdecode"></a>

- `fn codec.decode[T: serializable](data: Bytes) -> Result[T, DecodeError]`

<a id="def-core_401_2fcodec_2fencode"></a>

- `fn codec.encode[T: serializable](value: T) -> Bytes`


## codegen

<a id="def-core_401_2fcodegen_2fbody"></a>

- `fn codegen.body[F](text: String) -> Body[F]`


## csv

<a id="def-std_401_2fcsv_2fConfig"></a>

- `pub type csv.Config { delimiter: u8, fields_per_record: Option[u64], max_record_bytes: u64, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fcsv_2fCsvError"></a>

- `pub type csv.CsvError = Syntax(u64) | UnequalFields(u64) | RecordTooLarge | InvalidUtf8 | InvalidDelimiter`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcsv_2fCsvError_23InvalidDelimiter"></a>

- `InvalidDelimiter -> csv.CsvError`

Variant of csv.CsvError.

<a id="def-std_401_2fcsv_2fCsvError_23InvalidUtf8"></a>

- `InvalidUtf8 -> csv.CsvError`

Variant of csv.CsvError.

<a id="def-std_401_2fcsv_2fCsvError_23RecordTooLarge"></a>

- `RecordTooLarge -> csv.CsvError`

Variant of csv.CsvError.

<a id="def-std_401_2fcsv_2fCsvError_23Syntax"></a>

- `Syntax(u64) -> csv.CsvError`

Variant of csv.CsvError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcsv_2fCsvError_23UnequalFields"></a>

- `UnequalFields(u64) -> csv.CsvError`

Variant of csv.CsvError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcsv_2fReadError"></a>

- `pub type csv.ReadError[E] = Source(E) | Csv(CsvError) | InvalidRead`

References: [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fReadError_23Csv"></a>

- `Csv(CsvError) -> csv.ReadError[E]`

Variant of csv.ReadError.

References: [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fReadError_23InvalidRead"></a>

- `InvalidRead -> csv.ReadError[E]`

Variant of csv.ReadError.

<a id="def-std_401_2fcsv_2fReadError_23Source"></a>

- `Source(E) -> csv.ReadError[E]`

Variant of csv.ReadError.

<a id="def-std_401_2fcsv_2fReader"></a>

- `pub opaque type csv.Reader: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fcsv_2fdefault_5fconfig"></a>

- `pub fn csv.default_config() -> Config uses {}`

References: [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig)

<a id="def-std_401_2fcsv_2ffeed"></a>

- `pub fn csv.feed(mut r: Reader, chunk: Bytes, eof: bool) -> Result[ List[List[String]], CsvError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2fparse"></a>

- `pub fn csv.parse(take text: String, config: Config) -> Result[List[List[String]], CsvError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fread_5frecord"></a>

- `pub fn csv.read_record[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E] ) -> Result[Option[List[String]], ReadError[E]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/ReadError`](#def-std_401_2fcsv_2fReadError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2freader"></a>

- `pub fn csv.reader(config: Config) -> Result[Reader, CsvError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2fwrite_5frecord"></a>

- `pub fn csv.write_record(fields: List[String], delimiter: u8, mut out: List[u8]) -> Result[ (), CsvError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)


## deque

<a id="def-std_401_2fdeque_2fDeque"></a>

- `pub opaque type deque.Deque[T]: () uses {T}`

<a id="def-std_401_2fdeque_2fempty"></a>

- `pub fn deque.empty[T]() -> unique Deque[T] uses {}`

References: [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2flen"></a>

- `@noalloc pub fn deque.len[T](q: Deque[T]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fpop_5fback"></a>

- `pub fn deque.pop_back[T](mut q: unique Deque[T]) -> unique Option[T] uses {q}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fpop_5ffront"></a>

- `pub fn deque.pop_front[T](mut q: unique Deque[T]) -> unique Option[T] uses {q}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fpush_5fback"></a>

- `pub fn deque.push_back[T](mut q: unique Deque[T], take value: unique T) -> () uses {q, value}`

References: [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fpush_5ffront"></a>

- `pub fn deque.push_front[T](mut q: unique Deque[T], take value: unique T) -> () uses {q, value}`

References: [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)


## dir

<a id="def-core_401_2fdir_2fnarrow"></a>

- `fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]`

<a id="def-core_401_2fdir_2fremove_5fall"></a>

- `fn dir.remove_all(d: Dir, p: Path) -> Result[(), FileError]`


## duration

<a id="def-std_401_2fduration_2fformat"></a>

- `pub fn duration.format(value: time.Duration) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fduration_2ffrom_5fparts"></a>

- `@noalloc pub fn duration.from_parts(seconds: i64, nanoseconds: i32) -> time.Duration uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fduration_2fparse"></a>

- `@noalloc pub fn duration.parse(text: String) -> Result[time.Duration, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)


## env

<a id="def-core_401_2fenv_2fget"></a>

- `fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]`


## err

<a id="def-core_401_2ferr_2fprint"></a>

- `@noalloc fn err.print(s: Stderr, text: String) -> Result[(), OutputError]`


## f32

<a id="def-core_401_2ff32_2fbits"></a>

- `@noalloc fn f32.bits(value: f32) -> u32`

<a id="def-std_401_2ff32_2fceil"></a>

- `@noalloc pub fn f32.ceil(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2ffloor"></a>

- `@noalloc pub fn f32.floor(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fformat"></a>

- `@noalloc pub fn f32.format[n: size](value: f32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2ff32_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn f32.format_into(value: f32, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff32_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn f32.format_into_spec( value: f32, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-core_401_2ff32_2ffrom_5fbits"></a>

- `@noalloc fn f32.from_bits(value: u32) -> f32`

<a id="def-std_401_2ff32_2fparse"></a>

- `@noalloc pub fn f32.parse(text: String) -> Result[f32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f32`](#def-core_401_2ff32), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff32_2fround"></a>

- `@noalloc pub fn f32.round(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fsqrt"></a>

- `@noalloc pub fn f32.sqrt(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fsum"></a>

- `@noalloc pub fn f32.sum(xs: Array[f32]) -> f32 uses {}`

References: [`core@1/Array`](#def-core_401_2fArray), [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fsum_5fexact"></a>

- `pub fn f32.sum_exact(xs: Array[f32]) -> f32 uses {}`

References: [`core@1/Array`](#def-core_401_2fArray), [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fto_5fstring"></a>

- `pub fn f32.to_string(value: f32) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/f32`](#def-core_401_2ff32)


## f64

<a id="def-core_401_2ff64_2fbits"></a>

- `@noalloc fn f64.bits(value: f64) -> u64`

<a id="def-std_401_2ff64_2fceil"></a>

- `@noalloc pub fn f64.ceil(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2ffloor"></a>

- `@noalloc pub fn f64.floor(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fformat"></a>

- `@noalloc pub fn f64.format[n: size](value: f64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2ff64_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn f64.format_into(value: f64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff64_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn f64.format_into_spec( value: f64, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-core_401_2ff64_2ffrom_5fbits"></a>

- `@noalloc fn f64.from_bits(value: u64) -> f64`

<a id="def-std_401_2ff64_2fparse"></a>

- `@noalloc pub fn f64.parse(text: String) -> Result[f64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff64_2fround"></a>

- `@noalloc pub fn f64.round(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fsqrt"></a>

- `@noalloc pub fn f64.sqrt(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fsum"></a>

- `@noalloc pub fn f64.sum(xs: Array[f64]) -> f64 uses {}`

References: [`core@1/Array`](#def-core_401_2fArray), [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fsum_5fexact"></a>

- `pub fn f64.sum_exact(xs: Array[f64]) -> f64 uses {}`

References: [`core@1/Array`](#def-core_401_2fArray), [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fto_5fstring"></a>

- `pub fn f64.to_string(value: f64) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64)


## failure

<a id="def-core_401_2ffailure_2fcheck"></a>

- `@noalloc fn failure.check(condition: bool, rule: CheckRule) -> () uses {}`


## file

<a id="def-core_401_2ffile_2fappend"></a>

- `fn file.append(d: Dir, mut f: File of d, data: Bytes) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fcommit"></a>

- `fn file.commit(d: Dir, take f: AtomicFile of d) -> Result[(), FileError]`

<a id="def-std_401_2ffile_2fcopy_5ffile"></a>

- `pub fn file.copy_file(d: Dir, from: Path, to: Path) -> Result[(), FileError]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-core_401_2ffile_2fcreate"></a>

- `fn file.create(d: Dir, p: Path) -> Result[File of d, FileError]`

<a id="def-core_401_2ffile_2fcreate_5fatomic"></a>

- `fn file.create_atomic(d: Dir, p: Path) -> Result[AtomicFile of d, FileError]`

<a id="def-std_401_2ffile_2flist_5frecursive"></a>

- `pub fn file.list_recursive(d: Dir, p: Path) -> Result[unique List[DirEntry], FileError]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/DirEntry`](#def-core_401_2fDirEntry), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-core_401_2ffile_2fmetadata"></a>

- `fn file.metadata(d: Dir, p: Path) -> Result[Metadata, FileError]`

<a id="def-core_401_2ffile_2fmkdir"></a>

- `fn file.mkdir(d: Dir, p: Path) -> Result[(), FileError]`

<a id="def-std_401_2ffile_2fmkdir_5fall"></a>

- `pub fn file.mkdir_all(d: Dir, p: Path) -> Result[(), FileError]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-core_401_2ffile_2fnarrow"></a>

- `fn file.narrow(d: Dir, take f: File) -> Option[File of d]`

<a id="def-core_401_2ffile_2fopen"></a>

- `fn file.open(d: Dir, p: Path) -> Result[File of d, FileError]`

<a id="def-core_401_2ffile_2fread"></a>

- `fn file.read(d: Dir, mut f: File of d, mut buffer: List[u8], max: u64) -> Result[u64, FileError]`

<a id="def-core_401_2ffile_2fread_5fbytes"></a>

- `fn file.read_bytes(d: Dir, p: Path) -> Result[Bytes, FileError] in d`

<a id="def-core_401_2ffile_2fread_5fdir"></a>

- `fn file.read_dir(d: Dir, p: Path) -> Result[List[DirEntry], FileError]`

<a id="def-core_401_2ffile_2fremove"></a>

- `@noalloc fn file.remove(d: Dir, p: Path) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fremove_5fdir"></a>

- `fn file.remove_dir(d: Dir, p: Path) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2frename"></a>

- `fn file.rename(d: Dir, from: Path, to: Path) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fseek"></a>

- `fn file.seek(d: Dir, mut f: File of d, offset: u64) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fset_5fpermissions"></a>

- `fn file.set_permissions(d: Dir, p: Path, permissions: u32) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fsize"></a>

- `fn file.size(d: Dir, p: Path) -> Result[u64, FileError]`

<a id="def-core_401_2ffile_2fwalk"></a>

- `fn file.walk(d: Dir) -> Result[Iter[Result[Path, FileError]], FileError]`

<a id="def-core_401_2ffile_2fwrite"></a>

- `fn file.write(d: Dir, mut f: File of d, data: Bytes) -> Result[u64, FileError]`

<a id="def-core_401_2ffile_2fwrite_5fall"></a>

- `fn file.write_all(d: Dir, mut f: AtomicFile of d, data: Bytes) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fwrite_5fbytes"></a>

- `fn file.write_bytes(d: Dir, p: Path, take data: Bytes) -> Result[(), FileError]`


## float

<a id="def-core_401_2ffloat_2ffrom_5fint"></a>

- `@noalloc fn float.from_int[I, F](value: I) -> F`

<a id="def-core_401_2ffloat_2fmax"></a>

- `@noalloc fn float.max[F](a: F, b: F) -> F`

<a id="def-core_401_2ffloat_2fmax_5fnumber"></a>

- `@noalloc fn float.max_number[F](a: F, b: F) -> F`

<a id="def-core_401_2ffloat_2fmin"></a>

- `@noalloc fn float.min[F](a: F, b: F) -> F`

<a id="def-core_401_2ffloat_2fmin_5fnumber"></a>

- `@noalloc fn float.min_number[F](a: F, b: F) -> F`

<a id="def-core_401_2ffloat_2fmul_5fadd"></a>

- `@noalloc fn float.mul_add[F](a: F, b: F, c: F) -> F`

<a id="def-core_401_2ffloat_2fto_5fi32"></a>

- `@noalloc fn float.to_i32[F](value: F) -> Result[i32, FloatIntError]`

<a id="def-core_401_2ffloat_2ftotal_5forder"></a>

- `@noalloc fn float.total_order[F](a: F, b: F) -> i32`


## glob

<a id="def-std_401_2fglob_2fExpandError"></a>

- `pub type glob.ExpandError = InvalidPattern(PatternError) | Files(FileError)`

References: [`core@1/FileError`](#def-core_401_2fFileError), [`std@1/glob/PatternError`](#def-std_401_2fglob_2fPatternError)

<a id="def-std_401_2fglob_2fExpandError_23Files"></a>

- `Files(FileError) -> glob.ExpandError`

Variant of glob.ExpandError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2fglob_2fExpandError_23InvalidPattern"></a>

- `InvalidPattern(PatternError) -> glob.ExpandError`

Variant of glob.ExpandError.

References: [`std@1/glob/PatternError`](#def-std_401_2fglob_2fPatternError)

<a id="def-std_401_2fglob_2fPattern"></a>

- `pub opaque type glob.Pattern: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fglob_2fPatternError"></a>

- `pub type glob.PatternError = Syntax(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fglob_2fPatternError_23Syntax"></a>

- `Syntax(u64) -> glob.PatternError`

Variant of glob.PatternError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fglob_2fcompile"></a>

- `pub fn glob.compile(pattern: String) -> Result[Pattern, PatternError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/glob/Pattern`](#def-std_401_2fglob_2fPattern), [`std@1/glob/PatternError`](#def-std_401_2fglob_2fPatternError)

<a id="def-std_401_2fglob_2ffiles"></a>

- `pub fn glob.files(d: Dir, pattern: String) -> Result[unique List[Path], ExpandError]`

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/List`](#def-core_401_2fList), [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/glob/ExpandError`](#def-std_401_2fglob_2fExpandError)

<a id="def-std_401_2fglob_2fmatches"></a>

- `pub fn glob.matches(pattern: Pattern, path: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/glob/Pattern`](#def-std_401_2fglob_2fPattern)


## hex

<a id="def-std_401_2fhex_2fDecodeError"></a>

- `pub type hex.DecodeError = InvalidDigit(u64) | OddLength`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhex_2fDecodeError_23InvalidDigit"></a>

- `InvalidDigit(u64) -> hex.DecodeError`

Variant of hex.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhex_2fDecodeError_23OddLength"></a>

- `OddLength -> hex.DecodeError`

Variant of hex.DecodeError.

<a id="def-std_401_2fhex_2fdecode"></a>

- `pub fn hex.decode(text: String) -> Result[unique Bytes, DecodeError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/hex/DecodeError`](#def-std_401_2fhex_2fDecodeError)

<a id="def-std_401_2fhex_2fencode"></a>

- `pub fn hex.encode(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)


## http

<a id="def-std_401_2fhttp_2fBody"></a>

- `pub type http.Body { bytes: Bytes, content_type: String, }`

An encoded request body and its media type. client_send installs exactly one
Content-Type, replacing any Content-Type supplied in its headers argument.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhttp_2fClient"></a>

- `pub opaque resource type http.Client: noalloc_drop uses {Net, MonotonicClock}`

A sequential client owns its idle connections. Only complete, framed responses
return a connection to the pool. Failed exchanges are never retried, including
writes on stale connections: callers decide whether resending is safe.
Supply a clock for idle_timeout, or disable idle_timeout and pass None().

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Net`](#def-core_401_2fNet)

<a id="def-std_401_2fhttp_2fConfig"></a>

- `pub type http.Config { max_redirects: u32, max_idle_connections: u64, max_idle_connections_per_host: u64, connect_timeout: Option[time.Duration], read_timeout: Option[time.Duration], write_timeout: Option[time.Duration], max_head_bytes: u64, max_body_bytes: u64, idle_timeout: Option[time.Duration], max_headers: u64, user_agent: String, roots: TlsRoots, status_as_error: bool, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/TlsRoots`](#def-core_401_2fTlsRoots), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fhttp_2fHeader"></a>

- `pub type http.Header { name: String, value: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhttp_2fHttpError"></a>

- `pub type http.HttpError = Net(NetError) | Url(uri.UriError) | Malformed(http_wire.HeadError) | Chunk | HeadTooLarge | BodyTooLarge | Truncated | TooManyRedirects | RedirectRequiresResend | RedirectLocationInvalid | UnsupportedScheme | UnsupportedTransferEncoding | Status( u16 ) | OutOfOrder | Closed | RequestTimeout | Clock(ClockError) | Json(json.JsonError) | InvalidJsonUtf8`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/uri/UriError`](#def-std_401_2furi_2fUriError)

<a id="def-std_401_2fhttp_2fHttpError_23BodyTooLarge"></a>

- `BodyTooLarge -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Chunk"></a>

- `Chunk -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Clock"></a>

- `Clock(ClockError) -> http.HttpError`

Variant of http.HttpError.

References: [`core@1/ClockError`](#def-core_401_2fClockError)

<a id="def-std_401_2fhttp_2fHttpError_23Closed"></a>

- `Closed -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23HeadTooLarge"></a>

- `HeadTooLarge -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23InvalidJsonUtf8"></a>

- `InvalidJsonUtf8 -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Json"></a>

- `Json(json.JsonError) -> http.HttpError`

Variant of http.HttpError.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fhttp_2fHttpError_23Malformed"></a>

- `Malformed(http_wire.HeadError) -> http.HttpError`

Variant of http.HttpError.

References: [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError)

<a id="def-std_401_2fhttp_2fHttpError_23Net"></a>

- `Net(NetError) -> http.HttpError`

Variant of http.HttpError.

References: [`core@1/NetError`](#def-core_401_2fNetError)

<a id="def-std_401_2fhttp_2fHttpError_23OutOfOrder"></a>

- `OutOfOrder -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23RedirectLocationInvalid"></a>

- `RedirectLocationInvalid -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23RedirectRequiresResend"></a>

- `RedirectRequiresResend -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23RequestTimeout"></a>

- `RequestTimeout -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Status"></a>

- `Status( u16 ) -> http.HttpError`

Variant of http.HttpError.

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fhttp_2fHttpError_23TooManyRedirects"></a>

- `TooManyRedirects -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Truncated"></a>

- `Truncated -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23UnsupportedScheme"></a>

- `UnsupportedScheme -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23UnsupportedTransferEncoding"></a>

- `UnsupportedTransferEncoding -> http.HttpError`

Variant of http.HttpError.

<a id="def-std_401_2fhttp_2fHttpError_23Url"></a>

- `Url(uri.UriError) -> http.HttpError`

Variant of http.HttpError.

References: [`std@1/uri/UriError`](#def-std_401_2furi_2fUriError)

<a id="def-std_401_2fhttp_2fMethod"></a>

- `pub type http.Method = Get | Head | Post | Put | Delete | Patch | Options`

<a id="def-std_401_2fhttp_2fMethod_23Delete"></a>

- `Delete -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Get"></a>

- `Get -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Head"></a>

- `Head -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Options"></a>

- `Options -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Patch"></a>

- `Patch -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Post"></a>

- `Post -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fMethod_23Put"></a>

- `Put -> http.Method`

Variant of http.Method.

<a id="def-std_401_2fhttp_2fRequest"></a>

- `pub opaque resource type http.Request: noalloc_drop uses {Net}`

References: [`core@1/Net`](#def-core_401_2fNet)

<a id="def-std_401_2fhttp_2fResponse"></a>

- `pub type http.Response { status: u16, headers: List[Header], body: Bytes, url: String, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fhttp_2fResponseHead"></a>

- `pub type http.ResponseHead { version: Version, status: u16, reason: String, headers: List[Header], content_length: Option[u64], chunked: bool, keep_alive: bool, location: Option[String], content_type: Option[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fServerConnection"></a>

- `pub opaque resource type http.ServerConnection: noalloc_drop uses {Net}`

References: [`core@1/Net`](#def-core_401_2fNet)

<a id="def-std_401_2fhttp_2fServerRequest"></a>

- `pub type http.ServerRequest { method: String, target: String, version: Version, headers: List[Header], body: Bytes, trailers: List[Header], keep_alive: bool, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fVersion"></a>

- `pub type http.Version = Http10 | Http11`

<a id="def-std_401_2fhttp_2fVersion_23Http10"></a>

- `Http10 -> http.Version`

Variant of http.Version.

<a id="def-std_401_2fhttp_2fVersion_23Http11"></a>

- `Http11 -> http.Version`

Variant of http.Version.

<a id="def-std_401_2fhttp_2fclient"></a>

- `pub fn http.client(n: Net, clock: Option[MonotonicClock], config: Config) -> Result[ Client, HttpError ]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Client`](#def-std_401_2fhttp_2fClient), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fhttp_2fclient_5ffetch"></a>

- `pub fn http.client_fetch( mut c: Client, method: Method, url: String, headers: List[Header], body: Option[Bytes] ) -> Result[Response, HttpError]`

Bound the complete exchange, including connect, TLS, redirects and body reads,
with task.scope_until: take the Client into the body, then return it with the
outcome. Finished and Expired both retain that pair for the next call.
Expired contains the body's result; cancellation is Net(Canceled(cause)).
A cancelled exchange never returns its checked-out stream to the idle pool.

Fetch with connection reuse. The exclusive borrow permits one exchange at a
time per Client; use one Client per concurrent worker. The per-host limit
bounds idle connections only. max_idle_connections bounds the entire idle pool.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Client`](#def-std_401_2fhttp_2fClient), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse)

<a id="def-std_401_2fhttp_2fclient_5fsend"></a>

- `pub fn http.client_send( mut c: Client, method: Method, url: String, headers: List[Header], body: Body ) -> Result[Response, HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody), [`std@1/http/Client`](#def-std_401_2fhttp_2fClient), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse)

<a id="def-std_401_2fhttp_2fclose_5fidle"></a>

- `pub fn http.close_idle(mut c: Client)`

Drop every idle stream. The Client remains usable.

References: [`std@1/http/Client`](#def-std_401_2fhttp_2fClient)

<a id="def-std_401_2fhttp_2fdefault_5fconfig"></a>

- `pub fn http.default_config() -> Config uses {}`

References: [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig)

<a id="def-std_401_2fhttp_2fend_5fbody"></a>

- `pub fn http.end_body(mut r: Request, request_trailers: List[Header]) -> Result[(), HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2ffetch"></a>

- `pub fn http.fetch( n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes] ) -> Result[Response, HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse)

<a id="def-std_401_2fhttp_2fform_5fbody"></a>

- `pub fn http.form_body(pairs: List[(String, String)]) -> Body uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody)

<a id="def-std_401_2fhttp_2fheader"></a>

- `pub fn http.header(headers: List[Header], name: String) -> Option[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fhttp_2fjson_5fbody"></a>

- `pub fn http.json_body(value: json.Value) -> Result[Body, HttpError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fhttp_2fparse_5fserver_5frequest"></a>

- `pub fn http.parse_server_request(buf: Bytes, max_head_bytes: u64, max_body_bytes: u64) -> Result[ Option[(ServerRequest, u64)], HttpError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest)

<a id="def-std_401_2fhttp_2fread_5fall"></a>

- `pub fn http.read_all(mut r: Request, limit: u64) -> Result[unique Bytes, HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fread_5fbody"></a>

- `@preserves_unique(out) pub fn http.read_body(mut r: Request, mut out: List[u8], max: u64) -> Result[u64, HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2freceive_5fhead"></a>

- `pub fn http.receive_head(mut r: Request) -> Result[ResponseHead, HttpError]`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/http/ResponseHead`](#def-std_401_2fhttp_2fResponseHead)

<a id="def-std_401_2fhttp_2freceive_5frequest"></a>

- `pub fn http.receive_request(mut c: ServerConnection, clock: Option[MonotonicClock]) -> Result[ ServerRequest, HttpError ]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection), [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest)

<a id="def-std_401_2fhttp_2frequest"></a>

- `pub fn http.request( n: Net, config: Config, method: Method, url: String, headers: List[Header] ) -> Result[Request, HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2frespond"></a>

- `pub fn http.respond( mut c: ServerConnection, status: u16, headers: List[Header], body: Bytes, close: bool ) -> Result[(), HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection)

<a id="def-std_401_2fhttp_2fresponse_5fjson"></a>

- `pub fn http.response_json(response: Response) -> Result[json.Value, HttpError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fhttp_2fsend_5fbodiless"></a>

- `pub fn http.send_bodiless(mut r: Request) -> Result[(), HttpError]`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fsend_5fbody"></a>

- `pub fn http.send_body(mut r: Request, body: Bytes) -> Result[(), HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fsend_5fchunk"></a>

- `pub fn http.send_chunk(mut r: Request, data: Bytes) -> Result[(), HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fserver_5fconnection"></a>

- `pub fn http.server_connection(n: Net, take stream: Stream of n, config: Config) -> ServerConnection`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Stream`](#def-core_401_2fStream), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection)

<a id="def-std_401_2fhttp_2ftrailers"></a>

- `pub fn http.trailers(r: Request) -> unique List[Header]`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)


## http_wire

<a id="def-std_401_2fhttp_5fwire_2fChunkError"></a>

- `pub type http_wire.ChunkError = InvalidChunkSize()`

<a id="def-std_401_2fhttp_5fwire_2fChunkError_23InvalidChunkSize"></a>

- `InvalidChunkSize() -> http_wire.ChunkError`

Variant of http_wire.ChunkError.

<a id="def-std_401_2fhttp_5fwire_2fChunkSize"></a>

- `pub type http_wire.ChunkSize = Complete(u64, u64) | Partial`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fChunkSize_23Complete"></a>

- `Complete(u64, u64) -> http_wire.ChunkSize`

Variant of http_wire.ChunkSize.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fChunkSize_23Partial"></a>

- `Partial -> http_wire.ChunkSize`

Variant of http_wire.ChunkSize.

<a id="def-std_401_2fhttp_5fwire_2fFraming"></a>

- `pub type http_wire.Framing { content_length: Option[u64], chunked: bool, keep_alive: bool, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fHeadError"></a>

- `pub type http_wire.HeadError = HeaderName | HeaderValue | NewLine | Status | Token | TooManyHeaders | Version | ContentLength | TransferEncoding | Continuation`

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23ContentLength"></a>

- `ContentLength -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23Continuation"></a>

- `Continuation -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23HeaderName"></a>

- `HeaderName -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23HeaderValue"></a>

- `HeaderValue -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23NewLine"></a>

- `NewLine -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23Status"></a>

- `Status -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23Token"></a>

- `Token -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23TooManyHeaders"></a>

- `TooManyHeaders -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23TransferEncoding"></a>

- `TransferEncoding -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeadError_23Version"></a>

- `Version -> http_wire.HeadError`

Variant of http_wire.HeadError.

<a id="def-std_401_2fhttp_5fwire_2fHeaderIterator"></a>

- `pub type http_wire.HeaderIterator { index: u64, is_trailer: bool, done: bool, }`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus"></a>

- `pub type http_wire.ParseStatus = Complete(u64) | Partial`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus_23Complete"></a>

- `Complete(u64) -> http_wire.ParseStatus`

Variant of http_wire.ParseStatus.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus_23Partial"></a>

- `Partial -> http_wire.ParseStatus`

Variant of http_wire.ParseStatus.

<a id="def-std_401_2fhttp_5fwire_2fRawHeader"></a>

- `pub type http_wire.RawHeader { name_start: u64, name_end: u64, value_start: u64, value_end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fRequestLine"></a>

- `pub type http_wire.RequestLine { method_start: u64, method_end: u64, target_start: u64, target_end: u64, version: u8, }`

References: [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fhttp_5fwire_2fStatusLine"></a>

- `pub type http_wire.StatusLine { version: u8, code: u16, reason_start: u64, reason_end: u64, }`

References: [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fhttp_5fwire_2ffind_5fheader"></a>

- `@noalloc pub fn http_wire.find_header[k: size]( buf: Bytes, headers: Inline[RawHeader, k], count: u64, name: Bytes ) -> Option[RawHeader] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader)

<a id="def-std_401_2fhttp_5fwire_2fframing"></a>

- `@noalloc pub fn http_wire.framing[k: size]( buf: Bytes, version: u8, headers: Inline[RawHeader, k], count: u64 ) -> Result[Framing, HeadError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/http_wire/Framing`](#def-std_401_2fhttp_5fwire_2fFraming), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader)

<a id="def-std_401_2fhttp_5fwire_2fheader_5fiterator"></a>

- `@noalloc pub fn http_wire.header_iterator(buf: Bytes) -> HeaderIterator uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/http_wire/HeaderIterator`](#def-std_401_2fhttp_5fwire_2fHeaderIterator)

<a id="def-std_401_2fhttp_5fwire_2fnext_5fheader"></a>

- `@noalloc pub fn http_wire.next_header(buf: Bytes, mut it: HeaderIterator) -> Option[RawHeader] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/http_wire/HeaderIterator`](#def-std_401_2fhttp_5fwire_2fHeaderIterator), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader)

<a id="def-std_401_2fhttp_5fwire_2fparse_5fchunk_5fsize"></a>

- `@noalloc pub fn http_wire.parse_chunk_size(buf: Bytes, start: u64) -> Result[ChunkSize, ChunkError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_wire/ChunkError`](#def-std_401_2fhttp_5fwire_2fChunkError), [`std@1/http_wire/ChunkSize`](#def-std_401_2fhttp_5fwire_2fChunkSize)

<a id="def-std_401_2fhttp_5fwire_2fparse_5fheaders"></a>

- `@noalloc pub fn http_wire.parse_headers[k: size]( buf: Bytes, start: u64, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/http_wire/ParseStatus`](#def-std_401_2fhttp_5fwire_2fParseStatus), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader)

<a id="def-std_401_2fhttp_5fwire_2fparse_5frequest"></a>

- `@noalloc pub fn http_wire.parse_request[k: size]( buf: Bytes, mut line: RequestLine, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/http_wire/ParseStatus`](#def-std_401_2fhttp_5fwire_2fParseStatus), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader), [`std@1/http_wire/RequestLine`](#def-std_401_2fhttp_5fwire_2fRequestLine)

<a id="def-std_401_2fhttp_5fwire_2fparse_5fresponse"></a>

- `@noalloc pub fn http_wire.parse_response[k: size]( buf: Bytes, mut line: StatusLine, mut headers: Inline[RawHeader, k], mut count: u64 ) -> Result[ParseStatus, HeadError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/http_wire/ParseStatus`](#def-std_401_2fhttp_5fwire_2fParseStatus), [`std@1/http_wire/RawHeader`](#def-std_401_2fhttp_5fwire_2fRawHeader), [`std@1/http_wire/StatusLine`](#def-std_401_2fhttp_5fwire_2fStatusLine)


## i128

<a id="def-std_401_2fi128_2fchecked_5fadd"></a>

- `@noalloc pub fn i128.checked_add(a: i128, b: i128) -> Option[i128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fchecked_5fmul"></a>

- `@noalloc pub fn i128.checked_mul(a: i128, b: i128) -> Option[i128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fchecked_5fsub"></a>

- `@noalloc pub fn i128.checked_sub(a: i128, b: i128) -> Option[i128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fformat"></a>

- `@noalloc pub fn i128.format[n: size](value: i128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i128`](#def-core_401_2fi128), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fi128_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn i128.format_into(value: i128, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i128`](#def-core_401_2fi128), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi128_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn i128.format_into_spec( value: i128, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/i128`](#def-core_401_2fi128), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi128_2fformat_5fradix"></a>

- `pub fn i128.format_radix(value: i128, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i128`](#def-core_401_2fi128), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fi128_2forder"></a>

- `const i128.order: Order[i128]`

<a id="def-std_401_2fi128_2fparse"></a>

- `@noalloc pub fn i128.parse(text: String) -> Result[i128, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i128`](#def-core_401_2fi128), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi128_2fparse_5fradix"></a>

- `@noalloc pub fn i128.parse_radix(text: String, radix: u8) -> Result[i128, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i128`](#def-core_401_2fi128), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi128_2fsaturating_5fadd"></a>

- `@noalloc pub fn i128.saturating_add(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fsaturating_5fmul"></a>

- `@noalloc pub fn i128.saturating_mul(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fsaturating_5fsub"></a>

- `@noalloc pub fn i128.saturating_sub(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fto_5fstring"></a>

- `pub fn i128.to_string(value: i128) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fwrapping_5fadd"></a>

- `@noalloc pub fn i128.wrapping_add(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fwrapping_5fmul"></a>

- `@noalloc pub fn i128.wrapping_mul(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2fi128_2fwrapping_5fsub"></a>

- `@noalloc pub fn i128.wrapping_sub(a: i128, b: i128) -> i128 uses {}`

References: [`core@1/i128`](#def-core_401_2fi128)


## i16

<a id="def-std_401_2fi16_2fchecked_5fadd"></a>

- `@noalloc pub fn i16.checked_add(a: i16, b: i16) -> Option[i16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fchecked_5fmul"></a>

- `@noalloc pub fn i16.checked_mul(a: i16, b: i16) -> Option[i16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fchecked_5fsub"></a>

- `@noalloc pub fn i16.checked_sub(a: i16, b: i16) -> Option[i16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fformat"></a>

- `@noalloc pub fn i16.format[n: size](value: i16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i16`](#def-core_401_2fi16), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fi16_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn i16.format_into(value: i16, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i16`](#def-core_401_2fi16), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi16_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn i16.format_into_spec( value: i16, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/i16`](#def-core_401_2fi16), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi16_2fformat_5fradix"></a>

- `pub fn i16.format_radix(value: i16, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i16`](#def-core_401_2fi16), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fi16_2forder"></a>

- `const i16.order: Order[i16]`

<a id="def-std_401_2fi16_2fparse"></a>

- `@noalloc pub fn i16.parse(text: String) -> Result[i16, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i16`](#def-core_401_2fi16), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi16_2fparse_5fradix"></a>

- `@noalloc pub fn i16.parse_radix(text: String, radix: u8) -> Result[i16, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i16`](#def-core_401_2fi16), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi16_2fsaturating_5fadd"></a>

- `@noalloc pub fn i16.saturating_add(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fsaturating_5fmul"></a>

- `@noalloc pub fn i16.saturating_mul(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fsaturating_5fsub"></a>

- `@noalloc pub fn i16.saturating_sub(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fto_5fstring"></a>

- `pub fn i16.to_string(value: i16) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fwrapping_5fadd"></a>

- `@noalloc pub fn i16.wrapping_add(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fwrapping_5fmul"></a>

- `@noalloc pub fn i16.wrapping_mul(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)

<a id="def-std_401_2fi16_2fwrapping_5fsub"></a>

- `@noalloc pub fn i16.wrapping_sub(a: i16, b: i16) -> i16 uses {}`

References: [`core@1/i16`](#def-core_401_2fi16)


## i32

<a id="def-std_401_2fi32_2fchecked_5fadd"></a>

- `@noalloc pub fn i32.checked_add(a: i32, b: i32) -> Option[i32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fchecked_5fmul"></a>

- `@noalloc pub fn i32.checked_mul(a: i32, b: i32) -> Option[i32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fchecked_5fsub"></a>

- `@noalloc pub fn i32.checked_sub(a: i32, b: i32) -> Option[i32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fformat"></a>

- `@noalloc pub fn i32.format[n: size](value: i32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fi32_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn i32.format_into(value: i32, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi32_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn i32.format_into_spec( value: i32, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi32_2fformat_5fradix"></a>

- `pub fn i32.format_radix(value: i32, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fi32_2forder"></a>

- `const i32.order: Order[i32]`

<a id="def-std_401_2fi32_2fparse"></a>

- `@noalloc pub fn i32.parse(text: String) -> Result[i32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi32_2fparse_5fradix"></a>

- `@noalloc pub fn i32.parse_radix(text: String, radix: u8) -> Result[i32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi32_2fsaturating_5fadd"></a>

- `@noalloc pub fn i32.saturating_add(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fsaturating_5fmul"></a>

- `@noalloc pub fn i32.saturating_mul(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fsaturating_5fsub"></a>

- `@noalloc pub fn i32.saturating_sub(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fto_5fstring"></a>

- `pub fn i32.to_string(value: i32) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fwrapping_5fadd"></a>

- `@noalloc pub fn i32.wrapping_add(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fwrapping_5fmul"></a>

- `@noalloc pub fn i32.wrapping_mul(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fi32_2fwrapping_5fsub"></a>

- `@noalloc pub fn i32.wrapping_sub(a: i32, b: i32) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32)


## i64

<a id="def-std_401_2fi64_2fchecked_5fadd"></a>

- `@noalloc pub fn i64.checked_add(a: i64, b: i64) -> Option[i64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fchecked_5fmul"></a>

- `@noalloc pub fn i64.checked_mul(a: i64, b: i64) -> Option[i64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fchecked_5fsub"></a>

- `@noalloc pub fn i64.checked_sub(a: i64, b: i64) -> Option[i64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fformat"></a>

- `@noalloc pub fn i64.format[n: size](value: i64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fi64_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn i64.format_into(value: i64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi64_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn i64.format_into_spec( value: i64, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi64_2fformat_5fradix"></a>

- `pub fn i64.format_radix(value: i64, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fi64_2forder"></a>

- `const i64.order: Order[i64]`

<a id="def-std_401_2fi64_2fparse"></a>

- `@noalloc pub fn i64.parse(text: String) -> Result[i64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi64_2fparse_5fradix"></a>

- `@noalloc pub fn i64.parse_radix(text: String, radix: u8) -> Result[i64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi64_2fsaturating_5fadd"></a>

- `@noalloc pub fn i64.saturating_add(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fsaturating_5fmul"></a>

- `@noalloc pub fn i64.saturating_mul(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fsaturating_5fsub"></a>

- `@noalloc pub fn i64.saturating_sub(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fto_5fstring"></a>

- `pub fn i64.to_string(value: i64) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fwrapping_5fadd"></a>

- `@noalloc pub fn i64.wrapping_add(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fwrapping_5fmul"></a>

- `@noalloc pub fn i64.wrapping_mul(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fi64_2fwrapping_5fsub"></a>

- `@noalloc pub fn i64.wrapping_sub(a: i64, b: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64)


## i8

<a id="def-std_401_2fi8_2fchecked_5fadd"></a>

- `@noalloc pub fn i8.checked_add(a: i8, b: i8) -> Option[i8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fchecked_5fmul"></a>

- `@noalloc pub fn i8.checked_mul(a: i8, b: i8) -> Option[i8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fchecked_5fsub"></a>

- `@noalloc pub fn i8.checked_sub(a: i8, b: i8) -> Option[i8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fformat"></a>

- `@noalloc pub fn i8.format[n: size](value: i8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i8`](#def-core_401_2fi8), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fi8_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn i8.format_into(value: i8, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i8`](#def-core_401_2fi8), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi8_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn i8.format_into_spec( value: i8, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/i8`](#def-core_401_2fi8), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fi8_2fformat_5fradix"></a>

- `pub fn i8.format_radix(value: i8, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i8`](#def-core_401_2fi8), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fi8_2forder"></a>

- `const i8.order: Order[i8]`

<a id="def-std_401_2fi8_2fparse"></a>

- `@noalloc pub fn i8.parse(text: String) -> Result[i8, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i8`](#def-core_401_2fi8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi8_2fparse_5fradix"></a>

- `@noalloc pub fn i8.parse_radix(text: String, radix: u8) -> Result[i8, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i8`](#def-core_401_2fi8), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fi8_2fsaturating_5fadd"></a>

- `@noalloc pub fn i8.saturating_add(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fsaturating_5fmul"></a>

- `@noalloc pub fn i8.saturating_mul(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fsaturating_5fsub"></a>

- `@noalloc pub fn i8.saturating_sub(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fto_5fstring"></a>

- `pub fn i8.to_string(value: i8) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fwrapping_5fadd"></a>

- `@noalloc pub fn i8.wrapping_add(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fwrapping_5fmul"></a>

- `@noalloc pub fn i8.wrapping_mul(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)

<a id="def-std_401_2fi8_2fwrapping_5fsub"></a>

- `@noalloc pub fn i8.wrapping_sub(a: i8, b: i8) -> i8 uses {}`

References: [`core@1/i8`](#def-core_401_2fi8)


## input

<a id="def-std_401_2finput_2fread_5fall"></a>

- `pub fn input.read_all(s: Stdin) -> Result[unique Bytes, InputError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin)

<a id="def-std_401_2finput_2fread_5fline"></a>

- `pub fn input.read_line(s: Stdin) -> Result[Option[unique String], InputError]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/String`](#def-core_401_2fString)


## instant

<a id="def-core_401_2finstant_2fchecked_5fadd"></a>

- `@noalloc fn instant.checked_add(clock: MonotonicClock, start: Instant of clock, nanoseconds: u64) -> Option[Instant of clock]`

<a id="def-core_401_2finstant_2fticks"></a>

- `@noalloc fn instant.ticks(value: Instant) -> u64 uses {}`


## int

<a id="def-core_401_2fint_2fchecked_5fadd"></a>

- `@noalloc fn int.checked_add[I](a: I, b: I) -> Option[I]`

<a id="def-core_401_2fint_2fdiv_5fexact"></a>

- `@noalloc fn int.div_exact[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fdiv_5ffloor"></a>

- `@noalloc fn int.div_floor[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fmod"></a>

- `@noalloc fn int.mod[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fnarrow"></a>

- `@noalloc fn int.narrow[I, J](value: I) -> J`

<a id="def-core_401_2fint_2fsaturating_5fadd"></a>

- `@noalloc fn int.saturating_add[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2ftruncate"></a>

- `@noalloc fn int.truncate[I, J](value: I) -> J`

<a id="def-core_401_2fint_2ftry_5fnarrow"></a>

- `@noalloc fn int.try_narrow[I, J](value: I) -> Option[J]`

<a id="def-core_401_2fint_2funchecked_5fadd"></a>

- `@noalloc fn int.unchecked_add[I](permit: Unsafe, a: I, b: I) -> I`

<a id="def-core_401_2fint_2fwiden"></a>

- `@noalloc fn int.widen[I, J](value: I) -> J`

<a id="def-core_401_2fint_2fwrapping_5fadd"></a>

- `@noalloc fn int.wrapping_add[I](a: I, b: I) -> I`


## io

<a id="def-core_401_2fio_2fdir"></a>

- `fn io.dir(rt: Io, p: Path) -> Result[Dir, FileError]`

<a id="def-core_401_2fio_2fmonotonic_5fclock"></a>

- `@noalloc fn io.monotonic_clock(rt: Io) -> Result[MonotonicClock, GrantError]`

<a id="def-core_401_2fio_2fnet"></a>

- `@noalloc fn io.net(rt: Io) -> Result[Net, GrantError]`

<a id="def-core_401_2fio_2fprocess"></a>

- `@noalloc fn io.process(rt: Io) -> Result[Proc, GrantError]`

<a id="def-core_401_2fio_2frandom"></a>

- `@noalloc fn io.random(rt: Io) -> Result[Random, GrantError]`

<a id="def-core_401_2fio_2fstderr"></a>

- `@noalloc fn io.stderr(rt: Io) -> Result[Stderr, GrantError]`

<a id="def-core_401_2fio_2fstdin"></a>

- `@noalloc fn io.stdin(rt: Io) -> Result[Stdin, GrantError]`

<a id="def-core_401_2fio_2fstdout"></a>

- `@noalloc fn io.stdout(rt: Io) -> Result[Stdout, GrantError]`

<a id="def-core_401_2fio_2ftasks"></a>

- `@noalloc fn io.tasks(rt: Io) -> Result[Tasks, GrantError]`

<a id="def-core_401_2fio_2fwall_5fclock"></a>

- `@noalloc fn io.wall_clock(rt: Io) -> Result[WallClock, GrantError]`


## iter

<a id="def-std_401_2fiter_2fcollect"></a>

- `pub fn iter.collect[T](take it: Iter[T]) -> List[T] uses {it}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2fiter_2fenumerate"></a>

- `@captures(it) pub fn iter.enumerate[T](take it: Iter[T]) -> Iter[(u64, T)] uses {it}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fiter_2ffilter"></a>

- `@captures(it, predicate) pub fn iter.filter[T](take it: Iter[T], take predicate: fn(T) -> bool) -> Iter[T] uses {it, predicate}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fiter_2ffold"></a>

- `pub fn iter.fold[T, A](take it: Iter[T], take initial: A, take f: fn(take A, take T) -> A) -> A uses {it, initial, f}`

References: [`core@1/Iter`](#def-core_401_2fIter)

<a id="def-core_401_2fiter_2ffrom"></a>

- `fn iter.from[S, T](take state: S, take next: fn(mut S) -> Option[T]) -> Iter[T] uses {state, next}`

<a id="def-core_401_2fiter_2fmap"></a>

- `fn iter.map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f}`

<a id="def-core_401_2fiter_2fnext"></a>

- `fn iter.next[T](mut it: Iter[T]) -> Option[T]`

<a id="def-std_401_2fiter_2ftake_5fn"></a>

- `@captures(it) pub fn iter.take_n[T](take it: Iter[T], count: u64) -> Iter[T] uses {it}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fiter_2fzip"></a>

- `@captures(a, b) pub fn iter.zip[T, U](take a: Iter[T], take b: Iter[U]) -> Iter[(T, U)] uses {a, b}`

References: [`core@1/Iter`](#def-core_401_2fIter)


## json

<a id="def-std_401_2fjson_2fJsonError"></a>

- `pub type json.JsonError = Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fJsonError_23DuplicateKey"></a>

- `DuplicateKey(u64) -> json.JsonError`

Variant of json.JsonError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fJsonError_23InvalidNumber"></a>

- `InvalidNumber -> json.JsonError`

Variant of json.JsonError.

<a id="def-std_401_2fjson_2fJsonError_23Nesting"></a>

- `Nesting(u64) -> json.JsonError`

Variant of json.JsonError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fJsonError_23Syntax"></a>

- `Syntax(u64) -> json.JsonError`

Variant of json.JsonError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fMAX_5fNESTING"></a>

- `pub const json.MAX_NESTING: u64`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fValue"></a>

- `pub type json.Value = Null | Bool(bool) | Number(String) | Text(String) | Array( List[Value] ) | Object(Map[String, Value])`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fjson_2fValue_23Array"></a>

- `Array( List[Value] ) -> json.Value`

Variant of json.Value.

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2fjson_2fValue_23Bool"></a>

- `Bool(bool) -> json.Value`

Variant of json.Value.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fjson_2fValue_23Null"></a>

- `Null -> json.Value`

Variant of json.Value.

<a id="def-std_401_2fjson_2fValue_23Number"></a>

- `Number(String) -> json.Value`

Variant of json.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fValue_23Object"></a>

- `Object(Map[String, Value]) -> json.Value`

Variant of json.Value.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fValue_23Text"></a>

- `Text(String) -> json.Value`

Variant of json.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fparse"></a>

- `pub fn json.parse(take text: String) -> Result[Value, JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_2fstringify"></a>

- `pub fn json.stringify(value: Value) -> Result[unique String, JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_2fwrite"></a>

- `@preserves_unique(builder) pub fn json.write(value: Value, mut builder: List[u8]) -> Result[(), JsonError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)


## list

<a id="def-std_401_2flist_2fall"></a>

- `@noalloc pub fn list.all[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flist_2fany"></a>

- `@noalloc pub fn list.any[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flist_2fbinary_5fsearch"></a>

- `@noalloc pub fn list.binary_search[T]( xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {} ) -> Option[u64] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flist_2fconcat"></a>

- `pub fn list.concat[T](take a: List[T], take b: List[T]) -> List[T] uses {a, b}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2fdedup"></a>

- `pub fn list.dedup[T: data](take xs: List[T]) -> List[T] uses {xs}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2ffilter"></a>

- `pub fn list.filter[T](take xs: List[T], predicate: fn(T) -> bool) -> List[T] uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flist_2ffind"></a>

- `@noalloc pub fn list.find[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> Option[u64] uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flist_2ffold"></a>

- `pub fn list.fold[T, A](xs: List[T], take initial: A, f: fn(take A, T) -> A) -> A uses {xs, initial, f}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-core_401_2flist_2flen"></a>

- `@noalloc fn list.len[T](xs: List[T]) -> u64`

Number of elements in `xs: List[T]`.

<a id="def-std_401_2flist_2fmap"></a>

- `pub fn list.map[T, U](xs: List[T], f: fn(T) -> U) -> List[U] uses {xs, f}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2fpartition"></a>

- `pub fn list.partition[T](take xs: List[T], predicate: fn(T) -> bool) -> (List[T], List[T]) uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-core_401_2flist_2fpop"></a>

- `@noalloc fn list.pop[T](mut xs: unique List[T]) -> Option[T]`

<a id="def-core_401_2flist_2fpush"></a>

- `@preserves_unique(xs) fn list.push[T](mut xs: List[T], take value: T) -> ()`

Appends `v: T` to a mutable `List[T]`.

<a id="def-core_401_2flist_2fremove"></a>

- `@noalloc fn list.remove[T](mut xs: unique List[T], index: u64) -> T`

<a id="def-core_401_2flist_2freserve"></a>

- `@preserves_unique(xs) fn list.reserve[T](mut xs: List[T], capacity: u64) -> ()`

<a id="def-std_401_2flist_2freverse"></a>

- `@noalloc pub fn list.reverse[T](mut xs: unique List[T]) -> ()`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2frotate"></a>

- `@noalloc pub fn list.rotate[T](mut xs: unique List[T], amount: u64) -> ()`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flist_2fsort"></a>

- `@noalloc pub fn list.sort[T](mut xs: unique List[T], by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-core_401_2flist_2fswap"></a>

- `@noalloc fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()`


## log

<a id="def-std_401_2flog_2fEntry"></a>

- `pub type log.Entry { level: Level, message: String, fields: List[Field], timestamp: Option[time.Timestamp], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/log/Field`](#def-std_401_2flog_2fField), [`std@1/log/Level`](#def-std_401_2flog_2fLevel), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2flog_2fField"></a>

- `pub type log.Field { name: String, value: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2flog_2fLevel"></a>

- `pub type log.Level = Debug | Info | Warn | Error`

<a id="def-std_401_2flog_2fLevel_23Debug"></a>

- `Debug -> log.Level`

Variant of log.Level.

<a id="def-std_401_2flog_2fLevel_23Error"></a>

- `Error -> log.Level`

Variant of log.Level.

<a id="def-std_401_2flog_2fLevel_23Info"></a>

- `Info -> log.Level`

Variant of log.Level.

<a id="def-std_401_2flog_2fLevel_23Warn"></a>

- `Warn -> log.Level`

Variant of log.Level.

<a id="def-std_401_2flog_2fformat"></a>

- `pub fn log.format(entry: Entry) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/log/Entry`](#def-std_401_2flog_2fEntry)

<a id="def-std_401_2flog_2fwrite"></a>

- `pub fn log.write(sink: Stderr, minimum: Level, entry: Entry) -> Result[(), OutputError]`

References: [`core@1/OutputError`](#def-core_401_2fOutputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stderr`](#def-core_401_2fStderr), [`std@1/log/Entry`](#def-std_401_2flog_2fEntry), [`std@1/log/Level`](#def-std_401_2flog_2fLevel)


## map

<a id="def-core_401_2fmap_2fcontains"></a>

- `@noalloc fn map.contains[K: key, V](m: Map[K, V], k: K) -> bool`

<a id="def-core_401_2fmap_2fempty"></a>

- `fn map.empty[K: key, V]() -> unique Map[K, V]`

<a id="def-core_401_2fmap_2fget"></a>

- `@noalloc fn map.get[K: key, V](m: Map[K, V], k: K) -> Option[V] in m`

<a id="def-core_401_2fmap_2finsert"></a>

- `@preserves_unique(m) fn map.insert[K: key, V](mut m: Map[K, V], take k: K, take v: V) -> ()`

<a id="def-core_401_2fmap_2flen"></a>

- `@noalloc fn map.len[K: key, V](m: Map[K, V]) -> u64`

<a id="def-core_401_2fmap_2fremove"></a>

- `fn map.remove[K: key, V](mut m: unique Map[K, V], k: K) -> unique Option[V]`

<a id="def-std_401_2fmap_2fsame_5fentries"></a>

- `@noalloc pub fn map.same_entries[K: key, V: data](a: Map[K, V], b: Map[K, V]) -> bool`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/bool`](#def-core_401_2fbool)


## mutex

<a id="def-core_401_2fmutex_2fcreate"></a>

- `fn mutex.create[T: sendable](tasks: Tasks, take v: unique T) -> Mutex[T]`

<a id="def-core_401_2fmutex_2fwith"></a>

- `fn mutex.with[T, R: sendable](tasks: Tasks, m: Mutex[T], body: fn(mut v: unique T) -> R uses {}) -> R`


## net

<a id="def-std_401_2fnet_2fAddrParseError"></a>

- `pub type net.AddrParseError = Invalid()`

<a id="def-std_401_2fnet_2fAddrParseError_23Invalid"></a>

- `Invalid() -> net.AddrParseError`

Variant of net.AddrParseError.

<a id="def-std_401_2fnet_2fHostError"></a>

- `pub type net.HostError = NameTooLong | InvalidHostName`

<a id="def-std_401_2fnet_2fHostError_23InvalidHostName"></a>

- `InvalidHostName -> net.HostError`

Variant of net.HostError.

<a id="def-std_401_2fnet_2fHostError_23NameTooLong"></a>

- `NameTooLong -> net.HostError`

Variant of net.HostError.

<a id="def-core_401_2fnet_2faccept"></a>

- `fn net.accept(n: Net, mut l: Listener of n) -> Result[(Stream of n, SocketAddr), NetError]`

the listener's grant; the peer is not checked

<a id="def-core_401_2fnet_2fclose"></a>

- `@noalloc fn net.close(n: Net, take s: Stream of n) -> ()`

<a id="def-core_401_2fnet_2fconnect"></a>

- `fn net.connect(n: Net, host: String, port: u16, timeout: NetTimeout) -> Result[Stream of n, NetError]`

`(host, port, Connect())` or `(host, port, ConnectTls())`

<a id="def-core_401_2fnet_2fconnect_5faddr"></a>

- `fn net.connect_addr(n: Net, addr: SocketAddr, timeout: NetTimeout) -> Result[Stream of n, NetError]`

a rule whose address is the canonical literal of `addr.ip`, with `addr.port`; a host-name rule never authorizes a raw address

<a id="def-std_401_2fnet_2fends_5fin_5fnumber"></a>

- `@noalloc pub fn net.ends_in_number(text: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fnet_2fformat_5fip"></a>

- `pub fn net.format_ip(ip: IpAddr) -> unique String uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fnet_2fformat_5fsocket_5faddr"></a>

- `pub fn net.format_socket_addr(addr: SocketAddr) -> unique String uses {}`

References: [`core@1/SocketAddr`](#def-core_401_2fSocketAddr), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fnet_2flisten"></a>

- `fn net.listen(n: Net, addr: SocketAddr) -> Result[Listener of n, NetError]`

`(literal, port, Listen())` for `addr`; port 0 grants an ephemeral port

<a id="def-core_401_2fnet_2flocal_5faddr"></a>

- `@noalloc fn net.local_addr(n: Net, l: Listener of n) -> Result[SocketAddr, NetError]`

<a id="def-core_401_2fnet_2fnarrow"></a>

- `fn net.narrow(n: Net, rules: List[NetRule]) -> Result[Net, NetError]`

<a id="def-std_401_2fnet_2fparse_5fip"></a>

- `@noalloc pub fn net.parse_ip(text: String) -> Result[IpAddr, AddrParseError] uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/net/AddrParseError`](#def-std_401_2fnet_2fAddrParseError)

<a id="def-std_401_2fnet_2fparse_5fipv4"></a>

- `@noalloc pub fn net.parse_ipv4(text: String) -> Result[Inline[u8, 4], AddrParseError] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/net/AddrParseError`](#def-std_401_2fnet_2fAddrParseError)

<a id="def-std_401_2fnet_2fparse_5fipv6"></a>

- `@noalloc pub fn net.parse_ipv6(text: String) -> Result[Inline[u8, 16], AddrParseError] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/net/AddrParseError`](#def-std_401_2fnet_2fAddrParseError)

<a id="def-std_401_2fnet_2fparse_5fsocket_5faddr"></a>

- `@noalloc pub fn net.parse_socket_addr(text: String) -> Result[SocketAddr, AddrParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/SocketAddr`](#def-core_401_2fSocketAddr), [`core@1/String`](#def-core_401_2fString), [`std@1/net/AddrParseError`](#def-std_401_2fnet_2fAddrParseError)

<a id="def-core_401_2fnet_2fpeer_5faddr"></a>

- `@noalloc fn net.peer_addr(n: Net, s: Stream of n) -> Result[SocketAddr, NetError]`

<a id="def-core_401_2fnet_2fread"></a>

- `fn net.read(n: Net, mut s: Stream of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fread_5finto"></a>

- `@noalloc fn net.read_into[k: size](n: Net, mut s: Stream of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fresolve"></a>

- `fn net.resolve(n: Net, host: String, port: u16) -> Result[List[SocketAddr], NetError]`

a rule naming `(host, port)`, any operation

<a id="def-core_401_2fnet_2fset_5fread_5ftimeout"></a>

- `@noalloc fn net.set_read_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]`

<a id="def-core_401_2fnet_2fset_5fwrite_5ftimeout"></a>

- `@noalloc fn net.set_write_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]`

<a id="def-core_401_2fnet_2fshutdown"></a>

- `@noalloc fn net.shutdown(n: Net, mut s: Stream of n, how: Shutdown) -> Result[(), NetError]`

<a id="def-core_401_2fnet_2fudp_5fbind"></a>

- `fn net.udp_bind(n: Net, addr: SocketAddr) -> Result[unique UdpSocket of n, NetError]`

`(literal, port, UdpBind())` for `addr`; port 0 permits an ephemeral port

<a id="def-std_401_2fnet_2fvalidate_5fhost"></a>

- `@noalloc pub fn net.validate_host(text: String) -> Result[(), HostError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/net/HostError`](#def-std_401_2fnet_2fHostError)

<a id="def-core_401_2fnet_2fwrite"></a>

- `@noalloc fn net.write(n: Net, mut s: Stream of n, data: Bytes) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fwrite_5fall"></a>

- `@noalloc fn net.write_all(n: Net, mut s: Stream of n, data: Bytes) -> Result[(), NetError]`


## option

<a id="def-std_401_2foption_2fand_5fthen"></a>

- `pub fn option.and_then[T, U](take value: Option[T], take f: once fn(take T) -> Option[U]) -> Option[ U ] uses {value, f}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2ffilter"></a>

- `pub fn option.filter[T](take value: Option[T], take predicate: once fn(T) -> bool) -> Option[T] uses {value, predicate}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2foption_2fflatten"></a>

- `@preserves_unique(value) pub fn option.flatten[T](take value: Option[Option[T]]) -> Option[T] uses {value}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2fis_5fnone"></a>

- `@noalloc pub fn option.is_none[T](value: Option[T]) -> bool uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2foption_2fis_5fsome"></a>

- `@noalloc pub fn option.is_some[T](value: Option[T]) -> bool uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2foption_2fmap"></a>

- `pub fn option.map[T, U](take value: Option[T], take f: once fn(take T) -> U) -> Option[U] uses {value, f}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2fok_5for"></a>

- `@preserves_unique(value) pub fn option.ok_or[T, E](take value: Option[T], take error: E) -> Result[T, E] uses {value, error}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2foption_2fok_5for_5felse"></a>

- `pub fn option.ok_or_else[T, E](take value: Option[T], take error: once fn() -> E) -> Result[ T, E ] uses {value, error}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2foption_2for_5felse"></a>

- `pub fn option.or_else[T](take value: Option[T], take fallback: once fn() -> Option[T]) -> Option[ T ] uses {value, fallback}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2fremove"></a>

- `@preserves_unique(value) pub fn option.remove[T](mut value: Option[T]) -> Option[T]`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2funwrap_5for"></a>

- `@preserves_unique(value) pub fn option.unwrap_or[T](take value: Option[T], take fallback: T) -> T uses {value, fallback}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2funwrap_5for_5felse"></a>

- `pub fn option.unwrap_or_else[T](take value: Option[T], take fallback: once fn() -> T) -> T uses {value, fallback}`

References: [`core@1/Option`](#def-core_401_2fOption)

<a id="def-std_401_2foption_2fzip"></a>

- `@preserves_unique(left) pub fn option.zip[T, U](take left: Option[T], take right: Option[U]) -> Option[(T, U)] uses {left, right}`

References: [`core@1/Option`](#def-core_401_2fOption)


## out

<a id="def-core_401_2fout_2fprint"></a>

- `@noalloc fn out.print(s: Stdout, text: String) -> Result[(), OutputError]`


## path

<a id="def-std_401_2fpath_2fPathError"></a>

- `pub type path.PathError = Invalid(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fpath_2fPathError_23Invalid"></a>

- `Invalid(u64) -> path.PathError`

Variant of path.PathError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fpath_2fcomponents"></a>

- `pub fn path.components(p: Path) -> unique List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fpath_2fextension"></a>

- `pub fn path.extension(p: Path) -> unique String uses {}`

References: [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fpath_2ffilename"></a>

- `pub fn path.filename(p: Path) -> unique String uses {}`

References: [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fpath_2fjoin"></a>

- `pub fn path.join(base: Path, child: Path) -> Result[Path, PathError] uses {}`

References: [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult), [`std@1/path/PathError`](#def-std_401_2fpath_2fPathError)

<a id="def-std_401_2fpath_2fnormalize"></a>

- `pub fn path.normalize(p: Path) -> Result[Path, PathError] uses {}`

References: [`core@1/Path`](#def-core_401_2fPath), [`core@1/Result`](#def-core_401_2fResult), [`std@1/path/PathError`](#def-std_401_2fpath_2fPathError)

<a id="def-core_401_2fpath_2fof"></a>

- `fn path.of(text: String) -> Path`

<a id="def-std_401_2fpath_2fparent"></a>

- `pub fn path.parent(p: Path) -> Option[Path] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Path`](#def-core_401_2fPath)

<a id="def-core_401_2fpath_2ftext"></a>

- `fn path.text(p: Path) -> String`


## priority_queue

<a id="def-std_401_2fpriority_5fqueue_2fHeap"></a>

- `pub opaque type priority_queue.Heap[T]: () uses {T}`

<a id="def-std_401_2fpriority_5fqueue_2fempty"></a>

- `pub fn priority_queue.empty[T]() -> unique Heap[T] uses {}`

References: [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2flen"></a>

- `@noalloc pub fn priority_queue.len[T](h: Heap[T]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpop"></a>

- `pub fn priority_queue.pop[T](mut h: unique Heap[T], by: @noalloc fn(T, T) -> i32 uses {}) -> unique Option[ T ] uses {h}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpush"></a>

- `pub fn priority_queue.push[T]( mut h: unique Heap[T], take value: unique T, by: @noalloc fn(T, T) -> i32 uses {} ) -> () uses {h, value}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)


## prng

<a id="def-std_401_2fprng_2fGenerator"></a>

- `pub opaque type prng.Generator: inert + noalloc_drop uses {}`

<a id="def-std_401_2fprng_2fSampleError"></a>

- `pub type prng.SampleError = EmptyRange()`

<a id="def-std_401_2fprng_2fSampleError_23EmptyRange"></a>

- `EmptyRange() -> prng.SampleError`

Variant of prng.SampleError.

<a id="def-std_401_2fprng_2fbelow"></a>

- `@noalloc pub fn prng.below(mut generator: Generator, upper_exclusive: u64) -> Result[u64, SampleError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator), [`std@1/prng/SampleError`](#def-std_401_2fprng_2fSampleError)

<a id="def-std_401_2fprng_2ffill"></a>

- `pub fn prng.fill(mut generator: Generator, mut out: List[u8], count: u64) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator)

<a id="def-std_401_2fprng_2fnext_5fu64"></a>

- `@noalloc pub fn prng.next_u64(mut generator: Generator) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator)

<a id="def-std_401_2fprng_2fseed"></a>

- `@noalloc pub fn prng.seed(value: u64) -> Generator uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator)


## probe

<a id="def-std_401_2fprobe_2fidentity"></a>

- `@noalloc pub fn probe.identity(value: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)


## proc

<a id="def-core_401_2fproc_2frun"></a>

- `fn proc.run(p: Proc, program: Path, args: List[String], stdin: Bytes, dir: Option[Dir], max_output: u64, deadline: Option[Deadline]) -> Result[ProcessOutput, ProcessError]`

<a id="def-core_401_2fproc_2fspawn"></a>

- `fn proc.spawn(p: Proc, program: Path, args: List[String], dir: Option[Dir]) -> Result[Child of p, ProcessError]`


## random

<a id="def-std_401_2frandom_2fSampleError"></a>

- `pub type random.SampleError = EmptyRange | Source(RandomError)`

References: [`core@1/RandomError`](#def-core_401_2fRandomError)

<a id="def-std_401_2frandom_2fSampleError_23EmptyRange"></a>

- `EmptyRange -> random.SampleError`

Variant of random.SampleError.

<a id="def-std_401_2frandom_2fSampleError_23Source"></a>

- `Source(RandomError) -> random.SampleError`

Variant of random.SampleError.

References: [`core@1/RandomError`](#def-core_401_2fRandomError)

<a id="def-std_401_2frandom_2fbelow"></a>

- `pub fn random.below(r: Random, upper_exclusive: u64) -> Result[u64, SampleError]`

References: [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`std@1/random/SampleError`](#def-std_401_2frandom_2fSampleError), [`std@1/random/u64`](#def-std_401_2frandom_2fu64)

<a id="def-core_401_2frandom_2fbytes"></a>

- `fn random.bytes(r: Random, count: u64) -> Result[Bytes, RandomError]`

<a id="def-std_401_2frandom_2fu64"></a>

- `pub fn random.u64(r: Random) -> Result[u64, RandomError]`

References: [`core@1/Random`](#def-core_401_2fRandom), [`core@1/RandomError`](#def-core_401_2fRandomError), [`core@1/Result`](#def-core_401_2fResult)


## regex

<a id="def-std_401_2fregex_2fCompileError"></a>

- `pub type regex.CompileError = Syntax(u64) | ProgramTooLarge`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fCompileError_23ProgramTooLarge"></a>

- `ProgramTooLarge -> regex.CompileError`

Variant of regex.CompileError.

<a id="def-std_401_2fregex_2fCompileError_23Syntax"></a>

- `Syntax(u64) -> regex.CompileError`

Variant of regex.CompileError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fMatchError"></a>

- `pub type regex.MatchError = WorkLimit()`

<a id="def-std_401_2fregex_2fMatchError_23WorkLimit"></a>

- `WorkLimit() -> regex.MatchError`

Variant of regex.MatchError.

<a id="def-std_401_2fregex_2fRegex"></a>

- `pub opaque type regex.Regex: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fregex_2fSpan"></a>

- `pub type regex.Span { start: u64, end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fcompile"></a>

- `pub fn regex.compile(pattern: String, max_instructions: u64) -> Result[Regex, CompileError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/CompileError`](#def-std_401_2fregex_2fCompileError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2ffind"></a>

- `pub fn regex.find(re: Regex, text: String, work_limit: u64) -> Result[ Option[Span], MatchError ] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex), [`std@1/regex/Span`](#def-std_401_2fregex_2fSpan)

<a id="def-std_401_2fregex_2fis_5fmatch"></a>

- `pub fn regex.is_match(re: Regex, text: String, work_limit: u64) -> Result[bool, MatchError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)


## region

<a id="def-core_401_2fregion_2fwith"></a>

- `@noalloc fn region.with[T, R](mut r: Region[T], body: fn(root: T) -> R) -> R`


## reservations

<a id="def-core_401_2freservations_2finsert"></a>

- `fn reservations.insert(tx: Tx, value: Reservation) -> Result[Reservation, StockError]`


## results

<a id="def-std_401_2fresults_2fand_5fthen"></a>

- `pub fn results.and_then[T, U, E]( take value: Result[T, E], take f: once fn(take T) -> Result[U, E] ) -> Result[U, E] uses {value, f}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2ferror"></a>

- `@preserves_unique(value) pub fn results.error[T, E](take value: Result[T, E]) -> Option[E] uses {value}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2fflatten"></a>

- `@preserves_unique(value) pub fn results.flatten[T, E](take value: Result[Result[T, E], E]) -> Result[T, E] uses {value}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2fis_5ferror"></a>

- `@noalloc pub fn results.is_error[T, E](value: Result[T, E]) -> bool uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fresults_2fis_5fok"></a>

- `@noalloc pub fn results.is_ok[T, E](value: Result[T, E]) -> bool uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fresults_2fmap"></a>

- `pub fn results.map[T, U, E](take value: Result[T, E], take f: once fn(take T) -> U) -> Result[ U, E ] uses {value, f}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2fmap_5ferror"></a>

- `pub fn results.map_error[T, E, F](take value: Result[T, E], take f: once fn(take E) -> F) -> Result[ T, F ] uses {value, f}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2fok"></a>

- `@preserves_unique(value) pub fn results.ok[T, E](take value: Result[T, E]) -> Option[T] uses {value}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2for_5felse"></a>

- `pub fn results.or_else[T, E, F]( take value: Result[T, E], take f: once fn(take E) -> Result[T, F] ) -> Result[T, F] uses {value, f}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2funwrap_5for"></a>

- `@preserves_unique(value) pub fn results.unwrap_or[T, E](take value: Result[T, E], take fallback: T) -> T uses {value, fallback}`

References: [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fresults_2funwrap_5for_5felse"></a>

- `pub fn results.unwrap_or_else[T, E]( take value: Result[T, E], take fallback: once fn(take E) -> T ) -> T uses {value, fallback}`

References: [`core@1/Result`](#def-core_401_2fResult)


## scalar

<a id="def-std_401_2fscalar_2fFormatError"></a>

- `pub type scalar.FormatError = BufferTooSmall()`

<a id="def-std_401_2fscalar_2fFormatError_23BufferTooSmall"></a>

- `BufferTooSmall() -> scalar.FormatError`

Variant of scalar.FormatError.

<a id="def-std_401_2fscalar_2fParseError"></a>

- `pub type scalar.ParseError = Invalid | OutOfRange`

<a id="def-std_401_2fscalar_2fParseError_23Invalid"></a>

- `Invalid -> scalar.ParseError`

Variant of scalar.ParseError.

<a id="def-std_401_2fscalar_2fParseError_23OutOfRange"></a>

- `OutOfRange -> scalar.ParseError`

Variant of scalar.ParseError.


## sender

<a id="def-core_401_2fsender_2fdup"></a>

- `fn sender.dup[T](tasks: Tasks, s: Sender[T]) -> Sender[T]`


## set

<a id="def-core_401_2fset_2fcontains"></a>

- `@noalloc fn set.contains[T: key](s: Set[T], value: T) -> bool`

<a id="def-core_401_2fset_2fempty"></a>

- `fn set.empty[T: key]() -> unique Set[T]`

<a id="def-core_401_2fset_2finsert"></a>

- `@preserves_unique(s) fn set.insert[T: key](mut s: Set[T], take value: T) -> ()`

<a id="def-core_401_2fset_2flen"></a>

- `@noalloc fn set.len[T: key](s: Set[T]) -> u64`

<a id="def-core_401_2fset_2fremove"></a>

- `fn set.remove[T: key](mut s: unique Set[T], value: T) -> bool`


## sha256

<a id="def-std_401_2fsha256_2fState"></a>

- `pub opaque type sha256.State: plain + inert uses {}`

<a id="def-std_401_2fsha256_2fdigest"></a>

- `@noalloc pub fn sha256.digest(data: Bytes) -> Inline[u8, 32] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha256_2fdigest_5fhex"></a>

- `pub fn sha256.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha256_2ffinish"></a>

- `@noalloc pub fn sha256.finish(take state: State) -> Inline[u8, 32] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)

<a id="def-std_401_2fsha256_2fnew"></a>

- `@noalloc pub fn sha256.new() -> State uses {}`

References: [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)

<a id="def-std_401_2fsha256_2fupdate"></a>

- `@noalloc pub fn sha256.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)


## shared

<a id="def-core_401_2fshared_2ffreeze"></a>

- `fn shared.freeze[T: inert](take x: unique T) -> Shared[T]`

<a id="def-core_401_2fshared_2fthaw"></a>

- `fn shared.thaw[T](take s: Shared[T]) -> Result[unique T, Shared[T]]`


## simd

<a id="def-core_401_2fsimd_2fadd"></a>

- `@noalloc fn simd.add[I, n: size](a: Simd[I, n], b: Simd[I, n]) -> Simd[I, n]`

<a id="def-core_401_2fsimd_2fsplat"></a>

- `@noalloc fn simd.splat[A, n: size](value: A) -> Simd[A, n]`


## slots

<a id="def-core_401_2fslots_2fempty"></a>

- `fn slots.empty[T]() -> Slots[T]`

<a id="def-core_401_2fslots_2fget"></a>

- `@noalloc fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s`

<a id="def-core_401_2fslots_2finsert"></a>

- `@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s`

<a id="def-core_401_2fslots_2fremove"></a>

- `fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]`


## sorted_map

<a id="def-core_401_2fsorted_5fmap_2fempty"></a>

- `fn sorted_map.empty[K, V](by: Order[K]) -> unique SortedMap[K, V] of by`

<a id="def-core_401_2fsorted_5fmap_2fget"></a>

- `fn sorted_map.get[K, V: copyable](by: Order[K], m: SortedMap[K, V] of by, key: K) -> Option[V] in m`

<a id="def-core_401_2fsorted_5fmap_2finsert"></a>

- `@preserves_unique(m) @unretained(by) fn sorted_map.insert[K, V](by: Order[K], mut m: SortedMap[K, V] of by, take k: K, take v: V) -> ()`

<a id="def-core_401_2fsorted_5fmap_2flen"></a>

- `@noalloc fn sorted_map.len[K, V](by: Order[K], m: SortedMap[K, V] of by) -> u64`

<a id="def-core_401_2fsorted_5fmap_2fremove"></a>

- `fn sorted_map.remove[K, V](by: Order[K], mut m: unique SortedMap[K, V] of by, key: K) -> unique Option[V]`


## stdin

<a id="def-core_401_2fstdin_2fread"></a>

- `@preserves_unique(buf) @unretained(s) fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]`


## stock

<a id="def-core_401_2fstock_2fget"></a>

- `fn stock.get(tx: Tx, sku: Sku) -> Result[u32, StockError]`

<a id="def-core_401_2fstock_2fput"></a>

- `fn stock.put(tx: Tx, sku: Sku, n: u32) -> Result[(), StockError]`


## store

<a id="def-core_401_2fstore_2ftransact"></a>

- `fn store.transact[R: inert, E: inert](db: Db, body: fn(tx: Tx) -> Result[R, E] uses {}) -> Result[R, TxFailure[E]]`


## string

<a id="def-core_401_2fstring_2fas_5fbytes"></a>

- `@noalloc fn string.as_bytes(s: String) -> Bytes in s`

<a id="def-std_401_2fstring_2fascii_5flower"></a>

- `pub fn string.ascii_lower(s: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fascii_5fupper"></a>

- `pub fn string.ascii_upper(s: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fstring_2fbyte_5flen"></a>

- `@noalloc fn string.byte_len(s: String) -> u64`

<a id="def-core_401_2fstring_2fbyte_5foffset"></a>

- `@noalloc fn string.byte_offset(s: String, pos: StrPos of s) -> u64`

<a id="def-std_401_2fstring_2fcasefold"></a>

- `pub fn string.casefold(s: String) -> unique String uses {}`

Full default Unicode16 case folding (CaseFolding C+F). Locale independent;
does not normalize text. Turkic-specific mappings are excluded.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fcompare"></a>

- `@noalloc pub fn string.compare(a: String, b: String) -> i32 uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fstring_2fconcat"></a>

- `pub fn string.concat(a: String, b: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fcontains"></a>

- `@noalloc pub fn string.contains(s: String, pattern: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fstring_2fends_5fwith"></a>

- `@noalloc pub fn string.ends_with(s: String, pattern: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fstring_2feq_5fignore_5fcase"></a>

- `pub fn string.eq_ignore_case(a: String, b: String) -> bool uses {}`

Compare full default case folds. Normalize explicitly when canonical or
compatibility equivalence is also wanted.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fstring_2ffind"></a>

- `@noalloc pub fn string.find(s: String, pattern: String) -> Option[StrPos of s] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/StrPos`](#def-core_401_2fStrPos), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fstring_2ffrom_5fbytes"></a>

- `@preserves_unique(bytes) fn string.from_bytes(take bytes: Bytes) -> Result[String, Bytes]`

<a id="def-std_401_2fstring_2fjoin"></a>

- `pub fn string.join(parts: List[String], separator: String) -> unique String uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2flines"></a>

- `pub fn string.lines(take s: String) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fstring_2fposition"></a>

- `@noalloc fn string.position(s: String, offset: u64) -> Option[StrPos of s]`

<a id="def-core_401_2fstring_2fpositions"></a>

- `@noalloc fn string.positions(s: String) -> PositionView of s in s`

<a id="def-std_401_2fstring_2freplace"></a>

- `pub fn string.replace(s: String, old: String, new: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fstring_2fscalars"></a>

- `@noalloc fn string.scalars(s: String) -> ScalarView in s`

<a id="def-core_401_2fstring_2fslice"></a>

- `@noalloc fn string.slice(s: String, start: StrPos of s, end: StrPos of s) -> String in s`

<a id="def-std_401_2fstring_2fsplit"></a>

- `pub fn string.split(take s: String, separator: String) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fsplit_5fascii_5fwhitespace"></a>

- `pub fn string.split_ascii_whitespace(take s: String) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fsplit_5fonce"></a>

- `@noalloc pub fn string.split_once(s: String, separator: String) -> Option[(String, String)] in s uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fstarts_5fwith"></a>

- `@noalloc pub fn string.starts_with(s: String, pattern: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fstring_2fto_5flower"></a>

- `pub fn string.to_lower(s: String) -> unique String uses {}`

Full Unicode16 lowercase, including expansions and context-sensitive final
sigma. Uses the original slice as context; locale independent.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fto_5fupper"></a>

- `pub fn string.to_upper(s: String) -> unique String uses {}`

Full Unicode16 uppercase, including expansions; locale independent.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2ftrim"></a>

- `@noalloc pub fn string.trim(s: String) -> String in s uses {}`

References: [`core@1/String`](#def-core_401_2fString)


## task

<a id="def-core_401_2ftask_2fadd"></a>

- `@noalloc fn task.add[R](s: Scope, mut set: TaskSet[R] of s, take h: Task[R] of s) -> ()`

<a id="def-std_401_2ftask_2fall"></a>

- `pub fn task.all[T, E](s: Scope, take set: TaskSet[Result[T, E]] of s) -> Result[List[T], E]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Scope`](#def-core_401_2fScope), [`core@1/TaskSet`](#def-core_401_2fTaskSet)

<a id="def-core_401_2ftask_2fasync"></a>

- `fn task.async[R: sendable](s: Scope, take f: once fn() -> R) -> Task[R] of s`

<a id="def-core_401_2ftask_2fawait"></a>

- `fn task.await[R](s: Scope, take h: Task[R] of s) -> R`

<a id="def-core_401_2ftask_2fawait_5fany"></a>

- `fn task.await_any[R](s: Scope, set: TaskSet[R] of s) -> Option[u64]`

<a id="def-core_401_2ftask_2fawait_5fat"></a>

- `fn task.await_at[R](s: Scope, mut set: TaskSet[R] of s, index: u64) -> R`

<a id="def-core_401_2ftask_2fcancel"></a>

- `@noalloc fn task.cancel[R](s: Scope, h: Task[R] of s) -> ()`

<a id="def-core_401_2ftask_2fcancel_5fall"></a>

- `fn task.cancel_all[R](s: Scope, take set: TaskSet[R] of s) -> ()`

<a id="def-core_401_2ftask_2fcheck_5fcanceled"></a>

- `@noalloc fn task.check_canceled(s: Scope) -> Result[(), CancelError]`

<a id="def-core_401_2ftask_2fdetach"></a>

- `fn task.detach[R](s: Scope, take h: Task[R] of s) -> ()`

<a id="def-core_401_2ftask_2fdetach_5fall"></a>

- `fn task.detach_all[R](s: Scope, take set: TaskSet[R] of s) -> ()`

<a id="def-core_401_2ftask_2fisolate"></a>

- `fn task.isolate[R: sendable](tasks: Tasks, take body: once fn() -> R) -> Result[R, Failure]`

<a id="def-core_401_2ftask_2fscope"></a>

- `fn task.scope[R](tasks: Tasks, body: once fn(s: Scope) -> R) -> R`

<a id="def-core_401_2ftask_2fscope_5funtil"></a>

- `fn task.scope_until[R](tasks: Tasks, deadline: Deadline, body: once fn(s: Scope) -> R) -> Bounded[R]`

<a id="def-std_401_2ftask_2fselect"></a>

- `pub fn task.select[R](s: Scope, take set: TaskSet[R] of s) -> (u64, R)`

References: [`core@1/Scope`](#def-core_401_2fScope), [`core@1/TaskSet`](#def-core_401_2fTaskSet), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-core_401_2ftask_2fset"></a>

- `fn task.set[R](s: Scope) -> TaskSet[R] of s`


## test

<a id="def-std_401_2ftest_2fapprox"></a>

- `@noalloc @track_caller pub fn test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest), [`core@1/f64`](#def-core_401_2ff64)

<a id="def-core_401_2ftest_2fcheck"></a>

- `@noalloc fn test.check(t: Test, condition: bool) -> ()`

<a id="def-core_401_2ftest_2fcheck_5fcase"></a>

- `@noalloc fn test.check_case(t: Test, ok: bool, case: u64) -> ()`

<a id="def-core_401_2ftest_2fdir"></a>

- `fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir`

<a id="def-std_401_2ftest_2fequal"></a>

- `@noalloc @track_caller pub fn test.equal[T: data](t: Test, actual: T, expected: T) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest)

<a id="def-std_401_2ftest_2fexpect_5ferror"></a>

- `@track_caller pub fn test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/Test`](#def-core_401_2fTest)

<a id="def-std_401_2ftest_2fexpect_5fok"></a>

- `@track_caller pub fn test.expect_ok[T, E](t: Test, take value: Result[T, E]) -> T`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/Test`](#def-core_401_2fTest)

<a id="def-core_401_2ftest_2ffail"></a>

- `@noalloc fn test.fail(t: Test) -> Never`

<a id="def-std_401_2ftest_2ffor_5fall"></a>

- `@track_caller pub fn test.for_all[T](t: Test, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2ftest_2ffor_5fall_5fcases"></a>

- `@track_caller pub fn test.for_all_cases[T](t: Test, cases: u64, gen: fn(Test) -> T, law: fn(T) -> bool uses {}) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-core_401_2ftest_2fnet"></a>

- `fn test.net(t: Test, peers: List[NetPeer], listeners: List[NetListener]) -> Net`

<a id="def-core_401_2ftest_2fprocess"></a>

- `fn test.process(t: Test, runs: List[ProcessScript], children: List[ChildScript]) -> Proc`


## text

<a id="def-std_401_2ftext_2fappend"></a>

- `@preserves_unique(builder) pub fn text.append(mut builder: List[u8], text: String) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2fdisplay_5fwidth"></a>

- `@noalloc pub fn text.display_width(s: String) -> u64 uses {}`

Estimated display cells over Unicode16 extended grapheme clusters.
Each cluster's first scalar counts 2 for EAW W/F or a P1868R2 wide range,
otherwise 1, including isolated combining marks and controls. Empty is 0.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftext_2ffinish"></a>

- `@preserves_unique(builder) pub fn text.finish(take builder: List[u8]) -> Result[String, Bytes] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2ffinish_5fvalid"></a>

- `@preserves_unique(builder) @track_caller pub fn text.finish_valid(take builder: List[u8]) -> String uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn text.format_into_spec( value: String, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2fpad_5ffrom"></a>

- `@preserves_unique(out) pub fn text.pad_from(start: u64, fill: char, align: u64, width: u64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2fpad_5fnumber_5ffrom"></a>

- `@preserves_unique(out) pub fn text.pad_number_from( start: u64, fill: char, align: u64, width: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)


## time

<a id="def-std_401_2ftime_2fDuration"></a>

- `@serializable pub type time.Duration { nanoseconds: i128, }`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2ftime_2fSleepError"></a>

- `pub type time.SleepError = NegativeDuration | OutOfRange | Wait(WaitError)`

References: [`core@1/WaitError`](#def-core_401_2fWaitError)

<a id="def-std_401_2ftime_2fSleepError_23NegativeDuration"></a>

- `NegativeDuration -> time.SleepError`

Variant of time.SleepError.

<a id="def-std_401_2ftime_2fSleepError_23OutOfRange"></a>

- `OutOfRange -> time.SleepError`

Variant of time.SleepError.

<a id="def-std_401_2ftime_2fSleepError_23Wait"></a>

- `Wait(WaitError) -> time.SleepError`

Variant of time.SleepError.

References: [`core@1/WaitError`](#def-core_401_2fWaitError)

<a id="def-std_401_2ftime_2fTimeError"></a>

- `pub type time.TimeError = OutOfRange()`

<a id="def-std_401_2ftime_2fTimeError_23OutOfRange"></a>

- `OutOfRange() -> time.TimeError`

Variant of time.TimeError.

<a id="def-std_401_2ftime_2fTimestamp"></a>

- `@serializable pub type time.Timestamp { unix_nanoseconds: i128, }`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2ftime_2fbetween"></a>

- `@noalloc pub fn time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> Duration`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2ftime_2fcompare"></a>

- `@noalloc pub fn time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ftime_2fdeadline"></a>

- `@noalloc pub fn time.deadline(clock: MonotonicClock, interval: Duration) -> Result[ Instant of clock, SleepError ]`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)

<a id="def-std_401_2ftime_2felapsed"></a>

- `@noalloc pub fn time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[ Duration, ClockError ]`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2ftime_2fformat"></a>

- `pub fn time.format(value: Timestamp) -> Result[unique String, TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_2fparse"></a>

- `@noalloc pub fn time.parse(text: String) -> Result[Timestamp, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_2fsleep"></a>

- `@noalloc pub fn time.sleep(clock: MonotonicClock, interval: Duration) -> Result[(), SleepError]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)


## tls

<a id="def-core_401_2ftls_2fclose"></a>

- `@noalloc fn tls.close(n: Net, take s: Stream of n) -> Result[(), NetError]`

<a id="def-core_401_2ftls_2fconnect"></a>

- `fn tls.connect(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots) -> Result[Stream of n, NetError]`

on a `ConnectTls` stream, `server_name` equal to the rule's address


## type_shape

<a id="def-core_401_2ftype_5fshape_2finfo"></a>

- `fn type_shape.info[T](s: TypeShape[T]) -> ShapeInfo`


## u128

<a id="def-std_401_2fu128_2fchecked_5fadd"></a>

- `@noalloc pub fn u128.checked_add(a: u128, b: u128) -> Option[u128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fchecked_5fmul"></a>

- `@noalloc pub fn u128.checked_mul(a: u128, b: u128) -> Option[u128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fchecked_5fsub"></a>

- `@noalloc pub fn u128.checked_sub(a: u128, b: u128) -> Option[u128] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fformat"></a>

- `@noalloc pub fn u128.format[n: size](value: u128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u128`](#def-core_401_2fu128), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fu128_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn u128.format_into(value: u128, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u128`](#def-core_401_2fu128), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu128_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn u128.format_into_spec( value: u128, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u128`](#def-core_401_2fu128), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu128_2fformat_5fradix"></a>

- `pub fn u128.format_radix(value: u128, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u128`](#def-core_401_2fu128), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fu128_2forder"></a>

- `const u128.order: Order[u128]`

<a id="def-std_401_2fu128_2fparse"></a>

- `@noalloc pub fn u128.parse(text: String) -> Result[u128, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u128`](#def-core_401_2fu128), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu128_2fparse_5fradix"></a>

- `@noalloc pub fn u128.parse_radix(text: String, radix: u8) -> Result[u128, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u128`](#def-core_401_2fu128), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu128_2fsaturating_5fadd"></a>

- `@noalloc pub fn u128.saturating_add(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fsaturating_5fmul"></a>

- `@noalloc pub fn u128.saturating_mul(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fsaturating_5fsub"></a>

- `@noalloc pub fn u128.saturating_sub(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fto_5fstring"></a>

- `pub fn u128.to_string(value: u128) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fwrapping_5fadd"></a>

- `@noalloc pub fn u128.wrapping_add(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fwrapping_5fmul"></a>

- `@noalloc pub fn u128.wrapping_mul(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)

<a id="def-std_401_2fu128_2fwrapping_5fsub"></a>

- `@noalloc pub fn u128.wrapping_sub(a: u128, b: u128) -> u128 uses {}`

References: [`core@1/u128`](#def-core_401_2fu128)


## u16

<a id="def-std_401_2fu16_2fchecked_5fadd"></a>

- `@noalloc pub fn u16.checked_add(a: u16, b: u16) -> Option[u16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fchecked_5fmul"></a>

- `@noalloc pub fn u16.checked_mul(a: u16, b: u16) -> Option[u16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fchecked_5fsub"></a>

- `@noalloc pub fn u16.checked_sub(a: u16, b: u16) -> Option[u16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fformat"></a>

- `@noalloc pub fn u16.format[n: size](value: u16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fu16_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn u16.format_into(value: u16, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu16_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn u16.format_into_spec( value: u16, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu16_2fformat_5fradix"></a>

- `pub fn u16.format_radix(value: u16, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fu16_2forder"></a>

- `const u16.order: Order[u16]`

<a id="def-std_401_2fu16_2fparse"></a>

- `@noalloc pub fn u16.parse(text: String) -> Result[u16, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu16_2fparse_5fradix"></a>

- `@noalloc pub fn u16.parse_radix(text: String, radix: u8) -> Result[u16, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu16_2fsaturating_5fadd"></a>

- `@noalloc pub fn u16.saturating_add(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fsaturating_5fmul"></a>

- `@noalloc pub fn u16.saturating_mul(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fsaturating_5fsub"></a>

- `@noalloc pub fn u16.saturating_sub(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fto_5fstring"></a>

- `pub fn u16.to_string(value: u16) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fwrapping_5fadd"></a>

- `@noalloc pub fn u16.wrapping_add(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fwrapping_5fmul"></a>

- `@noalloc pub fn u16.wrapping_mul(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fu16_2fwrapping_5fsub"></a>

- `@noalloc pub fn u16.wrapping_sub(a: u16, b: u16) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16)


## u32

<a id="def-std_401_2fu32_2fchecked_5fadd"></a>

- `@noalloc pub fn u32.checked_add(a: u32, b: u32) -> Option[u32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fchecked_5fmul"></a>

- `@noalloc pub fn u32.checked_mul(a: u32, b: u32) -> Option[u32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fchecked_5fsub"></a>

- `@noalloc pub fn u32.checked_sub(a: u32, b: u32) -> Option[u32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fformat"></a>

- `@noalloc pub fn u32.format[n: size](value: u32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fu32_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn u32.format_into(value: u32, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu32_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn u32.format_into_spec( value: u32, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu32_2fformat_5fradix"></a>

- `pub fn u32.format_radix(value: u32, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fu32_2forder"></a>

- `const u32.order: Order[u32]`

<a id="def-std_401_2fu32_2fparse"></a>

- `@noalloc pub fn u32.parse(text: String) -> Result[u32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u32`](#def-core_401_2fu32), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu32_2fparse_5fradix"></a>

- `@noalloc pub fn u32.parse_radix(text: String, radix: u8) -> Result[u32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu32_2fsaturating_5fadd"></a>

- `@noalloc pub fn u32.saturating_add(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fsaturating_5fmul"></a>

- `@noalloc pub fn u32.saturating_mul(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fsaturating_5fsub"></a>

- `@noalloc pub fn u32.saturating_sub(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fto_5fstring"></a>

- `pub fn u32.to_string(value: u32) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fwrapping_5fadd"></a>

- `@noalloc pub fn u32.wrapping_add(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fwrapping_5fmul"></a>

- `@noalloc pub fn u32.wrapping_mul(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fu32_2fwrapping_5fsub"></a>

- `@noalloc pub fn u32.wrapping_sub(a: u32, b: u32) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32)


## u64

<a id="def-std_401_2fu64_2fchecked_5fadd"></a>

- `@noalloc pub fn u64.checked_add(a: u64, b: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fchecked_5fmul"></a>

- `@noalloc pub fn u64.checked_mul(a: u64, b: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fchecked_5fsub"></a>

- `@noalloc pub fn u64.checked_sub(a: u64, b: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fformat"></a>

- `@noalloc pub fn u64.format[n: size](value: u64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fu64_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn u64.format_into(value: u64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu64_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn u64.format_into_spec( value: u64, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu64_2fformat_5fradix"></a>

- `pub fn u64.format_radix(value: u64, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fu64_2forder"></a>

- `const u64.order: Order[u64]`

<a id="def-std_401_2fu64_2fparse"></a>

- `@noalloc pub fn u64.parse(text: String) -> Result[u64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu64_2fparse_5fradix"></a>

- `@noalloc pub fn u64.parse_radix(text: String, radix: u8) -> Result[u64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu64_2fsaturating_5fadd"></a>

- `@noalloc pub fn u64.saturating_add(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fsaturating_5fmul"></a>

- `@noalloc pub fn u64.saturating_mul(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fsaturating_5fsub"></a>

- `@noalloc pub fn u64.saturating_sub(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fto_5fstring"></a>

- `pub fn u64.to_string(value: u64) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fwrapping_5fadd"></a>

- `@noalloc pub fn u64.wrapping_add(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fwrapping_5fmul"></a>

- `@noalloc pub fn u64.wrapping_mul(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fu64_2fwrapping_5fsub"></a>

- `@noalloc pub fn u64.wrapping_sub(a: u64, b: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)


## u8

<a id="def-std_401_2fu8_2fchecked_5fadd"></a>

- `@noalloc pub fn u8.checked_add(a: u8, b: u8) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fchecked_5fmul"></a>

- `@noalloc pub fn u8.checked_mul(a: u8, b: u8) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fchecked_5fsub"></a>

- `@noalloc pub fn u8.checked_sub(a: u8, b: u8) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fformat"></a>

- `@noalloc pub fn u8.format[n: size](value: u8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2fu8_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn u8.format_into(value: u8, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn u8.format_into_spec( value: u8, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fformat_5fradix"></a>

- `pub fn u8.format_radix(value: u8, radix: u8) -> Result[unique String, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-core_401_2fu8_2forder"></a>

- `const u8.order: Order[u8]`

<a id="def-std_401_2fu8_2fparse"></a>

- `@noalloc pub fn u8.parse(text: String) -> Result[u8, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu8_2fparse_5fradix"></a>

- `@noalloc pub fn u8.parse_radix(text: String, radix: u8) -> Result[u8, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fu8_2fsaturating_5fadd"></a>

- `@noalloc pub fn u8.saturating_add(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fsaturating_5fmul"></a>

- `@noalloc pub fn u8.saturating_mul(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fsaturating_5fsub"></a>

- `@noalloc pub fn u8.saturating_sub(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fto_5fstring"></a>

- `pub fn u8.to_string(value: u8) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fwrapping_5fadd"></a>

- `@noalloc pub fn u8.wrapping_add(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fwrapping_5fmul"></a>

- `@noalloc pub fn u8.wrapping_mul(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fu8_2fwrapping_5fsub"></a>

- `@noalloc pub fn u8.wrapping_sub(a: u8, b: u8) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8)


## udp

<a id="def-core_401_2fudp_2fclose"></a>

- `@noalloc fn udp.close(n: Net, take s: UdpSocket of n) -> ()`

the socket's originating `Net`

<a id="def-core_401_2fudp_2frecv_5ffrom"></a>

- `fn udp.recv_from(n: Net, mut s: UdpSocket of n, mut buffer: List[u8], max: u64) -> Result[UdpReceived, NetError]`

the socket's originating `Net`; only peers with an exact `UdpPeer()` rule deliver input

<a id="def-core_401_2fudp_2fsend_5fto"></a>

- `@noalloc fn udp.send_to(n: Net, mut s: UdpSocket of n, data: Bytes, addr: SocketAddr) -> Result[u64, NetError]`

the socket's originating `Net`, and `(literal, port, UdpPeer())` for `addr`


## unicode

<a id="def-std_401_2funicode_2fGraphemeCursor"></a>

- `pub opaque type unicode.GraphemeCursor: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2funicode_2fGraphemeView"></a>

- `@loop_source(graphemes_open, graphemes_next) pub opaque type unicode.GraphemeView: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2funicode_2fgraphemes"></a>

- `@noalloc pub fn unicode.graphemes(s: String) -> GraphemeView in s`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/unicode/GraphemeView`](#def-std_401_2funicode_2fGraphemeView)

<a id="def-std_401_2funicode_2fis_5fdigit"></a>

- `@noalloc pub fn unicode.is_digit(c: char) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/char`](#def-core_401_2fchar)

<a id="def-std_401_2funicode_2fis_5fletter"></a>

- `@noalloc pub fn unicode.is_letter(c: char) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/char`](#def-core_401_2fchar)

<a id="def-std_401_2funicode_2fis_5fwhitespace"></a>

- `@noalloc pub fn unicode.is_whitespace(c: char) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/char`](#def-core_401_2fchar)

<a id="def-std_401_2funicode_2fnormalize_5fnfc"></a>

- `pub fn unicode.normalize_nfc(s: String) -> unique String uses {}`

Unicode 16.0.0 canonical composition; returns independently owned text.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2funicode_2fnormalize_5fnfd"></a>

- `pub fn unicode.normalize_nfd(s: String) -> unique String uses {}`

Unicode 16.0.0 canonical decomposition; returns independently owned text.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2funicode_2fnormalize_5fnfkc"></a>

- `pub fn unicode.normalize_nfkc(s: String) -> unique String uses {}`

Unicode 16.0.0 compatibility decomposition followed by canonical composition; returns independently owned text.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2funicode_2fnormalize_5fnfkd"></a>

- `pub fn unicode.normalize_nfkd(s: String) -> unique String uses {}`

Unicode 16.0.0 compatibility decomposition; returns independently owned text.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2funicode_2fsimple_5flower"></a>

- `@noalloc pub fn unicode.simple_lower(c: char) -> char uses {}`

References: [`core@1/char`](#def-core_401_2fchar)

<a id="def-std_401_2funicode_2fsimple_5fupper"></a>

- `@noalloc pub fn unicode.simple_upper(c: char) -> char uses {}`

References: [`core@1/char`](#def-core_401_2fchar)


## uri

<a id="def-std_401_2furi_2fComponent"></a>

- `pub type uri.Component = Raw(String) | Encoded(String)`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fComponent_23Encoded"></a>

- `Encoded(String) -> uri.Component`

Variant of uri.Component.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fComponent_23Raw"></a>

- `Raw(String) -> uri.Component`

Variant of uri.Component.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fPart"></a>

- `pub type uri.Part = Unreserved | User | Password | Host | Path | Query | Fragment`

<a id="def-std_401_2furi_2fPart_23Fragment"></a>

- `Fragment -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23Host"></a>

- `Host -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23Password"></a>

- `Password -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23Path"></a>

- `Path -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23Query"></a>

- `Query -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23Unreserved"></a>

- `Unreserved -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fPart_23User"></a>

- `User -> uri.Part`

Variant of uri.Part.

<a id="def-std_401_2furi_2fQueryError"></a>

- `pub type uri.QueryError = InvalidEscape(u64) | InvalidSeparator(u64) | InvalidUtf8(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2furi_2fQueryError_23InvalidEscape"></a>

- `InvalidEscape(u64) -> uri.QueryError`

Variant of uri.QueryError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2furi_2fQueryError_23InvalidSeparator"></a>

- `InvalidSeparator(u64) -> uri.QueryError`

Variant of uri.QueryError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2furi_2fQueryError_23InvalidUtf8"></a>

- `InvalidUtf8(u64) -> uri.QueryError`

Variant of uri.QueryError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2furi_2fUri"></a>

- `pub type uri.Uri { scheme: String, user: Option[Component], password: Option[Component], host: Option[Component], port: Option[u16], path: Component, query: Option[Component], fragment: Option[Component], }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/uri/Component`](#def-std_401_2furi_2fComponent)

<a id="def-std_401_2furi_2fUriError"></a>

- `pub type uri.UriError = UnexpectedCharacter | InvalidFormat | InvalidPort | InvalidHostName`

<a id="def-std_401_2furi_2fUriError_23InvalidFormat"></a>

- `InvalidFormat -> uri.UriError`

Variant of uri.UriError.

<a id="def-std_401_2furi_2fUriError_23InvalidHostName"></a>

- `InvalidHostName -> uri.UriError`

Variant of uri.UriError.

<a id="def-std_401_2furi_2fUriError_23InvalidPort"></a>

- `InvalidPort -> uri.UriError`

Variant of uri.UriError.

<a id="def-std_401_2furi_2fUriError_23UnexpectedCharacter"></a>

- `UnexpectedCharacter -> uri.UriError`

Variant of uri.UriError.

<a id="def-std_401_2furi_2fcomponent_5fencoded"></a>

- `pub fn uri.component_encoded(component: Component, part: Part) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Component`](#def-std_401_2furi_2fComponent), [`std@1/uri/Part`](#def-std_401_2furi_2fPart)

<a id="def-std_401_2furi_2fcomponent_5fraw"></a>

- `pub fn uri.component_raw(component: Component) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/uri/Component`](#def-std_401_2furi_2fComponent)

<a id="def-std_401_2furi_2fcomponent_5ftext"></a>

- `pub fn uri.component_text(component: Component) -> Result[unique String, unique Bytes] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Component`](#def-std_401_2furi_2fComponent)

<a id="def-std_401_2furi_2fencode_5fquery"></a>

- `pub fn uri.encode_query(pairs: List[(String, String)]) -> unique String uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fformat"></a>

- `pub fn uri.format(parsed_uri: Uri) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri)

<a id="def-std_401_2furi_2fparse"></a>

- `pub fn uri.parse(text: String) -> Result[Uri, UriError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri), [`std@1/uri/UriError`](#def-std_401_2furi_2fUriError)

<a id="def-std_401_2furi_2fpercent_5fdecode"></a>

- `pub fn uri.percent_decode(text: String) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fpercent_5fencode"></a>

- `pub fn uri.percent_encode(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fquery_5fpairs"></a>

- `pub fn uri.query_pairs(query: String) -> Result[ unique List[(unique String, unique String)], QueryError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/uri/QueryError`](#def-std_401_2furi_2fQueryError)

<a id="def-std_401_2furi_2fremove_5fdot_5fsegments"></a>

- `pub fn uri.remove_dot_segments(path: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2furi_2fresolve"></a>

- `pub fn uri.resolve(base: Uri, reference: String) -> Result[Uri, UriError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri), [`std@1/uri/UriError`](#def-std_401_2furi_2fUriError)


## utf8

<a id="def-std_401_2futf8_2fdecode_5flossy"></a>

- `pub fn utf8.decode_lossy(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)
