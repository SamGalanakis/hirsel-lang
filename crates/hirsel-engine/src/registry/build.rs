//! Small declaration-data reader, intentionally independent of runtime dispatch.
use super::*;
use hirsel_syntax::Visibility;

pub(super) fn registry() -> BTreeMap<DefPath, SignatureRef> {
    let mut out = BTreeMap::new();
    // DROP-4 declares this intrinsic outside the core library ledger.
    let mut forget = Builder::new("T");
    let signature = forget.function("take x: T", "()", true);
    publish(
        forget.signature("forget", SignatureKind::Function(signature), false),
        &mut out,
    );
    // Core library overview fixtures are declarations too, never ambient authority.
    for (name, generic, parameters, result) in [
        (
            "store.transact",
            "R: inert,E: inert",
            "db: Db, body: fn(tx: Tx) -> Result[R,E] uses {}",
            "Result[R,TxFailure[E]]",
        ),
        ("stock.get", "", "tx: Tx,sku: Sku", "Result[u32,StockError]"),
        (
            "stock.put",
            "",
            "tx: Tx,sku: Sku,n: u32",
            "Result[(),StockError]",
        ),
        (
            "reservations.insert",
            "",
            "tx: Tx,value: Reservation",
            "Result[Reservation,StockError]",
        ),
    ] {
        let mut b = Builder::new(generic)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let f = b.function(parameters, result, false);
        publish(
            b.signature(name, SignatureKind::Function(f), false),
            &mut out,
        );
    }
    for name in [
        "Io",
        "Args",
        "Env",
        "Unsafe",
        "Dir",
        "Stdin",
        "Stderr",
        "Stdout",
        "MonotonicClock",
        "WallClock",
        "Random",
        "Tasks",
        "Net",
        "Proc",
        "Scope",
        "Arena",
        "Test",
        "Db",
        "Tx",
    ] {
        let scoped = matches!(name, "Scope" | "Arena" | "Test" | "Tx");
        let b = Builder::new("").with_declarations(&out);
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
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
    }
    // These inert, plain views/positions have intrinsic loop/brand rules (docs/spec/core-library.md).
    for name in [
        "Instant",
        "Path",
        "StrPos",
        "ScalarView",
        "PositionView",
        "Sku",
    ] {
        let b = Builder::new("").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.bounds = vec![Bound::Copyable, Bound::Inert, Bound::NoallocDrop];
        if matches!(name, "Instant" | "Path" | "StrPos" | "Sku") {
            t.bounds.push(Bound::Plain);
        }
        if matches!(name, "Path" | "StrPos" | "Sku") {
            t.bounds.extend([Bound::Data, Bound::Key, Bound::Sendable]);
        }
        if name == "Instant" {
            t.bounds.push(Bound::Sendable);
        }
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
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
    }
    for (name, generic, variants) in [
        ("Result", "T,E", "Ok(T)|Error(E)"),
        (
            "DecodeError",
            "",
            "Version()|SchemaMismatch()|Truncated()|Trailing()|Scalar()|Utf8()|Tag()|Length()|Duplicate()|Order()",
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
            "FileError",
            "",
            "FileFault(u32)|Canceled(CancelCause)|NotGranted()|NotFound()|PermissionDenied()|AlreadyExists()|IsDirectory()|NotDirectory()|DirectoryNotEmpty()|CrossDevice()",
        ),
        ("FileKind", "", "Regular()|Directory()|Symlink()|Other()"),
        ("IpAddr", "", "V4(Inline[u8, 4])|V6(Inline[u8, 16])"),
        ("NetOperation", "", "Connect()|ConnectTls()|Listen()"),
        ("NetTimeout", "", "Unbounded()|Nanos(u64)"),
        ("Shutdown", "", "Read()|Write()|Both()"),
        ("TlsRoots", "", "WebPki()|Pem(Bytes)"),
        (
            "TlsFailure",
            "",
            "UnknownIssuer()|Expired()|NotYetValid()|NameMismatch()|Revoked()|BadCertificate()|AlertReceived(u8)|HandshakeFailure()|PeerMisbehaved()|Truncated()",
        ),
        (
            "NetError",
            "",
            "NotGranted()|Canceled(CancelCause)|InvalidInput()|UnknownHost()|ResolverFailure()|ConnectionRefused()|ConnectionReset()|ConnectionAborted()|NotConnected()|AddrInUse()|AddrNotAvailable()|HostUnreachable()|NetworkUnreachable()|NetworkDown()|BrokenPipe()|TimedOut()|Tls(TlsFailure)|Fault(u32)",
        ),
        (
            "PeerTls",
            "",
            "Plain()|Certificate(List[String])|Fails(TlsFailure)",
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
        ("ExitStatus", "", "Exited(i32)|Signaled(i32)"),
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
        ("WaitError", "", "Clock(ClockError)|Canceled(CancelCause)"),
        ("ArgError", "", "MissingArgument()|InvalidUtf8()"),
        ("FloatIntError", "", "NotFinite()|OutOfRange()"),
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
        ("StockError", "", "OutOfStock(Sku)"),
        ("TxFailure", "E", "Aborted(E)|Canceled(CancelCause)"),
        ("ShapeVisibility", "", "Private()|Public()|Opaque()"),
        (
            "ShapeBody",
            "",
            "Record(List[FieldShape])|Choice(List[VariantShape])",
        ),
    ] {
        let mut b = Builder::new(generic)
            .with_declarations(&out)
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
                    matches!(g, GenericParameter::Type { .. }).then_some(
                        AuthorityTerm::TypeParameter(TypeParameterRef::Bound {
                            depth: 0,
                            parameter: TypeParamId(i as u32),
                        }),
                    )
                })
                .collect(),
        );
        if b.generics.is_empty()
            && !matches!(
                name,
                "TlsRoots" | "PeerTls" | "NetConnection" | "NetStep" | "ChildStep" | "ShapeBody"
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
        publish(b.signature(name, SignatureKind::Type(t), false), &mut out);
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
        ("Reservation", "", "sku: Sku,count: u32"),
        (
            "Metadata",
            "",
            "kind: FileKind,size: u64,modified_unix_ns: Option[i128]",
        ),
        ("DirEntry", "", "path: Path,kind: FileKind"),
        ("SocketAddr", "", "ip: IpAddr,port: u16"),
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
            "ProcessScript",
            "",
            "program: String,args: List[String],outcome: Result[ProcessOutput,ProcessError]",
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
        ("NetClient", "", "from: SocketAddr,steps: List[NetStep]"),
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
            "address: SocketAddr,clients: List[NetClient]",
        ),
    ] {
        let mut b = Builder::new(generic)
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
        publish(b.signature(name, SignatureKind::Type(t), false), &mut out);
    }
    for (name, fields) in [("File", ""), ("AtomicFile", "dir: Dir,tmp: Path,dst: Path")] {
        let mut b = Builder::new("").with_declarations(&out);
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
            b.signature(name, SignatureKind::Type(t), fields.is_empty()),
            &mut out,
        );
    }
    // Core library network and process resources have their capability's authority and a
    // noalloc close finalizer.
    for (name, capability) in [("Stream", "Net"), ("Listener", "Net"), ("Child", "Proc")] {
        let b = Builder::new("").with_declarations(&out);
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
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
    }
    // A scope's set of task handles (CONC-2): a noncopyable run-local resource holding its
    // members' await-or-detach obligations. Its drop releases the set; its members stay
    // children of their scope, joined at its exit.
    {
        let b = Builder::new("R").with_declarations(&out);
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
            b.signature("TaskSet", SignatureKind::Type(t), true),
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
        let mut b = Builder::new(generic)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let text = format!("{name}[{}]", b.names.join(","));
        let repr = b.ty(&text, PassingMode::Take);
        let mut t = type_definition(b.generics.clone(), Some(repr));
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
        publish(b.signature(name, SignatureKind::Type(t), false), &mut out);
    }
    for name in [
        "bool", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
        "char", "String", "Bytes",
    ] {
        let mut b = Builder::new("").with_declarations(&out);
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
        let mut s = b.signature(name, SignatureKind::Type(t), false);
        s.metadata.alias = true;
        publish(s, &mut out);
        if name.starts_with(['u', 'i']) {
            let mut b = Builder::new("").with_declarations(&out);
            let ty = b.ty(&format!("Order[{name}]"), PassingMode::Take);
            publish(
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
    for signature in super::dictionaries::signatures()
        .into_iter()
        .chain(super::loops::signatures())
        .chain(super::heap::signatures())
    {
        publish(signature, &mut out);
    }
    // `@preserves_unique(p)`, then `@unretained(q, ..)` naming the borrowed
    // parameters it reads and stores nothing of, may prefix a ledger line
    // (MODE-2, CLO-2).
    let ledger = SECTION22.lines().map(|line| {
        let (preserves_unique, line) = ledger_prefix(line, "@preserves_unique(");
        let (unretained, line) = ledger_prefix(line, "@unretained(");
        (preserves_unique, unretained, line)
    });
    for (preserves_unique, unretained, line) in
        ledger.filter(|(_, _, l)| l.starts_with("fn ") || l.starts_with("N fn "))
    {
        let noalloc = line.starts_with("N ");
        let text = line.trim_start_matches("N ").trim_start_matches("fn ");
        let opening = text.find('(').unwrap();
        let head = &text[..opening];
        let (name, generics) = head
            .split_once('[')
            .map_or((head, ""), |(n, g)| (n, g.strip_suffix(']').unwrap()));
        let closing = matching(text, opening);
        let mut b = Builder::new(generics)
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
        if matches!(name, "iter.from" | "iter.map") {
            let mut result = b.types.get(f.result).unwrap().clone();
            result.erased_captures = Some(ErasedCaptures::FromArguments(
                vec![
                    ParameterRef::bound(ParamId(0)).into(),
                    ParameterRef::bound(ParamId(1)).into(),
                ]
                .into(),
            ));
            f.result = b.types.push(result);
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
            apply_state_promises(&mut b.types, &mut f.parameters, &f.state_promises);
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
        publish(
            b.signature(name, SignatureKind::Function(f), false),
            &mut out,
        );
    }
    // `net.read_into`'s compiled body passes its buffer's size to one fixed runtime
    // entry, which no size generic can name.
    let mut b = Builder::new("k: size").with_declarations(&out);
    let f = b.function(
        "n: Net, mut s: Stream of n, mut buffer: Inline[u8, k], capacity: u64",
        "Result[u64, NetError]",
        true,
    );
    publish(
        b.signature("$intrinsic.NetReadInto", SignatureKind::Function(f), false),
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
            "s: Scope, take main: once fn() -> ()",
            "Task[R] of s",
        ),
        (
            "R",
            "$intrinsic.TaskFinish",
            "mut slot: Option[R], none: Option[R], size: u64",
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
        // A message crosses as a one-element list block the runtime queues and, unreceived,
        // releases (CONC-6). The status is 0 sent or received, 1 closed, 2 + cause canceled.
        (
            "T",
            "$intrinsic.ChannelSend",
            "mut s: Sender[T], mut message: Option[List[T]]",
            "u32",
        ),
        (
            "T",
            "$intrinsic.ChannelReceive",
            "mut r: Receiver[T], mut message: Option[List[T]]",
            "u32",
        ),
        // A Mutex holds its payload as the block of a one-element list, which a create or
        // put takes from `Some` and a take moves into `None`; an Atomic holds its scalar
        // payload as the `Some` image of an `Option` of `size` bytes, which the runtime
        // copies (CONC-4, CONC-5, FAIL-4).
        (
            "T",
            "$intrinsic.MutexCreate",
            "mut payload: Option[List[T]]",
            "Mutex[T]",
        ),
        // 0: the payload moved into `payload`; 1: the mutex is dead.
        (
            "T",
            "$intrinsic.MutexTake",
            "m: Mutex[T], mut payload: Option[List[T]]",
            "u32",
        ),
        (
            "T",
            "$intrinsic.MutexPut",
            "m: Mutex[T], mut payload: Option[List[T]]",
            "()",
        ),
        ("T", "$intrinsic.MutexKill", "m: Mutex[T]", "()"),
        ("T", "$intrinsic.MutexCause", "m: Mutex[T]", "()"),
        (
            "A",
            "$intrinsic.AtomicCreate",
            "value: Option[A], size: u64",
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
        // `task.isolate`'s caught chain, read record by record to build its `Failure` (FAIL-2).
        ("", "$intrinsic.IsolateBegin", "tasks: Tasks", "u64"),
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
    ] {
        let mut b = Builder::new(generics).with_declarations(&out);
        let f = b.function(params, result, false);
        let mut signature = b.signature(name, SignatureKind::Function(f), false);
        signature.metadata.track_caller = name == "$intrinsic.TaskSpawn";
        publish(signature, &mut out);
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
            |path: &DefPath| Ok::<_, hirsel_types::QueryError>(declarations.get(path).cloned());
        match &mut normalized.kind {
            SignatureKind::Function(f) => {
                for p in &mut f.parameters {
                    p.ty = normalize_type_properties(
                        &mut normalized.types,
                        p.ty,
                        &generics,
                        &mut lookup,
                    )
                    .unwrap();
                }
                f.result = normalize_type_properties(
                    &mut normalized.types,
                    f.result,
                    &generics,
                    &mut lookup,
                )
                .unwrap();
                if let Some(error) = &mut f.error {
                    *error = normalize_type_properties(
                        &mut normalized.types,
                        *error,
                        &generics,
                        &mut lookup,
                    )
                    .unwrap();
                }
            }
            SignatureKind::Type(t) => {
                if let Some(repr) = &mut t.representation {
                    *repr = normalize_type_properties(
                        &mut normalized.types,
                        *repr,
                        &generics,
                        &mut lookup,
                    )
                    .unwrap();
                }
            }
            SignatureKind::Const { ty: t, .. } => {
                *t = normalize_type_properties(&mut normalized.types, *t, &generics, &mut lookup)
                    .unwrap()
            }
        }
        normalized.fingerprint = hirsel_types::encoding::fingerprint(&normalized, &[])
            .expect("normalized core signature");
        *signature = Arc::new(normalized);
    }
    out
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
        layout: Layout::default(),
        distinct: false,
        resource: false,
        drop: None,
        fact: None,
        destructor_allocation: Allocation::NoAlloc,
    }
}
pub(super) struct Builder {
    pub types: TypeStore,
    pub generics: Vec<GenericParameter>,
    names: Vec<String>,
    schema: Vec<SchemaConstraint>,
    declarations: BTreeMap<DefPath, SignatureRef>,
    params: Vec<String>,
    outer_params: Vec<Vec<String>>,
    generic_depth: u32,
    /// The fields of the record being built, which `of .field` brands name (PROV-5).
    siblings: Vec<String>,
}
impl Builder {
    pub fn new(generics: &str) -> Self {
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
            types: TypeStore::default(),
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
    pub(super) fn with_schema(mut self, schema: Vec<SchemaConstraint>) -> Self {
        self.schema = schema;
        self
    }
    fn with_declarations(mut self, declarations: &BTreeMap<DefPath, SignatureRef>) -> Self {
        self.declarations = declarations.clone();
        self
    }
    pub fn signature(self, name: &str, kind: SignatureKind, opaque: bool) -> Signature {
        Signature {
            definition: hirsel_syntax::DefPath::core(name),
            types: self.types,
            kind,
            fingerprint: ContentHash::default(),
            metadata: SignatureMetadata {
                visibility: if opaque {
                    Visibility::PublicOpaque
                } else {
                    Visibility::Public
                },
                roles: super::nominal_roles(),
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
                    types,
                    ..
                }) = self.declarations.get(definition).map(|s| s.as_ref())
                {
                    declared_cleanup(declaration, types, arguments, component)
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
        self.types.push(Type {
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
                        self.types.push(t)
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
                            definition.0.name.as_str(),
                            "Io" | "Args"
                                | "Env"
                                | "Unsafe"
                                | "Dir"
                                | "Stdin"
                                | "Stderr"
                                | "Stdout"
                                | "MonotonicClock"
                                | "WallClock"
                                | "Random"
                                | "Tasks"
                                | "Net"
                                | "Proc"
                                | "Scope"
                                | "Arena"
                                | "Test"
                                | "Db"
                                | "Tx"
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
            } if *definition == hirsel_syntax::DefPath::core("Result") => match &arguments[1] {
                GenericArgument::Type(t) => Some(*t),
                _ => None,
            },
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
            ("once ", |t: &mut Type| {
                t.modes.multiplicity = Multiplicity::Once
            }),
        ] {
            if let Some(rest) = text.strip_prefix(word) {
                let id = self.ty(rest, passing);
                let mut t = self.types.get(id).unwrap().clone();
                set(&mut t);
                return self.types.push(t);
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
                        definition: hirsel_syntax::DefPath::core(name),
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
        if matches!(&t.kind,TypeKind::Named{definition,..} if matches!(definition.0.name.as_str(),"Scope"|"Arena"|"Test"|"Tx"))
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
        self.types.push(t)
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
