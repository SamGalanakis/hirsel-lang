use super::*;
use hirsel_syntax::DefPath;

pub fn alternate_name(definition: &DefPath, instance: ContentHash) -> String {
    format!(
        "{}_ahead",
        crate::emission::symbol_name(definition, Some(&instance))
    )
}
/// The resumable state-0 alternate of a body with a Resumable plan: the ahead
/// code with the body's frame appended, which saves at a boundary as the
/// ordinary state 0 does, so the ordinary dispatcher continues it.
pub fn alternate_resume_name(definition: &DefPath, instance: ContentHash) -> String {
    format!("{}_resume", alternate_name(definition, instance))
}
pub fn metadata_name(root: ContentHash, identity: ContentHash) -> String {
    let hex = |h: ContentHash| h.0.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("hirsel_ahead_{}_{}", hex(root), hex(identity))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureRecipe {
    pub wrapper: LayoutId,
    pub source: Option<LayoutId>,
    pub wrapper_extent: u32,
    pub callable_offset: u32,
    pub live_word_offset: u32,
    pub source_extent: u32,
    pub live_mask: u64,
    pub source_code: Symbol,
    pub source_abi: crate::abi_types::FnAbiId,
    pub source_environment: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootPlan {
    pub graph: ScalarGraph,
    pub recipe: CaptureRecipe,
    pub host: Option<HostReturnGraph>,
    pub cpu: Option<CpuWorkerGraph>,
}
impl RootPlan {
    /// CPU allocation and summary calls add a saved register and return
    /// address to the fixed 64-byte handoff. No Rust service frame is saved.
    pub fn handoff_bytes(&self, ordinary: u64) -> u64 {
        if self.cpu.as_ref().is_some_and(|cpu| {
            cpu.summary.is_some()
                || cpu.mapper.as_ref().is_some_and(|mapper| {
                    !mapper.allocations.is_empty()
                        || !mapper.growth.is_empty()
                        || !mapper.regional.is_empty()
                })
        }) {
            ordinary.max(96)
        } else {
            ordinary
        }
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AheadPlan {
    pub roots: BTreeMap<ContentHash, RootPlan>,
    pub bodies: BTreeMap<ContentHash, ScalarBody>,
    pub symbols: BTreeMap<ContentHash, Symbol>,
    pub spawns: BTreeMap<(ContentHash, CallId), ContentHash>,
    /// Ownership protocols awaiting separate mapper, frame and budget proof.
    /// These rows never authorize a spawn alternate.
    pub cpu_workers: BTreeMap<ContentHash, CpuWorkerProtocol>,
    pub refusals: BTreeMap<ContentHash, String>,
    pub identity: ContentHash,
}

/// The exact environment declaration attached to each known code, collected
/// while the complete Value graph still exists. Conflicting declarations
/// cannot certify a source capture image.
pub fn environments<S>(units: &[LirUnit<S>]) -> BTreeMap<ContentHash, Option<LayoutId>> {
    let mut result = BTreeMap::new();
    for unit in units {
        for site in &unit.tables.closures {
            if let Some(code) = site.code
                && let Some(id) = unit.tables.symbols[code.symbol.index()].instance
            {
                result
                    .entry(id)
                    .and_modify(|layout| {
                        if *layout != Some(site.environment) {
                            *layout = None;
                        }
                    })
                    .or_insert(Some(site.environment));
            }
        }
    }
    result
}

/// Bounded discovery only. Ready validation and the scalar/envelope proof
/// still decide eligibility. Materialize these bodies even on ordinary cache
/// or std-seed hits; an absent Ready body never borrows an old certificate.
pub fn prospective<S>(units: &[LirUnit<S>]) -> BTreeSet<usize> {
    let index: BTreeMap<_, _> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.header.instance, i))
        .collect();
    let mut all = BTreeSet::new();
    for (root, unit) in units.iter().enumerate() {
        let cpu = unit.header.definition.is_core("$native.cpu_worker");
        if !cpu
            && (!unit.header.definition.is_core("$native.task_main")
                || unit
                    .tables
                    .calls
                    .iter()
                    .any(|c| matches!(c.callee, Callee::Indirect(_))))
        {
            continue;
        }
        let mut pending = vec![root];
        let mut reached = BTreeSet::new();
        let mut instructions = 0usize;
        while let Some(i) = pending.pop() {
            if !reached.insert(i) {
                continue;
            }
            instructions = instructions
                .saturating_add(units[i].instructions.len())
                .saturating_add(units[i].terminators.len());
            if reached.len() > MAX_UNITS || instructions > MAX_INSTRUCTIONS {
                reached.clear();
                break;
            }
            for call in &units[i].tables.calls {
                if let Callee::Direct(s) | Callee::Bound { code: s, .. } = call.callee
                    && let Some(id) = units[i].tables.symbols[s.index()].instance
                    && let Some(&j) = index.get(&id)
                {
                    pending.push(j);
                }
            }
        }
        all.extend(reached);
    }
    let roots: BTreeSet<_> = all
        .iter()
        .filter_map(|&i| {
            units[i]
                .header
                .definition
                .is_core("$native.task_main")
                .then_some(units[i].header.instance)
                .or_else(|| {
                    units[i]
                        .header
                        .definition
                        .is_core("$native.cpu_worker")
                        .then_some(units[i].header.instance)
                })
        })
        .collect();
    for (i, u) in units.iter().enumerate() {
        if u.tables.closures.iter().any(|c| {
            c.code
                .and_then(|c| u.tables.symbols[c.symbol.index()].instance)
                .is_some_and(|id| roots.contains(&id))
        }) {
            all.insert(i);
        }
    }
    all
}

impl AheadPlan {
    pub fn build(
        units: &[&ValidatedUnit<Ready>],
        table: &LayoutTable,
        target: &TargetSpec,
        envs: &BTreeMap<ContentHash, Option<LayoutId>>,
        fuel: bool,
        helpers: ContentHash,
    ) -> Self {
        let mut plan = Self::default();
        for unit in units {
            let validated = *unit;
            let unit = validated.unit();
            plan.symbols.insert(
                unit.header.instance,
                Symbol {
                    kind: SymbolKind::Function,
                    definition: unit.header.definition.clone(),
                    instance: Some(unit.header.instance),
                },
            );
            if unit.header.definition.is_core("$native.cpu_worker") {
                if let Some(protocol) = CpuWorkerProtocol::derive(validated, table) {
                    plan.cpu_workers.insert(unit.header.instance, protocol);
                } else {
                    plan.refusals.insert(
                        unit.header.instance,
                        "unproved CPU worker ownership protocol".into(),
                    );
                }
                match super::cpu_worker::propose(
                    units, table, target, validated, envs, fuel, helpers,
                ) {
                    Ok(root) => {
                        plan.bodies.extend(root.graph.bodies.clone());
                        plan.roots.insert(unit.header.instance, root);
                    }
                    Err(error) => {
                        plan.refusals.insert(unit.header.instance, error.reason);
                    }
                }
                continue;
            }
            if !unit.header.definition.is_core("$native.task_main") {
                continue;
            }
            let attempt = (|| {
                let scalar = envelope(unit, table, envs, true).and_then(|(recipe, source)| {
                    scalar_graph(units, table, target, source, fuel, helpers)
                        .map(|graph| (recipe, graph, None))
                });
                let (recipe, mut graph, mut host) = match scalar {
                    Ok(scalar) => scalar,
                    Err(scalar_error) => {
                        let (recipe, source) =
                            envelope(unit, table, envs, false).map_err(|_| scalar_error)?;
                        let host = super::host_return::propose(
                            units, table, target, source, fuel, helpers,
                        )?;
                        (recipe, host.graph.clone(), Some(host))
                    }
                };
                let source = graph.root;
                if graph.bodies.len() == MAX_UNITS
                    || graph
                        .bodies
                        .values()
                        .any(|b| b.calls.contains(&unit.header.instance))
                {
                    return Err(refused(
                        unit.header.instance,
                        "recursive/oversize wrapper graph",
                    ));
                }
                let count: usize = units
                    .iter()
                    .filter(|u| graph.bodies.contains_key(&u.unit().header.instance))
                    .map(|u| u.unit().instructions.len() + u.unit().terminators.len())
                    .sum();
                if count + unit.instructions.len() + unit.terminators.len() > MAX_INSTRUCTIONS {
                    return Err(refused(unit.header.instance, "oversize wrapper graph"));
                }
                graph.bodies.insert(
                    unit.header.instance,
                    ScalarBody {
                        body: unit.digest(),
                        calls: BTreeSet::from([source]),
                        fuel_points: crate::fuel::fuel_points(unit),
                    },
                );
                graph.root = unit.header.instance;
                graph.identity = ContentHash::of_parts(
                    "hirsel.ahead.wrapper-graph.v1",
                    [
                        graph.identity.0.as_slice(),
                        unit.header.instance.0.as_slice(),
                        unit.digest().0.as_slice(),
                        format!("{:?}", recipe).as_bytes(),
                    ],
                );

                if let Some(host) = &mut host {
                    host.spines.insert(
                        unit.header.instance,
                        super::host_return::wrapper_spine(unit),
                    );
                    graph.identity = ContentHash::of_parts(
                        "hirsel.ahead.host-callers.v1",
                        [
                            graph.identity.0.as_slice(),
                            format!("{:?}", host.spines).as_bytes(),
                        ],
                    );
                    host.graph = graph.clone();
                }
                Ok(RootPlan {
                    graph,
                    recipe,
                    host,
                    cpu: None,
                })
            })();
            match attempt {
                Ok(root) => {
                    plan.bodies.extend(root.graph.bodies.clone());
                    plan.roots.insert(unit.header.instance, root);
                }
                Err(error) => {
                    plan.refusals.insert(unit.header.instance, error.reason);
                }
            }
        }
        // A shared alternate has one ordinal source. Refuse conflicting CPU
        // roots before emitting any body, so ordinary compilation still works.
        let conflicting: Vec<_> = plan
            .roots
            .iter()
            .filter_map(|(id, root)| {
                let mapper = root.cpu.as_ref()?.mapper.as_ref()?;
                mapper
                    .growth
                    .keys()
                    .chain(mapper.regional.keys())
                    .any(|&(body, _)| {
                        plan.cpu_ordinal(body, table).is_none()
                            || plan.roots.values().any(|other| {
                                other.cpu.is_none() && other.graph.bodies.contains_key(&body)
                            })
                    })
                    .then_some(*id)
            })
            .collect();
        if !conflicting.is_empty() {
            for id in conflicting {
                plan.roots.remove(&id);
                plan.refusals
                    .insert(id, "ambiguous CPU growth ordinal custody".into());
            }
            plan.bodies = plan
                .roots
                .values()
                .flat_map(|root| root.graph.bodies.clone())
                .collect();
        }
        // Discover known wrapper values with the same monotone SSA join used
        // by code-target specialization, including storage and CFG arguments.
        for unit in units {
            let u = unit.unit();
            let codes = code_values(u, &[]);
            for (i, call) in u.tables.calls.iter().enumerate() {
                if !matches!(call.callee,Callee::Operation(op) if u.tables.operations[op.index()].definition.is_core("$intrinsic.TaskSpawn"))
                {
                    continue;
                }
                if let Some(ArgumentValue::Value(v)) = call
                    .arguments
                    .get(&u.pools.call_arguments)
                    .and_then(|args| args.iter().find(|a| a.parameter == 1))
                    .map(|a| a.value)
                    && let Some(Code::Known(root)) = codes.get(v.index())
                    && plan.roots.contains_key(root)
                {
                    plan.spawns
                        .insert((u.header.instance, CallId(i as u32)), *root);
                }
            }
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&helpers.0);
        bytes.extend_from_slice(&target.digest().0);
        bytes.extend_from_slice(&table.digest().0);
        bytes.push(u8::from(fuel));
        for (id, root) in &plan.roots {
            bytes.extend_from_slice(&id.0);
            bytes.extend_from_slice(&root.graph.identity.0);
            bytes.extend_from_slice(
                format!("{:?}{:?}{:?}", root.recipe, root.host, root.cpu).as_bytes(),
            );
            for (id, body) in &root.graph.bodies {
                bytes.extend_from_slice(&id.0);
                bytes.extend_from_slice(&body.body.0);
            }
        }
        for (id, protocol) in &plan.cpu_workers {
            bytes.extend_from_slice(&id.0);
            bytes.extend_from_slice(format!("{protocol:?}").as_bytes());
        }
        plan.identity = ContentHash::of_parts("hirsel.ahead.plan.v2", [bytes.as_slice()]);
        plan
    }
}

fn whole(u: &LirUnit<Ready>, a: AddressId) -> Option<SlotId> {
    let a = &u.addresses[a.index()];
    if a.projection.len != 0 {
        return None;
    }
    match a.base {
        AddressBase::Slot(s) => Some(s),
        AddressBase::Address(a) => whole(u, a),
        _ => None,
    }
}
fn envelope(
    u: &LirUnit<Ready>,
    table: &LayoutTable,
    envs: &BTreeMap<ContentHash, Option<LayoutId>>,
    inert: bool,
) -> Result<(CaptureRecipe, ContentHash), Refusal> {
    let no = |reason: &str| refused(u.header.instance, reason);
    if u.header.origin != UnitOrigin::Library
        || u.tables.calls.len() != 2
        || !u.parameters.is_empty()
    {
        return Err(no("unknown task_main envelope"));
    }
    let source_call = &u.tables.calls[0];
    let (symbol, has_env) = match source_call.callee {
        Callee::Bound { code, .. } => (code, true),
        Callee::Direct(s) => (s, false),
        _ => return Err(no("erased task_main callable")),
    };
    let source = u.tables.symbols[symbol.index()].clone();
    let source_id = source
        .instance
        .ok_or_else(|| no("unknown task callable instance"))?;
    if !matches!(u.tables.calls[1].callee,Callee::Operation(op) if u.tables.operations[op.index()].definition.is_core("$intrinsic.TaskFinish"))
    {
        return Err(no("unknown task finish protocol"));
    }
    let flag = u
        .tables
        .init_flags
        .iter()
        .find(|f| f.environment.is_some())
        .ok_or_else(|| no("missing wrapper live word"))?;
    let wrapper = flag.environment.unwrap();
    let layout = table
        .layout(wrapper)
        .ok_or_else(|| no("missing wrapper layout"))?;
    if !matches!(&layout.kind,LayoutKind::Environment{movable} if movable==&[0])
        || layout.fields.len() != 2
        || layout.finalizer.is_some()
    {
        return Err(no("unknown wrapper ownership shape"));
    }
    let callable = &layout.fields[0];
    let live = &layout.fields[1];
    if !matches!(
        table.layout(callable.layout).map(|l| &l.kind),
        Some(LayoutKind::Callable { .. })
    ) || !matches!(
        table.layout(live.layout).map(|l| &l.kind),
        Some(LayoutKind::Scalar(ScalarKind::U64))
    ) {
        return Err(no("unknown wrapper fields"));
    }
    let capture0 = u
        .slots
        .iter()
        .position(|s| s.kind == SlotKind::Capture(0))
        .map(|i| SlotId(i as u32))
        .ok_or_else(|| no("missing callable capture"))?;
    let capture1 = u
        .slots
        .iter()
        .position(|s| s.kind == SlotKind::Capture(1))
        .map(|i| SlotId(i as u32))
        .ok_or_else(|| no("missing live capture"))?;
    if flag.slot != capture1 || flag.path.len != 0 {
        return Err(no("unknown wrapper live-mask binding"));
    }
    if u.values
        .iter()
        .map(|v| v.ty)
        .chain(u.slots.iter().map(|s| s.ty))
        .any(|ty| {
            let ty = &u.types[ty.index()];
            if ty.layout == callable.layout {
                ty.repr != ValueRepr::Callable
            } else {
                !scalar_image(table, ty.layout)
                    || !matches!(
                        ty.repr,
                        ValueRepr::Unit
                            | ValueRepr::Scalar(_)
                            | ValueRepr::Aggregate
                            | ValueRepr::InitMask { .. }
                    )
            }
        })
    {
        return Err(no("non-inline wrapper image"));
    }
    let mut moved = None;
    let mut old = None;
    let mut cleared = None;
    let mut stored = 0;
    for inst in &u.instructions {
        match *inst {
            Inst::Read {
                result,
                address,
                mode: ReadMode::Take,
                ..
            } if whole(u, address) == Some(capture0) => {
                if moved.replace(result).is_some() {
                    return Err(no("multiple wrapper callable moves"));
                }
            }
            Inst::Read {
                result,
                address,
                mode: ReadMode::Bits,
                ..
            } if whole(u, address) == Some(capture1) => {
                old = Some(result);
            }
            Inst::UpdateMask {
                result,
                mask,
                field: 0,
                initialized: false,
            } if Some(mask) == old => {
                if cleared.replace(result).is_some() {
                    return Err(no("multiple live-mask changes"));
                }
            }
            Inst::Store { address, value, .. }
                if whole(u, address) == Some(capture1) && Some(value) == cleared =>
            {
                stored += 1;
            }
            Inst::Store { address, .. } | Inst::Replace { address, .. }
                if [Some(capture0), Some(capture1)].contains(&whole(u, address)) =>
            {
                return Err(no("unexpected wrapper capture mutation"));
            }
            Inst::Release { .. } => {} // Alternate lowering barriers before every release.
            Inst::Construct { construct, .. }
                if scalar_image(table, u.tables.constructs[construct.index()].layout) => {}
            Inst::Const { constant, .. }
                if matches!(
                    u.tables.constants[constant.index()].value,
                    ConstantValue::Unit | ConstantValue::Integer(_) | ConstantValue::Descriptor(_)
                ) => {}
            Inst::FrameEnter { .. }
            | Inst::FrameLeave { .. }
            | Inst::Address { .. }
            | Inst::Read { .. }
            | Inst::StorageLive { .. }
            | Inst::StorageDead { .. }
            | Inst::Store { .. }
            | Inst::BeginLoan { .. }
            | Inst::EndLoan { .. }
            | Inst::BindOrigin { .. }
            | Inst::EndOrigin { .. }
            | Inst::EndScope { .. } => {}
            _ => return Err(no("unrecognized task_main instruction")),
        }
    }
    if moved.is_none() || cleared.is_none() || stored != 1 {
        return Err(no("missing callable move/live-mask transition"));
    }
    // Bind the source invocation and every callable release to this exact move.
    let aliases = code_values(u, &[(moved.unwrap(), source_id)]);
    if let Callee::Bound { closure, .. } = source_call.callee
        && aliases[closure.index()] != Code::Known(source_id)
    {
        return Err(no("source call is not the moved wrapper callable"));
    }
    for inst in &u.instructions {
        if let Inst::Release {
            value,
            layout,
            live,
            ..
        } = inst
            && (*layout != callable.layout
                || !matches!(live, MaskRef::All)
                || aliases[value.index()] != Code::Known(source_id))
        {
            return Err(no("unknown wrapper release"));
        }
    }
    if u.addresses
        .iter()
        .enumerate()
        .any(|(i, _)| whole(u, AddressId(i as u32)).is_none())
    {
        return Err(no("nonlocal wrapper address"));
    }
    if u.slots.iter().any(|slot| {
        let ty = &u.types[slot.ty.index()];
        ty.layout != callable.layout && !scalar_image(table, ty.layout)
    }) {
        return Err(no("unknown wrapper local image"));
    }
    if u.terminators
        .iter()
        .filter(|t| {
            matches!(
                t,
                Terminator::Call {
                    call: CallId(0),
                    ..
                }
            )
        })
        .count()
        != 1
        || u.terminators
            .iter()
            .filter(|t| {
                matches!(
                    t,
                    Terminator::Call {
                        call: CallId(1),
                        ..
                    }
                )
            })
            .count()
            != 1
        || crate::fuel::fuel_points(u).iter().filter(|p| **p).count() != 1
    {
        return Err(no("repeated or cyclic wrapper protocol"));
    }
    let abi = &table.fn_abis[source_call.abi.0 as usize];
    if !abi.parameters.is_empty()
        || abi
            .result
            .as_ref()
            .is_some_and(|r| !scalar_image(table, r.layout))
    {
        return Err(no("owned/non-scalar task result"));
    }
    // task_main is a trusted generated adapter, but bind its held result and
    // complete call protocol rather than licensing an arbitrary library body.
    if u.tables.constructs.len() != 2
        || u.tables
            .constructs
            .iter()
            .any(|c| !scalar_image(table, c.layout))
        || !u.tables.constructs.iter().any(|c| {
            matches!(c.constructor, ConstructorKind::Variant { variant: 1, .. })
                && c.fields.len == 1
        })
        || !u.tables.constructs.iter().any(|c| {
            matches!(c.constructor, ConstructorKind::Variant { variant: 0, .. })
                && c.fields.len == 0
        })
        || u.terminators.iter().any(|t| {
            !matches!(
                t,
                Terminator::Call { .. }
                    | Terminator::Jump(_)
                    | Terminator::Return(_)
                    | Terminator::Propagate
                    | Terminator::Branch { .. }
            )
        })
    {
        return Err(no("unrecognized held Option/cleanup protocol"));
    }
    let some = u
        .tables
        .constructs
        .iter()
        .position(|c| matches!(c.constructor, ConstructorKind::Variant { variant: 1, .. }))
        .unwrap();
    let none = 1 - some;
    let option = u.tables.constructs[some].layout;
    if u.tables.constructs[none].layout != option {
        return Err(no("different held Option layouts"));
    }
    let fields = u.tables.constructs[some]
        .fields
        .get(&u.pools.fields)
        .unwrap_or(&[]);
    if fields.len()!=1 || fields[0].field!=0 || !match source_call.result {
        Some(value)=>fields[0].value==value,
        None=>u.instructions.iter().any(|i|matches!(i,Inst::Const{result,constant} if *result==fields[0].value && matches!(u.tables.constants[constant.index()].value,ConstantValue::Unit))),
    } {return Err(no("held Option does not hold the source result"));}
    let constructed = |id: usize| {
        u.instructions.iter().find_map(|i| match *i {
            Inst::Construct {
                result, construct, ..
            } if construct.index() == id => Some(result),
            _ => None,
        })
    };
    let (Some(some), Some(none)) = (constructed(some), constructed(none)) else {
        return Err(no("missing held Option values"));
    };
    let finish = &u.tables.calls[1];
    let args = finish.arguments.get(&u.pools.call_arguments).unwrap_or(&[]);
    let arg = |p| args.iter().find(|a| a.parameter == p).map(|a| a.value);
    let Some(ArgumentValue::Address { address, .. }) = arg(0) else {
        return Err(no("missing held result address"));
    };
    let Some(held) = whole(u, address) else {
        return Err(no("nonlocal held result"));
    };
    if !u.instructions.iter().any(|i|matches!(i,Inst::Store{address,value,..} if whole(u,*address)==Some(held) && *value==some)) {
        return Err(no("held result address does not own Some"));
    }
    let none_key = ContentHash::of_bytes(b"hirsel.ahead.none-origin");
    let facts = code_values(u, &[(none, none_key)]);
    if !matches!(arg(1),Some(ArgumentValue::Value(v)) if facts[v.index()]==Code::Known(none_key)) {
        return Err(no("missing held None replacement"));
    }
    let constant = |p| match arg(p) {
        Some(ArgumentValue::Value(v)) => u.instructions.iter().find_map(|i| match *i {
            Inst::Const { result, constant } if result == v => {
                Some(&u.tables.constants[constant.index()].value)
            }
            _ => None,
        }),
        _ => None,
    };
    if args.len() != 4
        || !matches!(constant(2),Some(ConstantValue::Integer(n)) if matches!(table.layout(option).map(|l|&l.size),Some(SizeSpec::Fixed(size)) if u128::from(*size)==*n))
        || !matches!(constant(3),Some(ConstantValue::Descriptor(l)) if *l==option)
    {
        return Err(no("unknown TaskFinish result descriptor/extent"));
    }
    let source_layout = if has_env {
        Some(
            envs.get(&source_id)
                .copied()
                .flatten()
                .ok_or_else(|| no("missing/conflicting source environment"))?,
        )
    } else {
        None
    };
    if inert && source_layout.is_some_and(|l| !scalar_image(table, l)) {
        return Err(no("non-inert source captures"));
    }
    let extent = |l| match table.layout(l).map(|l| &l.size) {
        Some(SizeSpec::Fixed(n)) => u32::try_from(*n).ok(),
        _ => None,
    };
    let wrapper_extent = extent(wrapper)
        .filter(|n| u64::from(*n) <= MAX_IMAGE_BYTES)
        .ok_or_else(|| no("unbounded wrapper"))?;
    Ok((
        CaptureRecipe {
            wrapper,
            source: source_layout,
            wrapper_extent,
            callable_offset: u32::try_from(callable.offset)
                .map_err(|_| no("callable offset overflow"))?,
            live_word_offset: u32::try_from(live.offset).map_err(|_| no("live offset overflow"))?,
            source_extent: source_layout.and_then(extent).unwrap_or(0),
            live_mask: 1,
            source_code: source,
            source_abi: source_call.abi,
            source_environment: has_env,
        },
        source_id,
    ))
}

// The three-point lattice mirrors closure_calls::discover. Unknown inputs
// widen a join to Dynamic; they are never treated as an absent predecessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Code {
    Pending,
    Known(ContentHash),
    Dynamic,
}
impl Code {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Pending, x) | (x, Self::Pending) => x,
            (Self::Known(a), Self::Known(b)) if a == b => self,
            _ => Self::Dynamic,
        }
    }
}
fn code_values(u: &LirUnit<Ready>, seeds: &[(ValueId, ContentHash)]) -> Vec<Code> {
    code_values_inner(u, seeds, false)
}

pub(super) fn known_code(
    unit: &LirUnit<Ready>,
    value: ValueId,
    seeds: &[(ValueId, ContentHash)],
    frozen: bool,
) -> Option<ContentHash> {
    match code_values_inner(unit, seeds, frozen).get(value.index())? {
        Code::Known(code) => Some(*code),
        _ => None,
    }
}

fn code_values_inner(
    u: &LirUnit<Ready>,
    seeds: &[(ValueId, ContentHash)],
    frozen: bool,
) -> Vec<Code> {
    let mut inputs = vec![Vec::new(); u.values.len()];
    let mut constants = vec![Code::Dynamic; u.values.len()];
    let mutable: BTreeSet<_> = u
        .tables
        .loans
        .iter()
        .filter(|l| l.mutable)
        .filter_map(|l| whole(u, l.address))
        .collect();
    let safe = |a| whole(u, a).is_some_and(|s| !mutable.contains(&s));
    for edge in &u.tables.edges {
        for (&p, &a) in u.blocks[edge.target.index()]
            .parameters
            .get(&u.pools.values)
            .unwrap_or(&[])
            .iter()
            .zip(edge.arguments.get(&u.pools.values).unwrap_or(&[]))
        {
            constants[p.index()] = Code::Pending;
            inputs[p.index()].push(a);
        }
    }
    for inst in &u.instructions {
        let transfer = match *inst {
            Inst::Read {
                result,
                address,
                reaching,
                ..
            }
            | Inst::Copy {
                result,
                address,
                reaching,
                ..
            } if safe(address) => Some((result, reaching)),
            Inst::Store {
                address,
                value,
                version,
            }
            | Inst::Replace {
                address,
                value,
                version,
                ..
            } if safe(address) => Some((version, value)),
            Inst::CopyValue { result, value } | Inst::Weaken { result, value } => {
                Some((result, value))
            }
            Inst::Freeze { result, value } if frozen => Some((result, value)),
            _ => None,
        };
        if let Some((to, from)) = transfer {
            constants[to.index()] = Code::Pending;
            inputs[to.index()].push(from);
        } else {
            for v in Inst::defs(inst) {
                constants[v.index()] = Code::Dynamic;
            }
            let known = match *inst {
                Inst::Closure {
                    result, closure, ..
                } => Some((
                    result,
                    u.tables.closures[closure.index()]
                        .code
                        .and_then(|c| u.tables.symbols[c.symbol.index()].instance),
                )),
                Inst::FunctionRef { result, symbol } => {
                    Some((result, u.tables.symbols[symbol.index()].instance))
                }
                _ => None,
            };
            if let Some((v, Some(id))) = known {
                constants[v.index()] = Code::Known(id);
            }
        }
    }
    for &(value, id) in seeds {
        constants[value.index()] = Code::Known(id);
        inputs[value.index()].clear();
    }
    let mut codes = constants.clone();
    let mut users = vec![Vec::new(); codes.len()];
    for (i, args) in inputs.iter().enumerate() {
        for a in args {
            users[a.index()].push(i);
        }
    }
    let mut queue: std::collections::VecDeque<_> = (0..codes.len()).collect();
    let mut queued = vec![true; codes.len()];
    while let Some(i) = queue.pop_front() {
        queued[i] = false;
        let code = inputs[i]
            .iter()
            .fold(constants[i], |c, v| c.join(codes[v.index()]));
        if code != codes[i] {
            codes[i] = code;
            for &j in &users[i] {
                if !std::mem::replace(&mut queued[j], true) {
                    queue.push_back(j);
                }
            }
        }
    }
    codes
}

impl AheadPlan {
    /// Sharing an emitted body does not add a private edge to another root.
    pub fn has_private_edge(&self, caller: ContentHash, callee: ContentHash) -> bool {
        self.roots.values().any(|root| {
            root.graph.bodies.contains_key(&callee)
                && root
                    .graph
                    .bodies
                    .get(&caller)
                    .is_some_and(|body| body.calls.contains(&callee))
        })
    }
    pub fn host_spine(&self, id: ContentHash) -> Option<&Spine> {
        self.roots
            .values()
            .filter_map(|r| r.host.as_ref())
            .find_map(|h| h.spines.get(&id))
            .or_else(|| self.roots.get(&id)?.cpu.as_ref().map(|cpu| &cpu.spine))
            .or_else(|| {
                self.roots
                    .values()
                    .filter_map(|r| r.cpu.as_ref()?.mapper.as_ref())
                    .find_map(|mapper| mapper.spines.get(&id))
            })
    }
    /// The ordinal is read from the original worker capture, including in a
    /// private helper. Conflicting capture offsets cannot share an alternate.
    pub fn cpu_growth(
        &self,
        id: ContentHash,
        instruction: usize,
        table: &LayoutTable,
    ) -> Option<(&CpuGrowth, u64)> {
        let mut found = None;
        for cpu in self.roots.values().filter_map(|root| root.cpu.as_ref()) {
            let Some(recipe) = cpu
                .mapper
                .as_ref()
                .and_then(|m| m.growth.get(&(id, instruction)))
            else {
                continue;
            };
            let wrapper = table.layout(cpu.binding.worker.environment)?;
            let offset = crate::layout::payload_offset(wrapper.align)
                .ok()?
                .checked_add(wrapper.fields.get(4)?.offset)?;
            if found
                .as_ref()
                .is_some_and(|previous| *previous != (recipe, offset))
            {
                return None;
            }
            found = Some((recipe, offset));
        }
        found
    }
    pub fn cpu_ordinal(&self, id: ContentHash, table: &LayoutTable) -> Option<u64> {
        let mut found = None;
        for cpu in self
            .roots
            .values()
            .filter_map(|r| r.cpu.as_ref())
            .filter(|cpu| {
                cpu.mapper
                    .as_ref()
                    .is_some_and(|m| m.graph.bodies.contains_key(&id))
            })
        {
            let wrapper = table.layout(cpu.binding.worker.environment)?;
            let offset = crate::layout::payload_offset(wrapper.align)
                .ok()?
                .checked_add(wrapper.fields.get(4)?.offset)?;
            if found.is_some_and(|old| old != offset) {
                return None;
            }
            found = Some(offset);
        }
        found
    }
    pub fn cpu_regional(&self, id: ContentHash, instruction: usize) -> Option<&CpuRegional> {
        self.roots
            .values()
            .filter_map(|r| r.cpu.as_ref()?.mapper.as_ref())
            .find_map(|m| m.regional.get(&(id, instruction)))
    }
    /// A dropless sequence release can run in the job's private allocation
    /// lists. The checked mapper graph still binds its exact instruction.
    pub fn cpu_release(
        &self,
        unit: &LirUnit<Ready>,
        instruction: usize,
        table: &LayoutTable,
    ) -> Option<(ValueId, LayoutId)> {
        let Inst::Release {
            value,
            layout,
            live: MaskRef::All,
            ..
        } = *unit.instructions.get(instruction)?
        else {
            return None;
        };
        let permitted = self
            .roots
            .values()
            .filter_map(|r| r.cpu.as_ref()?.mapper.as_ref())
            .any(|mapper| {
                mapper
                    .cleanup
                    .contains(&(unit.header.instance, instruction))
            });
        if !permitted {
            return None;
        }
        let ty = unit.types.get(unit.values.get(value.index())?.ty.index())?;
        if ty.repr != ValueRepr::Block
            || !matches!(table.layout(ty.layout)?.kind, LayoutKind::Block { payload } if payload == layout)
        {
            return None;
        }
        // Release names the payload layout, as checked by Ready validation.
        let record = table.layout(layout)?;
        let LayoutKind::Sequence { element } = record.kind else {
            return None;
        };
        (record.finalizer.is_none()
            && record.drop.is_empty()
            && record.scan.is_empty()
            && scalar_image(table, element))
        .then_some((value, layout))
    }
    pub fn cpu_allocation(&self, id: ContentHash, instruction: usize) -> bool {
        self.roots
            .values()
            .filter_map(|r| r.cpu.as_ref()?.mapper.as_ref())
            .any(|mapper| mapper.allocations.contains_key(&(id, instruction)))
    }
    /// Route a known local callable only along the independently checked CPU
    /// graph edge. Ordinary callers retain their ordinary code entry.
    pub fn cpu_callee(&self, unit: &LirUnit<Ready>, call: CallId) -> Option<&Symbol> {
        if let Some(symbol) = self.cpu_mapper(unit.header.instance, call) {
            return Some(symbol);
        }
        let Callee::Indirect(value) = unit.tables.calls.get(call.index())?.callee else {
            return None;
        };
        let callee = known_code(unit, value, &[], false)?;
        self.cpu_private_edge(unit.header.instance, callee)
            .then(|| self.symbols.get(&callee))
            .flatten()
    }
    pub fn cpu_private_edge(&self, caller: ContentHash, callee: ContentHash) -> bool {
        self.roots.values().any(|root| {
            root.cpu
                .as_ref()
                .and_then(|cpu| cpu.mapper.as_ref())
                .and_then(|mapper| mapper.graph.bodies.get(&caller))
                .is_some_and(|body| body.calls.contains(&callee))
        })
    }
    pub fn cpu_mapper(&self, unit: ContentHash, call: CallId) -> Option<&Symbol> {
        let cpu = self.roots.get(&unit)?.cpu.as_ref()?;
        (cpu.binding.worker.mapper == call).then_some(&cpu.binding.callback)
    }
    /// Only the checked worker's constant entry/claim observations are local.
    pub fn cpu_observation(&self, unit: ContentHash, call: CallId) -> bool {
        self.roots
            .get(&unit)
            .and_then(|root| root.cpu.as_ref())
            .is_some_and(|cpu| cpu.binding.worker.observations.contains(&call))
    }
    pub fn cpu_claim(&self, unit: ContentHash, call: CallId) -> bool {
        self.roots
            .get(&unit)
            .and_then(|root| root.cpu.as_ref())
            .is_some_and(|cpu| cpu.binding.worker.claim == Some(call))
    }
    pub fn host_read(&self, id: ContentHash, call: CallId) -> Option<&ReadContinuation> {
        self.roots
            .values()
            .filter_map(|r| r.host.as_ref())
            .find_map(|h| h.reads.get(&(id, call)))
    }
}
