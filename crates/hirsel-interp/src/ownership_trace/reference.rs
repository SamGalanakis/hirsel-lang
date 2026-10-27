use super::closure_support::{children, resources_in};
use super::{Observer, TraceError};
use crate::frame::Frame;
use crate::{OperationResult, Value};
use hirsel_run_schema::ownership_trace::{
    Context, Owner, Projection as Step, Storage, TaskPath, Trace, Tracker,
};
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::{DefId, DefPath};
use hirsel_tir::{InstructionKind, PlaceId, Rvalue, Terminator};
use hirsel_types::UseKind;
use std::collections::{BTreeMap, VecDeque};

mod channels;
use channels::Channels;
mod borrows;
mod callbacks;
use callbacks::Callbacks;
mod mutexes;
use mutexes::Mutexes;

struct FrameState {
    id: u64,
    finalizer: bool,
    aliases: BTreeMap<u32, Owner>,
    return_to: Option<Owner>,
    returned: Option<(Owner, Context)>,
}
struct CallState {
    definition: DefPath,
    arena_scope: bool,
    body: Option<hirsel_tir::BodyId>,
    captures: Vec<(u32, Owner)>,
    borrowed_captures: Vec<(u32, Owner)>,
    capture_views: Vec<(u32, Owner, bool)>,
    views: Vec<(u32, UseKind, Owner)>,
    arguments: Vec<(u32, UseKind, Owner)>,
    destination: Option<Owner>,
    context: Context,
}
enum Pending {
    Create {
        destination: PlaceId,
        fields: Vec<(Owner, u32, u64)>,
        borrowed: Vec<(u32, Owner)>,
        views: Vec<(Owner, u32, bool)>,
        context: Context,
    },
    Move {
        from: Owner,
        to: Owner,
        context: Context,
    },
    Replace {
        from: Owner,
        destination: Owner,
        displaced: Owner,
        incoming: u64,
        outgoing: u64,
        copy: bool,
        context: Context,
    },
    BorrowMove {
        from: Owner,
        to: Owner,
        copy: bool,
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

struct ResultState {
    count: u64,
    context: Context,
}

struct AwaitState {
    child: u64,
    destination: Option<Owner>,
    context: Context,
}

struct SpawnState {
    definition: DefPath,
    body: hirsel_tir::BodyId,
    captures: Vec<(u32, Owner, u64)>,
    context: Context,
}

#[derive(Default)]
struct TaskState {
    frames: Vec<FrameState>,
    releases: VecDeque<Owner>,
    pending: Option<Pending>,
    forget_source: Option<Owner>,
    call: Option<CallState>,
}

pub(super) struct Resources {
    current_task: u64,
    current_path: TaskPath,
    paths: BTreeMap<u64, TaskPath>,
    suspended: BTreeMap<u64, TaskState>,
    task_limit: usize,
    spawns: BTreeMap<u64, SpawnState>,
    results: BTreeMap<u64, ResultState>,
    awaits: BTreeMap<u64, AwaitState>,
    seals: BTreeMap<u64, CallState>,
    tracker: Tracker,
    channels: Channels,
    callbacks: Callbacks,
    mutexes: Mutexes,
    borrows: BTreeMap<Owner, Owner>,
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
            current_task: 0,
            current_path: TaskPath::default(),
            paths: BTreeMap::from([(0, TaskPath::default())]),
            suspended: BTreeMap::new(),
            task_limit: event_limit,
            spawns: BTreeMap::new(),
            results: BTreeMap::new(),
            awaits: BTreeMap::new(),
            seals: BTreeMap::new(),
            tracker: Tracker::new(event_limit),
            channels: Channels::default(),
            callbacks: Callbacks::default(),
            mutexes: Mutexes::default(),
            borrows: BTreeMap::new(),
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
        &self,
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
            task: self.current_path.clone(),
            operation: 0,
            site: hirsel_run_schema::Site {
                definition: format!("{}@{}::{name}", path.0.package.name, path.0.package.version),
                file: source.as_ref().map_or_else(String::new, |s| s.file.clone()),
                line: source.as_ref().map_or(0, |s| u64::from(s.line)),
                column: source.as_ref().map_or(0, |s| u64::from(s.column)),
            },
        }
    }
    fn at(&self, frame: &Frame, span: Span, sources: &SourceMap) -> Context {
        self.context(frame.definition.path, frame.body_id, span, sources)
    }
    fn slot(&self, local: u32) -> Option<Owner> {
        Some(self.frame_slot(self.frames.last()?.id, local))
    }
    fn frame_slot(&self, frame: u64, slot: u32) -> Owner {
        Owner {
            task: Some(self.current_path.clone()),
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
        Some(self.borrow_target(&owner).unwrap_or(owner))
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
        ) || matches!(value, Value::Handle { kind, .. } if kind == "Region")
    }
    fn capture_local(frame: &Frame, capture: u32) -> Option<u32> {
        frame
            .body()
            .locals
            .iter()
            .enumerate()
            .filter(|(_, local)| local.storage == hirsel_tir::Storage::Capture)
            .nth(capture as usize)
            .map(|(local, _)| local as u32)
    }
    fn type_name(value: &Value) -> Option<String> {
        if matches!(value, Value::Handle { kind, .. } if kind == "Region") {
            return Some("core@1::Region".into());
        }
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
            if matches!(value, Value::Handle { kind, .. } if kind == "Region") {
                continue;
            }
            if Self::field(value, 0).is_none() {
                return false;
            }
            children(value, |_, child| work.push(child));
        }
        true
    }
    fn result_owner(task: u64) -> Owner {
        Owner {
            task: None,
            storage: Storage::Object {
                name: "task.result".into(),
                id: task,
                item: 0,
            },
            path: vec![],
        }
    }
    fn scratch(&mut self) -> Owner {
        let owner = Owner {
            task: Some(self.current_path.clone()),
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
        let complete = self.spawns.is_empty()
            && self.seals.is_empty()
            && completed
            && self.frames.is_empty()
            && self.releases.is_empty()
            && self
                .suspended
                .values()
                .all(|state| state.frames.is_empty() && state.releases.is_empty());
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
    fn task_started(&mut self, parent: u64, child: u64, path: Option<Vec<u32>>, frame: &Frame) {
        if self.error.is_some() {
            return;
        }
        let Some(path) = path else {
            self.unsupported("task provider supplies no canonical spawn path");
            return;
        };
        if parent != self.current_task
            || self.paths.contains_key(&child)
            || path.len() != self.current_path.0.len() + 1
            || !path.starts_with(&self.current_path.0)
            || self.paths.values().any(|known| known.0 == path)
            || self.paths.len() > self.task_limit
        {
            self.unsupported("invalid, duplicate or excessive canonical task entry");
            return;
        }
        let path = TaskPath(path);
        let mut state = TaskState::default();
        if let Some(spawn) = self.spawns.remove(&child) {
            if frame.definition.path.path() != &spawn.definition || frame.body_id != spawn.body {
                self.unsupported("spawned callable differs from the committed task argument");
                return;
            }
            let id = self.next_frame;
            self.next_frame += 1;
            let mut context = spawn.context;
            context.task = path.clone();
            let mut expected = 0;
            for (capture, from, count) in spawn.captures {
                let Some((local, _)) = frame
                    .body()
                    .locals
                    .iter()
                    .enumerate()
                    .filter(|(_, local)| local.storage == hirsel_tir::Storage::Capture)
                    .nth(capture as usize)
                else {
                    self.unsupported("spawn capture has no initialized child slot");
                    return;
                };
                if frame
                    .slots
                    .get(local)
                    .and_then(Option::as_ref)
                    .map(resources_in)
                    != Some(count)
                {
                    self.unsupported(
                        "spawn capture inventory differs from its initialized child slot",
                    );
                    return;
                }
                let to = Owner {
                    task: Some(path.clone()),
                    storage: Storage::Slot {
                        frame: id,
                        slot: local as u32,
                    },
                    path: vec![],
                };
                let result = self.tracker.move_tree(&context, &from, &to);
                if let Some(tokens) = self.checked(result)
                    && tokens.len() as u64 != count
                {
                    self.unsupported("spawn capture lineage inventory differs");
                    return;
                }
                expected += count;
            }
            if frame.slots.iter().flatten().map(resources_in).sum::<u64>() != expected {
                self.unsupported("unattributed resource in spawned child slots");
                return;
            }
            state.frames.push(FrameState {
                id,
                finalizer: false,
                aliases: BTreeMap::new(),
                return_to: None,
                returned: None,
            });
        } else if frame
            .slots
            .iter()
            .flatten()
            .any(|value| resources_in(value) != 0)
        {
            self.unsupported("spawned resource capture transport");
            return;
        }
        self.paths.insert(child, path);
        self.suspended.insert(child, state);
    }
    fn sync(&mut self, task: u64, frames: &[Frame], sources: &SourceMap) {
        if self.error.is_some() {
            return;
        }
        if task != self.current_task {
            let Some(incoming) = self.suspended.remove(&task) else {
                self.unsupported("task resumed without an observed canonical entry");
                return;
            };
            let outgoing = TaskState {
                frames: std::mem::replace(&mut self.frames, incoming.frames),
                releases: std::mem::replace(&mut self.releases, incoming.releases),
                pending: std::mem::replace(&mut self.pending, incoming.pending),
                forget_source: std::mem::replace(&mut self.forget_source, incoming.forget_source),
                call: std::mem::replace(&mut self.call, incoming.call),
            };
            self.suspended.insert(self.current_task, outgoing);
            self.current_task = task;
            self.current_path = self.paths[&task].clone();
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
                let context = self.context(anchor.definition, anchor.body, anchor.site, sources);
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
        if let Some(state) = self.frames.last() {
            self.awaits.remove(&state.id);
        }
        if self.error.is_some() {
            return;
        }
        let context = self.at(frame, site, sources);
        match instruction {
            Some(InstructionKind::Assign { destination, value }) => {
                match frame.body().rvalues.get(value.0 as usize) {
                    Some(Rvalue::Construct { fields, .. }) => {
                        let mut incoming = vec![];
                        let mut views = vec![];
                        for field in fields {
                            let Ok(value) = frame.observe(field.value.place) else {
                                self.unsupported("construction field is unavailable");
                                return;
                            };
                            let owner = self.place(frame, field.value.place);
                            if let Some(owner) = &owner
                                && self.has_borrows(owner)
                            {
                                views.push((
                                    owner.clone(),
                                    field.field,
                                    field.value.usage != UseKind::Take,
                                ));
                            }
                            let count = resources_in(value);
                            if count == 0 {
                                continue;
                            }
                            if field.value.usage != UseKind::Take || !Self::supported_tree(value) {
                                self.unsupported("unsupported construction field transport");
                                return;
                            }
                            let Some(owner) = owner else {
                                self.unsupported("construction field owner unavailable");
                                return;
                            };
                            incoming.push((owner, field.field, count));
                        }
                        self.pending = Some(Pending::Create {
                            destination: *destination,
                            fields: incoming,
                            borrowed: vec![],
                            views,
                            context,
                        });
                    }
                    Some(Rvalue::Closure { captures, .. }) => {
                        let mut incoming = vec![];
                        let mut borrowed = vec![];
                        let mut views = vec![];
                        for (index, operand) in captures.iter().enumerate() {
                            let Ok(value) = frame.observe(operand.place) else {
                                self.unsupported("closure capture is unavailable");
                                return;
                            };
                            let owner = self.place(frame, operand.place);
                            if let Some(owner) = &owner
                                && self.has_borrows(owner)
                            {
                                views.push((
                                    owner.clone(),
                                    index as u32,
                                    operand.usage != UseKind::Take,
                                ));
                            }
                            let count = resources_in(value);
                            if count == 0 {
                                continue;
                            }
                            let Some(owner) = owner else {
                                self.unsupported("closure capture owner unavailable");
                                return;
                            };
                            if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) {
                                borrowed.push((index as u32, owner));
                            } else if operand.usage == UseKind::Take && Self::supported_tree(value)
                            {
                                incoming.push((owner, index as u32, count));
                            } else {
                                self.unsupported("copied resource capture");
                                return;
                            }
                        }
                        self.pending = Some(Pending::Create {
                            destination: *destination,
                            fields: incoming,
                            borrowed,
                            views,
                            context,
                        });
                    }
                    Some(Rvalue::Use(operand)) => {
                        let Ok(value) = frame.observe(operand.place) else {
                            self.unsupported("unavailable resource operand");
                            return;
                        };
                        if resources_in(value) == 0 {
                            if let (Some(from), Some(to)) = (
                                self.place(frame, operand.place),
                                self.destination(frame, *destination),
                            ) {
                                self.pending = Some(Pending::BorrowMove {
                                    from,
                                    to,
                                    copy: operand.usage != UseKind::Take,
                                });
                            }
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
            Some(InstructionKind::Replace {
                destination,
                value,
                displaced,
                ..
            }) => {
                let Ok(incoming) = frame.observe(value.place) else {
                    self.unsupported("replacement input unavailable");
                    return;
                };
                let old = frame.observe(*destination).ok();
                if !Self::supported_tree(incoming)
                    || old.is_some_and(|value| !Self::supported_tree(value))
                    || (resources_in(incoming) > 0 && value.usage != UseKind::Take)
                {
                    self.unsupported("unsupported replacement resource tree or copied input");
                    return;
                }
                let (Some(from), Some(destination), Some(displaced)) = (
                    self.place(frame, value.place),
                    self.place(frame, *destination),
                    self.destination(frame, *displaced),
                ) else {
                    self.unsupported("replacement owner unavailable");
                    return;
                };
                self.pending = Some(Pending::Replace {
                    from,
                    destination,
                    displaced,
                    incoming: resources_in(incoming),
                    outgoing: old.map_or(0, resources_in),
                    copy: value.usage != UseKind::Take,
                    context,
                });
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
                        let mut borrowed_captures = vec![];
                        let mut capture_views = vec![];
                        let (mut definition, mut body, captures) = match callee {
                            hirsel_tir::Callee::Definition { path, .. } => {
                                (path.clone(), None, vec![])
                            }
                            hirsel_tir::Callee::Indirect { value: operand, .. } => {
                                if let Ok(Value::FunctionReference(reference)) =
                                    frame.observe(operand.place)
                                {
                                    (reference.definition.clone(), None, vec![])
                                } else {
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
                                        || invoked
                                            .as_ref()
                                            .is_some_and(|state| state.get().is_some())
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
                                        let Some(mut from) = self.place(frame, operand.place)
                                        else {
                                            self.unsupported("invoked closure owner unavailable");
                                            return;
                                        };
                                        from.path.push(Step::Field {
                                            field: index as u32,
                                        });
                                        if resources_in(value) == 0 && self.has_borrows(&from) {
                                            capture_views.push((
                                                index as u32,
                                                from.clone(),
                                                *owned,
                                            ));
                                        }
                                        if resources_in(value) == 0 {
                                            continue;
                                        }
                                        if !owned {
                                            let Some(target) = self.borrow_target(&from) else {
                                                self.unsupported(
                                                    "borrowed capture has no observed owner",
                                                );
                                                return;
                                            };
                                            borrowed_captures.push((index as u32, target));
                                        } else if invoked.is_some()
                                            && operand.usage == UseKind::Take
                                        {
                                            incoming.push((index as u32, from));
                                        } else {
                                            self.unsupported(
                                                "reusable owned resource closure invocation",
                                            );
                                            return;
                                        }
                                    }
                                    (definition.path().clone(), Some(*body), incoming)
                                }
                            }
                        };
                        let mut incoming = vec![];
                        let mut views = vec![];
                        for argument in arguments {
                            let Ok(value) = frame.observe(argument.value.place) else {
                                self.unsupported("call argument is unavailable");
                                return;
                            };
                            if resources_in(value) == 0
                                && let Some(owner) = self.place(frame, argument.value.place)
                                && self.has_borrows(&owner)
                            {
                                views.push((argument.parameter.0, argument.value.usage, owner));
                            }
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
                        let arena_scope = definition.is_core("arena.with");
                        if arena_scope {
                            let callback = arguments
                                .iter()
                                .find(|argument| argument.parameter.0 == 0)
                                .and_then(|argument| frame.observe(argument.value.place).ok());
                            if !incoming.is_empty() || !views.is_empty() {
                                self.unsupported(
                                    "arena callback has untransported resource arguments",
                                );
                                return;
                            }
                            match callback {
                                Some(Value::FunctionReference(callback)) => {
                                    definition = callback.definition.clone();
                                }
                                Some(Value::Closure {
                                    definition: callback,
                                    body: callback_body,
                                    captures,
                                    environment,
                                    invoked,
                                }) if captures.len() == environment.owned_captures.len()
                                    && invoked
                                        .as_ref()
                                        .is_none_or(|state| state.get().is_none())
                                    && captures
                                        .iter()
                                        .flatten()
                                        .all(|value| resources_in(value) == 0) =>
                                {
                                    definition = callback.path().clone();
                                    body = Some(*callback_body);
                                }
                                _ => {
                                    self.unsupported(
                                        "arena callback needs observed capture transport",
                                    );
                                    return;
                                }
                            }
                        }
                        self.call = Some(CallState {
                            definition,
                            arena_scope,
                            body,
                            captures,
                            borrowed_captures,
                            capture_views,
                            views,
                            arguments: incoming,
                            destination,
                            context,
                        });
                    }
                } else if let Terminator::Return { value, .. } = terminator
                    && frame.observe(value.place).is_ok_and(|v| {
                        resources_in(v) > 0
                            || self
                                .place(frame, value.place)
                                .is_some_and(|owner| self.has_borrows(&owner))
                    })
                {
                    let Some(owner) = self.place(frame, value.place) else {
                        self.unsupported("return owner unavailable");
                        return;
                    };
                    if value.usage != UseKind::Take
                        && frame
                            .observe(value.place)
                            .is_ok_and(|v| resources_in(v) > 0)
                    {
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
            || callee.arena_scope.is_some() != call.arena_scope
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
            let to = self.frame_slot(id, local.0);
            if usage == UseKind::Take {
                let result = self.tracker.move_tree(&call.context, &from, &to);
                self.checked(result);
                self.move_borrows(&from, &to, false);
            } else {
                state.aliases.insert(local.0, from);
            }
        }
        for (parameter, usage, from) in call.views {
            let Some(local) = callee.body().parameters.get(parameter as usize) else {
                self.unsupported("borrowed environment parameter unavailable");
                return;
            };
            let to = self.frame_slot(id, local.0);
            if matches!(usage, UseKind::Borrow | UseKind::Mut) {
                state.aliases.insert(local.0, from);
            } else {
                self.move_borrows(&from, &to, usage != UseKind::Take);
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
            let to = self.frame_slot(id, local as u32);
            let result = self.tracker.move_tree(&call.context, &from, &to);
            self.checked(result);
            self.move_borrows(&from, &to, false);
        }
        for (capture, target) in call.borrowed_captures {
            let Some(local) = Self::capture_local(callee, capture) else {
                self.unsupported("borrowed capture local unavailable");
                return;
            };
            state.aliases.insert(local, target);
        }
        for (capture, from, owned) in call.capture_views {
            let Some(local) = Self::capture_local(callee, capture) else {
                self.unsupported("captured environment local unavailable");
                return;
            };
            let to = self.frame_slot(id, local);
            if owned {
                self.move_borrows(&from, &to, false);
            } else {
                state.aliases.insert(local, from);
            }
        }
        self.frames.push(state);
    }
    fn builtin_entered(
        &mut self,
        _task: u64,
        _frame: &Frame,
        arguments: &[Value],
        result: &crate::OperationResult,
        channel: Option<u64>,
    ) {
        if self.error.is_some() {
            return;
        }
        let Some(call) = self.call.take() else {
            self.unsupported("builtin entered without its actual source call");
            return;
        };
        if call.definition.is_core("arena.seal") {
            if !matches!(result, OperationResult::SealArena { .. }) || !call.arguments.is_empty() {
                self.unsupported("seal has no resource-free entered root");
                return;
            }
            let Some(state) = self.frames.last() else {
                self.unsupported("seal has no observed caller");
                return;
            };
            if self.seals.insert(state.id, call).is_some() {
                self.unsupported("seal caller reused before completion");
            }
            return;
        }
        if call.definition.is_core("region.with") {
            let OperationResult::BorrowRegion { owner, result, .. } = result else {
                self.unsupported("region callback has no exclusive root loan");
                return;
            };
            if !matches!(arguments.first(), Some(Value::Handle { kind, id }) if kind == "Region" && id == owner)
            {
                self.unsupported("region callback owner differs from its actual argument");
                return;
            }
            let mut call = call;
            let Some(index) = call
                .arguments
                .iter()
                .position(|(index, usage, _)| *index == 0 && *usage == UseKind::Mut)
            else {
                self.unsupported("region callback lacks its exclusive owner");
                return;
            };
            let (_, _, at) = call.arguments.remove(index);
            if !self.tracker.inventory().contains_owner(&at) {
                self.unsupported("region callback owner has no live lineage");
                return;
            }
            self.callback_called(call, arguments, result);
            return;
        }
        if call.definition.is_core("mutex.create") {
            self.mutex_created(call, arguments, result);
            return;
        }
        if call.definition.is_core("channel.send") || call.definition.is_core("channel.receive") {
            self.channel_entered(call, arguments, result, channel);
            return;
        }
        if call.definition.is_core("task.await") {
            if let Some(Value::Handle { kind, id }) = arguments.get(1)
                && kind == "Task"
            {
                let Some(state) = self.frames.last() else {
                    self.unsupported("await has no observed caller frame");
                    return;
                };
                self.awaits.insert(
                    state.id,
                    AwaitState {
                        child: *id,
                        destination: call.destination.clone(),
                        context: call.context.clone(),
                    },
                );
            }
        }
        if call.definition.is_core("task.scope")
            || call.definition.is_core("mutex.with")
            || call.definition.is_core("shared.with")
        {
            self.callback_called(call, arguments, result);
            return;
        }
        if call.arguments.is_empty() {
            return;
        }
        if !call.definition.is_core("task.async") || !call.captures.is_empty() {
            self.unsupported("builtin resource transport");
            return;
        }
        let crate::OperationResult::Complete {
            value: Value::Handle { kind, id: child },
            consumed,
            arguments: updates,
            released,
        } = result
        else {
            self.unsupported("resource spawn did not commit a task handle");
            return;
        };
        if kind != "Task"
            || consumed.as_slice() != [1]
            || !updates.is_empty()
            || !released.is_empty()
            || self.spawns.contains_key(child)
            || self.paths.contains_key(child)
            || self.spawns.len() >= self.task_limit
            || call.arguments.len() != 1
        {
            self.unsupported("unsupported resource spawn completion");
            return;
        }
        let (parameter, usage, from) = &call.arguments[0];
        let Some(Value::Closure {
            definition,
            body,
            captures,
            environment,
            invoked,
        }) = arguments.get(1)
        else {
            self.unsupported("resource spawn argument is not a closure environment");
            return;
        };
        if *parameter != 1
            || *usage != UseKind::Take
            || captures.len() != environment.owned_captures.len()
            || invoked.as_ref().is_none_or(|state| state.get().is_some())
        {
            self.unsupported("resource spawn requires an uninvoked owned once environment");
            return;
        }
        let custody = Owner {
            task: None,
            storage: Storage::Object {
                name: "task.spawn".into(),
                id: *child,
                item: 0,
            },
            path: vec![],
        };
        let mut incoming = vec![];
        for (index, (value, owned)) in captures.iter().zip(&environment.owned_captures).enumerate()
        {
            let Some(value) = value else {
                continue;
            };
            let count = resources_in(value);
            if count == 0 {
                continue;
            }
            if !owned || !Self::supported_tree(value) {
                self.unsupported("borrowed or unsupported resource spawn capture");
                return;
            }
            let mut owner = custody.clone();
            owner.path.push(Step::Field {
                field: index as u32,
            });
            incoming.push((index as u32, owner, count));
        }
        let result = self.tracker.move_tree(&call.context, from, &custody);
        if let Some(tokens) = self.checked(result)
            && tokens.len() as u64 != incoming.iter().map(|(_, _, count)| count).sum::<u64>()
        {
            self.unsupported("spawn environment lineage inventory differs");
            return;
        }
        self.spawns.insert(
            *child,
            SpawnState {
                definition: definition.path().clone(),
                body: *body,
                captures: incoming,
                context: call.context,
            },
        );
    }
    fn callback_entered(&mut self, _task: u64, _caller: &Frame, callee: &Frame) {
        self.callback_frame(callee);
    }
    fn builtin_completed(&mut self, _task: u64, _frame: &Frame, result: &crate::OperationResult) {
        if matches!(result, OperationResult::Failed(_))
            && let Some(state) = self.frames.last()
        {
            self.seals.remove(&state.id);
        }
        if let Some(state) = self.frames.last() {
            self.channel_committed(state.id, result);
        }
        self.mutex_completed(result);
    }
    fn before_handle_drop(&mut self, _frame: &Frame, kind: &str, id: u64, channel: Option<u64>) {
        if kind == "Mutex"
            && let Some(state) = self.frames.last()
        {
            self.mutexes.drops.insert(state.id, id);
        }
        if kind == "Receiver"
            && let (Some(state), Some(channel)) = (self.frames.last(), channel)
        {
            self.channels.drops.insert(state.id, channel);
        }
    }
    fn handle_released(
        &mut self,
        _task: u64,
        frame: &Frame,
        values: &[Value],
        anchor: &crate::observation::Anchor,
        sources: &SourceMap,
    ) {
        self.channel_released(frame, values, anchor, sources);
        self.mutex_released(values, anchor, sources);
    }
    fn callback_released(
        &mut self,
        _frame: &Frame,
        values: &[Value],
        abandoned: bool,
        anchor: &crate::observation::Anchor,
        sources: &SourceMap,
    ) {
        if abandoned {
            self.mutex_abandoned(values, anchor, sources);
        } else if values.iter().any(|value| resources_in(value) != 0) {
            self.unsupported("unattributed callback writeback cleanup");
        }
    }
    fn builtin_returned(&mut self, _task: u64, _frame: &Frame, value: &Value) {
        if self.error.is_some() {
            return;
        }
        if let Some(call) = self
            .frames
            .last()
            .and_then(|state| self.seals.remove(&state.id))
        {
            if !matches!(value, Value::Handle { kind, .. } if kind == "Region") {
                self.unsupported("seal returned no committed owner");
                return;
            }
            let Some(owner) = call.destination else {
                self.unsupported("seal has no receiving place");
                return;
            };
            let created = self
                .tracker
                .create(&call.context, owner, "core@1::Region".into());
            self.checked(created);
            return;
        }
        let count = resources_in(value);
        if self.channel_returned(value) || self.callback_returned(value) {
            return;
        }
        let awaiting = self
            .frames
            .last()
            .and_then(|state| self.awaits.remove(&state.id));
        if count == 0 {
            return;
        }
        let Some(awaiting) = awaiting else {
            self.unsupported("builtin returned resources without an observed transport");
            return;
        };
        let Some(destination) = awaiting.destination else {
            self.unsupported("resource await has no receiving place");
            return;
        };
        if self
            .results
            .remove(&awaiting.child)
            .map(|result| result.count)
            != Some(count)
            || !Self::supported_tree(value)
        {
            self.unsupported("await result inventory differs from the child return");
            return;
        }
        let from = Self::result_owner(awaiting.child);
        let result = self
            .tracker
            .move_tree(&awaiting.context, &from, &destination);
        if let Some(tokens) = self.checked(result)
            && tokens.len() as u64 != count
        {
            self.unsupported("await result lineage inventory differs");
        }
    }
    fn return_discarded(&mut self, _task: u64, _frame: &Frame, value: &Value) {
        if self.error.is_some() || resources_in(value) == 0 {
            return;
        }
        let Some((from, context)) = self
            .frames
            .last_mut()
            .and_then(|state| state.returned.take())
        else {
            self.unsupported("discarded task result has no observed returned owner");
            return;
        };
        let to = self.scratch();
        let result = self.tracker.move_tree(&context, &from, &to);
        if let Some(tokens) = self.checked(result) {
            if tokens.len() as u64 != resources_in(value) {
                self.unsupported("discarded result lineage inventory differs");
                return;
            }
            self.releases.push_back(to);
        }
    }
    fn rest_entered(&mut self, task: u64, frame: &Frame) {
        if self.error.is_some() {
            return;
        }
        let values: Vec<_> = frame
            .work
            .iter()
            .filter_map(|work| match work {
                crate::resources::Work::Release(value, ..) if resources_in(value) != 0 => {
                    Some(value)
                }
                _ => None,
            })
            .collect();
        if values.is_empty() {
            return;
        }
        let Some(result) = self.results.remove(&task) else {
            self.unsupported("task rest has no observed retained result");
            return;
        };
        if self.current_task != task || values.len() != 1 || resources_in(values[0]) != result.count
        {
            self.unsupported("task rest differs from retained result custody");
            return;
        }
        let from = Self::result_owner(task);
        let to = self.scratch();
        let moved = self.tracker.move_tree(&result.context, &from, &to);
        if let Some(tokens) = self.checked(moved) {
            if tokens.len() as u64 != result.count {
                self.unsupported("task rest lineage inventory differs");
                return;
            }
            self.releases.push_back(to);
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
                borrowed,
                views,
                context,
            }) => {
                let Ok(value) = frame.observe(destination) else {
                    self.unsupported("construction has no committed result");
                    return;
                };
                let count = resources_in(value);
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
                self.forget_borrows(&owner);
                for (from, field, copy) in views {
                    let Some(step) = Self::field(value, field) else {
                        self.unsupported("borrowed environment field projection unavailable");
                        return;
                    };
                    let mut to = owner.clone();
                    to.path.push(step);
                    self.move_borrows(&from, &to, copy);
                }
                for (field, target) in borrowed {
                    let mut at = owner.clone();
                    at.path.push(Step::Field { field });
                    self.insert_borrow(at, target);
                }
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
                self.move_borrows(&from, &to, false);
            }
            Some(Pending::Replace {
                from,
                destination,
                displaced,
                incoming,
                outgoing,
                copy,
                context,
            }) => {
                // The ordinary instruction has committed both transfers. Vacate
                // the old owner before installing the incoming tree at it.
                for (source, target, expected, copy) in [
                    (&destination, &displaced, outgoing, false),
                    (&from, &destination, incoming, copy),
                ] {
                    if expected != 0 {
                        let moved = self.tracker.move_tree(&context, source, target);
                        if let Some(tokens) = self.checked(moved)
                            && tokens.len() as u64 != expected
                        {
                            self.unsupported("replacement lineage inventory differs");
                            return;
                        }
                    }
                    self.move_borrows(source, target, copy);
                }
            }
            Some(Pending::BorrowMove { from, to, copy }) => {
                if let Storage::Slot { frame, slot } = to.storage
                    && to.path.is_empty()
                    && self.frames.last().is_some_and(|state| state.id == frame)
                {
                    self.frames.last_mut().unwrap().aliases.remove(&slot);
                }
                self.move_borrows(&from, &to, copy);
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
        let context = self.at(frame, site, sources);
        let owner = self.scratch();
        let result = self.tracker.move_tree(&context, &from, &owner);
        if self.checked(result).is_some() {
            // Work::Release for this active cleanup is pushed to the front.
            // Pending siblings remain behind a finalizer's own local cleanup.
            self.releases.push_front(owner);
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
        if let Some(id) = self.frames.last().map(|state| state.id)
            && self.seals.remove(&id).is_some()
            && !matches!(ending, crate::resources::Ending::Failed)
        {
            self.unsupported("seal frame returned without observed publication");
            return;
        }
        self.mutex_retiring(frame);
        self.callback_retiring(ending);
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
            let context = self.context(anchor.definition, anchor.body, anchor.site, sources);
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
                if let Some((from, _)) = returned
                    && self.has_borrows(&from)
                {
                    let Some(to) = destination else {
                        self.unsupported("external borrowed environment return");
                        return;
                    };
                    self.move_borrows(&from, &to, true);
                }
                if let Some(state) = self.frames.pop() {
                    self.awaits.remove(&state.id);
                    self.retired_frame(state.id);
                }
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
            let to = match destination {
                Some(to) => to,
                None if self.current_task != 0 && self.frames.len() == 1 => {
                    if self
                        .results
                        .insert(
                            self.current_task,
                            ResultState {
                                count: resources_in(value),
                                context: context.clone(),
                            },
                        )
                        .is_some()
                    {
                        self.unsupported("task result custody reused");
                        return;
                    }
                    Self::result_owner(self.current_task)
                }
                None => {
                    self.unsupported("externally returned resource transport");
                    return;
                }
            };
            let result = self.tracker.move_tree(&context, &from, &to);
            self.checked(result);
            self.move_borrows(&from, &to, false);
        }
        if let Some(state) = self.frames.pop() {
            self.awaits.remove(&state.id);
            self.retired_frame(state.id);
        }
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
        let context = self.at(frame, site, sources);
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
            task: Some(self.current_path.clone()),
            storage: Storage::Forgotten { frame: state.id },
            path: vec![Step::Index {
                index: self.next_scratch,
            }],
        };
        self.next_scratch += 1;
        let context = self.at(frame, site, sources);
        let result = self.tracker.move_tree(&context, &from, &owner);
        self.checked(result);
    }
}
