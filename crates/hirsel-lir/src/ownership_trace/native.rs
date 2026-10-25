//! Native emission of leaf ownership transitions. This staged adapter accepts
//! only whole local storage and leaf values. Direct calls without owned
//! transport require a closed diagnostic call graph; captures, joins, nested
//! ownership and compound operations fail closed. It supplies Ready-local
//! coverage, not the still-required earlier compiler-pass transport proof.
use super::*;
use crate::layout_types::{LayoutId, LayoutKind, LayoutTable};
use crate::stage::{Ready, ValidatedUnit};

/// Optional diagnostic extension to the base native ABI. Ordinary code and
/// default runtime archives keep their existing ABI and context layout.
pub const ABI_VERSION: u32 = 67;
const _: () = assert!(ABI_VERSION > crate::runtime_abi::ABI_VERSION);

/// Scalar diagnostic operands shared by the two native emitters. A location
/// identifies shadow storage within a fresh dynamic frame, never a token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeLocation {
    /// 0 is an SSA value; 1 is a whole local slot.
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
    markers: BTreeMap<MarkerId, NativeMarker>,
    commits: Vec<(Point, MarkerId)>,
}
impl LeafPlan {
    pub fn prepare(unit: &ValidatedUnit<Ready>, layouts: &LayoutTable) -> Result<Self, String> {
        Self::from_unit(unit.unit(), layouts, false)
    }

    /// Initial observations precede placement, RC insertion and cleanup. The
    /// native producer must transport this plan through those passes, then
    /// consume it with from_relocated; Ready enumeration cannot replace it.
    /// Owned direct-call operands use existing semantic commit points here;
    /// native execution still refuses them until its commit transport exists.
    pub fn prepare_value(
        unit: &ValidatedUnit<crate::stage::Value>,
        layouts: &LayoutTable,
    ) -> Result<Plan, String> {
        Self::from_unit(unit.unit(), layouts, true).map(|plan| plan.plan)
    }

    fn from_unit<S: crate::stage::Stage>(
        unit: &LirUnit<S>,
        layouts: &LayoutTable,
        call_transport: bool,
    ) -> Result<Self, String> {
        let owned = |v: ValueId| unit.values[v.index()].holding == Holding::Owned;
        let layout = |v: ValueId| unit.types[unit.values[v.index()].ty.index()].layout;
        if unit.captures.iter().any(|&v| owned(v))
            || (!call_transport && unit.parameters.iter().any(|&v| owned(v)))
        {
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
            if !leaf(layouts, layout) {
                return Err(format!(
                    "native ownership tracing: nested ownership at {point:?}"
                ));
            }
            let id = MarkerId(
                u32::try_from(observations.len()).map_err(|_| "too many ownership observations")?,
            );
            let site =
                super::check::site(unit, point).ok_or("ownership observation has no source")?;
            if !call_transport {
                markers.insert(
                    id,
                    NativeMarker {
                        id,
                        kind: match kind {
                            Kind::Create => 0,
                            Kind::Move => 1,
                            Kind::Drop => 2,
                        },
                        token: location(unit, from)?,
                        owner: location(unit, to)?,
                        site,
                        layout,
                    },
                );
            }
            observations.push(Observation {
                point,
                marker: ObserveOwnership {
                    id,
                    site,
                    kind,
                    token: from,
                    owner: to,
                },
            });
            Ok(())
        };
        if call_transport {
            for (parameter, &value) in unit.parameters.iter().enumerate() {
                if owned(value) {
                    push(
                        Point::Entry,
                        Kind::Move,
                        Location::Parameter(parameter as u32),
                        Location::Value(value),
                        layout(value),
                    )?;
                }
            }
        }
        for (b, block) in unit.blocks.iter().enumerate() {
            if block
                .parameters
                .get(&unit.pools.values)
                .unwrap_or(&[])
                .iter()
                .any(|&v| owned(v))
            {
                return Err(
                    "native ownership tracing: owned join transport is not implemented".into(),
                );
            }
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
                        if site
                            .fields
                            .get(&unit.pools.fields)
                            .unwrap_or(&[])
                            .iter()
                            .any(|f| owned(f.value))
                        {
                            return Err("native ownership tracing: owned construction fields are not implemented".into());
                        }
                        if owned(result) {
                            push(
                                after,
                                Kind::Create,
                                Location::Value(result),
                                Location::Value(result),
                                layout(result),
                            )?;
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
                    Inst::Drop {
                        address,
                        if_live: false,
                        ..
                    } => {
                        let layout = unit.types[unit.addresses[address.index()].ty.index()].layout;
                        push(
                            before,
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
                    if !matches!(call.callee, Callee::Direct(_)) {
                        return Err("native ownership tracing: mutable or indirect call transport is not implemented".into());
                    }
                    for argument in call
                        .arguments
                        .get(&unit.pools.call_arguments)
                        .unwrap_or(&[])
                    {
                        match argument.value {
                            ArgumentValue::Address { .. } => return Err("native ownership tracing: mutable or indirect call transport is not implemented".into()),
                            ArgumentValue::Value(value) if owned(value) => {
                                if !call_transport || argument.passing != hirsel_core_contract::Passing::Take {
                                    return Err("native ownership tracing: owned call transport is not implemented".into());
                                }
                                push(Point::CallEntered(b), Kind::Move, Location::Value(value), Location::Parameter(argument.parameter), layout(value))?;
                            }
                            _ => (),
                        }
                    }
                    if let Some(result) = call.result.filter(|&value| owned(value)) {
                        if !call_transport || normal.is_none() {
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
                    if !call_transport {
                        return Err(
                            "native ownership tracing: owned return is not implemented".into()
                        );
                    }
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
        let plan = Plan::new(unit, observations);
        check(unit, &plan).map_err(|error| error.to_string())?;
        let commits = plan
            .observations
            .iter()
            .map(|o| (o.point, o.marker.id))
            .collect();
        Ok(Self {
            plan,
            markers,
            commits,
        })
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
        for observation in &plan.observations {
            let marker = &observation.marker;
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
                let at = expected.markers[&expected_observation.marker.id].layout;
                if layout.replace(at).is_some_and(|old| old != at) {
                    return Err("ownership transport changes layout between commits".into());
                }
            }
            let layout = layout.ok_or("ownership marker has no committed transition")?;
            markers.insert(
                marker.id,
                NativeMarker {
                    id: marker.id,
                    kind: match marker.kind {
                        Kind::Create => 0,
                        Kind::Move => 1,
                        Kind::Drop => 2,
                    },
                    token: location(body, marker.token)?,
                    owner: location(body, marker.owner)?,
                    site: marker.site,
                    layout,
                },
            );
            commits.extend(points.into_iter().map(|point| (point, marker.id)));
        }
        if covered.len() != expected.plan.observations.len() {
            return Err("ownership plan omits a Ready transition".into());
        }
        Ok(Self {
            plan: plan.clone(),
            markers,
            commits,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn at(
        &self,
        block: BlockId,
        index: u32,
        moment: Moment,
    ) -> impl Iterator<Item = NativeMarker> + '_ {
        let point = Point::Instruction {
            block,
            index,
            moment,
        };
        self.commits
            .iter()
            .filter(move |(at, _)| *at == point)
            .map(|(_, id)| self.markers[id])
    }
}

/// Independently derived parent/target unit identities for diagnostic objects.
/// Every direct target must be present: an ordinary object cannot silently
/// supply an unobserved callee. Descriptor finalizers remain checked separately.
pub fn local_calls(
    units: &[&ValidatedUnit<Ready>],
) -> Result<BTreeMap<[u8; 32], std::collections::BTreeSet<[u8; 32]>>, String> {
    let mut symbols = BTreeMap::new();
    for unit in units {
        let body = unit.unit();
        if symbols
            .insert(
                (&body.header.definition, body.header.instance),
                body.digest().0,
            )
            .is_some()
        {
            return Err("native ownership tracing: duplicate diagnostic call target".into());
        }
    }
    let mut calls = BTreeMap::new();
    for unit in units {
        let body = unit.unit();
        let mut targets = std::collections::BTreeSet::new();
        for terminator in &body.terminators {
            let Terminator::Call { call, .. } = terminator else {
                continue;
            };
            let Callee::Direct(symbol) = body.tables.calls[call.index()].callee else {
                return Err("native ownership tracing: indirect or operation call target".into());
            };
            let symbol = &body.tables.symbols[symbol.index()];
            let instance = symbol
                .instance
                .ok_or("native ownership tracing: unspecialized diagnostic call target")?;
            let target = symbols
                .get(&(&symbol.definition, instance))
                .ok_or("native ownership tracing: missing diagnostic call target")?;
            targets.insert(*target);
        }
        calls.insert(body.digest().0, targets);
    }
    Ok(calls)
}

fn location<S>(unit: &LirUnit<S>, location: Location) -> Result<NativeLocation, String> {
    match location {
        Location::Value(value) => Ok(NativeLocation {
            kind: 0,
            index: value.0,
        }),
        Location::Address(mut address) => {
            for _ in 0..unit.addresses.len() {
                let info = &unit.addresses[address.index()];
                if info.projection.len != 0 {
                    break;
                }
                match info.base {
                    AddressBase::Slot(slot) => {
                        return Ok(NativeLocation {
                            kind: 1,
                            index: slot.0,
                        });
                    }
                    AddressBase::Address(parent) => address = parent,
                    _ => break,
                }
            }
            Err("native ownership tracing: projected or nonlocal storage is not implemented".into())
        }
        _ => Err("native ownership tracing: nonlocal transport is not implemented".into()),
    }
}

fn plain(table: &LayoutTable, id: LayoutId, depth: usize) -> bool {
    let Some(record) = table.layout(id) else {
        return false;
    };
    if depth > table.layouts.len() {
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
        LayoutKind::Inline { element, .. }
        | LayoutKind::Simd { element, .. }
        | LayoutKind::Sequence { element } => plain(table, element, depth + 1),
        _ => false,
    }
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
        if let Self::Relocated(plans) = self
            && plans.len() != units.len()
        {
            return Err("ownership emission requires exactly one plan per unit".into());
        }
        let mut prepared = BTreeMap::new();
        for unit in units {
            let body = unit.unit();
            let plan = match self {
                Self::Leaf => LeafPlan::prepare(unit, layouts)?,
                Self::Relocated(plans) => LeafPlan::from_relocated(
                    unit,
                    layouts,
                    plans
                        .get(&body.digest())
                        .ok_or("ownership emission is missing a transported plan")?,
                )?,
            };
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
                            hirsel_codec::digest("hirsel.ownership-plan.v3", "Plan", plan),
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
