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
}
enum Pending {
    Create {
        destination: PlaceId,
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
        Some(Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Slot {
                frame: self.frames.last()?.id,
                slot: local,
            },
            path: vec![],
        })
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
            } else if frame
                .slots
                .iter()
                .flatten()
                .any(|v| crate::checked::resources_in(v) > 0)
            {
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
                    Some(Rvalue::Construct { .. }) => {
                        self.pending = Some(Pending::Create {
                            destination: *destination,
                            context,
                        })
                    }
                    Some(Rvalue::Use(operand)) => {
                        let Ok(value) = frame.observe(operand.place) else {
                            self.unsupported("unavailable resource operand");
                            return;
                        };
                        if crate::checked::resources_in(value) == 0 {
                            return;
                        }
                        if !Self::resource(value) {
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
                        let source = frame.place(operand.place).unwrap();
                        let alias = self
                            .frames
                            .last()
                            .is_some_and(|f| f.aliases.contains_key(&source.root.0));
                        if matches!(operand.usage, UseKind::Borrow | UseKind::Mut) || alias {
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
                    .is_ok_and(|v| crate::checked::resources_in(v) > 0)
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
                        self.unsupported("ordinary call resource transport");
                    }
                } else if let Terminator::Return { value, .. } = terminator
                    && frame
                        .observe(value.place)
                        .is_ok_and(|v| crate::checked::resources_in(v) > 0)
                {
                    self.unsupported("returned resource transport");
                }
            }
            _ => (),
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
                context,
            }) => {
                let Ok(value) = frame.observe(destination) else {
                    self.unsupported("construction has no committed result");
                    return;
                };
                let count = crate::checked::resources_in(value);
                if count == 0 {
                    return;
                }
                if count != 1 || !Self::resource(value) {
                    self.unsupported("nested resource construction");
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
                let result = self
                    .tracker
                    .create(&context, owner, Self::type_name(value).unwrap());
                self.checked(result);
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
        if self.error.is_some() || crate::checked::resources_in(value) == 0 {
            return;
        }
        if !Self::resource(value) {
            self.unsupported("aggregate resource cleanup");
            return;
        }
        let Some(from) = self.place(frame, place) else {
            self.unsupported("cleanup owner unavailable");
            return;
        };
        let context = Self::at(frame, site, sources);
        let owner = Owner {
            task: Some(TaskPath::default()),
            storage: Storage::Argument {
                call: self.next_scratch,
                parameter: 0,
            },
            path: vec![],
        };
        self.next_scratch += 1;
        let result = self.tracker.move_tree(&context, &from, &owner);
        if self.checked(result).is_some() {
            self.releases.push_back(owner);
        }
    }
    fn finish_frame(&mut self, _task: u64, frame: &Frame, sources: &SourceMap) {
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
        }
        self.frames.pop();
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
