use super::*;

/// Supplied by the executing engine at a committed transition. No clock read or
/// scheduler decision is needed to construct this context.
#[derive(Clone, Debug)]
pub struct Context {
    pub task: TaskPath,
    pub operation: u64,
    pub site: Site,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub version: u32,
    pub program: [u8; 32],
    pub events: Vec<Event>,
    pub event_count: u64,
    pub complete: bool,
}
impl Trace {
    pub fn analyze(&self, program: [u8; 32], event_limit: usize) -> Result<Analysis, Diagnostic> {
        let problem = if self.version != VERSION {
            Some(Problem::Version)
        } else if self.program != program {
            Some(Problem::Program)
        } else {
            None
        };
        if let Some(problem) = problem {
            return Err(Diagnostic {
                problem,
                event: None,
                history: vec![],
            });
        }
        let mut inventory = Inventory::new(event_limit);
        for event in &self.events {
            inventory.apply(event)?;
        }
        inventory.finish(self.complete, self.event_count)
    }
}

/// Dynamic shadow storage shared by engine adapters. The caller supplies actual
/// locations; this tracker allocates token identities itself. Payload bits and
/// native addresses cannot select, merge or recover a token.
///
/// A whole-value move transfers the root and every tracked field/index subtree.
/// Drop is deliberately shallow: an engine reports each actual destruction as
/// it commits, rather than guessing that all descendants were finalized.
/// Errors concern the diagnostic stream. Engine adapters retain them separately
/// from the application's result and may never turn them into language effects.
pub struct Tracker {
    inventory: Inventory,
    next: BTreeMap<TaskPath, u64>,
    events: Vec<Event>,
    failure: Option<Diagnostic>,
}
impl Tracker {
    pub fn new(event_limit: usize) -> Self {
        Self {
            inventory: Inventory::new(event_limit),
            next: BTreeMap::new(),
            events: vec![],
            failure: None,
        }
    }

    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    fn ready(&self) -> Result<(), Diagnostic> {
        self.failure
            .as_ref()
            .map_or(Ok(()), |error| Err(error.clone()))
    }

    fn reject(&mut self, problem: Problem, owner: &Owner) -> Diagnostic {
        let token = self
            .events
            .iter()
            .rev()
            .find_map(|event| (event.action.owner() == owner).then(|| event.action.token()));
        let history = token
            .and_then(|token| self.inventory.lineage(token))
            .map_or_else(Vec::new, |l| l.events.clone());
        let error = Diagnostic {
            problem,
            event: None,
            history,
        };
        self.failure = Some(error.clone());
        error
    }

    fn emit(&mut self, context: &Context, action: Action) -> Result<(), Diagnostic> {
        self.ready()?;
        let subordinal = match self.inventory.last.get(&context.task) {
            Some(&(operation, subordinal)) if operation == context.operation => {
                subordinal.checked_add(1)
            }
            _ => Some(0),
        };
        let Some(subordinal) = subordinal else {
            return Err(self.reject(Problem::EventBudget, action.owner()));
        };
        let event = Event {
            task: context.task.clone(),
            operation: context.operation,
            subordinal,
            site: context.site.clone(),
            action,
        };
        if let Err(error) = self.inventory.apply(&event) {
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.events.push(event);
        Ok(())
    }

    pub fn create(
        &mut self,
        context: &Context,
        owner: Owner,
        value_type: String,
    ) -> Result<Token, Diagnostic> {
        self.ready()?;
        let sequence = self.next.get(&context.task).copied().unwrap_or(0);
        let Some(next) = sequence.checked_add(1) else {
            return Err(self.reject(Problem::EventBudget, &owner));
        };
        let token = Token {
            task: context.task.clone(),
            sequence,
        };
        self.emit(
            context,
            Action::Create {
                token: token.clone(),
                owner,
                value_type,
            },
        )?;
        self.next.insert(context.task.clone(), next);
        Ok(token)
    }

    fn subtree(&mut self, owner: &Owner) -> Result<Vec<(Owner, Token, String)>, Diagnostic> {
        self.ready()?;
        let values: Vec<_> = self
            .inventory
            .owners
            .iter()
            .filter(|(at, _)| {
                at.task == owner.task
                    && at.storage == owner.storage
                    && at.path.starts_with(&owner.path)
            })
            .map(|(at, token)| {
                (
                    at.clone(),
                    token.clone(),
                    self.inventory.lineage(token).unwrap().value_type.clone(),
                )
            })
            .collect();
        if values.is_empty() {
            return Err(self.reject(Problem::MissingOwner(owner.clone()), owner));
        }
        Ok(values)
    }

    pub fn move_tree(
        &mut self,
        context: &Context,
        from: &Owner,
        owner: &Owner,
    ) -> Result<Vec<Token>, Diagnostic> {
        let values = self.subtree(from)?;
        let mut moved = Vec::with_capacity(values.len());
        for (at, token, _) in values {
            let mut to = owner.clone();
            to.path.extend_from_slice(&at.path[from.path.len()..]);
            self.emit(
                context,
                Action::Move {
                    token: token.clone(),
                    from: at,
                    owner: to,
                },
            )?;
            moved.push(token);
        }
        Ok(moved)
    }

    /// The engine has already validated the ordinary semantic Copy operation.
    /// New value instances get fresh identities; the source retains its owners.
    pub fn copy_tree(
        &mut self,
        context: &Context,
        from: &Owner,
        owner: &Owner,
    ) -> Result<Vec<Token>, Diagnostic> {
        let values = self.subtree(from)?;
        let mut copies = Vec::with_capacity(values.len());
        for (at, _, value_type) in values {
            let mut to = owner.clone();
            to.path.extend_from_slice(&at.path[from.path.len()..]);
            copies.push(self.create(context, to, value_type)?);
        }
        Ok(copies)
    }

    pub fn drop_value(&mut self, context: &Context, owner: &Owner) -> Result<Token, Diagnostic> {
        self.ready()?;
        let Some(token) = self.inventory.owners.get(owner).cloned() else {
            return Err(self.reject(Problem::MissingOwner(owner.clone()), owner));
        };
        self.emit(
            context,
            Action::Drop {
                token: token.clone(),
                owner: owner.clone(),
            },
        )?;
        Ok(token)
    }

    pub fn finish(self, program: [u8; 32], complete: bool) -> Result<Trace, Diagnostic> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        Ok(Trace {
            version: VERSION,
            program,
            event_count: self.inventory.events,
            events: self.events,
            complete,
        })
    }
}
