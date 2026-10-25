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
pub const ABI_VERSION: u32 = 64;
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
}
impl LeafPlan {
    pub fn prepare(unit: &ValidatedUnit<Ready>, layouts: &LayoutTable) -> Result<Self, String> {
        Self::from_unit(unit.unit(), layouts)
    }

    fn from_unit(unit: &LirUnit<Ready>, layouts: &LayoutTable) -> Result<Self, String> {
        let owned = |v: ValueId| unit.values[v.index()].holding == Holding::Owned;
        let layout = |v: ValueId| unit.types[unit.values[v.index()].ty.index()].layout;
        if unit
            .parameters
            .iter()
            .chain(&unit.captures)
            .any(|&v| owned(v))
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
                Terminator::Call { call, .. } => {
                    let call = &unit.tables.calls[call.index()];
                    if !matches!(call.callee, Callee::Direct(_))
                        || call.result.is_some_and(owned)
                        || call
                            .arguments
                            .get(&unit.pools.call_arguments)
                            .unwrap_or(&[])
                            .iter()
                            .any(|argument| match argument.value {
                                ArgumentValue::Value(value) => owned(value),
                                ArgumentValue::Address { .. } => true,
                            })
                    {
                        return Err("native ownership tracing: owned, mutable or indirect call transport is not implemented".into());
                    }
                }
                Terminator::Return(Some(v)) if owned(v) => {
                    return Err("native ownership tracing: owned return is not implemented".into());
                }
                _ => (),
            }
        }
        let plan = Plan::new(unit, observations);
        check(unit, &plan).map_err(|error| error.to_string())?;
        Ok(Self { plan, markers })
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
        self.plan
            .observations
            .iter()
            .filter(move |o| o.point == point)
            .map(|o| self.markers[&o.marker.id])
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
