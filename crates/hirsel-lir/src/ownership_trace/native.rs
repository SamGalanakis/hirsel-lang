//! Native emission of leaf ownership transitions. This staged adapter accepts
//! static field storage, aggregate child custody, checked finalizer groups,
//! owned calls, once captures and disjoint joins within a closed call graph.
//! Unsupported dynamic/nonlocal ownership fails closed. Ready coverage is
//! checked independently against the transported original Value plan.
use super::*;
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable};
use crate::stage::{Ready, ValidatedUnit};
use std::collections::BTreeSet;

mod calls;
mod edges;
mod runtime_calls;
use runtime_calls::{resource_free_handle, runtime_call};

/// Optional diagnostic extension to the base native ABI. Ordinary code and
/// default runtime archives keep their existing ABI and context layout.
pub const ABI_VERSION: u32 = 89;
const _: () = assert!(ABI_VERSION > crate::runtime_abi::ABI_VERSION);

/// Scalar diagnostic operands shared by the two native emitters. A location
/// identifies shadow storage within a fresh dynamic frame, never a token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeLocation {
    /// 0 = SSA, 1 = slot, 2/3 = outgoing/incoming argument,
    /// 4/5 = outgoing/incoming result, 6 = capture slot, 7 = checked static
    /// address path, 8+field = constructed SSA field, u32::MAX = temporary
    /// parallel-edge custody. Relations name logical frames.
    pub kind: u32,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeMarker {
    pub id: MarkerId,
    /// Create = 0, Move = 1, Drop = 2.
    pub kind: u32,
    pub token: NativeLocation,
    pub owner: NativeLocation,
    pub site: SiteRange,
    pub layout: LayoutId,
}

/// Constructed only after ordinary Ready validation and exhaustive checking of
/// this adapter's supported instruction set. An arbitrary user plan cannot
/// suppress an ownership transition before native emission.
#[derive(Clone, Debug)]
pub struct LeafPlan {
    plan: Plan,
    markers: BTreeMap<MarkerId, Vec<NativeMarker>>,
    commits: Vec<(Point, NativeMarker)>,
    codes: Option<Vec<calls::Code>>,
    empty_codes: Vec<bool>,
}
impl LeafPlan {
    pub fn prepare(unit: &ValidatedUnit<Ready>, layouts: &LayoutTable) -> Result<Self, String> {
        Self::from_unit(unit.unit(), layouts, false, true)
    }

    /// Initial observations precede placement, RC insertion and cleanup. The
    /// native producer must transport this plan through those passes, then
    /// consume it with from_relocated; Ready enumeration cannot replace it.
    /// Owned direct-call operands use existing semantic commit points here;
    /// Ready enumeration cannot replace their original marker identities.
    pub fn prepare_value(
        unit: &ValidatedUnit<crate::stage::Value>,
        layouts: &LayoutTable,
    ) -> Result<Plan, String> {
        Self::from_unit(unit.unit(), layouts, true, false).map(|plan| plan.plan)
    }

    /// Build an original program plan including incoming callable custody.
    /// This is observation data, not code authority: Request::prepare must
    /// bind the complete checked call graph before any object can be emitted.
    pub fn prepare_value_for_program(
        unit: &ValidatedUnit<crate::stage::Value>,
        layouts: &LayoutTable,
    ) -> Result<Plan, String> {
        Self::from_unit(unit.unit(), layouts, true, true).map(|plan| plan.plan)
    }

    fn from_unit<S: crate::stage::Stage>(
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        source_only: bool,
        program: bool,
    ) -> Result<Self, String> {
        let owned = |v: ValueId| {
            unit.values[v.index()].holding == Holding::Owned
                && !resource_free_handle(
                    layouts,
                    unit.types[unit.values[v.index()].ty.index()].layout,
                )
        };
        let layout = |v: ValueId| unit.types[unit.values[v.index()].ty.index()].layout;
        if unit.captures.iter().any(|&v| owned(v)) {
            return Err(
                "native ownership tracing: owned entry transport is not implemented".into(),
            );
        }
        if unit.tables.failures.iter().any(|f| {
            f.dispose
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .any(|&v| owned(v))
        }) {
            return Err(
                "native ownership tracing: failure token disposal is not implemented".into(),
            );
        }
        let mut observations = Vec::new();
        let mut markers = BTreeMap::new();
        let mut push = |point: Point,
                        kind: Kind,
                        from: Location,
                        to: Location,
                        layout: LayoutId|
         -> Result<(), String> {
            // A source callable carries its captured lineage; it does not
            // create an additional resource token for the callable shell.
            // Alias binds a borrowed environment to its still-owned caller slot;
            // native callable retirement must prove every capture is gone.
            let carrier_move = matches!(kind, Kind::Move | Kind::Drop | Kind::Alias)
                && layouts
                    .layout(layout)
                    .is_some_and(|record| matches!(record.kind, LayoutKind::Callable { .. }));
            let aggregate_move = kind == Kind::Move
                && layouts
                    .layout(layout)
                    .is_some_and(|record| matches!(record.kind, LayoutKind::Aggregate));
            let source_aggregate = source_only
                && layouts.layout(layout).is_some_and(|record| {
                    matches!(record.kind, LayoutKind::Aggregate | LayoutKind::Resource)
                });
            let resource_tree = layouts.layout(layout).is_some_and(|record| {
                matches!(record.kind, LayoutKind::Resource)
                    && static_resource_tree(layouts, layout, &mut BTreeSet::new())
            });
            if !leaf(layouts, layout)
                && !resource_tree
                && !carrier_move
                && !source_aggregate
                && !aggregate_move
            {
                return Err(format!(
                    "native ownership tracing: nested ownership in {} at {point:?}: {kind:?}, layout {layout:?} {:?}",
                    unit.header.definition,
                    layouts.layout(layout)
                ));
            }
            let id = MarkerId(
                u32::try_from(observations.len()).map_err(|_| "too many ownership observations")?,
            );
            let site =
                super::check::site(unit, point).ok_or("ownership observation has no source")?;
            let observation = Observation {
                point,
                marker: ObserveOwnership {
                    id,
                    site,
                    kind,
                    token: from,
                    owner: to,
                },
            };
            if kind == Kind::Alias && source_only && !program
                && unit.parameters.iter().enumerate().any(|(parameter, _)|
                    matches!(to, Location::Address(address) if super::check::parameter_address(unit, parameter as u32, address).is_some())) {
                // The per-unit API cannot bind an incoming dynamic code.
                // A program request keeps this relation and later proves it.
                return Ok(());
            }
            if kind == Kind::Alias && !super::check::call_alias(unit, &observation) {
                if source_only {
                    // Preserve source observations for callable holders whose
                    // custody this native vocabulary cannot yet prove. Native
                    // preparation must refuse rather than omit the relation.
                    return Ok(());
                }
                return Err("native ownership tracing: unsupported callable owner binding".into());
            }
            if !source_only {
                markers.insert(
                    id,
                    vec![NativeMarker {
                        id,
                        kind: match kind {
                            Kind::Create => 0,
                            Kind::Move => 1,
                            Kind::Drop if carrier_move => 4,
                            Kind::Drop => 2,
                            Kind::Alias => 3,
                        },
                        token: location(unit, from, point)?,
                        owner: location(unit, to, point)?,
                        site,
                        layout,
                    }],
                );
            }
            observations.push(observation);
            Ok(())
        };
        {
            for (parameter, &value) in unit.parameters.iter().enumerate() {
                if owned(value) {
                    push(
                        Point::Entry,
                        Kind::Move,
                        Location::Parameter(parameter as u32),
                        Location::Value(value),
                        layout(value),
                    )?;
                } else if !resource_free_handle(layouts, layout(value))
                    && layouts
                        .fn_abis
                        .get(unit.header.abi.0 as usize)
                        .and_then(|abi| abi.parameters.get(parameter))
                        .is_some_and(|formal| {
                            formal.convention == hirsel_core_contract::Passing::Take
                        })
                    && layouts
                        .layout(layout(value))
                        .is_some_and(|record| !record.drop.is_empty() || record.finalizer.is_some())
                {
                    let address = unit
                        .addresses
                        .iter()
                        .enumerate()
                        .find_map(|(index, _)| {
                            let address = AddressId(index as u32);
                            super::check::parameter_address(unit, parameter as u32, address)
                                .map(|_| address)
                        })
                        .ok_or("owned parameter has no checked entry slot")?;
                    push(
                        Point::Entry,
                        Kind::Move,
                        Location::Parameter(parameter as u32),
                        Location::Address(address),
                        layout(value),
                    )?;
                }
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            let b = BlockId(b as u32);
            for (index, inst) in block
                .instructions
                .get(&unit.instructions)
                .unwrap_or(&[])
                .iter()
                .enumerate()
            {
                let after = Point::Instruction {
                    block: b,
                    index: index as u32,
                    moment: Moment::After,
                };
                let before = Point::Instruction {
                    block: b,
                    index: index as u32,
                    moment: Moment::Before,
                };
                match *inst {
                    Inst::Construct {
                        result, construct, ..
                    } => {
                        let site = &unit.tables.constructs[construct.index()];
                        let fields = site
                            .fields
                            .get(&unit.pools.fields)
                            .ok_or("ownership construction has no checked fields")?;
                        let nested = fields.iter().any(|field| owned(field.value));
                        // An aggregate carries existing child lineage without a new
                        // identity. A Resource constructor also creates its own token.
                        let aggregate = nested
                            && layouts
                                .layout(layout(result))
                                .is_some_and(|record| matches!(record.kind, LayoutKind::Aggregate));
                        if owned(result) && !aggregate {
                            push(
                                after,
                                Kind::Create,
                                Location::Value(result),
                                Location::Value(result),
                                layout(result),
                            )?;
                        }
                        for field in fields.iter().filter(|field| owned(field.value)) {
                            push(
                                after,
                                Kind::Move,
                                Location::Value(field.value),
                                Location::Field {
                                    aggregate: result,
                                    field: field.field,
                                },
                                layout(field.value),
                            )?;
                        }
                    }
                    Inst::Closure {
                        result, closure, ..
                    } => {
                        let site = &unit.tables.closures[closure.index()];
                        for (capture, captured) in site
                            .captures
                            .get(&unit.pools.captures)
                            .unwrap_or(&[])
                            .iter()
                            .enumerate()
                        {
                            match captured.usage {
                                CaptureUse::Take if owned(captured.value) => push(
                                    after,
                                    Kind::Move,
                                    Location::Value(captured.value),
                                    Location::Capture { closure: result, capture: capture as u32 },
                                    layout(captured.value),
                                )?,
                                CaptureUse::Copy if owned(captured.value) => return Err(
                                    "native ownership tracing: copied owned capture needs independent lineage".into()),
                                _ => (),
                            }
                        }
                    }
                    Inst::Read {
                        result,
                        address,
                        mode: ReadMode::Take,
                        ..
                    } if owned(result) => {
                        push(
                            after,
                            Kind::Move,
                            Location::Address(address),
                            Location::Value(result),
                            layout(result),
                        )?;
                    }
                    Inst::Store { address, value, .. } if owned(value) => {
                        push(
                            after,
                            Kind::Move,
                            Location::Value(value),
                            Location::Address(address),
                            layout(value),
                        )?;
                    }
                    Inst::Recast { result, value } | Inst::Weaken { result, value }
                        if owned(value) =>
                    {
                        push(
                            after,
                            Kind::Move,
                            Location::Value(value),
                            Location::Value(result),
                            layout(value),
                        )?;
                    }
                    Inst::Release { value, .. } if owned(value) => {
                        push(
                            before,
                            Kind::Drop,
                            Location::Value(value),
                            Location::Value(value),
                            layout(value),
                        )?;
                    }
                    Inst::FinalizeEnd { address, .. } => {
                        let layout = unit.types[unit.addresses[address.index()].ty.index()].layout;
                        push(
                            after,
                            Kind::Drop,
                            Location::Address(address),
                            Location::Address(address),
                            layout,
                        )?;
                    }
                    Inst::Drop { address, .. }
                        if resource_free_handle(
                            layouts,
                            unit.types[unit.addresses[address.index()].ty.index()].layout,
                        ) || plain(
                            layouts,
                            unit.types[unit.addresses[address.index()].ty.index()].layout,
                            0,
                        ) =>
                    {
                        ()
                    }
                    Inst::Drop {
                        address, if_live, ..
                    } => {
                        if if_live && !source_only {
                            return Err("native ownership tracing: conditional Drop has not reached a checked cleanup commit".into());
                        }
                        let point = if if_live {
                            Point::Instruction {
                                block: b,
                                index: index as u32,
                                moment: Moment::IfLive,
                            }
                        } else {
                            before
                        };
                        let layout = unit.types[unit.addresses[address.index()].ty.index()].layout;
                        push(
                            point,
                            Kind::Drop,
                            Location::Address(address),
                            Location::Address(address),
                            layout,
                        )?;
                    }
                    Inst::CopyValue { result, .. } if owned(result) => {
                        push(
                            after,
                            Kind::Create,
                            Location::Value(result),
                            Location::Value(result),
                            layout(result),
                        )?;
                    }
                    // FunctionRef has no environment or captured resource.
                    // Custody moves remain observed and emission separately
                    // proves the exact code identity of empty carriers.
                    Inst::FunctionRef { result, .. }
                        if unit.types[unit.values[result.index()].ty.index()].repr
                            == ValueRepr::Callable =>
                    {
                        ()
                    }
                    Inst::ArenaOpen { result, .. } if source_only && !owned(result) => (),
                    Inst::RegionEnter { result, .. }
                        if unit.types[unit.values[result.index()].ty.index()].repr
                            == ValueRepr::RegionMark =>
                    {
                        ()
                    }
                    Inst::ArenaToken { result, .. } if !owned(result) => (),
                    Inst::RegionExit { .. } => (),
                    Inst::StorageLive { .. }
                    | Inst::StorageDead { .. }
                    | Inst::EndScope { .. }
                    | Inst::Address { .. }
                    | Inst::FrameEnter { .. }
                    | Inst::FrameLeave
                    | Inst::BeginLoan { .. }
                    | Inst::EndLoan { .. }
                    | Inst::FinalizeBegin { .. }
                    | Inst::BindOrigin { .. }
                    | Inst::BindFields { .. }
                    | Inst::EndOrigin { .. } => (),
                    Inst::Const { .. }
                    | Inst::FunctionRef { .. }
                    | Inst::Unary { .. }
                    | Inst::Binary { .. }
                    | Inst::Compare { .. }
                    | Inst::Convert { .. }
                    | Inst::IntArith { .. }
                    | Inst::MulAdd { .. }
                    | Inst::ScalarUnary { .. }
                    | Inst::UpdateMask { .. }
                    | Inst::Check { .. }
                    | Inst::Read { .. }
                    | Inst::Store { .. }
                    | Inst::Recast { .. }
                    | Inst::Weaken { .. }
                    | Inst::IsUnique { .. }
                    | Inst::IsWritable { .. }
                    | Inst::SeqLen { .. }
                    | Inst::SeqCapacity { .. }
                    | Inst::Copy { .. }
                    | Inst::CopyValue { .. }
                    | Inst::FlatCopy { .. }
                        if inst.defs().iter().all(|&v| !owned(v)) =>
                    {
                        ()
                    }
                    _ => {
                        return Err(format!(
                            "native ownership tracing: unsupported instruction at {before:?}: {:?}",
                            inst.opcode()
                        ));
                    }
                }
            }
            match unit.terminators[b.index()] {
                Terminator::Call { call, normal, .. } => {
                    let call = &unit.tables.calls[call.index()];
                    if matches!(call.callee, Callee::Operation(_)) {
                        runtime_call(unit, layouts, call, None, None)?;
                        continue;
                    }
                    let once = call.callee.value().is_some()
                        && layouts.fn_abis.get(call.abi.0 as usize).is_some_and(|abi| {
                            abi.environment
                                && abi.multiplicity == crate::abi_types::CallMultiplicity::Once
                        });
                    if (!matches!(call.callee, Callee::Direct(_)) && !once)
                        || call.pretransfer_failure.is_some()
                    {
                        return Err(format!(
                            "native ownership tracing: mutable or indirect call transport is not implemented in {} block {}: {:?}, pretransfer failure {}",
                            unit.header.definition,
                            b.0,
                            call.callee,
                            call.pretransfer_failure.is_some()
                        ));
                    }
                    if once {
                        let view = call
                            .callee
                            .value()
                            .ok_or("once call has no callable view")?;
                        let mut reads = unit.instructions.iter().filter_map(|inst| match *inst {
                            Inst::Read {
                                result,
                                address,
                                mode: ReadMode::Borrow,
                                ..
                            } if result == view => Some(address),
                            _ => None,
                        });
                        let address = reads
                            .next()
                            .ok_or("once call has no checked borrowed environment owner")?;
                        if reads.next().is_some() {
                            return Err("once callable view is not unique".into());
                        }
                        push(
                            Point::CallEntered(b),
                            Kind::Alias,
                            Location::Value(view),
                            Location::Address(address),
                            layout(view),
                        )?;
                    }
                    for argument in call
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .unwrap_or(&[])
                    {
                        match argument.value {
                            ArgumentValue::Address { .. } => {
                                return Err(format!(
                                    "native ownership tracing: mutable or indirect call transport is not implemented in {} block {} parameter {}",
                                    unit.header.definition, b.0, argument.parameter
                                ));
                            }
                            ArgumentValue::Value(value) if owned(value) => {
                                if argument.passing != hirsel_core_contract::Passing::Take {
                                    return Err("native ownership tracing: owned call transport is not implemented".into());
                                }
                                push(
                                    Point::CallEntered(b),
                                    Kind::Move,
                                    Location::Value(value),
                                    Location::Parameter(argument.parameter),
                                    layout(value),
                                )?;
                            }
                            _ => (),
                        }
                    }
                    if let Some(result) = call.result.filter(|&value| owned(value)) {
                        if normal.is_none() {
                            return Err("native ownership tracing: owned call result transport is not implemented".into());
                        }
                        push(
                            Point::CallReturned(b),
                            Kind::Move,
                            Location::Return,
                            Location::Value(result),
                            layout(result),
                        )?;
                    }
                }
                Terminator::Return(Some(value)) if owned(value) => {
                    push(
                        Point::Return(b),
                        Kind::Move,
                        Location::Value(value),
                        Location::Return,
                        layout(value),
                    )?;
                }
                _ => (),
            }
        }
        for edge_id in super::check::used_edges(unit) {
            let edge = unit
                .tables
                .edges
                .get(edge_id.index())
                .ok_or("ownership edge is missing")?;
            let arguments = edge
                .arguments
                .get(&unit.pools.values)
                .ok_or("ownership edge has no arguments")?;
            let parameters = unit.blocks[edge.target.index()]
                .parameters
                .get(&unit.pools.values)
                .ok_or("ownership edge has no parameters")?;
            if arguments.len() != parameters.len() {
                return Err("ownership edge arity differs".into());
            }
            let transfers = arguments
                .iter()
                .copied()
                .zip(parameters.iter().copied())
                .filter(|&(_, to)| owned(to))
                .collect::<Vec<_>>();
            for (from, to) in transfers {
                if !owned(from) || layout(from) != layout(to) {
                    return Err("ownership edge changes owned representation".into());
                }
                push(
                    Point::Edge(edge_id),
                    Kind::Move,
                    Location::Value(from),
                    Location::Value(to),
                    layout(from),
                )?;
            }
        }
        let plan = Plan::new(unit, observations);
        check(unit, &plan).map_err(|error| error.to_string())?;
        let commits = if source_only {
            Vec::new()
        } else {
            plan.observations
                .iter()
                .map(|o| (o.point, markers[&o.marker.id][0]))
                .collect()
        };
        let mut prepared = Self {
            plan,
            markers,
            commits,
            codes: None,
            empty_codes: Vec::new(),
        };
        if !source_only {
            edges::stage(&mut prepared)?;
        }
        Ok(prepared)
    }

    /// Consume a plan transported from an earlier stage. Independent Ready
    /// coverage checks every physical transition, but its newly enumerated ids
    /// never replace the source plan's identities, sites or semantic drops.
    pub fn from_relocated(
        unit: &ValidatedUnit<Ready>,
        layouts: &LayoutTable,
        plan: &Plan,
    ) -> Result<Self, String> {
        let body = unit.unit();
        let checked = check(body, plan).map_err(|error| error.to_string())?;
        let expected = Self::prepare(unit, layouts)?;
        let mut covered = std::collections::BTreeSet::new();
        let mut markers = BTreeMap::new();
        let mut commits = vec![];
        let mut next_physical = plan
            .observations
            .iter()
            .map(|row| row.marker.id.0)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("ownership physical marker overflow")?;
        for observation in &plan.observations {
            let marker = &observation.marker;
            if matches!(observation.point, Point::StagedReturn { .. }) {
                let staged = super::staged::binding(body, observation)
                    .ok_or("ownership staged return lost checked storage")?;
                let mut transitions = vec![
                    (
                        staged.store,
                        Kind::Move,
                        Location::Value(staged.original),
                        Location::Address(staged.address),
                    ),
                    (
                        staged.take,
                        Kind::Move,
                        Location::Address(staged.address),
                        Location::Value(staged.returned),
                    ),
                    (
                        Point::Return(staged.returning),
                        Kind::Move,
                        Location::Value(staged.returned),
                        Location::Return,
                    ),
                ];
                transitions.extend(
                    expected
                        .plan
                        .observations
                        .iter()
                        .filter(|row| {
                            row.marker.kind == Kind::Drop
                                && row.marker.token == Location::Address(staged.address)
                        })
                        .map(|row| {
                            (
                                row.point,
                                row.marker.kind,
                                row.marker.token,
                                row.marker.owner,
                            )
                        }),
                );
                let mut physical = Vec::new();
                for (point, kind, token, owner) in transitions {
                    let row = expected
                        .plan
                        .observations
                        .iter()
                        .find(|row| {
                            row.point == point
                                && row.marker.kind == kind
                                && row.marker.token == token
                                && row.marker.owner == owner
                        })
                        .ok_or("staged return has no independent Ready transition")?;
                    if !covered.insert(row.marker.id) {
                        return Err("staged return covers a Ready transition twice".into());
                    }
                    let mut native = expected.markers[&row.marker.id][0];
                    native.id = MarkerId(next_physical);
                    next_physical = next_physical
                        .checked_add(1)
                        .ok_or("ownership physical marker overflow")?;
                    native.site = marker.site;
                    physical.push(native);
                    commits.push((point, native));
                }
                markers.insert(marker.id, physical);
                continue;
            }
            let points = checked
                .commits(body, marker.id)
                .map_err(|error| error.to_string())?;
            let mut obligations = vec![];
            if let Point::ReleaseDrop { block, index } = observation.point {
                let code = body.blocks[block.index()]
                    .instructions
                    .get(&body.instructions)
                    .ok_or("ownership release has no code")?;
                let Inst::Read { result, .. } = code[index as usize - 1] else {
                    return Err("ownership release has no checked read".into());
                };
                obligations.push((
                    Point::Instruction {
                        block,
                        index: index - 1,
                        moment: Moment::After,
                    },
                    Kind::Move,
                    marker.token,
                    Location::Value(result),
                    None,
                ));
                obligations.push((
                    Point::Instruction {
                        block,
                        index,
                        moment: Moment::Before,
                    },
                    Kind::Drop,
                    Location::Value(result),
                    Location::Value(result),
                    Some(marker.site),
                ));
            } else if matches!(observation.point, Point::CompoundDrop { .. }) {
                // CheckedPlan proves the complete aggregate cut. Each physical
                // completion retires only its exact child, retaining the source id.
                for &point in &points {
                    let Point::Instruction {
                        block,
                        index,
                        moment: Moment::After,
                    } = point
                    else {
                        return Err("compound ownership completion is not finalizer exit".into());
                    };
                    let code = body.blocks[block.index()]
                        .instructions
                        .get(&body.instructions)
                        .ok_or("compound ownership completion has no code")?;
                    let Inst::FinalizeEnd { address, .. } = code[index as usize] else {
                        return Err("compound ownership completion has no finalizer end".into());
                    };
                    obligations.push((
                        point,
                        Kind::Drop,
                        Location::Address(address),
                        Location::Address(address),
                        Some(marker.site),
                    ));
                }
            } else {
                obligations.extend(points.iter().map(|&point| {
                    (
                        point,
                        marker.kind,
                        marker.token,
                        marker.owner,
                        Some(marker.site),
                    )
                }));
            }
            let mut layout = None;
            let compound = matches!(observation.point, Point::CompoundDrop { .. });
            let mut physical = Vec::new();
            for (point, kind, token, owner, site) in obligations {
                let expected_observation = expected
                    .plan
                    .observations
                    .iter()
                    .find(|o| {
                        o.point == point
                            && o.marker.kind == kind
                            && o.marker.token == token
                            && o.marker.owner == owner
                            && site.is_none_or(|site| site == o.marker.site)
                    })
                    .ok_or_else(|| {
                        format!(
                            "ownership marker {:?} has no matching Ready transition",
                            marker.id
                        )
                    })?;
                if !covered.insert(expected_observation.marker.id) {
                    return Err("ownership plans cover a Ready transition twice".into());
                }
                let at = expected.markers[&expected_observation.marker.id][0].layout;
                if layout.replace(at).is_some_and(|old| old != at) && !compound {
                    return Err("ownership transport changes layout between commits".into());
                }
                if compound {
                    let native = NativeMarker {
                        id: marker.id,
                        kind: 2,
                        token: location(body, token, point)?,
                        owner: location(body, owner, point)?,
                        site: marker.site,
                        layout: at,
                    };
                    if !physical.contains(&native) {
                        physical.push(native);
                    }
                    commits.push((point, native));
                }
            }
            if compound {
                if physical.is_empty() {
                    return Err("compound ownership has no completion".into());
                }
                markers.insert(marker.id, physical);
                continue;
            }
            let layout = layout.ok_or("ownership marker has no committed transition")?;
            let native = NativeMarker {
                id: marker.id,
                kind: match marker.kind {
                    Kind::Create => 0,
                    Kind::Move => 1,
                    Kind::Drop
                        if layouts.layout(layout).is_some_and(|record| {
                            matches!(record.kind, LayoutKind::Callable { .. })
                        }) =>
                    {
                        4
                    }
                    Kind::Drop => 2,
                    Kind::Alias => 3,
                },
                token: location(body, marker.token, observation.point)?,
                owner: location(body, marker.owner, observation.point)?,
                site: marker.site,
                layout,
            };
            markers.insert(marker.id, vec![native]);
            commits.extend(points.into_iter().map(|point| (point, native)));
        }
        if covered.len() != expected.plan.observations.len() {
            return Err("ownership plan omits a Ready transition".into());
        }
        let mut prepared = Self {
            plan: plan.clone(),
            markers,
            commits,
            codes: None,
            empty_codes: Vec::new(),
        };
        edges::stage(&mut prepared)?;
        Ok(prepared)
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// An implicit owned-parameter cleanup has no spelled TIR span. Its
    /// readable source is the checked parameter Entry binding, not a nearby
    /// instruction or an arbitrary declaration range. The marker's original
    /// site remains unchanged for recipes, coverage and native row checks.
    pub fn implicit_parameter_cleanup_source(
        &self,
        unit: &LirUnit<Ready>,
        id: MarkerId,
    ) -> Option<(SiteRange, u32)> {
        if unit.digest() != self.plan.unit {
            return None;
        }
        let original = self
            .markers
            .iter()
            .find_map(|(original, markers)| {
                markers
                    .iter()
                    .any(|marker| marker.id == id)
                    .then_some(*original)
            })
            .unwrap_or(id);
        let observation = self
            .plan
            .observations
            .iter()
            .find(|row| row.marker.id == original)?;
        let marker = &observation.marker;
        if marker.kind != Kind::Drop
            || marker.site
                != (SiteRange {
                    source: u32::MAX,
                    start: 0,
                    end: 0,
                })
            || !matches!(
                observation.point,
                Point::FinalizerDrop { .. }
                    | Point::ReleaseDrop { .. }
                    | Point::CompoundDrop { .. }
            )
        {
            return None;
        }
        let Location::Address(mut address) = marker.token else {
            return None;
        };
        let slot = loop {
            let info = unit.addresses.get(address.index())?;
            if info.projection.len != 0 {
                return None;
            }
            match info.base {
                AddressBase::Slot(slot) => break slot,
                AddressBase::Address(parent) if parent.0 < address.0 => address = parent,
                _ => return None,
            }
        };
        let SlotKind::Parameter(parameter) = unit.slots.get(slot.index())?.kind else {
            return None;
        };
        let entry = self.plan.observations.iter().find(|row| {
            row.point == Point::Entry
                && row.marker.kind == Kind::Move
                && row.marker.token == Location::Parameter(parameter)
                && matches!(row.marker.owner, Location::Address(address)
                    if super::check::parameter_address(unit, parameter, address) == Some(slot))
        })?;
        (entry.marker.site.source != u32::MAX).then_some((entry.marker.site, parameter))
    }

    pub fn markers(&self) -> impl Iterator<Item = NativeMarker> + '_ {
        self.markers.values().flatten().copied()
    }
    pub fn at_point(&self, point: Point) -> impl Iterator<Item = NativeMarker> + '_ {
        self.commits
            .iter()
            .filter(move |(at, _)| *at == point)
            .map(|(_, marker)| *marker)
    }
    pub fn at_call(
        &self,
        unit: &LirUnit<Ready>,
        call: CallId,
        returned: bool,
    ) -> Result<Vec<NativeMarker>, String> {
        let blocks = unit
            .terminators
            .iter()
            .enumerate()
            .filter_map(|(b, term)| {
                matches!(term, Terminator::Call { call: actual, .. } if *actual == call)
                    .then_some(BlockId(b as u32))
            })
            .collect::<Vec<_>>();
        let [block] = blocks.as_slice() else {
            return Err("ownership call site must have one checked terminator".into());
        };
        Ok(self
            .at_point(if returned {
                Point::CallReturned(*block)
            } else {
                Point::CallEntered(*block)
            })
            .collect())
    }
    pub fn at(
        &self,
        block: BlockId,
        index: u32,
        moment: Moment,
    ) -> impl Iterator<Item = NativeMarker> + '_ {
        self.at_point(Point::Instruction {
            block,
            index,
            moment,
        })
    }
}

/// Ordinary checked calls, excluding exact implicit cleanup protocols. Cleanup
/// lowering uses Function symbols too: classification must prove the borrowed
/// receiver and every matching finalization exit, never a symbol name or merely
/// a FinalizeBegin elsewhere in the block. All targets remain required by the
/// complete physical local_call_sites inventory before this projection.
pub fn local_calls(
    units: &[&ValidatedUnit<Ready>],
) -> Result<BTreeMap<[u8; 32], std::collections::BTreeSet<[u8; 32]>>, String> {
    let mut calls = call_targets(units)?;
    for unit in units {
        let body = unit.unit();
        let sites = calls
            .get_mut(&body.digest().0)
            .ok_or("missing checked call inventory")?;
        sites.retain(|block, _| {
            !super::finalizer::begin(body, *block).is_some_and(|(address, _)| {
                super::finalizer::exits(body, *block, address).is_some()
            })
        });
    }
    Ok(calls
        .into_iter()
        .map(|(unit, sites)| (unit, sites.into_values().flatten().collect()))
        .collect())
}

/// Complete physical callees, including implicit finalizers and runtime callbacks.
pub fn physical_calls(
    units: &[&ValidatedUnit<Ready>],
) -> Result<BTreeMap<[u8; 32], BTreeSet<[u8; 32]>>, String> {
    Ok(call_targets(units)?
        .into_iter()
        .map(|(unit, sites)| (unit, sites.into_values().flatten().collect()))
        .collect())
}

/// Exact checked target of each original caller block. An Alias names one
/// call site, never the union of all functions that caller might invoke.
pub fn local_call_sites(
    units: &[&ValidatedUnit<Ready>],
) -> Result<BTreeMap<[u8; 32], BTreeMap<BlockId, [u8; 32]>>, String> {
    Ok(call_targets(units)?
        .into_iter()
        .map(|(unit, sites)| {
            (
                unit,
                sites
                    .into_iter()
                    .filter_map(|(block, targets)| {
                        let [target] = targets.as_slice() else {
                            return None;
                        };
                        Some((block, *target))
                    })
                    .collect(),
            )
        })
        .collect())
}
fn call_targets(
    units: &[&ValidatedUnit<Ready>],
) -> Result<BTreeMap<[u8; 32], BTreeMap<BlockId, Vec<[u8; 32]>>>, String> {
    let mut symbols = BTreeMap::new();
    for unit in units {
        let body = unit.unit();
        if symbols
            .insert(
                (&body.header.definition, body.header.instance),
                (body.digest().0, body.header.abi),
            )
            .is_some()
        {
            return Err("native ownership tracing: duplicate diagnostic call target".into());
        }
    }
    let resolve = |symbol: &Symbol, abi: Option<crate::abi_types::FnAbiId>| {
        let instance = symbol
            .instance
            .ok_or("native ownership tracing: unspecialized diagnostic call target")?;
        let target = symbols
            .get(&(&symbol.definition, instance))
            .ok_or("native ownership tracing: missing diagnostic call target")?;
        if abi.is_some_and(|abi| abi != target.1) {
            return Err("native ownership tracing: closure code ABI differs from emitted body");
        }
        Ok(target.0)
    };
    let codes = calls::closed_runtime(units)?;
    let mut calls = BTreeMap::new();
    for (index, unit) in units.iter().enumerate() {
        let body = unit.unit();
        let mut targets = BTreeMap::new();
        // Constructors and references can escape without a local invocation.
        // They still require diagnostic bodies; an ordinary unobserved object
        // must never fill a hole in the emitted closure.
        for inst in &body.instructions {
            match *inst {
                Inst::Closure { closure, .. } => {
                    let code = body.tables.closures[closure.index()]
                        .code
                        .ok_or("native ownership tracing: closure has no diagnostic code")?;
                    resolve(&body.tables.symbols[code.symbol.index()], Some(code.abi))?;
                }
                Inst::FunctionRef { symbol, .. } => {
                    resolve(&body.tables.symbols[symbol.index()], None)?;
                }
                _ => (),
            }
        }
        let codes = &codes[index];
        for (block, terminator) in body.terminators.iter().enumerate() {
            let Terminator::Call { call, .. } = terminator else {
                continue;
            };
            let choices = match body.tables.calls[call.index()].callee {
                Callee::Direct(symbol) | Callee::Bound { code: symbol, .. } => {
                    vec![(&body.tables.symbols[symbol.index()], None)]
                }
                Callee::Indirect(value) => {
                    let Some(targets) = codes[value.index()].targets() else {
                        return Err(
                            "native ownership tracing: unresolved diagnostic callable target"
                                .into(),
                        );
                    };
                    targets
                        .iter()
                        .map(|target| (&target.symbol, target.abi))
                        .collect()
                }
                Callee::Operation(_) => {
                    // A runtime callback re-enters a checked language body.
                    // Keep that exact code edge for the decoder even though
                    // the runtime call itself has no language frame.
                    let call = &body.tables.calls[call.index()];
                    let mut callbacks = call
                        .arguments
                        .get(&body.pools.call_arguments)
                        .ok_or("ownership runtime call arguments are absent")?
                        .iter()
                        .filter_map(|argument| match argument.value {
                            ArgumentValue::Value(value)
                                if body.types[body.values[value.index()].ty.index()].repr
                                    == ValueRepr::Callable =>
                            {
                                Some(value)
                            }
                            _ => None,
                        });
                    let Some(value) = callbacks.next() else {
                        continue;
                    };
                    if callbacks.next().is_some() {
                        return Err(
                            "native ownership tracing: multiple runtime callback targets".into(),
                        );
                    }
                    let Some(targets) = codes[value.index()].targets() else {
                        return Err("native ownership tracing: unresolved runtime callable".into());
                    };
                    targets
                        .iter()
                        .map(|target| (&target.symbol, target.abi))
                        .collect()
                }
            };
            targets.insert(
                BlockId(block as u32),
                choices
                    .into_iter()
                    .map(|(symbol, abi)| resolve(symbol, abi))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        calls.insert(body.digest().0, targets);
    }
    Ok(calls)
}

fn location<S>(
    unit: &LirUnit<S>,
    location: Location,
    point: Point,
) -> Result<NativeLocation, String> {
    match location {
        Location::Value(value) => Ok(NativeLocation {
            kind: 0,
            index: value.0,
        }),
        Location::Address(address) => {
            let path = address_path(unit, address)?;
            if path.fields.is_empty() {
                Ok(path.root)
            } else {
                Ok(NativeLocation {
                    kind: 7,
                    index: address.0,
                })
            }
        }
        Location::Field {
            aggregate: closure,
            field: capture,
        }
        | Location::Capture { closure, capture } => Ok(NativeLocation {
            kind: capture
                .checked_add(8)
                .ok_or("capture ordinal exceeds native diagnostic encoding")?,
            index: closure.0,
        }),
        Location::Parameter(index) => Ok(NativeLocation {
            kind: if point == Point::Entry { 3 } else { 2 },
            index,
        }),
        Location::Return => Ok(NativeLocation {
            kind: if matches!(point, Point::Return(_)) {
                4
            } else {
                5
            },
            index: 0,
        }),
        _ => Err("native ownership tracing: nonlocal transport is not implemented".into()),
    }
}

fn plain(table: &LayoutTable, id: LayoutId, depth: usize) -> bool {
    let Some(record) = table.layout(id) else {
        return false;
    };
    if depth > table.layouts.len() || record.finalizer.is_some() {
        return false;
    }
    match record.kind {
        LayoutKind::Unit
        | LayoutKind::Scalar(_)
        | LayoutKind::Path
        | LayoutKind::Header
        | LayoutKind::Flat => true,
        LayoutKind::Aggregate => record
            .fields
            .iter()
            .all(|f| plain(table, f.layout, depth + 1)),
        LayoutKind::Variant | LayoutKind::Packed => record.tag.as_ref().is_some_and(|tag| {
            tag.variants.iter().all(|variant| {
                variant
                    .fields
                    .iter()
                    .all(|f| plain(table, f.layout, depth + 1))
            })
        }),
        LayoutKind::Inline { element, .. }
        | LayoutKind::Simd { element, .. }
        | LayoutKind::Sequence { element } => plain(table, element, depth + 1),
        _ => false,
    }
}

// Static inline Resource/aggregate fields preserve independent child identities.
// Each FinalizeEnd retires only its own Resource; no parent event drops children.
fn static_resource_tree(
    table: &LayoutTable,
    layout: LayoutId,
    active: &mut BTreeSet<LayoutId>,
) -> bool {
    if plain(table, layout, 0) {
        return true;
    }
    let Some(record) = table.layout(layout) else {
        return false;
    };
    if !matches!(record.kind, LayoutKind::Resource | LayoutKind::Aggregate)
        || !active.insert(layout)
    {
        return false;
    }
    let supported = record
        .fields
        .iter()
        .all(|field| static_resource_tree(table, field.layout, active));
    active.remove(&layout);
    supported
}

fn leaf(table: &LayoutTable, id: LayoutId) -> bool {
    let Some(record) = table.layout(id) else {
        return false;
    };
    match record.kind {
        LayoutKind::Block { payload } | LayoutKind::Box { payload } => plain(table, payload, 0),
        LayoutKind::Slice { element } => plain(table, element, 0),
        LayoutKind::Resource | LayoutKind::Aggregate => {
            record.fields.iter().all(|f| plain(table, f.layout, 0))
        }
        LayoutKind::Handle | LayoutKind::HostCapability { .. } => true,
        _ => false,
    }
}

/// Explicit diagnostic selection. Ordinary emission passes None and never
/// prepares plans or computes their fingerprint.
#[derive(Clone, Copy)]
pub enum Request<'a> {
    Leaf,
    Relocated(&'a BTreeMap<ContentHash, Plan>),
}
impl Request<'_> {
    /// One complete closed set, keyed by current unit digests. Supplying a
    /// subset must refuse rather than silently synthesizing the missing plans.
    pub fn prepare(
        self,
        units: &[&ValidatedUnit<Ready>],
        layouts: &LayoutTable,
    ) -> Result<BTreeMap<ContentHash, LeafPlan>, String> {
        local_calls(units)?;
        let codes = calls::closed_runtime(units)?;
        let empty = runtime_calls::empty_callables(units, layouts, &codes);
        if let Self::Relocated(plans) = self
            && plans.len() != units.len()
        {
            return Err("ownership emission requires exactly one plan per unit".into());
        }
        let mut prepared = BTreeMap::new();
        for (index, unit) in units.iter().enumerate() {
            let body = unit.unit();
            let mut plan = match self {
                Self::Leaf => LeafPlan::prepare(unit, layouts)?,
                Self::Relocated(plans) => LeafPlan::from_relocated(
                    unit,
                    layouts,
                    plans
                        .get(&body.digest())
                        .ok_or("ownership emission is missing a transported plan")?,
                )?,
            };
            for call in &body.tables.calls {
                if matches!(call.callee, Callee::Operation(_)) {
                    runtime_call(
                        body,
                        layouts,
                        call,
                        Some(&codes[index]),
                        Some(&empty[index]),
                    )?;
                }
            }
            plan.codes = Some(codes[index].clone());
            plan.empty_codes = empty[index].clone();
            if prepared.insert(body.header.instance, plan).is_some() {
                return Err("ownership emission has duplicate instance identities".into());
            }
        }
        Ok(prepared)
    }

    pub fn fingerprint(self) -> Option<ContentHash> {
        match self {
            Self::Leaf => None,
            Self::Relocated(plans) => {
                let identities: Vec<_> = plans
                    .iter()
                    .map(|(unit, plan)| {
                        (
                            *unit,
                            hirsel_codec::digest("hirsel.ownership-plan.v4", "Plan", plan),
                        )
                    })
                    .collect();
                Some(hirsel_codec::digest(
                    "hirsel.ownership-relocated-plans.v1",
                    "Plans",
                    &identities,
                ))
            }
        }
    }
}

/// Rebind an already transported plan when independently checked local units
/// enter a shared emission layout table. Numeric table selectors may change;
/// complete canonical body/layout/ABI content and every marker must agree.
/// This grants no source or ordinary C authority and never enumerates new ids.
pub fn rebind_plan(
    before: &ValidatedUnit<Ready>,
    before_inputs: &crate::validate::Inputs<'_>,
    after: &ValidatedUnit<Ready>,
    after_inputs: &crate::validate::Inputs<'_>,
    plan: &Plan,
) -> Result<Plan, String> {
    LeafPlan::from_relocated(before, before_inputs.layouts, plan)?;
    let (_, source) = crate::products::LayoutClosure::capture(before, before_inputs, &[])?;
    let (_, bound) = crate::products::LayoutClosure::capture(after, after_inputs, &[])?;
    if source != bound {
        return Err("ownership binding changed canonical body, layout or ABI content".into());
    }
    let rebound = Plan {
        unit: after.unit().digest(),
        ..plan.clone()
    };
    LeafPlan::from_relocated(after, after_inputs.layouts, &rebound)?;
    Ok(rebound)
}

/// A physical capture cell is a lookup key during synchronous cleanup only.
/// Its declaration ordinal selects the original logical custody path.
#[derive(Clone, Debug)]
pub struct CleanupCapture {
    /// Scope-local binding key, never token identity or a byte offset.
    pub field: u32,
    pub path: Vec<u32>,
    pub offset: u64,
    pub layout: LayoutId,
}
impl LeafPlan {
    /// A statically known function reference or resource-free environment has
    /// no Resource lineage to move. Unknown code never proves emptiness.
    pub fn empty_environment(
        &self,
        unit: &LirUnit<Ready>,
        layouts: &LayoutTable,
        id: MarkerId,
    ) -> Result<bool, String> {
        let (point, marker) = self
            .commits
            .iter()
            .find(|(_, marker)| marker.id == id)
            .ok_or("ownership marker has no committed operation")?;
        if !matches!(marker.kind, 1 | 3)
            || !layouts
                .layout(marker.layout)
                .is_some_and(|record| matches!(record.kind, LayoutKind::Callable { .. }))
        {
            return Ok(false);
        }
        let value = if marker.token.kind == 0 {
            ValueId(marker.token.index)
        } else if marker.token.kind == edges::TEMPORARY {
            let logical = self
                .markers
                .iter()
                .find_map(|(id, markers)| {
                    markers.iter().any(|at| at.id == marker.id).then_some(*id)
                })
                .ok_or("ownership temporary has no original marker")?;
            let original = self
                .plan
                .observations
                .iter()
                .find(|row| row.marker.id == logical)
                .ok_or("ownership temporary lost its original edge")?;
            let Location::Value(value) = original.marker.token else {
                return Err("ownership temporary has no original SSA source".into());
            };
            value
        } else {
            match *point {
                Point::Entry => *unit
                    .parameters
                    .get(marker.token.index as usize)
                    .ok_or("ownership callable parameter is absent")?,
                Point::CallReturned(block) => {
                    let Terminator::Call { call, .. } = unit.terminators[block.index()] else {
                        return Err("ownership callable result has no call".into());
                    };
                    unit.tables.calls[call.index()]
                        .result
                        .ok_or("ownership callable result is absent")?
                }
                Point::Instruction { block, index, .. } => {
                    let code = unit.blocks[block.index()]
                        .instructions
                        .get(&unit.instructions)
                        .ok_or("ownership callable operation is absent")?;
                    match code[index as usize] {
                        Inst::Read { result, .. } => result,
                        _ => return Err("ownership callable storage is not a checked read".into()),
                    }
                }
                _ => return Err("ownership callable has no exact code operand".into()),
            }
        };
        if self.empty_codes.get(value.index()) == Some(&true) {
            return Ok(true);
        }
        let codes = self
            .codes
            .as_ref()
            .ok_or("ownership callable needs the complete checked code graph")?;
        let targets = codes[value.index()]
            .targets()
            .ok_or("ownership callable has unresolved environment identity")?;
        targets.iter().try_fold(true, |empty, target| {
            Ok(empty
                && match target.environment {
                    None => true,
                    Some(environment) => layouts
                        .layout(environment)
                        .map(|record| record.drop.is_empty() && record.finalizer.is_none())
                        .ok_or("ownership callable environment layout is absent")?,
                })
        })
    }

    /// Exact statically nested leaf Resource cells of the callable released by this marker.
    /// Unknown code/environment identities and ambiguous cells fail closed.
    pub fn cleanup_captures(
        &self,
        unit: &LirUnit<Ready>,
        layouts: &LayoutTable,
        id: MarkerId,
    ) -> Result<(ValueId, Vec<CleanupCapture>), String> {
        let mut released = None;
        for &(point, marker) in &self.commits {
            if marker.id != id {
                continue;
            }
            let Point::Instruction {
                block,
                index,
                moment: Moment::Before,
            } = point
            else {
                return Err("ownership cleanup is not a committed release".into());
            };
            let code = unit.blocks[block.index()]
                .instructions
                .get(&unit.instructions)
                .ok_or("ownership cleanup has no block")?;
            let Inst::Release { value, .. } = code[index as usize] else {
                return Err("ownership cleanup has no physical release".into());
            };
            if released.replace(value).is_some_and(|old| old != value) {
                return Err("ownership cleanup has ambiguous physical holders".into());
            }
        }
        let value = released.ok_or("ownership cleanup has no release")?;
        if self.empty_codes.get(value.index()) == Some(&true) {
            return Ok((value, Vec::new()));
        }
        let codes = self.codes.clone().unwrap_or_else(|| calls::discover(unit));
        let targets = codes[value.index()]
            .targets()
            .ok_or("ownership cleanup has unresolved callable identity")?;
        if targets.iter().all(|target| {
            target.environment.is_none_or(|environment| {
                layouts
                    .layout(environment)
                    .is_some_and(|record| record.drop.is_empty() && record.finalizer.is_none())
            })
        }) {
            return Ok((value, Vec::new()));
        }
        let [target] = targets else {
            return Err("ownership cleanup has ambiguous callable environment".into());
        };
        let Some(environment) = target.environment else {
            return Ok((value, Vec::new()));
        };
        let record = layouts
            .layout(environment)
            .ok_or("ownership cleanup environment is absent")?;
        let mut captures = Vec::new();
        let mut next = record
            .fields
            .iter()
            .map(|field| field.ordinal)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("ownership capture binding overflow")?;
        let mut active = std::collections::BTreeSet::new();
        #[allow(clippy::too_many_arguments)]
        fn descend(
            layouts: &LayoutTable,
            layout: LayoutId,
            offset: u64,
            path: Vec<u32>,
            binding: u32,
            next: &mut u32,
            active: &mut std::collections::BTreeSet<LayoutId>,
            captures: &mut Vec<CleanupCapture>,
        ) -> Result<(), String> {
            let record = layouts
                .layout(layout)
                .ok_or("ownership cleanup field is absent")?;
            if record.drop.is_empty() && record.finalizer.is_none() {
                return Ok(());
            }
            if !active.insert(layout) {
                return Err("ownership cleanup has recursive inline storage".into());
            }
            if matches!(record.kind, LayoutKind::Resource)
                && record.finalizer.is_some()
                && leaf(layouts, layout)
            {
                if captures.iter().any(|prior| prior.offset == offset) {
                    return Err("ownership cleanup has ambiguous capture cells".into());
                }
                captures.push(CleanupCapture {
                    field: binding,
                    path,
                    offset,
                    layout,
                });
            } else if matches!(record.kind, LayoutKind::Aggregate) && record.finalizer.is_none() {
                let mut seen = std::collections::BTreeSet::new();
                for step in &record.drop {
                    let crate::layout_types::DropStep::Child {
                        offset: child_offset,
                        layout: child_layout,
                    } = *step
                    else {
                        return Err("ownership cleanup requires static inline child drops".into());
                    };
                    let fields = record
                        .fields
                        .iter()
                        .filter(|field| {
                            field.offset == child_offset && field.layout == child_layout
                        })
                        .collect::<Vec<_>>();
                    let [field] = fields.as_slice() else {
                        return Err(
                            "ownership cleanup child has no unique declaration field".into()
                        );
                    };
                    if !seen.insert(field.ordinal) {
                        return Err("ownership cleanup repeats a declaration field".into());
                    }
                    let binding = *next;
                    *next = next
                        .checked_add(1)
                        .ok_or("ownership capture binding overflow")?;
                    let mut path = path.clone();
                    path.push(field.ordinal);
                    descend(
                        layouts,
                        child_layout,
                        offset
                            .checked_add(child_offset)
                            .ok_or("ownership capture offset overflow")?,
                        path,
                        binding,
                        next,
                        active,
                        captures,
                    )?;
                }
            } else {
                return Err(
                    "ownership cleanup requires static aggregates of leaf Resource captures".into(),
                );
            }
            active.remove(&layout);
            Ok(())
        }
        for field in record.fields.iter().rev() {
            descend(
                layouts,
                field.layout,
                field.offset,
                vec![field.ordinal],
                field.ordinal,
                &mut next,
                &mut active,
                &mut captures,
            )?;
        }
        Ok((value, captures))
    }
}

/// Static declaration path for a projected slot; runtime addresses and byte
/// offsets never identify the resource. Dynamic and nonlocal projections refuse.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePath {
    pub root: NativeLocation,
    pub fields: Vec<u32>,
}
fn address_path<S>(unit: &LirUnit<S>, mut address: AddressId) -> Result<NativePath, String> {
    let mut parts = Vec::new();
    for _ in 0..unit.addresses.len() {
        let info = unit
            .addresses
            .get(address.index())
            .ok_or("ownership address is absent")?;
        let steps = info
            .projection
            .get(&unit.pools.projections)
            .ok_or("ownership projection is absent")?;
        let fields = steps
            .iter()
            .map(|step| match *step {
                ProjectionStep::Field(field) => Ok(field),
                _ => Err(
                    "native ownership tracing: dynamic or non-field storage is not implemented"
                        .to_string(),
                ),
            })
            .collect::<Result<Vec<_>, _>>()?;
        parts.push(fields);
        match info.base {
            AddressBase::Slot(slot) => {
                let root = match unit
                    .slots
                    .get(slot.index())
                    .ok_or("ownership slot is absent")?
                    .kind
                {
                    SlotKind::Capture(capture) => NativeLocation {
                        kind: 6,
                        index: capture,
                    },
                    _ => NativeLocation {
                        kind: 1,
                        index: slot.0,
                    },
                };
                return Ok(NativePath {
                    root,
                    fields: parts.into_iter().rev().flatten().collect(),
                });
            }
            AddressBase::Address(parent) => address = parent,
            _ => break,
        }
    }
    Err("native ownership tracing: nonlocal storage is not implemented".into())
}
/// Resolve only the reserved projected-address location class, against the
/// same checked unit used for emission. Plain locations have no extra path.
pub fn projected_location<S>(
    unit: &LirUnit<S>,
    location: NativeLocation,
) -> Result<Option<NativePath>, String> {
    if location.kind != 7 {
        return Ok(None);
    }
    let path = address_path(unit, AddressId(location.index))?;
    if path.fields.is_empty() {
        return Err("ownership projected location has no fields".into());
    }
    Ok(Some(path))
}
