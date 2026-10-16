//! Small declaration-data reader, intentionally independent of runtime dispatch.
use super::*;
use hirsel_syntax::Visibility;

pub(super) fn registry() -> BTreeMap<DefPath, SignatureRef> {
    let mut out = BTreeMap::new();
    // §14 DROP-4 declares this intrinsic outside the §22 library ledger.
    let mut forget = Builder::new("T");
    let signature = forget.function("take x: T", "()", true);
    publish(
        forget.signature("forget", SignatureKind::Function(signature), false),
        &mut out,
    );
    // §22 overview fixtures are declarations too, never ambient authority.
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
        "Stdout",
        "MonotonicClock",
        "WallClock",
        "Random",
        "Tasks",
        "Net",
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
        t.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(core_path(name))].into());
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
    }
    // These inert, plain views/positions have intrinsic loop/brand rules (§22).
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
                open: core_path(&format!("$intrinsic.{protocol}_open")),
                next: core_path(&format!("$intrinsic.{protocol}_next")),
                option: core_path("Option"),
            });
        }
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
    }
    for (name, generic, variants) in [
        ("Result", "T,E", "Ok(T)|Error(E)"),
        ("Option", "T", "None()|Some(T)"),
        ("CheckRule", "", "Arithmetic()|TextInvariant()|Bounds()"),
        ("CancelCause", "", "Requested()|Parent()|ChildFailed()"),
        ("CancelError", "", "Canceled(CancelCause)"),
        ("ChannelError", "", "Closed()|Canceled(CancelCause)"),
        (
            "FileError",
            "",
            "FileFault(u32)|Canceled(CancelCause)|NotGranted()",
        ),
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
        ("OutputError", "", "OutputFault(u32)|Canceled(CancelCause)"),
        ("ClockError", "", "Unavailable()|OutOfRange()"),
        ("ArgError", "", "MissingArgument()"),
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
            && !matches!(name, "TlsRoots" | "PeerTls" | "NetConnection" | "NetStep")
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
    ] {
        let mut b = Builder::new(generic)
            .with_declarations(&out)
            .with_schema(super::schema(name));
        let repr = b.record(fields);
        let t = type_definition(b.generics.clone(), Some(repr));
        publish(b.signature(name, SignatureKind::Type(t), false), &mut out);
    }
    for (name, fields) in [("File", ""), ("AtomicFile", "dir: Dir,tmp: Path,dst: Path")] {
        let mut b = Builder::new("").with_declarations(&out);
        let repr = (!fields.is_empty()).then(|| b.record(fields));
        let mut t = type_definition(vec![], repr);
        t.resource = true;
        t.bounds = vec![Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(core_path("Dir"))].into());
        t.drop = Some(core_path(name));
        publish(
            b.signature(name, SignatureKind::Type(t), fields.is_empty()),
            &mut out,
        );
    }
    // §22 network resources have Net authority and a noalloc close finalizer.
    for name in ["Stream", "Listener"] {
        let b = Builder::new("").with_declarations(&out);
        let mut t = type_definition(vec![], None);
        t.resource = true;
        t.bounds = vec![Bound::Sendable, Bound::NoallocDrop];
        t.destructor_allocation = Allocation::NoAlloc;
        t.authority =
            AuthoritySummary::Known(vec![AuthorityTerm::Capability(core_path("Net"))].into());
        t.drop = Some(core_path(name));
        publish(b.signature(name, SignatureKind::Type(t), true), &mut out);
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
        "String", "Bytes",
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
                b.signature(&format!("{name}.order"), SignatureKind::Const(ty), false),
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
    for line in SECTION22
        .lines()
        .filter(|l| l.starts_with("fn ") || l.starts_with("N fn "))
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
            }]
            .into();
            apply_state_promises(&mut b.types, &mut f.parameters, &f.state_promises);
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
            SignatureKind::Const(t) => {
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
fn type_definition(
    generics: Vec<GenericParameter>,
    representation: Option<TypeId>,
) -> TypeDefinition {
    TypeDefinition {
        loop_source: None,
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
            definition: core_path(name),
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
            kind,
            modes: Modes::global(),
            sendability_requirement: hirsel_types::SendabilityRequirement::None,
            allocation_target: None,
            erased_captures: None,
            brand: Brand::default(),
            drop_allocation,
        })
    }
    fn record(&mut self, text: &str) -> TypeId {
        let fields = split(text, ',')
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
                                | "Stdout"
                                | "MonotonicClock"
                                | "WallClock"
                                | "Random"
                                | "Tasks"
                                | "Net"
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
            } if *definition == core_path("Result") => match &arguments[1] {
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
                    || AuthorityTerm::Capability(core_path(n)),
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
                        definition: core_path(name),
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
        _ => return None,
    })
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
