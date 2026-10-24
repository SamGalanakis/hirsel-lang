use super::closure_support::{children, resources_in};
use super::{Observer, TraceError};
use crate::Value;
use crate::frame::Frame;
use hirsel_run_schema::ownership_trace::{
    Context, Owner, Projection as Step, Storage, TaskPath, Trace, Tracker,
};
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::{DefId, DefPath};
use hirsel_tir::{InstructionKind, PlaceId, Rvalue, Terminator};
use hirsel_types::UseKind;
use std::collections::{BTreeMap, VecDeque};

struct FrameState {
    id: u64,
    finalizer: bool,
    aliases: BTreeMap<u32, Owner>,
    return_to: Option<Owner>,
    returned: Option<(Owner, Context)>,
}
struct CallState {
    definition: DefPath,
    body: Option<hirsel_tir::BodyId>,
    captures: Vec<(u32, Owner)>,
    arguments: Vec<(u32, UseKind, Owner)>,
    destination: Option<Owner>,
    context: Context,
}
enum Pending {
    Create {
        destination: PlaceId,
        fields: Vec<(Owner, u32, u64)>,
        context: Context,
    },
    Move {
        from: Owner,
        to: Owner,
        context: Context,
    },
    Alias {
        local: u32,
        from: Owner,
    },
    Defuse {
        owner: Owner,
        context: Context,
    },
}

pub(super) struct Resources {
    tracker: Tracker,
    frames: Vec<FrameState>,
    next_frame: u64,
    next_scratch: u64,
    releases: VecDeque<Owner>,
    pending: Option<Pending>,
    forget_source: Option<Owner>,
    forget: Option<DefPath>,
    call: Option<CallState>,
    error: Option<TraceError>,
}
impl Resources {
    pub fn new(forget: Option<DefPath>, event_limit: usize) -> Self {
        Self {
            tracker: Tracker::new(event_limit),
            frames: vec![],
            next_frame: 0,
            next_scratch: 0,
            releases: VecDeque::new(),
            pending: None,
            forget_source: None,
            forget,
            call: None,
            error: None,
        }
    }
    fn unsupported(&mut self, reason: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(TraceError::Unsupported(reason.into()));
        }
    }
    fn checked<T>(
        &mut self,
        result: Result<T, hirsel_run_schema::ownership_trace::Diagnostic>,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                if self.error.is_none() {
                    self.error = Some(TraceError::Consistency(error));
                }
                None
            }
        }
    }
    fn context(
        definition: DefId,
        body: hirsel_tir::BodyId,
        span: Span,
        sources: &SourceMap,
    ) -> Context {
        let anchor = crate::observation::Anchor {
            definition,
            body,
            cleanup: None,
            site: span,
        };
        let source = anchor.source_site(sources);
        let path = definition.path();
        let name = path
            .0
            .modules
            .iter()
            .chain(std::iter::once(&path.0.name))
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        Context {
            task: TaskPath::default(),
            operation: 0,
            site: hirsel_run_schema::Site {
                definition: format!("{}@{}::{name}", path.0.package.name, path.0.package.version),
                file: source.as_ref().map_or_else(String::new, |s| s.file.clone()),
                line: source.as_ref().map_or(0, |s| u64::from(s.line)),
                column: source.as_ref().map_or(0, |s| u64::from(s.column)),
            },
        }
    }
    fn at(frame: &Frame, span: Span, sources: &SourceMap) -> Context {
        Self::context(frame.definition.path, frame.body_id, span, sources)
    }
    fn slot(&self, local: u32) -> Option<Owner> {
        Some(Self::frame_slot(self.frames.last()?.id, local))
    }
    fn frame_slot(frame: u64, slot: u32) -> Owner {
        Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Slot { frame, slot },
            path: vec![],
        }
    }
    fn place(&self, frame: &Frame, place: PlaceId) -> Option<Owner> {
        let place = frame.place(place).ok()?;
        let state = self.frames.last()?;
        let mut owner = state
            .aliases
            .get(&place.root.0)
            .cloned()
            .or_else(|| self.slot(place.root.0))?;
        for projection in place.projection.iter() {
            owner.path.push(match *projection {
                hirsel_tir::Projection::Field(field) => Step::Field { field },
                hirsel_tir::Projection::VariantField { variant, field } => {
                    Step::VariantField { variant, field }
                }
                hirsel_tir::Projection::Index(local) => {
                    let Value::Integer(index) = frame.slots.get(local.0 as usize)?.as_ref()? else {
                        return None;
                    };
                    Step::Index {
                        index: u64::try_from(index.bits()).ok()?,
                    }
                }
            });
        }
        Some(owner)
    }
    fn destination(&self, frame: &Frame, place: PlaceId) -> Option<Owner> {
        let at = frame.place(place).ok()?;
        if at.projection.is_empty() {
            self.slot(at.root.0)
        } else {
            self.place(frame, place)
        }
    }
    fn resource(value: &Value) -> bool {
        matches!(
            value,
            Value::Resource { .. } | Value::ResourceVariant { .. }
        )
    }
    fn type_name(value: &Value) -> Option<String> {
        let (Value::Resource { definition, .. } | Value::ResourceVariant { definition, .. }) =
            value
        else {
            return None;
        };
        let path = definition.path();
        Some(format!(
            "{}@{}::{}",
            path.0.package.name,
            path.0.package.version,
            path.0
                .modules
                .iter()
                .chain(std::iter::once(&path.0.name))
                .cloned()
                .collect::<Vec<_>>()
                .join(".")
        ))
    }
    fn field(value: &Value, field: u32) -> Option<Step> {
        Some(match value {
            Value::Tuple(_)
            | Value::Record { .. }
            | Value::Distinct { .. }
            | Value::Resource { .. }
            | Value::Box(_)
            | Value::Closure { .. } => Step::Field { field },
            Value::Variant { variant, .. } | Value::ResourceVariant { variant, .. } => {
                Step::VariantField {
                    variant: *variant,
                    field,
                }
            }
            Value::Sequence { .. } => Step::Index {
                index: u64::from(field),
            },
            _ => return None,
        })
    }
    fn supported_tree(value: &Value) -> bool {
        let mut work = vec![value];
        while let Some(value) = work.pop() {
            if resources_in(value) == 0 {
                continue;
            }
            if Self::field(value, 0).is_none() {
                return false;
            }
            children(value, |_, child| work.push(child));
        }
        true
    }
    fn scratch(&mut self) -> Owner {
        let owner = Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Argument {
                call: self.next_scratch,
                parameter: 0,
            },
            path: vec![],
        };
        self.next_scratch += 1;
        owner
    }
    /// Called at the actual field-release boundary. Match release_fields' push
    /// front in declaration order, so the next destruction visits high to low.
    /// Each child's subtree is moved into its own cleanup custody; dropping a
    /// parent never fabricates drops for its children.
    fn release_fields(&mut self, value: &Value, owner: &Owner, context: &Context) {
        let mut fields = vec![];
        children(value, |index, child| fields.push((index, child)));
        for (index, child) in fields {
            if resources_in(child) == 0 {
                continue;
            }
            let Some(step) = Self::field(value, index as u32) else {
                self.unsupported("unsupported resource field destruction");
                return;
            };
            let mut from = owner.clone();
            from.path.push(step);
            let to = self.scratch();
            let result = self.tracker.move_tree(context, &from, &to);
            if self.checked(result).is_some() {
                self.releases.push_front(to);
            }
        }
    }
    pub fn finish(self, program: [u8; 32], completed: bool) -> Result<Trace, TraceError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        let complete = completed && self.frames.is_empty() && self.releases.is_empty();
        self.tracker
            .finish(program, complete)
            .map_err(TraceError::Consistency)
    }
}
impl Observer for Resources {
    const ENABLED: bool = true;
    fn run(&mut self, index: usize) {
        if index != 0 {
            self.unsupported("nested runner resource transport");
        }
    }
    fn sync(&mut self, task: u64, frames: &[Frame], sources: &SourceMap) {
        if self.error.is_some() {
            return;
        }
        if task != 0 {
            self.unsupported("task resource transport");
            return;
        }
        if self.call.is_some() {
            self.unsupported("call did not commit an observed user-frame entry");
            return;
        }
        if frames.len() < self.frames.len() {
            self.unsupported("unobserved frame retirement");
            return;
        }
        while self.frames.len() < frames.len() {
            let frame = &frames[self.frames.len()];
            let id = self.next_frame;
            self.next_frame += 1;
            let finalizer = frame.finalized.is_some();
            if !self.frames.is_empty() && !finalizer {
                self.unsupported("ordinary call resource transport");
                return;
            }
            self.frames.push(FrameState {
                id,
                finalizer,
                aliases: BTreeMap::new(),
                return_to: None,
                returned: None,
            });
            if let Some((local, anchor, _)) = frame.finalized.as_deref() {
                let Some(from) = self.releases.pop_front() else {
                    self.unsupported("finalizer has no observed incoming owner");
                    return;
                };
                let Some(to) = self.slot(local.0) else {
                    self.unsupported("missing finalizer frame");
                    return;
                };
                let context = Self::context(anchor.definition, anchor.body, anchor.site, sources);
                let result = self.tracker.move_tree(&context, &from, &to);
                self.checked(result);
            } else if frame.slots.iter().flatten().any(|v| resources_in(v) > 0) {
                self.unsupported("externally supplied resource root");
                return;
            }
        }
    }
    fn before_step(
        &mut self,
        _task: u64,
        frame: &Frame,
        instruction: Option<&InstructionKind>,
        terminator: &Terminator,
        site: Span,
        sources: &SourceMap,
    ) {
        self.pending = None;
        if self.error.is_some() {
            return;
        }
        let context = Self::at(frame, site, sources);
        match instruction {
            Some(InstructionKind::Assign { destination, value }) => {
                match frame.body().rvalues.get(value.0 as usize) {
                    Some(Rvalue::Construct { fields, .. }) => {
                        let mut incoming = vec![];
                        for field in fields {
                            let Ok(value) = frame.observe(field.value.place) else {
                                self.unsupported("construction field is unavailable");
                                return;
                            };
                            let count = resources_in(value);
                            if count == 0 {
                                continue;
                            }
                            if field.value.usage != UseKind::Take || !Self::supported_tree(value) {
                                self.unsupported("unsupported construction field transport");
                                return;
                            }
                            let Some(owner) = self.place(frame, field.value.place) else {
                                self.unsupported("construction field owner unavailable");
                                return;
                            };
                            incoming.push((owner, field.field, count));
                        }
                        self.pending = Some(Pending::Create {
                            destination: *destination,
                            fields: incoming,
                            context,
                        })
                    }
                    Some(Rvalue::Closure { captures, .. }) => {
                        let mut incoming = vec![];
                        for (index, operand) in captures.iter().enumerate() {
                            let Ok(value) = frame.observe(operand.place) else {
                                self.unsupported("closure capture is unavailable");
                                return;
                            };
                            let count = resources_in(value);
                            if count == 0 {
                                continue;
                            }
                            if operand.usage != UseKind::Take || !Self::supported_tree(value) {
                                self.unsupported("borrowed or copied resource capture");
                                return;
                            }
                            let Some(owner) = self.place(frame, operand.place) else {
                                self.unsupported("closure capture owner unavailable");
                                return;
                            };
                            incoming.push((owner, index as u32, count));
                        }
                        self.pending = Some(Pending::Create {
                            destination: *destination,
                            fields: incoming,
                            context,
                        });
                    }
                    Some(Rvalue::Use(operand)) => {
                        let Ok(value) = frame.observe(operand.place) else {
                            self.unsupported("unavailable resource operand");
                            return;
                        };
                        if resources_in(value) == 0 {
                            return;
                        }
                        if !Self::supported_tree(value) {
                            self.unsupported("aggregate resource transport");
                            return;
                        }
                        let Some(from) = self.place(frame, operand.place) else {
                            self.unsupported("unavailable resource projection");
                            return;
                        };
                        let Some(to) = self.destination(frame, *destination) else {
                            self.unsupported("unavailable resource destination");
                            return;
                        };
                        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
                            let destination = frame.place(*destination).unwrap();
                            if !destination.projection.is_empty() {
                                self.unsupported("projected resource alias");
                                return;
                            }
                            self.pending = Some(Pending::Alias {
                                local: destination.root.0,
                                from,
                            });
                        } else if operand.usage == UseKind::Take {
                            self.pending = Some(Pending::Move { from, to, context });
                        } else {
                            self.unsupported("resource copy");
                        }
                    }
                    _ => (),
                }
            }
            Some(InstructionKind::Defuse { place }) => {
                let Some(owner) = self.place(frame, *place) else {
                    self.unsupported("unavailable defuse owner");
                    return;
                };
                self.pending = Some(Pending::Defuse { owner, context });
            }
            Some(InstructionKind::Replace { value, .. }) => {
                if frame
                    .observe(value.place)
                    .is_ok_and(|v| resources_in(v) > 0)
                {
                    self.unsupported("resource replacement transport");
                }
            }
            Some(InstructionKind::StorageDead { place, .. }) => {
                if let Ok(place) = frame.place(*place) {
                    self.frames
                        .last_mut()
                        .unwrap()
                        .aliases
                        .remove(&place.root.0);
                }
            }
            Some(InstructionKind::EndScope(scope)) => {
                let body = frame.body();
                self.frames.last_mut().unwrap().aliases.retain(|local, _| {
                    let mut current = Some(body.locals[*local as usize].scope);
                    while let Some(id) = current {
                        if id == *scope {
                            return false;
                        }
                        current = body.scopes[id.0 as usize].parent;
                    }
                    true
                });
            }
            None => {
                if let Terminator::Call {
                    callee, arguments, ..
                } = terminator
                {
                    if matches!(callee,hirsel_tir::Callee::Definition {path,..} if self.forget.as_ref()==Some(path))
                    {
                        self.forget_source = arguments
                            .first()
                            .and_then(|a| self.place(frame, a.value.place));
                    } else {
                        let (definition, body, captures) = match callee {
                            hirsel_tir::Callee::Definition { path, .. } => {
                                (path.clone(), None, vec![])
                            }
                            hirsel_tir::Callee::Indirect { value: operand, .. } => {
                                let Ok(Value::Closure {
                                    definition,
                                    body,
                                    captures,
                                    environment,
                                    invoked,
                                }) = frame.observe(operand.place)
                                else {
                                    self.unsupported("unsupported indirect callable");
                                    return;
                                };
                                if captures.len() != environment.owned_captures.len()
                                    || invoked.as_ref().is_some_and(|state| state.get().is_some())
                                {
                                    self.unsupported(
                                        "closure environment is inconsistent or already invoked",
                                    );
                                    return;
                                }
                                let mut incoming = vec![];
                                for (index, (value, owned)) in
                                    captures.iter().zip(&environment.owned_captures).enumerate()
                                {
                                    let Some(value) = value else {
                                        continue;
                                    };
                                    if resources_in(value) == 0 {
                                        continue;
                                    }
                                    if !*owned
                                        || invoked.is_none()
                                        || operand.usage != UseKind::Take
                                    {
                                        self.unsupported(
                                            "borrowed or reusable resource closure invocation",
                                        );
                                        return;
                                    }
                                    let Some(mut from) = self.place(frame, operand.place) else {
                                        self.unsupported("invoked closure owner unavailable");
                                        return;
                                    };
                                    from.path.push(Step::Field {
                                        field: index as u32,
                                    });
                                    incoming.push((index as u32, from));
                                }
                                (definition.path().clone(), Some(*body), incoming)
                            }
                        };
                        let mut incoming = vec![];
                        for argument in arguments {
                            let Ok(value) = frame.observe(argument.value.place) else {
                                self.unsupported("call argument is unavailable");
                                return;
                            };
                            if resources_in(value) == 0 {
                                continue;
                            }
                            if !Self::supported_tree(value) {
                                self.unsupported("aggregate call argument");
                                return;
                            }
                            let Some(owner) = self.place(frame, argument.value.place) else {
                                self.unsupported("call argument owner unavailable");
                                return;
                            };
                            if !matches!(
                                argument.value.usage,
                                UseKind::Take | UseKind::Borrow | UseKind::Mut
                            ) {
                                self.unsupported("copied resource call argument");
                                return;
                            }
                            incoming.push((argument.parameter.0, argument.value.usage, owner));
                        }
                        let Terminator::Call { returns, .. } = terminator else {
                            unreachable!()
                        };
                        let destination = match returns {
                            Some(returns) => {
                                let Some(owner) = self.destination(frame, returns.destination)
                                else {
                                    self.unsupported("call return owner unavailable");
                                    return;
                                };
                                Some(owner)
                            }
                            None => None,
                        };
                        self.call = Some(CallState {
                            definition,
                            body,
                            captures,
                            arguments: incoming,
                            destination,
                            context,
                        });
                    }
                } else if let Terminator::Return { value, .. } = terminator
                    && frame
                        .observe(value.place)
                        .is_ok_and(|v| resources_in(v) > 0)
                {
                    let Some(owner) = self.place(frame, value.place) else {
                        self.unsupported("return owner unavailable");
                        return;
                    };
                    if value.usage != UseKind::Take {
                        self.unsupported("borrowed resource return");
                        return;
                    }
                    self.frames.last_mut().unwrap().returned = Some((owner, context));
                }
            }
            _ => (),
        }
    }
    fn call_entered(&mut self, _task: u64, _caller: &Frame, callee: &Frame) {
        if self.error.is_some() {
            return;
        }
        let Some(call) = self.call.take() else {
            self.unsupported("user call entered without its source call");
            return;
        };
        if callee.definition.path.path() != &call.definition
            || call.body.is_some_and(|body| callee.body_id != body)
            || callee.bounded.is_some()
        {
            self.unsupported("call target or bounded transport changed");
            return;
        }
        let id = self.next_frame;
        self.next_frame += 1;
        let mut state = FrameState {
            id,
            finalizer: false,
            aliases: BTreeMap::new(),
            return_to: call.destination,
            returned: None,
        };
        for (parameter, usage, from) in call.arguments {
            let Some(local) = callee.body().parameters.get(parameter as usize) else {
                self.unsupported("callee parameter is unavailable");
                return;
            };
            let to = Self::frame_slot(id, local.0);
            if usage == UseKind::Take {
                let result = self.tracker.move_tree(&call.context, &from, &to);
                self.checked(result);
            } else {
                state.aliases.insert(local.0, from);
            }
        }
        for (capture, from) in call.captures {
            let Some((local, _)) = callee
                .body()
                .locals
                .iter()
                .enumerate()
                .filter(|(_, local)| local.storage == hirsel_tir::Storage::Capture)
                .nth(capture as usize)
            else {
                self.unsupported("callee capture slot is unavailable");
                return;
            };
            let to = Self::frame_slot(id, local as u32);
            let result = self.tracker.move_tree(&call.context, &from, &to);
            self.checked(result);
        }
        self.frames.push(state);
    }
    fn builtin_entered(&mut self, _task: u64, _frame: &Frame) {
        if self.error.is_some() {
            return;
        }
        let Some(call) = self.call.take() else {
            self.unsupported("builtin entered without its actual source call");
            return;
        };
        if !call.arguments.is_empty() {
            self.unsupported("builtin resource transport");
        }
    }
    fn builtin_returned(&mut self, _task: u64, _frame: &Frame, value: &Value) {
        if resources_in(value) != 0 {
            self.unsupported("builtin returned resources without an observed transport");
        }
    }
    fn after_step(&mut self, _task: u64, frame: &Frame, success: bool) {
        if self.error.is_some() {
            return;
        }
        let pending = self.pending.take();
        if !success && pending.is_some() {
            self.unsupported("failed resource transition");
            return;
        }
        match pending {
            Some(Pending::Create {
                destination,
                fields,
                context,
            }) => {
                let Ok(value) = frame.observe(destination) else {
                    self.unsupported("construction has no committed result");
                    return;
                };
                let count = resources_in(value);
                if count == 0 {
                    if !fields.is_empty() {
                        self.unsupported("construction discarded tracked resource fields");
                    }
                    return;
                }
                if !Self::supported_tree(value)
                    || count
                        != fields.iter().map(|f| f.2).sum::<u64>()
                            + u64::from(Self::resource(value))
                {
                    self.unsupported("unattributed resource construction");
                    return;
                }
                if let Ok(place) = frame.place(destination)
                    && place.projection.is_empty()
                {
                    self.frames
                        .last_mut()
                        .unwrap()
                        .aliases
                        .remove(&place.root.0);
                }
                let Some(owner) = self.destination(frame, destination) else {
                    self.unsupported("construction owner unavailable");
                    return;
                };
                for (from, field, expected) in fields {
                    let Some(step) = Self::field(value, field) else {
                        self.unsupported("unsupported construction field projection");
                        return;
                    };
                    let mut to = owner.clone();
                    to.path.push(step);
                    let result = self.tracker.move_tree(&context, &from, &to);
                    if let Some(tokens) = self.checked(result)
                        && tokens.len() as u64 != expected
                    {
                        self.unsupported("construction field resource inventory differs");
                        return;
                    }
                }
                if let Some(name) = Self::type_name(value) {
                    let result = self.tracker.create(&context, owner, name);
                    self.checked(result);
                }
            }
            Some(Pending::Move { from, to, context }) => {
                if let Storage::Slot { frame, slot } = to.storage
                    && to.path.is_empty()
                    && self.frames.last().is_some_and(|state| state.id == frame)
                {
                    self.frames.last_mut().unwrap().aliases.remove(&slot);
                }
                let result = self.tracker.move_tree(&context, &from, &to);
                self.checked(result);
            }
            Some(Pending::Alias { local, from }) => {
                self.frames.last_mut().unwrap().aliases.insert(local, from);
            }
            Some(Pending::Defuse { owner, context }) => {
                let result = self.tracker.drop_value(&context, &owner);
                self.checked(result);
            }
            None => (),
        }
    }
    fn drop_place(
        &mut self,
        _task: u64,
        frame: &Frame,
        place: PlaceId,
        value: &Value,
        site: Span,
        sources: &SourceMap,
    ) {
        if self.error.is_some() || resources_in(value) == 0 {
            return;
        }
        if !Self::supported_tree(value) {
            self.unsupported("aggregate resource cleanup");
            return;
        }
        let Some(from) = self.place(frame, place) else {
            self.unsupported("cleanup owner unavailable");
            return;
        };
        let context = Self::at(frame, site, sources);
        let owner = self.scratch();
        let result = self.tracker.move_tree(&context, &from, &owner);
        if self.checked(result).is_some() {
            self.releases.push_back(owner);
        }
    }
    fn finish_frame(
        &mut self,
        _task: u64,
        frame: &Frame,
        ending: &crate::resources::Ending,
        sources: &SourceMap,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(state) = self.frames.last() else {
            self.unsupported("frame retirement without an entry");
            return;
        };
        if state.finalizer {
            let Some((local, anchor, _)) = frame.finalized.as_deref() else {
                self.unsupported("finalizer attribution changed");
                return;
            };
            let Some(owner) = self.slot(local.0) else {
                self.unsupported("finalizer owner unavailable");
                return;
            };
            let context = Self::context(anchor.definition, anchor.body, anchor.site, sources);
            let result = self.tracker.drop_value(&context, &owner);
            self.checked(result);
            if let Some(value) = frame.slots.get(local.0 as usize).and_then(Option::as_ref) {
                self.release_fields(value, &owner, &context);
            } else {
                self.unsupported("finalizer self disappeared before field cleanup");
            }
        } else if let crate::resources::Ending::Return(value) = ending {
            let returned = state.returned.clone();
            let destination = state.return_to.clone();
            if let Some(Owner {
                storage: Storage::Slot { frame, slot },
                path,
                ..
            }) = &destination
                && path.is_empty()
                && let Some(caller) = self.frames.iter_mut().rev().nth(1)
                && caller.id == *frame
            {
                caller.aliases.remove(slot);
            }
            if resources_in(value) == 0 {
                self.frames.pop();
                return;
            }
            if !Self::supported_tree(value) {
                self.unsupported("aggregate resource return");
                return;
            }
            let Some((from, context)) = returned else {
                self.unsupported("resource return has no observed source");
                return;
            };
            let Some(to) = destination else {
                self.unsupported("externally returned resource transport");
                return;
            };
            let result = self.tracker.move_tree(&context, &from, &to);
            self.checked(result);
        }
        self.frames.pop();
    }
    fn released_without_finalizer(
        &mut self,
        _task: u64,
        frame: &Frame,
        value: &Value,
        site: Span,
        sources: &SourceMap,
    ) {
        if self.error.is_some() || resources_in(value) == 0 {
            return;
        }
        if !Self::supported_tree(value) {
            self.unsupported("compound resource destruction without a finalizer");
            return;
        }
        let Some(owner) = self.releases.pop_front() else {
            self.unsupported("resource destruction has no observed incoming owner");
            return;
        };
        let context = Self::at(frame, site, sources);
        if Self::resource(value) {
            let result = self.tracker.drop_value(&context, &owner);
            self.checked(result);
        }
        self.release_fields(value, &owner, &context);
    }
    fn forgotten(&mut self, _task: u64, frame: &Frame, site: Span, sources: &SourceMap) {
        if self.error.is_some() {
            return;
        }
        let Some(from) = self.forget_source.take() else {
            self.unsupported("forget has no observed incoming owner");
            return;
        };
        let Some(state) = self.frames.last() else {
            self.unsupported("forget frame unavailable");
            return;
        };
        let owner = Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Forgotten { frame: state.id },
            path: vec![Step::Index {
                index: self.next_scratch,
            }],
        };
        self.next_scratch += 1;
        let context = Self::at(frame, site, sources);
        let result = self.tracker.move_tree(&context, &from, &owner);
        self.checked(result);
    }
}
