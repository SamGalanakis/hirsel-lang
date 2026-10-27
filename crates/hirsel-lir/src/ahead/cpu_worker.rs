//! CPU worker proposals bind the Shared mapper loan and consuming job cursor
//! separately from scalar spawn wrappers. Only a proved scalar mapper segment
//! runs privately; worker allocation, publication and cleanup remain barriers.
use super::*;
use hirsel_core_contract::Passing;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CpuWorkerGraph {
    pub binding: CpuMapperBinding,
    pub shared_payload: LayoutId,
    pub spine: Spine,
}

/// Recheck CPU work at each backend's public entry. Pointer matching in the
/// runtime supplies the actual callback identity; these checks supply its
/// complete effect-free Ready graph and the worker's retained ownership.
pub fn validate_cpu_workers(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    plan: &AheadPlan,
    fuel: bool,
) -> bool {
    for (root, proposal) in &plan.roots {
        let Some(cpu) = &proposal.cpu else { continue };
        let Some(worker) = units
            .iter()
            .find(|unit| unit.unit().header.instance == *root)
        else {
            return false;
        };
        let Some(protocol) = CpuWorkerProtocol::derive(worker, table) else {
            return false;
        };
        if protocol != cpu.binding.worker
            || proposal.host.is_some()
            || proposal.graph.root != *root
            || cpu.binding.callback.instance.is_none()
            || proposal.recipe.source_code != cpu.binding.callback
            || proposal.recipe.source != cpu.binding.environment
            || proposal
                .recipe
                .source
                .is_none_or(|layout| !scalar_image(table, layout))
        {
            return false;
        }
        let callback = cpu.binding.callback.instance.unwrap();
        let Some(callback_unit) = units
            .iter()
            .find(|unit| unit.unit().header.instance == callback)
        else {
            return false;
        };
        let callback_unit = callback_unit.unit();
        let expected =
            &table.fn_abis[worker.unit().tables.calls[protocol.mapper.index()].abi.0 as usize];
        let actual = &table.fn_abis[callback_unit.header.abi.0 as usize];
        if cpu.binding.callback.definition != callback_unit.header.definition
            || expected.parameters != actual.parameters
            || expected.result != actual.result
            || expected.context != actual.context
            || expected.destination != actual.destination
            || expected.caller_site != actual.caller_site
            || !actual.environment
            || proposal.recipe.source_abi != worker.unit().tables.calls[protocol.mapper.index()].abi
        {
            return false;
        }
        let Ok(mut graph) =
            scalar_graph(units, table, target, callback, fuel, ContentHash::default())
        else {
            return false;
        };
        if graph
            .bodies
            .get(&callback)
            .is_none_or(|body| body.body != cpu.binding.callback_digest)
        {
            return false;
        }
        graph.bodies.insert(
            *root,
            ScalarBody {
                body: worker.unit().digest(),
                calls: BTreeSet::from([callback]),
                fuel_points: crate::fuel::fuel_points(worker.unit()),
            },
        );
        if graph.bodies != proposal.graph.bodies || cpu.spine != spine(worker.unit(), &protocol) {
            return false;
        }
        let Some(wrapper) = table.layout(protocol.environment) else {
            return false;
        };
        if !matches!(table.layout(wrapper.fields[0].layout).map(|layout| &layout.kind),
            Some(LayoutKind::Shared { payload }) if *payload == cpu.shared_payload)
        {
            return false;
        }
        let recipe = &proposal.recipe;
        let source = table.layout(recipe.source.unwrap()).unwrap();
        if recipe.wrapper != protocol.environment
            || recipe.live_mask != 1
            || !recipe.source_environment
            || u64::from(recipe.callable_offset) != wrapper.fields[0].offset
            || u64::from(recipe.live_word_offset) != wrapper.fields[5].offset
            || wrapper.size != SizeSpec::Fixed(u64::from(recipe.wrapper_extent))
            || source.size != SizeSpec::Fixed(u64::from(recipe.source_extent))
            || u64::from(recipe.wrapper_extent) > MAX_IMAGE_BYTES
            || proposal.graph.bodies.len() > MAX_UNITS
        {
            return false;
        }
    }
    true
}

fn spine(u: &LirUnit<Ready>, protocol: &CpuWorkerProtocol) -> Spine {
    let private_instructions: Vec<_> = u
        .instructions
        .iter()
        .map(|inst| {
            matches!(
                inst,
                Inst::FrameEnter { .. }
                    | Inst::FrameLeave
                    | Inst::Const { .. }
                    | Inst::Address { .. }
                    | Inst::Read { .. }
                    | Inst::Store { .. }
                    | Inst::UpdateMask { .. }
                    | Inst::StorageLive { .. }
                    | Inst::StorageDead { .. }
                    | Inst::EndScope { .. }
                    | Inst::BeginLoan { .. }
                    | Inst::EndLoan { .. }
                    | Inst::BindOrigin { .. }
                    | Inst::EndOrigin { .. }
                    | Inst::Binary { .. }
            )
        })
        .collect();
    let barriers = private_instructions
        .iter()
        .enumerate()
        .filter_map(|(i, private)| (!private).then_some(i))
        .collect();
    Spine {
        private_instructions,
        scalar_calls: BTreeSet::from([protocol.mapper]),
        reached_calls: BTreeSet::new(),
        suspended: BTreeMap::new(),
        barriers,
        deferred_releases: BTreeMap::new(),
    }
}

pub(super) fn propose(
    units: &[&ValidatedUnit<Ready>],
    table: &LayoutTable,
    target: &TargetSpec,
    worker: &ValidatedUnit<Ready>,
    environments: &BTreeMap<ContentHash, Option<LayoutId>>,
    fuel: bool,
    helpers: ContentHash,
) -> Result<RootPlan, Refusal> {
    let u = worker.unit();
    let no = |reason| refused(u.header.instance, reason);
    let protocol = CpuWorkerProtocol::derive(worker, table)
        .ok_or_else(|| no("unproved CPU worker ownership protocol"))?;
    let mut selected = None;
    for map in units {
        if !map.unit().tables.closures.iter().any(|site| {
            site.code.is_some_and(|code| {
                map.unit().tables.symbols[code.symbol.index()].instance == Some(u.header.instance)
            })
        }) {
            continue;
        }
        for caller in units {
            for (index, call) in caller.unit().tables.calls.iter().enumerate() {
                if !matches!(call.callee, Callee::Direct(symbol)
                    if caller.unit().tables.symbols[symbol.index()].instance == Some(map.unit().header.instance))
                {
                    continue;
                }
                let Some(ArgumentValue::Value(value)) = call
                    .arguments
                    .get(&caller.unit().pools.call_arguments)
                    .and_then(|args| args.iter().find(|arg| arg.parameter == 3))
                    .map(|arg| arg.value)
                else {
                    continue;
                };
                let Some(id) = super::plan::known_code(caller.unit(), value, &[], false) else {
                    continue;
                };
                let Some(callback) = units
                    .iter()
                    .find(|callback| callback.unit().header.instance == id)
                else {
                    continue;
                };
                if let Some(binding) = CpuMapperBinding::derive(
                    caller,
                    CallId(index as u32),
                    map,
                    worker,
                    callback,
                    table,
                    environments,
                ) {
                    selected = Some(binding);
                    break;
                }
            }
            if selected.is_some() {
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let binding = selected.ok_or_else(|| no("unknown CPU mapper provenance"))?;
    let source = binding.callback.instance.unwrap();
    if binding
        .environment
        .is_some_and(|layout| !scalar_image(table, layout))
    {
        return Err(no("non-inert CPU mapper captures"));
    }
    let mut graph = scalar_graph(units, table, target, source, fuel, helpers)?;
    if graph.bodies.len() >= MAX_UNITS
        || graph
            .bodies
            .values()
            .any(|body| body.calls.contains(&u.header.instance))
    {
        return Err(no("recursive/oversize CPU mapper graph"));
    }
    let instructions: usize = units
        .iter()
        .filter(|unit| graph.bodies.contains_key(&unit.unit().header.instance))
        .map(|unit| unit.unit().instructions.len() + unit.unit().terminators.len())
        .sum();
    if instructions + u.instructions.len() + u.terminators.len() > MAX_INSTRUCTIONS {
        return Err(no("oversize CPU worker graph"));
    }
    let spine = spine(u, &protocol);
    let wrapper = table.layout(protocol.environment).unwrap();
    let LayoutKind::Shared {
        payload: shared_payload,
    } = table.layout(wrapper.fields[0].layout).unwrap().kind
    else {
        return Err(no("missing CPU Shared root"));
    };
    let extent = |layout| match table.layout(layout)?.size {
        SizeSpec::Fixed(size) => u32::try_from(size).ok(),
        _ => None,
    };
    let recipe = CaptureRecipe {
        wrapper: protocol.environment,
        source: binding.environment,
        wrapper_extent: extent(protocol.environment)
            .filter(|size| u64::from(*size) <= MAX_IMAGE_BYTES)
            .ok_or_else(|| no("unbounded CPU worker environment"))?,
        callable_offset: u32::try_from(wrapper.fields[0].offset)
            .map_err(|_| no("CPU root offset overflow"))?,
        live_word_offset: u32::try_from(wrapper.fields[5].offset)
            .map_err(|_| no("CPU live offset overflow"))?,
        source_extent: binding
            .environment
            .and_then(extent)
            .ok_or_else(|| no("missing CPU mapper extent"))?,
        live_mask: 1,
        source_code: binding.callback.clone(),
        source_abi: u.tables.calls[protocol.mapper.index()].abi,
        source_environment: true,
    };
    graph.bodies.insert(
        u.header.instance,
        ScalarBody {
            body: u.digest(),
            calls: BTreeSet::from([source]),
            fuel_points: crate::fuel::fuel_points(u),
        },
    );
    graph.root = u.header.instance;
    graph.identity = ContentHash::of_parts(
        "hirsel.ahead.cpu-worker.v1",
        [
            graph.identity.0.as_slice(),
            format!("{binding:?}{spine:?}{recipe:?}").as_bytes(),
        ],
    );
    Ok(RootPlan {
        graph,
        recipe,
        host: None,
        cpu: Some(CpuWorkerGraph {
            binding,
            shared_payload,
            spine,
        }),
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CpuWorkerProtocol {
    pub worker: ContentHash,
    pub digest: ContentHash,
    pub environment: LayoutId,
    pub shared_mapper: SlotId,
    pub jobs: SlotId,
    pub live: SlotId,
    pub ordinal: SlotId,
    pub mapper: CallId,
    pub mapper_loan: LoanId,
    pub claim: CallId,
    pub finish: CallId,
    pub result: SlotId,
    pub job_layout: LayoutId,
    pub result_layout: LayoutId,
}

/// The actual source callback carried through one map invocation's frozen
/// root. It binds code identity without replacing the Shared owner or loan.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CpuMapperBinding {
    pub caller: ContentHash,
    pub caller_digest: ContentHash,
    pub invocation: CallId,
    pub map: ContentHash,
    pub map_digest: ContentHash,
    pub worker: CpuWorkerProtocol,
    pub callback: Symbol,
    pub callback_digest: ContentHash,
    pub environment: Option<LayoutId>,
}

impl CpuMapperBinding {
    pub fn derive(
        caller: &ValidatedUnit<Ready>,
        invocation: CallId,
        map: &ValidatedUnit<Ready>,
        worker: &ValidatedUnit<Ready>,
        callback: &ValidatedUnit<Ready>,
        table: &LayoutTable,
        environments: &BTreeMap<ContentHash, Option<LayoutId>>,
    ) -> Option<Self> {
        let protocol = CpuWorkerProtocol::derive(worker, table)?;
        let caller = caller.unit();
        let map = map.unit();
        let callback = callback.unit();
        if map.header.origin != UnitOrigin::Library
            || !map.header.definition.is_core("cpu.map_scoped")
        {
            return None;
        }
        let call = caller.tables.calls.get(invocation.index())?;
        let Callee::Direct(symbol) = call.callee else {
            return None;
        };
        let symbol = caller.tables.symbols.get(symbol.index())?;
        if symbol.definition != map.header.definition
            || symbol.instance != Some(map.header.instance)
            || call.abi != map.header.abi
        {
            return None;
        }
        let actual = call
            .arguments
            .get(&caller.pools.call_arguments)?
            .iter()
            .find(|argument| argument.parameter == 3)?;
        let ArgumentValue::Value(value) = actual.value else {
            return None;
        };
        if actual.passing != Passing::Take
            || super::plan::known_code(caller, value, &[], false)? != callback.header.instance
        {
            return None;
        }
        let abi = &table.fn_abis[map.header.abi.0 as usize];
        let parameter = abi.parameters.iter().position(|p| p.source == 3)?;
        let seed = *map.parameters.get(parameter)?;
        let seeds = [(seed, callback.header.instance)];
        let mut sites = map.tables.closures.iter().filter(|site| {
            site.code.is_some_and(|code| {
                map.tables.symbols[code.symbol.index()].instance == Some(protocol.worker)
            })
        });
        let site = sites.next()?;
        if sites.next().is_some() || site.environment != protocol.environment {
            return None;
        }
        let captures = site.captures.get(&map.pools.captures)?;
        if captures.len() != 5
            || captures[0].usage != CaptureUse::Take
            || captures[1].usage != CaptureUse::Take
            || captures[2].usage != CaptureUse::Copy
            || captures[3].usage != CaptureUse::Copy
            || captures[4].usage != CaptureUse::Copy
            || super::plan::known_code(map, captures[0].value, &seeds, true)?
                != callback.header.instance
        {
            return None;
        }
        let expected =
            &table.fn_abis[worker.unit().tables.calls[protocol.mapper.index()].abi.0 as usize];
        let actual = &table.fn_abis[callback.header.abi.0 as usize];
        if expected.parameters != actual.parameters
            || expected.result != actual.result
            || expected.context != actual.context
            || expected.destination != actual.destination
            || expected.caller_site != actual.caller_site
            || !actual.environment
        {
            return None;
        }
        let environment = environments
            .get(&callback.header.instance)
            .copied()
            .flatten();
        // Missing environment evidence is not an empty capture image.
        let environment = Some(environment?);
        Some(Self {
            caller: caller.header.instance,
            caller_digest: caller.digest(),
            invocation,
            map: map.header.instance,
            map_digest: map.digest(),
            worker: protocol,
            callback: Symbol {
                kind: SymbolKind::Function,
                definition: callback.header.definition.clone(),
                instance: Some(callback.header.instance),
            },
            callback_digest: callback.digest(),
            environment,
        })
    }
}

fn place(unit: &LirUnit<Ready>, address: AddressId) -> Option<(SlotId, Vec<ProjectionStep>)> {
    let mut at = address;
    let mut seen = BTreeSet::new();
    let mut path = Vec::new();
    while seen.insert(at) {
        let address = unit.addresses.get(at.index())?;
        if !address.unchecked.is_empty() {
            return None;
        }
        let mut prefix = address.projection.get(&unit.pools.projections)?.to_vec();
        prefix.extend(path);
        path = prefix;
        match address.base {
            AddressBase::Slot(slot) => return Some((slot, path)),
            AddressBase::Address(parent) => at = parent,
            AddressBase::Value(_) => return None,
        }
    }
    None
}

fn whole(unit: &LirUnit<Ready>, address: AddressId) -> Option<SlotId> {
    let (slot, path) = place(unit, address)?;
    path.is_empty().then_some(slot)
}

fn sequence(table: &LayoutTable, id: LayoutId) -> Option<LayoutId> {
    let LayoutKind::Block { payload } = table.layout(id)?.kind else {
        return None;
    };
    let LayoutKind::Sequence { element } = table.layout(payload)?.kind else {
        return None;
    };
    Some(element)
}

fn integer(unit: &LirUnit<Ready>, value: ValueId) -> Option<u128> {
    unit.instructions.iter().find_map(|inst| match *inst {
        Inst::Const { result, constant } if result == value => {
            match unit.tables.constants.get(constant.index())?.value {
                ConstantValue::Integer(value) => Some(value),
                _ => None,
            }
        }
        _ => None,
    })
}

impl CpuWorkerProtocol {
    /// Reconstruct only from strict-C validated Ready content. The worker name
    /// chooses the protocol to inspect; it never supplies missing evidence.
    pub fn derive(validated: &ValidatedUnit<Ready>, table: &LayoutTable) -> Option<Self> {
        let unit = validated.unit();
        if unit.header.origin != UnitOrigin::Library
            || !unit.header.definition.is_core("$native.cpu_worker")
            || !unit.parameters.is_empty()
            || unit.captures.len() != 6
        {
            return None;
        }
        let capture = |ordinal| {
            let mut slots = unit
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.kind == SlotKind::Capture(ordinal));
            let (index, slot) = slots.next()?;
            slots
                .next()
                .is_none()
                .then_some((SlotId(index as u32), slot))
        };
        let (shared_mapper, shared_slot) = capture(0)?;
        let (jobs, jobs_slot) = capture(1)?;
        let (tasks, tasks_slot) = capture(2)?;
        let (width, width_slot) = capture(3)?;
        let (ordinal, ordinal_slot) = capture(4)?;
        let (live, live_slot) = capture(5)?;
        let mut flags = unit
            .tables
            .init_flags
            .iter()
            .filter(|flag| flag.environment.is_some());
        let flag = flags.next()?;
        if flags.next().is_some() || flag.slot != live || flag.path.len != 0 {
            return None;
        }
        let environment = flag.environment?;
        let wrapper = table.layout(environment)?;
        if !matches!(&wrapper.kind, LayoutKind::Environment { movable } if movable == &[1])
            || wrapper.fields.len() != 6
            || wrapper.finalizer.is_some()
            || wrapper.fields[0].layout != unit.types[shared_slot.ty.index()].layout
            || wrapper.fields[1].layout != unit.types[jobs_slot.ty.index()].layout
            || wrapper.fields[2].layout != unit.types[tasks_slot.ty.index()].layout
            || wrapper.fields[3].layout != unit.types[width_slot.ty.index()].layout
            || wrapper.fields[4].layout != unit.types[ordinal_slot.ty.index()].layout
            || wrapper.fields[5].layout != unit.types[live_slot.ty.index()].layout
            || !matches!(
                table.layout(wrapper.fields[4].layout)?.kind,
                LayoutKind::Scalar(ScalarKind::U64)
            )
            || !matches!(
                table.layout(wrapper.fields[5].layout)?.kind,
                LayoutKind::Scalar(ScalarKind::U64)
            )
        {
            return None;
        }
        let LayoutKind::Shared { payload } = table.layout(wrapper.fields[0].layout)?.kind else {
            return None;
        };
        // Shared's Frozen payload contains the immutable callable at field 0.
        let frozen = table.layout(payload)?;
        let mapper_layout = frozen.fields.first()?.layout;
        if frozen.fields.len() != 1
            || !matches!(
                table.layout(mapper_layout)?.kind,
                LayoutKind::Callable { .. }
            )
        {
            return None;
        }
        let job_layout = sequence(table, wrapper.fields[1].layout)?;
        let mut mapper_calls =
            unit.tables.calls.iter().enumerate().filter(|(_, call)| {
                matches!(call.callee, Callee::Indirect(_) | Callee::Bound { .. })
            });
        let (mapper_index, mapper) = mapper_calls.next()?;
        if mapper_calls.next().is_some() {
            return None;
        }
        let mapper_value = match mapper.callee {
            Callee::Indirect(value) => value,
            _ => return None,
        };
        let Holding::Borrowed(mapper_loan) = unit.values.get(mapper_value.index())?.holding else {
            return None;
        };
        let loan = unit.tables.loans.get(mapper_loan.index())?;
        if loan.mutable
            || place(unit, loan.address)? != (shared_mapper, vec![ProjectionStep::Field(0)])
            || unit.types[unit.values[mapper_value.index()].ty.index()].layout != mapper_layout
        {
            return None;
        }
        // No path can acquire a mutable loan into the Shared mapper.
        if unit.tables.loans.iter().any(|loan| {
            loan.mutable && place(unit, loan.address).is_some_and(|(slot, _)| slot == shared_mapper)
        }) {
            return None;
        }
        let abi = table.fn_abis.get(mapper.abi.0 as usize)?;
        let args = mapper.arguments.get(&unit.pools.call_arguments)?;
        if abi.parameters.len() != 1
            || abi.parameters[0].source != 0
            || abi.parameters[0].convention != Passing::Take
            || abi.parameters[0].layout != job_layout
            || args.len() != 1
            || args[0].parameter != 0
            || args[0].passing != Passing::Take
        {
            return None;
        }
        let ArgumentValue::Value(job) = args[0].value else {
            return None;
        };
        let result_layout = abi.result.as_ref()?.layout;
        let mapped = mapper.result?;
        let mut taken_jobs = unit.instructions.iter().filter_map(|inst| match *inst {
            Inst::Read {
                result,
                address,
                mode: ReadMode::Take,
                ..
            } if whole(unit, address) == Some(jobs) => Some(result),
            _ => None,
        });
        let taken_jobs_value = taken_jobs.next()?;
        if taken_jobs.next().is_some()
            || !unit.instructions.iter().any(|inst| {
                matches!(inst,
                Inst::Read { result, address, mode: ReadMode::Take, .. }
                    if *result == job && place(unit, *address).is_some_and(|(_, path)|
                        path == [ProjectionStep::VariantField { variant: 1, field: 0 }]))
            })
        {
            return None;
        }
        let mut clears = unit.instructions.iter().filter_map(|inst| match *inst {
            Inst::UpdateMask {
                result,
                mask,
                field: 0,
                initialized: false,
            } if unit.instructions.iter().any(|inst| {
                matches!(inst,
                    Inst::Read { result, address, mode: ReadMode::Bits, .. }
                        if *result == mask && whole(unit, *address) == Some(live))
            }) =>
            {
                Some(result)
            }
            _ => None,
        });
        let cleared = clears.next()?;
        if clears.next().is_some()
            || unit
                .instructions
                .iter()
                .filter(|inst| {
                    matches!(inst,
                Inst::Store { address, value, .. }
                    if whole(unit, *address) == Some(live) && *value == cleared)
                })
                .count()
                != 1
        {
            return None;
        }
        for inst in &unit.instructions {
            let address = match *inst {
                Inst::Store { address, value, .. }
                    if whole(unit, address) == Some(live) && value == cleared =>
                {
                    continue;
                }
                Inst::Store { address, .. }
                | Inst::Replace { address, .. }
                | Inst::SeqReserve { address, .. }
                | Inst::SeqSetLen { address, .. }
                | Inst::SeqAppend { address, .. }
                | Inst::ArenaPush { address, .. } => Some(address),
                Inst::Read {
                    address,
                    mode: ReadMode::Take,
                    ..
                } if whole(unit, address) != Some(jobs) => Some(address),
                _ => None,
            };
            if address
                .and_then(|address| place(unit, address))
                .is_some_and(|(slot, _)| {
                    matches!(unit.slots[slot.index()].kind, SlotKind::Capture(_))
                })
            {
                return None;
            }
        }
        let mut claim = None;
        let mut finish = None;
        let mut sink = None;
        let mut opened_jobs = None;
        let mut next_job = None;
        for (index, call) in unit.tables.calls.iter().enumerate() {
            if index == mapper_index {
                continue;
            }
            let arguments = call.arguments.get(&unit.pools.call_arguments)?;
            match call.callee {
                Callee::Operation(op) => {
                    let definition = &unit.tables.operations.get(op.index())?.definition;
                    let selected = if definition.is_core("$intrinsic.CpuMapScoped") {
                        let from_capture = |parameter, slot| {
                            arguments.iter().any(|arg| {
                            arg.parameter == parameter && arg.passing == Passing::Borrow
                                && matches!(arg.value, ArgumentValue::Value(value)
                                    if unit.instructions.iter().any(|inst| matches!(inst,
                                        Inst::Read { result, address, mode: ReadMode::Bits, .. }
                                            if *result == value && whole(unit, *address) == Some(slot))))
                        })
                        };
                        if arguments.len() != 4
                            || !from_capture(0, tasks)
                            || !from_capture(1, width)
                            || !from_capture(3, ordinal)
                            || !arguments.iter().any(|arg| {
                                arg.parameter == 2
                                    && arg.passing == Passing::Borrow
                                    && matches!(arg.value, ArgumentValue::Value(value)
                                    if integer(unit, value) == Some(3))
                            })
                        {
                            return None;
                        }
                        &mut claim
                    } else if definition.is_core("$intrinsic.TaskFinish") {
                        &mut finish
                    } else {
                        return None;
                    };
                    if selected.replace(CallId(index as u32)).is_some() {
                        return None;
                    }
                }
                Callee::Direct(symbol) => {
                    let definition = &unit.tables.symbols.get(symbol.index())?.definition;
                    if definition.is_core("$intrinsic.ListConsume_open") {
                        if opened_jobs.is_some()
                            || !arguments.iter().any(|arg| {
                                arg.parameter == 0
                                    && arg.passing == Passing::Take
                                    && arg.value == ArgumentValue::Value(taken_jobs_value)
                            })
                        {
                            return None;
                        }
                        opened_jobs = Some(call);
                    } else if definition.is_core("list.push") {
                        let destination = arguments.iter().find(|arg| arg.parameter == 0)?;
                        let ArgumentValue::Address { address, .. } = destination.value else {
                            return None;
                        };
                        let slot = whole(unit, address)?;
                        if destination.passing != Passing::Mut
                            || matches!(unit.slots.get(slot.index())?.kind, SlotKind::Capture(_))
                            || sequence(
                                table,
                                unit.types[unit.slots[slot.index()].ty.index()].layout,
                            )? != result_layout
                            || !arguments.iter().any(|arg| {
                                arg.parameter == 1
                                    && arg.passing == Passing::Take
                                    && arg.value == ArgumentValue::Value(mapped)
                            })
                            || sink.replace(slot).is_some()
                        {
                            return None;
                        }
                    } else if definition.is_core("$intrinsic.ListConsume_next") {
                        if next_job.replace(call).is_some() {
                            return None;
                        }
                    } else if definition.is_core("$native.owned_cursor_drop") {
                        // RC materializes the cursor's finalizer on normal and
                        // failure cleanup. These calls remain canonical barriers.
                    } else {
                        return None;
                    }
                }
                _ => return None,
            }
        }
        let opened_jobs = opened_jobs?;
        let next_job = next_job?;
        let cursor = opened_jobs.result?;
        let cursor_slot = stored(unit, cursor)?;
        let next_args = next_job.arguments.get(&unit.pools.call_arguments)?;
        if next_args.len() != 1
            || next_args[0].passing != Passing::Mut
            || !matches!(next_args[0].value, ArgumentValue::Address { address, .. }
                if whole(unit, address) == Some(cursor_slot))
        {
            return None;
        }
        let item_slot = stored(unit, next_job.result?)?;
        if !unit.instructions.iter().any(|inst| {
            matches!(inst,
            Inst::Read { result, address, mode: ReadMode::Take, .. }
                if *result == job && place(unit, *address) == Some((item_slot,
                    vec![ProjectionStep::VariantField { variant: 1, field: 0 }])))
        }) {
            return None;
        }
        Some(Self {
            worker: unit.header.instance,
            digest: unit.digest(),
            environment,
            shared_mapper,
            jobs,
            live,
            ordinal,
            mapper: CallId(mapper_index as u32),
            mapper_loan,
            claim: claim?,
            finish: finish?,
            result: sink?,
            job_layout,
            result_layout,
        })
    }

    pub fn check(&self, unit: &ValidatedUnit<Ready>, table: &LayoutTable) -> bool {
        Self::derive(unit, table).as_ref() == Some(self)
    }
}

fn stored(unit: &LirUnit<Ready>, value: ValueId) -> Option<SlotId> {
    let mut slots = unit.instructions.iter().filter_map(|inst| match *inst {
        Inst::Store {
            address,
            value: stored,
            ..
        } if stored == value => whole(unit, address),
        _ => None,
    });
    let slot = slots.next()?;
    slots.next().is_none().then_some(slot)
}
