# Public API

Generated from manifest-selected public declarations. Do not edit by hand.
Signatures retain modes, brands, attributes, relations and `uses` clauses.
Opaque representations stay hidden; bodies are not checked or evaluated.

2288 exported definitions; 999 variants.

## Types and roots

<a id="def-core_401_2fArena"></a>

- `@capability(local) opaque type Arena: noalloc_drop uses {Arena}`

<a id="def-core_401_2fArgError"></a>

- `type ArgError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = MissingArgument() | InvalidUtf8()`

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

<a id="def-core_401_2fBase64Alphabet"></a>

- `type Base64Alphabet: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Standard() | UrlSafe()`

<a id="def-core_401_2fBase64Alphabet_23Standard"></a>

- `Standard() -> Base64Alphabet`

Variant of Base64Alphabet.

<a id="def-core_401_2fBase64Alphabet_23UrlSafe"></a>

- `UrlSafe() -> Base64Alphabet`

Variant of Base64Alphabet.

<a id="def-core_401_2fBase64Padding"></a>

- `type Base64Padding: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Required() | Omitted()`

<a id="def-core_401_2fBase64Padding_23Omitted"></a>

- `Omitted() -> Base64Padding`

Variant of Base64Padding.

<a id="def-core_401_2fBase64Padding_23Required"></a>

- `Required() -> Base64Padding`

Variant of Base64Padding.

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

<a id="def-core_401_2fCallEnd"></a>

- `type CallEnd: serializable uses {} = Returned(Bytes) | Failed(Bytes) | Faulted(Bytes) | NotRun(NotRunReason) | InDoubt(InDoubtReason) | Canceled(CancelCause)`

<a id="def-core_401_2fCallEnd_23Canceled"></a>

- `Canceled(CancelCause) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallEnd_23Failed"></a>

- `Failed(Bytes) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallEnd_23Faulted"></a>

- `Faulted(Bytes) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallEnd_23InDoubt"></a>

- `InDoubt(InDoubtReason) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallEnd_23NotRun"></a>

- `NotRun(NotRunReason) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallEnd_23Returned"></a>

- `Returned(Bytes) -> CallEnd`

Variant of CallEnd.

<a id="def-core_401_2fCallForm"></a>

- `type CallForm: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Compute() | Call()`

<a id="def-core_401_2fCallForm_23Call"></a>

- `Call() -> CallForm`

Variant of CallForm.

<a id="def-core_401_2fCallForm_23Compute"></a>

- `Compute() -> CallForm`

Variant of CallForm.

<a id="def-core_401_2fCallRef"></a>

- `type CallRef: serializable uses {} = { epoch: u64, position: u64, key: Bytes }`

<a id="def-core_401_2fCancelCause"></a>

- `type CancelCause: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Requested() | Parent() | ChildFailed() | DeadlineExceeded()`

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

- `type CancelError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Canceled(CancelCause)`

<a id="def-core_401_2fCancelError_23Canceled"></a>

- `Canceled(CancelCause) -> CancelError`

Variant of CancelError.

<a id="def-core_401_2fChannelError"></a>

- `type ChannelError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Closed() | Canceled(CancelCause)`

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

<a id="def-core_401_2fChildGroup"></a>

- `opaque resource type ChildGroup: sendable + noalloc_drop uses {Proc}`

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

- `type CliError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Usage(ArgError) | Files(FileError) | Output(OutputError)`

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

- `type ClockError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Unavailable() | OutOfRange()`

<a id="def-core_401_2fClockError_23OutOfRange"></a>

- `OutOfRange() -> ClockError`

Variant of ClockError.

<a id="def-core_401_2fClockError_23Unavailable"></a>

- `Unavailable() -> ClockError`

Variant of ClockError.

<a id="def-core_401_2fCommand"></a>

- `type Command uses {Dir} = { program: Path, args: List[String], dir: Option[Dir], env: Option[List[(String, String)]], own_group: bool }`

<a id="def-core_401_2fDeadline"></a>

- `type Deadline: plain + copyable + sendable + noalloc_drop uses {MonotonicClock} = { clock: MonotonicClock, at: Instant of .clock }`

<a id="def-core_401_2fDecodeError"></a>

- `type DecodeError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Version() | SchemaMismatch() | Truncated() | Trailing() | Scalar() | Utf8() | Tag() | Length() | Duplicate() | Order() | Invalid()`

<a id="def-core_401_2fDecodeError_23Duplicate"></a>

- `Duplicate() -> DecodeError`

Variant of DecodeError.

<a id="def-core_401_2fDecodeError_23Invalid"></a>

- `Invalid() -> DecodeError`

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

- `type DirEntry: serializable uses {} = { path: Path, kind: FileKind }`

<a id="def-core_401_2fDirGrant"></a>

- `type DirGrant: serializable uses {} = { path: String, access: GrantAccess }`

<a id="def-core_401_2fDurability"></a>

- `type Durability: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Durable() | Volatile()`

<a id="def-core_401_2fDurability_23Durable"></a>

- `Durable() -> Durability`

Variant of Durability.

<a id="def-core_401_2fDurability_23Volatile"></a>

- `Volatile() -> Durability`

Variant of Durability.

<a id="def-core_401_2fEntry"></a>

- `opaque type Entry[F]: plain + copyable + inert + sendable + noalloc_drop uses {}`

<a id="def-core_401_2fEnv"></a>

- `@capability(global) opaque type Env: plain + copyable + sendable + noalloc_drop uses {Env}`

<a id="def-core_401_2fEnvError"></a>

- `type EnvError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = NotGranted() | InvalidUtf8()`

<a id="def-core_401_2fEnvError_23InvalidUtf8"></a>

- `InvalidUtf8() -> EnvError`

Variant of EnvError.

<a id="def-core_401_2fEnvError_23NotGranted"></a>

- `NotGranted() -> EnvError`

Variant of EnvError.

<a id="def-core_401_2fEq"></a>

- `type Eq[T] uses {} = { equal: fn(T, T) -> bool uses {} }`

<a id="def-core_401_2fExitStatus"></a>

- `type ExitStatus: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Exited(i32) | Signaled(i32)`

<a id="def-core_401_2fExitStatus_23Exited"></a>

- `Exited(i32) -> ExitStatus`

Variant of ExitStatus.

<a id="def-core_401_2fExitStatus_23Signaled"></a>

- `Signaled(i32) -> ExitStatus`

Variant of ExitStatus.

<a id="def-core_401_2fFailure"></a>

- `type Failure: serializable uses {} = { kind: FailureKind, rule: String, site: SourceSite, spawn: List[u64], cause: Option[box Failure] }`

<a id="def-core_401_2fFailureKind"></a>

- `type FailureKind: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Check() | Contract() | Depth() | OutOfSpace() | DeadMutex()`

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

- `type FileError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = FileFault(u32) | Canceled(CancelCause) | NotGranted() | NotFound() | PermissionDenied() | AlreadyExists() | IsDirectory() | NotDirectory() | DirectoryNotEmpty() | CrossDevice() | Truncated()`

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

<a id="def-core_401_2fFileError_23Truncated"></a>

- `Truncated() -> FileError`

Variant of FileError.

<a id="def-core_401_2fFileKind"></a>

- `type FileKind: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Regular() | Directory() | Symlink() | Other()`

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

- `type FloatIntError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = NotFinite() | OutOfRange()`

<a id="def-core_401_2fFloatIntError_23NotFinite"></a>

- `NotFinite() -> FloatIntError`

Variant of FloatIntError.

<a id="def-core_401_2fFloatIntError_23OutOfRange"></a>

- `OutOfRange() -> FloatIntError`

Variant of FloatIntError.

<a id="def-core_401_2fGrantAccess"></a>

- `type GrantAccess: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Read() | ReadWrite()`

<a id="def-core_401_2fGrantAccess_23Read"></a>

- `Read() -> GrantAccess`

Variant of GrantAccess.

<a id="def-core_401_2fGrantAccess_23ReadWrite"></a>

- `ReadWrite() -> GrantAccess`

Variant of GrantAccess.

<a id="def-core_401_2fGrantError"></a>

- `type GrantError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = NotGranted()`

<a id="def-core_401_2fGrantError_23NotGranted"></a>

- `NotGranted() -> GrantError`

Variant of GrantError.

<a id="def-core_401_2fGrantSet"></a>

- `type GrantSet: serializable uses {} = { stdout: bool, stderr: bool, tasks: bool, monotonic_clock: bool, wall_clock: bool, random: bool, dirs: List[DirGrant], net: List[NetRule], process: List[ProcessRule] }`

<a id="def-core_401_2fGroupOutput"></a>

- `type GroupOutput uses {} = { statuses: List[ExitStatus], stderr: List[Bytes] }`

<a id="def-core_401_2fHasher"></a>

- `type Hasher[T] uses {} = { hash: fn(T, u64) -> u64 uses {} }`

<a id="def-core_401_2fInDoubtReason"></a>

- `type InDoubtReason: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Lost() | TimedOut() | Abandoned()`

<a id="def-core_401_2fInDoubtReason_23Abandoned"></a>

- `Abandoned() -> InDoubtReason`

Variant of InDoubtReason.

<a id="def-core_401_2fInDoubtReason_23Lost"></a>

- `Lost() -> InDoubtReason`

Variant of InDoubtReason.

<a id="def-core_401_2fInDoubtReason_23TimedOut"></a>

- `TimedOut() -> InDoubtReason`

Variant of InDoubtReason.

<a id="def-core_401_2fInline"></a>

- `type Inline[T, n: size] uses {T}`

<a id="def-core_401_2fInputError"></a>

- `type InputError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = InputFault(u32) | Canceled(CancelCause) | InvalidInput() | InvalidUtf8()`

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

- `type IpAddr: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = V4(Inline[u8, 4]) | V6(Inline[u8, 16])`

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

<a id="def-core_401_2fMapped"></a>

- `opaque resource type Mapped: noalloc_drop uses {Dir}`

<a id="def-core_401_2fMetadata"></a>

- `type Metadata: serializable uses {} = { kind: FileKind, size: u64, modified_unix_ns: Option[i128], permissions: u32 }`

<a id="def-core_401_2fMonotonicClock"></a>

- `@capability(global) opaque type MonotonicClock: plain + copyable + sendable + noalloc_drop uses {MonotonicClock}`

<a id="def-core_401_2fMutex"></a>

- `type Mutex[T] uses {Tasks, T}`

<a id="def-core_401_2fNet"></a>

- `@capability(global) opaque type Net: plain + copyable + sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fNetClient"></a>

- `type NetClient uses {} = { from: SocketAddr, tls: PeerTls, steps: List[NetStep] }`

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

- `type NetError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = NotGranted() | Canceled(CancelCause) | InvalidInput() | UnknownHost() | ResolverFailure() | ConnectionRefused() | ConnectionReset() | ConnectionAborted() | NotConnected() | AddrInUse() | AddrNotAvailable() | HostUnreachable() | NetworkUnreachable() | NetworkDown() | BrokenPipe() | TimedOut() | Tls(TlsFailure) | Fault(u32)`

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

- `type NetListener uses {} = { address: String, port: u16, clients: List[NetClient] }`

<a id="def-core_401_2fNetOperation"></a>

- `type NetOperation: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Connect() | ConnectTls() | Listen() | UdpBind() | UdpPeer() | ListenTls() | UnixConnect() | UnixListen() | UnixListenRemoveStale()`

<a id="def-core_401_2fNetOperation_23Connect"></a>

- `Connect() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23ConnectTls"></a>

- `ConnectTls() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23Listen"></a>

- `Listen() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23ListenTls"></a>

- `ListenTls() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UdpBind"></a>

- `UdpBind() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UdpPeer"></a>

- `UdpPeer() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UnixConnect"></a>

- `UnixConnect() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UnixListen"></a>

- `UnixListen() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetOperation_23UnixListenRemoveStale"></a>

- `UnixListenRemoveStale() -> NetOperation`

Variant of NetOperation.

<a id="def-core_401_2fNetPeer"></a>

- `type NetPeer uses {} = { address: String, port: u16, resolves_to: List[IpAddr], tls: PeerTls, connections: List[NetConnection] }`

<a id="def-core_401_2fNetRule"></a>

- `type NetRule: serializable uses {} = { address: String, port: u16, operation: NetOperation }`

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

<a id="def-core_401_2fNotRunReason"></a>

- `type NotRunReason: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Denied() | VersionMismatch() | Unreachable() | Overloaded() | Rejected()`

<a id="def-core_401_2fNotRunReason_23Denied"></a>

- `Denied() -> NotRunReason`

Variant of NotRunReason.

<a id="def-core_401_2fNotRunReason_23Overloaded"></a>

- `Overloaded() -> NotRunReason`

Variant of NotRunReason.

<a id="def-core_401_2fNotRunReason_23Rejected"></a>

- `Rejected() -> NotRunReason`

Variant of NotRunReason.

<a id="def-core_401_2fNotRunReason_23Unreachable"></a>

- `Unreachable() -> NotRunReason`

Variant of NotRunReason.

<a id="def-core_401_2fNotRunReason_23VersionMismatch"></a>

- `VersionMismatch() -> NotRunReason`

Variant of NotRunReason.

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

- `type OutOfSpace: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = OutOfSpace()`

<a id="def-core_401_2fOutOfSpace_23OutOfSpace"></a>

- `OutOfSpace() -> OutOfSpace`

Variant of OutOfSpace.

<a id="def-core_401_2fOutputError"></a>

- `type OutputError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = OutputFault(u32) | Canceled(CancelCause)`

<a id="def-core_401_2fOutputError_23Canceled"></a>

- `Canceled(CancelCause) -> OutputError`

Variant of OutputError.

<a id="def-core_401_2fOutputError_23OutputFault"></a>

- `OutputFault(u32) -> OutputError`

Variant of OutputError.

<a id="def-core_401_2fPacked"></a>

- `type Packed[T] uses {T}`

<a id="def-core_401_2fPath"></a>

- `opaque type Path: copyable + inert + noalloc_drop + plain + data + key + sendable + serializable uses {}`

<a id="def-core_401_2fPayloadShape"></a>

- `type PayloadShape: copyable + data + key + inert + noalloc_drop uses {} = { label: Option[String], ty: String }`

<a id="def-core_401_2fPeerTls"></a>

- `type PeerTls uses {} = Plain() | Certificate(List[String]) | Fails(TlsFailure) | Presents(List[String], Bytes) | Serves(TlsServer) | Offers(List[String], List[String])`

<a id="def-core_401_2fPeerTls_23Certificate"></a>

- `Certificate(List[String]) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Fails"></a>

- `Fails(TlsFailure) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Offers"></a>

- `Offers(List[String], List[String]) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Plain"></a>

- `Plain() -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Presents"></a>

- `Presents(List[String], Bytes) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPeerTls_23Serves"></a>

- `Serves(TlsServer) -> PeerTls`

Variant of PeerTls.

<a id="def-core_401_2fPipelineOutput"></a>

- `type PipelineOutput: serializable uses {} = { statuses: List[ExitStatus], stdout: Bytes, stderr: Bytes }`

<a id="def-core_401_2fPositionView"></a>

- `opaque type PositionView: copyable + inert + noalloc_drop uses {}`

<a id="def-core_401_2fProc"></a>

- `@capability(global) opaque type Proc: plain + copyable + sendable + noalloc_drop uses {Proc}`

<a id="def-core_401_2fProcessError"></a>

- `type ProcessError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = NotGranted() | InvalidInput() | NotFound() | PermissionDenied() | OutputLimit() | Fault(u32) | TimedOut() | Canceled(CancelCause) | InputClosed()`

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

- `type ProcessOutput: serializable uses {} = { status: ExitStatus, stdout: Bytes, stderr: Bytes }`

<a id="def-core_401_2fProcessRule"></a>

- `type ProcessRule: serializable uses {} = { program: String, env: List[String] }`

<a id="def-core_401_2fProcessScript"></a>

- `type ProcessScript uses {} = { program: String, args: List[String], outcome: Result[ProcessOutput, ProcessError] }`

<a id="def-core_401_2fPtr"></a>

- `type Ptr[T] uses {}`

<a id="def-core_401_2fRandom"></a>

- `@capability(global) opaque type Random: plain + copyable + sendable + noalloc_drop uses {Random}`

<a id="def-core_401_2fRandomError"></a>

- `type RandomError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Unavailable()`

<a id="def-core_401_2fRandomError_23Unavailable"></a>

- `Unavailable() -> RandomError`

Variant of RandomError.

<a id="def-core_401_2fReadHalf"></a>

- `opaque resource type ReadHalf: sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fReceiver"></a>

- `type Receiver[T] uses {Tasks, T}`

<a id="def-core_401_2fRegion"></a>

- `opaque resource type Region[T] uses {T}`

<a id="def-core_401_2fRemoteCall"></a>

- `type RemoteCall: serializable uses {} = { epoch: u64, position: u64, ordinal: u64, key: Bytes, form: CallForm, path: String, fingerprint: Bytes, code: Bytes, grants: GrantSet, definitions: Option[List[String]], argument: Bytes, acknowledged: u64 }`

<a id="def-core_401_2fRemoteOutcome"></a>

- `type RemoteOutcome: serializable uses {} = { epoch: u64, position: u64, key: Bytes, digest: Bytes, end: CallEnd, callee: Option[Bytes], missing: List[Bytes] }`

<a id="def-core_401_2fResult"></a>

- `type Result[T, E] uses {T, E} = Ok(T) | Error(E)`

<a id="def-core_401_2fResult_23Error"></a>

- `Error(E) -> Result[T, E]`

Variant of Result.

<a id="def-core_401_2fResult_23Ok"></a>

- `Ok(T) -> Result[T, E]`

Variant of Result.

<a id="def-core_401_2fRunError"></a>

- `type RunError[E] uses {E} = NotRun(NotRunReason) | InDoubt(InDoubtReason) | Failed(E) | Faulted(Failure) | Canceled(CancelCause)`

<a id="def-core_401_2fRunError_23Canceled"></a>

- `Canceled(CancelCause) -> RunError[E]`

Variant of RunError.

<a id="def-core_401_2fRunError_23Failed"></a>

- `Failed(E) -> RunError[E]`

Variant of RunError.

<a id="def-core_401_2fRunError_23Faulted"></a>

- `Faulted(Failure) -> RunError[E]`

Variant of RunError.

<a id="def-core_401_2fRunError_23InDoubt"></a>

- `InDoubt(InDoubtReason) -> RunError[E]`

Variant of RunError.

<a id="def-core_401_2fRunError_23NotRun"></a>

- `NotRun(NotRunReason) -> RunError[E]`

Variant of RunError.

<a id="def-core_401_2fRunFrame"></a>

- `type RunFrame: serializable uses {} = Open(SessionOpen) | Opened(SessionOpened) | Refused(NotRunReason) | Call(RemoteCall) | Outcome(RemoteOutcome) | Cancel(CallRef, CancelCause) | Acknowledged(CallRef) | Status(CallRef) | Running(CallRef) | Unknown(CallRef) | Close(u64, u64)`

<a id="def-core_401_2fRunFrame_23Acknowledged"></a>

- `Acknowledged(CallRef) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Call"></a>

- `Call(RemoteCall) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Cancel"></a>

- `Cancel(CallRef, CancelCause) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Close"></a>

- `Close(u64, u64) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Open"></a>

- `Open(SessionOpen) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Opened"></a>

- `Opened(SessionOpened) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Outcome"></a>

- `Outcome(RemoteOutcome) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Refused"></a>

- `Refused(NotRunReason) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Running"></a>

- `Running(CallRef) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Status"></a>

- `Status(CallRef) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunFrame_23Unknown"></a>

- `Unknown(CallRef) -> RunFrame`

Variant of RunFrame.

<a id="def-core_401_2fRunner"></a>

- `@capability(global) opaque type Runner: plain + copyable + sendable + noalloc_drop uses {Runner}`

<a id="def-core_401_2fScalarView"></a>

- `opaque type ScalarView: copyable + inert + noalloc_drop uses {}`

<a id="def-core_401_2fScope"></a>

- `@capability(local) opaque type Scope: noalloc_drop uses {Scope}`

<a id="def-core_401_2fSender"></a>

- `type Sender[T] uses {Tasks, T}`

<a id="def-core_401_2fSessionOpen"></a>

- `type SessionOpen: serializable uses {} = { stream: Bytes, instance: Bytes, epoch: u64 }`

<a id="def-core_401_2fSessionOpened"></a>

- `type SessionOpened: serializable uses {} = { epoch: u64, incarnation: Bytes, applied: u64, reconciles: bool }`

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

<a id="def-core_401_2fSignal"></a>

- `type Signal: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Interrupt() | Terminate() | Kill()`

<a id="def-core_401_2fSignal_23Interrupt"></a>

- `Interrupt() -> Signal`

Variant of Signal.

<a id="def-core_401_2fSignal_23Kill"></a>

- `Kill() -> Signal`

Variant of Signal.

<a id="def-core_401_2fSignal_23Terminate"></a>

- `Terminate() -> Signal`

Variant of Signal.

<a id="def-core_401_2fSignalError"></a>

- `type SignalError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Canceled(CancelCause) | NotGranted() | InvalidInput()`

<a id="def-core_401_2fSignalError_23Canceled"></a>

- `Canceled(CancelCause) -> SignalError`

Variant of SignalError.

<a id="def-core_401_2fSignalError_23InvalidInput"></a>

- `InvalidInput() -> SignalError`

Variant of SignalError.

<a id="def-core_401_2fSignalError_23NotGranted"></a>

- `NotGranted() -> SignalError`

Variant of SignalError.

<a id="def-core_401_2fSignalKind"></a>

- `type SignalKind: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Interrupt() | Terminate() | Hangup()`

<a id="def-core_401_2fSignalKind_23Hangup"></a>

- `Hangup() -> SignalKind`

Variant of SignalKind.

<a id="def-core_401_2fSignalKind_23Interrupt"></a>

- `Interrupt() -> SignalKind`

Variant of SignalKind.

<a id="def-core_401_2fSignalKind_23Terminate"></a>

- `Terminate() -> SignalKind`

Variant of SignalKind.

<a id="def-core_401_2fSignals"></a>

- `@capability(global) opaque type Signals: plain + copyable + sendable + noalloc_drop uses {Signals}`

<a id="def-core_401_2fSimd"></a>

- `type Simd[A, n: size] uses {A}`

<a id="def-core_401_2fSlotId"></a>

- `type SlotId[T] uses {}`

<a id="def-core_401_2fSlots"></a>

- `type Slots[T] uses {T}`

<a id="def-core_401_2fSocketAddr"></a>

- `type SocketAddr: serializable uses {} = { ip: IpAddr, port: u16 }`

<a id="def-core_401_2fSortedMap"></a>

- `type SortedMap[K, V] uses {K, V}`

<a id="def-core_401_2fSourceSite"></a>

- `type SourceSite: serializable uses {} = { definition: String, line: u64, column: u64 }`

<a id="def-core_401_2fSpan"></a>

- `opaque resource type Span: noalloc_drop uses {Telemetry}`

<a id="def-core_401_2fStderr"></a>

- `@capability(global) opaque type Stderr: plain + copyable + sendable + noalloc_drop uses {Stderr}`

<a id="def-core_401_2fStdin"></a>

- `@capability(global) opaque type Stdin: plain + copyable + sendable + noalloc_drop uses {Stdin}`

<a id="def-core_401_2fStdout"></a>

- `@capability(global) opaque type Stdout: plain + copyable + sendable + noalloc_drop uses {Stdout}`

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

<a id="def-core_401_2fTelemetry"></a>

- `@capability(global) opaque type Telemetry: plain + copyable + sendable + noalloc_drop uses {Telemetry}`

<a id="def-core_401_2fTelemetryError"></a>

- `type TelemetryError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = TooLarge() | InvalidParent()`

<a id="def-core_401_2fTelemetryError_23InvalidParent"></a>

- `InvalidParent() -> TelemetryError`

Variant of TelemetryError.

<a id="def-core_401_2fTelemetryError_23TooLarge"></a>

- `TooLarge() -> TelemetryError`

Variant of TelemetryError.

<a id="def-core_401_2fTest"></a>

- `@capability(local) opaque type Test: noalloc_drop uses {Test}`

<a id="def-core_401_2fTlsClient"></a>

- `type TlsClient uses {} = { roots: TlsRoots, identity: Option[TlsIdentity], alpn: List[String] }`

<a id="def-core_401_2fTlsClientAuth"></a>

- `type TlsClientAuth uses {} = Anonymous() | Request(Bytes) | Require(Bytes)`

<a id="def-core_401_2fTlsClientAuth_23Anonymous"></a>

- `Anonymous() -> TlsClientAuth`

Variant of TlsClientAuth.

<a id="def-core_401_2fTlsClientAuth_23Request"></a>

- `Request(Bytes) -> TlsClientAuth`

Variant of TlsClientAuth.

<a id="def-core_401_2fTlsClientAuth_23Require"></a>

- `Require(Bytes) -> TlsClientAuth`

Variant of TlsClientAuth.

<a id="def-core_401_2fTlsFailure"></a>

- `type TlsFailure: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = UnknownIssuer() | Expired() | NotYetValid() | NameMismatch() | Revoked() | BadCertificate() | AlertReceived(u8) | HandshakeFailure() | PeerMisbehaved() | Truncated() | CertificateRequired()`

<a id="def-core_401_2fTlsFailure_23AlertReceived"></a>

- `AlertReceived(u8) -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23BadCertificate"></a>

- `BadCertificate() -> TlsFailure`

Variant of TlsFailure.

<a id="def-core_401_2fTlsFailure_23CertificateRequired"></a>

- `CertificateRequired() -> TlsFailure`

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

<a id="def-core_401_2fTlsIdentity"></a>

- `type TlsIdentity uses {} = { chain: Bytes, key: Bytes }`

<a id="def-core_401_2fTlsRoots"></a>

- `type TlsRoots: serializable uses {} = WebPki() | Pem(Bytes)`

<a id="def-core_401_2fTlsRoots_23Pem"></a>

- `Pem(Bytes) -> TlsRoots`

Variant of TlsRoots.

<a id="def-core_401_2fTlsRoots_23WebPki"></a>

- `WebPki() -> TlsRoots`

Variant of TlsRoots.

<a id="def-core_401_2fTlsServer"></a>

- `type TlsServer uses {} = { identities: List[TlsIdentity], clients: TlsClientAuth, alpn: List[String] }`

<a id="def-core_401_2fUdpReceived"></a>

- `type UdpReceived: serializable uses {} = { count: u64, peer: SocketAddr, truncated: bool }`

<a id="def-core_401_2fUdpSocket"></a>

- `opaque resource type UdpSocket: sendable + noalloc_drop uses {Net}`

<a id="def-core_401_2fUnsafe"></a>

- `@capability(global) opaque type Unsafe: plain + copyable + sendable + noalloc_drop uses {Unsafe}`

<a id="def-core_401_2fVariantShape"></a>

- `type VariantShape: copyable + data + key + inert + noalloc_drop uses {} = { name: String, payload: List[PayloadShape] }`

<a id="def-core_401_2fWaitError"></a>

- `type WaitError: plain + copyable + data + key + sendable + inert + noalloc_drop + serializable uses {} = Clock(ClockError) | Canceled(CancelCause)`

<a id="def-core_401_2fWaitError_23Canceled"></a>

- `Canceled(CancelCause) -> WaitError`

Variant of WaitError.

<a id="def-core_401_2fWaitError_23Clock"></a>

- `Clock(ClockError) -> WaitError`

Variant of WaitError.

<a id="def-core_401_2fWallClock"></a>

- `@capability(global) opaque type WallClock: plain + copyable + sendable + noalloc_drop uses {WallClock}`

<a id="def-core_401_2fWatchEvent"></a>

- `type WatchEvent: plain + copyable + data + key + sendable + inert + noalloc_drop uses {} = Created(Path) | Modified(Path) | Removed(Path) | Renamed(Path, Path) | Overflow()`

<a id="def-core_401_2fWatchEvent_23Created"></a>

- `Created(Path) -> WatchEvent`

Variant of WatchEvent.

<a id="def-core_401_2fWatchEvent_23Modified"></a>

- `Modified(Path) -> WatchEvent`

Variant of WatchEvent.

<a id="def-core_401_2fWatchEvent_23Overflow"></a>

- `Overflow() -> WatchEvent`

Variant of WatchEvent.

<a id="def-core_401_2fWatchEvent_23Removed"></a>

- `Removed(Path) -> WatchEvent`

Variant of WatchEvent.

<a id="def-core_401_2fWatchEvent_23Renamed"></a>

- `Renamed(Path, Path) -> WatchEvent`

Variant of WatchEvent.

<a id="def-core_401_2fWatcher"></a>

- `opaque resource type Watcher: noalloc_drop uses {Dir}`

<a id="def-core_401_2fWriteHalf"></a>

- `opaque resource type WriteHalf: sendable + noalloc_drop uses {Net}`

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


## actor

<a id="def-std_401_2factor_2fActor"></a>

- `pub opaque resource type actor.Actor[M]: sendable uses {Tasks, M}`

A mailbox handle. Duplicate it with `actor.dup`; the actor owns its state
and processes messages in FIFO order on one task.

References: [`core@1/Tasks`](#def-core_401_2fTasks)

<a id="def-std_401_2factor_2fReply"></a>

- `pub type actor.Reply[R] = Sender[R]`

The sending half of a one-request reply channel.

References: [`core@1/Sender`](#def-core_401_2fSender)

<a id="def-std_401_2factor_2fcall"></a>

- `pub fn actor.call[M: sendable, R: sendable]( tasks: Tasks, mut target: Actor[M], take make: once fn(take Reply[R]) -> M ) -> Result[R, ChannelError]`

Send a request and wait for its reply. Waiting may observe cancellation;
a closed mailbox or an abandoned reply returns `Closed`.

References: [`core@1/ChannelError`](#def-core_401_2fChannelError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/actor/Actor`](#def-std_401_2factor_2fActor), [`std@1/actor/Reply`](#def-std_401_2factor_2fReply)

<a id="def-std_401_2factor_2fcast"></a>

- `pub fn actor.cast[M: sendable](tasks: Tasks, mut target: Actor[M], take message: M) -> Result[ (), (M, ChannelError) ]`

Send without checking cancellation. A closed mailbox returns the message.

References: [`core@1/ChannelError`](#def-core_401_2fChannelError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/actor/Actor`](#def-std_401_2factor_2fActor)

<a id="def-std_401_2factor_2fdup"></a>

- `pub fn actor.dup[M: sendable](tasks: Tasks, target: Actor[M]) -> Actor[M]`

Acquire another mailbox handle; ordinary copies are forbidden.

References: [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/actor/Actor`](#def-std_401_2factor_2fActor)

<a id="def-std_401_2factor_2freply"></a>

- `pub fn actor.reply[R: sendable](tasks: Tasks, take response: Reply[R], take value: R) -> Result[ (), (R, ChannelError) ]`

Reply once, consuming the sending endpoint. A closed caller gets its value back.

References: [`core@1/ChannelError`](#def-core_401_2fChannelError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/actor/Reply`](#def-std_401_2factor_2fReply)

<a id="def-std_401_2factor_2fspawn"></a>

- `pub fn actor.spawn[S, M: sendable]( s: Scope, tasks: Tasks, take initial: unique sendable S, take handle: sendable fn(mut unique S, take M) -> () ) -> Actor[M]`

Start an actor in `s`. Closing every handle drains the mailbox and ends
the task. Handlers may use capabilities, including I/O.

References: [`core@1/Scope`](#def-core_401_2fScope), [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/actor/Actor`](#def-std_401_2factor_2fActor)


## adler32

<a id="def-std_401_2fadler32_2fState"></a>

- `pub opaque type adler32.State: plain + inert uses {}`

<a id="def-std_401_2fadler32_2fdigest"></a>

- `@noalloc pub fn adler32.digest(data: Bytes) -> u32 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fadler32_2fdigest_5fhex"></a>

- `pub fn adler32.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fadler32_2ffinish"></a>

- `@noalloc pub fn adler32.finish(take state: State) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/adler32/State`](#def-std_401_2fadler32_2fState)

<a id="def-std_401_2fadler32_2fnew"></a>

- `@noalloc pub fn adler32.new() -> State uses {}`

References: [`std@1/adler32/State`](#def-std_401_2fadler32_2fState)

<a id="def-std_401_2fadler32_2fupdate"></a>

- `@noalloc pub fn adler32.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/adler32/State`](#def-std_401_2fadler32_2fState)

<a id="def-std_401_2fadler32_2fupdate_5flist"></a>

- `@noalloc pub fn adler32.update_list(mut state: State, data: List[u8], start: u64) -> () uses {}`

Extends the checksum with the bytes of a list builder from `start` on.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/adler32/State`](#def-std_401_2fadler32_2fState)


## aead

<a id="def-std_401_2faead_2fOpenError"></a>

- `pub type aead.OpenError = AuthenticationFailed()`

<a id="def-std_401_2faead_2fOpenError_23AuthenticationFailed"></a>

- `AuthenticationFailed() -> aead.OpenError`

Variant of aead.OpenError.


## aes_gcm

<a id="def-std_401_2faes_5fgcm_2fopen"></a>

- `pub fn aes_gcm.open(key: Bytes, nonce: Bytes, sealed: Bytes, data: Bytes) -> Result[ unique Bytes, std@1/aead.OpenError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/aead/OpenError`](#def-std_401_2faead_2fOpenError)

<a id="def-std_401_2faes_5fgcm_2fseal"></a>

- `pub fn aes_gcm.seal(key: Bytes, nonce: Bytes, plaintext: Bytes, data: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)


## arena

<a id="def-core_401_2farena_2fbounded"></a>

- `@noalloc fn arena.bounded[R](cap: u64, body: once fn(a: Arena) -> R) -> Result[R, OutOfSpace]`

<a id="def-core_401_2farena_2fseal"></a>

- `fn arena.seal[T](take a: Arena in a, take root: T in root) -> Region[T]`

<a id="def-core_401_2farena_2fwith"></a>

- `@noalloc fn arena.with[R](body: once fn(a: Arena) -> R) -> R`


## arg

<a id="def-core_401_2farg_2flen"></a>

- `@noalloc fn arg.len(args: Args) -> u64`

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


## ascii85

<a id="def-std_401_2fascii85_2fDecodeError"></a>

- `@serializable pub type ascii85.DecodeError = CorruptInput(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fascii85_2fDecodeError_23CorruptInput"></a>

- `CorruptInput(u64) -> ascii85.DecodeError`

Variant of ascii85.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fascii85_2fdecode"></a>

- `pub fn ascii85.decode(data: Bytes) -> Result[unique Bytes, DecodeError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/ascii85/DecodeError`](#def-std_401_2fascii85_2fDecodeError)

<a id="def-std_401_2fascii85_2fencode"></a>

- `pub fn ascii85.encode(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)


## atomic

<a id="def-core_401_2fatomic_2fcreate"></a>

- `fn atomic.create[A](tasks: Tasks, value: A) -> Atomic[A]`

<a id="def-core_401_2fatomic_2fload"></a>

- `@noalloc fn atomic.load[A](tasks: Tasks, a: Atomic[A]) -> A`

<a id="def-core_401_2fatomic_2fstore"></a>

- `@noalloc fn atomic.store[A](tasks: Tasks, a: Atomic[A], value: A) -> ()`


## base32

<a id="def-std_401_2fbase32_2fAlphabet"></a>

- `@serializable pub type base32.Alphabet = Standard | Hex`

<a id="def-std_401_2fbase32_2fAlphabet_23Hex"></a>

- `Hex -> base32.Alphabet`

Variant of base32.Alphabet.

<a id="def-std_401_2fbase32_2fAlphabet_23Standard"></a>

- `Standard -> base32.Alphabet`

Variant of base32.Alphabet.

<a id="def-std_401_2fbase32_2fDecodeError"></a>

- `@serializable pub type base32.DecodeError = CorruptInput(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase32_2fDecodeError_23CorruptInput"></a>

- `CorruptInput(u64) -> base32.DecodeError`

Variant of base32.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase32_2fPadding"></a>

- `@serializable pub type base32.Padding = Required | Omitted`

<a id="def-std_401_2fbase32_2fPadding_23Omitted"></a>

- `Omitted -> base32.Padding`

Variant of base32.Padding.

<a id="def-std_401_2fbase32_2fPadding_23Required"></a>

- `Required -> base32.Padding`

Variant of base32.Padding.

<a id="def-std_401_2fbase32_2fdecode"></a>

- `pub fn base32.decode(input: String, alphabet: Alphabet, padding: Padding) -> Result[ unique Bytes, DecodeError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/base32/Alphabet`](#def-std_401_2fbase32_2fAlphabet), [`std@1/base32/DecodeError`](#def-std_401_2fbase32_2fDecodeError), [`std@1/base32/Padding`](#def-std_401_2fbase32_2fPadding)

<a id="def-std_401_2fbase32_2fencode"></a>

- `pub fn base32.encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString), [`std@1/base32/Alphabet`](#def-std_401_2fbase32_2fAlphabet), [`std@1/base32/Padding`](#def-std_401_2fbase32_2fPadding)

<a id="def-std_401_2fbase32_2fencoded_5flen"></a>

- `@noalloc pub fn base32.encoded_len(n: u64, padding: Padding) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/base32/Padding`](#def-std_401_2fbase32_2fPadding)


## base64

<a id="def-std_401_2fbase64_2fAlphabet"></a>

- `pub type base64.Alphabet = core@1/Base64Alphabet`

References: [`core@1/Base64Alphabet`](#def-core_401_2fBase64Alphabet)

<a id="def-std_401_2fbase64_2fDecodeError"></a>

- `@serializable pub type base64.DecodeError = InvalidByte(u64) | InvalidLength(u64) | InvalidPadding(u64) | NonCanonical( u64 )`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidByte"></a>

- `InvalidByte(u64) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidLength"></a>

- `InvalidLength(u64) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23InvalidPadding"></a>

- `InvalidPadding(u64) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecodeError_23NonCanonical"></a>

- `NonCanonical( u64 ) -> base64.DecodeError`

Variant of base64.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbase64_2fDecoder"></a>

- `pub opaque type base64.Decoder: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fbase64_2fEncoder"></a>

- `pub opaque type base64.Encoder: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fbase64_2fPadding"></a>

- `pub type base64.Padding = core@1/Base64Padding`

References: [`core@1/Base64Padding`](#def-core_401_2fBase64Padding)

<a id="def-std_401_2fbase64_2fStreamError"></a>

- `@serializable pub type base64.StreamError = Closed | Decode(DecodeError)`

References: [`std@1/base64/DecodeError`](#def-std_401_2fbase64_2fDecodeError)

<a id="def-std_401_2fbase64_2fStreamError_23Closed"></a>

- `Closed -> base64.StreamError`

Variant of base64.StreamError.

<a id="def-std_401_2fbase64_2fStreamError_23Decode"></a>

- `Decode(DecodeError) -> base64.StreamError`

Variant of base64.StreamError.

References: [`std@1/base64/DecodeError`](#def-std_401_2fbase64_2fDecodeError)

<a id="def-std_401_2fbase64_2fdecode"></a>

- `pub fn base64.decode(text: String, alphabet: Alphabet, padding: Padding) -> Result[ unique Bytes, DecodeError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/DecodeError`](#def-std_401_2fbase64_2fDecodeError), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)

<a id="def-std_401_2fbase64_2fdecode_5ffeed"></a>

- `pub fn base64.decode_feed(mut state: Decoder, data: Bytes) -> Result[unique Bytes, StreamError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/base64/Decoder`](#def-std_401_2fbase64_2fDecoder), [`std@1/base64/StreamError`](#def-std_401_2fbase64_2fStreamError)

<a id="def-std_401_2fbase64_2fdecode_5ffinish"></a>

- `pub fn base64.decode_finish(mut state: Decoder) -> Result[unique Bytes, StreamError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/base64/Decoder`](#def-std_401_2fbase64_2fDecoder), [`std@1/base64/StreamError`](#def-std_401_2fbase64_2fStreamError)

<a id="def-std_401_2fbase64_2fdecoder"></a>

- `@noalloc pub fn base64.decoder(alphabet: Alphabet, padding: Padding) -> Decoder uses {}`

References: [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/Decoder`](#def-std_401_2fbase64_2fDecoder), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)

<a id="def-std_401_2fbase64_2fencode"></a>

- `pub fn base64.encode(data: Bytes, alphabet: Alphabet, padding: Padding) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)

<a id="def-std_401_2fbase64_2fencode_5ffeed"></a>

- `pub fn base64.encode_feed(mut state: Encoder, data: Bytes) -> Result[ unique String, StreamError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Encoder`](#def-std_401_2fbase64_2fEncoder), [`std@1/base64/StreamError`](#def-std_401_2fbase64_2fStreamError)

<a id="def-std_401_2fbase64_2fencode_5ffinish"></a>

- `pub fn base64.encode_finish(mut state: Encoder) -> Result[unique String, StreamError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/base64/Encoder`](#def-std_401_2fbase64_2fEncoder), [`std@1/base64/StreamError`](#def-std_401_2fbase64_2fStreamError)

<a id="def-std_401_2fbase64_2fencoder"></a>

- `@noalloc pub fn base64.encoder(alphabet: Alphabet, padding: Padding) -> Encoder uses {}`

References: [`std@1/base64/Alphabet`](#def-std_401_2fbase64_2fAlphabet), [`std@1/base64/Encoder`](#def-std_401_2fbase64_2fEncoder), [`std@1/base64/Padding`](#def-std_401_2fbase64_2fPadding)


## bigint

<a id="def-std_401_2fbigint_2fBigInt"></a>

- `@format(format_into) pub opaque type bigint.BigInt: copyable + data + key + inert + noalloc_drop uses {}`

An exact integer of any size. Its representation is canonical (no high zero
limbs, and zero is never negative), so `==` and hashing compare values.

References: [`std@1/bigint/format_into`](#def-std_401_2fbigint_2fformat_5finto)

<a id="def-std_401_2fbigint_2fabs"></a>

- `pub fn bigint.abs(value: BigInt) -> BigInt uses {}`

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fadd"></a>

- `pub fn bigint.add(a: BigInt, b: BigInt) -> BigInt uses {}`

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fbit_5fand"></a>

- `pub fn bigint.bit_and(a: BigInt, b: BigInt) -> BigInt uses {}`

Bitwise and.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fbit_5flength"></a>

- `pub fn bigint.bit_length(value: BigInt) -> u64 uses {}`

The number of bits in the magnitude; zero has none.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fbit_5fnot"></a>

- `pub fn bigint.bit_not(value: BigInt) -> BigInt uses {}`

Bitwise complement, -value - 1.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fbit_5for"></a>

- `pub fn bigint.bit_or(a: BigInt, b: BigInt) -> BigInt uses {}`

Bitwise inclusive or.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fbit_5fxor"></a>

- `pub fn bigint.bit_xor(a: BigInt, b: BigInt) -> BigInt uses {}`

Bitwise exclusive or.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fcompare"></a>

- `pub fn bigint.compare(a: BigInt, b: BigInt) -> i32 uses {}`

-1, 0 or 1 as `a` is less than, equal to or greater than `b`.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fdiv_5fmod_5ffloor"></a>

- `pub fn bigint.div_mod_floor(a: BigInt, b: BigInt) -> (BigInt, BigInt) uses {}`

The quotient rounded toward negative infinity and the remainder with the divisor's
sign, as `int.div_floor` and `int.mod`. A zero divisor fails SEM-1.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fdiv_5frem"></a>

- `pub fn bigint.div_rem(a: BigInt, b: BigInt) -> (BigInt, BigInt) uses {}`

The quotient truncated toward zero and the remainder with the dividend's sign, as
integer `/` and `%`. A zero divisor fails SEM-1.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn bigint.format_into(value: BigInt, mut out: List[u8]) -> () uses {}`

Appends the decimal text `to_string` returns.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fformat_5fradix"></a>

- `pub fn bigint.format_radix(value: BigInt, radix: u8) -> Result[ unique String, scalar.ParseError ] uses {}`

Text in `radix` 2 through 36 with lowercase letters, a minus for negative values and
no prefix; an invalid radix is `Invalid`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fbigint_2ffrom_5fi128"></a>

- `pub fn bigint.from_i128(value: i128) -> BigInt uses {}`

The value of a signed 128-bit integer; the narrower widths widen to it.

References: [`core@1/i128`](#def-core_401_2fi128), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fi16"></a>

- `pub fn bigint.from_i16(value: i16) -> BigInt uses {}`

References: [`core@1/i16`](#def-core_401_2fi16), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fi32"></a>

- `pub fn bigint.from_i32(value: i32) -> BigInt uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fi64"></a>

- `pub fn bigint.from_i64(value: i64) -> BigInt uses {}`

References: [`core@1/i64`](#def-core_401_2fi64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fi8"></a>

- `pub fn bigint.from_i8(value: i8) -> BigInt uses {}`

References: [`core@1/i8`](#def-core_401_2fi8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fu128"></a>

- `pub fn bigint.from_u128(value: u128) -> BigInt uses {}`

The value of an unsigned 128-bit integer; the narrower widths widen to it.

References: [`core@1/u128`](#def-core_401_2fu128), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fu16"></a>

- `pub fn bigint.from_u16(value: u16) -> BigInt uses {}`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fu32"></a>

- `pub fn bigint.from_u32(value: u32) -> BigInt uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fu64"></a>

- `pub fn bigint.from_u64(value: u64) -> BigInt uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2ffrom_5fu8"></a>

- `pub fn bigint.from_u8(value: u8) -> BigInt uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fgcd"></a>

- `pub fn bigint.gcd(a: BigInt, b: BigInt) -> BigInt uses {}`

The nonnegative greatest common divisor; gcd(0, 0) is 0. Stein's binary algorithm.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fis_5fnegative"></a>

- `pub fn bigint.is_negative(value: BigInt) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fis_5fzero"></a>

- `pub fn bigint.is_zero(value: BigInt) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fmul"></a>

- `pub fn bigint.mul(a: BigInt, b: BigInt) -> BigInt uses {}`

The product; operands longer than 32 limbs multiply by Karatsuba.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fneg"></a>

- `pub fn bigint.neg(value: BigInt) -> BigInt uses {}`

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fparse"></a>

- `pub fn bigint.parse(text: String) -> Result[BigInt, scalar.ParseError] uses {}`

Parses `-?[0-9]+` in decimal, as `parse_radix(text, 10)`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fbigint_2fparse_5fradix"></a>

- `pub fn bigint.parse_radix(text: String, radix: u8) -> Result[BigInt, scalar.ParseError] uses {}`

Parses an optional minus and one or more digits of `radix` (2 through 36, either
letter case). Leading zeros are accepted; a plus, whitespace, underscores and radix
prefixes are `Invalid`, as is an invalid radix. No value is out of range.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fbigint_2fpow"></a>

- `pub fn bigint.pow(base: BigInt, exponent: u64) -> BigInt uses {}`

`base` raised to `exponent`, by binary exponentiation; zero to the zero is one.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fshl"></a>

- `pub fn bigint.shl(value: BigInt, bits: u64) -> BigInt uses {}`

`value` times 2 to the `bits`.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fshr"></a>

- `pub fn bigint.shr(value: BigInt, bits: u64) -> BigInt uses {}`

`value` divided by 2 to the `bits`, rounded toward negative infinity, as an
arithmetic shift of the two's-complement value.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fsignum"></a>

- `pub fn bigint.signum(value: BigInt) -> i32 uses {}`

-1, 0 or 1 by the sign of `value`.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fsub"></a>

- `pub fn bigint.sub(a: BigInt, b: BigInt) -> BigInt uses {}`

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fi128"></a>

- `pub fn bigint.to_i128(value: BigInt) -> Option[i128] uses {}`

The value as an i128, or None when it is out of range.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i128`](#def-core_401_2fi128), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fi16"></a>

- `pub fn bigint.to_i16(value: BigInt) -> Option[i16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i16`](#def-core_401_2fi16), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fi32"></a>

- `pub fn bigint.to_i32(value: BigInt) -> Option[i32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fi64"></a>

- `pub fn bigint.to_i64(value: BigInt) -> Option[i64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i64`](#def-core_401_2fi64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fi8"></a>

- `pub fn bigint.to_i8(value: BigInt) -> Option[i8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i8`](#def-core_401_2fi8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fstring"></a>

- `pub fn bigint.to_string(value: BigInt) -> unique String uses {}`

Decimal text: a minus for negative values, then digits without leading zeros.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fu128"></a>

- `pub fn bigint.to_u128(value: BigInt) -> Option[u128] uses {}`

The value as a u128, or None when it is negative or too large.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u128`](#def-core_401_2fu128), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fu16"></a>

- `pub fn bigint.to_u16(value: BigInt) -> Option[u16] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u16`](#def-core_401_2fu16), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fu32"></a>

- `pub fn bigint.to_u32(value: BigInt) -> Option[u32] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u32`](#def-core_401_2fu32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fu64"></a>

- `pub fn bigint.to_u64(value: BigInt) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fto_5fu8"></a>

- `pub fn bigint.to_u8(value: BigInt) -> Option[u8] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)

<a id="def-std_401_2fbigint_2fzero"></a>

- `pub fn bigint.zero() -> BigInt uses {}`

Zero.

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt)


## binary

<a id="def-std_401_2fbinary_2fByteOrder"></a>

- `pub type binary.ByteOrder = LittleEndian | BigEndian`

Explicit wire order, independent of host endianness and alignment.

<a id="def-std_401_2fbinary_2fByteOrder_23BigEndian"></a>

- `BigEndian -> binary.ByteOrder`

Variant of binary.ByteOrder.

<a id="def-std_401_2fbinary_2fByteOrder_23LittleEndian"></a>

- `LittleEndian -> binary.ByteOrder`

Variant of binary.ByteOrder.

<a id="def-std_401_2fbinary_2fDecodeError"></a>

- `pub type binary.DecodeError = Truncated(u64) | Syntax(u64) | Unsupported(u64) | Utf8(u64) | DuplicateKey( u64 ) | Nesting(u64) | Trailing(u64) | InvalidNumber`

Errors shared by MessagePack and CBOR JSON-domain codecs. Decode payloads
are byte offsets; encoding Nesting carries the attempted container depth.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23DuplicateKey"></a>

- `DuplicateKey( u64 ) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23InvalidNumber"></a>

- `InvalidNumber -> binary.DecodeError`

Variant of binary.DecodeError.

<a id="def-std_401_2fbinary_2fDecodeError_23Nesting"></a>

- `Nesting(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23Syntax"></a>

- `Syntax(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23Trailing"></a>

- `Trailing(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23Truncated"></a>

- `Truncated(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23Unsupported"></a>

- `Unsupported(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fDecodeError_23Utf8"></a>

- `Utf8(u64) -> binary.DecodeError`

Variant of binary.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fReadError"></a>

- `pub type binary.ReadError { offset: u64, }`

Incomplete fixed-width read. The offset is exactly the requested byte offset,
including when it is past the end of the input.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbinary_2fappend_5fu16"></a>

- `@preserves_unique(out) pub fn binary.append_u16(value: u16, order: ByteOrder, mut out: List[u8]) -> () uses {}`

Append two bytes, preserving the destination prefix and uniqueness.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u8`](#def-core_401_2fu8), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder)

<a id="def-std_401_2fbinary_2fappend_5fu32"></a>

- `@preserves_unique(out) pub fn binary.append_u32(value: u32, order: ByteOrder, mut out: List[u8]) -> () uses {}`

Append four bytes, preserving the destination prefix and uniqueness.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder)

<a id="def-std_401_2fbinary_2fappend_5fu64"></a>

- `@preserves_unique(out) pub fn binary.append_u64(value: u64, order: ByteOrder, mut out: List[u8]) -> () uses {}`

Append eight bytes, preserving the destination prefix and uniqueness.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder)

<a id="def-std_401_2fbinary_2fread_5fu16"></a>

- `@noalloc pub fn binary.read_u16(data: Bytes, offset: u64, order: ByteOrder) -> Result[u16, ReadError] uses {}`

Read two bytes at any offset, or return the requested offset on truncation.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder), [`std@1/binary/ReadError`](#def-std_401_2fbinary_2fReadError)

<a id="def-std_401_2fbinary_2fread_5fu32"></a>

- `@noalloc pub fn binary.read_u32(data: Bytes, offset: u64, order: ByteOrder) -> Result[u32, ReadError] uses {}`

Read four bytes at any offset, or return the requested offset on truncation.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder), [`std@1/binary/ReadError`](#def-std_401_2fbinary_2fReadError)

<a id="def-std_401_2fbinary_2fread_5fu64"></a>

- `@noalloc pub fn binary.read_u64(data: Bytes, offset: u64, order: ByteOrder) -> Result[u64, ReadError] uses {}`

Read eight bytes at any offset, or return the requested offset on truncation.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/binary/ByteOrder`](#def-std_401_2fbinary_2fByteOrder), [`std@1/binary/ReadError`](#def-std_401_2fbinary_2fReadError)


## bitset

<a id="def-std_401_2fbitset_2fBitSet"></a>

- `pub opaque type bitset.BitSet: () uses {}`

Fixed domain [0, domain_len), stored in zero-initialized u64 words.

<a id="def-std_401_2fbitset_2fBitSetError"></a>

- `pub type bitset.BitSetError = IndexOutOfBounds(u64, u64) | DomainMismatch(u64, u64)`

IndexOutOfBounds(requested index, domain length), or
DomainMismatch(destination domain length, source domain length).

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbitset_2fBitSetError_23DomainMismatch"></a>

- `DomainMismatch(u64, u64) -> bitset.BitSetError`

Variant of bitset.BitSetError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbitset_2fBitSetError_23IndexOutOfBounds"></a>

- `IndexOutOfBounds(u64, u64) -> bitset.BitSetError`

Variant of bitset.BitSetError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fbitset_2fclear"></a>

- `@noalloc pub fn bitset.clear(mut bits: unique BitSet) -> () uses {}`

References: [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet)

<a id="def-std_401_2fbitset_2fcontains"></a>

- `@noalloc pub fn bitset.contains(bits: BitSet, index: u64) -> Result[bool, BitSetError] uses {}`

Invalid indices return a typed error, including u64::MAX.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)

<a id="def-std_401_2fbitset_2fcount"></a>

- `@noalloc pub fn bitset.count(bits: BitSet) -> u64 uses {}`

Scalar counting consumes one set bit per iteration (at most 64 per word).

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet)

<a id="def-std_401_2fbitset_2fdifference"></a>

- `@noalloc pub fn bitset.difference(mut destination: unique BitSet, source: BitSet) -> Result[ (), BitSetError ] uses {}`

Removes source members from destination after checking equal domains.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)

<a id="def-std_401_2fbitset_2fdomain_5flen"></a>

- `@noalloc pub fn bitset.domain_len(bits: BitSet) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet)

<a id="def-std_401_2fbitset_2fempty"></a>

- `pub fn bitset.empty(length: u64) -> unique BitSet uses {}`

Allocates ceil(length / 64) words. Empty domains are valid.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet)

<a id="def-std_401_2fbitset_2findices"></a>

- `pub fn bitset.indices(bits: BitSet) -> unique List[u64] uses {}`

Ascending owned snapshot; later mutations or destruction cannot affect it.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet)

<a id="def-std_401_2fbitset_2finsert"></a>

- `@noalloc pub fn bitset.insert(mut bits: unique BitSet, index: u64) -> Result[bool, BitSetError] uses {}`

Reports membership BEFORE insertion; invalid indices leave the set unchanged.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)

<a id="def-std_401_2fbitset_2fintersection"></a>

- `@noalloc pub fn bitset.intersection(mut destination: unique BitSet, source: BitSet) -> Result[ (), BitSetError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)

<a id="def-std_401_2fbitset_2fremove"></a>

- `@noalloc pub fn bitset.remove(mut bits: unique BitSet, index: u64) -> Result[bool, BitSetError] uses {}`

Reports membership BEFORE removal; invalid indices leave the set unchanged.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)

<a id="def-std_401_2fbitset_2funion"></a>

- `@noalloc pub fn bitset.union(mut destination: unique BitSet, source: BitSet) -> Result[(), BitSetError] uses {}`

Mutates destination only after checking equal domain lengths.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/bitset/BitSet`](#def-std_401_2fbitset_2fBitSet), [`std@1/bitset/BitSetError`](#def-std_401_2fbitset_2fBitSetError)


## blake2b

<a id="def-std_401_2fblake2b_2fState"></a>

- `pub opaque type blake2b.State: plain + inert uses {}`

<a id="def-std_401_2fblake2b_2fdigest"></a>

- `@noalloc pub fn blake2b.digest(data: Bytes) -> Inline[u8, 64] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fblake2b_2fdigest_5fhex"></a>

- `pub fn blake2b.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fblake2b_2ffinish"></a>

- `@noalloc pub fn blake2b.finish(take state: State) -> Inline[u8, 64] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/blake2b/State`](#def-std_401_2fblake2b_2fState)

<a id="def-std_401_2fblake2b_2fhash"></a>

- `pub implementation blake2b.hash: std@1/digest.Digest { type State = State size bytes = 64 size block = 128 new = new update = update finish = finish }`

References: [`std@1/blake2b/State`](#def-std_401_2fblake2b_2fState)

<a id="def-std_401_2fblake2b_2fnew"></a>

- `@noalloc pub fn blake2b.new() -> State uses {}`

References: [`std@1/blake2b/State`](#def-std_401_2fblake2b_2fState)

<a id="def-std_401_2fblake2b_2fnew_5fkeyed"></a>

- `@noalloc pub fn blake2b.new_keyed(key: Bytes) -> State uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/blake2b/State`](#def-std_401_2fblake2b_2fState)

<a id="def-std_401_2fblake2b_2fupdate"></a>

- `@noalloc pub fn blake2b.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/blake2b/State`](#def-std_401_2fblake2b_2fState)


## blake2s

<a id="def-std_401_2fblake2s_2fState"></a>

- `pub opaque type blake2s.State: plain + inert uses {}`

<a id="def-std_401_2fblake2s_2fdigest"></a>

- `@noalloc pub fn blake2s.digest(data: Bytes) -> Inline[u8, 32] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fblake2s_2fdigest_5fhex"></a>

- `pub fn blake2s.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fblake2s_2ffinish"></a>

- `@noalloc pub fn blake2s.finish(take state: State) -> Inline[u8, 32] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/blake2s/State`](#def-std_401_2fblake2s_2fState)

<a id="def-std_401_2fblake2s_2fhash"></a>

- `pub implementation blake2s.hash: std@1/digest.Digest { type State = State size bytes = 32 size block = 64 new = new update = update finish = finish }`

References: [`std@1/blake2s/State`](#def-std_401_2fblake2s_2fState)

<a id="def-std_401_2fblake2s_2fnew"></a>

- `@noalloc pub fn blake2s.new() -> State uses {}`

References: [`std@1/blake2s/State`](#def-std_401_2fblake2s_2fState)

<a id="def-std_401_2fblake2s_2fnew_5fkeyed"></a>

- `@noalloc pub fn blake2s.new_keyed(key: Bytes) -> State uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/blake2s/State`](#def-std_401_2fblake2s_2fState)

<a id="def-std_401_2fblake2s_2fupdate"></a>

- `@noalloc pub fn blake2s.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/blake2s/State`](#def-std_401_2fblake2s_2fState)


## bool

<a id="def-std_401_2fbool_2fformat_5finline"></a>

- `@noalloc pub fn bool.format_inline[n: size](value: bool, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

- `@serializable pub type bufio.ReadError[E] = Source(E) | Limit | UnexpectedEof | InvalidUtf8 | InvalidRead`

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

<a id="def-std_401_2fbufio_2fWriteError"></a>

- `@serializable pub type bufio.WriteError[E] = Sink(E) | WriteZero | InvalidWrite`

<a id="def-std_401_2fbufio_2fWriteError_23InvalidWrite"></a>

- `InvalidWrite -> bufio.WriteError[E]`

Variant of bufio.WriteError.

<a id="def-std_401_2fbufio_2fWriteError_23Sink"></a>

- `Sink(E) -> bufio.WriteError[E]`

Variant of bufio.WriteError.

<a id="def-std_401_2fbufio_2fWriteError_23WriteZero"></a>

- `WriteZero -> bufio.WriteError[E]`

Variant of bufio.WriteError.

<a id="def-std_401_2fbufio_2fWriter"></a>

- `pub opaque type bufio.Writer: () uses {}`

<a id="def-std_401_2fbufio_2fbuffered"></a>

- `pub fn bufio.buffered(r: Reader) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fbuffered_5flen"></a>

- `pub fn bufio.buffered_len(w: Writer) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Writer`](#def-std_401_2fbufio_2fWriter)

<a id="def-std_401_2fbufio_2fconsume"></a>

- `@preserves_captures(r) pub fn bufio.consume(mut r: Reader, count: u64) -> () uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fcontents"></a>

- `pub fn bufio.contents(r: Reader) -> unique Bytes uses {}`

An independent owned snapshot; use `peek` to inspect without copying.

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

<a id="def-std_401_2fbufio_2fflush"></a>

- `pub fn bufio.flush[S, E]( mut w: unique Writer, mut sink: S, write: fn(mut S, List[u8], u64, u64) -> Result[u64, E] ) -> Result[(), WriteError[E]] uses {write}`

The callback borrows initialized storage, offering only [start,end).
Ok(n) commits exactly that prefix; Error(e) must accept no bytes this call.
A successful short write is retried. No empty callback, hidden error or drop I/O.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/WriteError`](#def-std_401_2fbufio_2fWriteError), [`std@1/bufio/Writer`](#def-std_401_2fbufio_2fWriter)

<a id="def-std_401_2fbufio_2fnew"></a>

- `pub fn bufio.new(capacity: u64) -> Reader uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fnew_5fwriter"></a>

- `pub fn bufio.new_writer(capacity: u64) -> unique Writer uses {}`

Fixed pending-payload capacity; zero is normalized to one.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Writer`](#def-std_401_2fbufio_2fWriter)

<a id="def-std_401_2fbufio_2fpeek"></a>

- `pub fn bufio.peek(r: Reader, max: u64) -> Bytes in r uses {}`

Borrows at most `max` unread bytes without refilling or advancing `r`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

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

- `@preserves_captures(s, r) pub fn bufio.stream_read_more(n: Net, mut s: Stream of n, mut r: Reader, max: u64) -> Result[ u64, ReadError[NetError] ]`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fstream_5fread_5funtil"></a>

- `@preserves_unique(out) pub fn bufio.stream_read_until( n: Net, mut s: Stream of n, mut r: Reader, delimiter: u8, mut out: List[u8], limit: u64 ) -> Result[u64, ReadError[NetError]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fbufio_2fwrite_5fbuffered"></a>

- `pub fn bufio.write_buffered(mut w: unique Writer, data: Bytes) -> u64 uses {}`

Accept only the available prefix; never call a sink. Reclaim committed space.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64), [`std@1/bufio/Writer`](#def-std_401_2fbufio_2fWriter)


## bytes

<a id="def-core_401_2fbytes_2fappend"></a>

- `@preserves_unique(builder) @unretained(data) fn bytes.append(mut builder: List[u8], data: Bytes) -> ()`

<a id="def-core_401_2fbytes_2fappend_5ffrom_5fhistory"></a>

- `@preserves_unique(out) fn bytes.append_from_history(mut out: List[u8], distance: u64, count: u64) -> ()`

<a id="def-core_401_2fbytes_2fbase64_5fdecode"></a>

- `fn bytes.base64_decode(data: Bytes, alphabet: Base64Alphabet, padding: Base64Padding) -> unique Option[Bytes] uses {}`

<a id="def-core_401_2fbytes_2fbase64_5fencode"></a>

- `fn bytes.base64_encode(data: Bytes, alphabet: Base64Alphabet, padding: Base64Padding) -> unique Bytes uses {}`

<a id="def-core_401_2fbytes_2fdecimal_5fword"></a>

- `@noalloc fn bytes.decimal_word(value: u64, mut buffer: Inline[u8, 20]) -> u64`

<a id="def-core_401_2fbytes_2ffind_5fany"></a>

- `@noalloc fn bytes.find_any(src: Bytes, needles: Bytes, start: u64) -> u64`

<a id="def-std_401_2fbytes_2ffrom_5finline"></a>

- `pub fn bytes.from_inline[n: size](xs: Inline[u8, n]) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-core_401_2fbytes_2ffrom_5flist"></a>

- `@preserves_unique(src) fn bytes.from_list(take src: List[u8]) -> Bytes`

<a id="def-core_401_2fbytes_2finteger"></a>

- `@noalloc fn bytes.integer(src: Bytes, radix: u8, signed: bool, positive_hi: u64, positive_lo: u64, negative_hi: u64, negative_lo: u64) -> (u64, u64, u64)`

<a id="def-core_401_2fbytes_2fjson_5fnumber_5fend"></a>

- `@noalloc fn bytes.json_number_end(src: Bytes, start: u64) -> (u64, u64)`

<a id="def-core_401_2fbytes_2flen"></a>

- `@noalloc fn bytes.len(src: Bytes) -> u64`

<a id="def-std_401_2fbytes_2freplace"></a>

- `pub fn bytes.replace(src: Bytes, old: Bytes, new: Bytes) -> unique Bytes in src`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-core_401_2fbytes_2freplace_5frange"></a>

- `@preserves_unique(builder) @unretained(data) fn bytes.replace_range(mut builder: List[u8], start: u64, data: Bytes) -> ()`

<a id="def-core_401_2fbytes_2fsha1"></a>

- `@noalloc fn bytes.sha1(data: Bytes) -> Inline[u8, 20] uses {}`

<a id="def-core_401_2fbytes_2fsha224"></a>

- `@noalloc fn bytes.sha224(data: Bytes) -> Inline[u8, 28] uses {}`

<a id="def-core_401_2fbytes_2fsha256"></a>

- `@noalloc fn bytes.sha256(data: Bytes) -> Inline[u8, 32] uses {}`

<a id="def-core_401_2fbytes_2fsha384"></a>

- `@noalloc fn bytes.sha384(data: Bytes) -> Inline[u8, 48] uses {}`

<a id="def-core_401_2fbytes_2fsha512"></a>

- `@noalloc fn bytes.sha512(data: Bytes) -> Inline[u8, 64] uses {}`

<a id="def-core_401_2fbytes_2fsha512_5f256"></a>

- `@noalloc fn bytes.sha512_256(data: Bytes) -> Inline[u8, 32] uses {}`

<a id="def-core_401_2fbytes_2fslice"></a>

- `@noalloc fn bytes.slice(src: Bytes, start: u64, end: u64) -> Bytes in src`


## capture

<a id="def-core_401_2fcapture_2fbegin"></a>

- `fn capture.begin(t: Telemetry, parent: Bytes, spec: Bytes) -> Result[Span of t, TelemetryError]`

<a id="def-core_401_2fcapture_2fcontext"></a>

- `fn capture.context(t: Telemetry, s: Span of t) -> Bytes`

<a id="def-core_401_2fcapture_2fevent"></a>

- `fn capture.event(t: Telemetry, s: Span of t, payload: Bytes) -> Result[(), TelemetryError]`

<a id="def-core_401_2fcapture_2ffinish"></a>

- `fn capture.finish(t: Telemetry, take s: Span of t, status: Bytes) -> Result[(), TelemetryError]`

<a id="def-core_401_2fcapture_2flog"></a>

- `fn capture.log(t: Telemetry, payload: Bytes) -> Result[(), TelemetryError]`

<a id="def-core_401_2fcapture_2fmeasure"></a>

- `fn capture.measure(t: Telemetry, payload: Bytes) -> Result[(), TelemetryError]`


## cbor

<a id="def-std_401_2fcbor_2fdecode"></a>

- `pub fn cbor.decode(data: Bytes) -> Result[json.Value, binary.DecodeError] uses {}`

Decode exactly one RFC 8949 item in the JSON domain. Definite and indefinite
text/arrays/maps and nonpreferred integer/float widths are accepted. Byte
strings, tags, simple values other than bool/null, nonfinite floats and
non-text keys return Unsupported. Duplicate keys and trailing bytes fail.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/binary/DecodeError`](#def-std_401_2fbinary_2fDecodeError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fcbor_2fencode"></a>

- `pub fn cbor.encode(value: json.Value, deterministic: bool) -> Result[ unique Bytes, binary.DecodeError ] uses {}`

Encode definite lengths and shortest integer/length headers. Deterministic
mode uses RFC 8949 core deterministic map order and shortest exact float
width, preserving signed zero. Otherwise maps retain insertion order and
decimal/exponent numbers use binary64. Integers remain exact in either mode.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/binary/DecodeError`](#def-std_401_2fbinary_2fDecodeError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)


## chacha20poly1305

<a id="def-std_401_2fchacha20poly1305_2fopen"></a>

- `pub fn chacha20poly1305.open(key: Bytes, nonce: Bytes, sealed: Bytes, data: Bytes) -> Result[ unique Bytes, std@1/aead.OpenError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/aead/OpenError`](#def-std_401_2faead_2fOpenError)

<a id="def-std_401_2fchacha20poly1305_2fseal"></a>

- `pub fn chacha20poly1305.seal(key: Bytes, nonce: Bytes, plaintext: Bytes, data: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)


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

- `@noalloc fn child.kill(p: Proc, mut c: Child of p, signal: Signal) -> ()`

<a id="def-core_401_2fchild_2fread_5fstderr"></a>

- `fn child.read_stderr(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]`

<a id="def-core_401_2fchild_2fread_5fstdout"></a>

- `fn child.read_stdout(p: Proc, mut c: Child of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]`

<a id="def-core_401_2fchild_2fwait"></a>

- `@noalloc fn child.wait(p: Proc, take c: Child of p, deadline: Option[Deadline]) -> Result[ExitStatus, ProcessError]`

<a id="def-core_401_2fchild_2fwrite"></a>

- `@noalloc fn child.write(p: Proc, mut c: Child of p, data: Bytes) -> Result[(), ProcessError]`


## cidr

<a id="def-std_401_2fcidr_2fPrefix"></a>

- `pub opaque type cidr.Prefix: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fcidr_2fPrefixError"></a>

- `@serializable pub type cidr.PrefixError = InvalidPrefix | InvalidAddress | InvalidWidth | WidthOutOfRange`

<a id="def-std_401_2fcidr_2fPrefixError_23InvalidAddress"></a>

- `InvalidAddress -> cidr.PrefixError`

Variant of cidr.PrefixError.

<a id="def-std_401_2fcidr_2fPrefixError_23InvalidPrefix"></a>

- `InvalidPrefix -> cidr.PrefixError`

Variant of cidr.PrefixError.

<a id="def-std_401_2fcidr_2fPrefixError_23InvalidWidth"></a>

- `InvalidWidth -> cidr.PrefixError`

Variant of cidr.PrefixError.

<a id="def-std_401_2fcidr_2fPrefixError_23WidthOutOfRange"></a>

- `WidthOutOfRange -> cidr.PrefixError`

Variant of cidr.PrefixError.

<a id="def-std_401_2fcidr_2faddr"></a>

- `@noalloc pub fn cidr.addr(prefix: Prefix) -> IpAddr uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2fbits"></a>

- `@noalloc pub fn cidr.bits(prefix: Prefix) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2fcontains"></a>

- `@noalloc pub fn cidr.contains(prefix: Prefix, ip: IpAddr) -> bool uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`core@1/bool`](#def-core_401_2fbool), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2fformat"></a>

- `pub fn cidr.format(prefix: Prefix) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2ffrom_5faddr"></a>

- `@noalloc pub fn cidr.from_addr(ip: IpAddr, width: i64) -> Result[Prefix, PrefixError] uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix), [`std@1/cidr/PrefixError`](#def-std_401_2fcidr_2fPrefixError)

<a id="def-std_401_2fcidr_2fmasked"></a>

- `@noalloc pub fn cidr.masked(prefix: Prefix) -> Prefix uses {}`

References: [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2foverlaps"></a>

- `@noalloc pub fn cidr.overlaps(a: Prefix, b: Prefix) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix)

<a id="def-std_401_2fcidr_2fparse"></a>

- `pub fn cidr.parse(text: String) -> Result[Prefix, PrefixError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/cidr/Prefix`](#def-std_401_2fcidr_2fPrefix), [`std@1/cidr/PrefixError`](#def-std_401_2fcidr_2fPrefixError)


## cli

<a id="def-std_401_2fcli_2fFlag"></a>

- `@serializable pub type cli.Flag { name: String, short: Option[u8], kind: Kind, help: String, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/cli/Kind`](#def-std_401_2fcli_2fKind)

<a id="def-std_401_2fcli_2fKind"></a>

- `@serializable pub type cli.Kind = Switch | Value`

<a id="def-std_401_2fcli_2fKind_23Switch"></a>

- `Switch -> cli.Kind`

Variant of cli.Kind.

<a id="def-std_401_2fcli_2fKind_23Value"></a>

- `Value -> cli.Kind`

Variant of cli.Kind.

<a id="def-std_401_2fcli_2fParseError"></a>

- `@serializable pub type cli.ParseError = UnknownFlag(u64) | MissingValue(u64) | UnexpectedValue(u64) | InvalidSpec`

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

- `@serializable pub type cli.Parsed { flags: Map[String, List[String]], positional: List[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fcli_2ferror_5fmessage"></a>

- `pub fn cli.error_message(args: List[String], specs: List[Flag], error: ParseError) -> unique String uses {}`

Render an error against the original argument snapshot, without I/O or LF.
Stale or forged errors get bounded fallback text instead of invented context.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/cli/Flag`](#def-std_401_2fcli_2fFlag), [`std@1/cli/ParseError`](#def-std_401_2fcli_2fParseError)

<a id="def-std_401_2fcli_2fparse"></a>

- `pub fn cli.parse(take args: List[String], specs: List[Flag]) -> Result[Parsed, ParseError] uses {}`

Parse flags until the first positional argument or standalone --.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/cli/Flag`](#def-std_401_2fcli_2fFlag), [`std@1/cli/ParseError`](#def-std_401_2fcli_2fParseError), [`std@1/cli/Parsed`](#def-std_401_2fcli_2fParsed)

<a id="def-std_401_2fcli_2fparse_5finterspersed"></a>

- `pub fn cli.parse_interspersed(take args: List[String], specs: List[Flag]) -> Result[ Parsed, ParseError ] uses {}`

Parse flags before and after positionals until a standalone --.

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


## constant_time

<a id="def-std_401_2fconstant_5ftime_2fequal"></a>

- `pub fn constant_time.equal(a: Bytes, b: Bytes) -> bool uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)


## cookie

<a id="def-std_401_2fcookie_2fCookieError"></a>

- `@serializable pub type cookie.CookieError = Blank | MissingEquals(u64) | InvalidName(u64) | InvalidValue(u64) | InvalidPath( u64 ) | InsecureSameSiteNone`

Offsets are byte positions in the parsed header, or in the offending field
when formatting.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcookie_2fCookieError_23Blank"></a>

- `Blank -> cookie.CookieError`

Variant of cookie.CookieError.

<a id="def-std_401_2fcookie_2fCookieError_23InsecureSameSiteNone"></a>

- `InsecureSameSiteNone -> cookie.CookieError`

Variant of cookie.CookieError.

<a id="def-std_401_2fcookie_2fCookieError_23InvalidName"></a>

- `InvalidName(u64) -> cookie.CookieError`

Variant of cookie.CookieError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcookie_2fCookieError_23InvalidPath"></a>

- `InvalidPath( u64 ) -> cookie.CookieError`

Variant of cookie.CookieError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcookie_2fCookieError_23InvalidValue"></a>

- `InvalidValue(u64) -> cookie.CookieError`

Variant of cookie.CookieError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcookie_2fCookieError_23MissingEquals"></a>

- `MissingEquals(u64) -> cookie.CookieError`

Variant of cookie.CookieError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcookie_2fSameSite"></a>

- `@serializable pub type cookie.SameSite = Strict | Lax | None`

<a id="def-std_401_2fcookie_2fSameSite_23Lax"></a>

- `Lax -> cookie.SameSite`

Variant of cookie.SameSite.

<a id="def-std_401_2fcookie_2fSameSite_23None"></a>

- `None -> cookie.SameSite`

Variant of cookie.SameSite.

<a id="def-std_401_2fcookie_2fSameSite_23Strict"></a>

- `Strict -> cookie.SameSite`

Variant of cookie.SameSite.

<a id="def-std_401_2fcookie_2fSetCookie"></a>

- `@serializable pub type cookie.SetCookie { name: String, value: String, path: Option[String], max_age: Option[u64], http_only: bool, secure: bool, same_site: Option[SameSite], }`

One Set-Cookie header value: the RFC 6265 name, value, Path, Max-Age,
HttpOnly and Secure attributes plus SameSite.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/cookie/SameSite`](#def-std_401_2fcookie_2fSameSite)

<a id="def-std_401_2fcookie_2fformat"></a>

- `pub fn cookie.format(c: SetCookie) -> Result[unique String, CookieError] uses {}`

Formats a Set-Cookie header value in Go's attribute order. A value
containing a space or comma is double-quoted, as in Go. SameSite=None
requires Secure (RFC 6265bis section 4.1.2.7).

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/cookie/CookieError`](#def-std_401_2fcookie_2fCookieError), [`std@1/cookie/SetCookie`](#def-std_401_2fcookie_2fSetCookie)

<a id="def-std_401_2fcookie_2fnew"></a>

- `pub fn cookie.new(name: String, value: String) -> SetCookie uses {}`

A cookie with no attributes.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/cookie/SetCookie`](#def-std_401_2fcookie_2fSetCookie)

<a id="def-std_401_2fcookie_2fparse"></a>

- `pub fn cookie.parse(header: String) -> Result[ unique List[(unique String, unique String)], CookieError ] uses {}`

Parses a Cookie request header value into its name-value pairs, in order and
with repeated names kept, as Go ParseCookie. Pairs are separated by `;` and
trimmed; every pair needs `=`, a token name and a cookie-octet value, which
may be enclosed in double quotes (removed). Errors return no pairs.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/cookie/CookieError`](#def-std_401_2fcookie_2fCookieError)


## crc32

<a id="def-std_401_2fcrc32_2fState"></a>

- `pub opaque type crc32.State: plain + inert uses {}`

<a id="def-std_401_2fcrc32_2fdigest"></a>

- `@noalloc pub fn crc32.digest(data: Bytes) -> u32 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fcrc32_2fdigest_5fhex"></a>

- `pub fn crc32.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fcrc32_2ffinish"></a>

- `@noalloc pub fn crc32.finish(take state: State) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/crc32/State`](#def-std_401_2fcrc32_2fState)

<a id="def-std_401_2fcrc32_2fnew"></a>

- `@noalloc pub fn crc32.new() -> State uses {}`

References: [`std@1/crc32/State`](#def-std_401_2fcrc32_2fState)

<a id="def-std_401_2fcrc32_2fupdate"></a>

- `@noalloc pub fn crc32.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/crc32/State`](#def-std_401_2fcrc32_2fState)

<a id="def-std_401_2fcrc32_2fupdate_5flist"></a>

- `@noalloc pub fn crc32.update_list(mut state: State, data: List[u8], start: u64) -> () uses {}`

Extends the checksum with the bytes of a list builder from `start` on.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/crc32/State`](#def-std_401_2fcrc32_2fState)


## crc32c

<a id="def-std_401_2fcrc32c_2fState"></a>

- `pub opaque type crc32c.State: plain + inert uses {}`

<a id="def-std_401_2fcrc32c_2fdigest"></a>

- `@noalloc pub fn crc32c.digest(data: Bytes) -> u32 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fcrc32c_2fdigest_5fhex"></a>

- `pub fn crc32c.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fcrc32c_2ffinish"></a>

- `@noalloc pub fn crc32c.finish(take state: State) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/crc32c/State`](#def-std_401_2fcrc32c_2fState)

<a id="def-std_401_2fcrc32c_2fnew"></a>

- `@noalloc pub fn crc32c.new() -> State uses {}`

References: [`std@1/crc32c/State`](#def-std_401_2fcrc32c_2fState)

<a id="def-std_401_2fcrc32c_2fupdate"></a>

- `@noalloc pub fn crc32c.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/crc32c/State`](#def-std_401_2fcrc32c_2fState)


## csv

<a id="def-std_401_2fcsv_2fConfig"></a>

- `@serializable pub type csv.Config { delimiter: u8, fields_per_record: Option[u64], max_record_bytes: u64, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fcsv_2fCsvError"></a>

- `@serializable pub type csv.CsvError = Syntax(Position) | UnequalFields(Position) | RecordTooLarge | InvalidUtf8 | InvalidDelimiter`

References: [`std@1/csv/Position`](#def-std_401_2fcsv_2fPosition)

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

- `Syntax(Position) -> csv.CsvError`

Variant of csv.CsvError.

References: [`std@1/csv/Position`](#def-std_401_2fcsv_2fPosition)

<a id="def-std_401_2fcsv_2fCsvError_23UnequalFields"></a>

- `UnequalFields(Position) -> csv.CsvError`

Variant of csv.CsvError.

References: [`std@1/csv/Position`](#def-std_401_2fcsv_2fPosition)

<a id="def-std_401_2fcsv_2fDriverError"></a>

- `pub type csv.DriverError[E] = Source(E) | Callback(E) | Csv(CsvError) | Canceled( CancelCause ) | Limit | InvalidRead | InvalidConfig`

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fDriverError_23Callback"></a>

- `Callback(E) -> csv.DriverError[E]`

Variant of csv.DriverError.

<a id="def-std_401_2fcsv_2fDriverError_23Canceled"></a>

- `Canceled( CancelCause ) -> csv.DriverError[E]`

Variant of csv.DriverError.

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause)

<a id="def-std_401_2fcsv_2fDriverError_23Csv"></a>

- `Csv(CsvError) -> csv.DriverError[E]`

Variant of csv.DriverError.

References: [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fDriverError_23InvalidConfig"></a>

- `InvalidConfig -> csv.DriverError[E]`

Variant of csv.DriverError.

<a id="def-std_401_2fcsv_2fDriverError_23InvalidRead"></a>

- `InvalidRead -> csv.DriverError[E]`

Variant of csv.DriverError.

<a id="def-std_401_2fcsv_2fDriverError_23Limit"></a>

- `Limit -> csv.DriverError[E]`

Variant of csv.DriverError.

<a id="def-std_401_2fcsv_2fDriverError_23Source"></a>

- `Source(E) -> csv.DriverError[E]`

Variant of csv.DriverError.

<a id="def-std_401_2fcsv_2fPosition"></a>

- `@serializable pub type csv.Position { start_line: u64, line: u64, column: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fcsv_2fReadError"></a>

- `@serializable pub type csv.ReadError[E] = Source(E) | Csv(CsvError) | InvalidRead`

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

<a id="def-std_401_2fcsv_2fSliceReader"></a>

- `pub opaque type csv.SliceReader: copyable + inert + noalloc_drop uses {}`

A borrowed input cursor with reusable field spans and one escape buffer per record.

<a id="def-std_401_2fcsv_2fStreamConfig"></a>

- `pub type csv.StreamConfig { csv: Config, refill_bytes: u64, max_input_bytes: u64, max_records: u64, max_outputs: u64, }`

Limits for an owning scoped fold. Zero raw/input budgets forbid further work;
zero output budget succeeds without pulling. Refill and record sizes are positive.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig)

<a id="def-std_401_2fcsv_2fWriteConfig"></a>

- `@serializable pub type csv.WriteConfig { delimiter: u8, preserve_single_empty: bool, }`

Writer policy; zero fields still produce a blank line.

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fcsv_2fdefault_5fconfig"></a>

- `pub fn csv.default_config() -> Config uses {}`

References: [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig)

<a id="def-std_401_2fcsv_2fdefault_5fwrite_5fconfig"></a>

- `pub fn csv.default_write_config() -> WriteConfig uses {}`

Comma delimiter and the legacy Go-compatible empty-field representation.

References: [`std@1/csv/WriteConfig`](#def-std_401_2fcsv_2fWriteConfig)

<a id="def-std_401_2fcsv_2ffeed"></a>

- `pub fn csv.feed(mut r: Reader, chunk: Bytes, eof: bool) -> Result[ List[List[String]], CsvError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2ffield_5fcount"></a>

- `pub fn csv.field_count(r: SliceReader) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/csv/SliceReader`](#def-std_401_2fcsv_2fSliceReader)

<a id="def-std_401_2fcsv_2ffold_5fscoped"></a>

- `pub fn csv.fold_scoped[S, A, E]( scope: Scope, take source: S, config: StreamConfig, take initial: A, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], accept: fn(List[String]) -> Result[bool, E], step: fn(take A, take List[String]) -> Result[A, E] ) -> Result[A, DriverError[E]] uses {scope, source, initial, refill, accept, step}`

Consume an owned source with bounded parsing and explicit cooperative checks.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Scope`](#def-core_401_2fScope), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/DriverError`](#def-std_401_2fcsv_2fDriverError), [`std@1/csv/StreamConfig`](#def-std_401_2fcsv_2fStreamConfig)

<a id="def-std_401_2fcsv_2fget_5ffield"></a>

- `pub fn csv.get_field(r: SliceReader, index: u64) -> Option[String] in r uses {}`

Borrow a decoded UTF-8 field from the current record.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/csv/SliceReader`](#def-std_401_2fcsv_2fSliceReader)

<a id="def-std_401_2fcsv_2fget_5ffield_5fbytes"></a>

- `@noalloc pub fn csv.get_field_bytes(r: SliceReader, index: u64) -> Option[Bytes] in r uses {}`

Borrow decoded field bytes without constructing String boundary positions.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/csv/SliceReader`](#def-std_401_2fcsv_2fSliceReader)

<a id="def-std_401_2fcsv_2fnext_5frecord"></a>

- `pub fn csv.next_record(mut r: SliceReader) -> Result[bool, CsvError] uses {}`

Advance one record; false means EOF. Errors are terminal. Release field
views before advancing again; the field-span storage is reused.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/SliceReader`](#def-std_401_2fcsv_2fSliceReader)

<a id="def-std_401_2fcsv_2fparse"></a>

- `pub fn csv.parse(take text: String, config: Config) -> Result[List[List[String]], CsvError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fparse_5fowned"></a>

- `pub fn csv.parse_owned(take text: String, config: Config) -> Result[ unique List[List[String]], CsvError ] uses {}`

Construct a deeply owned table, independent of the consumed input. Borrow
for validation, then shared.freeze(take rows) without cloning the table.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fread_5frecord"></a>

- `pub fn csv.read_record[S, E]( mut r: Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E] ) -> Result[Option[List[String]], ReadError[E]]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/ReadError`](#def-std_401_2fcsv_2fReadError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2freader"></a>

- `pub fn csv.reader(config: Config) -> Result[Reader, CsvError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/Reader`](#def-std_401_2fcsv_2fReader)

<a id="def-std_401_2fcsv_2fslice_5freader"></a>

- `pub fn csv.slice_reader(text: String, config: Config) -> Result[SliceReader, CsvError] in text uses {}`

Borrow UTF-8 input. Each next_record replaces the current record's views.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/csv/Config`](#def-std_401_2fcsv_2fConfig), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/SliceReader`](#def-std_401_2fcsv_2fSliceReader)

<a id="def-std_401_2fcsv_2fwrite_5frecord"></a>

- `pub fn csv.write_record(fields: List[String], delimiter: u8, mut out: List[u8]) -> Result[ (), CsvError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError)

<a id="def-std_401_2fcsv_2fwrite_5frecord_5fwith"></a>

- `pub fn csv.write_record_with(fields: List[String], config: WriteConfig, mut out: List[u8]) -> Result[ (), CsvError ] uses {}`

Append a record; optionally quote a sole empty field. Zero fields stay LF.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/CsvError`](#def-std_401_2fcsv_2fCsvError), [`std@1/csv/WriteConfig`](#def-std_401_2fcsv_2fWriteConfig)


## date

<a id="def-std_401_2fdate_2fadd_5fdays"></a>

- `@noalloc pub fn date.add_days(value: time.Date, days: i64) -> Result[time.Date, time.TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError)

<a id="def-std_401_2fdate_2fadd_5fmonths"></a>

- `@noalloc pub fn date.add_months(value: time.Date, months: i64, overflow: time.Overflow) -> Result[ time.Date, time.CalendarError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/CalendarError`](#def-std_401_2ftime_2fCalendarError), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/Overflow`](#def-std_401_2ftime_2fOverflow)

<a id="def-std_401_2fdate_2fadd_5fyears"></a>

- `@noalloc pub fn date.add_years(value: time.Date, years: i64, overflow: time.Overflow) -> Result[ time.Date, time.CalendarError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/CalendarError`](#def-std_401_2ftime_2fCalendarError), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/Overflow`](#def-std_401_2ftime_2fOverflow)

<a id="def-std_401_2fdate_2fcompare"></a>

- `@noalloc pub fn date.compare(a: time.Date, b: time.Date) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fday"></a>

- `@noalloc pub fn date.day(value: time.Date) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fdays_5fbetween"></a>

- `@noalloc pub fn date.days_between(start: time.Date, end: time.Date) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn date.format_into(value: time.Date, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2ffrom_5fepoch_5fdays"></a>

- `@noalloc pub fn date.from_epoch_days(days: i64) -> Option[time.Date] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2ffrom_5fiso_5fweek"></a>

- `@noalloc pub fn date.from_iso_week(y: i32, week: u8, day_of_week: time.Weekday) -> Option[time.Date] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/Weekday`](#def-std_401_2ftime_2fWeekday)

<a id="def-std_401_2fdate_2ffrom_5fjson"></a>

- `pub fn date.from_json(value: json.Value) -> Result[time.Date, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2ffrom_5fjson_5freader"></a>

- `pub fn date.from_json_reader(mut cursor: json.Reader) -> Result[time.Date, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2ffrom_5fordinal"></a>

- `@noalloc pub fn date.from_ordinal(y: i32, day_of_year: u16) -> Option[time.Date] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u16`](#def-core_401_2fu16), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fiso_5fweek"></a>

- `@noalloc pub fn date.iso_week(value: time.Date) -> time.IsoWeek uses {}`

References: [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/IsoWeek`](#def-std_401_2ftime_2fIsoWeek)

<a id="def-std_401_2fdate_2fjson_5finto"></a>

- `@preserves_unique(out) pub fn date.json_into(value: time.Date, mut out: List[u8], depth: u64) -> Result[ (), json.JsonError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fmonth"></a>

- `@noalloc pub fn date.month(value: time.Date) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fnew"></a>

- `@noalloc pub fn date.new(y: i32, m: u8, d: u8) -> Option[time.Date] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fordinal"></a>

- `@noalloc pub fn date.ordinal(value: time.Date) -> u16 uses {}`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fparse"></a>

- `@noalloc pub fn date.parse(text: String) -> Result[time.Date, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fto_5fepoch_5fdays"></a>

- `@noalloc pub fn date.to_epoch_days(value: time.Date) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fto_5fjson"></a>

- `pub fn date.to_json(value: time.Date) -> json.Value uses {}`

The derived JSON form: a date string, never an object of calendar fields.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fto_5fstring"></a>

- `pub fn date.to_string(value: time.Date) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2fdate_2fweekday"></a>

- `@noalloc pub fn date.weekday(value: time.Date) -> time.Weekday uses {}`

References: [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/Weekday`](#def-std_401_2ftime_2fWeekday)

<a id="def-std_401_2fdate_2fyear"></a>

- `@noalloc pub fn date.year(value: time.Date) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)


## date_time

<a id="def-std_401_2fdate_5ftime_2fadd"></a>

- `@noalloc pub fn date_time.add(value: time.DateTime, interval: time.Duration) -> Result[ time.DateTime, time.TimeError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError)

<a id="def-std_401_2fdate_5ftime_2fbetween"></a>

- `@noalloc pub fn date_time.between(start: time.DateTime, end: time.DateTime) -> time.Duration uses {}`

References: [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fdate_5ftime_2fcompare"></a>

- `@noalloc pub fn date_time.compare(a: time.DateTime, b: time.DateTime) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2fdate_5ftime_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn date_time.format_into(value: time.DateTime, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2fdate_5ftime_2ffrom_5ftimestamp"></a>

- `@noalloc pub fn date_time.from_timestamp(value: time.Timestamp, offset: i32) -> Result[ time.DateTime, time.TimeError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2fdate_5ftime_2fparse"></a>

- `@noalloc pub fn date_time.parse(text: String) -> Result[time.DateTime, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2fdate_5ftime_2fto_5fstring"></a>

- `pub fn date_time.to_string(value: time.DateTime) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2fdate_5ftime_2fto_5ftimestamp"></a>

- `@noalloc pub fn date_time.to_timestamp(value: time.DateTime, offset: i32) -> Result[ time.Timestamp, time.TimeError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)


## decimal

<a id="def-std_401_2fdecimal_2fDecimal"></a>

- `@format(format_into) pub opaque type decimal.Decimal: copyable + data + key + inert + noalloc_drop uses {}`

The exact value coefficient * 10^-scale. `==` compares representations, so 1.0 and
1.00 differ; `compare` compares values.

References: [`std@1/decimal/format_into`](#def-std_401_2fdecimal_2fformat_5finto)

<a id="def-std_401_2fdecimal_2fRoundingMode"></a>

- `@serializable pub type decimal.RoundingMode = HalfEven | HalfUp | Down | Up | Floor | Ceiling`

How a result that is not exact at its scale is rounded. `HalfEven` and `HalfUp`
round to the nearest value, breaking ties to an even last digit and away from zero
respectively; `Down` rounds toward zero, `Up` away from zero, `Floor` toward
negative infinity and `Ceiling` toward positive infinity.

<a id="def-std_401_2fdecimal_2fRoundingMode_23Ceiling"></a>

- `Ceiling -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fRoundingMode_23Down"></a>

- `Down -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fRoundingMode_23Floor"></a>

- `Floor -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fRoundingMode_23HalfEven"></a>

- `HalfEven -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fRoundingMode_23HalfUp"></a>

- `HalfUp -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fRoundingMode_23Up"></a>

- `Up -> decimal.RoundingMode`

Variant of decimal.RoundingMode.

<a id="def-std_401_2fdecimal_2fabs"></a>

- `pub fn decimal.abs(value: Decimal) -> Decimal uses {}`

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fadd"></a>

- `pub fn decimal.add(a: Decimal, b: Decimal) -> Decimal uses {}`

The exact sum, at the larger of the two scales.

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fcoefficient"></a>

- `pub fn decimal.coefficient(value: Decimal) -> bigint.BigInt uses {}`

References: [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fcompare"></a>

- `pub fn decimal.compare(a: Decimal, b: Decimal) -> i32 uses {}`

-1, 0 or 1 as `a` is less than, equal to or greater than `b`, by value.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fdiv"></a>

- `pub fn decimal.div(a: Decimal, b: Decimal, places: i32, mode: RoundingMode) -> Decimal uses {}`

`a / b` rounded to `places` fractional digits by `mode`. A zero divisor fails SEM-1.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal), [`std@1/decimal/RoundingMode`](#def-std_401_2fdecimal_2fRoundingMode)

<a id="def-std_401_2fdecimal_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn decimal.format_into(value: Decimal, mut out: List[u8]) -> () uses {}`

Appends the text `to_string` returns.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fis_5fzero"></a>

- `pub fn decimal.is_zero(value: Decimal) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fmul"></a>

- `pub fn decimal.mul(a: Decimal, b: Decimal) -> Decimal uses {}`

The exact product, at the sum of the scales; a sum outside i32 fails SEM-1.

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fneg"></a>

- `pub fn decimal.neg(value: Decimal) -> Decimal uses {}`

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fnew"></a>

- `pub fn decimal.new(unscaled: bigint.BigInt, places: i32) -> Decimal uses {}`

The decimal unscaled * 10^-places.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/bigint/BigInt`](#def-std_401_2fbigint_2fBigInt), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fnormalize"></a>

- `pub fn decimal.normalize(value: Decimal) -> Decimal uses {}`

The same value without trailing fractional zeros: zero has scale 0, and the scale
does not fall below zero unless it already was negative.

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fparse"></a>

- `pub fn decimal.parse(text: String) -> Result[Decimal, scalar.ParseError] uses {}`

Parses `-?[0-9]+(\.[0-9]+)?`, keeping every written fractional digit as the scale:
"1.50" has coefficient 150 and scale 2, and "-0.0" is zero at scale 1. A plus,
exponent, whitespace or missing digit is `Invalid`; a scale beyond i32 is `OutOfRange`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2fdecimal_2frescale"></a>

- `pub fn decimal.rescale(value: Decimal, places: i32, mode: RoundingMode) -> Decimal uses {}`

`value` at `places` fractional digits: exact when the scale grows, rounded by `mode`
when it shrinks. A negative count rounds to a multiple of a power of ten.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal), [`std@1/decimal/RoundingMode`](#def-std_401_2fdecimal_2fRoundingMode)

<a id="def-std_401_2fdecimal_2fscale"></a>

- `pub fn decimal.scale(value: Decimal) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fsignum"></a>

- `pub fn decimal.signum(value: Decimal) -> i32 uses {}`

-1, 0 or 1 by the sign of `value`.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fsub"></a>

- `pub fn decimal.sub(a: Decimal, b: Decimal) -> Decimal uses {}`

The exact difference, at the larger of the two scales.

References: [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)

<a id="def-std_401_2fdecimal_2fto_5fstring"></a>

- `pub fn decimal.to_string(value: Decimal) -> unique String uses {}`

Plain decimal text without an exponent: a minus for negative values, the integer
digits (at least one), and exactly `scale` fractional digits after a point when the
scale is positive. A negative scale appends that many zeros to a nonzero coefficient.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/decimal/Decimal`](#def-std_401_2fdecimal_2fDecimal)


## deque

<a id="def-std_401_2fdeque_2fDeque"></a>

- `pub opaque type deque.Deque[T]: () uses {T}`

<a id="def-std_401_2fdeque_2fback"></a>

- `pub fn deque.back[T: copyable](q: Deque[T]) -> Option[T] in q uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fempty"></a>

- `pub fn deque.empty[T]() -> unique Deque[T] uses {}`

References: [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2ffront"></a>

- `pub fn deque.front[T: copyable](q: Deque[T]) -> Option[T] in q uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

<a id="def-std_401_2fdeque_2fget"></a>

- `pub fn deque.get[T: copyable](q: Deque[T], index: u64) -> Option[T] in q uses {}`

Copies the item at a logical index, without changing either endpoint.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/deque/Deque`](#def-std_401_2fdeque_2fDeque)

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


## derive

<a id="def-std_401_2fderive_2fequal"></a>

- `pub fn derive.equal[T](s: TypeShape[T]) -> Result[Body[fn(T, T) -> bool uses {}], String] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fderive_2fformat"></a>

- `pub fn derive.format[T](s: TypeShape[T]) -> Result[ Body[fn(T, mut List[u8]) -> () uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fderive_2ffrom_5fjson"></a>

- `pub fn derive.from_json[T](s: TypeShape[T]) -> Result[ Body[fn(json.Value) -> Result[T, String] uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2ffrom_5fjson_5fbytes"></a>

- `pub fn derive.from_json_bytes[T](s: TypeShape[T]) -> Result[ Body[fn(Bytes) -> Result[T, String] uses {}], String ] uses {}`

Derive `type_from_json_bytes(Bytes)`: one complete UTF-8 document. Declare
type_from_json_reader as well, so recursive/nested types share the reader.
The reader decoder constructs owned fields; move its result without detaching
the completed graph. Token views remain bounded by the Reader.

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fderive_2ffrom_5fjson_5freader"></a>

- `pub fn derive.from_json_reader[T](s: TypeShape[T]) -> Result[ Body[fn(mut json.Reader) -> Result[T, String] uses {}], String ] uses {}`

Derive `type_from_json_reader(mut json.Reader)`: consumes one typed value
within the caller's reader. Nested nominal types use their matching entry.

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fhash"></a>

- `pub fn derive.hash[T](s: TypeShape[T]) -> Result[ Body[fn(T, mut siphash.State) -> () uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fderive_2fjson_5fbool"></a>

- `pub fn derive.json_bool(value: json.Value) -> Result[bool, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5ffloat"></a>

- `pub fn derive.json_float(take spelling: String) -> json.Value uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5ffloat_5ftext"></a>

- `pub fn derive.json_float_text(value: json.Value) -> Result[unique String, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5finto"></a>

- `pub fn derive.json_into[T](s: TypeShape[T]) -> Result[ Body[fn(T, mut List[u8], u64) -> Result[(), json.JsonError] uses {}], String ] uses {}`

Write the same compact JSON as json_value followed by stringify_into,
directly from a record or choice. Nested nominal types supply their own
`<snake_name>_json_into` writer. Pass depth 0 at the root.

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fderive_2fjson_5finvalid_5ftype"></a>

- `pub fn derive.json_invalid_type(value: json.Value, expected: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5flength"></a>

- `pub fn derive.json_length(length: u64, count: u64, expected: String) -> Result[(), String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fderive_2fjson_5fnon_5ffinite"></a>

- `pub fn derive.json_non_finite(spelling: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fderive_2fjson_5fnumber"></a>

- `pub fn derive.json_number(value: json.Value) -> Result[unique String, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5ftext"></a>

- `pub fn derive.json_text(value: json.Value) -> Result[unique String, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5funexpected"></a>

- `pub fn derive.json_unexpected(value: json.Value) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fjson_5fvalue"></a>

- `pub fn derive.json_value[T](s: TypeShape[T]) -> Result[ Body[fn(T) -> json.Value uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fderive_2fparsed"></a>

- `pub fn derive.parsed[N, E](take value: Result[N, E], ty: String) -> Result[N, String] uses {value}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fderive_2fread_5fbool"></a>

- `pub fn derive.read_bool(mut cursor: json.Reader) -> Result[bool, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fread_5fjson"></a>

- `pub fn derive.read_json[T](take value: Result[T, json.JsonError]) -> Result[T, String] uses {value}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fderive_2fread_5fjson_5ff32"></a>

- `pub fn derive.read_json_f32(mut cursor: json.Reader) -> Result[f32, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f32`](#def-core_401_2ff32), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fread_5fjson_5ff64"></a>

- `pub fn derive.read_json_f64(mut cursor: json.Reader) -> Result[f64, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fread_5fnull"></a>

- `pub fn derive.read_null(mut cursor: json.Reader) -> Result[(), String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fread_5fstring"></a>

- `pub fn derive.read_string(mut cursor: json.Reader) -> Result[unique String, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2freader_5finvalid_5ftype"></a>

- `pub fn derive.reader_invalid_type(mut cursor: json.Reader, expected: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fderive_2fremember_5funknown_5fkey"></a>

- `pub fn derive.remember_unknown_key(mut keys: Option[Set[String]], take key: String) -> bool uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Set`](#def-core_401_2fSet), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fderive_2fschema_5fdecode"></a>

- `pub fn derive.schema_decode[T](s: TypeShape[T], p: schema.Policy) -> Result[ Body[fn(json.Value, mut schema.CheckState) -> Result[T, schema.SchemaError] uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/Policy`](#def-std_401_2fschema_2fPolicy), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fderive_2fschema_5fencode"></a>

- `pub fn derive.schema_encode[T](s: TypeShape[T], p: schema.Policy) -> Result[ Body[fn(T, mut schema.CheckState) -> Result[json.Value, schema.SchemaError] uses {}], String ] uses {}`

References: [`core@1/Body`](#def-core_401_2fBody), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/Policy`](#def-std_401_2fschema_2fPolicy), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fderive_2funknown_5fvariant"></a>

- `pub fn derive.unknown_variant(tag: String, expected: String) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fderive_2fwrite_5fjson_5ff32"></a>

- `@preserves_unique(out) pub fn derive.write_json_f32(value: f32, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fderive_2fwrite_5fjson_5ff64"></a>

- `@preserves_unique(out) pub fn derive.write_json_f64(value: f64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u8`](#def-core_401_2fu8)


## diff

<a id="def-std_401_2fdiff_2fAlgorithm"></a>

- `pub type diff.Algorithm = Myers | MyersMinimal | Histogram | Patience`

`Myers()` uses heuristic pruning and search cutoffs, as git does;
`MyersMinimal()` always finds a minimal script.

<a id="def-std_401_2fdiff_2fAlgorithm_23Histogram"></a>

- `Histogram -> diff.Algorithm`

Variant of diff.Algorithm.

<a id="def-std_401_2fdiff_2fAlgorithm_23Myers"></a>

- `Myers -> diff.Algorithm`

Variant of diff.Algorithm.

<a id="def-std_401_2fdiff_2fAlgorithm_23MyersMinimal"></a>

- `MyersMinimal -> diff.Algorithm`

Variant of diff.Algorithm.

<a id="def-std_401_2fdiff_2fAlgorithm_23Patience"></a>

- `Patience -> diff.Algorithm`

Variant of diff.Algorithm.

<a id="def-std_401_2fdiff_2fEdit"></a>

- `pub type diff.Edit = Keep(u64, u64, u64) | Delete(u64, u64, u64) | Insert(u64, u64, u64)`

One run of an edit script: `Keep(old, new, count)` keeps `count` lines from
`old[old]` and `new[new]`; `Delete(old, new, count)` drops `old[old..old +
count]` before `new[new]`; `Insert(old, new, count)` adds `new[new..new +
count]` before `old[old]`. Runs cover both sequences in order, and a
replacement is a Delete followed by an Insert.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fEdit_23Delete"></a>

- `Delete(u64, u64, u64) -> diff.Edit`

Variant of diff.Edit.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fEdit_23Insert"></a>

- `Insert(u64, u64, u64) -> diff.Edit`

Variant of diff.Edit.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fEdit_23Keep"></a>

- `Keep(u64, u64, u64) -> diff.Edit`

Variant of diff.Edit.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fPatchError"></a>

- `pub type diff.PatchError = Malformed(u64)`

`Malformed` carries the one-based line number of the first patch line that
is not part of a well-formed unified diff.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fPatchError_23Malformed"></a>

- `Malformed(u64) -> diff.PatchError`

Variant of diff.PatchError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fPatched"></a>

- `pub type diff.Patched { text: String, rejected: List[u64], }`

`apply`'s result: the patched text, and the zero-based indexes of the hunks
that did not apply, in patch order.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdiff_2fapply"></a>

- `pub fn diff.apply(source: String, patch: String) -> Result[Patched, PatchError] uses {}`

Applies a unified diff of one file to `source`, as GNU `patch --fuzz=0
--forward` does: lines before the first `@@` are skipped; each hunk's old
lines (context and deletions) must match exactly, first where its header
places them shifted by the offset the previous applied hunk needed, then at
the nearest place not before the previous applied hunk, later before
earlier at each distance. A hunk with fewer context lines before its
changes than after them, starting at the first line, applies only at the
start of the file; one with fewer after than before only at its end. A hunk
that matches nowhere is rejected and the rest still apply.
`\ No newline at end of file` drops the preceding line's LF.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/diff/PatchError`](#def-std_401_2fdiff_2fPatchError), [`std@1/diff/Patched`](#def-std_401_2fdiff_2fPatched)

<a id="def-std_401_2fdiff_2fedits"></a>

- `pub fn diff.edits(old: List[String], new: List[String], algorithm: Algorithm) -> unique List[ Edit ] uses {}`

The edit script turning `old` into `new`, as runs of kept, deleted and
inserted lines (`Edit`). Lines compare whole, terminators included; the
common head and tail are kept first. `MyersMinimal()` finds a minimal script
by Myers' algorithm after dropping only lines the other side lacks, in
O((N + M) D) time and O(N + M) memory for N and M lines and D changed lines.
`Myers()` is heuristic: it may also drop frequent shared lines surrounded
by absent lines, and gives up minimality to bound the search, as git does:
past a cost of 256 a split may stop at a run of 20 equal lines, and past the
square root of N + M (at least 256) it stops at the furthest point reached.
`Histogram()` splits on the longest
common run of the rarest lines, falling back to `Myers()` where every shared
line occurs more than 63 times. `Patience()` anchors on lines that occur
once on each side, falling back to `Myers()` between anchors with none.
Then each change is slid down past equal lines, and back up to line up with
a change on the other side, the new side first.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/diff/Algorithm`](#def-std_401_2fdiff_2fAlgorithm), [`std@1/diff/Edit`](#def-std_401_2fdiff_2fEdit)

<a id="def-std_401_2fdiff_2fsplit_5flines"></a>

- `pub fn diff.split_lines(source: String) -> unique List[String] uses {}`

The lines of `text`, each with its LF; the last line lacks one when `text`
does not end in LF. Empty text has no lines.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fdiff_2funified"></a>

- `pub fn diff.unified( old_name: String, old: String, new_name: String, new: String, algorithm: Algorithm, context: u64 ) -> unique String uses {}`

A unified diff of `old` and `new`, as `git diff -U<context>
--no-indent-heuristic` prints one: `--- old_name` and `+++ new_name`
lines, then hunks of `@@ -start,count +start,count @@` (a count of one is
omitted, and an empty range names the line before it) with `context` lines
of context, merging changes at most 2 * context lines apart. A last line
without LF is followed by `\ No newline at end of file`. Equal texts give
the empty string. Names are written as given.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/diff/Algorithm`](#def-std_401_2fdiff_2fAlgorithm)


## digest

<a id="def-std_401_2fdigest_2fDigest"></a>

- `pub signature digest.Digest { type State: plain + inert uses {} size bytes size block fn new() -> State uses {} fn update(mut state: State, data: Bytes) -> () uses {} fn finish(take state: State) -> Inline[u8, bytes] uses {} }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)


## dir

<a id="def-core_401_2fdir_2fnarrow"></a>

- `fn dir.narrow(d: Dir, p: Path) -> Result[Dir, FileError]`

<a id="def-core_401_2fdir_2fread_5fonly"></a>

- `fn dir.read_only(d: Dir) -> Result[Dir, FileError]`

<a id="def-core_401_2fdir_2fremove_5fall"></a>

- `fn dir.remove_all(d: Dir, p: Path) -> Result[(), FileError]`


## dns_message

<a id="def-std_401_2fdns_5fmessage_2fCodecError"></a>

- `pub type dns_message.CodecError { kind: ErrorKind, offset: u64, }`

Decode offsets are absolute packet positions: offending control byte,
first missing byte, or first unconsumed byte. Limit errors point to header
count/packet end or the name byte exhausting work. Encode offsets identify
the prospective uncompressed field start; InvalidLimits uses zero.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/dns_message/ErrorKind`](#def-std_401_2fdns_5fmessage_2fErrorKind)

<a id="def-std_401_2fdns_5fmessage_2fErrorKind"></a>

- `pub type dns_message.ErrorKind = InvalidLimits | PacketLimit | RecordLimit | WorkLimit | Truncated | ReservedLabel | PointerLimit | InvalidPointer | NameTooLong | InvalidLabel | InvalidRdata | UnknownEncoding | Trailing`

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23InvalidLabel"></a>

- `InvalidLabel -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23InvalidLimits"></a>

- `InvalidLimits -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23InvalidPointer"></a>

- `InvalidPointer -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23InvalidRdata"></a>

- `InvalidRdata -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23NameTooLong"></a>

- `NameTooLong -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23PacketLimit"></a>

- `PacketLimit -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23PointerLimit"></a>

- `PointerLimit -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23RecordLimit"></a>

- `RecordLimit -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23ReservedLabel"></a>

- `ReservedLabel -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23Trailing"></a>

- `Trailing -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23Truncated"></a>

- `Truncated -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23UnknownEncoding"></a>

- `UnknownEncoding -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fErrorKind_23WorkLimit"></a>

- `WorkLimit -> dns_message.ErrorKind`

Variant of dns_message.ErrorKind.

<a id="def-std_401_2fdns_5fmessage_2fHeader"></a>

- `pub type dns_message.Header { id: u16, flags: u16, }`

All 16 flag bits are retained, including the reserved bit. Opcode occupies
bits 11..14, response code bits 0..3; no opcode or class policy is applied.

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fdns_5fmessage_2fLimits"></a>

- `pub type dns_message.Limits { packet_bytes: u64, records: u64, name_work: u64, }`

Caller may lower these hard ceilings: 65535 packet bytes, 4096 total
questions + records, 1048576 expanded-name work units. Each visited name
control byte/pointer second byte and copied label byte consumes one unit.
A name follows at most 128 pointers, including forward pointers.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fdns_5fmessage_2fMessage"></a>

- `pub type dns_message.Message { header: Header, questions: List[Question], answers: List[Record], authorities: List[Record], additionals: List[Record], }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/dns_message/Header`](#def-std_401_2fdns_5fmessage_2fHeader), [`std@1/dns_message/Question`](#def-std_401_2fdns_5fmessage_2fQuestion), [`std@1/dns_message/Record`](#def-std_401_2fdns_5fmessage_2fRecord)

<a id="def-std_401_2fdns_5fmessage_2fMx"></a>

- `pub type dns_message.Mx { preference: u16, exchange: Name, }`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fName"></a>

- `pub type dns_message.Name { labels: List[Bytes], }`

Root is an empty list. Labels contain arbitrary wire octets, never UTF-8.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2fdns_5fmessage_2fQuestion"></a>

- `pub type dns_message.Question { name: Name, qtype: u16, class: u16, }`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fRData"></a>

- `pub type dns_message.RData = A(Bytes) | AAAA(Bytes) | CNAME(Name) | NS(Name) | PTR(Name) | MX(Mx) | TXT( List[Bytes] ) | SOA(Soa) | SRV(Srv) | Unknown(kind: u16, raw: Bytes)`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u16`](#def-core_401_2fu16), [`std@1/dns_message/Mx`](#def-std_401_2fdns_5fmessage_2fMx), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName), [`std@1/dns_message/Soa`](#def-std_401_2fdns_5fmessage_2fSoa), [`std@1/dns_message/Srv`](#def-std_401_2fdns_5fmessage_2fSrv)

<a id="def-std_401_2fdns_5fmessage_2fRData_23A"></a>

- `A(Bytes) -> dns_message.RData`

Variant of dns_message.RData.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fdns_5fmessage_2fRData_23AAAA"></a>

- `AAAA(Bytes) -> dns_message.RData`

Variant of dns_message.RData.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fdns_5fmessage_2fRData_23CNAME"></a>

- `CNAME(Name) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fRData_23MX"></a>

- `MX(Mx) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Mx`](#def-std_401_2fdns_5fmessage_2fMx)

<a id="def-std_401_2fdns_5fmessage_2fRData_23NS"></a>

- `NS(Name) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fRData_23PTR"></a>

- `PTR(Name) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fRData_23SOA"></a>

- `SOA(Soa) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Soa`](#def-std_401_2fdns_5fmessage_2fSoa)

<a id="def-std_401_2fdns_5fmessage_2fRData_23SRV"></a>

- `SRV(Srv) -> dns_message.RData`

Variant of dns_message.RData.

References: [`std@1/dns_message/Srv`](#def-std_401_2fdns_5fmessage_2fSrv)

<a id="def-std_401_2fdns_5fmessage_2fRData_23TXT"></a>

- `TXT( List[Bytes] ) -> dns_message.RData`

Variant of dns_message.RData.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2fdns_5fmessage_2fRData_23Unknown"></a>

- `Unknown(kind: u16, raw: Bytes) -> dns_message.RData`

Variant of dns_message.RData.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fdns_5fmessage_2fRecord"></a>

- `pub type dns_message.Record { name: Name, class: u16, ttl: u32, data: RData, }`

References: [`core@1/u16`](#def-core_401_2fu16), [`core@1/u32`](#def-core_401_2fu32), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName), [`std@1/dns_message/RData`](#def-std_401_2fdns_5fmessage_2fRData)

<a id="def-std_401_2fdns_5fmessage_2fSoa"></a>

- `pub type dns_message.Soa { primary: Name, mailbox: Name, serial: u32, refresh: u32, retry: u32, expire: u32, minimum: u32, }`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fSrv"></a>

- `pub type dns_message.Srv { priority: u16, weight: u16, port: u16, target: Name, }`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fdecode"></a>

- `pub fn dns_message.decode(packet: Bytes, limits: Limits) -> Result[Message, CodecError] uses {}`

Parse exactly one owned DNS packet. No transport framing, I/O or authority.
Unknown RDATA is opaque inspection data; it may contain packet-relative
pointers. We never infer compression by scanning arbitrary opaque bytes.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/dns_message/CodecError`](#def-std_401_2fdns_5fmessage_2fCodecError), [`std@1/dns_message/Limits`](#def-std_401_2fdns_5fmessage_2fLimits), [`std@1/dns_message/Message`](#def-std_401_2fdns_5fmessage_2fMessage)

<a id="def-std_401_2fdns_5fmessage_2fdefault_5flimits"></a>

- `pub fn dns_message.default_limits() -> Limits uses {}`

References: [`std@1/dns_message/Limits`](#def-std_401_2fdns_5fmessage_2fLimits)

<a id="def-std_401_2fdns_5fmessage_2fdisplay_5fname"></a>

- `pub fn dns_message.display_name(name: Name) -> Result[String, CodecError] uses {}`

ASCII absolute display name. Dot, backslash, whitespace and nonprintable
octets use a backslash and three decimal digits, so labels stay unambiguous.
This is display only; no dotted-name parser or IDNA policy is provided.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/dns_message/CodecError`](#def-std_401_2fdns_5fmessage_2fCodecError), [`std@1/dns_message/Name`](#def-std_401_2fdns_5fmessage_2fName)

<a id="def-std_401_2fdns_5fmessage_2fencode"></a>

- `pub fn dns_message.encode(message: Message, limits: Limits) -> Result[Bytes, CodecError] uses {}`

Preflight validates all counts, labels, typed lengths and the complete packet
size before allocating the output. Counts and RDLENGTH are derived, never
truncated. Every Unknown record is rejected, even if its bytes look plain:
reconstructing a packet cannot safely relocate unknown compression pointers.
Keeping the original Bytes for whole-packet passthrough is a separate action.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/dns_message/CodecError`](#def-std_401_2fdns_5fmessage_2fCodecError), [`std@1/dns_message/Limits`](#def-std_401_2fdns_5fmessage_2fLimits), [`std@1/dns_message/Message`](#def-std_401_2fdns_5fmessage_2fMessage)


## duration

<a id="def-std_401_2fduration_2ffrom_5fparts"></a>

- `@noalloc pub fn duration.from_parts(seconds: i64, nanoseconds: i32) -> time.Duration uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`core@1/i64`](#def-core_401_2fi64), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fduration_2fparse"></a>

- `@noalloc pub fn duration.parse(text: String) -> Result[time.Duration, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fduration_2fto_5fstring"></a>

- `pub fn duration.to_string(value: time.Duration) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)


## ed25519

<a id="def-std_401_2fed25519_2fPrivateKey"></a>

- `pub type ed25519.PrivateKey { seed: Bytes, }`

A 32-byte Ed25519 seed; the public key is recomputed when signing.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fed25519_2fPublicKey"></a>

- `pub type ed25519.PublicKey { bytes: Bytes, }`

An encoded 32-byte Ed25519 public key. Verification rejects malformed bytes.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fed25519_2fpublic_5fkey"></a>

- `pub fn ed25519.public_key(seed: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fed25519_2fsign"></a>

- `pub fn ed25519.sign(seed: Bytes, message: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fed25519_2fverify"></a>

- `pub fn ed25519.verify(public: Bytes, message: Bytes, signature: Bytes) -> bool uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)


## edit_distance

<a id="def-std_401_2fedit_5fdistance_2fError"></a>

- `pub type edit_distance.Error = ScalarLimit | CellLimit`

Resource refusal, checked in this order: total input scalars, then DP cells.

<a id="def-std_401_2fedit_5fdistance_2fError_23CellLimit"></a>

- `CellLimit -> edit_distance.Error`

Variant of edit_distance.Error.

<a id="def-std_401_2fedit_5fdistance_2fError_23ScalarLimit"></a>

- `ScalarLimit -> edit_distance.Error`

Variant of edit_distance.Error.

<a id="def-std_401_2fedit_5fdistance_2flevenshtein"></a>

- `pub fn edit_distance.levenshtein(a: String, b: String, max_cells: u64, max_scalars: u64) -> Result[ u64, Error ] uses {}`

Exact unit-cost Levenshtein distance over Unicode scalar values.
No normalization, grapheme segmentation or transposition is performed.
`max_scalars` bounds the combined decoded input lengths and scalar buffers.
`max_cells` bounds only their product, not decoding or all CPU work.
ScalarLimit precedes CellLimit. Both are checked before allocating buffers
or the DP row. Empty inputs still obey max_scalars and use zero DP cells.
Storage: O(m+n) scalars and O(min(m,n)) u64 DP words; time O(m+n+m*n).

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/edit_distance/Error`](#def-std_401_2fedit_5fdistance_2fError)


## env

<a id="def-core_401_2fenv_2fget"></a>

- `fn env.get(e: Env, name: String) -> Result[Option[String], EnvError]`

<a id="def-core_401_2fenv_2fnarrow"></a>

- `fn env.narrow(e: Env, names: List[String]) -> Result[Env, EnvError]`


## err

<a id="def-core_401_2ferr_2fprint"></a>

- `@noalloc fn err.print(s: Stderr, text: String) -> Result[(), OutputError]`

<a id="def-core_401_2ferr_2fwrite"></a>

- `@noalloc fn err.write(s: Stderr, bytes: Bytes) -> Result[(), OutputError]`


## f32

<a id="def-std_401_2ff32_2fabs"></a>

- `@noalloc pub fn f32.abs(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2facos"></a>

- `@noalloc pub fn f32.acos(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2facosh"></a>

- `@noalloc pub fn f32.acosh(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fasin"></a>

- `@noalloc pub fn f32.asin(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fasinh"></a>

- `@noalloc pub fn f32.asinh(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fatan"></a>

- `@noalloc pub fn f32.atan(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fatan2"></a>

- `@noalloc pub fn f32.atan2(y: f32, x: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fatanh"></a>

- `@noalloc pub fn f32.atanh(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-core_401_2ff32_2fbits"></a>

- `@noalloc fn f32.bits(value: f32) -> u32`

<a id="def-std_401_2ff32_2fcbrt"></a>

- `@noalloc pub fn f32.cbrt(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fceil"></a>

- `@noalloc pub fn f32.ceil(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fcopysign"></a>

- `@noalloc pub fn f32.copysign(magnitude: f32, sign: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fcos"></a>

- `@noalloc pub fn f32.cos(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fcosh"></a>

- `@noalloc pub fn f32.cosh(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fexp"></a>

- `@noalloc pub fn f32.exp(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fexp2"></a>

- `@noalloc pub fn f32.exp2(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fexpm1"></a>

- `@noalloc pub fn f32.expm1(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2ffloor"></a>

- `@noalloc pub fn f32.floor(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2ffma"></a>

- `@noalloc pub fn f32.fma(a: f32, b: f32, c: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fformat_5finline"></a>

- `@noalloc pub fn f32.format_inline[n: size](value: f32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2ff32_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn f32.format_into(value: f32, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff32_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn f32.format_into_spec( value: f32, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/f32`](#def-core_401_2ff32), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff32_2ffract"></a>

- `@noalloc pub fn f32.fract(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-core_401_2ff32_2ffrom_5fbits"></a>

- `@noalloc fn f32.from_bits(value: u32) -> f32`

<a id="def-core_401_2ff32_2ffrom_5ff64"></a>

- `@noalloc fn f32.from_f64(value: f64) -> f32`

<a id="def-std_401_2ff32_2fhypot"></a>

- `@noalloc pub fn f32.hypot(x: f32, y: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fln"></a>

- `@noalloc pub fn f32.ln(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fln1p"></a>

- `@noalloc pub fn f32.ln1p(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2flog10"></a>

- `@noalloc pub fn f32.log10(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2flog2"></a>

- `@noalloc pub fn f32.log2(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fparse"></a>

- `@noalloc pub fn f32.parse(text: String) -> Result[f32, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f32`](#def-core_401_2ff32), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff32_2fparse_5fbytes"></a>

- `@noalloc pub fn f32.parse_bytes(data: Bytes) -> Result[f32, scalar.ParseError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f32`](#def-core_401_2ff32), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff32_2fpow"></a>

- `@noalloc pub fn f32.pow(x: f32, y: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fpowi"></a>

- `@noalloc pub fn f32.powi(base: f32, exponent: i32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ff32_2fround"></a>

- `@noalloc pub fn f32.round(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fsin"></a>

- `@noalloc pub fn f32.sin(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fsinh"></a>

- `@noalloc pub fn f32.sinh(value: f32) -> f32 uses {}`

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

<a id="def-std_401_2ff32_2ftan"></a>

- `@noalloc pub fn f32.tan(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2ftanh"></a>

- `@noalloc pub fn f32.tanh(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2fto_5fstring"></a>

- `pub fn f32.to_string(value: f32) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/f32`](#def-core_401_2ff32)

<a id="def-std_401_2ff32_2ftotal_5forder"></a>

- `@noalloc pub fn f32.total_order(a: f32, b: f32) -> i32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ff32_2ftrunc"></a>

- `@noalloc pub fn f32.trunc(value: f32) -> f32 uses {}`

References: [`core@1/f32`](#def-core_401_2ff32)


## f64

<a id="def-std_401_2ff64_2fabs"></a>

- `@noalloc pub fn f64.abs(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2facos"></a>

- `@noalloc pub fn f64.acos(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2facosh"></a>

- `@noalloc pub fn f64.acosh(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fasin"></a>

- `@noalloc pub fn f64.asin(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fasinh"></a>

- `@noalloc pub fn f64.asinh(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fatan"></a>

- `@noalloc pub fn f64.atan(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fatan2"></a>

- `@noalloc pub fn f64.atan2(y: f64, x: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fatanh"></a>

- `@noalloc pub fn f64.atanh(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-core_401_2ff64_2fbits"></a>

- `@noalloc fn f64.bits(value: f64) -> u64`

<a id="def-std_401_2ff64_2fcbrt"></a>

- `@noalloc pub fn f64.cbrt(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fceil"></a>

- `@noalloc pub fn f64.ceil(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fcopysign"></a>

- `@noalloc pub fn f64.copysign(magnitude: f64, sign: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fcos"></a>

- `@noalloc pub fn f64.cos(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fcosh"></a>

- `@noalloc pub fn f64.cosh(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fexp"></a>

- `@noalloc pub fn f64.exp(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fexp2"></a>

- `@noalloc pub fn f64.exp2(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fexpm1"></a>

- `@noalloc pub fn f64.expm1(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2ffloor"></a>

- `@noalloc pub fn f64.floor(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2ffma"></a>

- `@noalloc pub fn f64.fma(a: f64, b: f64, c: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fformat_5finline"></a>

- `@noalloc pub fn f64.format_inline[n: size](value: f64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/scalar/FormatError`](#def-std_401_2fscalar_2fFormatError)

<a id="def-std_401_2ff64_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn f64.format_into(value: f64, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff64_2fformat_5finto_5fspec"></a>

- `@preserves_unique(out) pub fn f64.format_into_spec( value: f64, fill: char, align: u64, width: u64, precision: u64, flags: u64, kind: u64, mut out: List[u8] ) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/char`](#def-core_401_2fchar), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ff64_2ffract"></a>

- `@noalloc pub fn f64.fract(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-core_401_2ff64_2ffrom_5fbits"></a>

- `@noalloc fn f64.from_bits(value: u64) -> f64`

<a id="def-core_401_2ff64_2ffrom_5ff32"></a>

- `@noalloc fn f64.from_f32(value: f32) -> f64`

<a id="def-std_401_2ff64_2fhypot"></a>

- `@noalloc pub fn f64.hypot(x: f64, y: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fln"></a>

- `@noalloc pub fn f64.ln(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fln1p"></a>

- `@noalloc pub fn f64.ln1p(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2flog10"></a>

- `@noalloc pub fn f64.log10(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2flog2"></a>

- `@noalloc pub fn f64.log2(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fparse"></a>

- `@noalloc pub fn f64.parse(text: String) -> Result[f64, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff64_2fparse_5fbytes"></a>

- `@noalloc pub fn f64.parse_bytes(data: Bytes) -> Result[f64, scalar.ParseError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/f64`](#def-core_401_2ff64), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError)

<a id="def-std_401_2ff64_2fpow"></a>

- `@noalloc pub fn f64.pow(x: f64, y: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fpowi"></a>

- `@noalloc pub fn f64.powi(base: f64, exponent: i32) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ff64_2fround"></a>

- `@noalloc pub fn f64.round(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fsin"></a>

- `@noalloc pub fn f64.sin(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fsinh"></a>

- `@noalloc pub fn f64.sinh(value: f64) -> f64 uses {}`

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

<a id="def-std_401_2ff64_2ftan"></a>

- `@noalloc pub fn f64.tan(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2ftanh"></a>

- `@noalloc pub fn f64.tanh(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2fto_5fstring"></a>

- `pub fn f64.to_string(value: f64) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ff64_2ftotal_5forder"></a>

- `@noalloc pub fn f64.total_order(a: f64, b: f64) -> i32 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ff64_2ftrunc"></a>

- `@noalloc pub fn f64.trunc(value: f64) -> f64 uses {}`

References: [`core@1/f64`](#def-core_401_2ff64)


## failure

<a id="def-core_401_2ffailure_2fcheck"></a>

- `@noalloc fn failure.check(condition: bool, rule: CheckRule) -> () uses {}`


## file

<a id="def-std_401_2ffile_2fReadBoundedError"></a>

- `@serializable pub type file.ReadBoundedError = Limit | File(FileError)`

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2ffile_2fReadBoundedError_23File"></a>

- `File(FileError) -> file.ReadBoundedError`

Variant of file.ReadBoundedError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2ffile_2fReadBoundedError_23Limit"></a>

- `Limit -> file.ReadBoundedError`

Variant of file.ReadBoundedError.

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

<a id="def-core_401_2ffile_2fmap"></a>

- `fn file.map(d: Dir, p: Path) -> Result[Mapped of d, FileError]`

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

<a id="def-core_401_2ffile_2fopen_5fappend"></a>

- `fn file.open_append(d: Dir, p: Path) -> Result[File of d, FileError]`

<a id="def-core_401_2ffile_2fput_5fnew"></a>

- `fn file.put_new(d: Dir, p: Path, take data: Bytes, mode: Durability) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fread"></a>

- `@preserves_captures(f, buffer) fn file.read(d: Dir, mut f: File of d, mut buffer: List[u8], max: u64) -> Result[u64, FileError]`

<a id="def-std_401_2ffile_2fread_5fall_5fbounded"></a>

- `pub fn file.read_all_bounded(d: Dir, mut f: File of d, limit: u64) -> Result[ unique Bytes, ReadBoundedError ]`

Reads from the current cursor, leaving the caller's file open and unseeked.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/file/ReadBoundedError`](#def-std_401_2ffile_2fReadBoundedError)

<a id="def-core_401_2ffile_2fread_5fbytes"></a>

- `fn file.read_bytes(d: Dir, p: Path) -> Result[Bytes, FileError] in d`

<a id="def-core_401_2ffile_2fread_5fdir"></a>

- `fn file.read_dir(d: Dir, p: Path) -> Result[Iter[Result[DirEntry, FileError]], FileError]`

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

<a id="def-core_401_2ffile_2fsync"></a>

- `fn file.sync(d: Dir, mut f: File of d) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fwalk"></a>

- `fn file.walk(d: Dir) -> Result[Iter[Result[Path, FileError]], FileError]`

<a id="def-core_401_2ffile_2fwatch"></a>

- `fn file.watch(d: Dir, p: Path, recursive: bool) -> Result[Watcher of d, FileError]`

<a id="def-core_401_2ffile_2fwrite"></a>

- `fn file.write(d: Dir, mut f: File of d, data: Bytes) -> Result[u64, FileError]`

<a id="def-core_401_2ffile_2fwrite_5fall"></a>

- `fn file.write_all(d: Dir, mut f: AtomicFile of d, data: Bytes) -> Result[(), FileError]`

<a id="def-core_401_2ffile_2fwrite_5fbytes"></a>

- `fn file.write_bytes(d: Dir, p: Path, take data: Bytes) -> Result[(), FileError]`


## flate

<a id="def-std_401_2fflate_2fDecoder"></a>

- `pub opaque type flate.Decoder: copyable + inert + noalloc_drop uses {}`

Streaming decompression state for one stream of a format: its position,
the current block's codes, the last 32 KiB of output and the input bytes
read so far, which `Corrupt` offsets count from.

<a id="def-std_401_2fflate_2fEncoder"></a>

- `pub opaque type flate.Encoder: copyable + inert + noalloc_drop uses {}`

Streaming DEFLATE compression state for one stream: `encode` appends the
output its input completes, and `finish` the rest.

<a id="def-std_401_2fflate_2fFlateError"></a>

- `@serializable pub type flate.FlateError = Truncated | Corrupt(u64) | Header | Checksum | TooLarge | TrailingData( u64 )`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fflate_2fFlateError_23Checksum"></a>

- `Checksum -> flate.FlateError`

Variant of flate.FlateError.

<a id="def-std_401_2fflate_2fFlateError_23Corrupt"></a>

- `Corrupt(u64) -> flate.FlateError`

Variant of flate.FlateError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fflate_2fFlateError_23Header"></a>

- `Header -> flate.FlateError`

Variant of flate.FlateError.

<a id="def-std_401_2fflate_2fFlateError_23TooLarge"></a>

- `TooLarge -> flate.FlateError`

Variant of flate.FlateError.

<a id="def-std_401_2fflate_2fFlateError_23TrailingData"></a>

- `TrailingData( u64 ) -> flate.FlateError`

Variant of flate.FlateError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fflate_2fFlateError_23Truncated"></a>

- `Truncated -> flate.FlateError`

Variant of flate.FlateError.

<a id="def-std_401_2fflate_2fFormat"></a>

- `@serializable pub type flate.Format = Raw | Zlib | Gzip`

The container around a DEFLATE stream: none (RFC 1951), zlib (RFC 1950)
or gzip (RFC 1952).

<a id="def-std_401_2fflate_2fFormat_23Gzip"></a>

- `Gzip -> flate.Format`

Variant of flate.Format.

<a id="def-std_401_2fflate_2fFormat_23Raw"></a>

- `Raw -> flate.Format`

Variant of flate.Format.

<a id="def-std_401_2fflate_2fFormat_23Zlib"></a>

- `Zlib -> flate.Format`

Variant of flate.Format.

<a id="def-std_401_2fflate_2fReadError"></a>

- `@serializable pub type flate.ReadError[E] = Input(bufio.ReadError[E]) | Data(FlateError)`

A read from a buffered source failed, or its bytes do not decode.

References: [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fflate_2fReadError_23Data"></a>

- `Data(FlateError) -> flate.ReadError[E]`

Variant of flate.ReadError.

References: [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fflate_2fReadError_23Input"></a>

- `Input(bufio.ReadError[E]) -> flate.ReadError[E]`

Variant of flate.ReadError.

References: [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError)

<a id="def-std_401_2fflate_2fdecoder"></a>

- `pub fn flate.decoder(format: Format) -> unique Decoder uses {}`

References: [`std@1/flate/Decoder`](#def-std_401_2fflate_2fDecoder), [`std@1/flate/Format`](#def-std_401_2fflate_2fFormat)

<a id="def-std_401_2fflate_2fdeflate"></a>

- `@track_caller pub fn flate.deflate(data: Bytes, level: u64) -> unique Bytes uses {}`

Compresses `data` to a raw DEFLATE stream (RFC 1951) at `level` 0 to 9.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fflate_2fencode"></a>

- `@preserves_unique(out) pub fn flate.encode(mut e: Encoder, data: Bytes, mut out: List[u8]) -> () uses {}`

Compresses `data` as the next input of the stream, appending to `out`
every byte this input completes.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/flate/Encoder`](#def-std_401_2fflate_2fEncoder)

<a id="def-std_401_2fflate_2fencoder"></a>

- `@track_caller pub fn flate.encoder(format: Format, level: u64) -> unique Encoder uses {}`

A compressor at `level` 0 (stored) through 9 (smallest), writing `format`.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/flate/Encoder`](#def-std_401_2fflate_2fEncoder), [`std@1/flate/Format`](#def-std_401_2fflate_2fFormat)

<a id="def-std_401_2fflate_2ffile_5fread"></a>

- `@preserves_unique(out) pub fn flate.file_read( dir: Dir, mut f: File of dir, mut d: Decoder, mut r: bufio.Reader, mut out: List[u8], max: u64 ) -> Result[u64, ReadError[FileError]]`

`read` from a file through the buffered reader `r`.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/flate/Decoder`](#def-std_401_2fflate_2fDecoder), [`std@1/flate/ReadError`](#def-std_401_2fflate_2fReadError)

<a id="def-std_401_2fflate_2ffinish"></a>

- `@preserves_unique(out) pub fn flate.finish(mut e: Encoder, mut out: List[u8]) -> () uses {}`

Ends the stream: appends the rest of the compressed data and the format's
trailer to `out`, and leaves `e` ready for a new stream of the same format
and level.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/flate/Encoder`](#def-std_401_2fflate_2fEncoder)

<a id="def-std_401_2fflate_2fgzip_5fdecode"></a>

- `pub fn flate.gzip_decode(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}`

Decodes gzip data (RFC 1952): one or more members, concatenated, producing
at most `limit` bytes in total. Bytes after a member must begin another.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fflate_2fgzip_5fencode"></a>

- `@track_caller pub fn flate.gzip_encode(data: Bytes, level: u64) -> unique Bytes uses {}`

Compresses `data` to one gzip member (RFC 1952) at `level` 0 to 9.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fflate_2finflate"></a>

- `pub fn flate.inflate(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}`

Decodes a raw DEFLATE stream (RFC 1951) that ends exactly at the end of
`data`, producing at most `limit` bytes.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fflate_2fread"></a>

- `@preserves_unique(out) pub fn flate.read[S, E]( mut d: Decoder, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}`

Appends at most `max` decoded bytes to `out` and returns how many, zero
once the stream has ended: after its trailer for raw and zlib data, and at
the end of the source for gzip members. Input is read through `r` only as
decoding needs it, and bytes after a raw or zlib stream stay unread in `r`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/flate/Decoder`](#def-std_401_2fflate_2fDecoder), [`std@1/flate/ReadError`](#def-std_401_2fflate_2fReadError)

<a id="def-std_401_2fflate_2fzlib_5fdecode"></a>

- `pub fn flate.zlib_decode(data: Bytes, limit: u64) -> Result[unique Bytes, FlateError] uses {}`

Decodes zlib data (RFC 1950) without a preset dictionary, producing at most
`limit` bytes. The stream must end exactly at the end of `data`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fflate_2fzlib_5fencode"></a>

- `@track_caller pub fn flate.zlib_encode(data: Bytes, level: u64) -> unique Bytes uses {}`

Compresses `data` to zlib data (RFC 1950) at `level` 0 to 9.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)


## float

<a id="def-core_401_2ffloat_2fabs"></a>

- `@noalloc fn float.abs[F](value: F) -> F`

<a id="def-core_401_2ffloat_2fceil"></a>

- `@noalloc fn float.ceil[F](value: F) -> F`

<a id="def-core_401_2ffloat_2ffloor"></a>

- `@noalloc fn float.floor[F](value: F) -> F`

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

<a id="def-core_401_2ffloat_2fround"></a>

- `@noalloc fn float.round[F](value: F) -> F`

<a id="def-core_401_2ffloat_2fsqrt"></a>

- `@noalloc fn float.sqrt[F](value: F) -> F`

<a id="def-core_401_2ffloat_2fto_5fi32"></a>

- `@noalloc fn float.to_i32[F](value: F) -> Result[i32, FloatIntError]`

<a id="def-core_401_2ffloat_2ftrunc"></a>

- `@noalloc fn float.trunc[F](value: F) -> F`


## fnv

<a id="def-std_401_2ffnv_2fState"></a>

- `@serializable pub type fnv.State { value: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ffnv_2fdigest"></a>

- `@noalloc pub fn fnv.digest(data: Bytes) -> u64 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ffnv_2ffinish"></a>

- `@noalloc pub fn fnv.finish(take state: State) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/fnv/State`](#def-std_401_2ffnv_2fState)

<a id="def-std_401_2ffnv_2fnew"></a>

- `@noalloc pub fn fnv.new() -> State uses {}`

References: [`std@1/fnv/State`](#def-std_401_2ffnv_2fState)

<a id="def-std_401_2ffnv_2fupdate"></a>

- `@noalloc pub fn fnv.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/fnv/State`](#def-std_401_2ffnv_2fState)


## glob

<a id="def-std_401_2fglob_2fExpandError"></a>

- `@serializable pub type glob.ExpandError = InvalidPattern(PatternError) | Files(FileError)`

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

- `@serializable pub type glob.PatternError = Syntax(u64)`

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

<a id="def-std_401_2fglob_2fis_5fmatch"></a>

- `pub fn glob.is_match(pattern: Pattern, path: String) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/glob/Pattern`](#def-std_401_2fglob_2fPattern)


## group

<a id="def-core_401_2fgroup_2fclose_5fstdin"></a>

- `@noalloc fn group.close_stdin(p: Proc, mut g: ChildGroup of p) -> ()`

<a id="def-core_401_2fgroup_2fkill"></a>

- `@noalloc fn group.kill(p: Proc, mut g: ChildGroup of p, signal: Signal) -> ()`

<a id="def-core_401_2fgroup_2fread_5fstdout"></a>

- `fn group.read_stdout(p: Proc, mut g: ChildGroup of p, mut buffer: List[u8], max: u64) -> Result[u64, ProcessError]`

<a id="def-core_401_2fgroup_2fwrite"></a>

- `@noalloc fn group.write(p: Proc, mut g: ChildGroup of p, data: Bytes) -> Result[(), ProcessError]`


## hex

<a id="def-std_401_2fhex_2fDecodeError"></a>

- `@serializable pub type hex.DecodeError = InvalidDigit(u64) | OddLength`

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

<a id="def-std_401_2fhex_2fencode_5finto"></a>

- `@preserves_unique(out) pub fn hex.encode_into(data: Bytes, mut out: List[u8]) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8)


## hkdf

<a id="def-std_401_2fhkdf_2fderive"></a>

- `pub fn hkdf.derive[H: implementation std@1/digest.Digest]( salt: Bytes, secret: Bytes, info: Bytes, length: u64 ) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhkdf_2fexpand"></a>

- `pub fn hkdf.expand[H: implementation std@1/digest.Digest]( prk: Bytes, info: Bytes, length: u64 ) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhkdf_2fextract"></a>

- `pub fn hkdf.extract[H: implementation std@1/digest.Digest](salt: Bytes, secret: Bytes) -> Inline[ u8, H.bytes ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)


## hmac

<a id="def-std_401_2fhmac_2fState"></a>

- `pub opaque type hmac.State[S: plain + inert]: plain + inert uses {}`

<a id="def-std_401_2fhmac_2ffinish"></a>

- `pub fn hmac.finish[H: implementation std@1/digest.Digest](take state: State[H.State]) -> Inline[ u8, H.bytes ] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/hmac/State`](#def-std_401_2fhmac_2fState)

<a id="def-std_401_2fhmac_2fmac"></a>

- `pub fn hmac.mac[H: implementation std@1/digest.Digest](key: Bytes, data: Bytes) -> Inline[ u8, H.bytes ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fhmac_2fnew"></a>

- `pub fn hmac.new[H: implementation std@1/digest.Digest](key: Bytes) -> State[H.State] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/hmac/State`](#def-std_401_2fhmac_2fState)

<a id="def-std_401_2fhmac_2fupdate"></a>

- `pub fn hmac.update[H: implementation std@1/digest.Digest]( mut state: State[H.State], data: Bytes ) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/hmac/State`](#def-std_401_2fhmac_2fState)

<a id="def-std_401_2fhmac_2fverify"></a>

- `pub fn hmac.verify[H: implementation std@1/digest.Digest]( key: Bytes, data: Bytes, expected: Bytes ) -> bool uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)


## hpack

<a id="def-std_401_2fhpack_2fDecoder"></a>

- `pub opaque type hpack.Decoder: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fhpack_2fEncoder"></a>

- `pub opaque type hpack.Encoder: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fhpack_2fError"></a>

- `pub type hpack.Error = Truncated | IntegerOverflow | InvalidIndex | InvalidHuffman | TableSize | HeaderListTooLarge | Failed`

<a id="def-std_401_2fhpack_2fError_23Failed"></a>

- `Failed -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23HeaderListTooLarge"></a>

- `HeaderListTooLarge -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23IntegerOverflow"></a>

- `IntegerOverflow -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23InvalidHuffman"></a>

- `InvalidHuffman -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23InvalidIndex"></a>

- `InvalidIndex -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23TableSize"></a>

- `TableSize -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fError_23Truncated"></a>

- `Truncated -> hpack.Error`

Variant of hpack.Error.

<a id="def-std_401_2fhpack_2fField"></a>

- `pub type hpack.Field { name: Bytes, value: Bytes, sensitive: bool, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fhpack_2fdecode"></a>

- `pub fn hpack.decode(mut d: Decoder, data: Bytes, limit: u64) -> Result[List[Field], Error] uses {}`

Decode one complete header block. `limit` counts uncompressed name and
value octets plus 32 per field, including indexed fields (RFC 9113 6.5.2).
Any failure invalidates this context; callers must close the connection.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/hpack/Decoder`](#def-std_401_2fhpack_2fDecoder), [`std@1/hpack/Error`](#def-std_401_2fhpack_2fError), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField)

<a id="def-std_401_2fhpack_2fdecoder"></a>

- `pub fn hpack.decoder(capacity: u32) -> Decoder uses {}`

One decoding context per connection direction. `capacity` is the locally
advertised SETTINGS_HEADER_TABLE_SIZE, initially 4096 for HTTP/2.

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Decoder`](#def-std_401_2fhpack_2fDecoder)

<a id="def-std_401_2fhpack_2fencode"></a>

- `pub fn hpack.encode(mut e: Encoder, fields: List[Field]) -> unique Bytes uses {}`

Encode a complete block, reusing static/dynamic entries. Sensitive fields
always use never-indexed literals, even when an exact table match exists.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`std@1/hpack/Encoder`](#def-std_401_2fhpack_2fEncoder), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField)

<a id="def-std_401_2fhpack_2fencoder"></a>

- `pub fn hpack.encoder(capacity: u32) -> Encoder uses {}`

One encoding context per connection direction, using the peer's table limit.

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Encoder`](#def-std_401_2fhpack_2fEncoder)

<a id="def-std_401_2fhpack_2fhuffman_5fdecode"></a>

- `pub fn hpack.huffman_decode(data: Bytes, limit: u64) -> Result[unique Bytes, Error] uses {}`

Decode static Huffman octets, enforcing EOS and at most seven one bits
of padding. `limit` bounds output before allocation of the next octet.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/hpack/Error`](#def-std_401_2fhpack_2fError)

<a id="def-std_401_2fhpack_2fhuffman_5fencode"></a>

- `pub fn hpack.huffman_encode(data: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fhpack_2fset_5fallowed_5fcapacity"></a>

- `pub fn hpack.set_allowed_capacity(mut d: Decoder, capacity: u32) uses {}`

Change the locally advertised upper bound. A reduction must be reflected
by the peer's first size update before any field in the next block.

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Decoder`](#def-std_401_2fhpack_2fDecoder)

<a id="def-std_401_2fhpack_2fset_5fcapacity"></a>

- `pub fn hpack.set_capacity(mut e: Encoder, capacity: u32) uses {}`

Apply the peer's SETTINGS_HEADER_TABLE_SIZE. Multiple changes before the
next block emit the smallest value followed by the final value (7541 4.2).

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Encoder`](#def-std_401_2fhpack_2fEncoder)


## html

<a id="def-std_401_2fhtml_2fEscaped"></a>

- `@format(escaped_format) pub type html.Escaped { text: String, }`

Text to escape when formatted.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhtml_2fesc"></a>

- `@noalloc pub fn html.esc(value: String) -> Escaped in value uses {}`

Wrap text so an f-string hole escapes it as HTML.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/html/Escaped`](#def-std_401_2fhtml_2fEscaped)

<a id="def-std_401_2fhtml_2fescape"></a>

- `pub fn html.escape(value: String) -> unique String uses {}`

Escape &, <, >, double quote and apostrophe as HTML character references.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhtml_2fescape_5finto"></a>

- `@preserves_unique(out) pub fn html.escape_into(mut out: List[u8], value: String) -> () uses {}`

Append HTML text/quoted-attribute escaping, retaining existing output bytes.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)


## html_tokenizer

<a id="def-std_401_2fhtml_5ftokenizer_2fAttribute"></a>

- `pub type html_tokenizer.Attribute { key: String, value: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhtml_5ftokenizer_2fKind"></a>

- `pub type html_tokenizer.Kind = Text | StartTag | EndTag | SelfClosingTag | Comment | Doctype`

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23Comment"></a>

- `Comment -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23Doctype"></a>

- `Doctype -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23EndTag"></a>

- `EndTag -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23SelfClosingTag"></a>

- `SelfClosingTag -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23StartTag"></a>

- `StartTag -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fKind_23Text"></a>

- `Text -> html_tokenizer.Kind`

Variant of html_tokenizer.Kind.

<a id="def-std_401_2fhtml_5ftokenizer_2fLimit"></a>

- `pub type html_tokenizer.Limit = InputBytes | Tokens | Attributes | Work`

<a id="def-std_401_2fhtml_5ftokenizer_2fLimit_23Attributes"></a>

- `Attributes -> html_tokenizer.Limit`

Variant of html_tokenizer.Limit.

<a id="def-std_401_2fhtml_5ftokenizer_2fLimit_23InputBytes"></a>

- `InputBytes -> html_tokenizer.Limit`

Variant of html_tokenizer.Limit.

<a id="def-std_401_2fhtml_5ftokenizer_2fLimit_23Tokens"></a>

- `Tokens -> html_tokenizer.Limit`

Variant of html_tokenizer.Limit.

<a id="def-std_401_2fhtml_5ftokenizer_2fLimit_23Work"></a>

- `Work -> html_tokenizer.Limit`

Variant of html_tokenizer.Limit.

<a id="def-std_401_2fhtml_5ftokenizer_2fLimits"></a>

- `pub type html_tokenizer.Limits { bytes: u64, tokens: u64, attributes: u64, work: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhtml_5ftokenizer_2fSpan"></a>

- `pub type html_tokenizer.Span { start: u64, end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhtml_5ftokenizer_2fToken"></a>

- `pub type html_tokenizer.Token { kind: Kind, span: Span, data: String, attributes: List[Attribute], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/html_tokenizer/Attribute`](#def-std_401_2fhtml_5ftokenizer_2fAttribute), [`std@1/html_tokenizer/Kind`](#def-std_401_2fhtml_5ftokenizer_2fKind), [`std@1/html_tokenizer/Span`](#def-std_401_2fhtml_5ftokenizer_2fSpan)

<a id="def-std_401_2fhtml_5ftokenizer_2fTokenizeError"></a>

- `pub type html_tokenizer.TokenizeError = LimitExceeded(Limit, u64)`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/html_tokenizer/Limit`](#def-std_401_2fhtml_5ftokenizer_2fLimit)

<a id="def-std_401_2fhtml_5ftokenizer_2fTokenizeError_23LimitExceeded"></a>

- `LimitExceeded(Limit, u64) -> html_tokenizer.TokenizeError`

Variant of html_tokenizer.TokenizeError.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/html_tokenizer/Limit`](#def-std_401_2fhtml_5ftokenizer_2fLimit)

<a id="def-std_401_2fhtml_5ftokenizer_2fTokenizer"></a>

- `pub opaque type html_tokenizer.Tokenizer: copyable + inert + noalloc_drop uses {}`

Owns the bounded whole UTF-8 input. No I/O or caller-owned views.

<a id="def-std_401_2fhtml_5ftokenizer_2fnext"></a>

- `pub fn html_tokenizer.next(mut z: Tokenizer, allow_cdata: bool) -> Result[Option[Token], TokenizeError] uses {}`

None is stable EOF; errors poison the tokenizer and repeat unchanged.
Caller selects CDATA for this token's foreign-content context.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/html_tokenizer/Token`](#def-std_401_2fhtml_5ftokenizer_2fToken), [`std@1/html_tokenizer/TokenizeError`](#def-std_401_2fhtml_5ftokenizer_2fTokenizeError), [`std@1/html_tokenizer/Tokenizer`](#def-std_401_2fhtml_5ftokenizer_2fTokenizer)

<a id="def-std_401_2fhtml_5ftokenizer_2fnext_5fis_5fnot_5fraw_5ftext"></a>

- `pub fn html_tokenizer.next_is_not_raw_text(mut z: Tokenizer) -> () uses {}`

Override the next rawtext/RCDATA transition (e.g. an SVG title).

References: [`std@1/html_tokenizer/Tokenizer`](#def-std_401_2fhtml_5ftokenizer_2fTokenizer)

<a id="def-std_401_2fhtml_5ftokenizer_2ftokenize"></a>

- `pub fn html_tokenizer.tokenize(take source: String, limits: Limits, allow_cdata: bool) -> Result[ List[Token], TokenizeError ] uses {}`

Collect using a fixed CDATA policy. Use tokenizer/next to vary the context.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/html_tokenizer/Limits`](#def-std_401_2fhtml_5ftokenizer_2fLimits), [`std@1/html_tokenizer/Token`](#def-std_401_2fhtml_5ftokenizer_2fToken), [`std@1/html_tokenizer/TokenizeError`](#def-std_401_2fhtml_5ftokenizer_2fTokenizeError)

<a id="def-std_401_2fhtml_5ftokenizer_2ftokenizer"></a>

- `pub fn html_tokenizer.tokenizer(take source: String, limits: Limits) -> Result[ Tokenizer, TokenizeError ] uses {}`

All caps are inclusive; zero permits zero, never unlimited.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/html_tokenizer/Limits`](#def-std_401_2fhtml_5ftokenizer_2fLimits), [`std@1/html_tokenizer/TokenizeError`](#def-std_401_2fhtml_5ftokenizer_2fTokenizeError), [`std@1/html_tokenizer/Tokenizer`](#def-std_401_2fhtml_5ftokenizer_2fTokenizer)


## http

<a id="def-std_401_2fhttp_2fBody"></a>

- `@serializable pub type http.Body { bytes: Bytes, content_type: String, }`

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

- `@serializable pub type http.Config { max_redirects: u32, max_idle_connections: u64, max_idle_connections_per_host: u64, connect_timeout: Option[time.Duration], read_timeout: Option[time.Duration], write_timeout: Option[time.Duration], max_head_bytes: u64, max_body_bytes: u64, idle_timeout: Option[time.Duration], max_headers: u64, user_agent: String, roots: TlsRoots, status_as_error: bool, decompress: bool, unix_socket: Option[Path], http2_prior_knowledge: bool, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString), [`core@1/TlsRoots`](#def-core_401_2fTlsRoots), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fhttp_2fDetached"></a>

- `pub resource type http.Detached: noalloc_drop uses {Net} { net: Net, stream: Stream of .net, input: bufio.Reader, } @noalloc drop(self)`

A server connection after an HTTP/1.1 request, with the bytes already read past that
request, for a protocol that takes over the connection (WebSocket, an event stream)
and writes its own response. The connection serves no further requests.

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Stream`](#def-core_401_2fStream), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader)

<a id="def-std_401_2fhttp_2fHeader"></a>

- `@serializable pub type http.Header { name: String, value: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fhttp_2fHttpError"></a>

- `@serializable pub type http.HttpError = Net(NetError) | Url(uri.UriError) | Malformed(http_wire.HeadError) | Chunk | HeadTooLarge | BodyTooLarge | Truncated | TooManyRedirects | RedirectRequiresResend | RedirectLocationInvalid | UnsupportedScheme | UnsupportedTransferEncoding | Status( u16 ) | OutOfOrder | Closed | RequestTimeout | Clock(ClockError) | Json(json.JsonError) | InvalidJsonUtf8 | Decode( flate.FlateError ) | UnsupportedContentEncoding`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/u16`](#def-core_401_2fu16), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError), [`std@1/http_wire/HeadError`](#def-std_401_2fhttp_5fwire_2fHeadError), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/uri/UriError`](#def-std_401_2furi_2fUriError)

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

<a id="def-std_401_2fhttp_2fHttpError_23Decode"></a>

- `Decode( flate.FlateError ) -> http.HttpError`

Variant of http.HttpError.

References: [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

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

<a id="def-std_401_2fhttp_2fHttpError_23UnsupportedContentEncoding"></a>

- `UnsupportedContentEncoding -> http.HttpError`

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

- `@serializable pub type http.Method = Get | Head | Post | Put | Delete | Patch | Options`

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

- `@serializable pub type http.Response { status: u16, headers: List[Header], body: Bytes, url: String, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fhttp_2fResponseHead"></a>

- `@serializable pub type http.ResponseHead { version: Version, status: u16, reason: String, headers: List[Header], content_length: Option[u64], chunked: bool, keep_alive: bool, location: Option[String], content_type: Option[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u16`](#def-core_401_2fu16), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fServerConnection"></a>

- `pub opaque resource type http.ServerConnection: noalloc_drop uses {Net, Tasks}`

Owns the connection between exchanges. An unfinished checked-out stream
makes next_stream return OutOfOrder; abandoning it closes the connection.

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Tasks`](#def-core_401_2fTasks)

<a id="def-std_401_2fhttp_2fServerRequest"></a>

- `@serializable pub type http.ServerRequest { method: String, target: String, version: Version, headers: List[Header], body: Bytes, trailers: List[Header], keep_alive: bool, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fServerRequestView"></a>

- `pub opaque type http.ServerRequestView: copyable + inert + noalloc_drop uses {}`

A parsed HTTP/1 head whose fields are byte spans over one retained buffer.
Accessors materialize only the fields the handler uses. The view may outlive
the callback: subsequent reads preserve its immutable bytes.

<a id="def-std_401_2fhttp_2fServerResponse"></a>

- `pub type http.ServerResponse { status: u16, headers: List[Header], body: Bytes, close: bool, }`

A complete response produced by an inline handler. Framing belongs to HTTP.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fhttp_2fServerStream"></a>

- `pub opaque resource type http.ServerStream: sendable + noalloc_drop uses {Net, Tasks}`

An owned request/response exchange. A completed response returns the
connection through its channel. Dropping an unfinished exchange closes it,
without fabricating an application response. The next next_stream sees EOF.

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Tasks`](#def-core_401_2fTasks)

<a id="def-std_401_2fhttp_2fSseError"></a>

- `@serializable pub type http.SseError = EventLineBreak | IdLineBreak | IdNul`

<a id="def-std_401_2fhttp_2fSseError_23EventLineBreak"></a>

- `EventLineBreak -> http.SseError`

Variant of http.SseError.

<a id="def-std_401_2fhttp_2fSseError_23IdLineBreak"></a>

- `IdLineBreak -> http.SseError`

Variant of http.SseError.

<a id="def-std_401_2fhttp_2fSseError_23IdNul"></a>

- `IdNul -> http.SseError`

Variant of http.SseError.

<a id="def-std_401_2fhttp_2fVersion"></a>

- `@serializable pub type http.Version = Http10 | Http11 | Http2`

<a id="def-std_401_2fhttp_2fVersion_23Http10"></a>

- `Http10 -> http.Version`

Variant of http.Version.

<a id="def-std_401_2fhttp_2fVersion_23Http11"></a>

- `Http11 -> http.Version`

Variant of http.Version.

<a id="def-std_401_2fhttp_2fVersion_23Http2"></a>

- `Http2 -> http.Version`

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

<a id="def-std_401_2fhttp_2fdecode_5fcontent"></a>

- `pub fn http.decode_content(headers: List[Header], body: Bytes, limit: u64) -> Result[ unique Bytes, HttpError ] uses {}`

Removes the content codings named by every Content-Encoding field, last
applied first (RFC 9110 section 8.4): `gzip` and `x-gzip` are gzip data,
`deflate` is zlib data and `identity` is ignored. Each decoded stage holds
at most `limit` bytes (`BodyTooLarge` beyond); any other coding is
`UnsupportedContentEncoding`, which a server answers with 415.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fhttp_2fdefault_5fconfig"></a>

- `pub fn http.default_config() -> Config uses {}`

References: [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig)

<a id="def-std_401_2fhttp_2fdetach"></a>

- `pub fn http.detach(take s: ServerStream) -> Result[Detached, HttpError]`

Hands over the connection once `next_stream` has yielded this stream and before any
response; otherwise `OutOfOrder()`, as for an HTTP/2 stream, whose connection other
streams share (`Closed()` once closed).

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Detached`](#def-std_401_2fhttp_2fDetached), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)

<a id="def-std_401_2fhttp_2fdial"></a>

- `pub fn http.dial(n: Net, config: Config, parsed_uri: uri.Uri) -> Result[ Stream of n, HttpError ]`

Opens the connection a request to `parsed_uri` uses: `http` or `https` (any other
scheme is `UnsupportedScheme()`), the config's `unix_socket` or a TCP connect under
its connect timeout, the read and write timeouts, then `tls.connect` for `https`.
Protocols that start with an HTTP/1.1 exchange, such as WebSocket, dial with it.

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri)

<a id="def-std_401_2fhttp_2fend_5frequest_5fbody"></a>

- `pub fn http.end_request_body(mut r: Request, request_trailers: List[Header]) -> Result[ (), HttpError ]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fend_5fresponse_5fbody"></a>

- `pub fn http.end_response_body(mut s: ServerStream) -> Result[(), HttpError]`

Complete a streaming reply and automatically return its transport.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)

<a id="def-std_401_2fhttp_2ffetch"></a>

- `pub fn http.fetch( n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes] ) -> Result[Response, HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse)

<a id="def-std_401_2fhttp_2ffetch_5fbounded"></a>

- `pub fn http.fetch_bounded( n: Net, config: Config, method: Method, url: String, headers: List[Header], body: Option[Bytes], limit: u64 ) -> Result[Response, HttpError]`

One-shot fetch with an explicit raw and decoded body limit. Config's
max_body_bytes is an additional raw-body bound; decoding uses `limit`.
A zero limit admits only an empty body. Status errors precede body reads.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse)

<a id="def-std_401_2fhttp_2fform_5fbody"></a>

- `pub fn http.form_body(pairs: List[(String, String)]) -> Body uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody)

<a id="def-std_401_2fhttp_2fheader"></a>

- `pub fn http.header(headers: List[Header], name: String) -> Option[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fhttp_2fhost_5fheader"></a>

- `pub fn http.host_header(parsed_uri: uri.Uri) -> Result[unique String, HttpError] uses {}`

The `host` field value for `parsed_uri`: its canonical host, bracketed when IPv6,
with `:port` when the URI spells a port.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri)

<a id="def-std_401_2fhttp_2fjson_5fbody"></a>

- `pub fn http.json_body(value: json.Value) -> Result[Body, HttpError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fhttp_2fnext_5fstream"></a>

- `pub fn http.next_stream(mut c: ServerConnection, clock: Option[MonotonicClock]) -> Result[ Option[(sendable ServerStream, unique ServerRequest)], HttpError ]`

Yield one owned exchange and its decoded request. While it remains open,
this returns OutOfOrder. Dropping it unfinished ends reception.

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection), [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)

<a id="def-std_401_2fhttp_2fparse_5fresponse_5fhead"></a>

- `pub fn http.parse_response_head(buf: Bytes, config: Config, head_request: bool) -> Result[ Option[(ResponseHead, u64)], HttpError ] uses {}`

Parses a complete response head at the start of `buf`, returning it with the head's
byte length, or `None()` while it is incomplete. At most `config.max_headers` fields;
`head_request` says the response answers HEAD, so it has no body.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ResponseHead`](#def-std_401_2fhttp_2fResponseHead)

<a id="def-std_401_2fhttp_2fparse_5fserver_5frequest"></a>

- `pub fn http.parse_server_request(buf: Bytes, config: Config) -> Result[ Option[(ServerRequest, u64)], HttpError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest)

<a id="def-std_401_2fhttp_2fprotocol"></a>

- `pub fn http.protocol(s: ServerStream) -> Version uses {}`

Protocol used by this exchange. The parsed request and response heads carry
the same Http2 version when HTTP/2 is selected.

References: [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fread_5fall"></a>

- `pub fn http.read_all(mut r: Request, limit: u64) -> Result[unique Bytes, HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fread_5fbody"></a>

- `@preserves_unique(out) @preserves_captures(r) pub fn http.read_body(mut r: Request, mut out: List[u8], max: u64) -> Result[u64, HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2freceive_5fhead"></a>

- `pub fn http.receive_head(mut r: Request) -> Result[ResponseHead, HttpError]`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/http/ResponseHead`](#def-std_401_2fhttp_2fResponseHead)

<a id="def-std_401_2fhttp_2frequest"></a>

- `pub fn http.request( n: Net, config: Config, method: Method, url: String, headers: List[Header] ) -> Result[Request, HttpError]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Method`](#def-std_401_2fhttp_2fMethod), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2frequest_5fprotocol"></a>

- `pub fn http.request_protocol(r: Request) -> Version uses {}`

Protocol selected for this request's transport, before sending its headers.

References: [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2frequest_5ftarget"></a>

- `pub fn http.request_target(parsed_uri: uri.Uri) -> unique String uses {}`

The origin-form request target for `parsed_uri`: its encoded path (`/` when empty)
and `?query` when present.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri)

<a id="def-std_401_2fhttp_2frespond"></a>

- `pub fn http.respond( mut s: ServerStream, status: u16, headers: List[Header], body: Bytes, close: bool ) -> Result[(), HttpError]`

Send one complete reply and return the transport to its connection.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)

<a id="def-std_401_2fhttp_2frespond_5fhead"></a>

- `pub fn http.respond_head(mut s: ServerStream, status: u16, headers: List[Header]) -> Result[ (), HttpError ]`

Begin a streaming reply. The stream remains checked out until completion.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u16`](#def-core_401_2fu16), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)

<a id="def-std_401_2fhttp_2fresponse_5fjson"></a>

- `pub fn http.response_json(response: Response) -> Result[json.Value, HttpError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Response`](#def-std_401_2fhttp_2fResponse), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fhttp_2fsend_5fbodiless"></a>

- `pub fn http.send_bodiless(mut r: Request) -> Result[(), HttpError]`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fsend_5fbody"></a>

- `pub fn http.send_body(mut r: Request, body: Bytes) -> Result[(), HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fserve_5fnext"></a>

- `pub fn http.serve_next( mut c: ServerConnection, clock: Option[MonotonicClock], handler: fn(incoming: ServerRequestView) -> Result[ServerResponse, HttpError] ) -> Result[bool, HttpError]`

Receive, handle and respond on this task, retaining the connection locally.
Returns false at clean EOF. Use next_stream when the response handle must
leave this call (worker handoff, streaming, or protocol detachment).

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView), [`std@1/http/ServerResponse`](#def-std_401_2fhttp_2fServerResponse)

<a id="def-std_401_2fhttp_2fserve_5ftls"></a>

- `pub fn http.serve_tls( tasks: Tasks, n: Net, take stream: Stream of n, server: TlsServer, config: Config ) -> Result[ServerConnection, HttpError]`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/Tasks`](#def-core_401_2fTasks), [`core@1/TlsServer`](#def-core_401_2fTlsServer), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection)

<a id="def-std_401_2fhttp_2fserver_5fconnection"></a>

- `pub fn http.server_connection(tasks: Tasks, n: Net, take stream: Stream of n, config: Config) -> ServerConnection`

Construct an HTTP server over an accepted transport. Explicit prior knowledge
enables cleartext h2c; otherwise this uses HTTP/1.1.

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/Tasks`](#def-core_401_2fTasks), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/ServerConnection`](#def-std_401_2fhttp_2fServerConnection)

<a id="def-std_401_2fhttp_2fserver_5frequest_5fbody"></a>

- `pub fn http.server_request_body(r: ServerRequestView) -> Bytes in r uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView)

<a id="def-std_401_2fhttp_2fserver_5frequest_5fheader"></a>

- `pub fn http.server_request_header(r: ServerRequestView, name: String) -> Option[String] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView)

<a id="def-std_401_2fhttp_2fserver_5frequest_5fmethod"></a>

- `pub fn http.server_request_method(r: ServerRequestView) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView)

<a id="def-std_401_2fhttp_2fserver_5frequest_5fowned"></a>

- `pub fn http.server_request_owned(r: ServerRequestView) -> unique ServerRequest uses {}`

References: [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView)

<a id="def-std_401_2fhttp_2fserver_5frequest_5ftarget"></a>

- `pub fn http.server_request_target(r: ServerRequestView) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView)

<a id="def-std_401_2fhttp_2fserver_5frequest_5fversion"></a>

- `pub fn http.server_request_version(r: ServerRequestView) -> Version uses {}`

References: [`std@1/http/ServerRequestView`](#def-std_401_2fhttp_2fServerRequestView), [`std@1/http/Version`](#def-std_401_2fhttp_2fVersion)

<a id="def-std_401_2fhttp_2fsse_5fevent"></a>

- `pub fn http.sse_event( event: Option[String], data: List[String], id: Option[String], retry: Option[u64] ) -> Result[unique Bytes, SseError] uses {}`

Frames one WHATWG EventSource block as UTF-8. Optional fields are omitted
for None and emitted even for Some(""). Retry is in milliseconds. Data
items are lines; embedded CRLF, CR and LF each start another data field.
Invalid event/id fields fail instead of allowing field injection.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/SseError`](#def-std_401_2fhttp_2fSseError)

<a id="def-std_401_2fhttp_2ftrailers"></a>

- `pub fn http.trailers(r: Request) -> unique List[Header]`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fvalidate_5fheaders"></a>

- `pub fn http.validate_headers(headers: List[Header]) -> Result[(), HttpError] uses {}`

Rejects an empty or non-token header name and a value with a control byte other
than tab (`Malformed(HeaderName())`, `Malformed(HeaderValue())`): the header-injection
boundary every writer applies before writing a head.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fhttp_2fwrite_5frequest_5fbody"></a>

- `pub fn http.write_request_body(mut r: Request, data: Bytes) -> Result[(), HttpError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest)

<a id="def-std_401_2fhttp_2fwrite_5fresponse_5fbody"></a>

- `pub fn http.write_response_body(mut s: ServerStream, bytes: Bytes) -> Result[(), HttpError]`

Flush one fragment without ending the body; empty fragments are ignored.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream)


## http2

<a id="def-std_401_2fhttp2_2fConnection"></a>

- `pub opaque resource type http2.Connection: noalloc_drop uses {Net}`

An explicit HTTP/2 connection over an already selected Net transport.
`connection` uses prior knowledge: the caller supplies a plain h2c stream
or a TLS stream whose protocol it has already selected. It never upgrades
an HTTP/1.1 connection. Net's host/script/replay provider remains authoritative.

References: [`core@1/Net`](#def-core_401_2fNet)

<a id="def-std_401_2fhttp2_2fError"></a>

- `pub type http2.Error = Net(NetError) | Truncated | InvalidPreface | Wire( http2_wire.FrameError ) | Closed | InvalidHeaders | UnknownStream | WouldBlock`

References: [`core@1/NetError`](#def-core_401_2fNetError), [`std@1/http2_wire/FrameError`](#def-std_401_2fhttp2_5fwire_2fFrameError)

<a id="def-std_401_2fhttp2_2fError_23Closed"></a>

- `Closed -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fError_23InvalidHeaders"></a>

- `InvalidHeaders -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fError_23InvalidPreface"></a>

- `InvalidPreface -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fError_23Net"></a>

- `Net(NetError) -> http2.Error`

Variant of http2.Error.

References: [`core@1/NetError`](#def-core_401_2fNetError)

<a id="def-std_401_2fhttp2_2fError_23Truncated"></a>

- `Truncated -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fError_23UnknownStream"></a>

- `UnknownStream -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fError_23Wire"></a>

- `Wire( http2_wire.FrameError ) -> http2.Error`

Variant of http2.Error.

References: [`std@1/http2_wire/FrameError`](#def-std_401_2fhttp2_5fwire_2fFrameError)

<a id="def-std_401_2fhttp2_2fError_23WouldBlock"></a>

- `WouldBlock -> http2.Error`

Variant of http2.Error.

<a id="def-std_401_2fhttp2_2fEvent"></a>

- `pub type http2.Event = Idle | Headers(u32, List[hpack.Field], bool) | Data( u32, Bytes, u32, bool ) | Reset(u32, u32) | Goaway(u32, u32) | PingAck(Bytes) | Settings`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField)

<a id="def-std_401_2fhttp2_2fEvent_23Data"></a>

- `Data( u32, Bytes, u32, bool ) -> http2.Event`

Variant of http2.Event.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fhttp2_2fEvent_23Goaway"></a>

- `Goaway(u32, u32) -> http2.Event`

Variant of http2.Event.

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fhttp2_2fEvent_23Headers"></a>

- `Headers(u32, List[hpack.Field], bool) -> http2.Event`

Variant of http2.Event.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField)

<a id="def-std_401_2fhttp2_2fEvent_23Idle"></a>

- `Idle -> http2.Event`

Variant of http2.Event.

<a id="def-std_401_2fhttp2_2fEvent_23PingAck"></a>

- `PingAck(Bytes) -> http2.Event`

Variant of http2.Event.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fhttp2_2fEvent_23Reset"></a>

- `Reset(u32, u32) -> http2.Event`

Variant of http2.Event.

References: [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fhttp2_2fEvent_23Settings"></a>

- `Settings -> http2.Event`

Variant of http2.Event.

<a id="def-std_401_2fhttp2_2fRole"></a>

- `pub type http2.Role = Client | Server`

<a id="def-std_401_2fhttp2_2fRole_23Client"></a>

- `Client -> http2.Role`

Variant of http2.Role.

<a id="def-std_401_2fhttp2_2fRole_23Server"></a>

- `Server -> http2.Role`

Variant of http2.Role.

<a id="def-std_401_2fhttp2_2fSession"></a>

- `pub opaque type http2.Session: copyable + inert + noalloc_drop uses {}`

A transport-independent HTTP/2 endpoint. Feed complete validated frames
with receive; drain bytes with output and write them through the caller's
Net Stream. DATA events are not buffered here. Call consumed only after
the consumer releases the octets, including the reported padding cost.

<a id="def-std_401_2fhttp2_2fconnection"></a>

- `pub fn http2.connection(n: Net, take stream: Stream of n, role: Role, header_limit: u64) -> Connection`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Stream`](#def-core_401_2fStream), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Role`](#def-std_401_2fhttp2_2fRole)

<a id="def-std_401_2fhttp2_2fconsumed"></a>

- `pub fn http2.consumed(mut s: Session, id: u32, amount: u32) -> Result[(), Error] uses {}`

Return receive credit after the application consumes a DATA event. The
amount includes its padding and never exceeds octets actually received.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2fdrain_5fframes"></a>

- `pub fn http2.drain_frames(mut s: Session) -> (Bytes, List[http2_wire.Frame]) uses {}`

Drain the pending preface and frames in wire order without coalescing them.
Write the returned preface first, followed by each encoded frame.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame)

<a id="def-std_401_2fhttp2_2fflush"></a>

- `pub fn http2.flush(mut c: Connection) -> Result[(), Error]`

Write queued protocol frames. Any write failure makes the connection unusable.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError)

<a id="def-std_401_2fhttp2_2fgoaway"></a>

- `pub fn http2.goaway(mut s: Session, code: http2_wire.Code) uses {}`

References: [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_2fheaders"></a>

- `pub fn http2.headers(mut s: Session, id: u32, fields: List[hpack.Field], end: bool) -> Result[ (), Error ] uses {}`

Send initial response headers, or trailers after initial headers.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2fnext_5fevent"></a>

- `pub fn http2.next_event(mut c: Connection) -> Result[Event, Error]`

Read one event, retaining partial frame bytes across reads. Protocol
responses (SETTINGS/PING acknowledgements, RST_STREAM and GOAWAY) are
flushed before returning. EOF in a partial frame/preface is Truncated.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Event`](#def-std_401_2fhttp2_2fEvent)

<a id="def-std_401_2fhttp2_2fopen"></a>

- `pub fn http2.open(mut s: Session, fields: List[hpack.Field], end: bool) -> Result[u32, Error] uses {}`

Open another client stream without waiting for earlier responses. Odd IDs
increase monotonically. WouldBlock means the peer's concurrency limit is full.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2foutput"></a>

- `pub fn http2.output(mut s: Session) -> unique Bytes uses {}`

Coalesce all pending writes into one wire buffer. For streaming writes,
drain_frames avoids copying all queued payloads into a combined buffer.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2fping"></a>

- `pub fn http2.ping(mut s: Session, payload: Bytes) -> Result[(), Error] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2freceive"></a>

- `pub fn http2.receive(mut s: Session, frame: http2_wire.Frame) -> Result[Event, Error] uses {}`

Process one complete frame. The parser validates its fixed shape first,
including when callers construct Frame values directly. Connection errors
emit GOAWAY and permanently fail the session; stream errors emit RST_STREAM.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Event`](#def-std_401_2fhttp2_2fEvent), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame)

<a id="def-std_401_2fhttp2_2frelease_5fbody"></a>

- `pub fn http2.release_body(mut c: Connection, id: u32, flow_bytes: u32) -> Result[(), Error]`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError)

<a id="def-std_401_2fhttp2_2freset"></a>

- `pub fn http2.reset(mut s: Session, id: u32, code: http2_wire.Code) -> Result[(), Error] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_2freset_5fstream"></a>

- `pub fn http2.reset_stream(mut c: Connection, id: u32, code: http2_wire.Code) -> Result[ (), Error ]`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_2fsend_5fdata"></a>

- `pub fn http2.send_data(mut s: Session, id: u32, data: Bytes, end: bool) -> Result[u64, Error] uses {}`

Send as many bytes as connection/stream windows and the peer frame limit
allow. Zero means wait for WINDOW_UPDATE. END_STREAM is sent only when
all supplied bytes fit. Empty final DATA is legal even at a zero window.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2fsession"></a>

- `pub fn http2.session(role: Role, header_limit: u64) -> Session uses {}`

Both peers advertise no server push. Clients emit the connection preface;
servers must verify the client's preface before passing its first frame.
Header blocks and decoded header lists are each bounded by `header_limit`.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/http2/Role`](#def-std_401_2fhttp2_2fRole), [`std@1/http2/Session`](#def-std_401_2fhttp2_2fSession)

<a id="def-std_401_2fhttp2_2fstart"></a>

- `pub fn http2.start(mut c: Connection, fields: List[hpack.Field], end: bool) -> Result[ u32, Error ]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError)

<a id="def-std_401_2fhttp2_2fwrite_5fbody"></a>

- `pub fn http2.write_body(mut c: Connection, id: u32, data: Bytes, end: bool) -> Result[ u64, Error ]`

The streaming body writer returns the prefix accepted by flow control.
Call next_event to receive WINDOW_UPDATE when this returns zero.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError)

<a id="def-std_401_2fhttp2_2fwrite_5fheaders"></a>

- `pub fn http2.write_headers(mut c: Connection, id: u32, fields: List[hpack.Field], end: bool) -> Result[ (), Error ]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u32`](#def-core_401_2fu32), [`std@1/hpack/Field`](#def-std_401_2fhpack_2fField), [`std@1/http2/Connection`](#def-std_401_2fhttp2_2fConnection), [`std@1/http2/Error`](#def-std_401_2fhttp2_2fError)


## http2_wire

<a id="def-std_401_2fhttp2_5fwire_2fCode"></a>

- `pub type http2_wire.Code = Protocol | Internal | FlowControl | SettingsTimeout | StreamClosed | FrameSize | RefusedStream | Cancel | Compression | Connect | EnhanceYourCalm | InadequateSecurity | Http11Required | NoError`

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Cancel"></a>

- `Cancel -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Compression"></a>

- `Compression -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Connect"></a>

- `Connect -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23EnhanceYourCalm"></a>

- `EnhanceYourCalm -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23FlowControl"></a>

- `FlowControl -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23FrameSize"></a>

- `FrameSize -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Http11Required"></a>

- `Http11Required -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23InadequateSecurity"></a>

- `InadequateSecurity -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Internal"></a>

- `Internal -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23NoError"></a>

- `NoError -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23Protocol"></a>

- `Protocol -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23RefusedStream"></a>

- `RefusedStream -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23SettingsTimeout"></a>

- `SettingsTimeout -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fCode_23StreamClosed"></a>

- `StreamClosed -> http2_wire.Code`

Variant of http2_wire.Code.

<a id="def-std_401_2fhttp2_5fwire_2fFrame"></a>

- `pub type http2_wire.Frame { kind: u8, flags: u8, stream: u32, payload: Bytes, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fhttp2_5fwire_2fFrameError"></a>

- `pub type http2_wire.FrameError = ConnectionError(Code) | StreamError(u32, Code)`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_5fwire_2fFrameError_23ConnectionError"></a>

- `ConnectionError(Code) -> http2_wire.FrameError`

Variant of http2_wire.FrameError.

References: [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_5fwire_2fFrameError_23StreamError"></a>

- `StreamError(u32, Code) -> http2_wire.FrameError`

Variant of http2_wire.FrameError.

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_5fwire_2fcode_5fnumber"></a>

- `@noalloc pub fn http2_wire.code_number(code: Code) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2_wire/Code`](#def-std_401_2fhttp2_5fwire_2fCode)

<a id="def-std_401_2fhttp2_5fwire_2fencode"></a>

- `pub fn http2_wire.encode(frame: Frame, max_size: u32) -> Result[unique Bytes, FrameError] uses {}`

Serialize a frame with the peer's maximum payload size. Stream IDs with
the reserved bit set are rejected rather than silently changed.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame), [`std@1/http2_wire/FrameError`](#def-std_401_2fhttp2_5fwire_2fFrameError)

<a id="def-std_401_2fhttp2_5fwire_2ffragment"></a>

- `@noalloc pub fn http2_wire.fragment(frame: Frame) -> Bytes in frame`

DATA octets or the HPACK fragment of HEADERS/PUSH_PROMISE/CONTINUATION,
excluding padding and priority fields. The input must have passed parse.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame)

<a id="def-std_401_2fhttp2_5fwire_2fparse"></a>

- `pub fn http2_wire.parse(data: Bytes, offset: u64, max_size: u32) -> Result[ Option[(Frame, u64)], FrameError ] in data`

Parse one frame at `offset`; incomplete input consumes nothing. The
reserved stream bit is ignored. Oversized frames fail from the header,
before waiting for their payload. `max_size` is the local frame limit.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame), [`std@1/http2_wire/FrameError`](#def-std_401_2fhttp2_5fwire_2fFrameError)

<a id="def-std_401_2fhttp2_5fwire_2fpreface"></a>

- `pub fn http2_wire.preface() -> Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fhttp2_5fwire_2fread_5fu32"></a>

- `@noalloc pub fn http2_wire.read_u32(data: Bytes, offset: u64) -> u32 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp2_5fwire_2fu32_5fbytes"></a>

- `pub fn http2_wire.u32_bytes(value: u32) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u32`](#def-core_401_2fu32)

<a id="def-std_401_2fhttp2_5fwire_2fvalidate"></a>

- `@noalloc pub fn http2_wire.validate(f: Frame) -> Result[(), FrameError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http2_wire/Frame`](#def-std_401_2fhttp2_5fwire_2fFrame), [`std@1/http2_wire/FrameError`](#def-std_401_2fhttp2_5fwire_2fFrameError)


## http_date

<a id="def-std_401_2fhttp_5fdate_2fFormatError"></a>

- `@serializable pub type http_date.FormatError = OutOfRange | Subsecond`

<a id="def-std_401_2fhttp_5fdate_2fFormatError_23OutOfRange"></a>

- `OutOfRange -> http_date.FormatError`

Variant of http_date.FormatError.

<a id="def-std_401_2fhttp_5fdate_2fFormatError_23Subsecond"></a>

- `Subsecond -> http_date.FormatError`

Variant of http_date.FormatError.

<a id="def-std_401_2fhttp_5fdate_2fParseError"></a>

- `@serializable pub type http_date.ParseError { offset: u64, kind: ParseErrorKind, }`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/http_date/ParseErrorKind`](#def-std_401_2fhttp_5fdate_2fParseErrorKind)

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind"></a>

- `@serializable pub type http_date.ParseErrorKind = Length | NonAscii | Separator | Weekday | Month | Digit | Year | Day | Hour | Minute | Second | Calendar`

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Calendar"></a>

- `Calendar -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Day"></a>

- `Day -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Digit"></a>

- `Digit -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Hour"></a>

- `Hour -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Length"></a>

- `Length -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Minute"></a>

- `Minute -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Month"></a>

- `Month -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23NonAscii"></a>

- `NonAscii -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Second"></a>

- `Second -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Separator"></a>

- `Separator -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Weekday"></a>

- `Weekday -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fParseErrorKind_23Year"></a>

- `Year -> http_date.ParseErrorKind`

Variant of http_date.ParseErrorKind.

<a id="def-std_401_2fhttp_5fdate_2fformat"></a>

- `pub fn http_date.format(value: time.Timestamp) -> Result[unique String, FormatError] uses {}`

Canonical UTC text; rejects out-of-range and subsecond timestamps.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http_date/FormatError`](#def-std_401_2fhttp_5fdate_2fFormatError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2fhttp_5fdate_2fparse"></a>

- `@noalloc pub fn http_date.parse(value: String) -> Result[time.Timestamp, ParseError] uses {}`

Exactly 29 ASCII bytes, matching weekday, GMT, and years 1970 through 9999.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http_date/ParseError`](#def-std_401_2fhttp_5fdate_2fParseError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)


## http_wire

<a id="def-std_401_2fhttp_5fwire_2fChunkError"></a>

- `@serializable pub type http_wire.ChunkError = InvalidChunkSize()`

<a id="def-std_401_2fhttp_5fwire_2fChunkError_23InvalidChunkSize"></a>

- `InvalidChunkSize() -> http_wire.ChunkError`

Variant of http_wire.ChunkError.

<a id="def-std_401_2fhttp_5fwire_2fChunkSize"></a>

- `@serializable pub type http_wire.ChunkSize = Complete(u64, u64) | Partial`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fChunkSize_23Complete"></a>

- `Complete(u64, u64) -> http_wire.ChunkSize`

Variant of http_wire.ChunkSize.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fChunkSize_23Partial"></a>

- `Partial -> http_wire.ChunkSize`

Variant of http_wire.ChunkSize.

<a id="def-std_401_2fhttp_5fwire_2fFraming"></a>

- `@serializable pub type http_wire.Framing { content_length: Option[u64], chunked: bool, keep_alive: bool, }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fHeadError"></a>

- `@serializable pub type http_wire.HeadError = HeaderName | HeaderValue | NewLine | Status | Token | TooManyHeaders | Version | ContentLength | TransferEncoding | Continuation`

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

- `@serializable pub type http_wire.HeaderIterator { index: u64, is_trailer: bool, done: bool, }`

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus"></a>

- `@serializable pub type http_wire.ParseStatus = Complete(u64) | Partial`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus_23Complete"></a>

- `Complete(u64) -> http_wire.ParseStatus`

Variant of http_wire.ParseStatus.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fParseStatus_23Partial"></a>

- `Partial -> http_wire.ParseStatus`

Variant of http_wire.ParseStatus.

<a id="def-std_401_2fhttp_5fwire_2fRawHeader"></a>

- `@serializable pub type http_wire.RawHeader { name_start: u64, name_end: u64, value_start: u64, value_end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fhttp_5fwire_2fRequestLine"></a>

- `@serializable pub type http_wire.RequestLine { method_start: u64, method_end: u64, target_start: u64, target_end: u64, version: u8, }`

References: [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fhttp_5fwire_2fStatusLine"></a>

- `@serializable pub type http_wire.StatusLine { version: u8, code: u16, reason_start: u64, reason_end: u64, }`

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

<a id="def-std_401_2fi128_2fformat_5finline"></a>

- `@noalloc pub fn i128.format_inline[n: size](value: i128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fi16_2fformat_5finline"></a>

- `@noalloc pub fn i16.format_inline[n: size](value: i16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fi32_2fformat_5finline"></a>

- `@noalloc pub fn i32.format_inline[n: size](value: i32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fi64_2fformat_5finline"></a>

- `@noalloc pub fn i64.format_inline[n: size](value: i64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fi8_2fformat_5finline"></a>

- `@noalloc pub fn i8.format_inline[n: size](value: i8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2finput_2fReadBoundedError"></a>

- `@serializable pub type input.ReadBoundedError = LineTooLong | Input(InputError)`

References: [`core@1/InputError`](#def-core_401_2fInputError)

<a id="def-std_401_2finput_2fReadBoundedError_23Input"></a>

- `Input(InputError) -> input.ReadBoundedError`

Variant of input.ReadBoundedError.

References: [`core@1/InputError`](#def-core_401_2fInputError)

<a id="def-std_401_2finput_2fReadBoundedError_23LineTooLong"></a>

- `LineTooLong -> input.ReadBoundedError`

Variant of input.ReadBoundedError.

<a id="def-std_401_2finput_2fread_5fall"></a>

- `pub fn input.read_all(s: Stdin) -> Result[unique Bytes, InputError]`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin)

<a id="def-std_401_2finput_2fread_5fline"></a>

- `pub fn input.read_line(s: Stdin) -> Result[Option[unique String], InputError]`

References: [`core@1/InputError`](#def-core_401_2fInputError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2finput_2fread_5fline_5fbounded"></a>

- `pub fn input.read_line_bounded(s: Stdin, max_line_bytes: u64) -> Result[ Option[unique String], ReadBoundedError ]`

Counts bytes before LF, including CR. A limit error consumes one rejected
non-LF byte and does not drain: recovery is the caller's policy.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stdin`](#def-core_401_2fStdin), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/input/ReadBoundedError`](#def-std_401_2finput_2fReadBoundedError)


## instant

<a id="def-core_401_2finstant_2fchecked_5fadd"></a>

- `@noalloc fn instant.checked_add(clock: MonotonicClock, start: Instant of clock, nanoseconds: u64) -> Option[Instant of clock]`

<a id="def-core_401_2finstant_2fticks"></a>

- `@noalloc fn instant.ticks(value: Instant) -> u64 uses {}`


## int

<a id="def-core_401_2fint_2fchecked_5fadd"></a>

- `@noalloc fn int.checked_add[I](a: I, b: I) -> Option[I]`

<a id="def-core_401_2fint_2fchecked_5fmul"></a>

- `@noalloc fn int.checked_mul[I](a: I, b: I) -> Option[I]`

<a id="def-core_401_2fint_2fchecked_5fsub"></a>

- `@noalloc fn int.checked_sub[I](a: I, b: I) -> Option[I]`

<a id="def-core_401_2fint_2fcount_5fones"></a>

- `@noalloc fn int.count_ones[I](value: I) -> I`

<a id="def-core_401_2fint_2fdiv_5fexact"></a>

- `@noalloc fn int.div_exact[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fdiv_5ffloor"></a>

- `@noalloc fn int.div_floor[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fleading_5fzeros"></a>

- `@noalloc fn int.leading_zeros[I](value: I) -> I`

<a id="def-core_401_2fint_2fmod"></a>

- `@noalloc fn int.mod[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fnarrow"></a>

- `@noalloc fn int.narrow[I, J](value: I) -> J`

<a id="def-core_401_2fint_2freverse_5fbits"></a>

- `@noalloc fn int.reverse_bits[I](value: I) -> I`

<a id="def-core_401_2fint_2fsaturating_5fadd"></a>

- `@noalloc fn int.saturating_add[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fsaturating_5fsub"></a>

- `@noalloc fn int.saturating_sub[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fswap_5fbytes"></a>

- `@noalloc fn int.swap_bytes[I](value: I) -> I`

<a id="def-core_401_2fint_2ftrailing_5fzeros"></a>

- `@noalloc fn int.trailing_zeros[I](value: I) -> I`

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

<a id="def-core_401_2fint_2fwrapping_5fmul"></a>

- `@noalloc fn int.wrapping_mul[I](a: I, b: I) -> I`

<a id="def-core_401_2fint_2fwrapping_5fsub"></a>

- `@noalloc fn int.wrapping_sub[I](a: I, b: I) -> I`


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

<a id="def-core_401_2fio_2frunner"></a>

- `@noalloc fn io.runner(rt: Io, name: String) -> Result[Runner, GrantError]`

<a id="def-core_401_2fio_2fsignals"></a>

- `@noalloc fn io.signals(rt: Io) -> Result[Signals, GrantError]`

<a id="def-core_401_2fio_2fstderr"></a>

- `@noalloc fn io.stderr(rt: Io) -> Result[Stderr, GrantError]`

<a id="def-core_401_2fio_2fstdin"></a>

- `@noalloc fn io.stdin(rt: Io) -> Result[Stdin, GrantError]`

<a id="def-core_401_2fio_2fstdout"></a>

- `@noalloc fn io.stdout(rt: Io) -> Result[Stdout, GrantError]`

<a id="def-core_401_2fio_2ftasks"></a>

- `@noalloc fn io.tasks(rt: Io) -> Result[Tasks, GrantError]`

<a id="def-core_401_2fio_2ftelemetry"></a>

- `@noalloc fn io.telemetry(rt: Io) -> Result[Telemetry, GrantError]`

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

<a id="def-std_401_2fiter_2fmap"></a>

- `@captures(it, f) pub fn iter.map[T, U](take it: Iter[T], take f: fn(T) -> U) -> Iter[U] uses {it, f}`

References: [`core@1/Iter`](#def-core_401_2fIter)

<a id="def-core_401_2fiter_2fnext"></a>

- `fn iter.next[T](mut it: Iter[T]) -> Option[T]`

<a id="def-std_401_2fiter_2ftake_5fn"></a>

- `@captures(it) pub fn iter.take_n[T](take it: Iter[T], count: u64) -> Iter[T] uses {it}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fiter_2ftry_5fcollect"></a>

- `pub fn iter.try_collect[T, E](take it: Iter[Result[T, E]]) -> Result[List[T], E] uses {it}`

Collects successful items, stopping at the first error.

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fiter_2fzip"></a>

- `@captures(a, b) pub fn iter.zip[T, U](take a: Iter[T], take b: Iter[U]) -> Iter[(T, U)] uses {a, b}`

References: [`core@1/Iter`](#def-core_401_2fIter)


## json

<a id="def-std_401_2fjson_2fJsonError"></a>

- `@serializable pub type json.JsonError = Syntax(u64) | DuplicateKey(u64) | Nesting(u64) | InvalidNumber`

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

<a id="def-std_401_2fjson_2fReadTextBoundedError"></a>

- `pub type json.ReadTextBoundedError = Json(JsonError) | Limit`

JSON syntax/UTF-8 failures retain their old cause; Limit has no partial text.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjson_2fReadTextBoundedError_23Json"></a>

- `Json(JsonError) -> json.ReadTextBoundedError`

Variant of json.ReadTextBoundedError.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjson_2fReadTextBoundedError_23Limit"></a>

- `Limit -> json.ReadTextBoundedError`

Variant of json.ReadTextBoundedError.

<a id="def-std_401_2fjson_2fReader"></a>

- `pub type json.Reader { data: Bytes, pos: u64, depth: u64, }`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_2fTextView"></a>

- `pub type json.TextView = Unescaped(Bytes) | Escaped(String)`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fTextView_23Escaped"></a>

- `Escaped(String) -> json.TextView`

Variant of json.TextView.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fTextView_23Unescaped"></a>

- `Unescaped(Bytes) -> json.TextView`

Variant of json.TextView.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fjson_2fToken"></a>

- `pub type json.Token = NullToken | BoolToken(bool) | NumberToken(Bytes) | TextToken(TextView) | ArrayStart | ArrayEnd | ObjectStart | ObjectEnd | Colon | Comma`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2fToken_23ArrayEnd"></a>

- `ArrayEnd -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23ArrayStart"></a>

- `ArrayStart -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23BoolToken"></a>

- `BoolToken(bool) -> json.Token`

Variant of json.Token.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fjson_2fToken_23Colon"></a>

- `Colon -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23Comma"></a>

- `Comma -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23NullToken"></a>

- `NullToken -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23NumberToken"></a>

- `NumberToken(Bytes) -> json.Token`

Variant of json.Token.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fjson_2fToken_23ObjectEnd"></a>

- `ObjectEnd -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23ObjectStart"></a>

- `ObjectStart -> json.Token`

Variant of json.Token.

<a id="def-std_401_2fjson_2fToken_23TextToken"></a>

- `TextToken(TextView) -> json.Token`

Variant of json.Token.

References: [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2fValue"></a>

- `@serializable pub type json.Value = Null | Bool(bool) | Number(String) | Text(String) | Array(List[Value]) | Object( Map[String, Value] )`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fjson_2fValue_23Array"></a>

- `Array(List[Value]) -> json.Value`

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

- `Object( Map[String, Value] ) -> json.Value`

Variant of json.Value.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fValue_23Text"></a>

- `Text(String) -> json.Value`

Variant of json.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2farray_5fnext"></a>

- `pub fn json.array_next(mut cursor: Reader, mut first: bool) -> Result[bool, JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fbegin_5farray"></a>

- `pub fn json.begin_array(mut cursor: Reader) -> Result[(), JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fbegin_5fobject"></a>

- `pub fn json.begin_object(mut cursor: Reader) -> Result[(), JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fcheck_5fdepth"></a>

- `@noalloc pub fn json.check_depth(depth: u64) -> Result[(), JsonError] uses {}`

Check a writer's number of open containers, before writing its delimiter.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjson_2fcolon"></a>

- `pub fn json.colon(mut cursor: Reader) -> Result[(), JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2ffinish_5freader"></a>

- `pub fn json.finish_reader(mut cursor: Reader) -> Result[(), JsonError] uses {}`

Require only whitespace after the decoded root.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fnext_5ftoken"></a>

- `pub fn json.next_token(mut cursor: Reader) -> Result[Token, JsonError] in cursor uses {}`

Read one lexical token. Container consumers enforce delimiter grammar.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader), [`std@1/json/Token`](#def-std_401_2fjson_2fToken)

<a id="def-std_401_2fjson_2fnumber_5ftext"></a>

- `pub fn json.number_text(raw: Bytes) -> unique String uses {}`

Copy only a token's bytes when an owned String is required by scalar.parse.
The pull reader itself exposes the original spelling without allocation.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_2fobject_5fkey"></a>

- `pub fn json.object_key(mut cursor: Reader, mut first: bool) -> Result[ Option[TextView], JsonError ] in cursor uses {}`

Next object key without allocating a key set. Generated records check their
declared field slots and keep a set only for unknown fields.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader), [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2fobject_5fnext"></a>

- `pub fn json.object_next(mut cursor: Reader, mut first: bool, mut keys: Set[String]) -> Result[ Option[String], JsonError ] uses {}`

Next object key, rejecting decoded duplicates before reading its value.
The caller owns the key set for this object, including ignored members.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Set`](#def-core_401_2fSet), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fparse"></a>

- `pub fn json.parse(take text: String) -> Result[Value, JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_2fpeek"></a>

- `pub fn json.peek(mut cursor: Reader) -> u8 uses {}`

The next non-whitespace byte, or zero at end of cursor.

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fread_5fnumber"></a>

- `pub fn json.read_number(mut cursor: Reader) -> Result[Bytes, JsonError] in cursor uses {}`

A lossless number spelling as a view of the cursor.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fread_5fowned_5ftext"></a>

- `pub fn json.read_owned_text(mut cursor: Reader) -> Result[unique String, JsonError] uses {}`

An independently owned decoded string for an owned typed field. The reader's
view path remains available for keys and transient consumers.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fread_5fowned_5ftext_5fbounded"></a>

- `pub fn json.read_owned_text_bounded(mut cursor: Reader, max_decoded_bytes: u64) -> Result[ unique String, ReadTextBoundedError ] uses {}`

Decode at most max_decoded_bytes UTF-8 bytes, excluding the quotes. Syntax
wins over Limit; success and Limit stop immediately after the closing quote.
Logical retained length is bounded, not the allocator's capacity/RSS slack.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/ReadTextBoundedError`](#def-std_401_2fjson_2fReadTextBoundedError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fread_5ftext"></a>

- `pub fn json.read_text(mut cursor: Reader) -> Result[TextView, JsonError] in cursor uses {}`

A quoted string: a view when unescaped, independent storage otherwise.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader), [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2freader"></a>

- `pub fn json.reader(data: Bytes) -> Reader in data uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fskip"></a>

- `pub fn json.skip(mut cursor: Reader) -> Result[(), JsonError] uses {}`

Validate and skip one value, retaining only object key sets, never a DOM.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader)

<a id="def-std_401_2fjson_2fstringify"></a>

- `pub fn json.stringify(value: Value) -> Result[unique String, JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_2fstringify_5finto"></a>

- `@preserves_unique(out) pub fn json.stringify_into(value: Value, mut out: List[u8]) -> Result[(), JsonError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_2ftext_5fis"></a>

- `pub fn json.text_is(value: TextView, expected: String) -> bool uses {}`

Compare a borrowed key against a field name quoted at compile time.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2ftext_5fowned"></a>

- `pub fn json.text_owned(value: TextView) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/TextView`](#def-std_401_2fjson_2fTextView)

<a id="def-std_401_2fjson_2fvalid_5futf8"></a>

- `pub fn json.valid_utf8(data: Bytes) -> bool uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fjson_2fwrite_5ftext_5finto"></a>

- `@preserves_unique(builder) pub fn json.write_text_into(text: String, mut builder: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)


## json_access

<a id="def-std_401_2fjson_5faccess_2fas_5farray"></a>

- `pub fn json_access.as_array(value: json.Value) -> Option[List[json.Value]] in value uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fas_5fbool"></a>

- `pub fn json_access.as_bool(value: json.Value) -> Option[bool] uses {}`

Bool payload only. The plain bool result has no source lifetime restriction.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fas_5fobject"></a>

- `pub fn json_access.as_object(value: json.Value) -> Option[Map[String, json.Value]] in value uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fas_5ftext"></a>

- `pub fn json_access.as_text(value: json.Value) -> Option[String] in value uses {}`

Text payload only; booleans and numbers are never formatted as text.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2felement"></a>

- `pub fn json_access.element(value: json.Value, index: u64) -> Option[json.Value] in value uses {}`

Select a zero-based array element; wrong variants and out-of-range return None().

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fis_5fnull"></a>

- `pub fn json_access.is_null(value: json.Value) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fmember"></a>

- `pub fn json_access.member(value: json.Value, key: String) -> Option[json.Value] in value uses {}`

Select an object member; present null is Some(Null()), absence is None().

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fjson_5faccess_2fnumber_5ftext"></a>

- `pub fn json_access.number_text(value: json.Value) -> Option[String] in value uses {}`

Number payload only, retaining its exact lexical spelling without parsing.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)


## json_document

<a id="def-std_401_2fjson_5fdocument_2fDocument"></a>

- `pub opaque type json_document.Document: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fjson_5fdocument_2fKind"></a>

- `pub type json_document.Kind = Null | Bool | Number | Text | Array | Object`

<a id="def-std_401_2fjson_5fdocument_2fKind_23Array"></a>

- `Array -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fKind_23Bool"></a>

- `Bool -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fKind_23Null"></a>

- `Null -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fKind_23Number"></a>

- `Number -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fKind_23Object"></a>

- `Object -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fKind_23Text"></a>

- `Text -> json_document.Kind`

Variant of json_document.Kind.

<a id="def-std_401_2fjson_5fdocument_2fbool_5fvalue"></a>

- `pub fn json_document.bool_value(doc: Document, at: u64) -> Option[bool] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fcount"></a>

- `pub fn json_document.count(doc: Document, at: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2ffirst_5fchild"></a>

- `pub fn json_document.first_child(doc: Document, at: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fkey_5fbytes"></a>

- `pub fn json_document.key_bytes(doc: Document, at: u64) -> Option[Bytes] in doc uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fkind"></a>

- `pub fn json_document.kind(doc: Document, at: u64) -> Option[Kind] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument), [`std@1/json_document/Kind`](#def-std_401_2fjson_5fdocument_2fKind)

<a id="def-std_401_2fjson_5fdocument_2fmember"></a>

- `pub fn json_document.member(doc: Document, at: u64, key: String) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fnext_5fsibling"></a>

- `pub fn json_document.next_sibling(doc: Document, at: u64) -> Option[u64] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fnumber_5fbytes"></a>

- `pub fn json_document.number_bytes(doc: Document, at: u64) -> Option[Bytes] in doc uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2froot"></a>

- `pub fn json_document.root(doc: Document) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fstringify"></a>

- `pub fn json_document.stringify(doc: Document) -> Result[unique String, json.JsonError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2ftext_5fbytes"></a>

- `pub fn json_document.text_bytes(doc: Document, at: u64) -> Option[Bytes] in doc uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2ftext_5fowned"></a>

- `pub fn json_document.text_owned(doc: Document, at: u64) -> Option[String] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fto_5fowned"></a>

- `pub fn json_document.to_owned(doc: Document, at: u64) -> Option[json.Value] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)

<a id="def-std_401_2fjson_5fdocument_2fwith_5fdocument"></a>

- `pub fn json_document.with_document[R](take input: String, body: fn(doc: Document) -> R) -> Result[ R, json.JsonError ] uses {body}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/json_document/Document`](#def-std_401_2fjson_5fdocument_2fDocument)


## json_merge_patch

<a id="def-std_401_2fjson_5fmerge_5fpatch_2fMergeError"></a>

- `@serializable pub type json_merge_patch.MergeError = Depth(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fmerge_5fpatch_2fMergeError_23Depth"></a>

- `Depth(u64) -> json_merge_patch.MergeError`

Variant of json_merge_patch.MergeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fmerge_5fpatch_2fapply"></a>

- `pub fn json_merge_patch.apply(take target: json.Value, patch: json.Value, max_depth: u64) -> Result[ json.Value, MergeError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_merge_patch/MergeError`](#def-std_401_2fjson_5fmerge_5fpatch_2fMergeError)


## json_patch

<a id="def-std_401_2fjson_5fpatch_2fErrorKind"></a>

- `@serializable pub type json_patch.ErrorKind = LimitsError | BytesLimit | OperationsLimit | DepthLimit | TokensLimit | WorkLimit | InvalidJson( json.JsonError ) | Shape | Field(String) | UnknownOperation | InvalidPath(u64) | MissingTarget | MissingSource | InvalidIndex | MoveDescendant | TestFailed | InvalidNumber`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23BytesLimit"></a>

- `BytesLimit -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23DepthLimit"></a>

- `DepthLimit -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23Field"></a>

- `Field(String) -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23InvalidIndex"></a>

- `InvalidIndex -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23InvalidJson"></a>

- `InvalidJson( json.JsonError ) -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23InvalidNumber"></a>

- `InvalidNumber -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23InvalidPath"></a>

- `InvalidPath(u64) -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23LimitsError"></a>

- `LimitsError -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23MissingSource"></a>

- `MissingSource -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23MissingTarget"></a>

- `MissingTarget -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23MoveDescendant"></a>

- `MoveDescendant -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23OperationsLimit"></a>

- `OperationsLimit -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23Shape"></a>

- `Shape -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23TestFailed"></a>

- `TestFailed -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23TokensLimit"></a>

- `TokensLimit -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23UnknownOperation"></a>

- `UnknownOperation -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fErrorKind_23WorkLimit"></a>

- `WorkLimit -> json_patch.ErrorKind`

Variant of json_patch.ErrorKind.

<a id="def-std_401_2fjson_5fpatch_2fLimits"></a>

- `pub type json_patch.Limits { max_bytes: u64, max_operations: u64, max_depth: u64, max_tokens: u64, max_work: u64, }`

Limits are inclusive. Hard ceilings: 16 MiB, 4096 operations, depth 128,
128 tokens/path, 100 million work units. Invalid limits return LimitsError.
Bytes count UTF-8 payload bytes plus one per node (and object key bytes).
parse_text additionally bounds the complete encoded input before parsing.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fpatch_2fOperation"></a>

- `pub type json_patch.Operation = Add(Path, json.Value) | Remove(Path) | Replace(Path, json.Value) | Move( Path, Path ) | Copy(Path, Path) | Test(Path, json.Value)`

Move and Copy carry destination first, then source; other paths are targets.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Add"></a>

- `Add(Path, json.Value) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Copy"></a>

- `Copy(Path, Path) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Move"></a>

- `Move( Path, Path ) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Remove"></a>

- `Remove(Path) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Replace"></a>

- `Replace(Path, json.Value) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fOperation_23Test"></a>

- `Test(Path, json.Value) -> json_patch.Operation`

Variant of json_patch.Operation.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)

<a id="def-std_401_2fjson_5fpatch_2fPatch"></a>

- `pub opaque type json_patch.Patch: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fjson_5fpatch_2fPatchError"></a>

- `@serializable pub type json_patch.PatchError { operation: u64, path: String, kind: ErrorKind, }`

operation is zero-based. Errors before operation dispatch use operation 0
and path "". For source failures path is the original `from` string.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json_patch/ErrorKind`](#def-std_401_2fjson_5fpatch_2fErrorKind)

<a id="def-std_401_2fjson_5fpatch_2fPath"></a>

- `pub opaque type json_patch.Path: copyable + inert + noalloc_drop uses {}`

Validated RFC6901 path. Both the shipped pointer and decoded mutation tokens
are owned; no source JSON or source text is borrowed by a parsed patch.

<a id="def-std_401_2fjson_5fpatch_2fapply"></a>

- `pub fn json_patch.apply(document: Option[json.Value], patch: Patch, limits: Limits) -> Result[ Option[json.Value], PatchError ] uses {}`

Borrows both inputs and publishes only a completely successful owned result.
None is document absence, distinct from Some(Null()). Only root add creates
an absent document. An empty patch preserves absence. Root remove returns
None; later root add may recreate it. All failures leave both inputs intact.
Each call has one work budget across preflight, copies, edits and tests.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Limits`](#def-std_401_2fjson_5fpatch_2fLimits), [`std@1/json_patch/Patch`](#def-std_401_2fjson_5fpatch_2fPatch), [`std@1/json_patch/PatchError`](#def-std_401_2fjson_5fpatch_2fPatchError)

<a id="def-std_401_2fjson_5fpatch_2fdefault_5flimits"></a>

- `pub fn json_patch.default_limits() -> Limits uses {}`

References: [`std@1/json_patch/Limits`](#def-std_401_2fjson_5fpatch_2fLimits)

<a id="def-std_401_2fjson_5fpatch_2foperations"></a>

- `pub fn json_patch.operations(patch: Patch) -> List[Operation] in patch uses {}`

Borrows the typed operation sequence from a parsed patch.

References: [`core@1/List`](#def-core_401_2fList), [`std@1/json_patch/Operation`](#def-std_401_2fjson_5fpatch_2fOperation), [`std@1/json_patch/Patch`](#def-std_401_2fjson_5fpatch_2fPatch)

<a id="def-std_401_2fjson_5fpatch_2fparse"></a>

- `pub fn json_patch.parse(take value: json.Value, limits: Limits) -> Result[Patch, PatchError] uses {}`

Validates the complete owned patch list before application. Extra operation
members are ignored, as RFC6902 requires. Duplicate keys are json.parse errors.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_patch/Limits`](#def-std_401_2fjson_5fpatch_2fLimits), [`std@1/json_patch/Patch`](#def-std_401_2fjson_5fpatch_2fPatch), [`std@1/json_patch/PatchError`](#def-std_401_2fjson_5fpatch_2fPatchError)

<a id="def-std_401_2fjson_5fpatch_2fparse_5ftext"></a>

- `pub fn json_patch.parse_text(take text: String, limits: Limits) -> Result[Patch, PatchError] uses {}`

Bounds raw UTF-8 bytes and parser work before calling the shipped JSON parser.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json_patch/Limits`](#def-std_401_2fjson_5fpatch_2fLimits), [`std@1/json_patch/Patch`](#def-std_401_2fjson_5fpatch_2fPatch), [`std@1/json_patch/PatchError`](#def-std_401_2fjson_5fpatch_2fPatchError)

<a id="def-std_401_2fjson_5fpatch_2fpath_5ftext"></a>

- `pub fn json_patch.path_text(selected: Path) -> String in selected uses {}`

Borrows a path's original encoded RFC6901 text.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json_patch/Path`](#def-std_401_2fjson_5fpatch_2fPath)


## json_pointer

<a id="def-std_401_2fjson_5fpointer_2fPointer"></a>

- `pub opaque type json_pointer.Pointer: copyable + inert + noalloc_drop uses {}`

Owned decoded RFC6901 reference tokens. No source-text borrow is retained.

<a id="def-std_401_2fjson_5fpointer_2fPointerError"></a>

- `pub type json_pointer.PointerError = MissingSlash(u64) | InvalidEscape(u64)`

Zero-based UTF-8 byte offsets in the original pointer. InvalidEscape points
at the tilde, including a trailing tilde. Fragment form is not accepted.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fpointer_2fPointerError_23InvalidEscape"></a>

- `InvalidEscape(u64) -> json_pointer.PointerError`

Variant of json_pointer.PointerError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fpointer_2fPointerError_23MissingSlash"></a>

- `MissingSlash(u64) -> json_pointer.PointerError`

Variant of json_pointer.PointerError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjson_5fpointer_2fget"></a>

- `pub fn json_pointer.get(value: json.Value, pointer: Pointer) -> Option[json.Value] in value uses {}`

Borrows the selected value from value, never from pointer. Empty selects
the complete value. Missing members, invalid/missing array indexes and
scalar descent return None. No mutation or deep copy is performed.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/json_pointer/Pointer`](#def-std_401_2fjson_5fpointer_2fPointer)

<a id="def-std_401_2fjson_5fpointer_2fparse"></a>

- `pub fn json_pointer.parse(pointer: String) -> Result[Pointer, PointerError] uses {}`

Parses the string form (not a URI fragment). Decode each escape once:
~01 is the literal token ~1. Every token owns its UTF-8 bytes.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json_pointer/Pointer`](#def-std_401_2fjson_5fpointer_2fPointer), [`std@1/json_pointer/PointerError`](#def-std_401_2fjson_5fpointer_2fPointerError)


## jwt

<a id="def-std_401_2fjwt_2fJwtError"></a>

- `pub type jwt.JwtError = InvalidKey | Malformed | Json(json.JsonError) | UnsupportedAlgorithm( String ) | Critical | Signature | Expired | NotYetValid | Audience | Issuer | MissingExpiration | InvalidClaim( String ) | Clock(ClockError)`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/String`](#def-core_401_2fString), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjwt_2fJwtError_23Audience"></a>

- `Audience -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23Clock"></a>

- `Clock(ClockError) -> jwt.JwtError`

Variant of jwt.JwtError.

References: [`core@1/ClockError`](#def-core_401_2fClockError)

<a id="def-std_401_2fjwt_2fJwtError_23Critical"></a>

- `Critical -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23Expired"></a>

- `Expired -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23InvalidClaim"></a>

- `InvalidClaim( String ) -> jwt.JwtError`

Variant of jwt.JwtError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjwt_2fJwtError_23InvalidKey"></a>

- `InvalidKey -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23Issuer"></a>

- `Issuer -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23Json"></a>

- `Json(json.JsonError) -> jwt.JwtError`

Variant of jwt.JwtError.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fjwt_2fJwtError_23Malformed"></a>

- `Malformed -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23MissingExpiration"></a>

- `MissingExpiration -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23NotYetValid"></a>

- `NotYetValid -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23Signature"></a>

- `Signature -> jwt.JwtError`

Variant of jwt.JwtError.

<a id="def-std_401_2fjwt_2fJwtError_23UnsupportedAlgorithm"></a>

- `UnsupportedAlgorithm( String ) -> jwt.JwtError`

Variant of jwt.JwtError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fjwt_2fKey"></a>

- `pub type jwt.Key = HS256(Bytes) | HS384(Bytes) | HS512(Bytes) | EdDSA(ed25519.PublicKey) | EdDSAPrivate( ed25519.PrivateKey )`

A verification or signing key and the algorithm it is for.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/ed25519/PrivateKey`](#def-std_401_2fed25519_2fPrivateKey), [`std@1/ed25519/PublicKey`](#def-std_401_2fed25519_2fPublicKey)

<a id="def-std_401_2fjwt_2fKey_23EdDSA"></a>

- `EdDSA(ed25519.PublicKey) -> jwt.Key`

Variant of jwt.Key.

References: [`std@1/ed25519/PublicKey`](#def-std_401_2fed25519_2fPublicKey)

<a id="def-std_401_2fjwt_2fKey_23EdDSAPrivate"></a>

- `EdDSAPrivate( ed25519.PrivateKey ) -> jwt.Key`

Variant of jwt.Key.

References: [`std@1/ed25519/PrivateKey`](#def-std_401_2fed25519_2fPrivateKey)

<a id="def-std_401_2fjwt_2fKey_23HS256"></a>

- `HS256(Bytes) -> jwt.Key`

Variant of jwt.Key.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fjwt_2fKey_23HS384"></a>

- `HS384(Bytes) -> jwt.Key`

Variant of jwt.Key.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fjwt_2fKey_23HS512"></a>

- `HS512(Bytes) -> jwt.Key`

Variant of jwt.Key.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fjwt_2fValidation"></a>

- `pub type jwt.Validation { audience: Option[String], issuer: Option[String], leeway: u64, require_expiration: bool, }`

Claim checks beyond the signature. `audience` must appear in the `aud` claim (a
string or an array of strings) and `issuer` equal `iss`; `leeway` seconds widen
the `exp` and `nbf` windows; `require_expiration` refuses a token without `exp`.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fjwt_2fdefault_5fvalidation"></a>

- `pub fn jwt.default_validation() -> Validation uses {}`

No audience or issuer, no leeway and `exp` optional, as golang-jwt's defaults.

References: [`std@1/jwt/Validation`](#def-std_401_2fjwt_2fValidation)

<a id="def-std_401_2fjwt_2fheader"></a>

- `pub fn jwt.header(token: String) -> Result[json.Value, JwtError] uses {}`

The decoded JOSE header of `token`, unverified: for choosing a key by `kid`
before `verify`. Never trust it otherwise.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/jwt/JwtError`](#def-std_401_2fjwt_2fJwtError)

<a id="def-std_401_2fjwt_2fsign"></a>

- `pub fn jwt.sign(key: Key, claims: json.Value) -> Result[unique String, JwtError] uses {}`

Signs `claims`, a JSON object, under the header `{"alg":...,"typ":"JWT"}`:
`header.claims.signature`, each segment unpadded base64url. A claims value that is
not an object is `InvalidClaim("claims")`. EdDSA needs `EdDSAPrivate` with a
32-byte seed; a public key or malformed seed is `InvalidKey()`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/jwt/JwtError`](#def-std_401_2fjwt_2fJwtError), [`std@1/jwt/Key`](#def-std_401_2fjwt_2fKey)

<a id="def-std_401_2fjwt_2fvalidate"></a>

- `pub fn jwt.validate(claims: json.Value, validation: Validation, now: i64) -> Result[ (), JwtError ] uses {}`

Checks the claims of a verified token at `now` (Unix seconds), as golang-jwt's
Validator: `now` must be before `exp` plus the leeway (`Expired()`) and not before
`nbf` minus it (`NotYetValid()`), a missing `exp` is `MissingExpiration()` when
required, and `validation`'s audience and issuer must match (`Audience()`,
`Issuer()`). A claim of the wrong JSON type is `InvalidClaim(name)`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/jwt/JwtError`](#def-std_401_2fjwt_2fJwtError), [`std@1/jwt/Validation`](#def-std_401_2fjwt_2fValidation)

<a id="def-std_401_2fjwt_2fverify"></a>

- `pub fn jwt.verify(token: String, key: Key, validation: Validation, clock: WallClock) -> Result[ json.Value, JwtError ]`

`verify_at` at the current second of `clock`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/WallClock`](#def-core_401_2fWallClock), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/jwt/JwtError`](#def-std_401_2fjwt_2fJwtError), [`std@1/jwt/Key`](#def-std_401_2fjwt_2fKey), [`std@1/jwt/Validation`](#def-std_401_2fjwt_2fValidation)

<a id="def-std_401_2fjwt_2fverify_5fat"></a>

- `pub fn jwt.verify_at(token: String, key: Key, validation: Validation, now: i64) -> Result[ json.Value, JwtError ] uses {}`

Verifies `token` with `key` at `now` (Unix seconds) and returns its claims: the
header's `alg` must be the key's (`UnsupportedAlgorithm(alg)` otherwise), a `crit`
header is `Critical()`, the MAC or Ed25519 signature must match (`Signature()`), and
then the claims must pass `validate`. EdDSA uses the public key; a private key
or malformed key is `InvalidKey()`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/jwt/JwtError`](#def-std_401_2fjwt_2fJwtError), [`std@1/jwt/Key`](#def-std_401_2fjwt_2fKey), [`std@1/jwt/Validation`](#def-std_401_2fjwt_2fValidation)


## keys

<a id="def-std_401_2fkeys_2fdecode"></a>

- `pub fn keys.decode[K: serializable](data: Bytes) -> Result[unique K, DecodeError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/DecodeError`](#def-core_401_2fDecodeError), [`core@1/Result`](#def-core_401_2fResult)

<a id="def-std_401_2fkeys_2fencode"></a>

- `pub fn keys.encode[K: serializable](key: K) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fkeys_2fencode_5finto"></a>

- `@preserves_unique(out) pub fn keys.encode_into[K: serializable](key: K, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fkeys_2fprefix_5fend"></a>

- `pub fn keys.prefix_end(prefix: Bytes) -> Option[unique Bytes] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption)


## line_scan

<a id="def-std_401_2fline_5fscan_2fError"></a>

- `@serializable pub type line_scan.Error = InvalidPolicy | LineTooLong(u64) | OffsetOverflow | AlreadyFinished | ScannerFailed`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fline_5fscan_2fError_23AlreadyFinished"></a>

- `AlreadyFinished -> line_scan.Error`

Variant of line_scan.Error.

<a id="def-std_401_2fline_5fscan_2fError_23InvalidPolicy"></a>

- `InvalidPolicy -> line_scan.Error`

Variant of line_scan.Error.

<a id="def-std_401_2fline_5fscan_2fError_23LineTooLong"></a>

- `LineTooLong(u64) -> line_scan.Error`

Variant of line_scan.Error.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fline_5fscan_2fError_23OffsetOverflow"></a>

- `OffsetOverflow -> line_scan.Error`

Variant of line_scan.Error.

<a id="def-std_401_2fline_5fscan_2fError_23ScannerFailed"></a>

- `ScannerFailed -> line_scan.Error`

Variant of line_scan.Error.

<a id="def-std_401_2fline_5fscan_2fScanner"></a>

- `pub opaque type line_scan.Scanner: inert + noalloc_drop uses {}`

<a id="def-std_401_2fline_5fscan_2fStatus"></a>

- `@serializable pub type line_scan.Status = Open | Finished | Poisoned`

<a id="def-std_401_2fline_5fscan_2fStatus_23Finished"></a>

- `Finished -> line_scan.Status`

Variant of line_scan.Status.

<a id="def-std_401_2fline_5fscan_2fStatus_23Open"></a>

- `Open -> line_scan.Status`

Variant of line_scan.Status.

<a id="def-std_401_2fline_5fscan_2fStatus_23Poisoned"></a>

- `Poisoned -> line_scan.Status`

Variant of line_scan.Status.

<a id="def-std_401_2fline_5fscan_2fbuffered"></a>

- `@noalloc pub fn line_scan.buffered(s: Scanner) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/line_scan/Scanner`](#def-std_401_2fline_5fscan_2fScanner)

<a id="def-std_401_2fline_5fscan_2ffeed"></a>

- `pub fn line_scan.feed(mut s: unique Scanner, chunk: Bytes, eof: bool) -> Result[ List[Bytes], Error ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/line_scan/Error`](#def-std_401_2fline_5fscan_2fError), [`std@1/line_scan/Scanner`](#def-std_401_2fline_5fscan_2fScanner)

<a id="def-std_401_2fline_5fscan_2fnew"></a>

- `pub fn line_scan.new(max_line_bytes: u64, strip_cr: bool) -> Result[unique Scanner, Error] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/line_scan/Error`](#def-std_401_2fline_5fscan_2fError), [`std@1/line_scan/Scanner`](#def-std_401_2fline_5fscan_2fScanner)

<a id="def-std_401_2fline_5fscan_2foffset"></a>

- `@noalloc pub fn line_scan.offset(s: Scanner) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/line_scan/Scanner`](#def-std_401_2fline_5fscan_2fScanner)

<a id="def-std_401_2fline_5fscan_2fstatus"></a>

- `@noalloc pub fn line_scan.status(s: Scanner) -> Status uses {}`

References: [`std@1/line_scan/Scanner`](#def-std_401_2fline_5fscan_2fScanner), [`std@1/line_scan/Status`](#def-std_401_2fline_5fscan_2fStatus)


## list

<a id="def-std_401_2flist_2fall"></a>

- `@noalloc pub fn list.all[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flist_2fany"></a>

- `@noalloc pub fn list.any[T](xs: List[T], predicate: @noalloc fn(T) -> bool) -> bool uses {xs, predicate}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flist_2fbinary_5fsearch"></a>

- `@noalloc pub fn list.binary_search[T](xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {}) -> Option[ u64 ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flist_2fconcat"></a>

- `pub fn list.concat[T](take a: List[T], take b: List[T]) -> List[T] uses {a, b}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2fdedup"></a>

- `pub fn list.dedup[T: data](take xs: List[T]) -> List[T] uses {xs}`

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2flist_2ffilled"></a>

- `pub fn list.filled[T: plain](n: u64, take value: T) -> unique List[T]`

Reserves for the count, then copies the value into each initialized element.
Packed elements occupy their active variant's width. Reservation uses the
smallest width, so larger variants can require amortized backing growth.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flist_2ffilled_5fcopyable"></a>

- `pub fn list.filled_copyable[T: copyable](n: u64, take value: T) -> List[T] uses {value}`

Copies of counted seeds share their owners under copy-on-write.
Reserves for the count, then copies the value into each initialized element.
Packed elements occupy their active variant's width. Reservation uses the
smallest width, so larger variants can require amortized backing growth.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64)

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

Sorts in place without allocation. Equal elements may change order.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2flist_2fsort_5fstable"></a>

- `@noalloc pub fn list.sort_stable[T](mut xs: unique List[T], by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {}`

Stable, allocation-free WikiSort. Equal elements retain their input order.
O(n log n) time and O(1) auxiliary space, including move-only elements.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2flist_2fsort_5fstable_5ffast"></a>

- `pub fn list.sort_stable_fast[T](mut xs: unique List[T], by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {}`

Stable driftsort with bounded O(n) scratch, including move-only owners.
Choose sort_stable_plain for plain elements without index indirection;
choose sort_stable for its @noalloc guarantee. Equal keys retain order.
Above 20 elements, comparisons finish before elements move. Smaller inputs
use stable insertion by swaps; comparator failures propagate immediately.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2flist_2fsort_5fstable_5fplain"></a>

- `pub fn list.sort_stable_plain[T: plain](mut xs: unique List[T], by: @noalloc fn(T, T) -> i32 uses {}) -> () uses {}`

Stable driftsort for plain elements, with direct comparisons and O(n)
scratch. Choose this for plain T; sort_stable_fast also accepts move-only
and counted copyable elements by sorting indices. Choose sort_stable for its @noalloc guarantee.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-core_401_2flist_2fswap"></a>

- `@noalloc fn list.swap[T](mut xs: unique List[T], a: u64, b: u64) -> ()`

<a id="def-core_401_2flist_2ftruncate"></a>

- `@preserves_unique(xs) fn list.truncate[T](mut xs: List[T], len: u64) -> () uses {xs}`


## log

<a id="def-std_401_2flog_2fEntry"></a>

- `@serializable pub type log.Entry { level: Level, message: String, fields: List[Field], timestamp: Option[time.Timestamp], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/log/Field`](#def-std_401_2flog_2fField), [`std@1/log/Level`](#def-std_401_2flog_2fLevel), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2flog_2fField"></a>

- `@serializable pub type log.Field { name: String, value: Value, }`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/log/Value`](#def-std_401_2flog_2fValue)

<a id="def-std_401_2flog_2fLevel"></a>

- `@serializable pub type log.Level = Debug | Info | Warn | Error`

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

<a id="def-std_401_2flog_2fValue"></a>

- `@serializable pub type log.Value = Null | Text(String) | Int(i64) | Uint(u64) | Float(f64) | Bool(bool) | Duration( time.Duration ) | Timestamp(time.Timestamp) | Json(json.Value) | Failure(String) | Severity(Level)`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/f64`](#def-core_401_2ff64), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/log/Level`](#def-std_401_2flog_2fLevel), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2flog_2fValue_23Bool"></a>

- `Bool(bool) -> log.Value`

Variant of log.Value.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2flog_2fValue_23Duration"></a>

- `Duration( time.Duration ) -> log.Value`

Variant of log.Value.

References: [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2flog_2fValue_23Failure"></a>

- `Failure(String) -> log.Value`

Variant of log.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2flog_2fValue_23Float"></a>

- `Float(f64) -> log.Value`

Variant of log.Value.

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2flog_2fValue_23Int"></a>

- `Int(i64) -> log.Value`

Variant of log.Value.

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2flog_2fValue_23Json"></a>

- `Json(json.Value) -> log.Value`

Variant of log.Value.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2flog_2fValue_23Null"></a>

- `Null -> log.Value`

Variant of log.Value.

<a id="def-std_401_2flog_2fValue_23Severity"></a>

- `Severity(Level) -> log.Value`

Variant of log.Value.

References: [`std@1/log/Level`](#def-std_401_2flog_2fLevel)

<a id="def-std_401_2flog_2fValue_23Text"></a>

- `Text(String) -> log.Value`

Variant of log.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2flog_2fValue_23Timestamp"></a>

- `Timestamp(time.Timestamp) -> log.Value`

Variant of log.Value.

References: [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2flog_2fValue_23Uint"></a>

- `Uint(u64) -> log.Value`

Variant of log.Value.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flog_2fto_5fstring"></a>

- `pub fn log.to_string(entry: Entry) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/log/Entry`](#def-std_401_2flog_2fEntry)

<a id="def-std_401_2flog_2fwrite"></a>

- `pub fn log.write(sink: Stderr, minimum: Level, entry: Entry) -> Result[(), OutputError]`

References: [`core@1/OutputError`](#def-core_401_2fOutputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stderr`](#def-core_401_2fStderr), [`std@1/log/Entry`](#def-std_401_2flog_2fEntry), [`std@1/log/Level`](#def-std_401_2flog_2fLevel)


## lru

<a id="def-std_401_2flru_2fCache"></a>

- `pub opaque type lru.Cache[T]: () uses {T}`

<a id="def-std_401_2flru_2fCapacityError"></a>

- `pub type lru.CapacityError { requested: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2flru_2fEntry"></a>

- `pub type lru.Entry[T] { key: unique String, value: unique T, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2flru_2fPutResult"></a>

- `pub type lru.PutResult[T] = Inserted | Replaced(unique Entry[T]) | Evicted(unique Entry[T])`

References: [`std@1/lru/Entry`](#def-std_401_2flru_2fEntry)

<a id="def-std_401_2flru_2fPutResult_23Evicted"></a>

- `Evicted(unique Entry[T]) -> lru.PutResult[T]`

Variant of lru.PutResult.

References: [`std@1/lru/Entry`](#def-std_401_2flru_2fEntry)

<a id="def-std_401_2flru_2fPutResult_23Inserted"></a>

- `Inserted -> lru.PutResult[T]`

Variant of lru.PutResult.

<a id="def-std_401_2flru_2fPutResult_23Replaced"></a>

- `Replaced(unique Entry[T]) -> lru.PutResult[T]`

Variant of lru.PutResult.

References: [`std@1/lru/Entry`](#def-std_401_2flru_2fEntry)

<a id="def-std_401_2flru_2fcapacity"></a>

- `@noalloc pub fn lru.capacity[T](c: Cache[T]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache)

<a id="def-std_401_2flru_2flen"></a>

- `@noalloc pub fn lru.len[T](c: Cache[T]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache)

<a id="def-std_401_2flru_2fnew"></a>

- `pub fn lru.new[T](max_entries: u64) -> unique Result[Cache[T], CapacityError] uses {}`

Capacity counts entries, independently of key and value byte sizes.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache), [`std@1/lru/CapacityError`](#def-std_401_2flru_2fCapacityError)

<a id="def-std_401_2flru_2fpeek"></a>

- `pub fn lru.peek[T: copyable](c: Cache[T], key: String) -> Option[T] in c uses {}`

A view tied to c; does not change recency or deep-copy the payload.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache)

<a id="def-std_401_2flru_2fput"></a>

- `pub fn lru.put[T](mut c: unique Cache[T], take key: unique String, take value: unique T) -> unique PutResult[ T ] uses {c, value}`

Consumes value; replacement and eviction return the previous owned entry.
A replacement is MRU, just like an insertion or successful touch.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache), [`std@1/lru/PutResult`](#def-std_401_2flru_2fPutResult)

<a id="def-std_401_2flru_2fremove"></a>

- `pub fn lru.remove[T](mut c: unique Cache[T], key: String) -> unique Option[unique Entry[T]] uses {c}`

Transfers an entry's owners to the caller and clears its reusable slot.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache), [`std@1/lru/Entry`](#def-std_401_2flru_2fEntry)

<a id="def-std_401_2flru_2fslot_5fcount"></a>

- `@noalloc pub fn lru.slot_count[T](c: Cache[T]) -> u64 uses {}`

Allocated logical slots, including empty reusable slots; never exceeds capacity.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache)

<a id="def-std_401_2flru_2ftouch"></a>

- `pub fn lru.touch[T](mut c: unique Cache[T], key: String) -> bool uses {c}`

Reports presence and makes a hit most recently used, without observing its payload.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/lru/Cache`](#def-std_401_2flru_2fCache)


## mail

<a id="def-std_401_2fmail_2fAddress"></a>

- `pub type mail.Address { display_name: String, address: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmail_2fLimits"></a>

- `pub type mail.Limits { max_input_bytes: u64, max_comment_depth: u64, max_addresses: u64, max_output_bytes: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError"></a>

- `pub type mail.MailError = Syntax(u64) | Injection(u64) | UnsupportedCharset(u64) | InvalidUtf8( u64 ) | InputLimit(u64) | NestingLimit(u64) | AddressLimit(u64) | OutputLimit(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23AddressLimit"></a>

- `AddressLimit(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23Injection"></a>

- `Injection(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23InputLimit"></a>

- `InputLimit(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23InvalidUtf8"></a>

- `InvalidUtf8( u64 ) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23NestingLimit"></a>

- `NestingLimit(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23OutputLimit"></a>

- `OutputLimit(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23Syntax"></a>

- `Syntax(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fMailError_23UnsupportedCharset"></a>

- `UnsupportedCharset(u64) -> mail.MailError`

Variant of mail.MailError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmail_2fdefault_5flimits"></a>

- `pub fn mail.default_limits() -> Limits uses {}`

References: [`std@1/mail/Limits`](#def-std_401_2fmail_2fLimits)

<a id="def-std_401_2fmail_2fformat"></a>

- `pub fn mail.format(value: Address, limits: Limits) -> Result[unique String, MailError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/mail/Address`](#def-std_401_2fmail_2fAddress), [`std@1/mail/Limits`](#def-std_401_2fmail_2fLimits), [`std@1/mail/MailError`](#def-std_401_2fmail_2fMailError)

<a id="def-std_401_2fmail_2fformat_5flist"></a>

- `pub fn mail.format_list(values: List[Address], limits: Limits) -> Result[ unique String, MailError ] uses {}`

Canonical mailboxes in order, separated by comma-space. Groups stay flattened.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/mail/Address`](#def-std_401_2fmail_2fAddress), [`std@1/mail/Limits`](#def-std_401_2fmail_2fLimits), [`std@1/mail/MailError`](#def-std_401_2fmail_2fMailError)

<a id="def-std_401_2fmail_2fparse"></a>

- `pub fn mail.parse(value: String, limits: Limits) -> Result[Address, MailError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/mail/Address`](#def-std_401_2fmail_2fAddress), [`std@1/mail/Limits`](#def-std_401_2fmail_2fLimits), [`std@1/mail/MailError`](#def-std_401_2fmail_2fMailError)

<a id="def-std_401_2fmail_2fparse_5flist"></a>

- `pub fn mail.parse_list(value: String, limits: Limits) -> Result[List[Address], MailError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/mail/Address`](#def-std_401_2fmail_2fAddress), [`std@1/mail/Limits`](#def-std_401_2fmail_2fLimits), [`std@1/mail/MailError`](#def-std_401_2fmail_2fMailError)


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

<a id="def-std_401_2fmap_2fupdate"></a>

- `@preserves_unique(m) pub fn map.update[K: key, V]( mut m: unique Map[K, V], k: K, change: fn(mut value: unique V) -> () uses {} ) -> bool uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fmap_2fupdate_5fvalue"></a>

- `pub fn map.update_value[K: key, V: copyable]( mut m: Map[K, V], k: K, change: fn(mut value: V) -> () uses {} ) -> bool uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-core_401_2fmap_2fupdate_5fvalue_5fwith"></a>

- `fn map.update_value_with[K: key, V: copyable](mut m: Map[K, V], k: K, change: fn(mut value: V) -> () uses {}) -> bool uses {}`

<a id="def-core_401_2fmap_2fupdate_5fwith"></a>

- `@preserves_unique(m) @unretained(k, update) fn map.update_with[K: key, V](mut m: unique Map[K, V], k: K, update: fn(mut value: unique V) -> () uses {}) -> bool`

<a id="def-std_401_2fmap_2fupsert"></a>

- `@preserves_unique(m) pub fn map.upsert[K: key, V]( mut m: unique Map[K, V], k: K, initial: fn() -> unique V uses {}, change: fn(mut value: unique V) -> () uses {} ) -> () uses {}`

References: [`core@1/Map`](#def-core_401_2fMap)

<a id="def-core_401_2fmap_2fupsert_5fwith"></a>

- `@preserves_unique(m) @unretained(k, initial, update) fn map.upsert_with[K: key, V](mut m: unique Map[K, V], k: K, initial: fn() -> unique V uses {}, update: fn(mut value: unique V) -> () uses {}) -> ()`


## mapped

<a id="def-core_401_2fmapped_2fwith"></a>

- `fn mapped.with[R](d: Dir, m: Mapped of d, body: fn(view: Bytes) -> R) -> Result[R, FileError]`


## markdown

<a id="def-std_401_2fmarkdown_2fByteRange"></a>

- `pub type markdown.ByteRange { start: u64, end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fKind"></a>

- `pub type markdown.Kind = Document | Paragraph | Heading(u64) | BlockQuote | ListBlock( bool, u64, bool ) | Item(Option[bool]) | CodeBlock(String, String) | HtmlBlock(String) | ThematicBreak | Text( String ) | SoftBreak | HardBreak | Code(String) | HtmlInline(String) | Emphasis | Strong | Link( String, String ) | Image(String, String) | Strikethrough | Table | TableRow(bool) | TableCell(u8) | FootnoteDefinition( String ) | FootnoteReference(String, u64, u64) | FootnoteBackrefs(String, u64, u64)`

Block and inline payloads. Table alignment: 0 none, 1 left, 2 center, 3 right.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fmarkdown_2fKind_23BlockQuote"></a>

- `BlockQuote -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Code"></a>

- `Code(String) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23CodeBlock"></a>

- `CodeBlock(String, String) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23Document"></a>

- `Document -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Emphasis"></a>

- `Emphasis -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23FootnoteBackrefs"></a>

- `FootnoteBackrefs(String, u64, u64) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fKind_23FootnoteDefinition"></a>

- `FootnoteDefinition( String ) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23FootnoteReference"></a>

- `FootnoteReference(String, u64, u64) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fKind_23HardBreak"></a>

- `HardBreak -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Heading"></a>

- `Heading(u64) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fKind_23HtmlBlock"></a>

- `HtmlBlock(String) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23HtmlInline"></a>

- `HtmlInline(String) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23Image"></a>

- `Image(String, String) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23Item"></a>

- `Item(Option[bool]) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fmarkdown_2fKind_23Link"></a>

- `Link( String, String ) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23ListBlock"></a>

- `ListBlock( bool, u64, bool ) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fKind_23Paragraph"></a>

- `Paragraph -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23SoftBreak"></a>

- `SoftBreak -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Strikethrough"></a>

- `Strikethrough -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Strong"></a>

- `Strong -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23Table"></a>

- `Table -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fKind_23TableCell"></a>

- `TableCell(u8) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fmarkdown_2fKind_23TableRow"></a>

- `TableRow(bool) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fmarkdown_2fKind_23Text"></a>

- `Text( String ) -> markdown.Kind`

Variant of markdown.Kind.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmarkdown_2fKind_23ThematicBreak"></a>

- `ThematicBreak -> markdown.Kind`

Variant of markdown.Kind.

<a id="def-std_401_2fmarkdown_2fNode"></a>

- `pub type markdown.Node { kind: Kind, children: List[Node], }`

An owned, editable tree. HTML rendering depends only on this AST.

References: [`core@1/List`](#def-core_401_2fList), [`std@1/markdown/Kind`](#def-std_401_2fmarkdown_2fKind)

<a id="def-std_401_2fmarkdown_2fNodeChildren"></a>

- `pub opaque type markdown.NodeChildren: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fmarkdown_2fNodeView"></a>

- `@loop_source(node_children_open, node_children_next) pub opaque type markdown.NodeView: copyable + inert + noalloc_drop uses {}`

An unforgeable local borrow of one document, never an independently usable ID.

<a id="def-std_401_2fmarkdown_2fOptions"></a>

- `pub type markdown.Options { tables: bool, strikethrough: bool, autolinks: bool, task_lists: bool, footnotes: bool, allow_unsafe: bool, tagfilter: bool, }`

Extensions are opt-in. Rendering omits raw HTML and unsafe destinations by default.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fmarkdown_2fOrigin"></a>

- `pub type markdown.Origin = Cover(ByteRange) | Synthetic(SyntheticReason, Option[ByteRange]) | Unavailable( UnavailableReason )`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/markdown/ByteRange`](#def-std_401_2fmarkdown_2fByteRange), [`std@1/markdown/SyntheticReason`](#def-std_401_2fmarkdown_2fSyntheticReason), [`std@1/markdown/UnavailableReason`](#def-std_401_2fmarkdown_2fUnavailableReason)

<a id="def-std_401_2fmarkdown_2fOrigin_23Cover"></a>

- `Cover(ByteRange) -> markdown.Origin`

Variant of markdown.Origin.

References: [`std@1/markdown/ByteRange`](#def-std_401_2fmarkdown_2fByteRange)

<a id="def-std_401_2fmarkdown_2fOrigin_23Synthetic"></a>

- `Synthetic(SyntheticReason, Option[ByteRange]) -> markdown.Origin`

Variant of markdown.Origin.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/markdown/ByteRange`](#def-std_401_2fmarkdown_2fByteRange), [`std@1/markdown/SyntheticReason`](#def-std_401_2fmarkdown_2fSyntheticReason)

<a id="def-std_401_2fmarkdown_2fOrigin_23Unavailable"></a>

- `Unavailable( UnavailableReason ) -> markdown.Origin`

Variant of markdown.Origin.

References: [`std@1/markdown/UnavailableReason`](#def-std_401_2fmarkdown_2fUnavailableReason)

<a id="def-std_401_2fmarkdown_2fOutputLimit"></a>

- `pub type markdown.OutputLimit { limit: u64, }`

The bounded String helpers return no partial output on refusal.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fParseBudget"></a>

- `pub type markdown.ParseBudget { source_bytes: u64, work_items: u64, nesting: u64, }`

Independent original-source, cumulative parser-record and nesting bounds.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fParseLimit"></a>

- `pub type markdown.ParseLimit = SourceBytes | WorkItems | Nesting`

Operational refusal; Markdown grammar itself remains total.

<a id="def-std_401_2fmarkdown_2fParseLimit_23Nesting"></a>

- `Nesting -> markdown.ParseLimit`

Variant of markdown.ParseLimit.

<a id="def-std_401_2fmarkdown_2fParseLimit_23SourceBytes"></a>

- `SourceBytes -> markdown.ParseLimit`

Variant of markdown.ParseLimit.

<a id="def-std_401_2fmarkdown_2fParseLimit_23WorkItems"></a>

- `WorkItems -> markdown.ParseLimit`

Variant of markdown.ParseLimit.

<a id="def-std_401_2fmarkdown_2fParsedDocument"></a>

- `pub opaque type markdown.ParsedDocument: copyable + inert + noalloc_drop uses {}`

One immutable record AST, scalar source rows and original line index. No source
String, parser arena or editable Node is retained.

<a id="def-std_401_2fmarkdown_2fRenderError"></a>

- `pub type markdown.RenderError[E] = LimitReached(u64) | Sink(E)`

A render stops before emitting byte limit+1. Sink errors retain their payload.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fRenderError_23LimitReached"></a>

- `LimitReached(u64) -> markdown.RenderError[E]`

Variant of markdown.RenderError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fRenderError_23Sink"></a>

- `Sink(E) -> markdown.RenderError[E]`

Variant of markdown.RenderError.

<a id="def-std_401_2fmarkdown_2fSourceField"></a>

- `pub type markdown.SourceField = Destination | Title | CodeInfo | TaskMarker`

<a id="def-std_401_2fmarkdown_2fSourceField_23CodeInfo"></a>

- `CodeInfo -> markdown.SourceField`

Variant of markdown.SourceField.

<a id="def-std_401_2fmarkdown_2fSourceField_23Destination"></a>

- `Destination -> markdown.SourceField`

Variant of markdown.SourceField.

<a id="def-std_401_2fmarkdown_2fSourceField_23TaskMarker"></a>

- `TaskMarker -> markdown.SourceField`

Variant of markdown.SourceField.

<a id="def-std_401_2fmarkdown_2fSourceField_23Title"></a>

- `Title -> markdown.SourceField`

Variant of markdown.SourceField.

<a id="def-std_401_2fmarkdown_2fSourcePoint"></a>

- `pub type markdown.SourcePoint { line: u64, byte_column: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmarkdown_2fSyntheticReason"></a>

- `pub type markdown.SyntheticReason = InsertedBreak | MissingTableCell | FootnoteBackrefs | FootnoteSpacer | EmptyAutolinkFragment | GeneratedField`

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23EmptyAutolinkFragment"></a>

- `EmptyAutolinkFragment -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23FootnoteBackrefs"></a>

- `FootnoteBackrefs -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23FootnoteSpacer"></a>

- `FootnoteSpacer -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23GeneratedField"></a>

- `GeneratedField -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23InsertedBreak"></a>

- `InsertedBreak -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fSyntheticReason_23MissingTableCell"></a>

- `MissingTableCell -> markdown.SyntheticReason`

Variant of markdown.SyntheticReason.

<a id="def-std_401_2fmarkdown_2fUnavailableReason"></a>

- `pub type markdown.UnavailableReason = DetachedNode | NotPresent | NotApplicable`

<a id="def-std_401_2fmarkdown_2fUnavailableReason_23DetachedNode"></a>

- `DetachedNode -> markdown.UnavailableReason`

Variant of markdown.UnavailableReason.

<a id="def-std_401_2fmarkdown_2fUnavailableReason_23NotApplicable"></a>

- `NotApplicable -> markdown.UnavailableReason`

Variant of markdown.UnavailableReason.

<a id="def-std_401_2fmarkdown_2fUnavailableReason_23NotPresent"></a>

- `NotPresent -> markdown.UnavailableReason`

Variant of markdown.UnavailableReason.

<a id="def-std_401_2fmarkdown_2fdefault_5foptions"></a>

- `pub fn markdown.default_options() -> Options uses {}`

References: [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2ffield_5forigin"></a>

- `pub fn markdown.field_origin(node: NodeView, field: SourceField) -> Origin uses {}`

References: [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView), [`std@1/markdown/Origin`](#def-std_401_2fmarkdown_2fOrigin), [`std@1/markdown/SourceField`](#def-std_401_2fmarkdown_2fSourceField)

<a id="def-std_401_2fmarkdown_2ffirst_5fchild"></a>

- `pub fn markdown.first_child(node: NodeView) -> Option[NodeView] in node uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView)

<a id="def-std_401_2fmarkdown_2fgfm_5foptions"></a>

- `pub fn markdown.gfm_options() -> Options uses {}`

References: [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2finto_5fnode"></a>

- `pub fn markdown.into_node(take document: unique ParsedDocument) -> unique Node uses {}`

References: [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/ParsedDocument`](#def-std_401_2fmarkdown_2fParsedDocument)

<a id="def-std_401_2fmarkdown_2fkind"></a>

- `pub fn markdown.kind(node: NodeView) -> Kind in node uses {}`

References: [`std@1/markdown/Kind`](#def-std_401_2fmarkdown_2fKind), [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView)

<a id="def-std_401_2fmarkdown_2fnext_5fsibling"></a>

- `pub fn markdown.next_sibling(node: NodeView) -> Option[NodeView] in node uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView)

<a id="def-std_401_2fmarkdown_2forigin"></a>

- `pub fn markdown.origin(node: NodeView) -> Origin uses {}`

References: [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView), [`std@1/markdown/Origin`](#def-std_401_2fmarkdown_2fOrigin)

<a id="def-std_401_2fmarkdown_2fparse"></a>

- `pub fn markdown.parse(take source: String, options: Options) -> Node uses {}`

Parse every String, including incomplete Markdown. NUL becomes U+FFFD;
CRLF and bare CR become LF, as required by CommonMark preprocessing.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2fparse_5fbounded"></a>

- `pub fn markdown.parse_bounded(take source: String, options: Options, budget: ParseBudget) -> Result[ Node, ParseLimit ] uses {}`

Refuse before exhausting an original-byte, work-record or nesting allowance.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/ParseBudget`](#def-std_401_2fmarkdown_2fParseBudget), [`std@1/markdown/ParseLimit`](#def-std_401_2fmarkdown_2fParseLimit)

<a id="def-std_401_2fmarkdown_2fparse_5fbounded_5fowned"></a>

- `pub fn markdown.parse_bounded_owned(take source: String, options: Options, budget: ParseBudget) -> Result[ unique Node, ParseLimit ] uses {}`

The same admission points and refusals as parse_bounded, with a deeply unique
successful tree. This removes a consumer's completed-tree ownership copy.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/ParseBudget`](#def-std_401_2fmarkdown_2fParseBudget), [`std@1/markdown/ParseLimit`](#def-std_401_2fmarkdown_2fParseLimit)

<a id="def-std_401_2fmarkdown_2fparse_5flocated_5fbounded"></a>

- `pub fn markdown.parse_located_bounded(take source: String, options: Options, budget: ParseBudget) -> Result[ unique ParsedDocument, ParseLimit ] uses {}`

SourceBytes precedes all metadata allocation. Located-only records consume
the same cumulative work allowance; no refund and no partially located tree.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/ParseBudget`](#def-std_401_2fmarkdown_2fParseBudget), [`std@1/markdown/ParseLimit`](#def-std_401_2fmarkdown_2fParseLimit), [`std@1/markdown/ParsedDocument`](#def-std_401_2fmarkdown_2fParsedDocument)

<a id="def-std_401_2fmarkdown_2fparse_5fowned"></a>

- `pub fn markdown.parse_owned(take source: String, options: Options) -> unique Node uses {}`

Construct a deeply unique tree, independent of the consumed source and
temporary parser records. Payload construction still allocates and copies.
Borrow for validation, then move or freeze the completed tree without copying.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2fpoint"></a>

- `pub fn markdown.point(document: ParsedDocument, offset: u64) -> Option[SourcePoint] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/markdown/ParsedDocument`](#def-std_401_2fmarkdown_2fParsedDocument), [`std@1/markdown/SourcePoint`](#def-std_401_2fmarkdown_2fSourcePoint)

<a id="def-std_401_2fmarkdown_2frender_5fhtml"></a>

- `pub fn markdown.render_html(document: Node) -> String uses {}`

Render an editable AST safely with CommonMark defaults.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode)

<a id="def-std_401_2fmarkdown_2frender_5fhtml_5fbounded"></a>

- `pub fn markdown.render_html_bounded(document: Node, options: Options, limit: u64) -> Result[ String, OutputLimit ] uses {}`

Bound output payload storage, including escapes, image alts, tables and footnotes.
Parser/AST allocations and allocator metadata/size-class rounding are separate.
The output buffer reserves at most limit payload bytes, once, on first emission.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/OutputLimit`](#def-std_401_2fmarkdown_2fOutputLimit)

<a id="def-std_401_2fmarkdown_2frender_5fhtml_5fto"></a>

- `pub fn markdown.render_html_to[S, E]( document: Node, options: Options, limit: u64, mut sink: S, write: fn(mut S, u8) -> Result[(), E] uses {} ) -> Result[u64, RenderError[E]] uses {}`

Stream an AST through the same walker, with no rendered heap fragments.
The limit covers emitted bytes; parser/AST memory and sink-owned storage are separate.
Each callback accepts one byte or returns Error without accepting it. A failure
leaves the already accepted prefix in the caller's sink; no further calls occur.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/RenderError`](#def-std_401_2fmarkdown_2fRenderError)

<a id="def-std_401_2fmarkdown_2frender_5fhtml_5fwith_5foptions"></a>

- `pub fn markdown.render_html_with_options(document: Node, options: Options) -> String uses {}`

Render an AST with explicit safety and GFM tag filter options.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Node`](#def-std_401_2fmarkdown_2fNode), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2froot"></a>

- `pub fn markdown.root(document: ParsedDocument) -> NodeView in document uses {}`

References: [`std@1/markdown/NodeView`](#def-std_401_2fmarkdown_2fNodeView), [`std@1/markdown/ParsedDocument`](#def-std_401_2fmarkdown_2fParsedDocument)

<a id="def-std_401_2fmarkdown_2fsource_5flength"></a>

- `pub fn markdown.source_length(document: ParsedDocument) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/markdown/ParsedDocument`](#def-std_401_2fmarkdown_2fParsedDocument)

<a id="def-std_401_2fmarkdown_2fto_5fhtml"></a>

- `pub fn markdown.to_html(take source: String, options: Options) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions)

<a id="def-std_401_2fmarkdown_2fto_5fhtml_5fbounded"></a>

- `pub fn markdown.to_html_bounded(take source: String, options: Options, limit: u64) -> Result[ String, OutputLimit ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/markdown/Options`](#def-std_401_2fmarkdown_2fOptions), [`std@1/markdown/OutputLimit`](#def-std_401_2fmarkdown_2fOutputLimit)


## md5

<a id="def-std_401_2fmd5_2fState"></a>

- `pub opaque type md5.State: plain + inert uses {}`

<a id="def-std_401_2fmd5_2fdigest"></a>

- `@noalloc pub fn md5.digest(data: Bytes) -> Inline[u8, 16] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fmd5_2fdigest_5fhex"></a>

- `pub fn md5.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmd5_2ffinish"></a>

- `@noalloc pub fn md5.finish(take state: State) -> Inline[u8, 16] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/md5/State`](#def-std_401_2fmd5_2fState)

<a id="def-std_401_2fmd5_2fnew"></a>

- `@noalloc pub fn md5.new() -> State uses {}`

References: [`std@1/md5/State`](#def-std_401_2fmd5_2fState)

<a id="def-std_401_2fmd5_2fupdate"></a>

- `@noalloc pub fn md5.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/md5/State`](#def-std_401_2fmd5_2fState)


## metrics

<a id="def-std_401_2fmetrics_2fAggregate"></a>

- `@serializable pub type metrics.Aggregate { count: u64, unsigned: u64, signed: i64, buckets: List[u64], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmetrics_2fCheckedMeasurement"></a>

- `pub opaque type metrics.CheckedMeasurement: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fmetrics_2fCheckpoint"></a>

- `pub opaque type metrics.Checkpoint: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fmetrics_2fCheckpointData"></a>

- `@serializable pub type metrics.CheckpointData { reducer_version: u64, evidence: EvidenceId, next_ordinal: u64, config: MetricConfig, state: Snapshot, }`

A versioned plain projection, never an encoding of an opaque Registry.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/metrics/EvidenceId`](#def-std_401_2fmetrics_2fEvidenceId), [`std@1/metrics/MetricConfig`](#def-std_401_2fmetrics_2fMetricConfig), [`std@1/metrics/Snapshot`](#def-std_401_2fmetrics_2fSnapshot)

<a id="def-std_401_2fmetrics_2fDescriptor"></a>

- `@serializable pub type metrics.Descriptor { scope: String, version: String, name: String, kind: Kind, unit: String, labels: List[String], bounds: List[u64], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/metrics/Kind`](#def-std_401_2fmetrics_2fKind)

<a id="def-std_401_2fmetrics_2fEvidenceId"></a>

- `@serializable pub type metrics.EvidenceId { capture: String, sha256: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmetrics_2fInstrument"></a>

- `@serializable pub type metrics.Instrument { descriptor: Descriptor, series: List[Series], overflow: Aggregate, }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/metrics/Aggregate`](#def-std_401_2fmetrics_2fAggregate), [`std@1/metrics/Descriptor`](#def-std_401_2fmetrics_2fDescriptor), [`std@1/metrics/Series`](#def-std_401_2fmetrics_2fSeries)

<a id="def-std_401_2fmetrics_2fKind"></a>

- `@serializable pub type metrics.Kind = Counter | UpDownCounter | Gauge | Histogram`

<a id="def-std_401_2fmetrics_2fKind_23Counter"></a>

- `Counter -> metrics.Kind`

Variant of metrics.Kind.

<a id="def-std_401_2fmetrics_2fKind_23Gauge"></a>

- `Gauge -> metrics.Kind`

Variant of metrics.Kind.

<a id="def-std_401_2fmetrics_2fKind_23Histogram"></a>

- `Histogram -> metrics.Kind`

Variant of metrics.Kind.

<a id="def-std_401_2fmetrics_2fKind_23UpDownCounter"></a>

- `UpDownCounter -> metrics.Kind`

Variant of metrics.Kind.

<a id="def-std_401_2fmetrics_2fMeasurement"></a>

- `@serializable pub type metrics.Measurement { descriptor: Descriptor, labels: List[telemetry.Attribute], value: Number, exemplar: Option[telemetry.TraceContext], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/metrics/Descriptor`](#def-std_401_2fmetrics_2fDescriptor), [`std@1/metrics/Number`](#def-std_401_2fmetrics_2fNumber), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2fmetrics_2fMetricConfig"></a>

- `@serializable pub type metrics.MetricConfig { descriptors: List[Descriptor], max_series: u64, max_buckets: u64, max_labels: u64, max_label_bytes: u64, }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/metrics/Descriptor`](#def-std_401_2fmetrics_2fDescriptor)

<a id="def-std_401_2fmetrics_2fMetricError"></a>

- `pub type metrics.MetricError = Config | DescriptorConflict | InvalidDescriptor | InvalidLabels | InvalidValue | Arithmetic | UnknownInstrument | EvidenceMismatch | OrdinalGap | Complete | InvalidCheckpoint`

<a id="def-std_401_2fmetrics_2fMetricError_23Arithmetic"></a>

- `Arithmetic -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23Complete"></a>

- `Complete -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23Config"></a>

- `Config -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23DescriptorConflict"></a>

- `DescriptorConflict -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23EvidenceMismatch"></a>

- `EvidenceMismatch -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23InvalidCheckpoint"></a>

- `InvalidCheckpoint -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23InvalidDescriptor"></a>

- `InvalidDescriptor -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23InvalidLabels"></a>

- `InvalidLabels -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23InvalidValue"></a>

- `InvalidValue -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23OrdinalGap"></a>

- `OrdinalGap -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fMetricError_23UnknownInstrument"></a>

- `UnknownInstrument -> metrics.MetricError`

Variant of metrics.MetricError.

<a id="def-std_401_2fmetrics_2fNumber"></a>

- `@serializable pub type metrics.Number = Unsigned(u64) | Signed(i64)`

References: [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmetrics_2fNumber_23Signed"></a>

- `Signed(i64) -> metrics.Number`

Variant of metrics.Number.

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fmetrics_2fNumber_23Unsigned"></a>

- `Unsigned(u64) -> metrics.Number`

Variant of metrics.Number.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmetrics_2fObservation"></a>

- `pub type metrics.Observation { evidence: EvidenceId, ordinal: u64, measurement: Option[CheckedMeasurement], }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/metrics/CheckedMeasurement`](#def-std_401_2fmetrics_2fCheckedMeasurement), [`std@1/metrics/EvidenceId`](#def-std_401_2fmetrics_2fEvidenceId)

<a id="def-std_401_2fmetrics_2fRegistry"></a>

- `pub opaque type metrics.Registry: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fmetrics_2fSeries"></a>

- `@serializable pub type metrics.Series { labels: List[telemetry.Attribute], aggregate: Aggregate, }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/metrics/Aggregate`](#def-std_401_2fmetrics_2fAggregate), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute)

<a id="def-std_401_2fmetrics_2fSnapshot"></a>

- `@serializable pub type metrics.Snapshot { instruments: List[Instrument], complete: bool, }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`std@1/metrics/Instrument`](#def-std_401_2fmetrics_2fInstrument)

<a id="def-std_401_2fmetrics_2fcheck_5fmeasurement"></a>

- `pub fn metrics.check_measurement(m: Measurement) -> Result[CheckedMeasurement, MetricError] uses {}`

Filters disallowed dimensions before validation. Exemplars never form keys.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/CheckedMeasurement`](#def-std_401_2fmetrics_2fCheckedMeasurement), [`std@1/metrics/Measurement`](#def-std_401_2fmetrics_2fMeasurement), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError)

<a id="def-std_401_2fmetrics_2fcheckpoint"></a>

- `pub fn metrics.checkpoint(r: Registry) -> Checkpoint uses {}`

References: [`std@1/metrics/Checkpoint`](#def-std_401_2fmetrics_2fCheckpoint), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2fcheckpoint_5fdata"></a>

- `pub fn metrics.checkpoint_data(c: Checkpoint) -> Result[CheckpointData, MetricError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/Checkpoint`](#def-std_401_2fmetrics_2fCheckpoint), [`std@1/metrics/CheckpointData`](#def-std_401_2fmetrics_2fCheckpointData), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError)

<a id="def-std_401_2fmetrics_2fcomplete"></a>

- `pub fn metrics.complete(mut r: Registry, evidence: EvidenceId, next_ordinal: u64) -> Result[ (), MetricError ] uses {}`

Call only after the evidence parser confirms its complete final boundary.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/metrics/EvidenceId`](#def-std_401_2fmetrics_2fEvidenceId), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2ffold"></a>

- `pub fn metrics.fold(mut r: Registry, event: Observation) -> Result[(), MetricError] uses {}`

Contiguous zero-based ordinals include nonmetric observations. Repeated
ordinals in the same immutable evidence are no-ops; gaps never silently skip.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Observation`](#def-std_401_2fmetrics_2fObservation), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2fmeasurement_5fdata"></a>

- `pub fn metrics.measurement_data(m: CheckedMeasurement) -> Measurement uses {}`

References: [`std@1/metrics/CheckedMeasurement`](#def-std_401_2fmetrics_2fCheckedMeasurement), [`std@1/metrics/Measurement`](#def-std_401_2fmetrics_2fMeasurement)

<a id="def-std_401_2fmetrics_2frecord"></a>

- `pub fn metrics.record(mut r: Registry, m: CheckedMeasurement) -> Result[(), MetricError] uses {}`

Local state only. Bound offline registries cannot mix unnumbered updates.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/CheckedMeasurement`](#def-std_401_2fmetrics_2fCheckedMeasurement), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2fregistry"></a>

- `pub fn metrics.registry(config: MetricConfig) -> Result[Registry, MetricError] uses {}`

Registration is part of construction, so offline folds cannot grow descriptors.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/MetricConfig`](#def-std_401_2fmetrics_2fMetricConfig), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2frestore"></a>

- `pub fn metrics.restore(c: Checkpoint, evidence: EvidenceId) -> Result[Registry, MetricError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/Checkpoint`](#def-std_401_2fmetrics_2fCheckpoint), [`std@1/metrics/EvidenceId`](#def-std_401_2fmetrics_2fEvidenceId), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2frestore_5fdata"></a>

- `pub fn metrics.restore_data(data: CheckpointData, evidence: EvidenceId) -> Result[ Registry, MetricError ] uses {}`

Revalidate decoded plain state before it can become a live reducer. Restarts
bind the caller's evidence and all limits atomically with the consumed prefix.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/metrics/CheckpointData`](#def-std_401_2fmetrics_2fCheckpointData), [`std@1/metrics/EvidenceId`](#def-std_401_2fmetrics_2fEvidenceId), [`std@1/metrics/MetricError`](#def-std_401_2fmetrics_2fMetricError), [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry)

<a id="def-std_401_2fmetrics_2fsnapshot"></a>

- `pub fn metrics.snapshot(r: Registry) -> Snapshot uses {}`

References: [`std@1/metrics/Registry`](#def-std_401_2fmetrics_2fRegistry), [`std@1/metrics/Snapshot`](#def-std_401_2fmetrics_2fSnapshot)


## msgpack

<a id="def-std_401_2fmsgpack_2fdecode"></a>

- `pub fn msgpack.decode(data: Bytes) -> Result[json.Value, binary.DecodeError] uses {}`

Decode exactly one MessagePack value in the JSON domain. Decimal lexemes
represent wire integers exactly; finite floats use shortest binary64 text.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/binary/DecodeError`](#def-std_401_2fbinary_2fDecodeError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fmsgpack_2fencode"></a>

- `pub fn msgpack.encode(value: json.Value) -> Result[unique Bytes, binary.DecodeError] uses {}`

Encode the JSON domain with shortest integer/length headers, binary64
decimal/exponent numbers, and insertion-ordered objects. Invalid Number
lexemes, nonfinite conversions and integers outside the wire range fail.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/binary/DecodeError`](#def-std_401_2fbinary_2fDecodeError), [`std@1/json/Value`](#def-std_401_2fjson_2fValue)


## multipart

<a id="def-std_401_2fmultipart_2fField"></a>

- `pub type multipart.Field { name: String, filename: Option[String], content_type: Option[String], data: Bytes, }`

One field to encode: a file when `filename` is present, whose content type then
defaults to `application/octet-stream`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmultipart_2fLimits"></a>

- `pub type multipart.Limits { max_parts: u64, max_headers: u64, }`

Go's defaults are 1000 parts and 10000 header fields.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmultipart_2fMultipartError"></a>

- `pub type multipart.MultipartError = MediaType | NotMultipart | MissingBoundary | InvalidBoundary | UnexpectedEof | UnexpectedLine( u64 ) | MalformedHeader(u64) | TooManyParts | TooManyHeaders`

Offsets are byte positions in the decoded body.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmultipart_2fMultipartError_23InvalidBoundary"></a>

- `InvalidBoundary -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23MalformedHeader"></a>

- `MalformedHeader(u64) -> multipart.MultipartError`

Variant of multipart.MultipartError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmultipart_2fMultipartError_23MediaType"></a>

- `MediaType -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23MissingBoundary"></a>

- `MissingBoundary -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23NotMultipart"></a>

- `NotMultipart -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23TooManyHeaders"></a>

- `TooManyHeaders -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23TooManyParts"></a>

- `TooManyParts -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23UnexpectedEof"></a>

- `UnexpectedEof -> multipart.MultipartError`

Variant of multipart.MultipartError.

<a id="def-std_401_2fmultipart_2fMultipartError_23UnexpectedLine"></a>

- `UnexpectedLine( u64 ) -> multipart.MultipartError`

Variant of multipart.MultipartError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fmultipart_2fPart"></a>

- `pub type multipart.Part { headers: List[http.Header], name: Option[String], filename: Option[String], content_type: Option[String], body: Bytes, }`

One decoded part. `name` is the Content-Disposition `name` of a `form-data`
part, `filename` its `filename` without directories, as Go's FileName returns it.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader)

<a id="def-std_401_2fmultipart_2fdecode"></a>

- `pub fn multipart.decode(body: Bytes, boundary: String, limits: Limits) -> Result[ List[Part], MultipartError ] uses {}`

Decodes a multipart body delimited by `boundary` (Go Reader.NextPart over the
whole body): the preamble before the first delimiter and the epilogue after the
final one are ignored, delimiter lines may end in spaces or tabs, and a body
whose first delimiter line ends in LF alone uses LF line ends throughout. More
than `limits.max_parts` parts or `limits.max_headers` header fields in all fails.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/multipart/Limits`](#def-std_401_2fmultipart_2fLimits), [`std@1/multipart/MultipartError`](#def-std_401_2fmultipart_2fMultipartError), [`std@1/multipart/Part`](#def-std_401_2fmultipart_2fPart)

<a id="def-std_401_2fmultipart_2fdecode_5fform"></a>

- `pub fn multipart.decode_form(media: String, body: Bytes, limits: Limits) -> Result[ List[Part], MultipartError ] uses {}`

Decodes a `multipart/form-data` request body from its Content-Type value: a
different media type is `NotMultipart()`, a missing `boundary` parameter
`MissingBoundary()`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/multipart/Limits`](#def-std_401_2fmultipart_2fLimits), [`std@1/multipart/MultipartError`](#def-std_401_2fmultipart_2fMultipartError), [`std@1/multipart/Part`](#def-std_401_2fmultipart_2fPart)

<a id="def-std_401_2fmultipart_2fdefault_5flimits"></a>

- `pub fn multipart.default_limits() -> Limits uses {}`

References: [`std@1/multipart/Limits`](#def-std_401_2fmultipart_2fLimits)

<a id="def-std_401_2fmultipart_2fencode"></a>

- `pub fn multipart.encode(boundary: String, fields: List[Field]) -> Result[ unique Bytes, MultipartError ] uses {}`

Encodes `fields` as Go's Writer does: each part's Content-Disposition (`form-data;
name="..."` with `filename` for a file, quotes and backslashes escaped and CR and
LF percent-encoded) and Content-Type, CRLF line ends and a final delimiter. An
invalid boundary is `InvalidBoundary()`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/multipart/Field`](#def-std_401_2fmultipart_2fField), [`std@1/multipart/MultipartError`](#def-std_401_2fmultipart_2fMultipartError)

<a id="def-std_401_2fmultipart_2fform_5fbody"></a>

- `pub fn multipart.form_body(boundary: String, fields: List[Field]) -> Result[ http.Body, MultipartError ] uses {}`

`fields` encoded with `boundary` as an `http.Body` for `http.client_send`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Body`](#def-std_401_2fhttp_2fBody), [`std@1/multipart/Field`](#def-std_401_2fmultipart_2fField), [`std@1/multipart/MultipartError`](#def-std_401_2fmultipart_2fMultipartError)

<a id="def-std_401_2fmultipart_2fform_5ftype"></a>

- `pub fn multipart.form_type(boundary: String) -> unique String uses {}`

The Content-Type of a form with `boundary`, quoting the boundary when it holds a
tspecial or space.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmultipart_2fparse_5fmedia_5ftype"></a>

- `pub fn multipart.parse_media_type(value: String) -> Result[ (unique String, Map[String, String]), MultipartError ] uses {}`

Splits a media type or disposition value into its lowercased type (`token` or
`token/token`) and parameters, lowercased by name. Parameter values are tokens or
quoted strings; RFC 2231 `name*` values and `name*0`, `name*1*` continuations are
decoded from us-ascii or utf-8. A malformed value or a repeated name with another
value is `MediaType()`.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/multipart/MultipartError`](#def-std_401_2fmultipart_2fMultipartError)

<a id="def-std_401_2fmultipart_2frandom_5fboundary"></a>

- `pub fn multipart.random_boundary(r: Random) -> Result[unique String, RandomError]`

Sixty hex digits from 30 bytes of `r`, as Go's randomBoundary.

References: [`core@1/Random`](#def-core_401_2fRandom), [`core@1/RandomError`](#def-core_401_2fRandomError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fmultipart_2fvalid_5fboundary"></a>

- `pub fn multipart.valid_boundary(boundary: String) -> bool uses {}`

Go SetBoundary's rule: 1 to 70 bytes of letters, digits and `'()+_,-./:=?`, and
spaces except last.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)


## mutex

<a id="def-core_401_2fmutex_2fcreate"></a>

- `fn mutex.create[T: sendable](tasks: Tasks, take v: unique T) -> Mutex[T]`

<a id="def-core_401_2fmutex_2fwith"></a>

- `fn mutex.with[T, R: sendable](tasks: Tasks, m: Mutex[T], body: fn(mut v: unique T) -> R uses {}) -> R`


## net

<a id="def-std_401_2fnet_2fAddrParseError"></a>

- `@serializable pub type net.AddrParseError = Invalid()`

<a id="def-std_401_2fnet_2fAddrParseError_23Invalid"></a>

- `Invalid() -> net.AddrParseError`

Variant of net.AddrParseError.

<a id="def-std_401_2fnet_2fHostError"></a>

- `@serializable pub type net.HostError = NameTooLong | InvalidHostName`

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

<a id="def-std_401_2fnet_2fip_5fto_5fstring"></a>

- `pub fn net.ip_to_string(ip: IpAddr) -> unique String uses {}`

References: [`core@1/IpAddr`](#def-core_401_2fIpAddr), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fnet_2fjoin"></a>

- `fn net.join(n: Net, take r: ReadHalf of n, take w: WriteHalf of n) -> Result[Stream of n, NetError]`

<a id="def-core_401_2fnet_2flisten"></a>

- `fn net.listen(n: Net, addr: SocketAddr) -> Result[Listener of n, NetError]`

`(literal, port, Listen())` or `(literal, port, ListenTls())` for `addr`; port 0 grants an ephemeral port

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

- `@preserves_captures(s, buffer) fn net.read(n: Net, mut s: Stream of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fread_5finto"></a>

- `@preserves_captures(s) @noalloc fn net.read_into[k: size](n: Net, mut s: Stream of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fresolve"></a>

- `fn net.resolve(n: Net, host: String, port: u16) -> Result[List[SocketAddr], NetError]`

a rule naming `(host, port)`, any operation

<a id="def-core_401_2fnet_2fset_5fread_5ftimeout"></a>

- `@preserves_captures(s) @noalloc fn net.set_read_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]`

<a id="def-core_401_2fnet_2fset_5fwrite_5ftimeout"></a>

- `@preserves_captures(s) @noalloc fn net.set_write_timeout(n: Net, mut s: Stream of n, timeout: NetTimeout) -> Result[(), NetError]`

<a id="def-core_401_2fnet_2fshutdown"></a>

- `@preserves_captures(s) @noalloc fn net.shutdown(n: Net, mut s: Stream of n, how: Shutdown) -> Result[(), NetError]`

<a id="def-std_401_2fnet_2fsocket_5faddr_5fto_5fstring"></a>

- `pub fn net.socket_addr_to_string(addr: SocketAddr) -> unique String uses {}`

References: [`core@1/SocketAddr`](#def-core_401_2fSocketAddr), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2fnet_2fsplit"></a>

- `fn net.split(n: Net, take s: Stream of n) -> (ReadHalf of n, WriteHalf of n)`

the stream's own grant

<a id="def-core_401_2fnet_2fudp_5fbind"></a>

- `fn net.udp_bind(n: Net, addr: SocketAddr) -> Result[unique UdpSocket of n, NetError]`

`(literal, port, UdpBind())` for `addr`; port 0 permits an ephemeral port

<a id="def-core_401_2fnet_2funix_5fconnect"></a>

- `fn net.unix_connect(n: Net, path: Path) -> Result[Stream of n, NetError]`

`(path, 0, UnixConnect())`

<a id="def-core_401_2fnet_2funix_5flisten"></a>

- `fn net.unix_listen(n: Net, path: Path) -> Result[Listener of n, NetError]`

`(path, 0, UnixListen())` or `(path, 0, UnixListenRemoveStale())`

<a id="def-std_401_2fnet_2fvalidate_5fhost"></a>

- `@noalloc pub fn net.validate_host(text: String) -> Result[(), HostError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/net/HostError`](#def-std_401_2fnet_2fHostError)

<a id="def-core_401_2fnet_2fwrite"></a>

- `@preserves_captures(s) @noalloc fn net.write(n: Net, mut s: Stream of n, data: Bytes) -> Result[u64, NetError]`

<a id="def-core_401_2fnet_2fwrite_5fall"></a>

- `@preserves_captures(s) @noalloc fn net.write_all(n: Net, mut s: Stream of n, data: Bytes) -> Result[(), NetError]`


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


## ordered_map

<a id="def-std_401_2fordered_5fmap_2fBound"></a>

- `pub type ordered_map.Bound[K] = Unbounded | Included(K) | Excluded(K)`

One end of a key range: no limit, the key itself included, or excluded.

<a id="def-std_401_2fordered_5fmap_2fBound_23Excluded"></a>

- `Excluded(K) -> ordered_map.Bound[K]`

Variant of ordered_map.Bound.

<a id="def-std_401_2fordered_5fmap_2fBound_23Included"></a>

- `Included(K) -> ordered_map.Bound[K]`

Variant of ordered_map.Bound.

<a id="def-std_401_2fordered_5fmap_2fBound_23Unbounded"></a>

- `Unbounded -> ordered_map.Bound[K]`

Variant of ordered_map.Bound.

<a id="def-std_401_2fordered_5fmap_2fDirection"></a>

- `pub type ordered_map.Direction = Ascending | Descending`

The order a range is returned in.

<a id="def-std_401_2fordered_5fmap_2fDirection_23Ascending"></a>

- `Ascending -> ordered_map.Direction`

Variant of ordered_map.Direction.

<a id="def-std_401_2fordered_5fmap_2fDirection_23Descending"></a>

- `Descending -> ordered_map.Direction`

Variant of ordered_map.Direction.

<a id="def-std_401_2fordered_5fmap_2fOrderedMap"></a>

- `pub opaque type ordered_map.OrderedMap[K: copyable, V: copyable]: copyable uses {K, V}`

<a id="def-std_401_2fordered_5fmap_2fcontains"></a>

- `pub fn ordered_map.contains[K: copyable, V: copyable](m: OrderedMap[K, V], key: K) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2fempty"></a>

- `pub fn ordered_map.empty[K: copyable, V: copyable](take by: fn(K, K) -> i32 uses {}) -> OrderedMap[ K, V ] uses {}`

An empty map ordered by `by`, which returns a negative, zero or positive
result as its first argument sorts before, equal to or after its second.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2ffirst"></a>

- `pub fn ordered_map.first[K: copyable, V: copyable](m: OrderedMap[K, V]) -> Option[(K, V)] in m uses {}`

The least entry, or `None()` for an empty map.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2ffold_5frange"></a>

- `pub fn ordered_map.fold_range[K: copyable, V: copyable, A]( m: OrderedMap[K, V], from: Bound[K], to: Bound[K], direction: Direction, take initial: A, step: fn(take A, K, V) -> (A, bool) ) -> A uses {initial, step}`

Folds `step` over the entries whose keys lie between `from` and `to`, in
`direction` order, starting from `initial`. `step` returns the next
accumulator and whether to continue; the walk also ends at the far bound.
It reads entries in place, so collecting them is the caller's choice.

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound), [`std@1/ordered_map/Direction`](#def-std_401_2fordered_5fmap_2fDirection), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2fget"></a>

- `pub fn ordered_map.get[K: copyable, V: copyable](m: OrderedMap[K, V], key: K) -> Option[V] in m uses {}`

The value stored under `key`, or `None()`.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2finsert"></a>

- `pub fn ordered_map.insert[K: copyable, V: copyable]( mut m: OrderedMap[K, V], take key: K, take value: V ) -> () uses {}`

Stores `value` under `key`, replacing any value already there.

References: [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2flast"></a>

- `pub fn ordered_map.last[K: copyable, V: copyable](m: OrderedMap[K, V]) -> Option[(K, V)] in m uses {}`

The greatest entry, or `None()` for an empty map.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2flen"></a>

- `@noalloc pub fn ordered_map.len[K: copyable, V: copyable](m: OrderedMap[K, V]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2fremove"></a>

- `pub fn ordered_map.remove[K: copyable, V: copyable](mut m: OrderedMap[K, V], key: K) -> Option[V] uses {}`

Removes `key` and returns its value, or `None()` when the map lacks it.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)

<a id="def-std_401_2fordered_5fmap_2fvalid"></a>

- `pub fn ordered_map.valid[K: copyable, V: copyable](m: OrderedMap[K, V]) -> bool uses {}`

Whether the tree's invariants hold: keys strictly ordered across the whole
tree, every node within its key bounds, every leaf at one depth and the
count equal to the number of entries. For tests of this module.

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/ordered_map/OrderedMap`](#def-std_401_2fordered_5fmap_2fOrderedMap)


## out

<a id="def-core_401_2fout_2fprint"></a>

- `@noalloc fn out.print(s: Stdout, text: String) -> Result[(), OutputError]`

<a id="def-core_401_2fout_2fwrite"></a>

- `@noalloc fn out.write(s: Stdout, bytes: Bytes) -> Result[(), OutputError]`


## path

<a id="def-std_401_2fpath_2fPathError"></a>

- `@serializable pub type path.PathError = Invalid(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fpath_2fPathError_23Invalid"></a>

- `Invalid(u64) -> path.PathError`

Variant of path.PathError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fpath_2fcomponents"></a>

- `pub fn path.components(p: Path) -> unique List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fpath_2fextension"></a>

- `pub fn path.extension(p: Path) -> Option[unique String] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fpath_2ffilename"></a>

- `pub fn path.filename(p: Path) -> Option[unique String] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Path`](#def-core_401_2fPath), [`core@1/String`](#def-core_401_2fString)

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


## pbkdf2

<a id="def-std_401_2fpbkdf2_2fderive"></a>

- `pub fn pbkdf2.derive[H: implementation std@1/digest.Digest]( password: Bytes, salt: Bytes, iterations: u64, length: u64 ) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)


## peekable

<a id="def-std_401_2fpeekable_2fPeekable"></a>

- `@captures(inner) pub opaque type peekable.Peekable[T]: () uses {T}`

<a id="def-std_401_2fpeekable_2ffill"></a>

- `pub fn peekable.fill[T](mut p: Peekable[T]) -> bool uses {p}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/peekable/Peekable`](#def-std_401_2fpeekable_2fPeekable)

<a id="def-std_401_2fpeekable_2fnew"></a>

- `pub fn peekable.new[T](take it: Iter[T]) -> Peekable[T] uses {it}`

References: [`core@1/Iter`](#def-core_401_2fIter), [`std@1/peekable/Peekable`](#def-std_401_2fpeekable_2fPeekable)

<a id="def-std_401_2fpeekable_2fnext"></a>

- `pub fn peekable.next[T](mut p: Peekable[T]) -> Option[T] uses {p}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/peekable/Peekable`](#def-std_401_2fpeekable_2fPeekable)

<a id="def-std_401_2fpeekable_2fpeek"></a>

- `pub fn peekable.peek[T: copyable](p: Peekable[T]) -> Option[T] in p uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/peekable/Peekable`](#def-std_401_2fpeekable_2fPeekable)


## priority_queue

<a id="def-std_401_2fpriority_5fqueue_2fHeap"></a>

- `pub opaque type priority_queue.Heap[T]: () uses {T}`

<a id="def-std_401_2fpriority_5fqueue_2fempty"></a>

- `pub fn priority_queue.empty[T](take by: @noalloc fn(T, T) -> i32 uses {}) -> unique Heap[T] uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2ffrom_5flist"></a>

- `pub fn priority_queue.from_list[T]( take by: @noalloc fn(T, T) -> i32 uses {}, take input: unique List[unique T] ) -> unique Heap[T] uses {input}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2flen"></a>

- `@noalloc pub fn priority_queue.len[T](h: Heap[T]) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpeek"></a>

- `pub fn priority_queue.peek[T: copyable](h: Heap[T]) -> Option[T] in h uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpop"></a>

- `pub fn priority_queue.pop[T](mut h: unique Heap[T]) -> unique Option[T] uses {h}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpush"></a>

- `pub fn priority_queue.push[T](mut h: unique Heap[T], take value: unique T) -> () uses {h, value}`

References: [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2fpush_5fpop"></a>

- `pub fn priority_queue.push_pop[T](mut h: unique Heap[T], take value: unique T) -> unique T uses {h, value}`

References: [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)

<a id="def-std_401_2fpriority_5fqueue_2freplace_5ftop"></a>

- `pub fn priority_queue.replace_top[T](mut h: unique Heap[T], take value: unique T) -> unique Option[T] uses {h, value}`

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/priority_queue/Heap`](#def-std_401_2fpriority_5fqueue_2fHeap)


## prng

<a id="def-std_401_2fprng_2fGenerator"></a>

- `pub opaque type prng.Generator: inert + noalloc_drop uses {}`

<a id="def-std_401_2fprng_2fSampleError"></a>

- `@serializable pub type prng.SampleError = EmptyRange()`

<a id="def-std_401_2fprng_2fSampleError_23EmptyRange"></a>

- `EmptyRange() -> prng.SampleError`

Variant of prng.SampleError.

<a id="def-std_401_2fprng_2fbelow"></a>

- `@noalloc pub fn prng.below(mut generator: Generator, upper_exclusive: u64) -> Result[u64, SampleError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator), [`std@1/prng/SampleError`](#def-std_401_2fprng_2fSampleError), [`std@1/prng/u64`](#def-std_401_2fprng_2fu64)

<a id="def-std_401_2fprng_2fbytes"></a>

- `pub fn prng.bytes(mut generator: Generator, count: u64) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator), [`std@1/prng/u64`](#def-std_401_2fprng_2fu64)

<a id="def-std_401_2fprng_2fseed"></a>

- `@noalloc pub fn prng.seed(value: u64) -> Generator uses {}`

References: [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator), [`std@1/prng/u64`](#def-std_401_2fprng_2fu64)

<a id="def-std_401_2fprng_2fu64"></a>

- `@noalloc pub fn prng.u64(mut generator: Generator) -> u64 uses {}`

References: [`std@1/prng/Generator`](#def-std_401_2fprng_2fGenerator)


## probe

<a id="def-std_401_2fprobe_2fidentity"></a>

- `@noalloc pub fn probe.identity(value: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64)


## proc

<a id="def-core_401_2fproc_2fnarrow"></a>

- `fn proc.narrow(p: Proc, rules: List[ProcessRule]) -> Result[Proc, ProcessError]`

<a id="def-core_401_2fproc_2fpipeline"></a>

- `fn proc.pipeline(p: Proc, stages: List[Command], stdin: Bytes, max_output: u64, deadline: Option[Deadline]) -> Result[PipelineOutput, ProcessError]`

<a id="def-core_401_2fproc_2frun"></a>

- `fn proc.run(p: Proc, cmd: Command, stdin: Bytes, max_output: u64, deadline: Option[Deadline]) -> Result[ProcessOutput, ProcessError]`

<a id="def-core_401_2fproc_2fspawn"></a>

- `fn proc.spawn(p: Proc, cmd: Command) -> Result[Child of p, ProcessError]`

<a id="def-core_401_2fproc_2fspawn_5fpipeline"></a>

- `fn proc.spawn_pipeline(p: Proc, stages: List[Command], max_stderr: u64) -> Result[ChildGroup of p, ProcessError]`

<a id="def-core_401_2fproc_2fwait_5fgroup"></a>

- `fn proc.wait_group(p: Proc, take g: ChildGroup of p, deadline: Option[Deadline]) -> Result[GroupOutput, ProcessError]`


## process

<a id="def-std_401_2fprocess_2fExitCode"></a>

- `pub opaque type process.ExitCode: plain + inert + noalloc_drop uses {}`

A program's returned exit status, from 0 through 255. Returning it runs
ordinary cleanup; it performs no effect and requires no capability.

<a id="def-std_401_2fprocess_2fFAILURE"></a>

- `pub const process.FAILURE: ExitCode`

Unsuccessful completion (1), without an Error payload report.

References: [`std@1/process/ExitCode`](#def-std_401_2fprocess_2fExitCode)

<a id="def-std_401_2fprocess_2fSUCCESS"></a>

- `pub const process.SUCCESS: ExitCode`

Successful completion (0).

References: [`std@1/process/ExitCode`](#def-std_401_2fprocess_2fExitCode)

<a id="def-std_401_2fprocess_2fexit_5fcode"></a>

- `@noalloc pub fn process.exit_code(n: u8) -> ExitCode uses {}`

Construct an exit status from the portable unsigned byte range.

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/process/ExitCode`](#def-std_401_2fprocess_2fExitCode)

<a id="def-std_401_2fprocess_2fpipefail"></a>

- `@noalloc pub fn process.pipefail(pipeline: PipelineOutput) -> ExitStatus uses {}`

References: [`core@1/ExitStatus`](#def-core_401_2fExitStatus), [`core@1/PipelineOutput`](#def-core_401_2fPipelineOutput)


## quoted_printable

<a id="def-std_401_2fquoted_5fprintable_2fDecodeError"></a>

- `@serializable pub type quoted_printable.DecodeError = InvalidByte(u64) | InvalidHex(u64) | UnexpectedEof(u64) | InvalidSoftBreak( u64 )`

Original zero-based input byte offsets. Errors never contain partial output.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fquoted_5fprintable_2fDecodeError_23InvalidByte"></a>

- `InvalidByte(u64) -> quoted_printable.DecodeError`

Variant of quoted_printable.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fquoted_5fprintable_2fDecodeError_23InvalidHex"></a>

- `InvalidHex(u64) -> quoted_printable.DecodeError`

Variant of quoted_printable.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fquoted_5fprintable_2fDecodeError_23InvalidSoftBreak"></a>

- `InvalidSoftBreak( u64 ) -> quoted_printable.DecodeError`

Variant of quoted_printable.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fquoted_5fprintable_2fDecodeError_23UnexpectedEof"></a>

- `UnexpectedEof(u64) -> quoted_printable.DecodeError`

Variant of quoted_printable.DecodeError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fquoted_5fprintable_2fdecode"></a>

- `pub fn quoted_printable.decode(data: Bytes) -> Result[unique Bytes, DecodeError] uses {}`

Go Reader compatibility, including literal equals and permissive newlines.
See docs/quoted-printable.md; this is not a strict RFC 2045 validator.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/quoted_printable/DecodeError`](#def-std_401_2fquoted_5fprintable_2fDecodeError)

<a id="def-std_401_2fquoted_5fprintable_2fencode"></a>

- `pub fn quoted_printable.encode(data: Bytes) -> unique String uses {}`

Go Writer binary mode: preserves every byte, including CR and LF.
Uppercase escapes stay whole; soft-broken lines have at most 76 columns.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)


## random

<a id="def-std_401_2frandom_2fSampleError"></a>

- `@serializable pub type random.SampleError = EmptyRange | Source(RandomError)`

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


## read_half

<a id="def-core_401_2fread_5fhalf_2fclose"></a>

- `@noalloc fn read_half.close(n: Net, take r: ReadHalf of n) -> ()`

<a id="def-core_401_2fread_5fhalf_2fread"></a>

- `fn read_half.read(n: Net, mut r: ReadHalf of n, mut buffer: List[u8], max: u64) -> Result[u64, NetError]`

<a id="def-core_401_2fread_5fhalf_2fread_5finto"></a>

- `@noalloc fn read_half.read_into[k: size](n: Net, mut r: ReadHalf of n, mut buffer: Inline[u8, k]) -> Result[u64, NetError]`

<a id="def-core_401_2fread_5fhalf_2fset_5ftimeout"></a>

- `@noalloc fn read_half.set_timeout(n: Net, mut r: ReadHalf of n, timeout: NetTimeout) -> Result[(), NetError]`


## regex

<a id="def-std_401_2fregex_2fCaptures"></a>

- `pub type regex.Captures { spans: List[Option[Span]], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/regex/Span`](#def-std_401_2fregex_2fSpan)

<a id="def-std_401_2fregex_2fCompileError"></a>

- `@serializable pub type regex.CompileError = Syntax(u64) | ProgramTooLarge`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fCompileError_23ProgramTooLarge"></a>

- `ProgramTooLarge -> regex.CompileError`

Variant of regex.CompileError.

<a id="def-std_401_2fregex_2fCompileError_23Syntax"></a>

- `Syntax(u64) -> regex.CompileError`

Variant of regex.CompileError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fFlags"></a>

- `pub type regex.Flags { fold_case: bool, multiline: bool, dot_newline: bool, ungreedy: bool, }`

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fregex_2fMatchError"></a>

- `@serializable pub type regex.MatchError = WorkLimit | InvalidCaptures`

<a id="def-std_401_2fregex_2fMatchError_23InvalidCaptures"></a>

- `InvalidCaptures -> regex.MatchError`

Variant of regex.MatchError.

<a id="def-std_401_2fregex_2fMatchError_23WorkLimit"></a>

- `WorkLimit -> regex.MatchError`

Variant of regex.MatchError.

<a id="def-std_401_2fregex_2fRegex"></a>

- `pub opaque type regex.Regex: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fregex_2fSpan"></a>

- `@serializable pub type regex.Span { start: u64, end: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fregex_2fcompile"></a>

- `pub fn regex.compile(pattern: String, max_instructions: u64) -> Result[Regex, CompileError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/CompileError`](#def-std_401_2fregex_2fCompileError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2fcompile_5fflags"></a>

- `pub fn regex.compile_flags(pattern: String, flags: Flags, max_instructions: u64) -> Result[ Regex, CompileError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/CompileError`](#def-std_401_2fregex_2fCompileError), [`std@1/regex/Flags`](#def-std_401_2fregex_2fFlags), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2fdefault_5fflags"></a>

- `pub fn regex.default_flags() -> Flags uses {}`

References: [`std@1/regex/Flags`](#def-std_401_2fregex_2fFlags)

<a id="def-std_401_2fregex_2fexpand"></a>

- `pub fn regex.expand( re: Regex, mut dst: List[u8], template: String, src: String, captures: Captures, work_limit: u64 ) -> Result[(), MatchError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/regex/Captures`](#def-std_401_2fregex_2fCaptures), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2ffind"></a>

- `pub fn regex.find(re: Regex, text: String, work_limit: u64) -> Result[ Option[Span], MatchError ] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex), [`std@1/regex/Span`](#def-std_401_2fregex_2fSpan)

<a id="def-std_401_2fregex_2ffind_5fall"></a>

- `pub fn regex.find_all(re: Regex, text: String, max_matches: u64, work_limit: u64) -> Result[ List[Span], MatchError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex), [`std@1/regex/Span`](#def-std_401_2fregex_2fSpan)

<a id="def-std_401_2fregex_2ffind_5fall_5fcaptures"></a>

- `pub fn regex.find_all_captures(re: Regex, text: String, max_matches: u64, work_limit: u64) -> Result[ List[Captures], MatchError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/Captures`](#def-std_401_2fregex_2fCaptures), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2ffind_5fcaptures"></a>

- `pub fn regex.find_captures(re: Regex, text: String, work_limit: u64) -> Result[ Option[Captures], MatchError ] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/Captures`](#def-std_401_2fregex_2fCaptures), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2fis_5fmatch"></a>

- `pub fn regex.is_match(re: Regex, text: String, work_limit: u64) -> Result[bool, MatchError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2fnum_5fsubexp"></a>

- `pub fn regex.num_subexp(re: Regex) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2freplace_5fall"></a>

- `pub fn regex.replace_all(re: Regex, src: String, replacement: String, work_limit: u64) -> Result[ String, MatchError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2freplace_5fall_5ffunc"></a>

- `pub fn regex.replace_all_func[T]( re: Regex, src: String, state: T, callback: fn(T, String, Captures) -> String uses {}, work_limit: u64 ) -> Result[String, MatchError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/Captures`](#def-std_401_2fregex_2fCaptures), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2freplace_5fall_5fliteral"></a>

- `pub fn regex.replace_all_literal(re: Regex, src: String, replacement: String, work_limit: u64) -> Result[ String, MatchError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/regex/MatchError`](#def-std_401_2fregex_2fMatchError), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)

<a id="def-std_401_2fregex_2fsubexp_5fnames"></a>

- `pub fn regex.subexp_names(re: Regex) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/regex/Regex`](#def-std_401_2fregex_2fRegex)


## region

<a id="def-core_401_2fregion_2fwith"></a>

- `@noalloc fn region.with[T, R](mut r: Region[T], body: fn(root: T) -> R) -> R`


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


## runner

<a id="def-core_401_2frunner_2fcall"></a>

- `fn runner.call[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]`

<a id="def-core_401_2frunner_2fcompute"></a>

- `fn runner.compute[A: serializable, R: serializable, E: serializable](r: Runner, f: Entry[fn(a: A) -> Result[R, E]], a: A) -> unique Result[R, RunError[E]]`

<a id="def-core_401_2frunner_2fentry_5fpath"></a>

- `fn runner.entry_path[F](f: Entry[F]) -> String`

<a id="def-core_401_2frunner_2fgrants"></a>

- `fn runner.grants(r: Runner) -> GrantSet`

<a id="def-core_401_2frunner_2fnarrow"></a>

- `fn runner.narrow(r: Runner, keep: GrantSet) -> Result[Runner, GrantError]`

<a id="def-core_401_2frunner_2fonly"></a>

- `fn runner.only(r: Runner, definitions: List[String]) -> Result[Runner, GrantError]`

<a id="def-core_401_2frunner_2fspawn"></a>

- `fn runner.spawn[A: serializable, R: serializable, E: serializable](s: Scope, r: Runner, f: Entry[fn(rt: Io, a: A) -> Result[R, E]], a: A) -> Task[Result[R, RunError[E]]] of s`


## scalar

<a id="def-std_401_2fscalar_2fFormatError"></a>

- `@serializable pub type scalar.FormatError = BufferTooSmall()`

<a id="def-std_401_2fscalar_2fFormatError_23BufferTooSmall"></a>

- `BufferTooSmall() -> scalar.FormatError`

Variant of scalar.FormatError.

<a id="def-std_401_2fscalar_2fParseError"></a>

- `@serializable pub type scalar.ParseError = Invalid | OutOfRange`

<a id="def-std_401_2fscalar_2fParseError_23Invalid"></a>

- `Invalid -> scalar.ParseError`

Variant of scalar.ParseError.

<a id="def-std_401_2fscalar_2fParseError_23OutOfRange"></a>

- `OutOfRange -> scalar.ParseError`

Variant of scalar.ParseError.


## schedule

<a id="def-std_401_2fschedule_2fFailure"></a>

- `pub type schedule.Failure[E] = GiveUp(E, u64) | Interrupted(Option[E], u64, ScheduleError)`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/ScheduleError`](#def-std_401_2fschedule_2fScheduleError)

<a id="def-std_401_2fschedule_2fFailure_23GiveUp"></a>

- `GiveUp(E, u64) -> schedule.Failure[E]`

Variant of schedule.Failure.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschedule_2fFailure_23Interrupted"></a>

- `Interrupted(Option[E], u64, ScheduleError) -> schedule.Failure[E]`

Variant of schedule.Failure.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/ScheduleError`](#def-std_401_2fschedule_2fScheduleError)

<a id="def-std_401_2fschedule_2fJitter"></a>

- `pub type schedule.Jitter = Full | Equal | Decorrelated`

<a id="def-std_401_2fschedule_2fJitter_23Decorrelated"></a>

- `Decorrelated -> schedule.Jitter`

Variant of schedule.Jitter.

<a id="def-std_401_2fschedule_2fJitter_23Equal"></a>

- `Equal -> schedule.Jitter`

Variant of schedule.Jitter.

<a id="def-std_401_2fschedule_2fJitter_23Full"></a>

- `Full -> schedule.Jitter`

Variant of schedule.Jitter.

<a id="def-std_401_2fschedule_2fSchedule"></a>

- `pub opaque type schedule.Schedule: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fschedule_2fScheduleError"></a>

- `pub type schedule.ScheduleError = Clock(ClockError) | Random(random.SampleError) | Sleep( time.SleepError )`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`std@1/random/SampleError`](#def-std_401_2frandom_2fSampleError), [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)

<a id="def-std_401_2fschedule_2fScheduleError_23Clock"></a>

- `Clock(ClockError) -> schedule.ScheduleError`

Variant of schedule.ScheduleError.

References: [`core@1/ClockError`](#def-core_401_2fClockError)

<a id="def-std_401_2fschedule_2fScheduleError_23Random"></a>

- `Random(random.SampleError) -> schedule.ScheduleError`

Variant of schedule.ScheduleError.

References: [`std@1/random/SampleError`](#def-std_401_2frandom_2fSampleError)

<a id="def-std_401_2fschedule_2fScheduleError_23Sleep"></a>

- `Sleep( time.SleepError ) -> schedule.ScheduleError`

Variant of schedule.ScheduleError.

References: [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)

<a id="def-std_401_2fschedule_2fcapped"></a>

- `pub fn schedule.capped(s: Schedule, max_delay: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fexponential"></a>

- `pub fn schedule.exponential(base: u64, factor: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2ffibonacci"></a>

- `pub fn schedule.fibonacci(base: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2ffixed"></a>

- `pub fn schedule.fixed(interval: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fintersect"></a>

- `pub fn schedule.intersect(a: Schedule, b: Schedule) -> Schedule uses {}`

References: [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fjittered"></a>

- `pub fn schedule.jittered(s: Schedule, kind: Jitter) -> Schedule uses {}`

References: [`std@1/schedule/Jitter`](#def-std_401_2fschedule_2fJitter), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fmax_5fattempts"></a>

- `pub fn schedule.max_attempts(s: Schedule, n: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fmax_5felapsed"></a>

- `pub fn schedule.max_elapsed(s: Schedule, nanoseconds: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fnext"></a>

- `pub fn schedule.next(clock: MonotonicClock, r: Random, mut s: Schedule, start: Instant of clock) -> Result[ Option[time.Duration], ScheduleError ]`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule), [`std@1/schedule/ScheduleError`](#def-std_401_2fschedule_2fScheduleError), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2fschedule_2frepeat"></a>

- `pub fn schedule.repeat[T, E]( clock: MonotonicClock, r: Random, schedule: Schedule, op: fn() -> Result[T, E] ) -> Result[(T, u64), Failure[E]]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Failure`](#def-std_401_2fschedule_2fFailure), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fretry"></a>

- `pub fn schedule.retry[T, E]( clock: MonotonicClock, r: Random, schedule: Schedule, op: fn() -> Result[T, E], should_retry: fn(E) -> bool ) -> Result[(T, u64), Failure[E]]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Failure`](#def-std_401_2fschedule_2fFailure), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2fspaced"></a>

- `pub fn schedule.spaced(interval: u64) -> Schedule uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)

<a id="def-std_401_2fschedule_2funion"></a>

- `pub fn schedule.union(a: Schedule, b: Schedule) -> Schedule uses {}`

References: [`std@1/schedule/Schedule`](#def-std_401_2fschedule_2fSchedule)


## schema

<a id="def-std_401_2fschema_2fBoundary"></a>

- `pub type schema.Boundary[D: inert, W: data + inert] { decode: fn(W, mut CheckState) -> Result[D, SchemaError] uses {}, encode: fn(D, mut CheckState) -> Result[W, SchemaError] uses {}, }`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fBoundaryError"></a>

- `pub type schema.BoundaryError = Schema(SchemaError) | Json(std@1/json.JsonError) | FormatLimit( FormatLimit )`

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/schema/FormatLimit`](#def-std_401_2fschema_2fFormatLimit), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fBoundaryError_23FormatLimit"></a>

- `FormatLimit( FormatLimit ) -> schema.BoundaryError`

Variant of schema.BoundaryError.

References: [`std@1/schema/FormatLimit`](#def-std_401_2fschema_2fFormatLimit)

<a id="def-std_401_2fschema_2fBoundaryError_23Json"></a>

- `Json(std@1/json.JsonError) -> schema.BoundaryError`

Variant of schema.BoundaryError.

References: [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError)

<a id="def-std_401_2fschema_2fBoundaryError_23Schema"></a>

- `Schema(SchemaError) -> schema.BoundaryError`

Variant of schema.BoundaryError.

References: [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fCheckState"></a>

- `pub opaque type schema.CheckState: plain + inert + noalloc_drop uses {}`

<a id="def-std_401_2fschema_2fCheckedWriteError"></a>

- `pub type schema.CheckedWriteError = Boundary(BoundaryError) | ProjectionMismatch`

References: [`std@1/schema/BoundaryError`](#def-std_401_2fschema_2fBoundaryError)

<a id="def-std_401_2fschema_2fCheckedWriteError_23Boundary"></a>

- `Boundary(BoundaryError) -> schema.CheckedWriteError`

Variant of schema.CheckedWriteError.

References: [`std@1/schema/BoundaryError`](#def-std_401_2fschema_2fBoundaryError)

<a id="def-std_401_2fschema_2fCheckedWriteError_23ProjectionMismatch"></a>

- `ProjectionMismatch -> schema.CheckedWriteError`

Variant of schema.CheckedWriteError.

<a id="def-std_401_2fschema_2fDecoder"></a>

- `pub type schema.Decoder[D: inert, W: data + inert] { decode: fn(W, mut CheckState) -> Result[D, SchemaError] uses {}, }`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fFieldPolicy"></a>

- `pub type schema.FieldPolicy { name: String, wire_name: String, aliases: List[String], missing: Missing, nulls: Nulls, }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/schema/Missing`](#def-std_401_2fschema_2fMissing), [`std@1/schema/Nulls`](#def-std_401_2fschema_2fNulls)

<a id="def-std_401_2fschema_2fFormatLimit"></a>

- `pub type schema.FormatLimit = InputBytes(u64) | OutputBytes(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fFormatLimit_23InputBytes"></a>

- `InputBytes(u64) -> schema.FormatLimit`

Variant of schema.FormatLimit.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fFormatLimit_23OutputBytes"></a>

- `OutputBytes(u64) -> schema.FormatLimit`

Variant of schema.FormatLimit.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fIssue"></a>

- `pub type schema.Issue { code: String, stage: Stage, path: List[Segment], schema_site: String, expected: Option[String], children: List[Issue], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/schema/Segment`](#def-std_401_2fschema_2fSegment), [`std@1/schema/Stage`](#def-std_401_2fschema_2fStage)

<a id="def-std_401_2fschema_2fIssues"></a>

- `pub type schema.Issues { items: List[Issue], truncated: bool, }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`std@1/schema/Issue`](#def-std_401_2fschema_2fIssue)

<a id="def-std_401_2fschema_2fLawError"></a>

- `pub type schema.LawError = Schema(SchemaError) | ProjectionMismatch`

References: [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fLawError_23ProjectionMismatch"></a>

- `ProjectionMismatch -> schema.LawError`

Variant of schema.LawError.

<a id="def-std_401_2fschema_2fLawError_23Schema"></a>

- `Schema(SchemaError) -> schema.LawError`

Variant of schema.LawError.

References: [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fLeaf"></a>

- `pub type schema.Leaf { ty: String, decode: String, encode: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fschema_2fLimitError"></a>

- `pub type schema.LimitError { kind: LimitKind, limit: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/schema/LimitKind`](#def-std_401_2fschema_2fLimitKind)

<a id="def-std_401_2fschema_2fLimitKind"></a>

- `pub type schema.LimitKind = InvalidOptions | Depth | Work | DiagnosticBytes`

<a id="def-std_401_2fschema_2fLimitKind_23Depth"></a>

- `Depth -> schema.LimitKind`

Variant of schema.LimitKind.

<a id="def-std_401_2fschema_2fLimitKind_23DiagnosticBytes"></a>

- `DiagnosticBytes -> schema.LimitKind`

Variant of schema.LimitKind.

<a id="def-std_401_2fschema_2fLimitKind_23InvalidOptions"></a>

- `InvalidOptions -> schema.LimitKind`

Variant of schema.LimitKind.

<a id="def-std_401_2fschema_2fLimitKind_23Work"></a>

- `Work -> schema.LimitKind`

Variant of schema.LimitKind.

<a id="def-std_401_2fschema_2fLimits"></a>

- `pub type schema.Limits { max_depth: u64, max_issues: u64, max_work: u64, max_input_bytes: u64, max_output_bytes: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fMAX_5fLABEL_5fBYTES"></a>

- `pub const schema.MAX_LABEL_BYTES: u64`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fMissing"></a>

- `pub type schema.Missing = Required | NoneValue | Default(std@1/json.Value)`

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fschema_2fMissing_23Default"></a>

- `Default(std@1/json.Value) -> schema.Missing`

Variant of schema.Missing.

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue)

<a id="def-std_401_2fschema_2fMissing_23NoneValue"></a>

- `NoneValue -> schema.Missing`

Variant of schema.Missing.

<a id="def-std_401_2fschema_2fMissing_23Required"></a>

- `Required -> schema.Missing`

Variant of schema.Missing.

<a id="def-std_401_2fschema_2fMode"></a>

- `pub type schema.Mode = First | All`

<a id="def-std_401_2fschema_2fMode_23All"></a>

- `All -> schema.Mode`

Variant of schema.Mode.

<a id="def-std_401_2fschema_2fMode_23First"></a>

- `First -> schema.Mode`

Variant of schema.Mode.

<a id="def-std_401_2fschema_2fNulls"></a>

- `pub type schema.Nulls = Accept | Reject`

<a id="def-std_401_2fschema_2fNulls_23Accept"></a>

- `Accept -> schema.Nulls`

Variant of schema.Nulls.

<a id="def-std_401_2fschema_2fNulls_23Reject"></a>

- `Reject -> schema.Nulls`

Variant of schema.Nulls.

<a id="def-std_401_2fschema_2fOptions"></a>

- `pub type schema.Options { limits: Limits, mode: Mode, }`

References: [`std@1/schema/Limits`](#def-std_401_2fschema_2fLimits), [`std@1/schema/Mode`](#def-std_401_2fschema_2fMode)

<a id="def-std_401_2fschema_2fPolicy"></a>

- `pub type schema.Policy { fields: List[FieldPolicy], tags: List[TagPolicy], leaves: List[Leaf], }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/schema/FieldPolicy`](#def-std_401_2fschema_2fFieldPolicy), [`std@1/schema/Leaf`](#def-std_401_2fschema_2fLeaf), [`std@1/schema/TagPolicy`](#def-std_401_2fschema_2fTagPolicy)

<a id="def-std_401_2fschema_2fSchemaError"></a>

- `pub type schema.SchemaError = Invalid(Issues) | Limit(LimitError)`

References: [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues), [`std@1/schema/LimitError`](#def-std_401_2fschema_2fLimitError)

<a id="def-std_401_2fschema_2fSchemaError_23Invalid"></a>

- `Invalid(Issues) -> schema.SchemaError`

Variant of schema.SchemaError.

References: [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues)

<a id="def-std_401_2fschema_2fSchemaError_23Limit"></a>

- `Limit(LimitError) -> schema.SchemaError`

Variant of schema.SchemaError.

References: [`std@1/schema/LimitError`](#def-std_401_2fschema_2fLimitError)

<a id="def-std_401_2fschema_2fSegment"></a>

- `pub type schema.Segment = Field(String) | Index(u64) | Variant(String) | MapEntry(u64) | Key | Value`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fSegment_23Field"></a>

- `Field(String) -> schema.Segment`

Variant of schema.Segment.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fschema_2fSegment_23Index"></a>

- `Index(u64) -> schema.Segment`

Variant of schema.Segment.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fSegment_23Key"></a>

- `Key -> schema.Segment`

Variant of schema.Segment.

<a id="def-std_401_2fschema_2fSegment_23MapEntry"></a>

- `MapEntry(u64) -> schema.Segment`

Variant of schema.Segment.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fschema_2fSegment_23Value"></a>

- `Value -> schema.Segment`

Variant of schema.Segment.

<a id="def-std_401_2fschema_2fSegment_23Variant"></a>

- `Variant(String) -> schema.Segment`

Variant of schema.Segment.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fschema_2fStage"></a>

- `pub type schema.Stage = Syntax | Wire | Fact | Transform | Migration`

<a id="def-std_401_2fschema_2fStage_23Fact"></a>

- `Fact -> schema.Stage`

Variant of schema.Stage.

<a id="def-std_401_2fschema_2fStage_23Migration"></a>

- `Migration -> schema.Stage`

Variant of schema.Stage.

<a id="def-std_401_2fschema_2fStage_23Syntax"></a>

- `Syntax -> schema.Stage`

Variant of schema.Stage.

<a id="def-std_401_2fschema_2fStage_23Transform"></a>

- `Transform -> schema.Stage`

Variant of schema.Stage.

<a id="def-std_401_2fschema_2fStage_23Wire"></a>

- `Wire -> schema.Stage`

Variant of schema.Stage.

<a id="def-std_401_2fschema_2fTagPolicy"></a>

- `pub type schema.TagPolicy { name: String, wire_name: String, aliases: List[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fschema_2faccepted_5fname"></a>

- `pub fn schema.accepted_name(name: String, canonical: String, aliases: List[String]) -> bool uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fschema_2fappend_5fissues"></a>

- `pub fn schema.append_issues(mut into: Issues, take more: Issues) -> () uses {}`

References: [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues)

<a id="def-std_401_2fschema_2farray_5fjson"></a>

- `pub fn schema.array_json() -> Boundary[List[std@1/json.Value], std@1/json.Value] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2fcharge_5fcheck"></a>

- `pub fn schema.charge_check(mut check: CheckState) -> Result[(), SchemaError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fcheck_5fnormalization"></a>

- `pub fn schema.check_normalization[D: inert, W: data + inert, P: serializable]( b: Boundary[D, W], wire: W, projection: fn(W) -> P uses {}, o: Options ) -> Result[(), LawError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/LawError`](#def-std_401_2fschema_2fLawError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fcheck_5fnull"></a>

- `pub fn schema.check_null( wire: std@1/json.Value, p: FieldPolicy, site: String, mut check: CheckState ) -> Result[(), SchemaError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/FieldPolicy`](#def-std_401_2fschema_2fFieldPolicy), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fcheck_5fround_5ftrip"></a>

- `pub fn schema.check_round_trip[D: inert, W: data + inert, P: serializable]( b: Boundary[D, W], value: D, projection: fn(D) -> P uses {}, o: Options ) -> Result[(), LawError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/LawError`](#def-std_401_2fschema_2fLawError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fcheck_5fstate"></a>

- `pub fn schema.check_state(o: Options) -> Result[CheckState, SchemaError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fcollect"></a>

- `pub fn schema.collect[T: inert]( take outcome: Result[T, SchemaError], segment: Segment, mut issues: Issues ) -> Result[Option[T], SchemaError] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError), [`std@1/schema/Segment`](#def-std_401_2fschema_2fSegment)

<a id="def-std_401_2fschema_2fdecode"></a>

- `pub fn schema.decode[D: inert, W: data + inert](b: Boundary[D, W], wire: W, o: Options) -> Result[ D, SchemaError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fdecode_5fonly"></a>

- `pub fn schema.decode_only[D: inert, W: data + inert](b: Decoder[D, W], wire: W, o: Options) -> Result[ D, SchemaError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Decoder`](#def-std_401_2fschema_2fDecoder), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fdefault_5foptions"></a>

- `pub fn schema.default_options() -> Options uses {}`

References: [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fempty_5fissues"></a>

- `pub fn schema.empty_issues() -> Issues uses {}`

References: [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues)

<a id="def-std_401_2fschema_2fencode"></a>

- `pub fn schema.encode[D: inert, W: data + inert](b: Boundary[D, W], value: D, o: Options) -> Result[ W, SchemaError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fenter_5fnode"></a>

- `pub fn schema.enter_node(mut check: CheckState) -> Result[(), SchemaError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2ffield_5fpolicy"></a>

- `pub fn schema.field_policy(p: Policy, name: String) -> FieldPolicy uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema/FieldPolicy`](#def-std_401_2fschema_2fFieldPolicy), [`std@1/schema/Policy`](#def-std_401_2fschema_2fPolicy)

<a id="def-std_401_2fschema_2fformat"></a>

- `pub fn schema.format(issues: Issues) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues)

<a id="def-std_401_2fschema_2fidentity"></a>

- `pub fn schema.identity[W: plain + data + inert]() -> Boundary[W, W] uses {}`

References: [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2finvalid"></a>

- `pub fn schema.invalid( mut check: CheckState, code: String, stage: Stage, schema_site: String, expected: Option[String] ) -> SchemaError uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError), [`std@1/schema/Stage`](#def-std_401_2fschema_2fStage)

<a id="def-std_401_2fschema_2fleave_5fnode"></a>

- `pub fn schema.leave_node(mut check: CheckState) -> () uses {}`

References: [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState)

<a id="def-std_401_2fschema_2flist"></a>

- `pub fn schema.list[D: inert, W: data + inert](take item: Boundary[D, W]) -> Boundary[ List[D], List[W] ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2flist_5fjson"></a>

- `pub fn schema.list_json[D: inert](take item: Boundary[D, std@1/json.Value]) -> Boundary[ List[D], std@1/json.Value ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2fmap_5fdecode"></a>

- `pub fn schema.map_decode[K: key + inert, V: inert]( wire: std@1/json.Value, key_decode: fn(std@1/json.Value, mut CheckState) -> Result[K, SchemaError] uses {}, value_decode: fn(std@1/json.Value, mut CheckState) -> Result[V, SchemaError] uses {}, mut check: CheckState ) -> Result[Map[K, V], SchemaError] uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fmap_5fencode"></a>

- `pub fn schema.map_encode[K: key + inert, V: inert]( values: Map[K, V], key_encode: fn(K, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, value_encode: fn(V, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, mut check: CheckState ) -> Result[std@1/json.Value, SchemaError] uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fmember"></a>

- `pub fn schema.member( fields: Map[String, std@1/json.Value], p: FieldPolicy, site: String, mut check: CheckState ) -> Result[std@1/json.Value, SchemaError] uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/FieldPolicy`](#def-std_401_2fschema_2fFieldPolicy), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fobject_5fmap_5fdecode"></a>

- `pub fn schema.object_map_decode[V: inert]( wire: std@1/json.Value, key_decode: fn(std@1/json.Value, mut CheckState) -> Result[String, SchemaError] uses {}, value_decode: fn(std@1/json.Value, mut CheckState) -> Result[V, SchemaError] uses {}, mut check: CheckState ) -> Result[Map[String, V], SchemaError] uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fobject_5fmap_5fencode"></a>

- `pub fn schema.object_map_encode[V: inert]( values: Map[String, V], key_encode: fn(String, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, value_encode: fn(V, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, mut check: CheckState ) -> Result[std@1/json.Value, SchemaError] uses {}`

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2foption_5fdecode"></a>

- `pub fn schema.option_decode[T: inert]( wire: std@1/json.Value, item_decode: fn(std@1/json.Value, mut CheckState) -> Result[T, SchemaError] uses {}, mut check: CheckState ) -> Result[Option[T], SchemaError] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2foption_5fencode"></a>

- `pub fn schema.option_encode[T: inert]( value: Option[T], item_encode: fn(T, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, mut check: CheckState ) -> Result[std@1/json.Value, SchemaError] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fparse_5fjson"></a>

- `pub fn schema.parse_json[D: inert](b: Boundary[D, std@1/json.Value], input: String, o: Options) -> Result[ D, BoundaryError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/BoundaryError`](#def-std_401_2fschema_2fBoundaryError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fprefix"></a>

- `pub fn schema.prefix(segment: Segment, take issues: Issues) -> Issues uses {}`

References: [`std@1/schema/Issues`](#def-std_401_2fschema_2fIssues), [`std@1/schema/Segment`](#def-std_401_2fschema_2fSegment)

<a id="def-std_401_2fschema_2fprefix_5ferror"></a>

- `pub fn schema.prefix_error(segment: Segment, take error: SchemaError) -> SchemaError uses {}`

References: [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError), [`std@1/schema/Segment`](#def-std_401_2fschema_2fSegment)

<a id="def-std_401_2fschema_2fscalar_5ferror"></a>

- `pub fn schema.scalar_error(mut check: CheckState, site: String) -> SchemaError uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fsequence_5fdecode"></a>

- `pub fn schema.sequence_decode[T: inert]( wire: std@1/json.Value, item_decode: fn(std@1/json.Value, mut CheckState) -> Result[T, SchemaError] uses {}, mut check: CheckState ) -> Result[List[T], SchemaError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fsequence_5fencode"></a>

- `pub fn schema.sequence_encode[T: inert]( values: List[T], item_encode: fn(T, mut CheckState) -> Result[std@1/json.Value, SchemaError] uses {}, mut check: CheckState ) -> Result[std@1/json.Value, SchemaError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fshould_5fstop"></a>

- `pub fn schema.should_stop(check: CheckState) -> bool uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState)

<a id="def-std_401_2fschema_2fstrict_5fpolicy"></a>

- `pub fn schema.strict_policy() -> Policy uses {}`

References: [`std@1/schema/Policy`](#def-std_401_2fschema_2fPolicy)

<a id="def-std_401_2fschema_2ftag_5fpolicy"></a>

- `pub fn schema.tag_policy(p: Policy, name: String) -> TagPolicy uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema/Policy`](#def-std_401_2fschema_2fPolicy), [`std@1/schema/TagPolicy`](#def-std_401_2fschema_2fTagPolicy)

<a id="def-std_401_2fschema_2ftext_5fjson"></a>

- `pub fn schema.text_json() -> Boundary[String, std@1/json.Value] uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2fthen"></a>

- `pub fn schema.then[A: inert, B: data + inert, W: data + inert]( take outer: Boundary[B, W], take inner: Boundary[A, B] ) -> Boundary[A, W] uses {}`

References: [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2fu8_5fjson"></a>

- `pub fn schema.u8_json() -> Boundary[u8, std@1/json.Value] uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary)

<a id="def-std_401_2fschema_2funknown_5ffield"></a>

- `pub fn schema.unknown_field(mut check: CheckState, name: String, site: String) -> SchemaError uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema/CheckState`](#def-std_401_2fschema_2fCheckState), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError)

<a id="def-std_401_2fschema_2fwrite_5fchecked"></a>

- `pub fn schema.write_checked[D: inert, P: serializable]( b: Boundary[D, std@1/json.Value], value: D, projection: fn(D) -> P uses {}, o: Options ) -> Result[String, CheckedWriteError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/CheckedWriteError`](#def-std_401_2fschema_2fCheckedWriteError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fwrite_5fjson"></a>

- `pub fn schema.write_json[D: inert](b: Boundary[D, std@1/json.Value], value: D, o: Options) -> Result[ String, BoundaryError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/BoundaryError`](#def-std_401_2fschema_2fBoundaryError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)

<a id="def-std_401_2fschema_2fwrite_5fjson_5fchecked"></a>

- `pub fn schema.write_json_checked[D: inert, P: serializable]( b: Boundary[D, std@1/json.Value], value: D, projection: fn(D) -> P uses {}, o: Options ) -> Result[String, CheckedWriteError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema/Boundary`](#def-std_401_2fschema_2fBoundary), [`std@1/schema/CheckedWriteError`](#def-std_401_2fschema_2fCheckedWriteError), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions)


## schema_evolution

<a id="def-std_401_2fschema_5fevolution_2fCompatibility"></a>

- `pub type schema_evolution.Compatibility = Exact | AdapterRequired | Incompatible | UnknownPredicate`

<a id="def-std_401_2fschema_5fevolution_2fCompatibility_23AdapterRequired"></a>

- `AdapterRequired -> schema_evolution.Compatibility`

Variant of schema_evolution.Compatibility.

<a id="def-std_401_2fschema_5fevolution_2fCompatibility_23Exact"></a>

- `Exact -> schema_evolution.Compatibility`

Variant of schema_evolution.Compatibility.

<a id="def-std_401_2fschema_5fevolution_2fCompatibility_23Incompatible"></a>

- `Incompatible -> schema_evolution.Compatibility`

Variant of schema_evolution.Compatibility.

<a id="def-std_401_2fschema_5fevolution_2fCompatibility_23UnknownPredicate"></a>

- `UnknownPredicate -> schema_evolution.Compatibility`

Variant of schema_evolution.Compatibility.

<a id="def-std_401_2fschema_5fevolution_2fField"></a>

- `pub type schema_evolution.Field { name: String, aliases: List[String], scalar: Scalar, predicate_version: Option[String], default: Option[std@1/json.Value], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/schema_evolution/Scalar`](#def-std_401_2fschema_5fevolution_2fScalar)

<a id="def-std_401_2fschema_5fevolution_2fFinding"></a>

- `pub type schema_evolution.Finding { field: String, code: String, compatibility: Compatibility, }`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/schema_evolution/Compatibility`](#def-std_401_2fschema_5fevolution_2fCompatibility)

<a id="def-std_401_2fschema_5fevolution_2fIdentity"></a>

- `pub type schema_evolution.Identity { package: String, wire_policy: String, constructor: String, codec_envelope: String, }`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fschema_5fevolution_2fRecord"></a>

- `pub type schema_evolution.Record { identity: Identity, fields: List[Field], }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/schema_evolution/Field`](#def-std_401_2fschema_5fevolution_2fField), [`std@1/schema_evolution/Identity`](#def-std_401_2fschema_5fevolution_2fIdentity)

<a id="def-std_401_2fschema_5fevolution_2fReport"></a>

- `pub type schema_evolution.Report { compatibility: Compatibility, findings: List[Finding], }`

References: [`core@1/List`](#def-core_401_2fList), [`std@1/schema_evolution/Compatibility`](#def-std_401_2fschema_5fevolution_2fCompatibility), [`std@1/schema_evolution/Finding`](#def-std_401_2fschema_5fevolution_2fFinding)

<a id="def-std_401_2fschema_5fevolution_2fScalar"></a>

- `pub type schema_evolution.Scalar = U8 | U16 | U32 | U64 | Text | Boolean`

<a id="def-std_401_2fschema_5fevolution_2fScalar_23Boolean"></a>

- `Boolean -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fScalar_23Text"></a>

- `Text -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fScalar_23U16"></a>

- `U16 -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fScalar_23U32"></a>

- `U32 -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fScalar_23U64"></a>

- `U64 -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fScalar_23U8"></a>

- `U8 -> schema_evolution.Scalar`

Variant of schema_evolution.Scalar.

<a id="def-std_401_2fschema_5fevolution_2fcompare"></a>

- `pub fn schema_evolution.compare(writer: Record, reader: Record, options: std@1/schema.Options) -> Result[ Report, std@1/schema.SchemaError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/schema/Options`](#def-std_401_2fschema_2fOptions), [`std@1/schema/SchemaError`](#def-std_401_2fschema_2fSchemaError), [`std@1/schema_evolution/Record`](#def-std_401_2fschema_5fevolution_2fRecord), [`std@1/schema_evolution/Report`](#def-std_401_2fschema_5fevolution_2fReport)


## semver

<a id="def-std_401_2fsemver_2fComparator"></a>

- `pub type semver.Comparator { op: Op, major: u64, minor: Option[u64], patch: Option[u64], pre: String, }`

One comparator of a requirement: an operator and a version whose minor and
patch may be absent (`^1`, `~1.2`, `>=1.*`). `pre` is empty unless the patch
is present.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/semver/Op`](#def-std_401_2fsemver_2fOp)

<a id="def-std_401_2fsemver_2fOp"></a>

- `pub type semver.Op = Exact | Greater | GreaterEq | Less | LessEq | Tilde | Caret | Wildcard`

A comparator's operator; `Wildcard` is a requirement such as `1.*`.

<a id="def-std_401_2fsemver_2fOp_23Caret"></a>

- `Caret -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23Exact"></a>

- `Exact -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23Greater"></a>

- `Greater -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23GreaterEq"></a>

- `GreaterEq -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23Less"></a>

- `Less -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23LessEq"></a>

- `LessEq -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23Tilde"></a>

- `Tilde -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fOp_23Wildcard"></a>

- `Wildcard -> semver.Op`

Variant of semver.Op.

<a id="def-std_401_2fsemver_2fReq"></a>

- `@format(format_req_into) pub type semver.Req { sets: List[List[Comparator]], }`

A requirement: a version matches when it matches every comparator of any
one set. A Cargo requirement has one set; an npm range has one per `||`
alternative. An empty set matches every version without a prerelease.

References: [`core@1/List`](#def-core_401_2fList), [`std@1/semver/Comparator`](#def-std_401_2fsemver_2fComparator), [`std@1/semver/format_req_into`](#def-std_401_2fsemver_2fformat_5freq_5finto)

<a id="def-std_401_2fsemver_2fSemverError"></a>

- `pub type semver.SemverError = Empty | Syntax(u64) | LeadingZero(u64) | Overflow(u64)`

Offsets are byte positions in the parsed text.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsemver_2fSemverError_23Empty"></a>

- `Empty -> semver.SemverError`

Variant of semver.SemverError.

<a id="def-std_401_2fsemver_2fSemverError_23LeadingZero"></a>

- `LeadingZero(u64) -> semver.SemverError`

Variant of semver.SemverError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsemver_2fSemverError_23Overflow"></a>

- `Overflow(u64) -> semver.SemverError`

Variant of semver.SemverError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsemver_2fSemverError_23Syntax"></a>

- `Syntax(u64) -> semver.SemverError`

Variant of semver.SemverError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsemver_2fVersion"></a>

- `@format(format_into) pub type semver.Version { major: u64, minor: u64, patch: u64, pre: String, build: String, }`

A SemVer 2.0 version. `pre` and `build` are the dot-separated identifiers
after `-` and `+`, empty when absent.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/semver/format_into`](#def-std_401_2fsemver_2fformat_5finto)

<a id="def-std_401_2fsemver_2fcompare"></a>

- `@noalloc pub fn semver.compare(a: Version, b: Version) -> i32 uses {}`

SemVer 2.0 precedence: -1, 0 or 1. Major, minor and patch compare
numerically, then prereleases by `compare_pre`; build metadata is ignored,
so versions differing only in it compare 0 though `==` tells them apart.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/semver/Version`](#def-std_401_2fsemver_2fVersion)

<a id="def-std_401_2fsemver_2fcompare_5fpre"></a>

- `@noalloc pub fn semver.compare_pre(a: String, b: String) -> i32 uses {}`

Compares prerelease strings by SemVer 2.0 precedence: an empty prerelease
(a release) is greatest; identifiers compare in turn, numeric ones by value
and below alphanumeric ones, which compare in ASCII order; a longer list of
otherwise equal identifiers is greater.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fsemver_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn semver.format_into(v: Version, mut out: List[u8]) -> () uses {}`

Appends `major.minor.patch`, then `-pre` and `+build` when present.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/semver/Version`](#def-std_401_2fsemver_2fVersion)

<a id="def-std_401_2fsemver_2fformat_5freq_5finto"></a>

- `@preserves_unique(out) pub fn semver.format_req_into(req: Req, mut out: List[u8]) -> () uses {}`

Appends the requirement as dtolnay/semver displays one: comparators joined
by `, `, `*` for an empty set, and sets joined by ` || `.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/semver/Req`](#def-std_401_2fsemver_2fReq)

<a id="def-std_401_2fsemver_2fmatches"></a>

- `pub fn semver.matches(req: Req, v: Version) -> bool uses {}`

Whether `v` matches every comparator of some set of `req`. A version with a
prerelease matches a set only when one of its comparators has a prerelease
and the same major, minor and patch, so `^1.2.3` excludes `1.3.0-beta`.
Build metadata is ignored.

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/semver/Req`](#def-std_401_2fsemver_2fReq), [`std@1/semver/Version`](#def-std_401_2fsemver_2fVersion)

<a id="def-std_401_2fsemver_2fparse"></a>

- `pub fn semver.parse(spelling: String) -> Result[Version, SemverError] uses {}`

Parses a SemVer 2.0 version, `major.minor.patch` with an optional
`-prerelease` and `+build`, as dtolnay/semver `Version::parse`: no leading
`v`, whitespace, or leading zero in a number or numeric prerelease
identifier; numbers fit in u64.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/semver/SemverError`](#def-std_401_2fsemver_2fSemverError), [`std@1/semver/Version`](#def-std_401_2fsemver_2fVersion)

<a id="def-std_401_2fsemver_2fparse_5fcargo"></a>

- `pub fn semver.parse_cargo(spelling: String) -> Result[Req, SemverError] uses {}`

Parses a Cargo version requirement, as dtolnay/semver `VersionReq::parse`:
comparators separated by commas, each an optional `=`, `>`, `>=`, `<`, `<=`,
`~` or `^` (the default) and a version whose minor and patch may be omitted
or `*`, `x` or `X`, with spaces allowed around operators and commas. A lone
wildcard is the requirement matching every release, and must be the only
comparator. Build metadata is accepted and ignored.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/semver/Req`](#def-std_401_2fsemver_2fReq), [`std@1/semver/SemverError`](#def-std_401_2fsemver_2fSemverError)

<a id="def-std_401_2fsemver_2fparse_5fnpm"></a>

- `pub fn semver.parse_npm(spelling: String) -> Result[Req, SemverError] uses {}`

Parses an npm range, as node-semver `new Range(text)` without the loose and
includePrerelease options: `||` separates alternatives, each a hyphen range
`a - b` or blank-separated comparators: `<`, `<=`, `>`, `>=`, `=` or none
before an x-range version (`1`, `1.2`, `1.x`, `*`, `1.2.3-pre`), `~` or `~>`,
or `^`. A version may start with `v`; build metadata is ignored. Each
alternative desugars to primitive comparators exactly as node-semver's do,
and an empty alternative matches every release.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/semver/Req`](#def-std_401_2fsemver_2fReq), [`std@1/semver/SemverError`](#def-std_401_2fsemver_2fSemverError)

<a id="def-std_401_2fsemver_2freq_5fto_5fstring"></a>

- `pub fn semver.req_to_string(req: Req) -> unique String uses {}`

The requirement's spelling, as `format_req_into`.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/semver/Req`](#def-std_401_2fsemver_2fReq)

<a id="def-std_401_2fsemver_2fto_5fstring"></a>

- `pub fn semver.to_string(v: Version) -> unique String uses {}`

The version's spelling.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/semver/Version`](#def-std_401_2fsemver_2fVersion)


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


## sha1

<a id="def-std_401_2fsha1_2fState"></a>

- `pub opaque type sha1.State: plain + inert uses {}`

<a id="def-std_401_2fsha1_2fdigest"></a>

- `@noalloc pub fn sha1.digest(data: Bytes) -> Inline[u8, 20] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha1_2fdigest_5fhex"></a>

- `pub fn sha1.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha1_2ffinish"></a>

- `@noalloc pub fn sha1.finish(take state: State) -> Inline[u8, 20] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha1/State`](#def-std_401_2fsha1_2fState)

<a id="def-std_401_2fsha1_2fhash"></a>

- `pub implementation sha1.hash: std@1/digest.Digest { type State = State size bytes = 20 size block = 64 new = new update = update finish = finish }`

References: [`std@1/sha1/State`](#def-std_401_2fsha1_2fState)

<a id="def-std_401_2fsha1_2fnew"></a>

- `@noalloc pub fn sha1.new() -> State uses {}`

References: [`std@1/sha1/State`](#def-std_401_2fsha1_2fState)

<a id="def-std_401_2fsha1_2fupdate"></a>

- `@noalloc pub fn sha1.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha1/State`](#def-std_401_2fsha1_2fState)


## sha224

<a id="def-std_401_2fsha224_2fState"></a>

- `pub opaque type sha224.State: plain + inert uses {}`

<a id="def-std_401_2fsha224_2fdigest"></a>

- `@noalloc pub fn sha224.digest(data: Bytes) -> Inline[u8, 28] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha224_2fdigest_5fhex"></a>

- `pub fn sha224.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha224_2ffinish"></a>

- `@noalloc pub fn sha224.finish(take state: State) -> Inline[u8, 28] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha224/State`](#def-std_401_2fsha224_2fState)

<a id="def-std_401_2fsha224_2fhash"></a>

- `pub implementation sha224.hash: std@1/digest.Digest { type State = State size bytes = 28 size block = 64 new = new update = update finish = finish }`

References: [`std@1/sha224/State`](#def-std_401_2fsha224_2fState)

<a id="def-std_401_2fsha224_2fnew"></a>

- `@noalloc pub fn sha224.new() -> State uses {}`

References: [`std@1/sha224/State`](#def-std_401_2fsha224_2fState)

<a id="def-std_401_2fsha224_2fupdate"></a>

- `@noalloc pub fn sha224.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha224/State`](#def-std_401_2fsha224_2fState)


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

<a id="def-std_401_2fsha256_2fhash"></a>

- `pub implementation sha256.hash: std@1/digest.Digest { type State = State size bytes = 32 size block = 64 new = new update = update finish = finish }`

References: [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)

<a id="def-std_401_2fsha256_2fnew"></a>

- `@noalloc pub fn sha256.new() -> State uses {}`

References: [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)

<a id="def-std_401_2fsha256_2fupdate"></a>

- `@noalloc pub fn sha256.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha256/State`](#def-std_401_2fsha256_2fState)


## sha384

<a id="def-std_401_2fsha384_2fState"></a>

- `pub opaque type sha384.State: plain + inert uses {}`

<a id="def-std_401_2fsha384_2fdigest"></a>

- `@noalloc pub fn sha384.digest(data: Bytes) -> Inline[u8, 48] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha384_2fdigest_5fhex"></a>

- `pub fn sha384.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha384_2ffinish"></a>

- `@noalloc pub fn sha384.finish(take state: State) -> Inline[u8, 48] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha384/State`](#def-std_401_2fsha384_2fState)

<a id="def-std_401_2fsha384_2fhash"></a>

- `pub implementation sha384.hash: std@1/digest.Digest { type State = State size bytes = 48 size block = 128 new = new update = update finish = finish }`

References: [`std@1/sha384/State`](#def-std_401_2fsha384_2fState)

<a id="def-std_401_2fsha384_2fnew"></a>

- `@noalloc pub fn sha384.new() -> State uses {}`

References: [`std@1/sha384/State`](#def-std_401_2fsha384_2fState)

<a id="def-std_401_2fsha384_2fupdate"></a>

- `@noalloc pub fn sha384.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha384/State`](#def-std_401_2fsha384_2fState)


## sha3_224

<a id="def-std_401_2fsha3_5f224_2fState"></a>

- `pub opaque type sha3_224.State: plain + inert uses {}`

<a id="def-std_401_2fsha3_5f224_2fdigest"></a>

- `@noalloc pub fn sha3_224.digest(data: Bytes) -> Inline[u8, 28] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha3_5f224_2fdigest_5fhex"></a>

- `pub fn sha3_224.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha3_5f224_2ffinish"></a>

- `@noalloc pub fn sha3_224.finish(take state: State) -> Inline[u8, 28] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha3_224/State`](#def-std_401_2fsha3_5f224_2fState)

<a id="def-std_401_2fsha3_5f224_2fhash"></a>

- `pub implementation sha3_224.hash: std@1/digest.Digest { type State = State size bytes = 28 size block = 144 new = new update = update finish = finish }`

References: [`std@1/sha3_224/State`](#def-std_401_2fsha3_5f224_2fState)

<a id="def-std_401_2fsha3_5f224_2fnew"></a>

- `@noalloc pub fn sha3_224.new() -> State uses {}`

References: [`std@1/sha3_224/State`](#def-std_401_2fsha3_5f224_2fState)

<a id="def-std_401_2fsha3_5f224_2fupdate"></a>

- `@noalloc pub fn sha3_224.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha3_224/State`](#def-std_401_2fsha3_5f224_2fState)


## sha3_256

<a id="def-std_401_2fsha3_5f256_2fState"></a>

- `pub opaque type sha3_256.State: plain + inert uses {}`

<a id="def-std_401_2fsha3_5f256_2fdigest"></a>

- `@noalloc pub fn sha3_256.digest(data: Bytes) -> Inline[u8, 32] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha3_5f256_2fdigest_5fhex"></a>

- `pub fn sha3_256.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha3_5f256_2ffinish"></a>

- `@noalloc pub fn sha3_256.finish(take state: State) -> Inline[u8, 32] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha3_256/State`](#def-std_401_2fsha3_5f256_2fState)

<a id="def-std_401_2fsha3_5f256_2fhash"></a>

- `pub implementation sha3_256.hash: std@1/digest.Digest { type State = State size bytes = 32 size block = 136 new = new update = update finish = finish }`

References: [`std@1/sha3_256/State`](#def-std_401_2fsha3_5f256_2fState)

<a id="def-std_401_2fsha3_5f256_2fnew"></a>

- `@noalloc pub fn sha3_256.new() -> State uses {}`

References: [`std@1/sha3_256/State`](#def-std_401_2fsha3_5f256_2fState)

<a id="def-std_401_2fsha3_5f256_2fupdate"></a>

- `@noalloc pub fn sha3_256.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha3_256/State`](#def-std_401_2fsha3_5f256_2fState)


## sha3_384

<a id="def-std_401_2fsha3_5f384_2fState"></a>

- `pub opaque type sha3_384.State: plain + inert uses {}`

<a id="def-std_401_2fsha3_5f384_2fdigest"></a>

- `@noalloc pub fn sha3_384.digest(data: Bytes) -> Inline[u8, 48] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha3_5f384_2fdigest_5fhex"></a>

- `pub fn sha3_384.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha3_5f384_2ffinish"></a>

- `@noalloc pub fn sha3_384.finish(take state: State) -> Inline[u8, 48] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha3_384/State`](#def-std_401_2fsha3_5f384_2fState)

<a id="def-std_401_2fsha3_5f384_2fhash"></a>

- `pub implementation sha3_384.hash: std@1/digest.Digest { type State = State size bytes = 48 size block = 104 new = new update = update finish = finish }`

References: [`std@1/sha3_384/State`](#def-std_401_2fsha3_5f384_2fState)

<a id="def-std_401_2fsha3_5f384_2fnew"></a>

- `@noalloc pub fn sha3_384.new() -> State uses {}`

References: [`std@1/sha3_384/State`](#def-std_401_2fsha3_5f384_2fState)

<a id="def-std_401_2fsha3_5f384_2fupdate"></a>

- `@noalloc pub fn sha3_384.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha3_384/State`](#def-std_401_2fsha3_5f384_2fState)


## sha3_512

<a id="def-std_401_2fsha3_5f512_2fState"></a>

- `pub opaque type sha3_512.State: plain + inert uses {}`

<a id="def-std_401_2fsha3_5f512_2fdigest"></a>

- `@noalloc pub fn sha3_512.digest(data: Bytes) -> Inline[u8, 64] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha3_5f512_2fdigest_5fhex"></a>

- `pub fn sha3_512.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha3_5f512_2ffinish"></a>

- `@noalloc pub fn sha3_512.finish(take state: State) -> Inline[u8, 64] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha3_512/State`](#def-std_401_2fsha3_5f512_2fState)

<a id="def-std_401_2fsha3_5f512_2fhash"></a>

- `pub implementation sha3_512.hash: std@1/digest.Digest { type State = State size bytes = 64 size block = 72 new = new update = update finish = finish }`

References: [`std@1/sha3_512/State`](#def-std_401_2fsha3_5f512_2fState)

<a id="def-std_401_2fsha3_5f512_2fnew"></a>

- `@noalloc pub fn sha3_512.new() -> State uses {}`

References: [`std@1/sha3_512/State`](#def-std_401_2fsha3_5f512_2fState)

<a id="def-std_401_2fsha3_5f512_2fupdate"></a>

- `@noalloc pub fn sha3_512.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha3_512/State`](#def-std_401_2fsha3_5f512_2fState)


## sha512

<a id="def-std_401_2fsha512_2fState"></a>

- `pub opaque type sha512.State: plain + inert uses {}`

<a id="def-std_401_2fsha512_2fdigest"></a>

- `@noalloc pub fn sha512.digest(data: Bytes) -> Inline[u8, 64] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha512_2fdigest_5fhex"></a>

- `pub fn sha512.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha512_2ffinish"></a>

- `@noalloc pub fn sha512.finish(take state: State) -> Inline[u8, 64] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha512/State`](#def-std_401_2fsha512_2fState)

<a id="def-std_401_2fsha512_2fhash"></a>

- `pub implementation sha512.hash: std@1/digest.Digest { type State = State size bytes = 64 size block = 128 new = new update = update finish = finish }`

References: [`std@1/sha512/State`](#def-std_401_2fsha512_2fState)

<a id="def-std_401_2fsha512_2fnew"></a>

- `@noalloc pub fn sha512.new() -> State uses {}`

References: [`std@1/sha512/State`](#def-std_401_2fsha512_2fState)

<a id="def-std_401_2fsha512_2fupdate"></a>

- `@noalloc pub fn sha512.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha512/State`](#def-std_401_2fsha512_2fState)


## sha512_256

<a id="def-std_401_2fsha512_5f256_2fState"></a>

- `pub opaque type sha512_256.State: plain + inert uses {}`

<a id="def-std_401_2fsha512_5f256_2fdigest"></a>

- `@noalloc pub fn sha512_256.digest(data: Bytes) -> Inline[u8, 32] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fsha512_5f256_2fdigest_5fhex"></a>

- `pub fn sha512_256.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fsha512_5f256_2ffinish"></a>

- `@noalloc pub fn sha512_256.finish(take state: State) -> Inline[u8, 32] uses {}`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sha512_256/State`](#def-std_401_2fsha512_5f256_2fState)

<a id="def-std_401_2fsha512_5f256_2fhash"></a>

- `pub implementation sha512_256.hash: std@1/digest.Digest { type State = State size bytes = 32 size block = 128 new = new update = update finish = finish }`

References: [`std@1/sha512_256/State`](#def-std_401_2fsha512_5f256_2fState)

<a id="def-std_401_2fsha512_5f256_2fnew"></a>

- `@noalloc pub fn sha512_256.new() -> State uses {}`

References: [`std@1/sha512_256/State`](#def-std_401_2fsha512_5f256_2fState)

<a id="def-std_401_2fsha512_5f256_2fupdate"></a>

- `@noalloc pub fn sha512_256.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/sha512_256/State`](#def-std_401_2fsha512_5f256_2fState)


## shake128

<a id="def-std_401_2fshake128_2fState"></a>

- `pub opaque type shake128.State: plain + inert uses {}`

<a id="def-std_401_2fshake128_2fdigest"></a>

- `pub fn shake128.digest(data: Bytes, length: u64) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fshake128_2ffinish"></a>

- `pub fn shake128.finish(take state: State, length: u64) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64), [`std@1/shake128/State`](#def-std_401_2fshake128_2fState)

<a id="def-std_401_2fshake128_2fnew"></a>

- `@noalloc pub fn shake128.new() -> State uses {}`

References: [`std@1/shake128/State`](#def-std_401_2fshake128_2fState)

<a id="def-std_401_2fshake128_2fupdate"></a>

- `@noalloc pub fn shake128.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/shake128/State`](#def-std_401_2fshake128_2fState)


## shake256

<a id="def-std_401_2fshake256_2fState"></a>

- `pub opaque type shake256.State: plain + inert uses {}`

<a id="def-std_401_2fshake256_2fdigest"></a>

- `pub fn shake256.digest(data: Bytes, length: u64) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fshake256_2ffinish"></a>

- `pub fn shake256.finish(take state: State, length: u64) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64), [`std@1/shake256/State`](#def-std_401_2fshake256_2fState)

<a id="def-std_401_2fshake256_2fnew"></a>

- `@noalloc pub fn shake256.new() -> State uses {}`

References: [`std@1/shake256/State`](#def-std_401_2fshake256_2fState)

<a id="def-std_401_2fshake256_2fupdate"></a>

- `@noalloc pub fn shake256.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/shake256/State`](#def-std_401_2fshake256_2fState)


## shared

<a id="def-core_401_2fshared_2ffreeze"></a>

- `fn shared.freeze[T: inert](take x: unique T) -> Shared[T]`

<a id="def-core_401_2fshared_2fthaw"></a>

- `fn shared.thaw[T](take s: Shared[T]) -> Result[unique T, Shared[T]]`

<a id="def-core_401_2fshared_2fwith"></a>

- `@noalloc fn shared.with[T, R](s: Shared[T], body: fn(v: T) -> R) -> R`


## shlex

<a id="def-std_401_2fshlex_2fSplitError"></a>

- `pub type shlex.SplitError = UnclosedQuote(u64) | TrailingEscape(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fshlex_2fSplitError_23TrailingEscape"></a>

- `TrailingEscape(u64) -> shlex.SplitError`

Variant of shlex.SplitError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fshlex_2fSplitError_23UnclosedQuote"></a>

- `UnclosedQuote(u64) -> shlex.SplitError`

Variant of shlex.SplitError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fshlex_2fjoin"></a>

- `pub fn shlex.join(words: List[String]) -> unique String uses {}`

Quotes each word and joins them with single spaces, as Python `shlex.join`;
`split` of the result returns the words.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fshlex_2fquote"></a>

- `pub fn shlex.quote(word: String) -> unique String uses {}`

Quotes a word for a POSIX shell, as Python `shlex.quote`: a nonempty word of
ASCII letters, digits and `%+,-./:=@_` is unchanged; any other word is
enclosed in single quotes, each `'` written as `'"'"'`.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fshlex_2fsplit"></a>

- `pub fn shlex.split(line: String) -> Result[unique List[String], SplitError] uses {}`

Splits a command line into words as POSIX shells do, as Python
`shlex.split(line)`: space, tab, CR and LF separate words; single quotes keep
their contents literally; inside double quotes a backslash escapes only `"`
and `\`; elsewhere a backslash escapes the next character. Adjacent quoted
and unquoted parts join into one word, and `""` is an empty word. There is no
expansion of any kind, and `#` is an ordinary character.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/shlex/SplitError`](#def-std_401_2fshlex_2fSplitError)

<a id="def-std_401_2fshlex_2fsplit_5fcomments"></a>

- `pub fn shlex.split_comments(line: String) -> Result[unique List[String], SplitError] uses {}`

`split`, except that an unquoted, unescaped `#` starts a comment through the
end of its line, ending any word in progress, as Python
`shlex.split(line, comments=True)`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/shlex/SplitError`](#def-std_401_2fshlex_2fSplitError)


## signal

<a id="def-core_401_2fsignal_2fwait"></a>

- `@noalloc fn signal.wait(s: Signals, kinds: List[SignalKind]) -> Result[SignalKind, SignalError]`


## simd

<a id="def-core_401_2fsimd_2fadd"></a>

- `@noalloc fn simd.add[I, n: size](a: Simd[I, n], b: Simd[I, n]) -> Simd[I, n]`

<a id="def-core_401_2fsimd_2fsplat"></a>

- `@noalloc fn simd.splat[A, n: size](value: A) -> Simd[A, n]`


## siphash

<a id="def-std_401_2fsiphash_2fState"></a>

- `pub opaque type siphash.State: plain + inert uses {}`

<a id="def-std_401_2fsiphash_2fdigest"></a>

- `@noalloc pub fn siphash.digest(k0: u64, k1: u64, data: Bytes) -> u64 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsiphash_2ffinish"></a>

- `@noalloc pub fn siphash.finish(state: State) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2ffresh"></a>

- `@noalloc pub fn siphash.fresh(state: State) -> State uses {}`

References: [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fhash_5fone"></a>

- `pub fn siphash.hash_one[T](value: T, writer: fn(T, mut State) -> () uses {}, k0: u64, k1: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fnew"></a>

- `@noalloc pub fn siphash.new(k0: u64, k1: u64) -> State uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite"></a>

- `@noalloc pub fn siphash.write(mut s: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fbool"></a>

- `@noalloc pub fn siphash.write_bool(mut s: State, value: bool) -> () uses {}`

References: [`core@1/bool`](#def-core_401_2fbool), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fi128"></a>

- `@noalloc pub fn siphash.write_i128(mut s: State, value: i128) -> () uses {}`

References: [`core@1/i128`](#def-core_401_2fi128), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fi16"></a>

- `@noalloc pub fn siphash.write_i16(mut s: State, value: i16) -> () uses {}`

References: [`core@1/i16`](#def-core_401_2fi16), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fi32"></a>

- `@noalloc pub fn siphash.write_i32(mut s: State, value: i32) -> () uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fi64"></a>

- `@noalloc pub fn siphash.write_i64(mut s: State, value: i64) -> () uses {}`

References: [`core@1/i64`](#def-core_401_2fi64), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fi8"></a>

- `@noalloc pub fn siphash.write_i8(mut s: State, value: i8) -> () uses {}`

References: [`core@1/i8`](#def-core_401_2fi8), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fstr"></a>

- `@noalloc pub fn siphash.write_str(mut s: State, text: String) -> () uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fu128"></a>

- `@noalloc pub fn siphash.write_u128(mut s: State, value: u128) -> () uses {}`

References: [`core@1/u128`](#def-core_401_2fu128), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fu16"></a>

- `@noalloc pub fn siphash.write_u16(mut s: State, value: u16) -> () uses {}`

References: [`core@1/u16`](#def-core_401_2fu16), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fu32"></a>

- `@noalloc pub fn siphash.write_u32(mut s: State, value: u32) -> () uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fu64"></a>

- `@noalloc pub fn siphash.write_u64(mut s: State, value: u64) -> () uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)

<a id="def-std_401_2fsiphash_2fwrite_5fu8"></a>

- `@noalloc pub fn siphash.write_u8(mut s: State, value: u8) -> () uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/siphash/State`](#def-std_401_2fsiphash_2fState)


## slots

<a id="def-core_401_2fslots_2fempty"></a>

- `fn slots.empty[T]() -> Slots[T]`

<a id="def-core_401_2fslots_2fget"></a>

- `@noalloc fn slots.get[T](s: Slots[T], id: SlotId[T] of s) -> Option[T] in s`

<a id="def-core_401_2fslots_2finsert"></a>

- `@preserves_unique(s) fn slots.insert[T](mut s: Slots[T], take value: T) -> SlotId[T] of s`

<a id="def-core_401_2fslots_2fremove"></a>

- `fn slots.remove[T](mut s: Slots[T], id: SlotId[T] of s) -> Option[T]`


## sorted

<a id="def-std_401_2fsorted_2fequal_5frange"></a>

- `@noalloc pub fn sorted.equal_range[T](xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {}) -> ( u64, u64 ) uses {}`

Half-open equal range (lower_bound, upper_bound); absent values give (i, i).
Searches the lower boundary first, then only its suffix for the upper one.
Failures propagate immediately with ordinary source attribution and cleanup.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsorted_2flower_5fbound"></a>

- `@noalloc pub fn sorted.lower_bound[T](xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {}) -> u64 uses {}`

First index i with by(xs[i], value) >= 0, or len(xs).
Input must be sorted by the same ordering. Borrows elements; never clones.
Calls by only at actual midpoints. Empty input makes no calls.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsorted_2fupper_5fbound"></a>

- `@noalloc pub fn sorted.upper_bound[T](xs: List[T], value: T, by: @noalloc fn(T, T) -> i32 uses {}) -> u64 uses {}`

First index i with by(xs[i], value) > 0, or len(xs).
Same sorted-input, borrowing and comparator restrictions as lower_bound.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i32`](#def-core_401_2fi32), [`core@1/u64`](#def-core_401_2fu64)


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


## sse

<a id="def-std_401_2fsse_2fEvent"></a>

- `pub type sse.Event { event: String, data: String, id: Option[String], retry: Option[u64], }`

One dispatched event. Read: `event` is the type (`message` when unnamed), `id`
the stream's last event ID (`None()` while empty) and `retry` the reconnection
time in milliseconds when this event's block set one. Written: an `event` of
`message` or empty is omitted, as are `None()` fields.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fsse_2fParser"></a>

- `pub opaque type sse.Parser: copyable + inert + noalloc_drop uses {}`

Incremental event-stream decoder: `push` bytes, then take events with `next`.

<a id="def-std_401_2fsse_2fSseError"></a>

- `pub type sse.SseError = Http(http.HttpError) | Net(NetError) | TooLarge | NotEventStream | InvalidField`

References: [`core@1/NetError`](#def-core_401_2fNetError), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fsse_2fSseError_23Http"></a>

- `Http(http.HttpError) -> sse.SseError`

Variant of sse.SseError.

References: [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fsse_2fSseError_23InvalidField"></a>

- `InvalidField -> sse.SseError`

Variant of sse.SseError.

<a id="def-std_401_2fsse_2fSseError_23Net"></a>

- `Net(NetError) -> sse.SseError`

Variant of sse.SseError.

References: [`core@1/NetError`](#def-core_401_2fNetError)

<a id="def-std_401_2fsse_2fSseError_23NotEventStream"></a>

- `NotEventStream -> sse.SseError`

Variant of sse.SseError.

<a id="def-std_401_2fsse_2fSseError_23TooLarge"></a>

- `TooLarge -> sse.SseError`

Variant of sse.SseError.

<a id="def-std_401_2fsse_2fWriter"></a>

- `pub opaque resource type sse.Writer: noalloc_drop uses {Net}`

The server end of one event stream; dropping it closes the connection.

References: [`core@1/Net`](#def-core_401_2fNet)

<a id="def-std_401_2fsse_2fcomment"></a>

- `pub fn sse.comment(mut w: Writer, note: String) -> Result[(), SseError]`

Writes a comment line, which readers ignore: a keep-alive. CR or LF in `note` is
`InvalidField()`.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError), [`std@1/sse/Writer`](#def-std_401_2fsse_2fWriter)

<a id="def-std_401_2fsse_2fencode"></a>

- `pub fn sse.encode(event: Event) -> Result[unique String, SseError] uses {}`

`event` in wire form, as `encode_into` appends it.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/sse/Event`](#def-std_401_2fsse_2fEvent), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError)

<a id="def-std_401_2fsse_2fencode_5finto"></a>

- `@preserves_unique(out) pub fn sse.encode_into(event: Event, mut out: List[u8]) -> Result[(), SseError] uses {}`

Appends `event` in wire form: `event`, `id` and `retry` fields, one `data` field
per line of `data` (split at CRLF, LF or CR) and the blank line. An event type or
id containing CR, LF or NUL is `InvalidField()`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/sse/Event`](#def-std_401_2fsse_2fEvent), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError)

<a id="def-std_401_2fsse_2flast_5fevent_5fid"></a>

- `pub fn sse.last_event_id(p: Parser) -> unique String uses {}`

The last event ID the stream set, empty when none.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser)

<a id="def-std_401_2fsse_2fnext"></a>

- `pub fn sse.next(mut p: Parser) -> Option[Event] uses {}`

The oldest queued event.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/sse/Event`](#def-std_401_2fsse_2fEvent), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser)

<a id="def-std_401_2fsse_2fparser"></a>

- `pub fn sse.parser(max_event_bytes: u64) -> Parser uses {}`

A decoder that rejects an event block, or one line, above `max_event_bytes`.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser)

<a id="def-std_401_2fsse_2fpush"></a>

- `pub fn sse.push(mut p: Parser, chunk: Bytes) -> Result[(), SseError] uses {}`

Decodes `chunk`, queueing every event whose block it completes. A stream may
start with a UTF-8 byte order mark; lines end with CRLF, LF or CR. A final block
without its blank line is never dispatched.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError)

<a id="def-std_401_2fsse_2fread_5fevent"></a>

- `pub fn sse.read_event(mut r: http.Request, mut p: Parser) -> Result[Option[Event], SseError]`

The next event of the response body `r`, decoded by `p`; `None()` at the end of
the body. Keep one parser per stream, seeded with `set_last_event_id` to resume.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/sse/Event`](#def-std_401_2fsse_2fEvent), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError)

<a id="def-std_401_2fsse_2frequest"></a>

- `pub fn sse.request( n: Net, config: http.Config, url: String, headers: List[http.Header], resume_from: String ) -> Result[http.Request, SseError]`

Opens an event stream: a GET of `url` with `accept: text/event-stream`,
`cache-control: no-cache`, `last-event-id` when `resume_from` is nonempty, and
`headers`. A final status other than 200 is `Http(Status(code))`; a content type
other than `text/event-stream` is `NotEventStream()`. Read it with `read_event`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError)

<a id="def-std_401_2fsse_2frespond"></a>

- `pub fn sse.respond(take c: http.ServerStream, headers: List[http.Header]) -> Result[ Writer, SseError ]`

Answers the request `next_stream` yielded with `c` with a 200 event stream: `content-type:
text/event-stream`, `cache-control: no-cache`, `connection: close` and
`headers`, with no length, so the body ends when the stream does. Framing fields
among `headers` are `Http(Malformed(ContentLength()))`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError), [`std@1/sse/Writer`](#def-std_401_2fsse_2fWriter)

<a id="def-std_401_2fsse_2fsend"></a>

- `pub fn sse.send(mut w: Writer, event: Event) -> Result[(), SseError]`

Writes one event.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/sse/Event`](#def-std_401_2fsse_2fEvent), [`std@1/sse/SseError`](#def-std_401_2fsse_2fSseError), [`std@1/sse/Writer`](#def-std_401_2fsse_2fWriter)

<a id="def-std_401_2fsse_2fset_5flast_5fevent_5fid"></a>

- `pub fn sse.set_last_event_id(mut p: Parser, id: String) -> () uses {}`

Starts the last event ID at `id`, as a reconnecting client does.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/sse/Parser`](#def-std_401_2fsse_2fParser)


## stdin

<a id="def-core_401_2fstdin_2fread"></a>

- `@preserves_unique(buf) @unretained(s) fn stdin.read(s: Stdin, mut buf: List[u8], max: u64) -> Result[u64, InputError]`


## store

<a id="def-std_401_2fstore_2fBackend"></a>

- `pub type store.Backend[S] { read: fn(mut state: S, String) -> Result[Bytes, FileError] uses {state}, put_new: fn(mut state: S, String, Bytes, Durability) -> Result[(), FileError] uses {state}, list: fn(mut state: S, String) -> Result[List[String], FileError] uses {state}, remove: fn(mut state: S, String) -> Result[(), FileError] uses {state}, mkdir: fn(mut state: S, String) -> Result[(), FileError] uses {state}, }`

The storage seam: immutable whole-object reads, conditional creation,
ordered prefix discovery, deletion and directory setup. A successful durable
create acknowledges the exact bytes; an uncertain create must be read back.
Implementations may issue range GETs and cache immutable data behind `read`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Durability`](#def-core_401_2fDurability), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstore_2fCallFailure"></a>

- `@serializable pub type store.CallFailure[E] = Tx(TxFailure[E]) | Denied | Rejected`

Why a call returned no outcome of its own: `Tx(f)` as for `transact`;
`Denied()` when a later executor epoch holds the stream; `Rejected()` for a
position the stream cannot take, or a retained position with another digest.

References: [`std@1/store/TxFailure`](#def-std_401_2fstore_2fTxFailure)

<a id="def-std_401_2fstore_2fCallFailure_23Denied"></a>

- `Denied -> store.CallFailure[E]`

Variant of store.CallFailure.

<a id="def-std_401_2fstore_2fCallFailure_23Rejected"></a>

- `Rejected -> store.CallFailure[E]`

Variant of store.CallFailure.

<a id="def-std_401_2fstore_2fCallFailure_23Tx"></a>

- `Tx(TxFailure[E]) -> store.CallFailure[E]`

Variant of store.CallFailure.

References: [`std@1/store/TxFailure`](#def-std_401_2fstore_2fTxFailure)

<a id="def-std_401_2fstore_2fCallRequest"></a>

- `@serializable pub type store.CallRequest { stream: Bytes, epoch: u64, position: u64, digest: Bytes, acknowledged: u64, }`

One call on an ordered call stream: the caller's stream identity and
executor epoch, the call's position (consecutive from 1), the digest of
the request, and the highest position whose outcome the caller has made
durable, which the store may then forget.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fstore_2fIsolation"></a>

- `pub type store.Isolation = SnapshotIsolation | SerializableSnapshot`

How a transaction's commit detects conflicts, after SlateDB's
IsolationLevel (transaction_manager.rs:13). `SnapshotIsolation` conflicts
with a later commit that wrote a key it writes or marked read;
`SerializableSnapshot` also with one that wrote a key it read with
`txn.get` or inside a range it read with `txn.range`.

<a id="def-std_401_2fstore_2fIsolation_23SerializableSnapshot"></a>

- `SerializableSnapshot -> store.Isolation`

Variant of store.Isolation.

<a id="def-std_401_2fstore_2fIsolation_23SnapshotIsolation"></a>

- `SnapshotIsolation -> store.Isolation`

Variant of store.Isolation.

<a id="def-std_401_2fstore_2fSnapshot"></a>

- `pub opaque type store.Snapshot: copyable uses {}`

A stable view of the store as of one commit: later commits never change it.

<a id="def-std_401_2fstore_2fStore"></a>

- `@captures(backend) pub opaque type store.Store[S]: () uses {S}`

The writer of one store directory.

<a id="def-std_401_2fstore_2fStoreConfig"></a>

- `pub type store.StoreConfig { durability: Durability, group_limit: u64, faults: StoreFaults, limits: StoreLimits, }`

How the store makes commits survive a crash, and where it may stop for a
test. `durability` is the mode of every log object: `Durable()` returns a
commit's ticket from `durable` only once its group is synced, `Volatile()`
writes groups without syncing, so a crash may lose any suffix of commits
(never part of one). `group_limit` commits share one log object: a commit
that fills the group writes it, and `durable` writes a partial group; 1
writes every commit on its own. `faults` injects a stop for crash tests.

References: [`core@1/Durability`](#def-core_401_2fDurability), [`core@1/u64`](#def-core_401_2fu64), [`std@1/store/StoreFaults`](#def-std_401_2fstore_2fStoreFaults), [`std@1/store/StoreLimits`](#def-std_401_2fstore_2fStoreLimits)

<a id="def-std_401_2fstore_2fStoreError"></a>

- `@serializable pub type store.StoreError = Storage(FileError) | Corrupt(String) | Fenced | Conflict | Injected | Closed`

Why a store operation failed. `Storage` is the Dir's own error; `Corrupt`
names an object that fails its checksum or the layout's rules before the
log's tail; `Fenced` means another writer claimed the store; `Conflict` means
a commit after the transaction began wrote a key it conflicts on; `Injected` is a
`StopAfterWrites` stop. After `Storage`, `Corrupt`, `Fenced` or `Injected`
the store is closed and every later call returns `Closed`.

References: [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstore_2fStoreError_23Closed"></a>

- `Closed -> store.StoreError`

Variant of store.StoreError.

<a id="def-std_401_2fstore_2fStoreError_23Conflict"></a>

- `Conflict -> store.StoreError`

Variant of store.StoreError.

<a id="def-std_401_2fstore_2fStoreError_23Corrupt"></a>

- `Corrupt(String) -> store.StoreError`

Variant of store.StoreError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstore_2fStoreError_23Fenced"></a>

- `Fenced -> store.StoreError`

Variant of store.StoreError.

<a id="def-std_401_2fstore_2fStoreError_23Injected"></a>

- `Injected -> store.StoreError`

Variant of store.StoreError.

<a id="def-std_401_2fstore_2fStoreError_23Storage"></a>

- `Storage(FileError) -> store.StoreError`

Variant of store.StoreError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2fstore_2fStoreFaults"></a>

- `pub type store.StoreFaults = NoFaults | StopAfterWrites(u64)`

A test seam: `StopAfterWrites(n)` refuses the store's storage mutation after
its first `n` (creates, removals and directory creations, from `open` on),
with no effect, as a crash at that point would; the store then closes.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fstore_2fStoreFaults_23NoFaults"></a>

- `NoFaults -> store.StoreFaults`

Variant of store.StoreFaults.

<a id="def-std_401_2fstore_2fStoreFaults_23StopAfterWrites"></a>

- `StopAfterWrites(u64) -> store.StoreFaults`

Variant of store.StoreFaults.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fstore_2fStoreLimits"></a>

- `pub type store.StoreLimits { key_bytes: u64, value_bytes: u64, transaction_bytes: u64, range_rows: u64, }`

The most a `transact` commit may write: any key over `key_bytes`, any value
over `value_bytes`, or keys and values together over `transaction_bytes`
refuse the commit with `Limit(..)` and no effect.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fstore_2fTicket"></a>

- `pub opaque type store.Ticket: copyable + plain + data uses {}`

A commit's place in the store's commit order, awaited with `durable`.

<a id="def-std_401_2fstore_2fTx"></a>

- `@capability(local) pub opaque type store.Tx: () uses {}`

The scoped capability a `transact` body holds: reads see the commit the
transaction began at plus its own writes. It cannot leave the body.

<a id="def-std_401_2fstore_2fTxFailure"></a>

- `@serializable pub type store.TxFailure[E] = Aborted(E) | Canceled(CancelCause) | Storage(StoreError) | Limit( TxLimit ) | Unknown(StoreError)`

How a transaction ended without its outcome: `Aborted(e)` when the body
returned `Error(e)` and its writes were discarded; `Canceled(cause)` before
the body started (only the call-stream form is canceled); `Storage(e)` and
`Limit(..)` when nothing was committed; `Unknown(e)` when the commit was
attempted and the store could not tell whether it became durable, which the
next `open` decides.

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError), [`std@1/store/TxLimit`](#def-std_401_2fstore_2fTxLimit)

<a id="def-std_401_2fstore_2fTxFailure_23Aborted"></a>

- `Aborted(E) -> store.TxFailure[E]`

Variant of store.TxFailure.

<a id="def-std_401_2fstore_2fTxFailure_23Canceled"></a>

- `Canceled(CancelCause) -> store.TxFailure[E]`

Variant of store.TxFailure.

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause)

<a id="def-std_401_2fstore_2fTxFailure_23Limit"></a>

- `Limit( TxLimit ) -> store.TxFailure[E]`

Variant of store.TxFailure.

References: [`std@1/store/TxLimit`](#def-std_401_2fstore_2fTxLimit)

<a id="def-std_401_2fstore_2fTxFailure_23Storage"></a>

- `Storage(StoreError) -> store.TxFailure[E]`

Variant of store.TxFailure.

References: [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fTxFailure_23Unknown"></a>

- `Unknown(StoreError) -> store.TxFailure[E]`

Variant of store.TxFailure.

References: [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fTxLimit"></a>

- `@serializable pub type store.TxLimit = KeyBytes | ValueBytes | TransactionBytes | RangeRows`

Which of the configured limits a commit exceeded.

<a id="def-std_401_2fstore_2fTxLimit_23KeyBytes"></a>

- `KeyBytes -> store.TxLimit`

Variant of store.TxLimit.

<a id="def-std_401_2fstore_2fTxLimit_23RangeRows"></a>

- `RangeRows -> store.TxLimit`

Variant of store.TxLimit.

<a id="def-std_401_2fstore_2fTxLimit_23TransactionBytes"></a>

- `TransactionBytes -> store.TxLimit`

Variant of store.TxLimit.

<a id="def-std_401_2fstore_2fTxLimit_23ValueBytes"></a>

- `ValueBytes -> store.TxLimit`

Variant of store.TxLimit.

<a id="def-std_401_2fstore_2fTxn"></a>

- `pub opaque type store.Txn: copyable uses {}`

A transaction: reads see the snapshot it began at plus its own writes.
Dropping it aborts it; `rollback` also frees its conflict tracking at once.

<a id="def-std_401_2fstore_2fbegin"></a>

- `pub fn store.begin[S](mut s: Store[S]) -> Txn uses {}`

Starts a serializable transaction at the latest commit.

References: [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2fstore_2fbegin_5fwith"></a>

- `pub fn store.begin_with[S](mut s: Store[S], isolation: Isolation) -> Txn uses {}`

Starts a transaction at the latest commit under `isolation`, registered
with the store's transaction manager (DbTransaction::begin).

References: [`std@1/store/Isolation`](#def-std_401_2fstore_2fIsolation), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2fstore_2fcall"></a>

- `pub fn store.call[S, R: inert + serializable, E: inert + serializable]( mut s: Store[S], req: CallRequest, body: fn(mut tx: Tx) -> Result[R, E] uses {} ) -> Result[R, CallFailure[E]] uses {s}`

`transact` as one call on an ordered call stream, exactly once per
position. The stream's row decides the request in the transaction that
would run it: an epoch below the row's is `Denied()`; a position at or
below the acknowledged one, above the next one, or retained with another
digest is `Rejected()`; a retained position with an equal digest returns
its recorded outcome without running `body`; the next position runs
`body`, and the same commit records the outcome in the row, so an outcome
is recorded exactly when its writes commit. An outcome that writes
nothing (an abort, a limit) is recorded by a commit of the row alone.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/CallFailure`](#def-std_401_2fstore_2fCallFailure), [`std@1/store/CallRequest`](#def-std_401_2fstore_2fCallRequest), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx)

<a id="def-std_401_2fstore_2fclose"></a>

- `pub fn store.close[S](take store: Store[S]) -> Result[(), StoreError] uses {store}`

Writes any pending commits and closes the store.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fcommit"></a>

- `pub fn store.commit[S](mut s: Store[S], take t: Txn) -> Result[Ticket, StoreError] uses {s}`

Commits `t`'s writes as the next commit, visible at once to later
snapshots and transactions, and returns its ticket. `Conflict()` when a
commit after `t` began wrote a key `t` conflicts on (see `Isolation`); a
transaction older than the store's conflict retention conflicts whenever
it writes or read anything. A commit that fills its group writes the
group's log object; await its durability with `durable`.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError), [`std@1/store/Ticket`](#def-std_401_2fstore_2fTicket), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2fstore_2fcompact"></a>

- `pub fn store.compact[S](mut s: Store[S]) -> Result[(), StoreError] uses {s}`

Writes every commit into a new snapshot object, publishes it with a new
manifest and removes the objects it supersedes. A crash at any point
recovers: until the manifest exists the old state is current, and after it
the superseded objects are garbage that the next open removes.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fcompare_5fkeys"></a>

- `@noalloc pub fn store.compare_keys(a: Bytes, b: Bytes) -> i32 uses {}`

Byte-lexicographic order, a proper prefix first: the order of every key.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2fstore_2fdefault_5fconfig"></a>

- `pub fn store.default_config() -> StoreConfig uses {}`

Durable log objects, 64 commits per group, no injected faults, and limits
of 4096-byte keys, 1 MiB values and 16 MiB per transaction.

References: [`std@1/store/StoreConfig`](#def-std_401_2fstore_2fStoreConfig)

<a id="def-std_401_2fstore_2fdurable"></a>

- `pub fn store.durable[S](mut s: Store[S], t: Ticket) -> Result[(), StoreError] uses {s}`

The output gate: returns once the commit `t` names is in a written log
object, synced when the store is `Durable()`, together with every earlier
commit. Writes the pending group if `t` is in it.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError), [`std@1/store/Ticket`](#def-std_401_2fstore_2fTicket)

<a id="def-std_401_2fstore_2fget"></a>

- `pub fn store.get(snap: Snapshot, key: Bytes) -> Option[Bytes] in snap uses {}`

The value stored under `key` in the snapshot.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/store/Snapshot`](#def-std_401_2fstore_2fSnapshot)

<a id="def-std_401_2fstore_2finto_5fstate"></a>

- `pub fn store.into_state[S](take store: Store[S]) -> Result[S, StoreError] uses {store}`

Flushes the pending group and returns the backend state, for explicit
lifecycle management, simulation observations or reopening the same medium.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2flocal_5fbackend"></a>

- `pub fn store.local_backend() -> Backend[Dir] uses {}`

The default backend: an explicitly granted local or simulated Dir.

References: [`core@1/Dir`](#def-core_401_2fDir), [`std@1/store/Backend`](#def-std_401_2fstore_2fBackend)

<a id="def-std_401_2fstore_2fopen"></a>

- `pub fn store.open(d: Dir, config: StoreConfig) -> Result[Store[Dir], StoreError] uses {d}`

Opens the store in `d`, creating its layout when `d` is empty, recovers the
newest valid state and claims the store as its writer at the next epoch.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreConfig`](#def-std_401_2fstore_2fStoreConfig), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fopen_5fbackend"></a>

- `pub fn store.open_backend[S](take state: S, take backend: Backend[S], config: StoreConfig) -> Result[ Store[S], StoreError ] uses {state, backend}`

Opens the same engine over a host-selected storage implementation. The state
is owned by the store; all effects go through this explicit function dictionary.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Backend`](#def-std_401_2fstore_2fBackend), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/StoreConfig`](#def-std_401_2fstore_2fStoreConfig), [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2fstore_2fprefix_5fbounds"></a>

- `pub fn store.prefix_bounds(prefix: Bytes) -> ( ordered_map.Bound[Bytes], ordered_map.Bound[Bytes] ) uses {}`

The bounds that select exactly the keys starting with `prefix`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound)

<a id="def-std_401_2fstore_2frange"></a>

- `pub fn store.range( snap: Snapshot, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64 ) -> List[(Bytes, Bytes)] uses {}`

At most `limit` entries of the snapshot with keys between `from` and `to`,
in `direction` order, as fresh copies. Page by passing `Excluded` of the
last key received.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound), [`std@1/ordered_map/Direction`](#def-std_401_2fordered_5fmap_2fDirection), [`std@1/store/Snapshot`](#def-std_401_2fstore_2fSnapshot)

<a id="def-std_401_2fstore_2freserved"></a>

- `pub fn store.reserved(key: Bytes) -> bool uses {}`

Whether `key` lies in `hirsel.`, the key space the store keeps for its own
rows; a transaction body writes no key there.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fstore_2frollback"></a>

- `pub fn store.rollback[S](mut s: Store[S], take t: Txn) -> () uses {}`

Ends `t` without committing it, as dropping it does, and frees what the
store tracks for its conflicts (DbTransaction::rollback).

References: [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2fstore_2fsequence"></a>

- `@noalloc pub fn store.sequence(t: Ticket) -> u64 uses {}`

The commit order position of a ticket: the n-th commit since the store
was created has sequence n.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/store/Ticket`](#def-std_401_2fstore_2fTicket)

<a id="def-std_401_2fstore_2fsnapshot"></a>

- `pub fn store.snapshot[S](mut s: Store[S]) -> Snapshot uses {}`

A stable view of every commit so far, durable or not. Later commits never
change it, and holding it never holds up the writer.

References: [`std@1/store/Snapshot`](#def-std_401_2fstore_2fSnapshot), [`std@1/store/Store`](#def-std_401_2fstore_2fStore)

<a id="def-std_401_2fstore_2fsnapshot_5fsequence"></a>

- `@noalloc pub fn store.snapshot_sequence(snap: Snapshot) -> u64 uses {}`

The commit a snapshot reflects: every commit up to it, no later one.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/store/Snapshot`](#def-std_401_2fstore_2fSnapshot)

<a id="def-std_401_2fstore_2ftransact"></a>

- `pub fn store.transact[S, R: inert, E: inert]( mut s: Store[S], body: fn(mut tx: Tx) -> Result[R, E] uses {} ) -> Result[R, TxFailure[E]] uses {s}`

Runs `body` once with a fresh `Tx` at the latest commit and commits its
writes as one commit when it returns `Ok`. The outcome, of any kind, is
returned only once every commit the body could see, and its own, is
durable: the output gate. `Error(e)` discards the writes and returns
`Aborted(e)`; a failure in the body discards them and propagates.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Store`](#def-std_401_2fstore_2fStore), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/store/TxFailure`](#def-std_401_2fstore_2fTxFailure)

<a id="def-std_401_2fstore_2ftx_5fdelete"></a>

- `pub fn store.tx_delete(mut tx: Tx, take key: Bytes) -> () uses {} requires !reserved(key)`

Removes `key` when the transaction commits.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/store/reserved`](#def-std_401_2fstore_2freserved)

<a id="def-std_401_2fstore_2ftx_5fget"></a>

- `pub fn store.tx_get(tx: Tx, key: Bytes) -> Option[Bytes] in tx uses {}`

The value under `key` as the transaction sees it.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx)

<a id="def-std_401_2fstore_2ftx_5fput"></a>

- `pub fn store.tx_put(mut tx: Tx, take key: Bytes, take value: Bytes) -> () uses {} requires !reserved(key)`

Stores `value` under `key` when the transaction commits.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/store/reserved`](#def-std_401_2fstore_2freserved)

<a id="def-std_401_2fstore_2ftx_5frange"></a>

- `pub fn store.tx_range( tx: Tx, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64 ) -> List[(Bytes, Bytes)] uses {}`

At most `limit` entries with keys between `from` and `to` as the
transaction sees them, in `direction` order, as fresh copies.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound), [`std@1/ordered_map/Direction`](#def-std_401_2fordered_5fmap_2fDirection), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx)


## stream

<a id="def-std_401_2fstream_2fBoundError"></a>

- `pub type stream.BoundError[E] = Source(E) | Limit`

A source failure before the item budget, or an extra result after it.

<a id="def-std_401_2fstream_2fBoundError_23Limit"></a>

- `Limit -> stream.BoundError[E]`

Variant of stream.BoundError.

<a id="def-std_401_2fstream_2fBoundError_23Source"></a>

- `Source(E) -> stream.BoundError[E]`

Variant of stream.BoundError.

<a id="def-std_401_2fstream_2fChunkLimit"></a>

- `pub opaque type stream.ChunkLimit: plain + sendable + inert + noalloc_drop uses {}`

A positive chunk ceiling, at most 65536 bytes. Construction performs no I/O.

<a id="def-std_401_2fstream_2fFlow"></a>

- `@captures(inner) pub opaque type stream.Flow[T, E]: () uses {}`

An owned fallible cursor. Its source and callbacks stay owned until destruction.

<a id="def-std_401_2fstream_2fStreamConfigError"></a>

- `pub type stream.StreamConfigError = InvalidChunkLimit()`

<a id="def-std_401_2fstream_2fStreamConfigError_23InvalidChunkLimit"></a>

- `InvalidChunkLimit() -> stream.StreamConfigError`

Variant of stream.StreamConfigError.

<a id="def-std_401_2fstream_2fWriteError"></a>

- `pub type stream.WriteError[E, W] = Source(E) | Write(W) | InvalidWrite`

Source and writer errors remain distinct; invalid progress is never EOF.

<a id="def-std_401_2fstream_2fWriteError_23InvalidWrite"></a>

- `InvalidWrite -> stream.WriteError[E, W]`

Variant of stream.WriteError.

<a id="def-std_401_2fstream_2fWriteError_23Source"></a>

- `Source(E) -> stream.WriteError[E, W]`

Variant of stream.WriteError.

<a id="def-std_401_2fstream_2fWriteError_23Write"></a>

- `Write(W) -> stream.WriteError[E, W]`

Variant of stream.WriteError.

<a id="def-std_401_2fstream_2fbound"></a>

- `@captures(input) pub fn stream.bound[T, E](take input: Flow[T, E], max_items: u64) -> Flow[T, BoundError[E]] uses {input}`

Validate whole-stream cardinality when drained, with one-result lookahead.
Construction and early destruction do not pull. Zero probes on first demand.
Any extra result, including an error, is dropped and reported as Limit.
Pull/drop language failures propagate normally, even during the probe.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/stream/BoundError`](#def-std_401_2fstream_2fBoundError), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fchunk_5flimit"></a>

- `pub fn stream.chunk_limit(bytes: u64) -> Result[ChunkLimit, StreamConfigError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/stream/ChunkLimit`](#def-std_401_2fstream_2fChunkLimit), [`std@1/stream/StreamConfigError`](#def-std_401_2fstream_2fStreamConfigError)

<a id="def-std_401_2fstream_2fcsv_5ffold_5fscoped"></a>

- `pub fn stream.csv_fold_scoped[S, A, E]( scope: Scope, take source: S, config: csv.StreamConfig, take initial: A, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], accept: fn(List[String]) -> Result[bool, E], step: fn(take A, take List[String]) -> Result[A, E] ) -> Result[A, csv.DriverError[E]] uses {scope, source, initial, refill, accept, step}`

Own and consume a CSV source; checks include rejected rows and every refill.
Callbacks are scoped and never stored in the returned value or an iterator.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Scope`](#def-core_401_2fScope), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/csv/DriverError`](#def-std_401_2fcsv_2fDriverError), [`std@1/csv/StreamConfig`](#def-std_401_2fcsv_2fStreamConfig)

<a id="def-std_401_2fstream_2ffile_5fchunks"></a>

- `@captures(dir, file) pub fn stream.file_chunks(dir: Dir, take file: File of dir, limit: ChunkLimit) -> Flow[ Bytes, FileError ] uses {dir, file}`

Own the exact branded file; read only on demand, with no path reopening.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`std@1/stream/ChunkLimit`](#def-std_401_2fstream_2fChunkLimit), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ffile_5fsink"></a>

- `pub fn stream.file_sink[E]( dir: Dir, take destination: AtomicFile of dir, take input: Flow[Bytes, E] ) -> Result[(), WriteError[E, FileError]] uses {dir, destination, input}`

Commit an owned atomic file only after clean upstream EOF; drop aborts.

References: [`core@1/AtomicFile`](#def-core_401_2fAtomicFile), [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Dir`](#def-core_401_2fDir), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow), [`std@1/stream/WriteError`](#def-std_401_2fstream_2fWriteError)

<a id="def-std_401_2fstream_2ffrom"></a>

- `@captures(state, step) pub fn stream.from[S, T, E]( take state: S, take step: @preserves_captures(cursor) fn(mut cursor: S) -> Result[Option[T], E] ) -> Flow[T, E] uses {state, step}`

Own state and its preserving callback without pulling. Error or EOF ends it.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ffrom_5fresults"></a>

- `@captures(input) pub fn stream.from_results[T, E](take input: Iter[Result[T, E]]) -> Flow[T, E] uses {input}`

Normalize a raw result iterator, ending at its first error or EOF.

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fhttp_5fchunks"></a>

- `@captures(request) pub fn stream.http_chunks(take request: http.Request, limit: ChunkLimit) -> Flow[ Bytes, http.HttpError ] uses {request}`

Consume an already prepared response body as raw bytes; discard trailers.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/stream/ChunkLimit`](#def-std_401_2fstream_2fChunkLimit), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fhttp_5fupload"></a>

- `pub fn stream.http_upload[E]( take request: http.Request, take input: Flow[Bytes, E], trailers: List[http.Header] ) -> Result[http.Request, WriteError[E, http.HttpError]] uses {request, input}`

Finish a streaming upload only on clean EOF. On success return the Request
for receive_head; errors drop/abort it without a successful terminator.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/http/Request`](#def-std_401_2fhttp_2fRequest), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow), [`std@1/stream/WriteError`](#def-std_401_2fstream_2fWriteError)

<a id="def-std_401_2fstream_2fnet_5fchunks"></a>

- `@captures(net, socket) pub fn stream.net_chunks(net: Net, take socket: Stream of net, limit: ChunkLimit) -> Flow[ Bytes, NetError ] uses {net, socket}`

Own a byte-stream socket, yielding short reads unchanged. This is not UDP.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Stream`](#def-core_401_2fStream), [`std@1/stream/ChunkLimit`](#def-std_401_2fstream_2fChunkLimit), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fnet_5fsink"></a>

- `pub fn stream.net_sink[E](net: Net, take socket: Stream of net, take input: Flow[Bytes, E]) -> Result[ (), WriteError[E, NetError] ] uses {net, socket, input}`

Consume a socket and all partial writes; drop closes it on every exit.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Net`](#def-core_401_2fNet), [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stream`](#def-core_401_2fStream), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow), [`std@1/stream/WriteError`](#def-std_401_2fstream_2fWriteError)

<a id="def-std_401_2fstream_2fnext"></a>

- `@advance_only(flow) pub fn stream.next[T, E](mut flow: Flow[T, E]) -> Option[Result[T, E]] uses {flow}`

Advance once. An error is reported once; later calls return None.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ftake_5fn"></a>

- `@captures(input) pub fn stream.take_n[T, E](take input: Flow[T, E], count: u64) -> Flow[T, E] uses {input}`

Own at most count upstream pulls; zero performs no work.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fto_5fiter"></a>

- `@captures(input) pub fn stream.to_iter[T, E](take input: Flow[T, E]) -> Iter[Result[T, E]] uses {input}`

Leave the error-aware namespace explicitly, retaining terminal pull behavior.

References: [`core@1/Iter`](#def-core_401_2fIter), [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ftry_5ffilter"></a>

- `@captures(input, predicate) pub fn stream.try_filter[T, E]( take input: Flow[T, E], take predicate: fn(T) -> Result[bool, E] ) -> Flow[T, E] uses {input, predicate}`

Test successful items in order, dropping rejections; errors end the flow.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ftry_5ffold"></a>

- `pub fn stream.try_fold[T, A, E]( take input: Flow[T, E], take initial: A, take step: fn(take A, take T) -> Result[A, E] ) -> Result[A, E] uses {input, initial, step}`

Consume through the first error or clean end, releasing the owned pipeline.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ftry_5ffor_5feach"></a>

- `pub fn stream.try_for_each[T, E]( take input: Flow[T, E], take visit: fn(take T) -> Result[(), E] ) -> Result[(), E] uses {input, visit}`

Visit successful items in order and release the pipeline before returning.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2ftry_5fmap"></a>

- `@captures(input, f) pub fn stream.try_map[T, U, E](take input: Flow[T, E], take f: fn(take T) -> Result[U, E]) -> Flow[ U, E ] uses {input, f}`

Transform successful items in order; source and callback errors end the flow.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow)

<a id="def-std_401_2fstream_2fwrite_5fto"></a>

- `pub fn stream.write_to[S, E, W]( take input: Flow[Bytes, E], take destination: S, write: fn(mut S, Bytes) -> Result[u64, W] ) -> Result[S, WriteError[E, W]] uses {input, destination, write}`

Consume each chunk fully before pulling again. Return the owned destination
only on clean EOF, so callers can explicitly commit/finish it.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/stream/Flow`](#def-std_401_2fstream_2fFlow), [`std@1/stream/WriteError`](#def-std_401_2fstream_2fWriteError)


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

<a id="def-std_401_2fstring_2fcontains_5fignore_5fcase"></a>

- `pub fn string.contains_ignore_case(s: String, pattern: String) -> bool uses {}`

Whether `pattern` occurs in the full default case fold of `s`. Like casefold,
this does not normalize text. ASCII inputs need no intermediate strings.

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

<a id="def-std_401_2fstring_2fis_5fascii"></a>

- `@noalloc pub fn string.is_ascii(s: String) -> bool uses {}`

Whether every UTF-8 byte of this slice is ASCII.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

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

<a id="def-std_401_2fstring_2frfind"></a>

- `@noalloc pub fn string.rfind(s: String, pattern: String) -> Option[StrPos of s] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/StrPos`](#def-core_401_2fStrPos), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2frsplit"></a>

- `pub fn string.rsplit(take s: String, separator: String) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2frsplit_5fonce"></a>

- `@noalloc pub fn string.rsplit_once(s: String, separator: String) -> Option[(String, String)] in s uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2frsplitn"></a>

- `pub fn string.rsplitn(take s: String, separator: String, limit: u64) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

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

<a id="def-std_401_2fstring_2fsplit_5fwhitespace"></a>

- `pub fn string.split_whitespace(take s: String) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fstring_2fsplitn"></a>

- `pub fn string.splitn(take s: String, separator: String, limit: u64) -> List[String] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

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


## table

<a id="def-std_401_2ftable_2fRangeBound"></a>

- `pub type table.RangeBound[K] = Unbounded | Included(K) | Excluded(K)`

<a id="def-std_401_2ftable_2fRangeBound_23Excluded"></a>

- `Excluded(K) -> table.RangeBound[K]`

Variant of table.RangeBound.

<a id="def-std_401_2ftable_2fRangeBound_23Included"></a>

- `Included(K) -> table.RangeBound[K]`

Variant of table.RangeBound.

<a id="def-std_401_2ftable_2fRangeBound_23Unbounded"></a>

- `Unbounded -> table.RangeBound[K]`

Variant of table.RangeBound.

<a id="def-std_401_2ftable_2fScanOrder"></a>

- `pub type table.ScanOrder = Ascending | Descending`

<a id="def-std_401_2ftable_2fScanOrder_23Ascending"></a>

- `Ascending -> table.ScanOrder`

Variant of table.ScanOrder.

<a id="def-std_401_2ftable_2fScanOrder_23Descending"></a>

- `Descending -> table.ScanOrder`

Variant of table.ScanOrder.

<a id="def-std_401_2ftable_2fSchemaChange"></a>

- `@serializable pub type table.SchemaChange = Created | Renamed(String, String) | Migrated(String, String)`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftable_2fSchemaChange_23Created"></a>

- `Created -> table.SchemaChange`

Variant of table.SchemaChange.

<a id="def-std_401_2ftable_2fSchemaChange_23Migrated"></a>

- `Migrated(String, String) -> table.SchemaChange`

Variant of table.SchemaChange.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftable_2fSchemaChange_23Renamed"></a>

- `Renamed(String, String) -> table.SchemaChange`

Variant of table.SchemaChange.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftable_2fSchemaEntry"></a>

- `@serializable pub type table.SchemaEntry { epoch: u32, key_schema: String, value_schema: String, change: SchemaChange, }`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u32`](#def-core_401_2fu32), [`std@1/table/SchemaChange`](#def-std_401_2ftable_2fSchemaChange)

<a id="def-std_401_2ftable_2fSchemaError"></a>

- `@serializable pub type table.SchemaError = Renamed(String, String) | KeyMismatch | ValueMismatch`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftable_2fSchemaError_23KeyMismatch"></a>

- `KeyMismatch -> table.SchemaError`

Variant of table.SchemaError.

<a id="def-std_401_2ftable_2fSchemaError_23Renamed"></a>

- `Renamed(String, String) -> table.SchemaError`

Variant of table.SchemaError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftable_2fSchemaError_23ValueMismatch"></a>

- `ValueMismatch -> table.SchemaError`

Variant of table.SchemaError.

<a id="def-std_401_2ftable_2fTable"></a>

- `pub opaque type table.Table[K: key + serializable, V: serializable]: copyable + data + key + inert + sendable uses {}`

An inert name and expected schema; authority comes from each operation's Tx.

<a id="def-std_401_2ftable_2fTxError"></a>

- `@serializable pub type table.TxError = InvalidInput | ReadOnly | NoTable | TableExists | Schema(SchemaError) | Migrating | Limit( store.TxLimit ) | Storage(store.StoreError)`

References: [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError), [`std@1/store/TxLimit`](#def-std_401_2fstore_2fTxLimit), [`std@1/table/SchemaError`](#def-std_401_2ftable_2fSchemaError)

<a id="def-std_401_2ftable_2fTxError_23InvalidInput"></a>

- `InvalidInput -> table.TxError`

Variant of table.TxError.

<a id="def-std_401_2ftable_2fTxError_23Limit"></a>

- `Limit( store.TxLimit ) -> table.TxError`

Variant of table.TxError.

References: [`std@1/store/TxLimit`](#def-std_401_2fstore_2fTxLimit)

<a id="def-std_401_2ftable_2fTxError_23Migrating"></a>

- `Migrating -> table.TxError`

Variant of table.TxError.

<a id="def-std_401_2ftable_2fTxError_23NoTable"></a>

- `NoTable -> table.TxError`

Variant of table.TxError.

<a id="def-std_401_2ftable_2fTxError_23ReadOnly"></a>

- `ReadOnly -> table.TxError`

Variant of table.TxError.

<a id="def-std_401_2ftable_2fTxError_23Schema"></a>

- `Schema(SchemaError) -> table.TxError`

Variant of table.TxError.

References: [`std@1/table/SchemaError`](#def-std_401_2ftable_2fSchemaError)

<a id="def-std_401_2ftable_2fTxError_23Storage"></a>

- `Storage(store.StoreError) -> table.TxError`

Variant of table.TxError.

References: [`std@1/store/StoreError`](#def-std_401_2fstore_2fStoreError)

<a id="def-std_401_2ftable_2fTxError_23TableExists"></a>

- `TableExists -> table.TxError`

Variant of table.TxError.

<a id="def-std_401_2ftable_2fcreate"></a>

- `pub fn table.create[K: key + serializable, V: serializable](mut tx: store.Tx, t: Table[K, V]) -> Result[ (), TxError ]`

Creates an empty table with one schema entry; creation is explicit.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/Table`](#def-std_401_2ftable_2fTable), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2fdelete"></a>

- `pub fn table.delete[K: key + serializable, V: serializable]( mut tx: store.Tx, t: Table[K, V], key: K ) -> Result[bool, TxError]`

Deletes one row and reports whether it existed in this transaction.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/Table`](#def-std_401_2ftable_2fTable), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2fget"></a>

- `pub fn table.get[K: key + serializable, V: serializable](tx: store.Tx, t: Table[K, V], key: K) -> Result[ Option[V], TxError ]`

Returns a fresh value, or None when this transaction sees no row.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/Table`](#def-std_401_2ftable_2fTable), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2fhistory"></a>

- `pub fn table.history(tx: store.Tx, name: String) -> Result[List[SchemaEntry], TxError]`

Returns the immutable schema history; hashes exclude package version text.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/SchemaEntry`](#def-std_401_2ftable_2fSchemaEntry), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2fnamed"></a>

- `pub fn table.named[K: key + serializable, V: serializable](name: String) -> Table[K, V] uses {}`

Names a table without opening or creating it. Operations validate its name.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/table/Table`](#def-std_401_2ftable_2fTable)

<a id="def-std_401_2ftable_2fput"></a>

- `pub fn table.put[K: key + serializable, V: serializable]( mut tx: store.Tx, t: Table[K, V], key: K, value: V ) -> Result[(), TxError]`

Inserts or replaces one row in the transaction's write set.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/Table`](#def-std_401_2ftable_2fTable), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2frange"></a>

- `pub fn table.range[K: key + serializable, V: serializable]( tx: store.Tx, t: Table[K, V], from: RangeBound[K], to: RangeBound[K], limit: u64, order: ScanOrder ) -> Result[List[(K, V)], TxError]`

Scans in key value order with inclusive/exclusive bounds and a positive limit.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/RangeBound`](#def-std_401_2ftable_2fRangeBound), [`std@1/table/ScanOrder`](#def-std_401_2ftable_2fScanOrder), [`std@1/table/Table`](#def-std_401_2ftable_2fTable), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)

<a id="def-std_401_2ftable_2frename"></a>

- `pub fn table.rename(mut tx: store.Tx, name: String, from: String, to: String) -> Result[ (), TxError ]`

Declares a spelling change once, appending metadata without rewriting rows.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/store/Tx`](#def-std_401_2fstore_2fTx), [`std@1/table/TxError`](#def-std_401_2ftable_2fTxError)


## tar

<a id="def-std_401_2ftar_2fExtractError"></a>

- `@serializable pub type tar.ExtractError[E] = Read(ReadError[E]) | Insecure(String) | Unsupported(String) | TooLarge | File( FileError )`

Extraction refused an entry's name (`Insecure`) or kind (`Unsupported`),
would exceed its byte limit, or a file operation failed.

References: [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/String`](#def-core_401_2fString), [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError)

<a id="def-std_401_2ftar_2fExtractError_23File"></a>

- `File( FileError ) -> tar.ExtractError[E]`

Variant of tar.ExtractError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2ftar_2fExtractError_23Insecure"></a>

- `Insecure(String) -> tar.ExtractError[E]`

Variant of tar.ExtractError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftar_2fExtractError_23Read"></a>

- `Read(ReadError[E]) -> tar.ExtractError[E]`

Variant of tar.ExtractError.

References: [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError)

<a id="def-std_401_2ftar_2fExtractError_23TooLarge"></a>

- `TooLarge -> tar.ExtractError[E]`

Variant of tar.ExtractError.

<a id="def-std_401_2ftar_2fExtractError_23Unsupported"></a>

- `Unsupported(String) -> tar.ExtractError[E]`

Variant of tar.ExtractError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftar_2fHeader"></a>

- `@serializable pub type tar.Header { name: String, kind: Kind, linkname: String, size: u64, mode: u64, uid: u64, gid: u64, uname: String, gname: String, modified: i64, devmajor: u64, devminor: u64, records: Map[String, String], }`

One entry's metadata. `records` holds the PAX extended header records that
applied to it, which `name`, `size` and the other fields already reflect.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64), [`std@1/tar/Kind`](#def-std_401_2ftar_2fKind)

<a id="def-std_401_2ftar_2fKind"></a>

- `@serializable pub type tar.Kind = Regular | Link | Symlink | Char | Block | Directory | Fifo | Global | Other( u8 )`

An entry's type flag (POSIX.1-2001 ustar typeflag).

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftar_2fKind_23Block"></a>

- `Block -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Char"></a>

- `Char -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Directory"></a>

- `Directory -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Fifo"></a>

- `Fifo -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Global"></a>

- `Global -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Link"></a>

- `Link -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Other"></a>

- `Other( u8 ) -> tar.Kind`

Variant of tar.Kind.

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftar_2fKind_23Regular"></a>

- `Regular -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fKind_23Symlink"></a>

- `Symlink -> tar.Kind`

Variant of tar.Kind.

<a id="def-std_401_2ftar_2fReadError"></a>

- `@serializable pub type tar.ReadError[E] = Input(bufio.ReadError[E]) | Data(flate.FlateError) | Archive( TarError )`

A read from the buffered source failed, a compressed archive does not
decode, or the archive is malformed.

References: [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError), [`std@1/tar/TarError`](#def-std_401_2ftar_2fTarError)

<a id="def-std_401_2ftar_2fReadError_23Archive"></a>

- `Archive( TarError ) -> tar.ReadError[E]`

Variant of tar.ReadError.

References: [`std@1/tar/TarError`](#def-std_401_2ftar_2fTarError)

<a id="def-std_401_2ftar_2fReadError_23Data"></a>

- `Data(flate.FlateError) -> tar.ReadError[E]`

Variant of tar.ReadError.

References: [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2ftar_2fReadError_23Input"></a>

- `Input(bufio.ReadError[E]) -> tar.ReadError[E]`

Variant of tar.ReadError.

References: [`std@1/bufio/ReadError`](#def-std_401_2fbufio_2fReadError)

<a id="def-std_401_2ftar_2fReader"></a>

- `pub opaque type tar.Reader: copyable + inert + noalloc_drop uses {}`

Reading state: the decompressor (if any), the archive offset and the
current entry's unread data and padding.

<a id="def-std_401_2ftar_2fTarError"></a>

- `@serializable pub type tar.TarError = Header(u64) | Encoding(u64) | Unsupported(u64) | Truncated`

Malformed archive data. `Header(n)` names the byte offset of the header
block that is invalid; `Encoding(n)` one whose name is not UTF-8;
`Unsupported(n)` a GNU sparse entry.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftar_2fTarError_23Encoding"></a>

- `Encoding(u64) -> tar.TarError`

Variant of tar.TarError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftar_2fTarError_23Header"></a>

- `Header(u64) -> tar.TarError`

Variant of tar.TarError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftar_2fTarError_23Truncated"></a>

- `Truncated -> tar.TarError`

Variant of tar.TarError.

<a id="def-std_401_2ftar_2fTarError_23Unsupported"></a>

- `Unsupported(u64) -> tar.TarError`

Variant of tar.TarError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftar_2fWriteError"></a>

- `@serializable pub type tar.WriteError = Field | TooLong | Short | Closed`

A header that the writer cannot encode, data beyond or short of the
declared size, or a write after `close`.

<a id="def-std_401_2ftar_2fWriteError_23Closed"></a>

- `Closed -> tar.WriteError`

Variant of tar.WriteError.

<a id="def-std_401_2ftar_2fWriteError_23Field"></a>

- `Field -> tar.WriteError`

Variant of tar.WriteError.

<a id="def-std_401_2ftar_2fWriteError_23Short"></a>

- `Short -> tar.WriteError`

Variant of tar.WriteError.

<a id="def-std_401_2ftar_2fWriteError_23TooLong"></a>

- `TooLong -> tar.WriteError`

Variant of tar.WriteError.

<a id="def-std_401_2ftar_2fWriter"></a>

- `pub opaque type tar.Writer: copyable + inert + noalloc_drop uses {}`

Writing state: the current entry's unwritten data and padding.

<a id="def-std_401_2ftar_2fclose"></a>

- `@preserves_unique(out) pub fn tar.close(mut w: Writer, mut out: List[u8]) -> Result[(), WriteError] uses {}`

Go Writer.Close: finishes the last entry and appends the two zero blocks
that end an archive; later writes are `Closed`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/tar/WriteError`](#def-std_401_2ftar_2fWriteError), [`std@1/tar/Writer`](#def-std_401_2ftar_2fWriter)

<a id="def-std_401_2ftar_2fextract"></a>

- `pub fn tar.extract[S, E]( into: Dir, mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], limit: u64 ) -> Result[u64, ExtractError[E]]`

Extracts every entry from the reader into `into` and returns how many
were written: directories and regular files only, creating missing parent
directories. A name that is empty, absolute or holds a `..` component, a
NUL or a backslash is `Insecure`, as is a path through or onto an existing
symlink: extraction never follows one. Links, devices, FIFOs and other
kinds are `Unsupported`; PAX global headers are skipped. An entry whose
data would take the total past `limit` bytes is `TooLarge` before any of
it is written. Permissions and times are not applied. The first refusal
stops extraction, leaving earlier entries in place.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/tar/ExtractError`](#def-std_401_2ftar_2fExtractError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2ffile_5fextract"></a>

- `pub fn tar.file_extract( from: Dir, mut f: File of from, into: Dir, mut t: Reader, limit: u64 ) -> Result[u64, ExtractError[FileError]]`

`extract` from an archive file, through a 64 KiB buffer.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/tar/ExtractError`](#def-std_401_2ftar_2fExtractError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2ffile_5fnext"></a>

- `pub fn tar.file_next(dir: Dir, mut f: File of dir, mut t: Reader, mut r: bufio.Reader) -> Result[ Option[Header], ReadError[FileError] ]`

`next` from a file through the buffered reader `r`.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/tar/Header`](#def-std_401_2ftar_2fHeader), [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2ffile_5fread"></a>

- `@preserves_unique(out) pub fn tar.file_read( dir: Dir, mut f: File of dir, mut t: Reader, mut r: bufio.Reader, mut out: List[u8], max: u64 ) -> Result[u64, ReadError[FileError]]`

`read` from a file through the buffered reader `r`.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2fheader"></a>

- `pub fn tar.header(take name: String, kind: Kind, size: u64) -> Header uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/tar/Header`](#def-std_401_2ftar_2fHeader), [`std@1/tar/Kind`](#def-std_401_2ftar_2fKind)

<a id="def-std_401_2ftar_2fnext"></a>

- `pub fn tar.next[S, E]( mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E] ) -> Result[Option[Header], ReadError[E]] uses {refill}`

Go Reader.Next: skips the rest of the current entry and returns the next
entry's header, or None after the end-of-archive marker or at the end of
the input after an entry's data (including inside its padding). PAX `x`
records and GNU long names apply to the entry that follows them; a PAX global header is returned as a `Global`
entry. Entry names are returned as stored: `extract` refuses unsafe ones.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/tar/Header`](#def-std_401_2ftar_2fHeader), [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2fread"></a>

- `@preserves_unique(out) pub fn tar.read[S, E]( mut t: Reader, mut r: bufio.Reader, mut source: S, refill: fn(mut S, mut List[u8], u64) -> Result[u64, E], mut out: List[u8], max: u64 ) -> Result[u64, ReadError[E]] uses {refill}`

Appends at most `max` bytes of the current entry's data to `out` and
returns how many, zero at the end of the entry.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/bufio/Reader`](#def-std_401_2fbufio_2fReader), [`std@1/tar/ReadError`](#def-std_401_2ftar_2fReadError), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2freader"></a>

- `pub fn tar.reader(compression: Option[flate.Format]) -> unique Reader uses {}`

A reader of an uncompressed archive, or of one compressed as `format`.

References: [`core@1/Option`](#def-core_401_2fOption), [`std@1/flate/Format`](#def-std_401_2fflate_2fFormat), [`std@1/tar/Reader`](#def-std_401_2ftar_2fReader)

<a id="def-std_401_2ftar_2fwrite"></a>

- `@preserves_unique(out) pub fn tar.write(mut w: Writer, data: Bytes, mut out: List[u8]) -> Result[(), WriteError] uses {}`

Appends `data` to the current entry. Data beyond the declared size is
`TooLong`, and nothing is appended.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/tar/WriteError`](#def-std_401_2ftar_2fWriteError), [`std@1/tar/Writer`](#def-std_401_2ftar_2fWriter)

<a id="def-std_401_2ftar_2fwrite_5fheader"></a>

- `@preserves_unique(out) pub fn tar.write_header(mut w: Writer, h: Header, mut out: List[u8]) -> Result[ (), WriteError ] uses {}`

Go Writer.WriteHeader: finishes the previous entry and appends `h`'s
header blocks, USTAR when its fields fit (a long name split into prefix
and name), else a PAX extended header first; the entry's `size` bytes of
data follow through `write`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/tar/Header`](#def-std_401_2ftar_2fHeader), [`std@1/tar/WriteError`](#def-std_401_2ftar_2fWriteError), [`std@1/tar/Writer`](#def-std_401_2ftar_2fWriter)

<a id="def-std_401_2ftar_2fwriter"></a>

- `pub fn tar.writer() -> unique Writer uses {}`

References: [`std@1/tar/Writer`](#def-std_401_2ftar_2fWriter)


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

<a id="def-std_401_2ftask_2fmap_5fall"></a>

- `pub fn task.map_all[T: copyable + inert, U: copyable + inert, E: sendable]( tasks: Tasks, take items: List[T], grain: u64, take f: fn(T) -> Result[U, E] uses {} ) -> Result[List[U], E]`

Map in a nested scope, one child per `grain` items, and return input order.
`f` holds no authority. The first completed error cancels and joins the
remaining children through `task.all`. Zero grain fails SEM-1.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Tasks`](#def-core_401_2fTasks), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-core_401_2ftask_2fscope"></a>

- `fn task.scope[R](tasks: Tasks, body: once fn(s: Scope) -> R) -> R`

<a id="def-core_401_2ftask_2fscope_5funtil"></a>

- `fn task.scope_until[R](tasks: Tasks, deadline: Deadline, body: once fn(s: Scope) -> R) -> Bounded[R]`

<a id="def-std_401_2ftask_2fselect"></a>

- `pub fn task.select[R](s: Scope, take set: TaskSet[R] of s) -> (u64, R)`

References: [`core@1/Scope`](#def-core_401_2fScope), [`core@1/TaskSet`](#def-core_401_2fTaskSet), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-core_401_2ftask_2fset"></a>

- `fn task.set[R](s: Scope) -> TaskSet[R] of s`


## telemetry

<a id="def-std_401_2ftelemetry_2fAttribute"></a>

- `@serializable pub type telemetry.Attribute { name: String, value: Value, }`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/Value`](#def-std_401_2ftelemetry_2fValue)

<a id="def-std_401_2ftelemetry_2fCheckedEvent"></a>

- `pub opaque type telemetry.CheckedEvent: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2ftelemetry_2fCheckedLog"></a>

- `pub opaque type telemetry.CheckedLog: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2ftelemetry_2fContextError"></a>

- `@serializable pub type telemetry.ContextError = MissingParent | DuplicateHeader | InvalidParent | InvalidState | ContextLimit`

<a id="def-std_401_2ftelemetry_2fContextError_23ContextLimit"></a>

- `ContextLimit -> telemetry.ContextError`

Variant of telemetry.ContextError.

<a id="def-std_401_2ftelemetry_2fContextError_23DuplicateHeader"></a>

- `DuplicateHeader -> telemetry.ContextError`

Variant of telemetry.ContextError.

<a id="def-std_401_2ftelemetry_2fContextError_23InvalidParent"></a>

- `InvalidParent -> telemetry.ContextError`

Variant of telemetry.ContextError.

<a id="def-std_401_2ftelemetry_2fContextError_23InvalidState"></a>

- `InvalidState -> telemetry.ContextError`

Variant of telemetry.ContextError.

<a id="def-std_401_2ftelemetry_2fContextError_23MissingParent"></a>

- `MissingParent -> telemetry.ContextError`

Variant of telemetry.ContextError.

<a id="def-std_401_2ftelemetry_2fEventData"></a>

- `@serializable pub type telemetry.EventData { name: String, fields: List[Attribute], }`

A span event's plain description.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute)

<a id="def-std_401_2ftelemetry_2fEventError"></a>

- `@serializable pub type telemetry.EventError = DuplicateKey | KeyLimit | ValueLimit | AttributeLimit | EventLimit | InvalidContext`

<a id="def-std_401_2ftelemetry_2fEventError_23AttributeLimit"></a>

- `AttributeLimit -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fEventError_23DuplicateKey"></a>

- `DuplicateKey -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fEventError_23EventLimit"></a>

- `EventLimit -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fEventError_23InvalidContext"></a>

- `InvalidContext -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fEventError_23KeyLimit"></a>

- `KeyLimit -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fEventError_23ValueLimit"></a>

- `ValueLimit -> telemetry.EventError`

Variant of telemetry.EventError.

<a id="def-std_401_2ftelemetry_2fLogRecord"></a>

- `pub type telemetry.LogRecord { level: log.Level, name: String, message: String, metadata: Metadata, }`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/log/Level`](#def-std_401_2flog_2fLevel), [`std@1/telemetry/Metadata`](#def-std_401_2ftelemetry_2fMetadata)

<a id="def-std_401_2ftelemetry_2fMetadata"></a>

- `@serializable pub type telemetry.Metadata { trace: Option[TraceContext], fields: List[Attribute], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2fSpanData"></a>

- `@serializable pub type telemetry.SpanData { name: String, kind: SpanKind, fields: List[Attribute], links: List[TraceContext], sampled: bool, }`

A span's plain description. `sampled` is a root's sampling decision; a child keeps its
explicit parent's.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/SpanKind`](#def-std_401_2ftelemetry_2fSpanKind), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2fSpanKind"></a>

- `@serializable pub type telemetry.SpanKind = Internal | Server | Client | Producer | Consumer`

<a id="def-std_401_2ftelemetry_2fSpanKind_23Client"></a>

- `Client -> telemetry.SpanKind`

Variant of telemetry.SpanKind.

<a id="def-std_401_2ftelemetry_2fSpanKind_23Consumer"></a>

- `Consumer -> telemetry.SpanKind`

Variant of telemetry.SpanKind.

<a id="def-std_401_2ftelemetry_2fSpanKind_23Internal"></a>

- `Internal -> telemetry.SpanKind`

Variant of telemetry.SpanKind.

<a id="def-std_401_2ftelemetry_2fSpanKind_23Producer"></a>

- `Producer -> telemetry.SpanKind`

Variant of telemetry.SpanKind.

<a id="def-std_401_2ftelemetry_2fSpanKind_23Server"></a>

- `Server -> telemetry.SpanKind`

Variant of telemetry.SpanKind.

<a id="def-std_401_2ftelemetry_2fSpanSpec"></a>

- `pub opaque type telemetry.SpanSpec: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2ftelemetry_2fStatus"></a>

- `@serializable pub type telemetry.Status = Unset | Success | Error(String) | Canceled(CancelCause)`

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftelemetry_2fStatus_23Canceled"></a>

- `Canceled(CancelCause) -> telemetry.Status`

Variant of telemetry.Status.

References: [`core@1/CancelCause`](#def-core_401_2fCancelCause)

<a id="def-std_401_2ftelemetry_2fStatus_23Error"></a>

- `Error(String) -> telemetry.Status`

Variant of telemetry.Status.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftelemetry_2fStatus_23Success"></a>

- `Success -> telemetry.Status`

Variant of telemetry.Status.

<a id="def-std_401_2ftelemetry_2fStatus_23Unset"></a>

- `Unset -> telemetry.Status`

Variant of telemetry.Status.

<a id="def-std_401_2ftelemetry_2fTraceContext"></a>

- `@serializable pub type telemetry.TraceContext { trace_id: Inline[u8, 16], span_id: Inline[u8, 8], sampled: bool, remote: bool, tracestate: List[(String, String)], }`

References: [`core@1/Inline`](#def-core_401_2fInline), [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftelemetry_2fValue"></a>

- `@serializable pub type telemetry.Value = Bool(bool) | Signed(i64) | Unsigned(u64) | Text(String) | Bytes(Bytes)`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftelemetry_2fValue_23Bool"></a>

- `Bool(bool) -> telemetry.Value`

Variant of telemetry.Value.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2ftelemetry_2fValue_23Bytes"></a>

- `Bytes(Bytes) -> telemetry.Value`

Variant of telemetry.Value.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2ftelemetry_2fValue_23Signed"></a>

- `Signed(i64) -> telemetry.Value`

Variant of telemetry.Value.

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2ftelemetry_2fValue_23Text"></a>

- `Text(String) -> telemetry.Value`

Variant of telemetry.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftelemetry_2fValue_23Unsigned"></a>

- `Unsigned(u64) -> telemetry.Value`

Variant of telemetry.Value.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftelemetry_2fallow_5ffields"></a>

- `pub fn telemetry.allow_fields(fields: List[Attribute], allow: List[String]) -> List[Attribute] uses {}`

Retains only explicitly allowed attributes, before checking/capture. Trace
propagation and free-form name/message redaction remain explicit caller policy.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute)

<a id="def-std_401_2ftelemetry_2fbegin"></a>

- `pub fn telemetry.begin(t: Telemetry, parent: Option[TraceContext], spec: SpanSpec) -> Result[ Span of t, ContextError ]`

Begins a span under an explicit parent (None for a new root, with the spec's sampling
decision). An invalid raw parent is refused before any effect. The span's identities
come from the Telemetry provider: fresh for a root, the parent's trace for a child.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Span`](#def-core_401_2fSpan), [`core@1/Telemetry`](#def-core_401_2fTelemetry), [`std@1/telemetry/ContextError`](#def-std_401_2ftelemetry_2fContextError), [`std@1/telemetry/SpanSpec`](#def-std_401_2ftelemetry_2fSpanSpec), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2fcheck_5fevent"></a>

- `pub fn telemetry.check_event(name: String, fields: List[Attribute]) -> Result[ CheckedEvent, EventError ] uses {}`

Checks the name, fields (as `check_metadata`) and 16384 encoded bytes.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/CheckedEvent`](#def-std_401_2ftelemetry_2fCheckedEvent), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError)

<a id="def-std_401_2ftelemetry_2fcheck_5flog"></a>

- `pub fn telemetry.check_log(record: LogRecord) -> Result[CheckedLog, EventError] uses {}`

Checks all fixed limits, including 16 KiB for the complete JSON line.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/telemetry/CheckedLog`](#def-std_401_2ftelemetry_2fCheckedLog), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError), [`std@1/telemetry/LogRecord`](#def-std_401_2ftelemetry_2fLogRecord)

<a id="def-std_401_2ftelemetry_2fcheck_5fmetadata"></a>

- `pub fn telemetry.check_metadata(metadata: Metadata) -> Result[Metadata, EventError] uses {}`

Rejects duplicate keys, validates fixed limits, then sorts by UTF-8 bytes.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError), [`std@1/telemetry/Metadata`](#def-std_401_2ftelemetry_2fMetadata)

<a id="def-std_401_2ftelemetry_2fcontext"></a>

- `pub fn telemetry.context(t: Telemetry, span: Span of t) -> TraceContext`

The span's identities and its parent's sampling and tracestate, as data: it stays
usable after the span ends, grants nothing and emits nothing. Its `remote` is false.

References: [`core@1/Span`](#def-core_401_2fSpan), [`core@1/Telemetry`](#def-core_401_2fTelemetry), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2femit_5flog"></a>

- `pub fn telemetry.emit_log(t: Telemetry, record: CheckedLog) -> ()`

Captures a checked log record: its validated v1 JSON line (`render`).

References: [`core@1/Telemetry`](#def-core_401_2fTelemetry), [`std@1/telemetry/CheckedLog`](#def-std_401_2ftelemetry_2fCheckedLog)

<a id="def-std_401_2ftelemetry_2fenrich"></a>

- `pub fn telemetry.enrich(base: Metadata, fields: List[Attribute]) -> Result[Metadata, EventError] uses {}`

Explicit enrichment replaces base keys; duplicates within either set fail.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError), [`std@1/telemetry/Metadata`](#def-std_401_2ftelemetry_2fMetadata)

<a id="def-std_401_2ftelemetry_2fevent_5fdata"></a>

- `pub fn telemetry.event_data(event: CheckedEvent) -> EventData uses {}`

Explicit plain-data projection; CheckedEvent itself is opaque.

References: [`std@1/telemetry/CheckedEvent`](#def-std_401_2ftelemetry_2fCheckedEvent), [`std@1/telemetry/EventData`](#def-std_401_2ftelemetry_2fEventData)

<a id="def-std_401_2ftelemetry_2fextract_5ftrace"></a>

- `pub fn telemetry.extract_trace(headers: List[(String, String)]) -> Result[ TraceContext, ContextError ] uses {}`

Extracts exactly one v00 traceparent and at most one tracestate header.
Header names are ASCII case insensitive; baggage is never forwarded.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/ContextError`](#def-std_401_2ftelemetry_2fContextError), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2ffinish"></a>

- `pub fn telemetry.finish(t: Telemetry, take span: Span of t, status: Status) -> Result[ (), EventError ]`

Ends the span with an explicit status; its context remains usable. A status whose
Error message exceeds 4096 bytes is refused and the span is abandoned, as its drop
would. No outcome is inferred from a drop.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/Span`](#def-core_401_2fSpan), [`core@1/Telemetry`](#def-core_401_2fTelemetry), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError), [`std@1/telemetry/Status`](#def-std_401_2ftelemetry_2fStatus)

<a id="def-std_401_2ftelemetry_2finject_5ftrace"></a>

- `pub fn telemetry.inject_trace(ctx: TraceContext) -> Result[List[(String, String)], ContextError] uses {}`

Validates raw context, emits only v00 sampling bit and explicit tracestate.
Remote is local metadata and is not a wire flag.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/ContextError`](#def-std_401_2ftelemetry_2fContextError), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2flog_5fdata"></a>

- `pub fn telemetry.log_data(record: CheckedLog) -> LogRecord uses {}`

Explicit plain-data projection; CheckedLog itself is intentionally opaque.

References: [`std@1/telemetry/CheckedLog`](#def-std_401_2ftelemetry_2fCheckedLog), [`std@1/telemetry/LogRecord`](#def-std_401_2ftelemetry_2fLogRecord)

<a id="def-std_401_2ftelemetry_2frender"></a>

- `pub fn telemetry.render(record: CheckedLog) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/telemetry/CheckedLog`](#def-std_401_2ftelemetry_2fCheckedLog)

<a id="def-std_401_2ftelemetry_2fspan_5fdata"></a>

- `pub fn telemetry.span_data(spec: SpanSpec) -> SpanData uses {}`

Explicit plain-data projection; SpanSpec itself is opaque.

References: [`std@1/telemetry/SpanData`](#def-std_401_2ftelemetry_2fSpanData), [`std@1/telemetry/SpanSpec`](#def-std_401_2ftelemetry_2fSpanSpec)

<a id="def-std_401_2ftelemetry_2fspan_5fevent"></a>

- `pub fn telemetry.span_event(t: Telemetry, span: Span of t, event: CheckedEvent) -> ()`

Captures a checked event on the span. Accepted whatever the exporter does.

References: [`core@1/Span`](#def-core_401_2fSpan), [`core@1/Telemetry`](#def-core_401_2fTelemetry), [`std@1/telemetry/CheckedEvent`](#def-std_401_2ftelemetry_2fCheckedEvent)

<a id="def-std_401_2ftelemetry_2fspan_5fspec"></a>

- `pub fn telemetry.span_spec( name: String, kind: SpanKind, fields: List[Attribute], links: List[TraceContext], sampled: bool ) -> Result[SpanSpec, EventError] uses {}`

Checks the name, fields (as `check_metadata`), at most 32 valid links and 16384
encoded bytes.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/telemetry/Attribute`](#def-std_401_2ftelemetry_2fAttribute), [`std@1/telemetry/EventError`](#def-std_401_2ftelemetry_2fEventError), [`std@1/telemetry/SpanKind`](#def-std_401_2ftelemetry_2fSpanKind), [`std@1/telemetry/SpanSpec`](#def-std_401_2ftelemetry_2fSpanSpec), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2fvalidate_5ftrace"></a>

- `pub fn telemetry.validate_trace(ctx: TraceContext) -> Result[(), ContextError] uses {}`

Validates raw program-created IDs and ordered tracestate. No authority.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/telemetry/ContextError`](#def-std_401_2ftelemetry_2fContextError), [`std@1/telemetry/TraceContext`](#def-std_401_2ftelemetry_2fTraceContext)

<a id="def-std_401_2ftelemetry_2fwrite_5fstderr"></a>

- `pub fn telemetry.write_stderr(sink: Stderr, minimum: log.Level, record: CheckedLog) -> Result[ (), OutputError ]`

Existing fallible stderr effect, including its cancellation/scheduling rules.

References: [`core@1/OutputError`](#def-core_401_2fOutputError), [`core@1/Result`](#def-core_401_2fResult), [`core@1/Stderr`](#def-core_401_2fStderr), [`std@1/log/Level`](#def-std_401_2flog_2fLevel), [`std@1/telemetry/CheckedLog`](#def-std_401_2ftelemetry_2fCheckedLog)


## test

<a id="def-std_401_2ftest_2fSource"></a>

- `pub opaque type test.Source: inert + noalloc_drop uses {}`

<a id="def-std_401_2ftest_2fany_5fu64"></a>

- `pub fn test.any_u64(mut s: Source) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-std_401_2ftest_2fapprox"></a>

- `@noalloc @track_caller pub fn test.approx(t: Test, actual: f64, expected: f64, absolute: f64, relative: f64) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest), [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ftest_2fascii_5fstring"></a>

- `pub fn test.ascii_string(mut s: Source, max_len: u64) -> String`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-std_401_2ftest_2fassert_5feq"></a>

- `@track_caller pub fn test.assert_eq[T: data](t: Test, actual: T, expected: T, show: fn(T) -> String) -> ()`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/Test`](#def-core_401_2fTest)

<a id="def-std_401_2ftest_2fbool"></a>

- `pub fn test.bool(mut s: Source) -> bool uses {}`

References: [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-core_401_2ftest_2fcheck"></a>

- `@noalloc fn test.check(t: Test, condition: bool) -> ()`

<a id="def-core_401_2ftest_2fcheck_5fcase"></a>

- `@noalloc fn test.check_case(t: Test, ok: bool, case: u64) -> ()`

<a id="def-std_401_2ftest_2fchoice"></a>

- `pub fn test.choice(mut s: Source, max: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-core_401_2ftest_2fclock"></a>

- `@noalloc fn test.clock(t: Test) -> MonotonicClock`

<a id="def-core_401_2ftest_2fcrash"></a>

- `fn test.crash(t: Test, d: Dir, seed: u64) -> ()`

<a id="def-core_401_2ftest_2fdir"></a>

- `fn test.dir(t: Test, entries: List[(Path, Bytes)]) -> Dir`

<a id="def-core_401_2ftest_2fdir_5fevent"></a>

- `fn test.dir_event(t: Test, d: Dir, event: WatchEvent) -> ()`

<a id="def-std_401_2ftest_2fequal"></a>

- `@noalloc @track_caller pub fn test.equal[T: data](t: Test, actual: T, expected: T) -> ()`

References: [`core@1/Test`](#def-core_401_2fTest)

<a id="def-std_401_2ftest_2fexpect_5ferror"></a>

- `@track_caller pub fn test.expect_error[T, E](t: Test, take value: Result[T, E]) -> E`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/Test`](#def-core_401_2fTest)

<a id="def-std_401_2ftest_2fexpect_5fok"></a>

- `@track_caller pub fn test.expect_ok[T, E](t: Test, take value: Result[T, E]) -> T`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/Test`](#def-core_401_2fTest)

<a id="def-core_401_2ftest_2fexpect_5fsnapshot"></a>

- `@noalloc fn test.expect_snapshot(t: Test, name: String, text: String) -> ()`

<a id="def-core_401_2ftest_2ffail"></a>

- `@noalloc fn test.fail(t: Test) -> Never`

<a id="def-std_401_2ftest_2flist_5fof"></a>

- `pub fn test.list_of[T](mut s: Source, max_len: u64, element: fn(mut Source) -> T) -> List[T]`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-core_401_2ftest_2flog"></a>

- `@noalloc fn test.log(t: Test, text: String) -> ()`

<a id="def-core_401_2ftest_2fnet"></a>

- `fn test.net(t: Test, peers: List[NetPeer], listeners: List[NetListener]) -> Net`

<a id="def-core_401_2ftest_2fprocess"></a>

- `fn test.process(t: Test, runs: List[ProcessScript], children: List[ChildScript]) -> Proc`

<a id="def-std_401_2ftest_2fproperty"></a>

- `@track_caller pub fn test.property[T]( t: Test, gen: fn(mut Source) -> T, law: fn(T) -> bool uses {}, show: fn(T) -> String ) -> ()`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/Test`](#def-core_401_2fTest), [`std@1/test/Source`](#def-std_401_2ftest_2fSource), [`std@1/test/bool`](#def-std_401_2ftest_2fbool)

<a id="def-std_401_2ftest_2fproperty_5fcases"></a>

- `@track_caller pub fn test.property_cases[T]( t: Test, cases: u64, gen: fn(mut Source) -> T, law: fn(T) -> bool uses {}, show: fn(T) -> String ) -> ()`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/Test`](#def-core_401_2fTest), [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource), [`std@1/test/bool`](#def-std_401_2ftest_2fbool)

<a id="def-std_401_2ftest_2fquoted"></a>

- `pub fn test.quoted(value: String) -> String uses {}`

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftest_2frange_5fi64"></a>

- `pub fn test.range_i64(mut s: Source, low: i64, high: i64) -> i64 uses {}`

References: [`core@1/i64`](#def-core_401_2fi64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-std_401_2ftest_2frange_5fu64"></a>

- `pub fn test.range_u64(mut s: Source, low: u64, high: u64) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/test/Source`](#def-std_401_2ftest_2fSource)

<a id="def-core_401_2ftest_2frunner"></a>

- `fn test.runner(t: Test) -> Runner`

<a id="def-core_401_2ftest_2fseed"></a>

- `@noalloc fn test.seed(t: Test) -> u64`

<a id="def-std_401_2ftest_2fshow_5flist"></a>

- `pub fn test.show_list[T](xs: List[T], show: fn(T) -> String) -> String`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-core_401_2ftest_2ftasks"></a>

- `@noalloc fn test.tasks(t: Test) -> Tasks`


## text

<a id="def-std_401_2ftext_2fappend"></a>

- `@preserves_unique(builder) pub fn text.append(mut builder: List[u8], text: String) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftext_2fdisplay_5fwidth"></a>

- `@noalloc pub fn text.display_width(s: String) -> u64 uses {}`

Estimated display cells over Unicode16 extended grapheme clusters.
Each cluster's first scalar counts 2 for EAW W/F or one of the three C++23 wide blocks,
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


## text_similarity

<a id="def-std_401_2ftext_5fsimilarity_2fError"></a>

- `pub type text_similarity.Error = TooLarge()`

A scalar cap, matching-work budget or unrepresentable scratch size.

<a id="def-std_401_2ftext_5fsimilarity_2fError_23TooLarge"></a>

- `TooLarge() -> text_similarity.Error`

Variant of text_similarity.Error.

<a id="def-std_401_2ftext_5fsimilarity_2fjaro"></a>

- `pub fn text_similarity.jaro(a: String, b: String, max_cells: u64, max_scalars: u64) -> Result[ f64, Error ] uses {}`

Jaro similarity over Unicode scalars, without normalization or grapheme folding.
max_scalars bounds EACH input; max_cells bounds n*m matching work.
TooLarge is checked before scratch allocation. Empty inputs still obey the
scalar cap and require zero cells. Time O(n+m+n*m); scratch O(n+m).

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`std@1/text_similarity/Error`](#def-std_401_2ftext_5fsimilarity_2fError)

<a id="def-std_401_2ftext_5fsimilarity_2fjaro_5fwinkler"></a>

- `pub fn text_similarity.jaro_winkler(a: String, b: String, max_cells: u64, max_scalars: u64) -> Result[ f64, Error ] uses {}`

Jaro with a 0.1 boost per common prefix scalar (at most four), only above 0.7.
Uses the same bounds and refusal rules as jaro; both inputs remain unchanged.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/f64`](#def-core_401_2ff64), [`core@1/u64`](#def-core_401_2fu64), [`std@1/text_similarity/Error`](#def-std_401_2ftext_5fsimilarity_2fError)


## time

<a id="def-std_401_2ftime_2fBrokenDownTime"></a>

- `@serializable pub type time.BrokenDownTime { datetime: DateTime, offset: Option[i32], }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2ftime_2fCalendarError"></a>

- `@serializable pub type time.CalendarError = OutOfRange | NoSuchDay`

<a id="def-std_401_2ftime_2fCalendarError_23NoSuchDay"></a>

- `NoSuchDay -> time.CalendarError`

Variant of time.CalendarError.

<a id="def-std_401_2ftime_2fCalendarError_23OutOfRange"></a>

- `OutOfRange -> time.CalendarError`

Variant of time.CalendarError.

<a id="def-std_401_2ftime_2fDate"></a>

- `@format(date.format_into) @serializable(check = decoded_date) pub opaque type time.Date: plain + key + sendable + inert + noalloc_drop uses {}`

References: [`std@1/date/format_into`](#def-std_401_2fdate_2fformat_5finto)

<a id="def-std_401_2ftime_2fDateTime"></a>

- `@format(date_time.format_into) @serializable pub type time.DateTime { date: Date, time: TimeOfDay, }`

References: [`std@1/date_time/format_into`](#def-std_401_2fdate_5ftime_2fformat_5finto), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_2fDisambiguation"></a>

- `@serializable pub type time.Disambiguation = Earlier | Later | Reject`

<a id="def-std_401_2ftime_2fDisambiguation_23Earlier"></a>

- `Earlier -> time.Disambiguation`

Variant of time.Disambiguation.

<a id="def-std_401_2ftime_2fDisambiguation_23Later"></a>

- `Later -> time.Disambiguation`

Variant of time.Disambiguation.

<a id="def-std_401_2ftime_2fDisambiguation_23Reject"></a>

- `Reject -> time.Disambiguation`

Variant of time.Disambiguation.

<a id="def-std_401_2ftime_2fDuration"></a>

- `@serializable pub type time.Duration { nanoseconds: i128, }`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2ftime_2fFormatError"></a>

- `@serializable pub type time.FormatError = Invalid(u64) | NoOffset | OutOfRange`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftime_2fFormatError_23Invalid"></a>

- `Invalid(u64) -> time.FormatError`

Variant of time.FormatError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftime_2fFormatError_23NoOffset"></a>

- `NoOffset -> time.FormatError`

Variant of time.FormatError.

<a id="def-std_401_2ftime_2fFormatError_23OutOfRange"></a>

- `OutOfRange -> time.FormatError`

Variant of time.FormatError.

<a id="def-std_401_2ftime_2fIsoWeek"></a>

- `@serializable pub type time.IsoWeek { year: i32, week: u8, }`

References: [`core@1/i32`](#def-core_401_2fi32), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2ftime_2fOverflow"></a>

- `@serializable pub type time.Overflow = Clamp | Reject`

<a id="def-std_401_2ftime_2fOverflow_23Clamp"></a>

- `Clamp -> time.Overflow`

Variant of time.Overflow.

<a id="def-std_401_2ftime_2fOverflow_23Reject"></a>

- `Reject -> time.Overflow`

Variant of time.Overflow.

<a id="def-std_401_2ftime_2fSleepError"></a>

- `@serializable pub type time.SleepError = NegativeDuration | OutOfRange | Wait(WaitError)`

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

- `@serializable pub type time.TimeError = OutOfRange()`

<a id="def-std_401_2ftime_2fTimeError_23OutOfRange"></a>

- `OutOfRange() -> time.TimeError`

Variant of time.TimeError.

<a id="def-std_401_2ftime_2fTimeOfDay"></a>

- `@format(time_of_day.format_into) @serializable(check = decoded_time) pub opaque type time.TimeOfDay: plain + key + sendable + inert + noalloc_drop uses {}`

References: [`std@1/time_of_day/format_into`](#def-std_401_2ftime_5fof_5fday_2fformat_5finto)

<a id="def-std_401_2ftime_2fTimeZone"></a>

- `pub opaque type time.TimeZone: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2ftime_2fTimestamp"></a>

- `@serializable pub type time.Timestamp { unix_nanoseconds: i128, }`

References: [`core@1/i128`](#def-core_401_2fi128)

<a id="def-std_401_2ftime_2fWeekday"></a>

- `@serializable pub type time.Weekday = Monday | Tuesday | Wednesday | Thursday | Friday | Saturday | Sunday`

<a id="def-std_401_2ftime_2fWeekday_23Friday"></a>

- `Friday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Monday"></a>

- `Monday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Saturday"></a>

- `Saturday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Sunday"></a>

- `Sunday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Thursday"></a>

- `Thursday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Tuesday"></a>

- `Tuesday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fWeekday_23Wednesday"></a>

- `Wednesday -> time.Weekday`

Variant of time.Weekday.

<a id="def-std_401_2ftime_2fZoneError"></a>

- `@serializable pub type time.ZoneError = Gap | Fold`

<a id="def-std_401_2ftime_2fZoneError_23Fold"></a>

- `Fold -> time.ZoneError`

Variant of time.ZoneError.

<a id="def-std_401_2ftime_2fZoneError_23Gap"></a>

- `Gap -> time.ZoneError`

Variant of time.ZoneError.

<a id="def-std_401_2ftime_2fbetween"></a>

- `@noalloc pub fn time.between(clock: MonotonicClock, start: Instant of clock, end: Instant of clock) -> Duration`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2ftime_2fbroken_5fdown"></a>

- `@noalloc pub fn time.broken_down(value: Timestamp, offset: i32) -> Result[BrokenDownTime, TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/BrokenDownTime`](#def-std_401_2ftime_2fBrokenDownTime), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_2fcompare"></a>

- `@noalloc pub fn time.compare(clock: MonotonicClock, a: Instant of clock, b: Instant of clock) -> i32`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/i32`](#def-core_401_2fi32)

<a id="def-std_401_2ftime_2fdate_5ffrom_5fjson"></a>

- `pub fn time.date_from_json(value: json.Value) -> Result[Date, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2ftime_2fdate_5ffrom_5fjson_5freader"></a>

- `pub fn time.date_from_json_reader(mut cursor: json.Reader) -> Result[Date, String] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/json/Reader`](#def-std_401_2fjson_2fReader), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2ftime_2fdate_5fjson_5finto"></a>

- `@preserves_unique(out) pub fn time.date_json_into(value: Date, mut out: List[u8], depth: u64) -> Result[ (), json.JsonError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/json/JsonError`](#def-std_401_2fjson_2fJsonError), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2ftime_2fdate_5fto_5fjson"></a>

- `pub fn time.date_to_json(value: Date) -> json.Value uses {}`

References: [`std@1/json/Value`](#def-std_401_2fjson_2fValue), [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2ftime_2fdeadline"></a>

- `@noalloc pub fn time.deadline(clock: MonotonicClock, interval: Duration) -> Result[ Instant of clock, SleepError ]`

References: [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)

<a id="def-std_401_2ftime_2felapsed"></a>

- `@noalloc pub fn time.elapsed(clock: MonotonicClock, start: Instant of clock) -> Result[ Duration, ClockError ]`

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/Instant`](#def-core_401_2fInstant), [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration)

<a id="def-std_401_2ftime_2fformat"></a>

- `pub fn time.format(value: BrokenDownTime, pattern: String) -> Result[ unique String, FormatError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/time/BrokenDownTime`](#def-std_401_2ftime_2fBrokenDownTime), [`std@1/time/FormatError`](#def-std_401_2ftime_2fFormatError)

<a id="def-std_401_2ftime_2fparse"></a>

- `@noalloc pub fn time.parse(text: String) -> Result[Timestamp, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_2fparse_5fwith"></a>

- `@noalloc pub fn time.parse_with(text: String, pattern: String) -> Result[ BrokenDownTime, scalar.ParseError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/BrokenDownTime`](#def-std_401_2ftime_2fBrokenDownTime)

<a id="def-std_401_2ftime_2fsleep"></a>

- `@noalloc pub fn time.sleep(clock: MonotonicClock, interval: Duration) -> Result[(), SleepError]`

References: [`core@1/MonotonicClock`](#def-core_401_2fMonotonicClock), [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/Duration`](#def-std_401_2ftime_2fDuration), [`std@1/time/SleepError`](#def-std_401_2ftime_2fSleepError)

<a id="def-std_401_2ftime_2fto_5fstring"></a>

- `pub fn time.to_string(value: Timestamp) -> Result[unique String, TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_2fto_5fstring_5fat"></a>

- `pub fn time.to_string_at(value: Timestamp, offset: i32) -> Result[unique String, TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)


## time_of_day

<a id="def-std_401_2ftime_5fof_5fday_2fcompare"></a>

- `@noalloc pub fn time_of_day.compare(a: time.TimeOfDay, b: time.TimeOfDay) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn time_of_day.format_into(value: time.TimeOfDay, mut out: List[u8]) -> () uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fhour"></a>

- `@noalloc pub fn time_of_day.hour(value: time.TimeOfDay) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fmidnight"></a>

- `@noalloc pub fn time_of_day.midnight() -> time.TimeOfDay uses {}`

References: [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fminute"></a>

- `@noalloc pub fn time_of_day.minute(value: time.TimeOfDay) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fnanosecond"></a>

- `@noalloc pub fn time_of_day.nanosecond(value: time.TimeOfDay) -> u32 uses {}`

References: [`core@1/u32`](#def-core_401_2fu32), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fnew"></a>

- `@noalloc pub fn time_of_day.new(h: u8, m: u8, s: u8, ns: u32) -> Option[time.TimeOfDay] uses {}`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fparse"></a>

- `@noalloc pub fn time_of_day.parse(text: String) -> Result[time.TimeOfDay, scalar.ParseError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fsecond"></a>

- `@noalloc pub fn time_of_day.second(value: time.TimeOfDay) -> u8 uses {}`

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftime_5fof_5fday_2fto_5fstring"></a>

- `pub fn time_of_day.to_string(value: time.TimeOfDay) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)


## time_zone

<a id="def-std_401_2ftime_5fzone_2ffixed"></a>

- `pub fn time_zone.fixed(seconds: i32) -> Result[time.TimeZone, time.TimeError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone)

<a id="def-std_401_2ftime_5fzone_2ffrom_5ftzif"></a>

- `pub fn time_zone.from_tzif(data: Bytes) -> Result[time.TimeZone, scalar.ParseError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/scalar/ParseError`](#def-std_401_2fscalar_2fParseError), [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone)

<a id="def-std_401_2ftime_5fzone_2foffset"></a>

- `@noalloc pub fn time_zone.offset(zone: time.TimeZone, value: time.Timestamp) -> i32 uses {}`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_5fzone_2fto_5fdatetime"></a>

- `@noalloc pub fn time_zone.to_datetime(zone: time.TimeZone, value: time.Timestamp) -> Result[ time.DateTime, time.TimeError ] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/TimeError`](#def-std_401_2ftime_2fTimeError), [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp)

<a id="def-std_401_2ftime_5fzone_2fto_5ftimestamp"></a>

- `@noalloc pub fn time_zone.to_timestamp( zone: time.TimeZone, value: time.DateTime, disambiguation: time.Disambiguation ) -> Result[time.Timestamp, time.ZoneError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/Disambiguation`](#def-std_401_2ftime_2fDisambiguation), [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone), [`std@1/time/Timestamp`](#def-std_401_2ftime_2fTimestamp), [`std@1/time/ZoneError`](#def-std_401_2ftime_2fZoneError)

<a id="def-std_401_2ftime_5fzone_2futc"></a>

- `pub fn time_zone.utc() -> time.TimeZone uses {}`

References: [`std@1/time/TimeZone`](#def-std_401_2ftime_2fTimeZone)


## tls

<a id="def-core_401_2ftls_2faccept"></a>

- `fn tls.accept(n: Net, take s: Stream of n, server: TlsServer) -> Result[Stream of n, NetError]`

on a stream accepted on a listener of `n`

<a id="def-core_401_2ftls_2fclose"></a>

- `@noalloc fn tls.close(n: Net, take s: Stream of n) -> Result[(), NetError]`

<a id="def-core_401_2ftls_2fconnect"></a>

- `fn tls.connect(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots) -> Result[Stream of n, NetError]`

on a `ConnectTls` stream, `server_name` equal to the rule's address

<a id="def-core_401_2ftls_2fconnect_5fas"></a>

- `fn tls.connect_as(n: Net, take s: Stream of n, server_name: String, roots: TlsRoots, identity: TlsIdentity) -> Result[Stream of n, NetError]`

<a id="def-core_401_2ftls_2fconnect_5fwith"></a>

- `fn tls.connect_with(n: Net, take s: Stream of n, server_name: String, client: TlsClient) -> Result[Stream of n, NetError]`

<a id="def-core_401_2ftls_2fnegotiated_5fprotocol"></a>

- `fn tls.negotiated_protocol(n: Net, s: Stream of n) -> Result[Option[String], NetError]`

<a id="def-core_401_2ftls_2fpeer"></a>

- `fn tls.peer(n: Net, s: Stream of n) -> Result[Option[Bytes], NetError]`

the stream's own grant


## toml

<a id="def-std_401_2ftoml_2fDocument"></a>

- `pub opaque type toml.Document: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2ftoml_2fMAX_5fNESTING"></a>

- `pub const toml.MAX_NESTING: u64`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoml_2fMoment"></a>

- `@serializable pub type toml.Moment = OffsetDateTime(time.DateTime, i32) | LocalDateTime(time.DateTime) | LocalDate( time.Date ) | LocalTime(time.TimeOfDay)`

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/Date`](#def-std_401_2ftime_2fDate), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime), [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftoml_2fMoment_23LocalDate"></a>

- `LocalDate( time.Date ) -> toml.Moment`

Variant of toml.Moment.

References: [`std@1/time/Date`](#def-std_401_2ftime_2fDate)

<a id="def-std_401_2ftoml_2fMoment_23LocalDateTime"></a>

- `LocalDateTime(time.DateTime) -> toml.Moment`

Variant of toml.Moment.

References: [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2ftoml_2fMoment_23LocalTime"></a>

- `LocalTime(time.TimeOfDay) -> toml.Moment`

Variant of toml.Moment.

References: [`std@1/time/TimeOfDay`](#def-std_401_2ftime_2fTimeOfDay)

<a id="def-std_401_2ftoml_2fMoment_23OffsetDateTime"></a>

- `OffsetDateTime(time.DateTime, i32) -> toml.Moment`

Variant of toml.Moment.

References: [`core@1/i32`](#def-core_401_2fi32), [`std@1/time/DateTime`](#def-std_401_2ftime_2fDateTime)

<a id="def-std_401_2ftoml_2fPosition"></a>

- `@serializable pub type toml.Position { line: u64, column: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoml_2fSegment"></a>

- `@serializable pub type toml.Segment = Key(String) | Index(u64)`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoml_2fSegment_23Index"></a>

- `Index(u64) -> toml.Segment`

Variant of toml.Segment.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoml_2fSegment_23Key"></a>

- `Key(String) -> toml.Segment`

Variant of toml.Segment.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftoml_2fTomlError"></a>

- `@serializable pub type toml.TomlError = Syntax(Position) | DuplicateKey(Position) | Nesting(Position) | InvalidValue | NotFound | NotTable`

References: [`std@1/toml/Position`](#def-std_401_2ftoml_2fPosition)

<a id="def-std_401_2ftoml_2fTomlError_23DuplicateKey"></a>

- `DuplicateKey(Position) -> toml.TomlError`

Variant of toml.TomlError.

References: [`std@1/toml/Position`](#def-std_401_2ftoml_2fPosition)

<a id="def-std_401_2ftoml_2fTomlError_23InvalidValue"></a>

- `InvalidValue -> toml.TomlError`

Variant of toml.TomlError.

<a id="def-std_401_2ftoml_2fTomlError_23Nesting"></a>

- `Nesting(Position) -> toml.TomlError`

Variant of toml.TomlError.

References: [`std@1/toml/Position`](#def-std_401_2ftoml_2fPosition)

<a id="def-std_401_2ftoml_2fTomlError_23NotFound"></a>

- `NotFound -> toml.TomlError`

Variant of toml.TomlError.

<a id="def-std_401_2ftoml_2fTomlError_23NotTable"></a>

- `NotTable -> toml.TomlError`

Variant of toml.TomlError.

<a id="def-std_401_2ftoml_2fTomlError_23Syntax"></a>

- `Syntax(Position) -> toml.TomlError`

Variant of toml.TomlError.

References: [`std@1/toml/Position`](#def-std_401_2ftoml_2fPosition)

<a id="def-std_401_2ftoml_2fValue"></a>

- `@serializable pub type toml.Value = Text(String) | Integer(i64) | Float(f64) | Bool(bool) | Datetime( String ) | Array(List[Value]) | Table(Map[String, Value])`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/f64`](#def-core_401_2ff64), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2ftoml_2fValue_23Array"></a>

- `Array(List[Value]) -> toml.Value`

Variant of toml.Value.

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2ftoml_2fValue_23Bool"></a>

- `Bool(bool) -> toml.Value`

Variant of toml.Value.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2ftoml_2fValue_23Datetime"></a>

- `Datetime( String ) -> toml.Value`

Variant of toml.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftoml_2fValue_23Float"></a>

- `Float(f64) -> toml.Value`

Variant of toml.Value.

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2ftoml_2fValue_23Integer"></a>

- `Integer(i64) -> toml.Value`

Variant of toml.Value.

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2ftoml_2fValue_23Table"></a>

- `Table(Map[String, Value]) -> toml.Value`

Variant of toml.Value.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftoml_2fValue_23Text"></a>

- `Text(String) -> toml.Value`

Variant of toml.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2ftoml_2fdatetime"></a>

- `pub fn toml.datetime(value: Moment) -> Result[Value, TomlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/toml/Moment`](#def-std_401_2ftoml_2fMoment), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)

<a id="def-std_401_2ftoml_2fdocument"></a>

- `pub fn toml.document(take text: String) -> Result[Document, TomlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError)

<a id="def-std_401_2ftoml_2fget"></a>

- `pub fn toml.get(doc: Document, path: List[Segment]) -> Option[Value] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument), [`std@1/toml/Segment`](#def-std_401_2ftoml_2fSegment), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)

<a id="def-std_401_2ftoml_2fmoment"></a>

- `pub fn toml.moment(spelling: String) -> Result[Moment, TomlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/toml/Moment`](#def-std_401_2ftoml_2fMoment), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError)

<a id="def-std_401_2ftoml_2fparse"></a>

- `pub fn toml.parse(take text: String) -> Result[Value, TomlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)

<a id="def-std_401_2ftoml_2fremove"></a>

- `pub fn toml.remove(mut doc: Document, path: List[Segment]) -> Result[(), TomlError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument), [`std@1/toml/Segment`](#def-std_401_2ftoml_2fSegment), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError)

<a id="def-std_401_2ftoml_2froot"></a>

- `pub fn toml.root(doc: Document) -> Value uses {}`

References: [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)

<a id="def-std_401_2ftoml_2fset"></a>

- `pub fn toml.set(mut doc: Document, path: List[Segment], value: Value) -> Result[ (), TomlError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument), [`std@1/toml/Segment`](#def-std_401_2ftoml_2fSegment), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)

<a id="def-std_401_2ftoml_2fsource"></a>

- `pub fn toml.source(doc: Document) -> String in doc uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/toml/Document`](#def-std_401_2ftoml_2fDocument)

<a id="def-std_401_2ftoml_2fstringify"></a>

- `pub fn toml.stringify(value: Value) -> Result[unique String, TomlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/toml/TomlError`](#def-std_401_2ftoml_2fTomlError), [`std@1/toml/Value`](#def-std_401_2ftoml_2fValue)


## toposort

<a id="def-std_401_2ftoposort_2fEdge"></a>

- `pub type toposort.Edge { from: u64, to: u64, }`

An edge says `from` must precede `to`.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2fGraphError"></a>

- `pub type toposort.GraphError = NodeLimit(u64) | EdgeLimit(u64) | Endpoint(u64, u64) | Blocked( List[u64] )`

Limit payloads are supplied counts; Endpoint carries the first invalid
edge's input index and endpoint (from is checked first). Blocked is sorted
and includes downstream vertices that cannot be released, not just cycles.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2fGraphError_23Blocked"></a>

- `Blocked( List[u64] ) -> toposort.GraphError`

Variant of toposort.GraphError.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2fGraphError_23EdgeLimit"></a>

- `EdgeLimit(u64) -> toposort.GraphError`

Variant of toposort.GraphError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2fGraphError_23Endpoint"></a>

- `Endpoint(u64, u64) -> toposort.GraphError`

Variant of toposort.GraphError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2fGraphError_23NodeLimit"></a>

- `NodeLimit(u64) -> toposort.GraphError`

Variant of toposort.GraphError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftoposort_2forder"></a>

- `pub fn toposort.order(node_count: u64, edges: List[Edge], max_nodes: u64, max_edges: u64) -> Result[ List[u64], GraphError ] uses {}`

Order vertices 0..node_count within caller-supplied inclusive caps.
Initially ready IDs are ascending, then newly ready IDs join the FIFO in
input successor order. This is deterministic, not lexicographically minimal.
Counts and all endpoints are checked before allocating graph storage.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/toposort/Edge`](#def-std_401_2ftoposort_2fEdge), [`std@1/toposort/GraphError`](#def-std_401_2ftoposort_2fGraphError)


## txn

<a id="def-std_401_2ftxn_2fdelete"></a>

- `pub fn txn.delete(mut t: store.Txn, take key: Bytes) -> () uses {}`

Removes `key` when the transaction commits.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2ftxn_2fget"></a>

- `pub fn txn.get(mut t: store.Txn, key: Bytes) -> Option[Bytes] in t uses {}`

The value under `key` as this transaction sees it; a serializable
transaction records the read.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2ftxn_2fmark_5fread"></a>

- `pub fn txn.mark_read(mut t: store.Txn, take key: Bytes) -> () uses {}`

Records `key` as read, under either isolation, without reading it
(DbTransaction::mark_read, db_transaction.rs:410): a commit after the
transaction began that writes `key` makes its commit conflict.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2ftxn_2fput"></a>

- `pub fn txn.put(mut t: store.Txn, take key: Bytes, take value: Bytes) -> () uses {}`

Stores `value` under `key` when the transaction commits.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)

<a id="def-std_401_2ftxn_2frange"></a>

- `pub fn txn.range( mut t: store.Txn, from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], direction: ordered_map.Direction, limit: u64 ) -> List[(Bytes, Bytes)] uses {}`

At most `limit` entries with keys between `from` and `to` as this
transaction sees them, in `direction` order, as fresh copies. A
serializable transaction records the whole range it asked for, entries
it did not return included.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound), [`std@1/ordered_map/Direction`](#def-std_401_2fordered_5fmap_2fDirection), [`std@1/store/Txn`](#def-std_401_2fstore_2fTxn)


## txn_manager

<a id="def-std_401_2ftxn_5fmanager_2fLimits"></a>

- `pub type txn_manager.Limits { commits: u64, key_bytes: u64, }`

The most a manager retains: `commits` committed write sets or `key_bytes`
bytes of their keys, whichever is reached first.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2ftxn_5fmanager_2fManager"></a>

- `pub opaque type txn_manager.Manager: () uses {}`

Conflict state for one store's transactions.

<a id="def-std_401_2ftxn_5fmanager_2fReadRange"></a>

- `pub type txn_manager.ReadRange { from: ordered_map.Bound[Bytes], to: ordered_map.Bound[Bytes], }`

A key range read: both ends, as `ordered_map.fold_range` takes them.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/ordered_map/Bound`](#def-std_401_2fordered_5fmap_2fBound)

<a id="def-std_401_2ftxn_5fmanager_2factive"></a>

- `pub fn txn_manager.active(m: Manager) -> u64 uses {}`

How many transactions are active.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fbegin"></a>

- `pub fn txn_manager.begin(mut m: Manager) -> (u64, u64) uses {}`

Registers a transaction at the last committed sequence and returns its id
and that sequence (new_transaction, transaction_manager.rs:112).

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fbegin_5fat"></a>

- `pub fn txn_manager.begin_at(mut m: Manager, seq: u64) -> u64 uses {}`

Registers a transaction that began at `seq` and returns its id (SlateDB's
new_txn_with_id, transaction_manager.rs:130).

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fcontains"></a>

- `pub fn txn_manager.contains(r: ReadRange, key: Bytes) -> bool uses {}`

Whether `key` lies in `r`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool), [`std@1/txn_manager/ReadRange`](#def-std_401_2ftxn_5fmanager_2fReadRange)

<a id="def-std_401_2ftxn_5fmanager_2fdefault_5flimits"></a>

- `pub fn txn_manager.default_limits() -> Limits uses {}`

4096 write sets or 1 MiB of keys.

References: [`std@1/txn_manager/Limits`](#def-std_401_2ftxn_5fmanager_2fLimits)

<a id="def-std_401_2ftxn_5fmanager_2fend"></a>

- `pub fn txn_manager.end(mut m: Manager, id: u64) -> () uses {}`

Ends transaction `id` without a commit and recycles what no active
transaction needs (drop_txn, transaction_manager.rs:151). An unknown id
changes nothing but the recycling.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fhas_5fconflict"></a>

- `pub fn txn_manager.has_conflict( m: Manager, started: u64, writes: List[Bytes], reads: List[Bytes], ranges: List[ReadRange] ) -> bool uses {}`

Whether a transaction that began at `started` conflicts with a commit
after it: one that wrote a key in `writes` or `reads`, or a key inside one
of `ranges` (check_has_conflict, has_write_write_conflict and
has_read_write_conflict, transaction_manager.rs:190, 324 and 350). Pass
empty `reads` and `ranges` for snapshot isolation.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager), [`std@1/txn_manager/ReadRange`](#def-std_401_2ftxn_5fmanager_2fReadRange)

<a id="def-std_401_2ftxn_5fmanager_2fhorizon"></a>

- `pub fn txn_manager.horizon(m: Manager) -> u64 uses {}`

The commit at and before which nothing can be checked any more: a
transaction that began before it conflicts when it has anything to check.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2flast_5fcommitted"></a>

- `pub fn txn_manager.last_committed(m: Manager) -> u64 uses {}`

The last committed sequence the manager has seen.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fmin_5factive"></a>

- `pub fn txn_manager.min_active(m: Manager) -> Option[u64] uses {}`

The oldest sequence an active transaction began at (min_active_seq,
transaction_manager.rs:259).

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fnew"></a>

- `pub fn txn_manager.new(seq: u64, limits: Limits) -> Manager uses {}`

A manager whose last committed sequence is `seq`, with nothing active.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Limits`](#def-std_401_2ftxn_5fmanager_2fLimits), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2frecycle"></a>

- `pub fn txn_manager.recycle(mut m: Manager) -> () uses {}`

Drops every retained write set older than the oldest active transaction,
keeping one committed at its very sequence, or all of them when nothing is
active (recycle_recent_committed_txns, transaction_manager.rs:296).

References: [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2fretained"></a>

- `pub fn txn_manager.retained(m: Manager) -> List[u64] uses {}`

The sequences of the retained write sets, oldest first.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2ftrack_5fbatch"></a>

- `pub fn txn_manager.track_batch(mut m: Manager, seq: u64, take writes: List[Bytes]) -> () uses {}`

Records a commit made outside any registered transaction
(track_recent_committed_write_batch, transaction_manager.rs:241).

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)

<a id="def-std_401_2ftxn_5fmanager_2ftrack_5fcommitted"></a>

- `pub fn txn_manager.track_committed(mut m: Manager, id: u64, seq: u64, take writes: List[Bytes]) -> () uses {}`

Ends transaction `id` as committed at `seq` with write keys `writes`,
retained while an active transaction may conflict with them
(track_recent_committed_txn, transaction_manager.rs:219). An unknown id
records nothing but the sequence.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/txn_manager/Manager`](#def-std_401_2ftxn_5fmanager_2fManager)


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

<a id="def-std_401_2fu128_2fformat_5finline"></a>

- `@noalloc pub fn u128.format_inline[n: size](value: u128, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fu16_2fformat_5finline"></a>

- `@noalloc pub fn u16.format_inline[n: size](value: u16, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fu32_2fformat_5finline"></a>

- `@noalloc pub fn u32.format_inline[n: size](value: u32, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fu64_2fformat_5finline"></a>

- `@noalloc pub fn u64.format_inline[n: size](value: u64, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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

<a id="def-std_401_2fu8_2fformat_5finline"></a>

- `@noalloc pub fn u8.format_inline[n: size](value: u8, mut buffer: Inline[u8, n]) -> Result[ u64, scalar.FormatError ] uses {}`

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


## union_find

<a id="def-std_401_2funion_5ffind_2fError"></a>

- `pub type union_find.Error = NodeLimit(u64, u64) | InvalidId(u64, u64)`

Requested nodes and cap, or invalid ID and domain length.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2funion_5ffind_2fError_23InvalidId"></a>

- `InvalidId(u64, u64) -> union_find.Error`

Variant of union_find.Error.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2funion_5ffind_2fError_23NodeLimit"></a>

- `NodeLimit(u64, u64) -> union_find.Error`

Variant of union_find.Error.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2funion_5ffind_2fForest"></a>

- `pub opaque type union_find.Forest: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2funion_5ffind_2fcount"></a>

- `@noalloc pub fn union_find.count(forest: Forest) -> u64 uses {}`

Number of disjoint components, including isolated IDs.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)

<a id="def-std_401_2funion_5ffind_2ffind"></a>

- `pub fn union_find.find(mut forest: Forest, id: u64) -> Result[u64, Error] uses {}`

Returns an opaque representative; compresses paths with normal COW semantics.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Error`](#def-std_401_2funion_5ffind_2fError), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)

<a id="def-std_401_2funion_5ffind_2fnew"></a>

- `pub fn union_find.new(nodes: u64, max_nodes: u64) -> Result[Forest, Error] uses {}`

Creates singleton sets for IDs in [0, count); checks the cap before allocating.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Error`](#def-std_401_2funion_5ffind_2fError), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)

<a id="def-std_401_2funion_5ffind_2fsame"></a>

- `pub fn union_find.same(mut forest: Forest, a: u64, b: u64) -> Result[bool, Error] uses {}`

Checks both endpoints before compression, just like union.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Error`](#def-std_401_2funion_5ffind_2fError), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)

<a id="def-std_401_2funion_5ffind_2fsnapshot"></a>

- `pub fn union_find.snapshot(mut forest: Forest) -> unique List[u64] uses {}`

Owned ID-to-root snapshot, independent of later forest mutations.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)

<a id="def-std_401_2funion_5ffind_2funion"></a>

- `pub fn union_find.union(mut forest: Forest, a: u64, b: u64) -> Result[u64, Error] uses {}`

Returns the merged representative. Equal ranks prefer the first root.
Both IDs are checked before either path is compressed.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/union_find/Error`](#def-std_401_2funion_5ffind_2fError), [`std@1/union_find/Forest`](#def-std_401_2funion_5ffind_2fForest)


## uri

<a id="def-std_401_2furi_2fComponent"></a>

- `@serializable pub type uri.Component = Raw(String) | Encoded(String)`

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

- `@serializable pub type uri.Part = Unreserved | User | Password | Host | Path | Query | Fragment`

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

- `@serializable pub type uri.QueryError = InvalidEscape(u64) | InvalidSeparator(u64) | InvalidUtf8(u64)`

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

- `@serializable pub type uri.Uri { scheme: String, user: Option[Component], password: Option[Component], host: Option[Component], port: Option[u16], path: Component, query: Option[Component], fragment: Option[Component], }`

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/uri/Component`](#def-std_401_2furi_2fComponent)

<a id="def-std_401_2furi_2fUriError"></a>

- `@serializable pub type uri.UriError = UnexpectedCharacter | InvalidFormat | InvalidPort | InvalidHostName`

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

<a id="def-std_401_2furi_2fto_5fstring"></a>

- `pub fn uri.to_string(parsed_uri: Uri) -> unique String uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/uri/Uri`](#def-std_401_2furi_2fUri)


## utf16

<a id="def-std_401_2futf16_2fDecodeError"></a>

- `pub type utf16.DecodeError { index: u64, }`

The zero-based index of the first unpaired surrogate, in u16 code units.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2futf16_2fdecode_5flossy"></a>

- `pub fn utf16.decode_lossy(units: List[u16]) -> String uses {}`

Replace each unpaired surrogate with U+FFFD, preserving valid pairs.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2futf16_2fdecode_5fstrict"></a>

- `pub fn utf16.decode_strict(units: List[u16]) -> Result[String, DecodeError] uses {}`

Refuse the first unpaired surrogate; no partial String is returned.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/utf16/DecodeError`](#def-std_401_2futf16_2fDecodeError)

<a id="def-std_401_2futf16_2fencode"></a>

- `pub fn utf16.encode(value: String) -> unique List[u16] uses {}`

Encode valid String scalars into independently owned UTF-16 code units.
NUL and U+FEFF are ordinary data. No byte order or BOM policy is applied.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16)


## utf8

<a id="def-std_401_2futf8_2fDecoder"></a>

- `pub opaque type utf8.Decoder: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2futf8_2fPolicy"></a>

- `@serializable pub type utf8.Policy = Strict | Lossy`

<a id="def-std_401_2futf8_2fPolicy_23Lossy"></a>

- `Lossy -> utf8.Policy`

Variant of utf8.Policy.

<a id="def-std_401_2futf8_2fPolicy_23Strict"></a>

- `Strict -> utf8.Policy`

Variant of utf8.Policy.

<a id="def-std_401_2futf8_2fStreamError"></a>

- `@serializable pub type utf8.StreamError = InvalidSequence(u64) | IncompleteSequence(u64) | OffsetOverflow | AlreadyFinished | DecoderFailed`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2futf8_2fStreamError_23AlreadyFinished"></a>

- `AlreadyFinished -> utf8.StreamError`

Variant of utf8.StreamError.

<a id="def-std_401_2futf8_2fStreamError_23DecoderFailed"></a>

- `DecoderFailed -> utf8.StreamError`

Variant of utf8.StreamError.

<a id="def-std_401_2futf8_2fStreamError_23IncompleteSequence"></a>

- `IncompleteSequence(u64) -> utf8.StreamError`

Variant of utf8.StreamError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2futf8_2fStreamError_23InvalidSequence"></a>

- `InvalidSequence(u64) -> utf8.StreamError`

Variant of utf8.StreamError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2futf8_2fStreamError_23OffsetOverflow"></a>

- `OffsetOverflow -> utf8.StreamError`

Variant of utf8.StreamError.

<a id="def-std_401_2futf8_2fStreamStatus"></a>

- `@serializable pub type utf8.StreamStatus = Active | Finished | Failed`

<a id="def-std_401_2futf8_2fStreamStatus_23Active"></a>

- `Active -> utf8.StreamStatus`

Variant of utf8.StreamStatus.

<a id="def-std_401_2futf8_2fStreamStatus_23Failed"></a>

- `Failed -> utf8.StreamStatus`

Variant of utf8.StreamStatus.

<a id="def-std_401_2futf8_2fStreamStatus_23Finished"></a>

- `Finished -> utf8.StreamStatus`

Variant of utf8.StreamStatus.

<a id="def-std_401_2futf8_2fdecode_5flossy"></a>

- `pub fn utf8.decode_lossy(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2futf8_2fdecoder"></a>

- `@noalloc pub fn utf8.decoder(policy: Policy) -> Decoder uses {}`

References: [`std@1/utf8/Decoder`](#def-std_401_2futf8_2fDecoder), [`std@1/utf8/Policy`](#def-std_401_2futf8_2fPolicy)

<a id="def-std_401_2futf8_2ffeed"></a>

- `pub fn utf8.feed(mut d: Decoder, chunk: Bytes, eof: bool) -> Result[String, StreamError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`std@1/utf8/Decoder`](#def-std_401_2futf8_2fDecoder), [`std@1/utf8/StreamError`](#def-std_401_2futf8_2fStreamError)

<a id="def-std_401_2futf8_2foffset"></a>

- `@noalloc pub fn utf8.offset(d: Decoder) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/utf8/Decoder`](#def-std_401_2futf8_2fDecoder)

<a id="def-std_401_2futf8_2fpending_5fbytes"></a>

- `@noalloc pub fn utf8.pending_bytes(d: Decoder) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/utf8/Decoder`](#def-std_401_2futf8_2fDecoder)

<a id="def-std_401_2futf8_2fstatus"></a>

- `@noalloc pub fn utf8.status(d: Decoder) -> StreamStatus uses {}`

References: [`std@1/utf8/Decoder`](#def-std_401_2futf8_2fDecoder), [`std@1/utf8/StreamStatus`](#def-std_401_2futf8_2fStreamStatus)


## uuid

<a id="def-std_401_2fuuid_2fUuid"></a>

- `@format(format_into) pub type uuid.Uuid { bits: u128, }`

A UUID's 128 bits, most significant first: byte 0 of the RFC 9562 layout is
the top byte of `bits`, so comparing `bits` orders version 7 UUIDs by time.

References: [`core@1/u128`](#def-core_401_2fu128), [`std@1/uuid/format_into`](#def-std_401_2fuuid_2fformat_5finto)

<a id="def-std_401_2fuuid_2fUuidError"></a>

- `pub type uuid.UuidError = Length(u64) | Char(u64)`

`Length` carries the byte length of text that is none of the four spellings;
`Char` the byte offset of the first character that does not fit the
spelling its length selects.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fuuid_2fUuidError_23Char"></a>

- `Char(u64) -> uuid.UuidError`

Variant of uuid.UuidError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fuuid_2fUuidError_23Length"></a>

- `Length(u64) -> uuid.UuidError`

Variant of uuid.UuidError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fuuid_2fV7Error"></a>

- `pub type uuid.V7Error = Clock(ClockError) | Random(RandomError) | BeforeEpoch`

Version 7 generation fails when the clock or the random source fails, or
when the wall clock reads before the Unix epoch.

References: [`core@1/ClockError`](#def-core_401_2fClockError), [`core@1/RandomError`](#def-core_401_2fRandomError)

<a id="def-std_401_2fuuid_2fV7Error_23BeforeEpoch"></a>

- `BeforeEpoch -> uuid.V7Error`

Variant of uuid.V7Error.

<a id="def-std_401_2fuuid_2fV7Error_23Clock"></a>

- `Clock(ClockError) -> uuid.V7Error`

Variant of uuid.V7Error.

References: [`core@1/ClockError`](#def-core_401_2fClockError)

<a id="def-std_401_2fuuid_2fV7Error_23Random"></a>

- `Random(RandomError) -> uuid.V7Error`

Variant of uuid.V7Error.

References: [`core@1/RandomError`](#def-core_401_2fRandomError)

<a id="def-std_401_2fuuid_2fVariant"></a>

- `pub type uuid.Variant = Ncs | Rfc9562 | Microsoft | Future`

The RFC 9562 variant field: `Ncs` (0xxx), `Rfc9562` (10xx, the variant of
versions 1 through 8), `Microsoft` (110x) and `Future` (111x).

<a id="def-std_401_2fuuid_2fVariant_23Future"></a>

- `Future -> uuid.Variant`

Variant of uuid.Variant.

<a id="def-std_401_2fuuid_2fVariant_23Microsoft"></a>

- `Microsoft -> uuid.Variant`

Variant of uuid.Variant.

<a id="def-std_401_2fuuid_2fVariant_23Ncs"></a>

- `Ncs -> uuid.Variant`

Variant of uuid.Variant.

<a id="def-std_401_2fuuid_2fVariant_23Rfc9562"></a>

- `Rfc9562 -> uuid.Variant`

Variant of uuid.Variant.

<a id="def-std_401_2fuuid_2fformat_5finto"></a>

- `@preserves_unique(out) pub fn uuid.format_into(id: Uuid, mut out: List[u8]) -> () uses {}`

Appends the lowercase hyphenated spelling.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u8`](#def-core_401_2fu8), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2ffrom_5fbytes"></a>

- `pub fn uuid.from_bytes(data: Bytes) -> Option[Uuid] uses {}`

The UUID of 16 bytes in RFC 9562 order, or `None()` for any other length.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fparse"></a>

- `pub fn uuid.parse(spelling: String) -> Result[Uuid, UuidError] uses {}`

Parses `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, the same in braces, the same
after `urn:uuid:` (in any case), or 32 hex digits, with hex digits in either
case.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid), [`std@1/uuid/UuidError`](#def-std_401_2fuuid_2fUuidError)

<a id="def-std_401_2fuuid_2fto_5fbytes"></a>

- `pub fn uuid.to_bytes(id: Uuid) -> unique Bytes uses {}`

The 16 bytes in RFC 9562 order.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fto_5fstring"></a>

- `pub fn uuid.to_string(id: Uuid) -> unique String uses {}`

The lowercase hyphenated spelling, `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2funix_5fms"></a>

- `@noalloc pub fn uuid.unix_ms(id: Uuid) -> Option[u64] uses {}`

A version 7 UUID's Unix timestamp in milliseconds, its top 48 bits;
`None()` for any other version or variant.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fv4"></a>

- `pub fn uuid.v4(r: Random) -> Result[Uuid, RandomError]`

A version 4 UUID of 122 bits drawn from `r`.

References: [`core@1/Random`](#def-core_401_2fRandom), [`core@1/RandomError`](#def-core_401_2fRandomError), [`core@1/Result`](#def-core_401_2fResult), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fv4_5ffrom"></a>

- `pub fn uuid.v4_from(random: Bytes) -> Option[Uuid] uses {}`

The version 4 UUID of 16 random bytes: the version and variant fields
replace six of their bits, as RFC 9562 section 5.4. Other lengths are
`None()`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fv7"></a>

- `pub fn uuid.v7(clock: WallClock, r: Random) -> Result[Uuid, V7Error]`

A version 7 UUID of the wall clock's Unix millisecond, rounded down, and 74
bits drawn from `r`. Two UUIDs of the same millisecond are unordered: no
counter is kept between calls.

References: [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`core@1/WallClock`](#def-core_401_2fWallClock), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid), [`std@1/uuid/V7Error`](#def-std_401_2fuuid_2fV7Error)

<a id="def-std_401_2fuuid_2fv7_5ffrom"></a>

- `pub fn uuid.v7_from(millis: u64, random: Bytes) -> Option[Uuid] uses {}`

The version 7 UUID of a Unix millisecond timestamp below 2^48 and 10
random bytes, as RFC 9562 section 5.7: the timestamp fills the top 48
bits and the bytes the remaining 80, the version and variant fields
replacing the top four bits of the first byte and the top two of the third.
A larger timestamp or another length is `None()`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u64`](#def-core_401_2fu64), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)

<a id="def-std_401_2fuuid_2fvariant"></a>

- `@noalloc pub fn uuid.variant(id: Uuid) -> Variant uses {}`

The variant field, the top bits of byte 8.

References: [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid), [`std@1/uuid/Variant`](#def-std_401_2fuuid_2fVariant)

<a id="def-std_401_2fuuid_2fversion"></a>

- `@noalloc pub fn uuid.version(id: Uuid) -> u8 uses {}`

The version field, the top four bits of byte 6, whatever the variant.

References: [`core@1/u8`](#def-core_401_2fu8), [`std@1/uuid/Uuid`](#def-std_401_2fuuid_2fUuid)


## varint

<a id="def-std_401_2fvarint_2fReadError"></a>

- `pub type varint.ReadError = OffsetBeyondEnd(u64) | Truncated(u64) | Overflow(u64)`

Absolute offsets: requested out-of-range cursor, first missing byte, or
invalid tenth byte. Errors contain no partial scalar or advanced cursor.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fvarint_2fReadError_23OffsetBeyondEnd"></a>

- `OffsetBeyondEnd(u64) -> varint.ReadError`

Variant of varint.ReadError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fvarint_2fReadError_23Overflow"></a>

- `Overflow(u64) -> varint.ReadError`

Variant of varint.ReadError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fvarint_2fReadError_23Truncated"></a>

- `Truncated(u64) -> varint.ReadError`

Variant of varint.ReadError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fvarint_2fappend_5fi64"></a>

- `@preserves_unique(out) pub fn varint.append_i64(value: i64, mut out: List[u8]) -> () uses {}`

Append canonical ZigZag without negating a signed value.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fvarint_2fappend_5fu64"></a>

- `@preserves_unique(out) pub fn varint.append_u64(value: u64, mut out: List[u8]) -> () uses {}`

Append the canonical shortest unsigned base128 encoding, preserving prefix.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fvarint_2fread_5fi64"></a>

- `@noalloc pub fn varint.read_i64(data: Bytes, offset: u64) -> Result[(i64, u64), ReadError] uses {}`

Read signed ZigZag (not signed LEB128), including i64 MIN.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u64`](#def-core_401_2fu64), [`std@1/varint/ReadError`](#def-std_401_2fvarint_2fReadError)

<a id="def-std_401_2fvarint_2fread_5fu64"></a>

- `@noalloc pub fn varint.read_u64(data: Bytes, offset: u64) -> Result[(u64, u64), ReadError] uses {}`

Read unsigned base128, returning (value, next absolute offset). Overlong
encodings are accepted. An invalid tenth byte overflows immediately.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/varint/ReadError`](#def-std_401_2fvarint_2fReadError)


## watcher

<a id="def-core_401_2fwatcher_2fnext"></a>

- `fn watcher.next(d: Dir, mut w: Watcher of d) -> Result[WatchEvent, FileError]`


## websocket

<a id="def-std_401_2fwebsocket_2fConfig"></a>

- `pub type websocket.Config { transport: http.Config, protocols: List[String], max_message_bytes: u64, }`

`transport` dials the client and bounds its handshake head (`max_head_bytes`);
its read and write timeouts bound every socket wait on both sides.
`protocols` are offered by a client and, in preference order of the client's
offer, accepted by a server. A message, and so each frame, holds at most
`max_message_bytes`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/http/Config`](#def-std_401_2fhttp_2fConfig)

<a id="def-std_401_2fwebsocket_2fConn"></a>

- `pub opaque resource type websocket.Conn: noalloc_drop uses {Net, Random}`

References: [`core@1/Net`](#def-core_401_2fNet), [`core@1/Random`](#def-core_401_2fRandom)

<a id="def-std_401_2fwebsocket_2fFrame"></a>

- `pub type websocket.Frame { fin: bool, opcode: Opcode, payload: Bytes, }`

One frame with its payload unmasked.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/bool`](#def-core_401_2fbool), [`std@1/websocket/Opcode`](#def-std_401_2fwebsocket_2fOpcode)

<a id="def-std_401_2fwebsocket_2fHandshakeError"></a>

- `pub type websocket.HandshakeError = UnsupportedScheme | Status(u16) | NotWebSocket | BadAccept | UnexpectedProtocol | UnexpectedExtension | MethodNotGet | MissingKey | BadKey | BadVersion | ReservedHeader`

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fwebsocket_2fHandshakeError_23BadAccept"></a>

- `BadAccept -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23BadKey"></a>

- `BadKey -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23BadVersion"></a>

- `BadVersion -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23MethodNotGet"></a>

- `MethodNotGet -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23MissingKey"></a>

- `MissingKey -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23NotWebSocket"></a>

- `NotWebSocket -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23ReservedHeader"></a>

- `ReservedHeader -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23Status"></a>

- `Status(u16) -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fwebsocket_2fHandshakeError_23UnexpectedExtension"></a>

- `UnexpectedExtension -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23UnexpectedProtocol"></a>

- `UnexpectedProtocol -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fHandshakeError_23UnsupportedScheme"></a>

- `UnsupportedScheme -> websocket.HandshakeError`

Variant of websocket.HandshakeError.

<a id="def-std_401_2fwebsocket_2fMessage"></a>

- `pub type websocket.Message = Text(String) | Binary(Bytes)`

A complete data message.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fwebsocket_2fMessage_23Binary"></a>

- `Binary(Bytes) -> websocket.Message`

Variant of websocket.Message.

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fwebsocket_2fMessage_23Text"></a>

- `Text(String) -> websocket.Message`

Variant of websocket.Message.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fwebsocket_2fOpcode"></a>

- `pub type websocket.Opcode = Continuation | Text | Binary | Close | Ping | Pong`

<a id="def-std_401_2fwebsocket_2fOpcode_23Binary"></a>

- `Binary -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fOpcode_23Close"></a>

- `Close -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fOpcode_23Continuation"></a>

- `Continuation -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fOpcode_23Ping"></a>

- `Ping -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fOpcode_23Pong"></a>

- `Pong -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fOpcode_23Text"></a>

- `Text -> websocket.Opcode`

Variant of websocket.Opcode.

<a id="def-std_401_2fwebsocket_2fProtocolError"></a>

- `pub type websocket.ProtocolError = ReservedBits | ReservedOpcode(u8) | MaskRequired | UnexpectedMask | ControlTooLong | FragmentedControl | UnexpectedContinuation | ExpectedContinuation | BadCloseCode( u16 ) | BadClosePayload | LengthOverflow | InvalidUtf8 | TooLarge`

References: [`core@1/u16`](#def-core_401_2fu16), [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fwebsocket_2fProtocolError_23BadCloseCode"></a>

- `BadCloseCode( u16 ) -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

References: [`core@1/u16`](#def-core_401_2fu16)

<a id="def-std_401_2fwebsocket_2fProtocolError_23BadClosePayload"></a>

- `BadClosePayload -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23ControlTooLong"></a>

- `ControlTooLong -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23ExpectedContinuation"></a>

- `ExpectedContinuation -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23FragmentedControl"></a>

- `FragmentedControl -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23InvalidUtf8"></a>

- `InvalidUtf8 -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23LengthOverflow"></a>

- `LengthOverflow -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23MaskRequired"></a>

- `MaskRequired -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23ReservedBits"></a>

- `ReservedBits -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23ReservedOpcode"></a>

- `ReservedOpcode(u8) -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

References: [`core@1/u8`](#def-core_401_2fu8)

<a id="def-std_401_2fwebsocket_2fProtocolError_23TooLarge"></a>

- `TooLarge -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23UnexpectedContinuation"></a>

- `UnexpectedContinuation -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fProtocolError_23UnexpectedMask"></a>

- `UnexpectedMask -> websocket.ProtocolError`

Variant of websocket.ProtocolError.

<a id="def-std_401_2fwebsocket_2fWsError"></a>

- `pub type websocket.WsError = Http(http.HttpError) | Net(NetError) | Random(RandomError) | Handshake( HandshakeError ) | Protocol(ProtocolError) | Truncated | Closed`

References: [`core@1/NetError`](#def-core_401_2fNetError), [`core@1/RandomError`](#def-core_401_2fRandomError), [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError), [`std@1/websocket/HandshakeError`](#def-std_401_2fwebsocket_2fHandshakeError), [`std@1/websocket/ProtocolError`](#def-std_401_2fwebsocket_2fProtocolError)

<a id="def-std_401_2fwebsocket_2fWsError_23Closed"></a>

- `Closed -> websocket.WsError`

Variant of websocket.WsError.

<a id="def-std_401_2fwebsocket_2fWsError_23Handshake"></a>

- `Handshake( HandshakeError ) -> websocket.WsError`

Variant of websocket.WsError.

References: [`std@1/websocket/HandshakeError`](#def-std_401_2fwebsocket_2fHandshakeError)

<a id="def-std_401_2fwebsocket_2fWsError_23Http"></a>

- `Http(http.HttpError) -> websocket.WsError`

Variant of websocket.WsError.

References: [`std@1/http/HttpError`](#def-std_401_2fhttp_2fHttpError)

<a id="def-std_401_2fwebsocket_2fWsError_23Net"></a>

- `Net(NetError) -> websocket.WsError`

Variant of websocket.WsError.

References: [`core@1/NetError`](#def-core_401_2fNetError)

<a id="def-std_401_2fwebsocket_2fWsError_23Protocol"></a>

- `Protocol(ProtocolError) -> websocket.WsError`

Variant of websocket.WsError.

References: [`std@1/websocket/ProtocolError`](#def-std_401_2fwebsocket_2fProtocolError)

<a id="def-std_401_2fwebsocket_2fWsError_23Random"></a>

- `Random(RandomError) -> websocket.WsError`

Variant of websocket.WsError.

References: [`core@1/RandomError`](#def-core_401_2fRandomError)

<a id="def-std_401_2fwebsocket_2fWsError_23Truncated"></a>

- `Truncated -> websocket.WsError`

Variant of websocket.WsError.

<a id="def-std_401_2fwebsocket_2faccept"></a>

- `pub fn websocket.accept(take c: http.ServerStream, request: http.ServerRequest, config: Config) -> Result[ Conn, WsError ]`

Completes the server side of the opening handshake for `request`, the request
`next_stream` yielded with `c`. A request that is not a version-13 WebSocket
upgrade is answered and refused: 405 for a method other than GET, 426 with
`Sec-WebSocket-Version: 13` for another version, 400 otherwise. The selected
protocol is the client's first offer among `config.protocols`. Bytes already read
past the request belong to the connection; `config.transport`'s read and write
timeouts replace the HTTP request's.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/http/ServerRequest`](#def-std_401_2fhttp_2fServerRequest), [`std@1/http/ServerStream`](#def-std_401_2fhttp_2fServerStream), [`std@1/websocket/Config`](#def-std_401_2fwebsocket_2fConfig), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2faccept_5fkey"></a>

- `pub fn websocket.accept_key(key: String) -> unique String uses {}`

The Sec-WebSocket-Accept value for a Sec-WebSocket-Key: base64 of the SHA-1 of
the key followed by the RFC 6455 GUID.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fwebsocket_2fclose"></a>

- `pub fn websocket.close(mut c: Conn, code: u16, reason: String) -> Result[(), WsError]`

Starts the closing handshake with `code` and a UTF-8 `reason` of at most 123
bytes, then reads until the peer's close frame, discarding data messages, and
returns. Closing a connection whose peer already closed returns at once.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fconnect"></a>

- `pub fn websocket.connect( n: Net, r: Random, url: String, headers: List[http.Header], config: Config ) -> Result[Conn, WsError]`

Opens a client connection to a `ws` or `wss` URL: dials it as `http` or `https`
under `config.transport`, sends the RFC 6455 opening handshake with a key drawn
from `r`, the offered protocols and `headers` (which may not set a field the
handshake sets: `ReservedHeader()`), and checks the 101 response: the upgrade
fields, the accept key, no extensions and a protocol only among those offered.
Every frame the connection sends is masked with a fresh key from `r`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Net`](#def-core_401_2fNet), [`core@1/Random`](#def-core_401_2fRandom), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/http/Header`](#def-std_401_2fhttp_2fHeader), [`std@1/websocket/Config`](#def-std_401_2fwebsocket_2fConfig), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fdecode_5fclose"></a>

- `pub fn websocket.decode_close(payload: Bytes) -> Result[Option[(u16, String)], ProtocolError] uses {}`

Decodes a close frame's payload: `None()` when empty, otherwise the code and the
UTF-8 reason. A one-byte payload, an invalid code or a non-UTF-8 reason is rejected.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/websocket/ProtocolError`](#def-std_401_2fwebsocket_2fProtocolError)

<a id="def-std_401_2fwebsocket_2fdecode_5fframe"></a>

- `pub fn websocket.decode_frame(buf: Bytes, max_payload: u64) -> Result[ Option[(Frame, bool, u64)], ProtocolError ] uses {}`

Decodes the frame at the start of `buf`: the frame, whether it was masked and its
encoded length, or `None()` while incomplete. A payload above `max_payload` is
`TooLarge()`; reserved bits, reserved opcodes, a fragmented or long control frame
and a 64-bit length with its top bit set are rejected.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`core@1/bool`](#def-core_401_2fbool), [`core@1/u64`](#def-core_401_2fu64), [`std@1/websocket/Frame`](#def-std_401_2fwebsocket_2fFrame), [`std@1/websocket/ProtocolError`](#def-std_401_2fwebsocket_2fProtocolError)

<a id="def-std_401_2fwebsocket_2fdefault_5fconfig"></a>

- `pub fn websocket.default_config() -> Config uses {}`

x/net's default 32 MiB message bound over `http.default_config()`.

References: [`std@1/websocket/Config`](#def-std_401_2fwebsocket_2fConfig)

<a id="def-std_401_2fwebsocket_2fencode_5fframe"></a>

- `pub fn websocket.encode_frame(frame: Frame, mask: Option[Inline[u8, 4]]) -> unique Bytes uses {}`

Encodes `frame` with the minimal length form, masked with `mask` when present
(a client masks every frame; a server never does).

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Inline`](#def-core_401_2fInline), [`core@1/Option`](#def-core_401_2fOption), [`core@1/u8`](#def-core_401_2fu8), [`std@1/websocket/Frame`](#def-std_401_2fwebsocket_2fFrame)

<a id="def-std_401_2fwebsocket_2fpeer_5fclose"></a>

- `pub fn websocket.peer_close(c: Conn) -> Option[(u16, unique String)] uses {}`

The code and reason of the close frame the peer sent, once one was received:
1005 with an empty reason when its close frame carried no code.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/u16`](#def-core_401_2fu16), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn)

<a id="def-std_401_2fwebsocket_2fping"></a>

- `pub fn websocket.ping(mut c: Conn, data: Bytes) -> Result[(), WsError]`

Sends a ping carrying at most 125 bytes; the peer's pong is consumed by `receive`.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fprotocol"></a>

- `pub fn websocket.protocol(c: Conn) -> Option[unique String] uses {}`

The protocol the handshake selected, if any.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn)

<a id="def-std_401_2fwebsocket_2freceive"></a>

- `pub fn websocket.receive(mut c: Conn) -> Result[Option[Message], WsError]`

Receives the next data message, reassembling fragments. It answers each ping
with a pong carrying its payload and skips pongs. A close frame from the peer is
answered with a close frame echoing its code (when this side had not closed) and
returns `None()`; `peer_close` then reports it. A protocol violation sends a close
frame with code 1002 (1007 for text that is not UTF-8, 1009 for a message above
`max_message_bytes`) and returns `Protocol(...)`. End of stream before a close
frame is `Truncated()`; receiving after the closing handshake is `Closed()`.

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/Result`](#def-core_401_2fResult), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/Message`](#def-std_401_2fwebsocket_2fMessage), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fsend_5fbinary"></a>

- `pub fn websocket.send_binary(mut c: Conn, data: Bytes) -> Result[(), WsError]`

Sends `data` as one binary frame.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fsend_5fframe"></a>

- `pub fn websocket.send_frame(mut c: Conn, frame: Frame) -> Result[(), WsError]`

Sends one frame: a client masks it. Control frames must be final and carry at
most 125 bytes (`Protocol(...)` otherwise, sending nothing). After a close frame
is sent or received, sending is `Closed()`.

References: [`core@1/Result`](#def-core_401_2fResult), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/Frame`](#def-std_401_2fwebsocket_2fFrame), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)

<a id="def-std_401_2fwebsocket_2fsend_5ftext"></a>

- `pub fn websocket.send_text(mut c: Conn, message: String) -> Result[(), WsError]`

Sends `message` as one text frame.

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/websocket/Conn`](#def-std_401_2fwebsocket_2fConn), [`std@1/websocket/WsError`](#def-std_401_2fwebsocket_2fWsError)


## write_half

<a id="def-core_401_2fwrite_5fhalf_2fclose"></a>

- `@noalloc fn write_half.close(n: Net, take w: WriteHalf of n) -> ()`

<a id="def-core_401_2fwrite_5fhalf_2fset_5ftimeout"></a>

- `@noalloc fn write_half.set_timeout(n: Net, mut w: WriteHalf of n, timeout: NetTimeout) -> Result[(), NetError]`

<a id="def-core_401_2fwrite_5fhalf_2fshutdown"></a>

- `@noalloc fn write_half.shutdown(n: Net, mut w: WriteHalf of n) -> Result[(), NetError]`

<a id="def-core_401_2fwrite_5fhalf_2fwrite"></a>

- `@noalloc fn write_half.write(n: Net, mut w: WriteHalf of n, data: Bytes) -> Result[u64, NetError]`

<a id="def-core_401_2fwrite_5fhalf_2fwrite_5fall"></a>

- `@noalloc fn write_half.write_all(n: Net, mut w: WriteHalf of n, data: Bytes) -> Result[(), NetError]`


## x25519

<a id="def-std_401_2fx25519_2fSharedError"></a>

- `pub type x25519.SharedError = LowOrderPoint()`

<a id="def-std_401_2fx25519_2fSharedError_23LowOrderPoint"></a>

- `LowOrderPoint() -> x25519.SharedError`

Variant of x25519.SharedError.

<a id="def-std_401_2fx25519_2fpublic_5fkey"></a>

- `pub fn x25519.public_key(secret: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)

<a id="def-std_401_2fx25519_2fshared_5fsecret"></a>

- `pub fn x25519.shared_secret(secret: Bytes, peer: Bytes) -> Result[unique Bytes, SharedError] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/x25519/SharedError`](#def-std_401_2fx25519_2fSharedError)


## x509

<a id="def-std_401_2fx509_2fCertificate"></a>

- `@serializable pub type x509.Certificate { subject: String, names: List[String], uris: List[String], }`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fx509_2fX509Error"></a>

- `@serializable pub type x509.X509Error = Malformed(u64)`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fx509_2fX509Error_23Malformed"></a>

- `Malformed(u64) -> x509.X509Error`

Variant of x509.X509Error.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fx509_2fcertificates"></a>

- `pub fn x509.certificates(chain: Bytes) -> Result[List[Bytes], X509Error] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/x509/X509Error`](#def-std_401_2fx509_2fX509Error)

<a id="def-std_401_2fx509_2fdomain_5fname_5fvalid"></a>

- `pub fn x509.domain_name_valid(name: String, constraint: bool) -> bool uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fx509_2fparse"></a>

- `pub fn x509.parse(der: Bytes) -> Result[Certificate, X509Error] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/x509/Certificate`](#def-std_401_2fx509_2fCertificate), [`std@1/x509/X509Error`](#def-std_401_2fx509_2fX509Error)


## xchacha20poly1305

<a id="def-std_401_2fxchacha20poly1305_2fopen"></a>

- `pub fn xchacha20poly1305.open(key: Bytes, nonce: Bytes, sealed: Bytes, data: Bytes) -> Result[ unique Bytes, std@1/aead.OpenError ] uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/aead/OpenError`](#def-std_401_2faead_2fOpenError)

<a id="def-std_401_2fxchacha20poly1305_2fseal"></a>

- `pub fn xchacha20poly1305.seal(key: Bytes, nonce: Bytes, plaintext: Bytes, data: Bytes) -> unique Bytes uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes)


## xxhash64

<a id="def-std_401_2fxxhash64_2fState"></a>

- `pub opaque type xxhash64.State: plain + inert uses {}`

<a id="def-std_401_2fxxhash64_2fdigest"></a>

- `@noalloc pub fn xxhash64.digest(data: Bytes) -> u64 uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fxxhash64_2fdigest_5fhex"></a>

- `pub fn xxhash64.digest_hex(data: Bytes) -> unique String uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fxxhash64_2ffinish"></a>

- `@noalloc pub fn xxhash64.finish(take state: State) -> u64 uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/xxhash64/State`](#def-std_401_2fxxhash64_2fState)

<a id="def-std_401_2fxxhash64_2fnew"></a>

- `@noalloc pub fn xxhash64.new() -> State uses {}`

References: [`std@1/xxhash64/State`](#def-std_401_2fxxhash64_2fState)

<a id="def-std_401_2fxxhash64_2fnew_5fwith_5fseed"></a>

- `@noalloc pub fn xxhash64.new_with_seed(seed: u64) -> State uses {}`

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/xxhash64/State`](#def-std_401_2fxxhash64_2fState)

<a id="def-std_401_2fxxhash64_2fupdate"></a>

- `@noalloc pub fn xxhash64.update(mut state: State, data: Bytes) -> () uses {}`

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`std@1/xxhash64/State`](#def-std_401_2fxxhash64_2fState)

<a id="def-std_401_2fxxhash64_2fupdate_5flist"></a>

- `@noalloc pub fn xxhash64.update_list(mut state: State, data: List[u8], start: u64) -> () uses {}`

Absorb the byte-list suffix starting at start, without copying its storage.
A start at or beyond the end is an empty update, as crc32.update_list.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/xxhash64/State`](#def-std_401_2fxxhash64_2fState)


## yaml

<a id="def-std_401_2fyaml_2fConfig"></a>

- `@serializable pub type yaml.Config { merge_keys: bool, }`

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fyaml_2fDocument"></a>

- `pub opaque type yaml.Document: copyable + inert + noalloc_drop uses {}`

<a id="def-std_401_2fyaml_2fMAX_5fALIAS_5fNODES"></a>

- `pub const yaml.MAX_ALIAS_NODES: u64`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fyaml_2fMAX_5fNESTING"></a>

- `pub const yaml.MAX_NESTING: u64`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fyaml_2fPosition"></a>

- `@serializable pub type yaml.Position { line: u64, column: u64, }`

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fyaml_2fSegment"></a>

- `@serializable pub type yaml.Segment = Key(String) | Index(u64)`

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fyaml_2fSegment_23Index"></a>

- `Index(u64) -> yaml.Segment`

Variant of yaml.Segment.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fyaml_2fSegment_23Key"></a>

- `Key(String) -> yaml.Segment`

Variant of yaml.Segment.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fyaml_2fValue"></a>

- `@serializable pub type yaml.Value = Null | Bool(bool) | Integer(i64) | Float(f64) | Text(String) | Array( List[Value] ) | Object(Map[String, Value]) | Tagged(tag: String, value: box Value)`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString), [`core@1/bool`](#def-core_401_2fbool), [`core@1/f64`](#def-core_401_2ff64), [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fyaml_2fValue_23Array"></a>

- `Array( List[Value] ) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/List`](#def-core_401_2fList)

<a id="def-std_401_2fyaml_2fValue_23Bool"></a>

- `Bool(bool) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/bool`](#def-core_401_2fbool)

<a id="def-std_401_2fyaml_2fValue_23Float"></a>

- `Float(f64) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/f64`](#def-core_401_2ff64)

<a id="def-std_401_2fyaml_2fValue_23Integer"></a>

- `Integer(i64) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/i64`](#def-core_401_2fi64)

<a id="def-std_401_2fyaml_2fValue_23Null"></a>

- `Null -> yaml.Value`

Variant of yaml.Value.

<a id="def-std_401_2fyaml_2fValue_23Object"></a>

- `Object(Map[String, Value]) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/Map`](#def-core_401_2fMap), [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fyaml_2fValue_23Tagged"></a>

- `Tagged(tag: String, value: box Value) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fyaml_2fValue_23Text"></a>

- `Text(String) -> yaml.Value`

Variant of yaml.Value.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fyaml_2fYamlError"></a>

- `@serializable pub type yaml.YamlError = Syntax(Position) | DuplicateKey(Position) | Nesting(Position) | Aliases( Position ) | Tag(Position) | ComplexKey(Position) | Documents(Position) | Merge(Position) | InvalidValue | NotFound | NotCollection`

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23Aliases"></a>

- `Aliases( Position ) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23ComplexKey"></a>

- `ComplexKey(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23Documents"></a>

- `Documents(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23DuplicateKey"></a>

- `DuplicateKey(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23InvalidValue"></a>

- `InvalidValue -> yaml.YamlError`

Variant of yaml.YamlError.

<a id="def-std_401_2fyaml_2fYamlError_23Merge"></a>

- `Merge(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23Nesting"></a>

- `Nesting(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23NotCollection"></a>

- `NotCollection -> yaml.YamlError`

Variant of yaml.YamlError.

<a id="def-std_401_2fyaml_2fYamlError_23NotFound"></a>

- `NotFound -> yaml.YamlError`

Variant of yaml.YamlError.

<a id="def-std_401_2fyaml_2fYamlError_23Syntax"></a>

- `Syntax(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fYamlError_23Tag"></a>

- `Tag(Position) -> yaml.YamlError`

Variant of yaml.YamlError.

References: [`std@1/yaml/Position`](#def-std_401_2fyaml_2fPosition)

<a id="def-std_401_2fyaml_2fdefault_5fconfig"></a>

- `pub fn yaml.default_config() -> Config uses {}`

References: [`std@1/yaml/Config`](#def-std_401_2fyaml_2fConfig)

<a id="def-std_401_2fyaml_2fdocument"></a>

- `pub fn yaml.document(take text: String) -> Result[Document, YamlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fdocument_5fwith"></a>

- `pub fn yaml.document_with(take text: String, config: Config) -> Result[Document, YamlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Config`](#def-std_401_2fyaml_2fConfig), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fget"></a>

- `pub fn yaml.get(doc: Document, path: List[Segment]) -> Option[Value] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Option`](#def-core_401_2fOption), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/Segment`](#def-std_401_2fyaml_2fSegment), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue)

<a id="def-std_401_2fyaml_2fparse"></a>

- `pub fn yaml.parse(take text: String) -> Result[Value, YamlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fparse_5fall"></a>

- `pub fn yaml.parse_all(take text: String) -> Result[List[Value], YamlError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fparse_5fall_5fwith"></a>

- `pub fn yaml.parse_all_with(take text: String, config: Config) -> Result[ List[Value], YamlError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Config`](#def-std_401_2fyaml_2fConfig), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fparse_5fwith"></a>

- `pub fn yaml.parse_with(take text: String, config: Config) -> Result[Value, YamlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Config`](#def-std_401_2fyaml_2fConfig), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fremove"></a>

- `pub fn yaml.remove(mut doc: Document, path: List[Segment]) -> Result[(), YamlError] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/Segment`](#def-std_401_2fyaml_2fSegment), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2froot"></a>

- `pub fn yaml.root(doc: Document) -> Value uses {}`

References: [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue)

<a id="def-std_401_2fyaml_2fset"></a>

- `pub fn yaml.set(mut doc: Document, path: List[Segment], value: Value) -> Result[ (), YamlError ] uses {}`

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument), [`std@1/yaml/Segment`](#def-std_401_2fyaml_2fSegment), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)

<a id="def-std_401_2fyaml_2fsource"></a>

- `pub fn yaml.source(doc: Document) -> String in doc uses {}`

References: [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Document`](#def-std_401_2fyaml_2fDocument)

<a id="def-std_401_2fyaml_2fstringify"></a>

- `pub fn yaml.stringify(value: Value) -> Result[unique String, YamlError] uses {}`

References: [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/yaml/Value`](#def-std_401_2fyaml_2fValue), [`std@1/yaml/YamlError`](#def-std_401_2fyaml_2fYamlError)


## zip

<a id="def-std_401_2fzip_2fEntry"></a>

- `@serializable pub type zip.Entry { name: String, comment: String, method: Method, flags: u64, creator: u64, external: u64, modified: Option[i64], dos_time: u64, crc: u32, compressed: u64, size: u64, offset: u64, }`

One central directory entry. `offset` is the entry's local header offset
within the data (including any prefix), `modified` the Unix time of an
extended timestamp extra field, and `dos_time` the MS-DOS date (high 16
bits) and time (low 16 bits). `creator` and `external` are the version
made by and external attributes (Unix modes in the high 16 bits when the
creator's high byte is 3).

References: [`core@1/Option`](#def-core_401_2fOption), [`core@1/String`](#def-core_401_2fString), [`core@1/i64`](#def-core_401_2fi64), [`core@1/u32`](#def-core_401_2fu32), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/Method`](#def-std_401_2fzip_2fMethod)

<a id="def-std_401_2fzip_2fExtractError"></a>

- `@serializable pub type zip.ExtractError = Read(ZipError) | Insecure(String) | Unsupported(String) | File( FileError )`

Extraction refused an entry's name (`Insecure`) or kind (`Unsupported`),
the archive is malformed, or a file operation failed.

References: [`core@1/FileError`](#def-core_401_2fFileError), [`core@1/String`](#def-core_401_2fString), [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fExtractError_23File"></a>

- `File( FileError ) -> zip.ExtractError`

Variant of zip.ExtractError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2fzip_2fExtractError_23Insecure"></a>

- `Insecure(String) -> zip.ExtractError`

Variant of zip.ExtractError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fzip_2fExtractError_23Read"></a>

- `Read(ZipError) -> zip.ExtractError`

Variant of zip.ExtractError.

References: [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fExtractError_23Unsupported"></a>

- `Unsupported(String) -> zip.ExtractError`

Variant of zip.ExtractError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fzip_2fFileZipError"></a>

- `@serializable pub type zip.FileZipError = Storage(FileError) | Archive(ZipError)`

A file operation failed or the archive is malformed.

References: [`core@1/FileError`](#def-core_401_2fFileError), [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fFileZipError_23Archive"></a>

- `Archive(ZipError) -> zip.FileZipError`

Variant of zip.FileZipError.

References: [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fFileZipError_23Storage"></a>

- `Storage(FileError) -> zip.FileZipError`

Variant of zip.FileZipError.

References: [`core@1/FileError`](#def-core_401_2fFileError)

<a id="def-std_401_2fzip_2fMethod"></a>

- `@serializable pub type zip.Method = Stored | Deflate | Other(u64)`

An entry's compression method (APPNOTE 4.4.5).

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzip_2fMethod_23Deflate"></a>

- `Deflate -> zip.Method`

Variant of zip.Method.

<a id="def-std_401_2fzip_2fMethod_23Other"></a>

- `Other(u64) -> zip.Method`

Variant of zip.Method.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzip_2fMethod_23Stored"></a>

- `Stored -> zip.Method`

Variant of zip.Method.

<a id="def-std_401_2fzip_2fWriteError"></a>

- `@serializable pub type zip.WriteError = Field | Directory | Closed`

Writing errors: a name, comment or extra field too long for its 16-bit
length, data written to a directory entry, or a write after `close`.

<a id="def-std_401_2fzip_2fWriteError_23Closed"></a>

- `Closed -> zip.WriteError`

Variant of zip.WriteError.

<a id="def-std_401_2fzip_2fWriteError_23Directory"></a>

- `Directory -> zip.WriteError`

Variant of zip.WriteError.

<a id="def-std_401_2fzip_2fWriteError_23Field"></a>

- `Field -> zip.WriteError`

Variant of zip.WriteError.

<a id="def-std_401_2fzip_2fWriter"></a>

- `pub opaque type zip.Writer: copyable + inert + noalloc_drop uses {}`

Writing state: the central directory so far, the output offset and the
entry being written.

<a id="def-std_401_2fzip_2fZipError"></a>

- `@serializable pub type zip.ZipError = Format(u64) | Encoding(u64) | Algorithm(String) | Checksum(String) | TooLarge | Data( flate.FlateError )`

Malformed archive data: `Format(n)` at byte offset `n`, a name or
comment that is not UTF-8 (`Encoding(n)`, the offset of its directory
header), an unsupported method, data whose CRC-32 or size disagrees with
its entry, or more data than `limit` allows.

References: [`core@1/String`](#def-core_401_2fString), [`core@1/u64`](#def-core_401_2fu64), [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fzip_2fZipError_23Algorithm"></a>

- `Algorithm(String) -> zip.ZipError`

Variant of zip.ZipError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fzip_2fZipError_23Checksum"></a>

- `Checksum(String) -> zip.ZipError`

Variant of zip.ZipError.

References: [`core@1/String`](#def-core_401_2fString)

<a id="def-std_401_2fzip_2fZipError_23Data"></a>

- `Data( flate.FlateError ) -> zip.ZipError`

Variant of zip.ZipError.

References: [`std@1/flate/FlateError`](#def-std_401_2fflate_2fFlateError)

<a id="def-std_401_2fzip_2fZipError_23Encoding"></a>

- `Encoding(u64) -> zip.ZipError`

Variant of zip.ZipError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzip_2fZipError_23Format"></a>

- `Format(u64) -> zip.ZipError`

Variant of zip.ZipError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzip_2fZipError_23TooLarge"></a>

- `TooLarge -> zip.ZipError`

Variant of zip.ZipError.

<a id="def-std_401_2fzip_2farchive_5fcomment"></a>

- `pub fn zip.archive_comment(data: Bytes) -> Result[unique String, ZipError] uses {}`

The archive comment of the end-of-central-directory record.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fclose"></a>

- `@preserves_unique(out) pub fn zip.close(mut w: Writer, comment: String, mut out: List[u8]) -> Result[ (), WriteError ] uses {}`

Go Writer.Close: finishes the last entry and appends the central
directory, ZIP64 end records when an entry, the record count, the
directory's size or its offset needs them, and the end record with
`comment`. `offset` counts `out`'s bytes from the start of the archive,
so a writer that drains `out` between calls reports it with `flushed`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/String`](#def-core_401_2fString), [`core@1/u8`](#def-core_401_2fu8), [`std@1/zip/WriteError`](#def-std_401_2fzip_2fWriteError), [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)

<a id="def-std_401_2fzip_2fcreate"></a>

- `@preserves_unique(out) pub fn zip.create(mut w: Writer, h: Entry, level: u64, mut out: List[u8]) -> Result[ (), WriteError ] uses {}`

Go Writer.CreateHeader: finishes the previous entry and starts `h`, whose
data follows through `write`, compressed at `level` when `h.method` is
`Deflate`; a name ending in `/` is a stored directory with no data. The
CRC-32 and sizes follow the data in a data descriptor (flag 8). Other
methods are `Field`.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`core@1/u8`](#def-core_401_2fu8), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/WriteError`](#def-std_401_2fzip_2fWriteError), [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)

<a id="def-std_401_2fzip_2fcreate_5fraw"></a>

- `@preserves_unique(out) pub fn zip.create_raw(mut w: Writer, h: Entry, mut out: List[u8]) -> Result[(), WriteError] uses {}`

Go Writer.CreateRaw: starts `h` with its CRC-32 and sizes as given, its
data written through `write` as already compressed, with no data
descriptor; sizes of 4 GiB or more get a ZIP64 extra field.

References: [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/WriteError`](#def-std_401_2fzip_2fWriteError), [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)

<a id="def-std_401_2fzip_2fentries"></a>

- `pub fn zip.entries(data: Bytes) -> Result[List[Entry], ZipError] uses {}`

Go Reader.init: the entries of the archive in `data`, in central
directory order. Data before the archive (a prefix such as a
self-extracting stub) is allowed: offsets are relative to the end of the
central directory when its recorded offset does not hold a directory
header.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fentry"></a>

- `pub fn zip.entry(take name: String) -> Entry uses {}`

An entry to write: deflated, modified at Unix time 0, with no comment.

References: [`core@1/String`](#def-core_401_2fString), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry)

<a id="def-std_401_2fzip_2fextract"></a>

- `pub fn zip.extract(data: Bytes, into: Dir, limit: u64) -> Result[u64, ExtractError]`

Extracts every entry of the archive in `data` into `into` and returns how
many were written: directory entries (names ending in `/`) and files,
creating missing parents and replacing existing files. Names are refused
as for `tar.extract` (`Insecure`), as is a path through or onto an
existing symlink; Unix symlink, device, FIFO and socket entries are
`Unsupported`. Every entry's size counts against `limit` before it is
decoded, and decoding never produces more than the size, so at most
`limit` bytes are written. The first refusal stops extraction.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Dir`](#def-core_401_2fDir), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/ExtractError`](#def-std_401_2fzip_2fExtractError)

<a id="def-std_401_2fzip_2ffile_5fentries"></a>

- `pub fn zip.file_entries(dir: Dir, mut f: File of dir, size: u64) -> Result[ List[Entry], FileZipError ]`

`entries` of a zip file of `size` bytes, reading only its last 65 KiB
and its central directory.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/FileZipError`](#def-std_401_2fzip_2fFileZipError)

<a id="def-std_401_2fzip_2ffile_5fextract"></a>

- `pub fn zip.file_extract(from: Dir, mut f: File of from, size: u64, into: Dir, limit: u64) -> Result[ u64, ExtractError ]`

`extract` from a zip file of `size` bytes, holding one entry's data at a
time.

References: [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/ExtractError`](#def-std_401_2fzip_2fExtractError)

<a id="def-std_401_2fzip_2ffile_5fread"></a>

- `pub fn zip.file_read(dir: Dir, mut f: File of dir, e: Entry, limit: u64) -> Result[ unique Bytes, FileZipError ]`

`read` of an entry of a zip file, reading only that entry: its local
header, data and data descriptor.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Dir`](#def-core_401_2fDir), [`core@1/File`](#def-core_401_2fFile), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/FileZipError`](#def-std_401_2fzip_2fFileZipError)

<a id="def-std_401_2fzip_2fflushed"></a>

- `pub fn zip.flushed(mut w: Writer, count: u64) -> () uses {}`

Records that the caller drained `count` bytes of output, keeping the
offsets of later entries and the directory right.

References: [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)

<a id="def-std_401_2fzip_2fread"></a>

- `pub fn zip.read(data: Bytes, e: Entry, limit: u64) -> Result[unique Bytes, ZipError] uses {}`

Go File.Open read to the end: the entry's data, which must decode to
exactly its size with a matching CRC-32 (and, after a data descriptor,
the descriptor's CRC-32). An entry whose size exceeds `limit` is
`TooLarge` before any of it is decoded; a directory entry has no data.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zip/Entry`](#def-std_401_2fzip_2fEntry), [`std@1/zip/ZipError`](#def-std_401_2fzip_2fZipError)

<a id="def-std_401_2fzip_2fwrite"></a>

- `@preserves_unique(out) pub fn zip.write(mut w: Writer, data: Bytes, mut out: List[u8]) -> Result[(), WriteError] uses {}`

Appends data to the entry being written.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/List`](#def-core_401_2fList), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u8`](#def-core_401_2fu8), [`std@1/zip/WriteError`](#def-std_401_2fzip_2fWriteError), [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)

<a id="def-std_401_2fzip_2fwriter"></a>

- `pub fn zip.writer() -> unique Writer uses {}`

References: [`std@1/zip/Writer`](#def-std_401_2fzip_2fWriter)


## zstd

<a id="def-std_401_2fzstd_2fDictionary"></a>

- `pub opaque type zstd.Dictionary: copyable + inert + noalloc_drop uses {}`

A validated RFC 8878 dictionary or raw-content prefix. Reusable across frames;
decoding borrows it and resets its entropy tables and offsets at each frame.

<a id="def-std_401_2fzstd_2fZstdError"></a>

- `pub type zstd.ZstdError = Truncated | BadMagic | ReservedBit | Corrupt | BlockOversize | Checksum | ContentSize | WindowTooLarge( u64 ) | OutputTooLarge | DictionaryRequired(u64) | DictionaryMismatch | InvalidDictionary`

Malformed data and caller-selected resource bounds are ordinary errors.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzstd_2fZstdError_23BadMagic"></a>

- `BadMagic -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23BlockOversize"></a>

- `BlockOversize -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23Checksum"></a>

- `Checksum -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23ContentSize"></a>

- `ContentSize -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23Corrupt"></a>

- `Corrupt -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23DictionaryMismatch"></a>

- `DictionaryMismatch -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23DictionaryRequired"></a>

- `DictionaryRequired(u64) -> zstd.ZstdError`

Variant of zstd.ZstdError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzstd_2fZstdError_23InvalidDictionary"></a>

- `InvalidDictionary -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23OutputTooLarge"></a>

- `OutputTooLarge -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23ReservedBit"></a>

- `ReservedBit -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23Truncated"></a>

- `Truncated -> zstd.ZstdError`

Variant of zstd.ZstdError.

<a id="def-std_401_2fzstd_2fZstdError_23WindowTooLarge"></a>

- `WindowTooLarge( u64 ) -> zstd.ZstdError`

Variant of zstd.ZstdError.

References: [`core@1/u64`](#def-core_401_2fu64)

<a id="def-std_401_2fzstd_2fdecode"></a>

- `pub fn zstd.decode(data: Bytes, max_output: u64, max_window: u64) -> Result[ unique Bytes, ZstdError ] uses {}`

Decode concatenated Zstandard and skippable frames. Checksums are always
verified. max_output bounds the total returned bytes; max_window bounds each
frame's declared history requirement, including single-segment content size.
Rejects oversize headers before allocating output or entropy tables for them.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zstd/ZstdError`](#def-std_401_2fzstd_2fZstdError)

<a id="def-std_401_2fzstd_2fdecode_5fwith_5fdictionary"></a>

- `pub fn zstd.decode_with_dictionary( data: Bytes, dict: Dictionary, max_output: u64, max_window: u64 ) -> Result[unique Bytes, ZstdError] uses {}`

Decode using a dictionary, reset independently for every frame. A nonzero
frame dictionary ID must match; ID zero permits caller-selected raw content.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`core@1/u64`](#def-core_401_2fu64), [`std@1/zstd/Dictionary`](#def-std_401_2fzstd_2fDictionary), [`std@1/zstd/ZstdError`](#def-std_401_2fzstd_2fZstdError)

<a id="def-std_401_2fzstd_2fdictionary"></a>

- `pub fn zstd.dictionary(data: Bytes) -> Result[Dictionary, ZstdError] in data uses {}`

Parse a formatted dictionary (magic 0xec30a437), or use other bytes as a raw
content prefix. Formatted dictionary tables and repeat offsets are validated.
Dictionary input is borrowed; its content bytes remain shared with the input.

References: [`core@1/Bytes`](#def-core_401_2fBytes), [`core@1/Result`](#def-core_401_2fResult), [`std@1/zstd/Dictionary`](#def-std_401_2fzstd_2fDictionary), [`std@1/zstd/ZstdError`](#def-std_401_2fzstd_2fZstdError)
