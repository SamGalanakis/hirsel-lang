//! Local storage and places; the library extends values and projections here.
use crate::{Error, Value, invalid};
use hirsel_tir::LocalId;
use hirsel_tir::*;
use hirsel_types::UseKind;

/// Separate a source payload tag check from malformed storage instructions.
#[derive(Debug)]
pub(crate) enum AccessError {
    InactiveVariant,
    OutOfBounds,
    Api(Error),
}
impl From<Error> for AccessError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<AccessError> for Error {
    fn from(error: AccessError) -> Self {
        match error {
            AccessError::Api(error) => error,
            // Compiler-owned writebacks keep the tag protected by the emitted
            // mutable loan. A changed tag there indicates inconsistent TIR.
            AccessError::InactiveVariant => {
                invalid("inactive variant in protected storage operation")
            }
            AccessError::OutOfBounds => invalid("out-of-bounds protected storage operation"),
        }
    }
}

#[derive(Clone, Copy)]
enum AccessStep {
    Field(usize),
    VariantField { variant: u32, field: usize },
    Index(usize),
}
impl AccessStep {
    /// The field ordinal, the variant it requires and whether it is an index.
    fn parts(self) -> (usize, Option<u32>, bool) {
        match self {
            Self::Field(index) => (index, None, false),
            Self::VariantField { variant, field } => (field, Some(variant), false),
            Self::Index(index) => (index, None, true),
        }
    }
    fn missing(indexed: bool) -> AccessError {
        if indexed {
            AccessError::OutOfBounds
        } else {
            invalid("field out of range").into()
        }
    }
}

pub(crate) struct Continuation {
    /// None for a diverging call, whose callee never returns normally.
    pub returns: Option<hirsel_tir::CallReturn>,
    pub on_failure: CleanupId,
    pub writebacks: Vec<(PlaceId, LocalId)>,
}
/// Borrow-free immutable caller metadata. Cloning the definition handle allocates nothing.
#[derive(Clone, Debug)]
pub(crate) struct CallerSite {
    pub definition: DefinitionRef,
    pub body: BodyId,
    pub site: hirsel_span::Span,
    pub line: u32,
}
impl CallerSite {
    pub fn new(
        frame: &Frame,
        site: hirsel_span::Span,
        sources: &hirsel_span::SourceMap,
    ) -> Result<Self, Error> {
        Ok(Self {
            definition: frame.definition.clone(),
            body: frame.body_id,
            site,
            line: sources
                .location(site)
                .ok_or_else(|| invalid("caller site absent from source map"))?
                .line,
        })
    }
    pub fn anchor(&self) -> crate::observation::Anchor {
        crate::observation::Anchor {
            definition: self.definition.path,
            body: self.body,
            site: self.site,
            cleanup: None,
        }
    }
}
pub(crate) struct DropWait {
    pub result: crate::OperationResult,
    pub anchor: crate::observation::Anchor,
    pub caller: Option<CallerSite>,
}
pub(crate) struct Frame {
    pub depth: crate::depth::Task,
    pub activation: Option<crate::depth::Guard>,
    /// Arena primitive remains active while its synchronous callback executes.
    pub enclosing_activation: Option<crate::depth::Guard>,
    pub caller: Option<CallerSite>,
    pub definition: DefinitionRef,
    /// The session interner the definition's TypeIds are rows of.
    pub types: std::sync::Arc<hirsel_types::TypeInterner>,
    pub body_id: BodyId,
    pub mut_parameters: Vec<LocalId>,
    pub operation: Option<Box<(crate::OperationResult, crate::resources::OperationState)>>,
    pub callback: Option<Box<crate::resources::Callback>>,
    pub drop_wait: Option<Box<DropWait>>,
    pub work: std::collections::VecDeque<crate::resources::Work>,
    pub ending: Option<crate::resources::Ending>,
    pub finalized: Option<Box<(LocalId, crate::observation::Anchor, Option<CallerSite>)>>,
    pub loop_owned: Vec<(Value, crate::observation::Anchor)>,
    pub slots: Vec<Option<Value>>,
    pub sizes: crate::value::SizeBindings,
    pub type_arguments: crate::type_arguments::Bindings,
    pub block: BlockId,
    pub instruction: usize,
    /// A simulator pre-store point must fire once, before extracting call operands.
    pub store_point_passed: bool,
    pub continuation: Option<Continuation>,
    pub depth_failure: bool,
    /// Constructor-tail continuations at or below this frame: one logical
    /// activation each shares with its caller (LANG-10).
    pub restarts: usize,
    /// The site of an origin binding whose place is out of bounds or names an
    /// inactive variant. The access that then fails reports this site: the
    /// expression naming the place, not the statement reading it (docs/spec/diagnostics.md).
    pub place_failure_site: Option<hirsel_span::Span>,
    /// Lifetimes of this frame's values under checked execution.
    pub check: Option<crate::checked::FrameCheck>,
    /// An `arena.bounded` callback's boundary (MEM-6).
    pub bounded: Option<crate::bounded::Boundary>,
    pub arena_scope: Option<u64>,
    /// The next block start charges fuel: the frame was entered or jumped back.
    pub fuel_due: bool,
    /// This body's dispatch form in the run's `Codes`, resolved at its first step.
    pub code: Option<u32>,
    pub places: Option<std::rc::Rc<[crate::code::PlaceCode]>>,
    /// Reused projection workspace. Resolving a place never allocates a Vec.
    steps: std::cell::RefCell<Vec<AccessStep>>,
}
impl Frame {
    /// Replace value storage while retaining the lexical size environment and
    /// the caller's writeback/finalizer continuation. The witness proves that
    /// old-frame owners are moved, plain, or carried by `loop_owned`.
    pub fn restart_tail(&mut self, arguments: Vec<Value>) -> Result<(), Error> {
        let body = self.body();
        if arguments.len() != body.parameters.len() {
            return Err(invalid("tail parameter arity"));
        }
        let parameters = body.parameters.clone();
        let entry = body.entry;
        let mut slots = vec![None; body.locals.len()];
        for (local, value) in parameters.into_iter().zip(arguments) {
            let slot = slots
                .get_mut(local.0 as usize)
                .ok_or_else(|| invalid("tail parameter index"))?;
            if slot.is_some() {
                return Err(invalid("duplicate tail parameter"));
            }
            *slot = Some(value);
        }
        self.slots = slots;
        self.goto(entry);
        // A self-tail call is a function entry, wherever its body begins.
        self.fuel_due = true;
        Ok(())
    }

    pub fn size(&self, parameter: hirsel_types::SizeParameterRef) -> Result<u64, Error> {
        let hirsel_types::SizeParameterRef::Body { scope, parameter } = parameter else {
            return Err(invalid("unclosed generic size parameter"));
        };
        self.sizes
            .get(&(scope, parameter))
            .copied()
            .ok_or_else(|| invalid("unbound generic size parameter"))
    }
    /// Bind the actual arguments of exactly one invocation, preserving captures.
    pub fn bind_sizes(
        sizes: &mut crate::value::SizeBindings,
        scope: hirsel_types::SiteRef,
        arguments: impl IntoIterator<Item = Option<u64>>,
    ) {
        for (ordinal, size) in arguments.into_iter().enumerate() {
            if let Some(size) = size {
                sizes.insert((scope, hirsel_types::SizeParamId(ordinal as u32)), size);
            }
        }
    }
    /// The entry frame. External inputs are canonicalized here (SEM-2); every
    /// value the evaluator builds afterwards is canonical by construction.
    pub fn new(
        types: std::sync::Arc<hirsel_types::TypeInterner>,
        definition: DefinitionRef,
        mut arguments: Vec<Value>,
    ) -> Result<Self, Error> {
        arguments.iter_mut().for_each(Value::canonicalize);
        let entry = definition.entry;
        Self::new_body(
            types,
            definition,
            entry,
            arguments,
            vec![],
            Default::default(),
        )
    }
    pub fn new_body(
        types: std::sync::Arc<hirsel_types::TypeInterner>,
        definition: DefinitionRef,
        body_id: BodyId,
        arguments: Vec<Value>,
        captures: Vec<Option<Value>>,
        sizes: crate::value::SizeBindings,
    ) -> Result<Self, Error> {
        let body = definition
            .bodies
            .get(body_id.0 as usize)
            .ok_or_else(|| invalid("missing entry body"))?;
        if body.parameters.len() != arguments.len() {
            return Err(invalid("parameter count mismatch"));
        }
        let mut slots = vec![None; body.locals.len()];
        for (local, value) in body.parameters.iter().zip(arguments) {
            let slot = slots
                .get_mut(local.0 as usize)
                .ok_or_else(|| invalid("parameter slot out of range"))?;
            if slot.is_some() {
                return Err(invalid("duplicate parameter slot"));
            }
            *slot = Some(value);
        }
        let capture_slots: Vec<_> = body
            .locals
            .iter()
            .enumerate()
            .filter(|(_, l)| l.storage == Storage::Capture)
            .map(|(i, _)| i)
            .collect();
        if capture_slots.len() != captures.len() {
            return Err(invalid("capture count mismatch"));
        }
        for (index, value) in capture_slots.into_iter().zip(captures) {
            slots[index] = value;
        }
        let block = body.entry;
        let depth = crate::depth::Task::default();
        let activation = depth.admit();
        Ok(Self {
            depth,
            activation,
            enclosing_activation: None,
            caller: None,
            type_arguments: Default::default(),
            sizes,
            definition,
            types,
            body_id,
            mut_parameters: vec![],
            operation: None,
            drop_wait: None,
            callback: None,
            work: std::collections::VecDeque::new(),
            ending: None,
            finalized: None,
            loop_owned: vec![],
            slots,
            block,
            instruction: 0,
            store_point_passed: false,
            continuation: None,
            depth_failure: false,
            restarts: 0,
            place_failure_site: None,
            check: None,
            bounded: None,
            arena_scope: None,
            fuel_due: true,
            code: None,
            places: None,
            steps: Default::default(),
        })
    }
    pub fn body(&self) -> &Body {
        &self.definition.bodies[self.body_id.0 as usize]
    }
    /// A provider-owned child has no source stack to retain at result
    /// readiness. Give its cleanup work the caller's immutable source/type
    /// metadata and empty storage; no instruction of that body executes.
    pub fn cleanup_shell(&self) -> Result<Self, Error> {
        let body = self.body();
        let captures = body
            .locals
            .iter()
            .filter(|l| l.storage == Storage::Capture)
            .count();
        let mut frame = Self::new_body(
            self.types.clone(),
            self.definition.clone(),
            self.body_id,
            vec![Value::Unit; body.parameters.len()],
            vec![None; captures],
            self.sizes.clone(),
        )?;
        frame.slots.fill(None);
        frame.activation = None;
        frame.depth = self.depth.clone();
        frame.type_arguments = self.type_arguments.clone();
        frame.block = self.block;
        frame.caller = self.caller.clone();
        if let Some(check) = &self.check {
            let mut ended = crate::eval::enter_checked(&check.state, &frame, vec![], None)?;
            // Ordinary holding frames ended their storage lifetimes before
            // result readiness too. The rest retains the checking state.
            ended.exit();
            frame.check = Some(ended);
        }
        Ok(frame)
    }
    pub fn place(&self, id: PlaceId) -> Result<&Place, Error> {
        self.body()
            .places
            .get(id.0 as usize)
            .ok_or_else(|| invalid("place out of range"))
    }
    fn indices<'a>(
        &self,
        id: PlaceId,
        scratch: &'a std::cell::RefCell<Vec<AccessStep>>,
    ) -> Result<std::cell::Ref<'a, [AccessStep]>, AccessError> {
        use crate::code::ProjectionCode;
        let mut steps = scratch.borrow_mut();
        steps.clear();
        let place = self.place(id)?;
        let resolved = self
            .places
            .as_ref()
            .and_then(|places| places.get(id.0 as usize));
        let root = resolved.map_or(place.root.0 as usize, |place| place.root);
        let count = resolved.map_or(place.projection.len(), |place| place.projection.len());
        let mut value = self.slots.get(root).and_then(Option::as_ref);
        for ordinal in 0..count {
            let projection = resolved.map_or_else(
                || ProjectionCode::from(&place.projection[ordinal]),
                |place| place.projection[ordinal],
            );
            let step = match projection {
                ProjectionCode::Field(field) => AccessStep::Field(field),
                ProjectionCode::VariantField { variant, field } => {
                    AccessStep::VariantField { variant, field }
                }
                ProjectionCode::Index(index) => {
                    let index = match self.slots.get(index).and_then(Option::as_ref) {
                        Some(Value::Integer(i)) if !i.is_signed() || i.signed() >= 0 => {
                            usize::try_from(i.bits()).map_err(|_| AccessError::OutOfBounds)?
                        }
                        _ => return Err(invalid("invalid index operand").into()),
                    };
                    if matches!(value, Some(Value::Record { definition, .. }) if definition.is_core("SortedMap"))
                    {
                        steps.push(AccessStep::Field(1));
                        value = value
                            .and_then(Value::fields)
                            .and_then(|f| f.get(1))
                            .and_then(Option::as_ref);
                    }
                    AccessStep::Index(index)
                }
            };
            let (index, _, _) = step.parts();
            value = value
                .and_then(Value::fields)
                .and_then(|f| f.get(index))
                .and_then(Option::as_ref);
            steps.push(step);
        }
        drop(steps);
        Ok(std::cell::Ref::map(scratch.borrow(), |steps| &**steps))
    }
    pub fn slot(&mut self, id: PlaceId) -> Result<&mut Option<Value>, AccessError> {
        let place = self.place(id)?;
        let root = place.root.0 as usize;
        if place.projection.is_empty() {
            return self
                .slots
                .get_mut(root)
                .ok_or_else(|| invalid("local out of range").into());
        }
        let indices = self.indices(id, &self.steps)?;
        let mut slot = self
            .slots
            .get_mut(root)
            .ok_or_else(|| invalid("local out of range"))?;
        for &step in indices.iter() {
            let (index, variant, indexed) = step.parts();
            let fields = match slot.as_mut() {
                // Bytes indexing yields a scalar snapshot, not a storage slot.
                // Origin probes still need its bounds error to retain the
                // expression's site until the actual operand read fails.
                Some(Value::Bytes(bytes) | Value::ArenaBytes { bytes, .. })
                    if indexed && index >= bytes.len() =>
                {
                    return Err(AccessError::OutOfBounds);
                }
                Some(v)
                    if variant.is_none()
                        && !matches!(v, Value::Variant { .. } | Value::ResourceVariant { .. }) =>
                {
                    v.fields_mut()
                        .ok_or_else(|| invalid("projection on scalar"))?
                }
                Some(
                    Value::Variant {
                        variant: tag,
                        fields,
                    }
                    | Value::ResourceVariant {
                        variant: tag,
                        fields,
                        ..
                    },
                ) if variant == Some(*tag) => fields,
                Some(
                    Value::Variant { variant: tag, .. }
                    | Value::ResourceVariant { variant: tag, .. },
                ) if variant.is_some() && variant != Some(*tag) => {
                    return Err(AccessError::InactiveVariant);
                }
                _ => return Err(invalid("projection on dead or incompatible storage").into()),
            };
            slot = fields
                .get_mut(index)
                .ok_or_else(|| AccessStep::missing(indexed))?;
        }
        Ok(slot)
    }
    /// The storage at a place, read without writing: unlike `slot`, it never
    /// detaches a shared block on the path. Errors are `slot`'s.
    pub fn slot_ref(&self, id: PlaceId) -> Result<&Option<Value>, AccessError> {
        let place = self.place(id)?;
        let mut slot = self
            .slots
            .get(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?;
        if place.projection.is_empty() {
            return Ok(slot);
        }
        for &step in self.indices(id, &self.steps)?.iter() {
            let (index, variant, indexed) = step.parts();
            let fields = match slot.as_ref() {
                Some(v)
                    if variant.is_none()
                        && !matches!(v, Value::Variant { .. } | Value::ResourceVariant { .. }) =>
                {
                    v.fields().ok_or_else(|| invalid("projection on scalar"))?
                }
                Some(
                    Value::Variant {
                        variant: tag,
                        fields,
                    }
                    | Value::ResourceVariant {
                        variant: tag,
                        fields,
                        ..
                    },
                ) if variant == Some(*tag) => fields,
                Some(
                    Value::Variant { variant: tag, .. }
                    | Value::ResourceVariant { variant: tag, .. },
                ) if variant.is_some() && variant != Some(*tag) => {
                    return Err(AccessError::InactiveVariant);
                }
                _ => return Err(invalid("projection on dead or incompatible storage").into()),
            };
            slot = fields
                .get(index)
                .ok_or_else(|| AccessStep::missing(indexed))?;
        }
        Ok(slot)
    }
    /// MEM-6: a write at `id`, or a `mut` lend of it (`lend`), writes through every
    /// arena List or Array on its path. One reached through another block is
    /// shared first, since that block's run-time count is not a semantic fact; a
    /// shared one is copied within its arena before the write, charged its length,
    /// and the place holds the unshared copy. A lent value reached through a block
    /// is shared too, so the callee's first write copies it. Returns the arena
    /// whose budget a copy exceeded. A path that does not resolve is left to the
    /// access that follows, which reports it.
    pub fn arena_write(
        &mut self,
        id: PlaceId,
        lend: bool,
        mut charge: impl FnMut(u64, u64) -> Result<(), u64>,
    ) -> Result<Option<u64>, AccessError> {
        let place = self.place(id)?;
        // A whole local crosses no block, so nothing on its path is shared.
        if place.projection.is_empty() {
            return Ok(None);
        }
        let root = place.root.0 as usize;
        let Ok(steps) = self.indices(id, &self.steps) else {
            return Ok(None);
        };
        let Some(mut value) = self.slots.get_mut(root).and_then(Option::as_mut) else {
            return Ok(None);
        };
        let mut crossed = false;
        for &step in steps.iter() {
            let index = match step {
                AccessStep::Field(i) | AccessStep::Index(i) => i,
                AccessStep::VariantField { variant, field } => {
                    if !matches!(value, Value::Variant { variant: tag, .. } if *tag == variant) {
                        return Ok(None);
                    }
                    field
                }
            };
            if let Value::Sequence {
                arena: Some(storage),
                elements,
                ..
            } = value
            {
                if crossed {
                    storage.share();
                }
                // An index out of range fails before the write copies anything.
                if index >= elements.len() {
                    return Ok(None);
                }
                if storage.shared() {
                    let bytes = storage.element.saturating_mul(elements.len() as u64);
                    if let Err(arena) = charge(storage.arena, bytes) {
                        return Ok(Some(arena));
                    }
                    *storage = storage.detached();
                    // The original and the copy both hold every element.
                    elements
                        .iter()
                        .flatten()
                        .for_each(Value::share_arena_storage);
                }
            }
            crossed |= value.is_block();
            let Some(next) = value
                .fields_mut()
                .and_then(|fields| fields.get_mut(index))
                .and_then(Option::as_mut)
            else {
                return Ok(None);
            };
            value = next;
        }
        if lend && crossed {
            value.share_arena_storage();
        }
        Ok(None)
    }
    /// Observe table patterns without extracting, copying, or changing live bits.
    pub fn pattern_matches(&self, table: &MatchTable, root: MatchPatternId) -> Result<bool, Error> {
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            let pattern = &table.patterns[id.0 as usize];
            let value = self.observe(pattern.place)?;
            match (&pattern.kind, value) {
                (MatchPatternKind::Wildcard | MatchPatternKind::Binding { .. }, _) => (),
                (MatchPatternKind::Literal(c), v) => {
                    let matches = match (c, v) {
                        (Constant::Unit, Value::Unit) => true,
                        (Constant::Bool(a), Value::Bool(b)) => a == b,
                        (Constant::Integer(a), Value::Integer(b)) => *a == b.bits(),
                        (Constant::Integer(a), Value::Char(b)) => *a == u128::from(*b),
                        (Constant::F32(a), Value::F32(b)) => {
                            f32::from_bits(*a) == f32::from_bits(*b)
                        }
                        (Constant::F64(a), Value::F64(b)) => {
                            f64::from_bits(*a) == f64::from_bits(*b)
                        }
                        (
                            Constant::String(a),
                            Value::String(b) | Value::ArenaString { text: b, .. },
                        ) => a == b,
                        (
                            Constant::Bytes(a),
                            Value::Bytes(b) | Value::ArenaBytes { bytes: b, .. },
                        ) => a == b,
                        _ => false,
                    };
                    if !matches {
                        return Ok(false);
                    }
                }
                (
                    MatchPatternKind::Range {
                        start: Constant::Integer(start),
                        end: Constant::Integer(end),
                    },
                    v @ (Value::Integer(_) | Value::Char(_)),
                ) => {
                    let (primitive, bits) = match v {
                        Value::Integer(n) => (n.kind(), n.bits()),
                        Value::Char(c) => (hirsel_types::Primitive::Char, u128::from(*c)),
                        _ => unreachable!(),
                    };
                    let key = |bits| primitive.pattern_key(bits);
                    let inside = matches!(
                        (key(*start), key(bits), key(*end)),
                        (Some(low), Some(value), Some(high)) if low <= value && value <= high
                    );
                    if !inside {
                        return Ok(false);
                    }
                }
                (
                    MatchPatternKind::Variant { variant, .. },
                    Value::Variant { variant: tag, .. }
                    | Value::ResourceVariant { variant: tag, .. },
                ) => {
                    if variant != tag {
                        return Ok(false);
                    }
                }
                (MatchPatternKind::Tuple(_), Value::Tuple(_) | Value::Unit)
                | (
                    MatchPatternKind::Record { .. },
                    Value::Record { .. } | Value::Resource { .. } | Value::Distinct { .. },
                ) => (),
                _ => return Err(invalid("table pattern/value shape")),
            }
            pending.extend(pattern.kind.fields().iter().rev().map(|(_, child)| *child));
        }
        Ok(true)
    }
    pub fn observe(&self, id: PlaceId) -> Result<&Value, Error> {
        let place = self.place(id)?;
        let mut value = self.slots[place.root.0 as usize]
            .as_ref()
            .ok_or_else(|| invalid("match on dead storage"))?;
        for &step in self.indices(id, &self.steps)?.iter() {
            let (field, variant) = match step {
                AccessStep::Field(n) | AccessStep::Index(n) => (n, None),
                AccessStep::VariantField { variant, field } => (field, Some(variant)),
            };
            if let Some(expected) = variant {
                if !matches!(value, Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. } if *variant == expected)
                {
                    return Err(invalid("match payload tag"));
                }
            }
            value = value
                .fields()
                .and_then(|fs| fs.get(field))
                .and_then(Option::as_ref)
                .ok_or_else(|| invalid("match field missing"))?;
        }
        Ok(value)
    }
    pub fn operand(&mut self, operand: &Operand) -> Result<Value, AccessError> {
        if self.check.is_some() {
            return self.operand_shadow(operand).map(|(value, _)| value);
        }
        let value = self.extract(operand)?;
        if operand.usage == UseKind::Copy {
            value.share_arena_storage();
        }
        Ok(value)
    }
    /// Extract an operand with its bound; checked execution traps on an ended one.
    pub fn operand_shadow(
        &mut self,
        operand: &Operand,
    ) -> Result<(Value, crate::checked::Shadow), AccessError> {
        if self.check.is_none() {
            let value = self.extract(operand)?;
            if operand.usage == UseKind::Copy {
                value.share_arena_storage();
            }
            return Ok((value, Default::default()));
        }
        // A whole-local Take can transfer the existing shadow tree with its
        // value. Validate the original before extraction; preserved mutable
        // inline storage keeps its shadow and therefore still needs a copy.
        if operand.usage == UseKind::Take {
            let place = self.place(operand.place)?;
            if place.projection.is_empty() {
                let root = place.root.0 as usize;
                let check = self.check.as_ref().unwrap();
                let shadow = check
                    .shadows
                    .get(root)
                    .ok_or_else(|| invalid("local out of range"))?;
                let state = check.state.borrow();
                state.access(shadow, "operand")?;
                if !shadow.claim(state.task) {
                    return Err(state
                        .trap(
                            crate::checked::TrapRule::TaskShared,
                            format!(
                                "task {} reaches a counted block another task owns",
                                state.task
                            ),
                        )
                        .into());
                }
                drop(state);
                let value = self.extract(operand)?;
                let emptied = self.slots[root].is_none();
                let check = self.check.as_mut().unwrap();
                let shadow = if emptied {
                    check.copies.remove(&(root as u32));
                    check.borrowed.remove(&(root as u32));
                    std::mem::take(&mut check.shadows[root])
                } else {
                    check.shadows[root].clone()
                };
                let shadow = shadow.for_value(&value);
                return Ok((value, shadow));
            }
        }
        let mut shadow = self.access(operand.place, "operand")?;
        if let Some(check) = &self.check {
            let state = check.state.borrow();
            if !shadow.claim(state.task) {
                return Err(state
                    .trap(
                        crate::checked::TrapRule::TaskShared,
                        format!(
                            "task {} reaches a counted block another task owns",
                            state.task
                        ),
                    )
                    .into());
            }
        }
        let value = self.extract(operand)?;
        // Administrative moves transfer the holder with the value. Leaving
        // its shadow in an emptied staging slot would keep a later-dropped
        // counted alias alive until EndScope. A mut inline handback may keep
        // its source value, so clear only storage extraction actually emptied.
        if operand.usage == UseKind::Take && self.slot_ref(operand.place).is_ok_and(Option::is_none)
        {
            self.set_shadow(operand.place, Default::default())?;
        }
        if operand.usage == UseKind::Copy {
            if let Some(check) = &self.check {
                shadow = shadow.copy_out(&value, &check.state.borrow());
            }
            value.share_arena_storage();
        }
        let shadow = shadow.for_value(&value);
        Ok((value, shadow))
    }
    fn extract(&mut self, operand: &Operand) -> Result<Value, AccessError> {
        if let Some(local) = self
            .places
            .as_ref()
            .and_then(|places| places.get(operand.place.0 as usize))
            .filter(|place| place.projection.is_empty())
            .map(|place| crate::code::Local {
                slot: place.root as u32,
                usage: operand.usage,
                once: place.once,
            })
            && let Some(value) = self.local_operand(local)
        {
            return Ok(value);
        }
        // Bytes indexing is a scalar snapshot, never mutable byte storage.
        // Retain ordinary address bounds and ownership access for the parent.
        let place = self.place(operand.place)?;
        let steps = self.indices(operand.place, &self.steps)?;
        let mut source = self.slots[place.root.0 as usize].as_ref();
        for (depth, step) in steps.iter().enumerate() {
            if let (
                Some(Value::Bytes(bytes) | Value::ArenaBytes { bytes, .. }),
                AccessStep::Index(index),
            ) = (source, step)
            {
                if depth + 1 != steps.len() || operand.usage == UseKind::Mut {
                    return Err(invalid("mutable byte projection").into());
                }
                let byte = bytes.get(*index).ok_or(AccessError::OutOfBounds)?;
                return Ok(Value::Integer(
                    crate::Integer::new(hirsel_types::Primitive::U8, u128::from(*byte)).unwrap(),
                ));
            }
            let field = match step {
                AccessStep::Field(i)
                | AccessStep::Index(i)
                | AccessStep::VariantField { field: i, .. } => *i,
            };
            source = source
                .and_then(Value::fields)
                .and_then(|fields| fields.get(field))
                .and_then(Option::as_ref);
        }
        drop(steps);
        if operand.usage == UseKind::Copy {
            let ty = self
                .types
                .get(self.place(operand.place)?.ty)
                .ok_or_else(|| invalid("operand type"))?;
            if ty.modes.multiplicity == hirsel_types::Multiplicity::Once {
                return Err(invalid("copy of once value").into());
            }
            let value = self
                .slot_ref(operand.place)?
                .as_ref()
                .ok_or_else(|| invalid("dead storage"))?;
            let mut work = vec![value];
            while let Some(v) = work.pop() {
                if matches!(v, Value::Resource { .. } | Value::ResourceVariant { .. }) {
                    return Err(invalid("copy of resource-containing value").into());
                }
                if let Value::Closure {
                    captures,
                    environment,
                    ..
                } = v
                {
                    if captures.len() != environment.owned_captures.len() {
                        return Err(invalid("capture ownership count").into());
                    }
                    work.extend(
                        captures
                            .iter()
                            .zip(&environment.owned_captures)
                            .filter_map(|(v, owned)| owned.then(|| v.as_ref()).flatten()),
                    );
                } else if let Some(fields) = v.fields() {
                    work.extend(fields.iter().filter_map(Option::as_ref));
                }
            }
            value.acquire_roots();
        }
        let root = self.place(operand.place)?.root;
        let preserve = self.mut_parameters.contains(&root);
        // Only a move writes its source; every other use reads it, sharing blocks.
        let slot = self.slot_ref(operand.place)?;
        match operand.usage {
            UseKind::Take if !(preserve && slot.as_ref().is_some_and(Value::inline_bits)) => {
                self.slot(operand.place)?.take()
            }
            _ => slot.clone(),
        }
        .ok_or_else(|| invalid("use of dead storage").into())
    }
    /// A whole local's live inert value, when reading it as `local` cannot fail.
    pub fn peek(&self, local: crate::code::Local) -> Option<&Value> {
        let value = self.slots.get(local.slot as usize)?.as_ref()?;
        (value.inert() && !(local.once && local.usage == UseKind::Copy)).then_some(value)
    }
    /// Finish reading a peeked local: a move empties the slot, as `extract` does.
    pub fn commit(&mut self, local: crate::code::Local) {
        if local.usage == UseKind::Take && !self.preserves(local.slot) {
            self.slots[local.slot as usize] = None;
        }
    }
    /// The operand of a whole local, as `extract` gives it, when that needs no walk
    /// and cannot fail: a move or borrow of a live value or a copy of an inert one.
    /// None leaves the slot as it was.
    pub fn local_operand(&mut self, local: crate::code::Local) -> Option<Value> {
        let value = self.slots.get(local.slot as usize)?.as_ref()?;
        match local.usage {
            UseKind::Copy => (value.inert() && !local.once).then(|| value.clone()),
            UseKind::Take if !self.preserves(local.slot) => self.slots[local.slot as usize].take(),
            _ => Some(value.clone()),
        }
    }
    /// A move out of a `mut` parameter holding inline bits leaves it in place.
    fn preserves(&self, slot: u32) -> bool {
        self.mut_parameters.contains(&LocalId(slot))
            && self.slots[slot as usize]
                .as_ref()
                .is_some_and(Value::inline_bits)
    }
    pub fn assign(&mut self, destination: PlaceId, value: Value) -> Result<(), AccessError> {
        self.assign_shadow(destination, value, Default::default())
    }
    pub fn assign_shadow(
        &mut self,
        destination: PlaceId,
        value: Value,
        shadow: crate::checked::Shadow,
    ) -> Result<(), AccessError> {
        let slot = self.slot(destination)?;
        if slot.is_some() {
            return Err(invalid("assignment to live storage requires an explicit drop").into());
        }
        *slot = Some(value);
        self.set_shadow(destination, shadow)?;
        Ok(())
    }
    /// Install storage content at a place, live or dead, with its bound.
    pub fn put(
        &mut self,
        place: PlaceId,
        value: Option<Value>,
        shadow: crate::checked::Shadow,
    ) -> Result<(), AccessError> {
        *self.slot(place)? = value;
        self.set_shadow(place, shadow)?;
        Ok(())
    }
    /// The bound at a place, after checking that nothing on it has ended.
    pub fn access(&self, id: PlaceId, what: &str) -> Result<crate::checked::Shadow, AccessError> {
        let Some(check) = &self.check else {
            return Ok(Default::default());
        };
        let shadow = self.shadow_of(id)?;
        check.state.borrow().access(&shadow, what)?;
        Ok(shadow)
    }
    /// A write through a projection reads every enclosing block on its path.
    pub fn write_access(&self, id: PlaceId) -> Result<(), AccessError> {
        use crate::checked::Shadow;
        let Some(check) = &self.check else {
            return Ok(());
        };
        let place = self.place(id)?;
        if check.noalloc && !place.projection.is_empty() {
            let root = &self.body().locals[place.root.0 as usize];
            let row = self
                .types
                .get(root.ty)
                .ok_or_else(|| invalid("write root type"))?;
            // Inline fields are independent stores. Traversing a counted block
            // needs its own uniqueness; arena descendants always detach (MEM-4).
            let mut value = self.slots[place.root.0 as usize].as_ref();
            let mut ownership = check.shadows[place.root.0 as usize].clone();
            for (depth, step) in self.indices(id, &self.steps)?.iter().enumerate() {
                if value.is_some_and(|v| {
                    crate::checked::inline_fields(v).is_none() && crate::checked::holds_storage(v)
                }) && (!ownership.is_unique()
                    || depth > 0
                        && row.modes.relation.reaches().iter().any(|r| {
                            matches!(r, hirsel_types::Reach::Place(o) if self.arena_of(o).is_some())
                        }))
                {
                    return Err(check
                        .state
                        .borrow()
                        .trap(
                            crate::checked::TrapRule::AllocationInNoalloc,
                            "projected mutation may detach shared or arena storage",
                        )
                        .into());
                }
                let index = match *step {
                    AccessStep::Field(i)
                    | AccessStep::VariantField { field: i, .. }
                    | AccessStep::Index(i) => i,
                };
                ownership = ownership.part(index);
                value = value
                    .and_then(Value::fields)
                    .and_then(|f| f.get(index))
                    .and_then(Option::as_ref);
            }
        }
        let mut node = check
            .shadows
            .get(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?;
        for step in place.projection.iter() {
            let index = match step {
                Projection::Field(i) | Projection::VariantField { field: i, .. } => {
                    Some(*i as usize)
                }
                Projection::Index(_) => None,
            };
            match (node, index) {
                (Shadow::Parts(parts), Some(index)) => match parts.get(index) {
                    Some(part) => node = part,
                    None => break,
                },
                _ => {
                    check.state.borrow().access(node, "write")?;
                    break;
                }
            }
        }
        Ok(())
    }
    /// The shadow subtree of a place, along the same steps as its value: fields
    /// of inline aggregates descend, and an element of a block or an index keeps
    /// the enclosing bound. An element place yields the element's own subtree,
    /// never its container's, so writing it back cannot nest the container's
    /// shadow inside one of its elements (which grew it fourfold per level of a
    /// recursive build into a shared list).
    pub fn shadow_of(&self, id: PlaceId) -> Result<crate::checked::Shadow, Error> {
        use crate::checked::Shadow;
        let Some(check) = &self.check else {
            return Ok(Shadow::default());
        };
        let place = self.place(id)?;
        let root = check
            .shadows
            .get(place.root.0 as usize)
            .ok_or_else(|| invalid("local out of range"))?;
        Ok(
            root.project(self.indices(id, &self.steps)?.iter().map(|&step| {
                let (index, _, indexed) = step.parts();
                (index, indexed)
            })),
        )
    }
    /// Replace moves caller-owned content, excluding the callee's access root.
    /// Real region and arena bounds remain attached to the displaced owner.
    pub fn displaced_shadow(&self, id: PlaceId) -> Result<crate::checked::Shadow, Error> {
        let shadow = self.shadow_of(id)?;
        let place = self.place(id)?;
        if self.mut_parameters.contains(&place.root)
            && let Some(check) = &self.check
            && let Some(parameter) = self.body().parameters.iter().position(|p| *p == place.root)
        {
            return Ok(check.caller_content(parameter, shadow));
        }
        Ok(shadow)
    }
    /// Record a written value's bound: an `in a` destination row places it in
    /// a's arena (PROV-4); inline aggregates keep one bound per field.
    pub fn set_shadow(
        &mut self,
        id: PlaceId,
        mut shadow: crate::checked::Shadow,
    ) -> Result<(), Error> {
        use crate::checked::Shadow;
        if self.check.is_none() {
            return Ok(());
        }
        let place = self.place(id)?.clone();
        let holds = self
            .observe(id)
            .map(crate::checked::holds_storage)
            .unwrap_or(false);
        if let Some(arena) = self.placed_arena(place.ty)
            && holds
        {
            shadow.add_all(arena);
        }
        // MODE-2: a whole local declared `unique` holds only values no other
        // live holder shares, whatever either checker proved.
        if holds
            && place.projection.is_empty()
            && self
                .types
                .get(place.ty)
                .is_some_and(|r| r.modes.uniqueness == hirsel_types::Uniqueness::Unique)
            && !shadow.sole_owner()
        {
            let check = self.check.as_ref().unwrap();
            return Err(check.state.borrow().trap(
                crate::checked::TrapRule::UniqueShared,
                format!(
                    "local {} is declared unique, but its block has another live holder",
                    place.root.0
                ),
            ));
        }
        let indices = self.indices(id, &self.steps)?;
        let (check, slots) = (&mut self.check, &mut self.slots);
        let check = check.as_mut().unwrap();
        let root = place.root.0 as usize;
        if place.projection.is_empty() {
            check.copies.remove(&place.root.0);
            check.borrowed.remove(&place.root.0);
        }
        let mut value = slots.get(root).and_then(Option::as_ref);
        let mut node = check
            .shadows
            .get_mut(root)
            .ok_or_else(|| invalid("local out of range"))?;
        for &step in indices.iter() {
            while let Shadow::Ownership { content, .. } = node {
                node = content;
            }
            let index = match step {
                AccessStep::Field(i)
                | AccessStep::VariantField { field: i, .. }
                | AccessStep::Index(i) => i,
            };
            let Some(fields) = value.and_then(Value::fields) else {
                node.add_bound(&shadow.total());
                return Ok(());
            };
            let bound = node.total();
            if value.and_then(crate::checked::inline_fields).is_none() {
                // Elements share a lifetime, but each counted child has its
                // own ownership witness for a later projected mutation.
                node.add_bound(&shadow.total());
                shadow.add_bound(&bound);
            }
            if !matches!(node, Shadow::Parts(parts) if parts.len() == fields.len()) {
                *node = Shadow::Parts(
                    fields
                        .iter()
                        .map(|field| {
                            field
                                .as_ref()
                                .map(|v| Shadow::of(bound.clone()).ownership(v, false))
                                .unwrap_or_default()
                        })
                        .collect(),
                );
            }
            let Shadow::Parts(parts) = node else {
                unreachable!()
            };
            node = parts
                .get_mut(index)
                .ok_or_else(|| invalid("write shadow field"))?;
            value = fields.get(index).and_then(Option::as_ref);
        }
        *node = shadow;
        Ok(())
    }
    /// The open arena named by a row placed `in a`, read from a's current slot.
    pub fn placed_arena(&self, ty: hirsel_types::TypeId) -> Option<u32> {
        let row = self.types.get(ty)?;
        row.modes.relation.reaches().iter().find_map(|r| match r {
            hirsel_types::Reach::Place(origin) => self.arena_of(origin),
            _ => None,
        })
    }
    /// Whether a row's relation places its blocks in an open arena or a region.
    pub fn placed(&self, ty: hirsel_types::TypeId) -> bool {
        self.types
            .get(ty)
            .is_some_and(|row| row.modes.relation.scoped(|o| self.arena_of(o).is_some()))
    }
    pub fn arena_of(&self, origin: &hirsel_types::Origin) -> Option<u32> {
        arena_of(self.body(), &self.slots, origin)
    }
    pub fn record_storage(
        &mut self,
        place: PlaceId,
        runtime: &crate::aggregate::RuntimeTypes,
        storage: &mut crate::sealed_storage::Storage,
        source: &crate::sealed_storage::AllocationSite,
    ) -> Result<(), AccessError> {
        let place_root = self.place(place)?.root;
        let path = self
            .indices(place, &self.steps)?
            .iter()
            .map(|step| step.parts().0)
            .collect::<Vec<_>>();
        let ty = self.body().locals[place_root.0 as usize].ty;
        let actuals = crate::type_arguments::concrete(
            runtime,
            &[hirsel_types::GenericArgument::Type(ty)],
            &self.type_arguments,
        )?;
        let [hirsel_types::GenericArgument::Type(ty)] = actuals.arguments.as_slice() else {
            return Err(invalid("closed retained root type").into());
        };
        let value = self.slots[place_root.0 as usize]
            .as_mut()
            .ok_or_else(|| invalid("dead retained update root"))?;
        storage.record_path(runtime, *ty, value, &path, source)?;
        Ok(())
    }
    pub fn goto(&mut self, block: BlockId) {
        // A jump to this block or an earlier one may close a loop: it charges fuel.
        self.fuel_due |= block.0 <= self.block.0;
        self.block = block;
        self.instruction = 0;
    }
    /// The logical bytes this frame holds (`budgets.limits@2`): its slots, values its
    /// loops carry, a suspended operation and its callbacks, and pending releases.
    pub fn logical_bytes(&self) -> u64 {
        use crate::resources::Work;
        let mut total = self
            .slots
            .iter()
            .flatten()
            .chain(self.loop_owned.iter().map(|(v, _)| v))
            .map(Value::logical_bytes)
            .fold(0u64, u64::saturating_add);
        if let Some((result, state)) = self.operation.as_deref() {
            total = total
                .saturating_add(crate::resources::result_bytes(result))
                .saturating_add(state.logical_bytes());
        }
        if let Some(callback) = &self.callback {
            total = total.saturating_add(callback.state.logical_bytes());
        }
        for work in &self.work {
            total = total.saturating_add(match work {
                Work::Release(value, ..) => value.logical_bytes(),
                Work::Resume(result, callback) => crate::resources::result_bytes(result)
                    .saturating_add(callback.state.logical_bytes()),
                Work::Action(..) | Work::Chain(..) | Work::Retire(..) | Work::RetireShadow(..) => 0,
            });
        }
        total
    }
}
impl Drop for Frame {
    fn drop(&mut self) {
        // Residual slots are borrowed snapshots or malformed-IR leftovers; TIR
        // cleanup already owns every semantic release. Dispose memory iteratively.
        for value in self.slots.iter_mut().filter_map(Option::take) {
            value.discard();
        }
        while let Some(work) = self.work.pop_front() {
            match work {
                crate::resources::Work::Release(value, ..) => value.discard(),
                crate::resources::Work::Resume(result, _) => {
                    crate::resources::discard_result(result)
                }
                crate::resources::Work::Action(..)
                | crate::resources::Work::Chain(..)
                | crate::resources::Work::Retire(..)
                | crate::resources::Work::RetireShadow(..) => (),
            }
        }
        if let Some(crate::resources::Ending::Return(value)) = self.ending.take() {
            value.discard();
        }
        if let Some(drop) = self.drop_wait.take() {
            crate::resources::discard_result(drop.result);
        }
        if let Some((result, _)) = self.operation.take().map(|operation| *operation) {
            crate::resources::discard_result(result);
        }
    }
}
/// The open arena a relation place names, read from its binding's current slot.
pub(crate) fn arena_of(
    body: &hirsel_tir::Body,
    slots: &[Option<Value>],
    origin: &hirsel_types::Origin,
) -> Option<u32> {
    if !origin.fields.is_empty() {
        return None;
    }
    let root = match &origin.root {
        hirsel_types::OriginRoot::Parameter(hirsel_types::ParameterRef::Body {
            scope,
            parameter,
        }) if *scope == body.parameter_scope => body.parameters.get(parameter.0 as usize),
        _ => None,
    }
    .copied()
    .or_else(|| {
        body.places
            .iter()
            .find(|p| {
                p.projection.is_empty()
                    && p.binding
                        .as_ref()
                        .is_some_and(|b| b.root == origin.root && b.fields.is_empty())
            })
            .map(|p| p.root)
    })?;
    match slots.get(root.0 as usize)?.as_ref()? {
        Value::Handle { kind, id } if kind == crate::checked::ARENA_HANDLE => {
            u32::try_from(*id).ok()
        }
        _ => None,
    }
}
