//! Small declaration-data reader, intentionally independent of runtime dispatch.
use super::*;
use hirsel_syntax::Visibility;
use hirsel_type_analysis::cleanup::{
    CleanupLeaf, cleanup_summary, declared_cleanup, parameter_cleanup,
};

pub(crate) fn registry(types: &Arc<TypeInterner>) -> BTreeMap<DefId, SignatureRef> {
    let mut out = BTreeMap::new();
    // DROP-4 declares this intrinsic outside the core library ledger.
    let mut forget = Builder::new(types, "T");
    let signature = forget.function("take x: T", "()", true);
    publish(
        types,
        forget.signature("forget", SignatureKind::Function(signature), false),
        &mut out,
    );
    for name in [
        "Io",
        "Args",
        "Env",
        "Unsafe",
        "Dir",
        "Stdin",
        "Stderr",
        "Stdout",
        "Signals",
        "MonotonicClock",
        "WallClock",
        "Random",
        "Telemetry",
        "Tasks",
        "Net",
        "Proc",
        "Runner",
        "Scope",
        "Arena",
        "Test",
    ] {
        let scoped = matches!(name, "Scope" | "Arena" | "Test");
        let b = Builder::new(types, "").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.capability = Some(if scoped {
            CapabilityKind::Scoped
        } else {
            CapabilityKind::Global
        });
        t.bounds = if scoped {
            vec![Bound::NoallocDrop]
        } else {
            vec![
                Bound::Plain,
                Bound::Copyable,
                Bound::Sendable,
                Bound::NoallocDrop,
            ]
        };
        t.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                name,
            ))]
            .into(),
        );
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), true),
            &mut out,
        );
    }
    // These inert, plain views/positions have intrinsic loop/brand rules (docs/spec/core-library.md).
    for name in ["Instant", "Path", "StrPos", "ScalarView", "PositionView"] {
        let b = Builder::new(types, "").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.bounds = vec![Bound::Copyable, Bound::Inert, Bound::NoallocDrop];
        if matches!(name, "Instant" | "Path" | "StrPos") {
            t.bounds.push(Bound::Plain);
        }
        if matches!(name, "Path" | "StrPos") {
            t.bounds.extend([Bound::Data, Bound::Key, Bound::Sendable]);
        }
        if name == "Instant" {
            t.bounds.push(Bound::Sendable);
        }
        serializable_core(name, &mut t);
        if let Some(protocol) = match name {
            "ScalarView" => Some("UnicodeScalars"),
            "PositionView" => Some("ScalarPositions"),
            _ => None,
        } {
            t.loop_source = Some(LoopSourceDeclaration {
                open: hirsel_syntax::DefPath::core(&format!("$intrinsic.{protocol}_open")),
                next: hirsel_syntax::DefPath::core(&format!("$intrinsic.{protocol}_next")),
                option: hirsel_syntax::DefPath::core("Option"),
            });
        }
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), true),
            &mut out,
        );
    }
    // Typed generation: a generated body for the function type `F`. Like a TypeShape it is
    // compile-time only and not data; it holds text, never authority.
    let b = Builder::new(types, "F").with_declarations(&out);
    let mut t = type_definition(b.generics.clone(), None);
    t.bounds = vec![Bound::Copyable, Bound::Inert, Bound::NoallocDrop];
    publish(
        types,
        b.signature("Body", SignatureKind::Type(Box::new(t)), true),
        &mut out,
    );
    // RUN-3: a reference to one top-level definition of callable type `F`. It names the
    // definition, never an implementation, so it holds no authority and is not data.
    let b = Builder::new(types, "F").with_declarations(&out);
    let mut t = type_definition(b.generics.clone(), None);
    t.bounds = vec![
        Bound::Plain,
        Bound::Copyable,
        Bound::Inert,
        Bound::Sendable,
        Bound::NoallocDrop,
    ];
    publish(
        types,
        b.signature("Entry", SignatureKind::Type(Box::new(t)), true),
        &mut out,
    );
    for (name, generic, variants) in [
        ("Result", "T,E", "Ok(T)|Error(E)"),
        (
            "DecodeError",
            "",
            "Version()|SchemaMismatch()|Truncated()|Trailing()|Scalar()|Utf8()|Tag()|Length()|Duplicate()|Order()|Invalid()",
        ),
        ("Option", "T", "None()|Some(T)"),
        ("CheckRule", "", "Arithmetic()|TextInvariant()|Bounds()"),
        (
            "CancelCause",
            "",
            "Requested()|Parent()|ChildFailed()|DeadlineExceeded()",
        ),
        ("Bounded", "R", "Finished(R)|Expired(R)"),
        ("CancelError", "", "Canceled(CancelCause)"),
        ("ChannelError", "", "Closed()|Canceled(CancelCause)"),
        (
            "CpuPoolError",
            "",
            "InvalidWidth()|NotGranted()|Busy()|WidthUnavailable()|NestedPool()|BatchTooLarge()|WorkerStartFailed()|CapacityLost()|Canceled(CancelCause)|NotSupported()|ResourceLimit()|MapperNotCertified()",
        ),
        (
            "FileError",
            "",
            "FileFault(u32)|Canceled(CancelCause)|NotGranted()|NotFound()|PermissionDenied()|AlreadyExists()|IsDirectory()|NotDirectory()|DirectoryNotEmpty()|CrossDevice()|Truncated()",
        ),
        ("FileKind", "", "Regular()|Directory()|Symlink()|Other()"),
        ("Durability", "", "Durable()|Volatile()"),
        (
            "WatchEvent",
            "",
            "Created(Path)|Modified(Path)|Removed(Path)|Renamed(Path,Path)|Overflow()",
        ),
        ("IpAddr", "", "V4(Inline[u8, 4])|V6(Inline[u8, 16])"),
        (
            "NetOperation",
            "",
            "Connect()|ConnectTls()|Listen()|UdpBind()|UdpPeer()|ListenTls()|UnixConnect()|UnixListen()|UnixListenRemoveStale()",
        ),
        ("NetTimeout", "", "Unbounded()|Nanos(u64)"),
        ("Shutdown", "", "Read()|Write()|Both()"),
        ("TlsRoots", "", "WebPki()|Pem(Bytes)"),
        (
            "TlsClientAuth",
            "",
            "Anonymous()|Request(Bytes)|Require(Bytes)",
        ),
        (
            "TlsFailure",
            "",
            "UnknownIssuer()|Expired()|NotYetValid()|NameMismatch()|Revoked()|BadCertificate()|AlertReceived(u8)|HandshakeFailure()|PeerMisbehaved()|Truncated()|CertificateRequired()",
        ),
        (
            "NetError",
            "",
            "NotGranted()|Canceled(CancelCause)|InvalidInput()|UnknownHost()|ResolverFailure()|ConnectionRefused()|ConnectionReset()|ConnectionAborted()|NotConnected()|AddrInUse()|AddrNotAvailable()|HostUnreachable()|NetworkUnreachable()|NetworkDown()|BrokenPipe()|TimedOut()|Tls(TlsFailure)|Fault(u32)",
        ),
        (
            "PeerTls",
            "",
            "Plain()|Certificate(List[String])|Fails(TlsFailure)|Presents(List[String],Bytes)|Serves(TlsServer)|Offers(List[String],List[String])",
        ),
        (
            "NetConnection",
            "",
            "Refused()|Unanswered()|Open(List[NetStep])",
        ),
        (
            "NetStep",
            "",
            "Send(Bytes)|Expect(Bytes)|Stall()|Close()|Reset()|Truncate()",
        ),
        ("GrantError", "", "NotGranted()"),
        ("GrantAccess", "", "Read()|ReadWrite()"),
        (
            "NotRunReason",
            "",
            "Denied()|VersionMismatch()|Unreachable()|Overloaded()|Rejected()",
        ),
        ("InDoubtReason", "", "Lost()|TimedOut()|Abandoned()"),
        ("ExitStatus", "", "Exited(i32)|Signaled(i32)"),
        ("Signal", "", "Interrupt()|Terminate()|Kill()"),
        (
            "ChildStep",
            "",
            "Print(Bytes)|PrintErr(Bytes)|Expect(Bytes)|Exit(ExitStatus)",
        ),
        (
            "ProcessError",
            "",
            "NotGranted()|InvalidInput()|NotFound()|PermissionDenied()|OutputLimit()|Fault(u32)|TimedOut()|Canceled(CancelCause)|InputClosed()",
        ),
        (
            "InputError",
            "",
            "InputFault(u32)|Canceled(CancelCause)|InvalidInput()|InvalidUtf8()",
        ),
        ("EnvError", "", "NotGranted()|InvalidUtf8()"),
        ("OutputError", "", "OutputFault(u32)|Canceled(CancelCause)"),
        ("ClockError", "", "Unavailable()|OutOfRange()"),
        ("RandomError", "", "Unavailable()"),
        ("TelemetryError", "", "TooLarge()|InvalidParent()"),
        ("WaitError", "", "Clock(ClockError)|Canceled(CancelCause)"),
        ("SignalKind", "", "Interrupt()|Terminate()|Hangup()"),
        (
            "SignalError",
            "",
            "Canceled(CancelCause)|NotGranted()|InvalidInput()",
        ),
        ("ArgError", "", "MissingArgument()|InvalidUtf8()"),
        ("FloatIntError", "", "NotFinite()|OutOfRange()"),
        ("Base64Alphabet", "", "Standard()|UrlSafe()"),
        ("Base64Padding", "", "Required()|Omitted()"),
        ("OutOfSpace", "", "OutOfSpace()"),
        (
            "CliError",
            "",
            "Usage(ArgError)|Files(FileError)|Output(OutputError)",
        ),
        (
            "FailureKind",
            "",
            "Check()|Contract()|Depth()|OutOfSpace()|DeadMutex()",
        ),
        ("ShapeVisibility", "", "Private()|Public()|Opaque()"),
        (
            "ShapeBody",
            "",
            "Record(List[FieldShape])|Choice(List[VariantShape])",
        ),
    ] {
        variant(types, name, generic, variants, &mut out);
    }
    for (name, generic, fields) in [
        ("Order", "T", "compare: fn(T,T) -> i32 uses {}"),
        ("Eq", "T", "equal: fn(T,T) -> bool uses {}"),
        ("Hasher", "T", "hash: fn(T,u64) -> u64 uses {}"),
        ("SourceSite", "", "definition: String,line: u64,column: u64"),
        (
            "Failure",
            "",
            "kind: FailureKind,rule: String,site: SourceSite,spawn: List[u64],cause: Option[box Failure]",
        ),
        (
            "Metadata",
            "",
            "kind: FileKind,size: u64,modified_unix_ns: Option[i128],permissions: u32",
        ),
        ("DirEntry", "", "path: Path,kind: FileKind"),
        ("SocketAddr", "", "ip: IpAddr,port: u16"),
        ("TlsIdentity", "", "chain: Bytes,key: Bytes"),
        (
            "TlsServer",
            "",
            "identities: List[TlsIdentity],clients: TlsClientAuth,alpn: List[String]",
        ),
        (
            "TlsClient",
            "",
            "roots: TlsRoots,identity: Option[TlsIdentity],alpn: List[String]",
        ),
        (
            "NetRule",
            "",
            "address: String,port: u16,operation: NetOperation",
        ),
        (
            "NetPeer",
            "",
            "address: String,port: u16,resolves_to: List[IpAddr],tls: PeerTls,connections: List[NetConnection]",
        ),
        (
            "ProcessOutput",
            "",
            "status: ExitStatus,stdout: Bytes,stderr: Bytes",
        ),
        (
            "PipelineOutput",
            "",
            "statuses: List[ExitStatus],stdout: Bytes,stderr: Bytes",
        ),
        (
            "GroupOutput",
            "",
            "statuses: List[ExitStatus],stderr: List[Bytes]",
        ),
        (
            "ProcessScript",
            "",
            "program: String,args: List[String],outcome: Result[ProcessOutput,ProcessError]",
        ),
        ("ProcessRule", "", "program: String,env: List[String]"),
        ("DirGrant", "", "path: String,access: GrantAccess"),
        (
            "GrantSet",
            "",
            "stdout: bool,stderr: bool,tasks: bool,monotonic_clock: bool,wall_clock: bool,random: bool,dirs: List[DirGrant],net: List[NetRule],process: List[ProcessRule]",
        ),
        (
            "ChildScript",
            "",
            "program: String,args: List[String],steps: List[ChildStep]",
        ),
        (
            "Deadline",
            "",
            "clock: MonotonicClock,at: Instant of .clock",
        ),
        (
            "Command",
            "",
            "program: Path,args: List[String],dir: Option[Dir],env: Option[List[(String, String)]],own_group: bool",
        ),
        (
            "UdpReceived",
            "",
            "count: u64,peer: SocketAddr,truncated: bool",
        ),
        (
            "NetClient",
            "",
            "from: SocketAddr,tls: PeerTls,steps: List[NetStep]",
        ),
        ("FieldShape", "", "name: String,ty: String"),
        ("PayloadShape", "", "label: Option[String],ty: String"),
        (
            "VariantShape",
            "",
            "name: String,payload: List[PayloadShape]",
        ),
        (
            "ShapeInfo",
            "",
            "name: String,visibility: ShapeVisibility,body: ShapeBody",
        ),
        (
            "NetListener",
            "",
            "address: String,port: u16,clients: List[NetClient]",
        ),
    ] {
        let mut b = Builder::new(types, generic)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let repr = b.record(fields);
        let mut t = type_definition(b.generics.clone(), Some(repr));
        if matches!(
            name,
            "FieldShape" | "PayloadShape" | "VariantShape" | "ShapeInfo"
        ) {
            t.bounds = shape_data_bounds();
        }
        if name == "Deadline" {
            // A deadline holds its clock, so it carries that clock's authority.
            t.bounds = vec![
                Bound::Plain,
                Bound::Copyable,
                Bound::Sendable,
                Bound::NoallocDrop,
            ];
            t.authority = AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                    "MonotonicClock",
                ))]
                .into(),
            );
        }
        if name == "Command" {
            // A command names its working directory, so it carries Dir authority.
            t.authority = AuthoritySummary::Known(
                vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                    "Dir",
                ))]
                .into(),
            );
        }
        serializable_core(name, &mut t);
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), false),
            &mut out,
        );
    }
    // RUN-6: a runner call's outcome carries the callee's Failure.
    variant(
        types,
        "RunError",
        "E",
        "NotRun(NotRunReason)|InDoubt(InDoubtReason)|Failed(E)|Faulted(Failure)|Canceled(CancelCause)",
        &mut out,
    );
    // RUN-13: the frames a caller's remote.tls@1 provider and a runner host exchange.
    variant(types, "CallForm", "", "Compute()|Call()", &mut out);
    variant(
        types,
        "CallEnd",
        "",
        "Returned(Bytes)|Failed(Bytes)|Faulted(Bytes)|NotRun(NotRunReason)|InDoubt(InDoubtReason)|Canceled(CancelCause)",
        &mut out,
    );
    for (name, fields) in [
        ("SessionOpen", "stream: Bytes,instance: Bytes,epoch: u64"),
        (
            "SessionOpened",
            "epoch: u64,incarnation: Bytes,applied: u64,reconciles: bool",
        ),
        ("CallRef", "epoch: u64,position: u64,key: Bytes"),
        (
            "RemoteCall",
            "epoch: u64,position: u64,ordinal: u64,key: Bytes,form: CallForm,path: String,fingerprint: Bytes,code: Bytes,grants: GrantSet,definitions: Option[List[String]],argument: Bytes,acknowledged: u64",
        ),
        (
            "RemoteOutcome",
            "epoch: u64,position: u64,key: Bytes,digest: Bytes,end: CallEnd,callee: Option[Bytes],missing: List[Bytes]",
        ),
    ] {
        let mut b = Builder::new(types, "").with_declarations(&out);
        let repr = b.record(fields);
        let mut t = type_definition(vec![], Some(repr));
        serializable_core(name, &mut t);
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), false),
            &mut out,
        );
    }
    variant(
        types,
        "RunFrame",
        "",
        "Open(SessionOpen)|Opened(SessionOpened)|Refused(NotRunReason)|Call(RemoteCall)|Outcome(RemoteOutcome)|Cancel(CallRef,CancelCause)|Acknowledged(CallRef)|Status(CallRef)|Running(CallRef)|Unknown(CallRef)|Close(u64,u64)",
        &mut out,
    );
    for (name, fields) in [
        ("File", ""),
        ("AtomicFile", "dir: Dir,tmp: Path,dst: Path"),
        ("Mapped", ""),
        ("Watcher", ""),
    ] {
        let mut b = Builder::new(types, "").with_declarations(&out);
        let repr = (!fields.is_empty()).then(|| b.record(fields));
        let mut t = type_definition(vec![], repr);
        t.resource = true;
        t.bounds = vec![Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                "Dir",
            ))]
            .into(),
        );
        t.drop = Some(hirsel_syntax::DefPath::core(name));
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), fields.is_empty()),
            &mut out,
        );
    }
    // A live span retains its Telemetry; its noalloc drop records an abandonment.
    {
        let b = Builder::new(types, "").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.resource = true;
        t.bounds = vec![Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                "Telemetry",
            ))]
            .into(),
        );
        t.drop = Some(hirsel_syntax::DefPath::core("Span"));
        publish(
            types,
            b.signature("Span", SignatureKind::Type(Box::new(t)), true),
            &mut out,
        );
    }
    // Core library network and process resources have their capability's authority and a
    // noalloc close finalizer.
    for (name, capability) in [
        ("Stream", "Net"),
        ("ReadHalf", "Net"),
        ("WriteHalf", "Net"),
        ("Listener", "Net"),
        ("UdpSocket", "Net"),
        ("Child", "Proc"),
        ("ChildGroup", "Proc"),
    ] {
        let b = Builder::new(types, "").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.resource = true;
        t.bounds = vec![Bound::Sendable, Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority = AuthoritySummary::Known(
            vec![AuthorityTerm::Capability(hirsel_syntax::DefPath::core(
                capability,
            ))]
            .into(),
        );
        t.drop = Some(hirsel_syntax::DefPath::core(name));
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), true),
            &mut out,
        );
    }
    // A scope's set of task handles (CONC-2): a noncopyable run-local resource holding its
    // members' await-or-detach obligations. Its drop releases the set; its members stay
    // children of their scope, joined at its exit.
    {
        let b = Builder::new(types, "R").with_declarations(&out);
        let mut t = type_definition(b.generics.clone(), None);
        t.resource = true;
        t.bounds = vec![Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority = AuthoritySummary::Known(
            vec![
                AuthorityTerm::Capability(hirsel_syntax::DefPath::core("Scope")),
                AuthorityTerm::TypeParameter(TypeParameterRef::bound(TypeParamId(0))),
            ]
            .into(),
        );
        t.drop = Some(hirsel_syntax::DefPath::core("TaskSet"));
        publish(
            types,
            b.signature("TaskSet", SignatureKind::Type(Box::new(t)), true),
            &mut out,
        );
    }
    for (name, generic) in [
        ("Array", "T"),
        ("List", "T"),
        ("Map", "K: key,V"),
        ("Set", "T: key"),
        ("SortedMap", "K,V"),
        ("Inline", "T,n: size"),
        ("Simd", "A,n: size"),
        ("Packed", "T"),
        ("Shared", "T"),
        ("Slots", "T"),
        ("SlotId", "T"),
        ("Region", "T"),
        ("Iter", "T"),
        ("Task", "T"),
        ("Sender", "T"),
        ("Receiver", "T"),
        ("Mutex", "T"),
        ("Atomic", "A"),
        ("Ptr", "T"),
    ] {
        let mut b = Builder::new(types, generic)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let text = format!("{name}[{}]", b.names.join(","));
        let repr = b.ty(&text, PassingMode::Take);
        let mut t = type_definition(b.generics.clone(), Some(repr));
        // MEM-4: the opaque owner has mechanical cleanup, not a user finalizer.
        // The builtin property rule still checks its complete root modes.
        t.resource = name == "Region";
        // Authority is declared by the constructor, never recovered from a body.
        t.authority = match name {
            "Task" | "Mutex" | "Atomic" | "Sender" | "Receiver" => {
                let TypeKind::Builtin { kind, .. } = &b.types.get(repr).unwrap().kind else {
                    unreachable!()
                };
                builtin_held_authority(*kind, &super::nominal_roles()).join(
                    AuthoritySummary::Known(
                        vec![AuthorityTerm::TypeParameter(TypeParameterRef::bound(
                            TypeParamId(0),
                        ))]
                        .into(),
                    ),
                )
            }
            "Iter" => AuthoritySummary::Unknown,
            "SlotId" | "Ptr" => AuthoritySummary::inert(),
            _ => AuthoritySummary::Known(
                b.generics
                    .iter()
                    .enumerate()
                    .filter_map(|(i, g)| {
                        matches!(g, GenericParameter::Type { .. }).then_some(
                            AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                                depth: 0,
                                parameter: TypeParamId(i as u32),
                            }),
                        )
                    })
                    .collect(),
            ),
        };
        publish(
            types,
            b.signature(name, SignatureKind::Type(Box::new(t)), name == "Region"),
            &mut out,
        );
    }
    for name in [
        "bool", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
        "char", "String", "Bytes",
    ] {
        let mut b = Builder::new(types, "").with_declarations(&out);
        let repr = b.ty(name, PassingMode::Take);
        let mut t = type_definition(vec![], Some(repr));
        t.bounds = vec![
            Bound::Copyable,
            Bound::Data,
            Bound::Inert,
            Bound::NoallocDrop,
        ];
        if !matches!(name, "String" | "Bytes") {
            t.bounds.extend([Bound::Plain, Bound::Sendable]);
        }
        if !matches!(name, "f32" | "f64") {
            t.bounds.push(Bound::Key);
        }
        let mut s = b.signature(name, SignatureKind::Type(Box::new(t)), false);
        s.metadata.alias = true;
        publish(types, s, &mut out);
        if name.starts_with(['u', 'i']) {
            let mut b = Builder::new(types, "").with_declarations(&out);
            let ty = b.ty(&format!("Order[{name}]"), PassingMode::Take);
            publish(
                types,
                b.signature(
                    &format!("{name}.order"),
                    SignatureKind::Const {
                        ty,
                        generics: Default::default(),
                    },
                    false,
                ),
                &mut out,
            );
        }
    }
    for signature in super::dictionaries::signatures(types)
        .into_iter()
        .chain(super::loops::signatures(types))
        .chain(super::heap::signatures(types))
        .chain(super::text::signatures(types))
    {
        publish(types, signature, &mut out);
    }
    // Existing @preserves_captures(p), @preserves_unique(p), then
    // @unretained(q, ..) naming the borrowed
    // parameters it reads and stores nothing of, may prefix a ledger line
    // (MODE-2, CLO-2).
    let ledger = SECTION22.lines().map(|line| {
        let (preserves_captures, line) = ledger_prefix(line, "@preserves_captures(");
        let (preserves_unique, line) = ledger_prefix(line, "@preserves_unique(");
        let (unretained, line) = ledger_prefix(line, "@unretained(");
        (preserves_captures, preserves_unique, unretained, line)
    });
    for (preserves_captures, preserves_unique, unretained, line) in
        ledger.filter(|(_, _, _, l)| l.starts_with("fn ") || l.starts_with("N fn "))
    {
        let noalloc = line.starts_with("N ");
        let text = line.trim_start_matches("N ").trim_start_matches("fn ");
        let opening = text.find('(').unwrap();
        let head = &text[..opening];
        let (name, generics) = head
            .split_once('[')
            .map_or((head, ""), |(n, g)| (n, g.strip_suffix(']').unwrap()));
        let closing = matching(text, opening);
        let mut b = Builder::new(types, generics)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let mut f = b.function(
            &text[opening + 1..closing],
            text[closing + 1..]
                .trim()
                .strip_prefix("->")
                .unwrap()
                .trim(),
            noalloc,
        );
        if name == "iter.from" {
            let mut result = b.types.get(f.result).unwrap().clone();
            result.erased_captures = Some(ErasedCaptures::FromArguments(
                vec![
                    ParameterRef::bound(ParamId(0)).into(),
                    ParameterRef::bound(ParamId(1)).into(),
                ]
                .into(),
            ));
            f.result = b.types.intern(result);
        }
        if name == "iter.next" {
            f.state_promises = vec![StatePromise {
                selector: ParameterRef::bound(ParamId(0)).into(),
                advance_only: true,
                preserve_captures: true,
                preserve_unique: false,
                unretained: Default::default(),
            }]
            .into();
            apply_state_promises(&b.types, &mut f.parameters, &f.state_promises);
        }
        let ordinal = |name: &str| {
            ParamId(
                f.parameters
                    .iter()
                    .position(|p| p.name.as_deref() == Some(name))
                    .unwrap() as u32,
            )
        };
        if let Some(selected) = preserves_unique {
            let unretained =
                unretained.map_or_else(Vec::new, |names| names.split(", ").map(ordinal).collect());
            assert!(
                unretained
                    .iter()
                    .all(|p| f.parameters[p.0 as usize].passing == PassingMode::Borrow),
                "{name}: only a borrowed parameter is unretained"
            );
            f.state_promises = vec![StatePromise {
                selector: ParameterRef::bound(ordinal(selected)).into(),
                advance_only: false,
                preserve_captures: false,
                preserve_unique: true,
                unretained: unretained.into(),
            }]
            .into();
        } else {
            assert!(unretained.is_none(), "{name}: @unretained needs a promise");
        }
        if let Some(selected) = preserves_captures {
            let mut promises = f.state_promises.to_vec();
            for selected in selected.split(", ") {
                promises.push(StatePromise {
                    selector: ParameterRef::bound(ordinal(selected)).into(),
                    advance_only: false,
                    preserve_captures: true,
                    preserve_unique: false,
                    unretained: Default::default(),
                });
            }
            f.state_promises = promises.into();
            apply_state_promises(&b.types, &mut f.parameters, &f.state_promises);
        }
        let mut signature = b.signature(name, SignatureKind::Function(f), false);
        signature.metadata.track_caller = matches!(
            name,
            "task.await_any"
                | "task.cancel_all"
                | "clock.sleep_until"
                | "out.print"
                | "proc.spawn"
                | "proc.spawn_pipeline"
                | "child.write"
                | "child.close_stdin"
                | "child.wait"
                | "proc.wait_group"
                | "group.write"
                | "group.close_stdin"
        );
        publish(types, signature, &mut out);
    }
    // `net.read_into`'s compiled body passes its buffer's size to one fixed runtime
    // entry, which no size generic can name.
    let mut b = Builder::new(types, "k: size").with_declarations(&out);
    let mut f = b.function(
        "n: Net, mut s: Stream of n, mut buffer: Inline[u8, k], capacity: u64",
        "Result[u64, NetError]",
        true,
    );
    f.state_promises = vec![StatePromise {
        selector: ParameterRef::bound(ParamId(1)).into(),
        advance_only: false,
        preserve_captures: true,
        preserve_unique: false,
        unretained: Default::default(),
    }]
    .into();
    apply_state_promises(&b.types, &mut f.parameters, &f.state_promises);
    publish(
        types,
        b.signature("$intrinsic.NetReadInto", SignatureKind::Function(f), false),
        &mut out,
    );
    let mut b = Builder::new(types, "k: size").with_declarations(&out);
    let f = b.function(
        "n: Net, mut r: ReadHalf of n, mut buffer: Inline[u8, k], capacity: u64",
        "Result[u64, NetError]",
        true,
    );
    publish(
        types,
        b.signature(
            "$intrinsic.ReadHalfReadInto",
            SignatureKind::Function(f),
            false,
        ),
        &mut out,
    );
    // The native task runtime's entries (CONC-2), which the compiled bodies of `task.scope`,
    // `task.async` and `task.await` call. A result crosses tasks as a whole `Option[R]` image
    // of `size` bytes, so no entry depends on R's layout.
    for (generics, name, params, result) in [
        (
            "",
            "$intrinsic.TaskScopeEnter",
            "tasks: Tasks",
            "Scope in tasks",
        ),
        (
            "",
            "$intrinsic.TaskScopeExit",
            "s: Scope, error: bool",
            "()",
        ),
        // `task.scope_until` (CONC-3): a scope whose deadline arms one timer at entry; the
        // exit removes it and says whether it fired.
        (
            "",
            "$intrinsic.TaskScopeEnterUntil",
            "tasks: Tasks, deadline: Deadline",
            "Scope in tasks",
        ),
        (
            "",
            "$intrinsic.TaskScopeExitUntil",
            "s: Scope, error: bool",
            "bool",
        ),
        (
            "R",
            "$intrinsic.TaskSpawn",
            "s: Scope, take main: once fn() -> (), flags: u64",
            "Task[R] of s",
        ),
        (
            "R",
            "$intrinsic.TaskFinish",
            "mut slot: Option[R], none: Option[R], size: u64, descriptor: u32",
            "()",
        ),
        (
            "R",
            "$intrinsic.TaskJoin",
            "s: Scope, take h: Task[R] of s, mut slot: Option[R], size: u64",
            "()",
        ),
        // `task.await_at`'s compiled body checks that the member is unawaited, then takes its
        // handle from the set and joins it.
        (
            "R",
            "$intrinsic.TaskSetHas",
            "s: Scope, set: TaskSet[R] of s, index: u64",
            "bool",
        ),
        (
            "R",
            "$intrinsic.TaskSetTake",
            "s: Scope, mut set: TaskSet[R] of s, index: u64",
            "Task[R] of s",
        ),
        // A message crosses as the `Some` image of an `Option[T]` the runtime copies into
        // the channel's queue, leaving the channel's `None` image; the open gives the
        // runtime both, and the layout descriptor that releases unreceived messages
        // (CONC-6). The status is 0 sent or received, 1 closed, 2 + cause canceled.
        (
            "T",
            "$intrinsic.ChannelOpen",
            "tasks: Tasks, none: Option[T], descriptor: u32",
            "(Sender[T], Receiver[T])",
        ),
        (
            "T",
            "$intrinsic.ChannelSend",
            "mut s: Sender[T], mut message: Option[T]",
            "u32",
        ),
        (
            "T",
            "$intrinsic.ChannelReceive",
            "mut r: Receiver[T], mut message: Option[T]",
            "u32",
        ),
        // Mutexes move Option[T] images, described at creation; atomics copy
        // fixed scalar images (CONC-4, CONC-5, FAIL-4).
        (
            "T",
            "$intrinsic.MutexCreate",
            "mut payload: Option[T], none: Option[T], descriptor: u32, wrapper: u32",
            "Mutex[T]",
        ),
        // 0: the payload moved into `payload`; 1: the mutex is dead.
        (
            "T",
            "$intrinsic.MutexTake",
            "m: Mutex[T], mut payload: Option[T]",
            "u32",
        ),
        (
            "T",
            "$intrinsic.MutexPut",
            "m: Mutex[T], mut payload: Option[T]",
            "()",
        ),
        ("T", "$intrinsic.MutexKill", "m: Mutex[T]", "()"),
        ("T", "$intrinsic.MutexCause", "m: Mutex[T]", "()"),
        (
            "A",
            "$intrinsic.AtomicCreate",
            "value: Option[A], size: u64, payload_offset: u64, payload_size: u64",
            "Atomic[A]",
        ),
        (
            "A",
            "$intrinsic.AtomicLoad",
            "a: Atomic[A], mut slot: Option[A], size: u64",
            "()",
        ),
        (
            "A",
            "$intrinsic.AtomicStore",
            "a: Atomic[A], value: Option[A], size: u64",
            "()",
        ),
        // 0, or 1 + the running task's `CancelCause` ordinal (CONC-3).
        ("", "$intrinsic.TaskCanceled", "s: Scope", "u32"),
        (
            "",
            "$intrinsic.CpuMapScoped",
            "tasks: Tasks, width: u32, action: u32, count: u64",
            "u32",
        ),
        // `mapped.with`'s compiled body: the view it lends its body, then 0, or 1 when the
        // mapped file was truncated since the mapping was made.
        (
            "",
            "$intrinsic.MappedView",
            "d: Dir, m: Mapped of d",
            "Bytes",
        ),
        (
            "",
            "$intrinsic.MappedCheck",
            "d: Dir, m: Mapped of d",
            "u32",
        ),
        // `shared.thaw`'s compiled body asks whether its root is the last; in free mode the
        // answer is an op's, recorded live and replayed.
        ("T", "$intrinsic.SharedThawTurn", "root: Shared[T]", "bool"),
        // `task.isolate`'s caught chain, read record by record to build its `Failure` (FAIL-2).
        ("", "$intrinsic.IsolateBegin", "tasks: Tasks", "u64"),
        ("", "$intrinsic.IsolateEnd", "tasks: Tasks", "()"),
        ("", "$intrinsic.IsolateCount", "tasks: Tasks", "u64"),
        (
            "",
            "$intrinsic.IsolateKind",
            "tasks: Tasks, index: u64",
            "u32",
        ),
        (
            "",
            "$intrinsic.IsolateRule",
            "tasks: Tasks, index: u64",
            "String",
        ),
        (
            "",
            "$intrinsic.IsolateDefinition",
            "tasks: Tasks, index: u64",
            "String",
        ),
        (
            "",
            "$intrinsic.IsolateLine",
            "tasks: Tasks, index: u64",
            "u64",
        ),
        (
            "",
            "$intrinsic.IsolateColumn",
            "tasks: Tasks, index: u64",
            "u64",
        ),
        (
            "",
            "$intrinsic.IsolateSpawn",
            "tasks: Tasks, index: u64",
            "List[u64]",
        ),
        // Runner calls (RUN-4) behind the compiled bodies of `runner.compute`, `runner.call`
        // and `runner.spawn`: the caller sends `f`'s call with its argument's codec bytes and
        // the callee's `main`, a static `fn() -> ()` that runs in the callee run, then waits
        // for the outcome code and takes its bytes (`hirsel-rt/src/runners.rs`). The callee's
        // `main` reads its entry, argument and root Io, and replies with its outcome's bytes.
        (
            "F",
            "$intrinsic.RunnerSend",
            "r: Runner, f: Entry[F], argument: Bytes, main: fn() -> (), form: u64",
            "u64",
        ),
        ("", "$intrinsic.RunnerWait", "ticket: u64", "u64"),
        ("", "$intrinsic.RunnerTake", "ticket: u64", "Bytes"),
        ("F", "$intrinsic.RunnerEntry", "", "F"),
        ("", "$intrinsic.RunnerArgument", "", "Bytes"),
        ("", "$intrinsic.RunnerIo", "", "Io"),
        // The `Tasks` word the callee's `main` passes the isolation entries, which read none.
        ("", "$intrinsic.RunnerTasks", "", "Tasks"),
        ("", "$intrinsic.RunnerReply", "tag: u64, reply: Bytes", "()"),
        // `runner.grants`' compiled body: the grant set's flags, one bit per field in order.
        ("", "$intrinsic.RunnerGrants", "r: Runner", "u64"),
    ] {
        let mut b = Builder::new(types, generics).with_declarations(&out);
        let f = b.function(params, result, false);
        let mut signature = b.signature(name, SignatureKind::Function(f), false);
        signature.metadata.track_caller = matches!(
            name,
            "$intrinsic.TaskSpawn"
                | "$intrinsic.TaskScopeExit"
                | "$intrinsic.TaskScopeExitUntil"
                | "$intrinsic.TaskJoin"
                | "$intrinsic.ChannelReceive"
        );
        publish(types, signature, &mut out);
    }
    // All declaration snapshots exist before deriving structural properties.
    // This replaces the spelling-based plain/sendable allowlists in Builder.
    let declarations = out.clone();
    for signature in out.values_mut() {
        let mut normalized = signature.as_ref().clone();
        let mut generics = normalized.generics().to_vec();
        for constraint in &normalized.metadata.schema {
            if let SchemaConstraint::Domain { parameter, .. } = constraint {
                if let Some(GenericParameter::Type { bounds, .. }) =
                    generics.get_mut(parameter.parameter().0 as usize)
                {
                    bounds.push(Bound::Plain);
                }
            }
        }
        let generics = GenericDeclarations::signature(&generics);
        let mut lookup =
            |d: DefId| Ok::<_, hirsel_types::QueryError>(declarations.get(&d).cloned());
        match &mut normalized.kind {
            SignatureKind::Function(f) => {
                for p in &mut f.parameters {
                    p.ty = normalize_type_properties(types, p.ty, &generics, &mut lookup).unwrap();
                }
                f.result =
                    normalize_type_properties(types, f.result, &generics, &mut lookup).unwrap();
                if let Some(error) = &mut f.error {
                    *error =
                        normalize_type_properties(types, *error, &generics, &mut lookup).unwrap();
                }
            }
            SignatureKind::Type(t) => {
                if let Some(repr) = &mut t.representation {
                    *repr =
                        normalize_type_properties(types, *repr, &generics, &mut lookup).unwrap();
                }
            }
            SignatureKind::Const { ty: t, .. } => {
                *t = normalize_type_properties(types, *t, &generics, &mut lookup).unwrap()
            }
            SignatureKind::Family(_) | SignatureKind::Implementation(_) => {
                unreachable!("core declares no signature or implementation")
            }
        }
        normalized.fingerprint = hirsel_types::encoding::fingerprint(types, &normalized, &[])
            .expect("normalized core signature");
        *signature = Arc::new(normalized);
    }
    out
}
/// Declare a core variant type `name[generic] = variants`.
fn variant(
    types: &Arc<TypeInterner>,
    name: &str,
    generic: &str,
    variants: &str,
    out: &mut BTreeMap<DefId, SignatureRef>,
) {
    let mut b = Builder::new(types, generic)
        .with_declarations(out)
        .with_schema(super::schema(name));
    let variants = variants
        .split('|')
        .map(|v| {
            let (name, payload) = v.split_once('(').unwrap();
            let fields = split(payload.strip_suffix(')').unwrap(), ',')
                .iter()
                .map(|s| (None, b.ty(s, PassingMode::Take)))
                .collect();
            Variant {
                name: name.into(),
                fields,
            }
        })
        .collect();
    let repr = b.push(TypeKind::Variant(variants));
    let mut t = type_definition(b.generics.clone(), Some(repr));
    t.authority = AuthoritySummary::Known(
        b.generics
            .iter()
            .enumerate()
            .filter_map(|(i, g)| {
                matches!(g, GenericParameter::Type { .. }).then_some(AuthorityTerm::TypeParameter(
                    TypeParameterRef::Bound {
                        depth: 0,
                        parameter: TypeParamId(i as u32),
                    },
                ))
            })
            .collect(),
    );
    if b.generics.is_empty()
        && !matches!(
            name,
            "TlsRoots"
                | "TlsClientAuth"
                | "PeerTls"
                | "NetConnection"
                | "NetStep"
                | "ChildStep"
                | "ShapeBody"
                | "CallEnd"
                | "RunFrame"
        )
    {
        t.bounds = vec![
            Bound::Plain,
            Bound::Copyable,
            Bound::Data,
            Bound::Key,
            Bound::Sendable,
            Bound::Inert,
            Bound::NoallocDrop,
        ];
    }
    if name == "ShapeBody" {
        t.bounds = shape_data_bounds();
    }
    serializable_core(name, &mut t);
    publish(
        types,
        b.signature(name, SignatureKind::Type(Box::new(t)), false),
        out,
    );
}
/// CT-3 shape data holds only strings and lists of them: structurally
/// copyable, data, key, inert and noalloc_drop, but neither plain nor sendable.
fn shape_data_bounds() -> Vec<Bound> {
    vec![
        Bound::Copyable,
        Bound::Data,
        Bound::Key,
        Bound::Inert,
        Bound::NoallocDrop,
    ]
}
/// SER-1: the core errors and outcomes that can cross a runner call or be stored, and the
/// data they carry. A generic one (`Option`, `Result`, `Bounded`) admits no
/// bound on its parameters, so it is serializable exactly when its arguments are, proved
/// after substitution. `Path` is transparent text: a Path is data, and its authority stays in
/// the `Dir` it is used with. Capability holders (`Deadline`, `Command`), resources, other
/// opaque types (`Instant`) and test-script inputs stay out.
fn serializable_core(name: &str, t: &mut TypeDefinition) {
    if matches!(
        name,
        "Base64Alphabet"
            | "Base64Padding"
            | "Option"
            | "Result"
            | "Bounded"
            | "DecodeError"
            | "CancelCause"
            | "CancelError"
            | "ChannelError"
            | "CpuPoolError"
            | "FileError"
            | "FileKind"
            | "Durability"
            | "Path"
            | "DirEntry"
            | "Metadata"
            | "IpAddr"
            | "SocketAddr"
            | "UdpReceived"
            | "TlsRoots"
            | "TlsFailure"
            | "NetError"
            | "GrantError"
            | "ExitStatus"
            | "ProcessOutput"
            | "PipelineOutput"
            | "ProcessError"
            | "InputError"
            | "EnvError"
            | "OutputError"
            | "ClockError"
            | "RandomError"
            | "TelemetryError"
            | "WaitError"
            | "SignalKind"
            | "SignalError"
            | "ArgError"
            | "FloatIntError"
            | "OutOfSpace"
            | "CliError"
            | "FailureKind"
            | "SourceSite"
            | "Failure"
            | "NetOperation"
            | "NetRule"
            | "ProcessRule"
            | "GrantAccess"
            | "DirGrant"
            | "GrantSet"
            | "NotRunReason"
            | "InDoubtReason"
            | "RunError"
            | "CallForm"
            | "CallEnd"
            | "SessionOpen"
            | "SessionOpened"
            | "CallRef"
            | "RemoteCall"
            | "RemoteOutcome"
            | "RunFrame"
    ) {
        t.serializable = true;
        if t.generics.is_empty() {
            t.bounds.push(Bound::Serializable);
        }
    }
}
fn type_definition(
    generics: Vec<GenericParameter>,
    representation: Option<TypeId>,
) -> TypeDefinition {
    TypeDefinition {
        loop_source: None,
        format: None,
        generics,
        representation,
        bounds: vec![],
        authority: AuthoritySummary::inert(),
        capability: None,
        implementation_authority: None,
        serializable: false,
        serializable_check: None,
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        captures: None,
        destructor_allocation: Allocation::NoAlloc,
    }
}
pub(crate) struct Builder {
    pub types: Arc<TypeInterner>,
    pub generics: Vec<GenericParameter>,
    names: Vec<String>,
    schema: Vec<SchemaConstraint>,
    declarations: BTreeMap<DefId, SignatureRef>,
    params: Vec<String>,
    outer_params: Vec<Vec<String>>,
    generic_depth: u32,
    /// The fields of the record being built, which `of .field` brands name (PROV-5).
    siblings: Vec<String>,
}
impl Builder {
    pub fn new(types: &Arc<TypeInterner>, generics: &str) -> Self {
        let generics: Vec<_> = split(generics, ',')
            .iter()
            .map(|s| {
                let (name, bounds) = s
                    .split_once(':')
                    .map_or((s.trim(), ""), |(n, b)| (n.trim(), b.trim()));
                if bounds == "size" {
                    GenericParameter::Size { name: name.into() }
                } else {
                    GenericParameter::Type {
                        name: name.into(),
                        bounds: bounds
                            .split('+')
                            .filter(|s| !s.is_empty())
                            .map(|b| match b.trim() {
                                "inert" => Bound::Inert,
                                "sendable" => Bound::Sendable,
                                "copyable" => Bound::Copyable,
                                "data" => Bound::Data,
                                "key" => Bound::Key,
                                "serializable" => Bound::Serializable,
                                _ => panic!("unknown bound {b}"),
                            })
                            .collect(),
                    }
                }
            })
            .collect();
        let names = generics
            .iter()
            .map(|g| match g {
                GenericParameter::Type { name, .. } | GenericParameter::Size { name } => {
                    name.clone()
                }
            })
            .collect();
        Self {
            types: types.clone(),
            generics,
            names,
            schema: vec![],
            declarations: BTreeMap::new(),
            params: vec![],
            outer_params: vec![],
            generic_depth: 0,
            siblings: vec![],
        }
    }
    pub(crate) fn with_schema(mut self, schema: Vec<SchemaConstraint>) -> Self {
        self.schema = schema;
        self
    }
    fn with_declarations(mut self, declarations: &BTreeMap<DefId, SignatureRef>) -> Self {
        self.declarations = declarations.clone();
        self
    }
    pub fn signature(self, name: &str, kind: SignatureKind, opaque: bool) -> Signature {
        Signature {
            definition: DefId::intern(&hirsel_syntax::DefPath::core(name)),
            kind,
            fingerprint: ContentHash::default(),
            metadata: SignatureMetadata {
                visibility: if opaque {
                    Visibility::PublicOpaque
                } else {
                    Visibility::Public
                },
                roles: super::nominal_roles().into(),
                schema: self.schema,
                ..Default::default()
            },
        }
    }
    fn push(&mut self, kind: TypeKind) -> TypeId {
        let component = |t| self.types.get(t).unwrap().drop_allocation.clone();
        let drop_allocation = cleanup_summary(&kind, |leaf| match leaf {
            CleanupLeaf::Component(t) => component(t),
            CleanupLeaf::Parameter(p) => {
                if self.schema.contains(&SchemaConstraint::NoallocDrop(p)) {
                    DropAllocation::NoAlloc
                } else {
                    parameter_cleanup(
                        &GenericDeclarations::from_binders([(
                            GenericBinder::Bound(self.generic_depth),
                            self.generics.clone(),
                        )]),
                        p,
                    )
                }
            }
            CleanupLeaf::Nominal(definition, arguments) => {
                if let Some(Signature {
                    kind: SignatureKind::Type(declaration),
                    ..
                }) = self.declarations.get(&definition).map(|s| s.as_ref())
                {
                    declared_cleanup(declaration, &self.types, arguments, component)
                } else {
                    // Compiler-private forward declarations compose their owned
                    // arguments before their snapshots are published.
                    arguments
                        .iter()
                        .filter_map(|a| match a {
                            GenericArgument::Type(t) => Some(*t),
                            GenericArgument::Size(_) => None,
                        })
                        .fold(DropAllocation::NoAlloc, |a, t| a.join(component(t)))
                }
            }
        });
        self.types.intern(Type {
            drop_allocation,
            ..Type::with_kind(kind)
        })
    }
    fn record(&mut self, text: &str) -> TypeId {
        let fields = split(text, ',');
        self.siblings = fields
            .iter()
            .map(|s| s.split_once(':').unwrap().0.trim().to_owned())
            .collect();
        let fields = fields
            .iter()
            .map(|s| {
                let (name, ty) = s.split_once(':').unwrap();
                Field {
                    name: name.trim().into(),
                    ty: self.ty(ty, PassingMode::Take),
                    growable: false,
                }
            })
            .collect();
        self.push(TypeKind::Record(fields))
    }
    pub fn function(&mut self, params: &str, result: &str, noalloc: bool) -> FunctionType {
        self.outer_params.push(std::mem::take(&mut self.params));
        let params = split(params, ',');
        self.params = params
            .iter()
            .map(|p| {
                p.split_once(':')
                    .map_or("", |(n, _)| n.split_whitespace().last().unwrap())
                    .to_owned()
            })
            .collect();
        let parameters: Vec<_> = params
            .iter()
            .map(|p| {
                let (passing, name, ty) = if let Some((head, ty)) = p.split_once(':') {
                    let head = head.trim();
                    let (passing, name) = if let Some(n) = head.strip_prefix("take ") {
                        (PassingMode::Take, n)
                    } else if let Some(n) = head.strip_prefix("mut ") {
                        (PassingMode::Mut, n)
                    } else {
                        (PassingMode::Borrow, head)
                    };
                    (passing, name, ty)
                } else if let Some(t) = p.strip_prefix("mut ") {
                    (PassingMode::Mut, "", t)
                } else if let Some(t) = p.strip_prefix("take ") {
                    (PassingMode::Take, "", t)
                } else {
                    (PassingMode::Borrow, "", p.as_str())
                };
                // `take x: T in x` is local to its own argument: its own
                // scope, never a placement, even when x is an arena.
                let own = format!(" in {}", name.trim());
                let ty = match ty.trim().strip_suffix(own.as_str()) {
                    Some(base) if passing == PassingMode::Take && !name.is_empty() => {
                        let id = self.ty(base, passing);
                        let mut t = self.types.get(id).unwrap().clone();
                        t.modes.relation = Relation::environment();
                        self.types.intern(t)
                    }
                    _ => self.ty(ty, passing),
                };
                Parameter {
                    name: (!name.is_empty()).then(|| name.into()),
                    label: None,
                    passing,
                    ty,
                }
            })
            .collect();
        let (result, uses_text) = top_suffix(result, " uses ");
        let result = self.ty(result, PassingMode::Take);
        let authority = AuthoritySummary::Known(
            parameters
                .iter()
                .enumerate()
                .filter_map(|(i, p)| match &self.types.get(p.ty).unwrap().kind {
                    TypeKind::Named { definition, .. }
                        if matches!(
                            definition.path().0.name.as_str(),
                            "Io" | "Args"
                                | "Env"
                                | "Unsafe"
                                | "Dir"
                                | "Stdin"
                                | "Stderr"
                                | "Stdout"
                                | "Signals"
                                | "MonotonicClock"
                                | "WallClock"
                                | "Random"
                                | "Telemetry"
                                | "Tasks"
                                | "Net"
                                | "Proc"
                                | "Runner"
                                | "Scope"
                                | "Arena"
                                | "Test"
                        ) =>
                    {
                        Some(AuthorityTerm::Parameter(hirsel_types::ParameterRef::bound(
                            ParamId(i as u32),
                        )))
                    }
                    _ => None,
                })
                .collect(),
        );
        let uses = uses_text.map_or(Uses::Unrestricted, |s| Uses::Only(self.uses(s)));
        let error = match &self.types.get(result).unwrap().kind {
            TypeKind::Named {
                definition,
                arguments,
            } if *definition.path() == hirsel_syntax::DefPath::core("Result") => {
                match &arguments[1] {
                    GenericArgument::Type(t) => Some(*t),
                    _ => None,
                }
            }
            _ => None,
        };
        self.params = self.outer_params.pop().unwrap();
        FunctionType {
            state_promises: Default::default(),
            generics: self.generics.clone().into(),
            parameters: parameters.into(),
            result,
            error,
            allocation: if noalloc {
                Allocation::NoAlloc
            } else {
                Allocation::MayAllocate
            },
            authority,
            uses,
            contracts: vec![].into(),
            incomplete: false,
            foreign: None,
        }
    }
    fn uses(&self, text: &str) -> AuthoritySummary {
        AuthoritySummary::Known(
            split(
                text.trim().trim_start_matches('{').trim_end_matches('}'),
                ',',
            )
            .iter()
            .map(|n| {
                self.parameter(n).map_or_else(
                    || AuthorityTerm::Capability(hirsel_syntax::DefPath::core(n)),
                    AuthorityTerm::Parameter,
                )
            })
            .collect(),
        )
    }
    fn parameter(&self, name: &str) -> Option<ParameterRef> {
        std::iter::once(&self.params)
            .chain(self.outer_params.iter().rev())
            .enumerate()
            .find_map(|(depth, names)| {
                names
                    .iter()
                    .position(|n| n == name)
                    .map(|i| ParameterRef::Bound {
                        depth: depth as u32,
                        parameter: ParamId(i as u32),
                    })
            })
    }
    fn origin(&self, text: &str) -> Origin {
        if let Some(field) = text.trim().strip_prefix('.') {
            let index = self
                .siblings
                .iter()
                .position(|s| s == field)
                .unwrap_or_else(|| panic!("unknown sibling origin {text}"));
            return Origin {
                root: OriginRoot::Sibling(FieldId(index as u32)),
                fields: vec![].into(),
            };
        }
        let parameter = self
            .parameter(text.trim())
            .unwrap_or_else(|| panic!("unknown origin {text}"));
        Origin {
            root: OriginRoot::Parameter(parameter),
            fields: vec![].into(),
        }
    }
    pub fn ty(&mut self, text: &str, passing: PassingMode) -> TypeId {
        let text = text.trim();
        // Prefix modifiers cover the whole type after them (v22 R5).
        for (word, set) in [
            (
                "unique ",
                (|t: &mut Type| t.modes.uniqueness = Uniqueness::Unique) as fn(&mut Type),
            ),
            ("sendable ", |t: &mut Type| {
                t.sendability_requirement = SendabilityRequirement::Sendable;
                t.modes.sendability = Sendability::Sendable;
            }),
            ("once ", |t: &mut Type| {
                t.modes.multiplicity = Multiplicity::Once
            }),
        ] {
            if let Some(rest) = text.strip_prefix(word) {
                let id = self.ty(rest, passing);
                let mut t = self.types.get(id).unwrap().clone();
                set(&mut t);
                return self.types.intern(t);
            }
        }
        let (base, qualifiers) = first_qualifier(text);
        let kind = if base.starts_with('(') && matching(base, 0) == base.len() - 1 {
            let inside = &base[1..base.len() - 1];
            let fields = split(inside, ',');
            if fields.len() == 1 {
                let id = self.ty(&fields[0], passing);
                self.types.get(id).unwrap().kind.clone()
            } else {
                TypeKind::Tuple(
                    fields
                        .iter()
                        .map(|s| self.ty(s, PassingMode::Take))
                        .collect(),
                )
            }
        } else if let Some(inner) = base.strip_prefix("box ") {
            TypeKind::Box(self.ty(inner, PassingMode::Take))
        } else if base.starts_with("fn(") {
            let end = matching(base, 2);
            self.generic_depth += 1;
            let mut f = self.function(
                &base[3..end],
                base[end + 1..]
                    .trim()
                    .strip_prefix("->")
                    .unwrap_or("()")
                    .trim(),
                false,
            );
            self.generic_depth -= 1;
            f.generics.clear(); // schema generics belong to the containing declaration
            f.authority = match &f.uses {
                Uses::Only(a) => a.clone(),
                Uses::Unrestricted => AuthoritySummary::Unknown,
            };
            TypeKind::Function(f)
        } else {
            let (name, args) = base.split_once('[').map_or((base, ""), |(n, a)| {
                (
                    n,
                    a.strip_suffix(']')
                        .unwrap_or_else(|| panic!("DEBUGTY {text}")),
                )
            });
            if let Some(i) = self.names.iter().position(|n| n == name) {
                TypeKind::Parameter(TypeParameterRef::Bound {
                    depth: self.generic_depth,
                    parameter: TypeParamId(i as u32),
                })
            } else if let Some(p) = primitive(name) {
                TypeKind::Primitive(p)
            } else {
                let arguments = split(args, ',')
                    .iter()
                    .map(|a| {
                        if let Some(i) = self.names.iter().position(|n| n == a)
                            && matches!(self.generics[i], GenericParameter::Size { .. })
                        {
                            GenericArgument::Size(Size::Parameter(SizeParameterRef::Bound {
                                depth: self.generic_depth,
                                parameter: SizeParamId(i as u32),
                            }))
                        } else if let Ok(size) = a.parse::<u64>() {
                            GenericArgument::Size(Size::Constant(size))
                        } else {
                            GenericArgument::Type(self.ty(a, PassingMode::Take))
                        }
                    })
                    .collect();
                if let Some(kind) = builtin(name) {
                    TypeKind::Builtin { kind, arguments }
                } else {
                    TypeKind::Named {
                        definition: DefId::intern(&hirsel_syntax::DefPath::core(name)),
                        arguments,
                    }
                }
            }
        };
        let id = self.push(kind);
        let mut t = self.types.get(id).unwrap().clone();
        if passing != PassingMode::Take {
            t.modes.relation = Relation::environment();
        }
        if matches!(&t.kind,TypeKind::Named{definition,..} if matches!(definition.path().0.name.as_str(),"Scope"|"Arena"|"Test"))
        {
            t.modes.relation = Relation::environment();
        }
        let mut rest = qualifiers.trim();
        while !rest.is_empty() {
            let (word, next) = if rest.starts_with("of ") {
                ("of", &rest[3..])
            } else if rest.starts_with("in ") {
                ("in", &rest[3..])
            } else {
                panic!("unknown qualifier {rest}")
            };
            let (value, tail) = first_qualifier(next.trim());
            if word == "of" {
                t.brand = Brand {
                    alternatives: vec![self.origin(value)].into(),
                };
            } else {
                // A relation adds its places to the type's own root (S2).
                let value = value.trim();
                let places = value
                    .strip_prefix('{')
                    .and_then(|v| v.strip_suffix('}'))
                    .map_or_else(|| vec![value.to_owned()], |v| split(v, ','));
                for place in places {
                    let relation = match place.trim() {
                        "env" => Relation::environment(),
                        "heap" => Relation::global(),
                        name => Relation::place(self.origin(name)),
                    };
                    t.modes.relation = t.modes.relation.union(&relation);
                }
            }
            rest = tail.trim();
        }
        self.types.intern(t)
    }
}

pub(super) fn primitive(n: &str) -> Option<Primitive> {
    use Primitive::*;
    Some(match n {
        "bool" => Bool,
        "u8" => U8,
        "u16" => U16,
        "u32" => U32,
        "u64" => U64,
        "u128" => U128,
        "i8" => I8,
        "i16" => I16,
        "i32" => I32,
        "i64" => I64,
        "i128" => I128,
        "f32" => F32,
        "f64" => F64,
        "char" => Char,
        "String" => String,
        "Bytes" => Bytes,
        "Never" => Never,
        _ => return None,
    })
}
fn builtin(n: &str) -> Option<BuiltinType> {
    use BuiltinType::*;
    Some(match n {
        "Array" => Array,
        "List" => List,
        "Map" => Map,
        "Set" => Set,
        "SortedMap" => SortedMap,
        "Inline" => Inline,
        "Simd" => Simd,
        "Packed" => Packed,
        "Shared" => Shared,
        "Slots" => Slots,
        "SlotId" => SlotId,
        "Region" => Region,
        "Iter" => Iter,
        "Task" => Task,
        "Sender" => Sender,
        "Receiver" => Receiver,
        "Mutex" => Mutex,
        "Atomic" => Atomic,
        "Ptr" => Ptr,
        "TypeShape" => TypeShape,
        _ => return None,
    })
}
/// A ledger line's leading `@attribute(arguments) `, if it starts with `opening`.
fn ledger_prefix<'a>(line: &'a str, opening: &str) -> (Option<&'a str>, &'a str) {
    line.strip_prefix(opening)
        .and_then(|rest| rest.split_once(") "))
        .map_or((None, line), |(arguments, rest)| (Some(arguments), rest))
}
fn matching(s: &str, start: usize) -> usize {
    let mut depth = 0;
    for (i, c) in s.char_indices().skip_while(|(i, _)| *i < start) {
        if c == '(' {
            depth += 1;
        }
        if c == ')' {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    panic!("unclosed {s}")
}
fn split(s: &str, sep: char) -> Vec<String> {
    let mut depth = 0;
    let mut start = 0;
    let mut out = vec![];
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && c == sep {
            out.push(s[start..i].trim().into());
            start = i + 1;
        }
    }
    if !s[start..].trim().is_empty() {
        out.push(s[start..].trim().into());
    }
    out
}
fn top_suffix<'a>(s: &'a str, sep: &str) -> (&'a str, Option<&'a str>) {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        if depth == 0 && s[i..].starts_with(sep) {
            return (&s[..i], Some(&s[i + sep.len()..]));
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    (s, None)
}
fn first_qualifier(s: &str) -> (&str, &str) {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        if depth == 0 && (s[i..].starts_with(" of ") || s[i..].starts_with(" in ")) {
            return (s[..i].trim(), s[i..].trim());
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    (s.trim(), "")
}
