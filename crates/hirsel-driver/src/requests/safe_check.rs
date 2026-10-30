//! Conditional safety observation is an admitted continuation. A rejected or
//! incomplete check publishes its diagnostics without demanding a Run closure.
use hirsel_engine::store::{Action, Address, Domain, Reference};
use hirsel_engine::{
    CheckOutput, FrontCheckQueries, FrontRunQueries, ToolError, runtime::*, store::ResultStore,
};
use hirsel_syntax::{ContentHash, DefPath};
use std::{
    sync::{Arc, Mutex},
    task::Poll,
};

pub(super) const KIND: u32 = 0x5200_0010;
pub(super) const BYTES: u64 = 4096;
const OUTPUT: Domain = Domain {
    kind: 0x5200_0011,
    schema: 1,
};
#[derive(Clone)]
struct Settled {
    product: ProductRef,
    check: ProductRef,
    run: Option<ProductRef>,
}
pub(super) struct SafeCheck {
    id: QueryId,
    check: Arc<FrontCheckQueries>,
    run: Arc<FrontRunQueries>,
    launch: Option<QueryId>,
    store: Arc<dyn ResultStore>,
    errors: RuntimeErrors,
    completed: Mutex<Option<Settled>>,
    _allowance: MemoryReservation,
}
impl SafeCheck {
    pub fn new(
        check: Arc<FrontCheckQueries>,
        run: Arc<FrontRunQueries>,
        entry: Option<DefPath>,
        store: Arc<dyn ResultStore>,
        errors: RuntimeErrors,
        allowance: MemoryReservation,
    ) -> Result<Arc<Self>, ToolError> {
        if allowance.bytes() < BYTES {
            return Err(ToolError::Runtime(
                "safe-check inventory exceeds admission".into(),
            ));
        }
        let launch = entry
            .as_ref()
            .map(|entry| {
                run.query(entry).ok_or_else(|| {
                    ToolError::Runtime("safe-check entry has no admitted Run root".into())
                })
            })
            .transpose()?;
        let id = QueryId {
            kind: KIND,
            address: Address(ContentHash::of_parts(
                "hirsel.safe-check.address.v1",
                [
                    check.query().address.0.0.as_slice(),
                    launch
                        .as_ref()
                        .map_or(&[][..], |id| id.address.0.0.as_slice()),
                ],
            )),
        };
        Ok(Arc::new(Self {
            id,
            check,
            run,
            launch,
            store,
            errors,
            completed: Mutex::new(None),
            _allowance: allowance,
        }))
    }
    pub fn query(&self) -> QueryId {
        self.id
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    /// The settled check and, when it was accepted, the entry's Run product.
    pub fn settled(
        &self,
        product: ProductRef,
    ) -> Result<(CheckOutput, Option<ProductRef>), ToolError> {
        let selected = self
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|selected| selected.product == product)
            .ok_or_else(|| {
                ToolError::Runtime("selected safe-check product is unavailable".into())
            })?;
        let checked = self
            .check
            .outcome(selected.check)
            .ok_or_else(|| ToolError::Runtime("safe-check source product is unavailable".into()))?
            .checked;
        Ok((checked, selected.run))
    }
    pub fn outcome(
        &self,
        product: ProductRef,
    ) -> Result<
        (
            CheckOutput,
            Option<(DefPath, Option<hirsel_engine::UnsafeReach>)>,
        ),
        ToolError,
    > {
        let selected = self
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|selected| selected.product == product)
            .ok_or_else(|| {
                ToolError::Runtime("selected safe-check product is unavailable".into())
            })?;
        let checked = self
            .check
            .outcome(selected.check)
            .ok_or_else(|| ToolError::Runtime("safe-check source product is unavailable".into()))?
            .checked;
        let reach = match selected.run {
            Some(product) => {
                let facts = self.run.outcome(product).ok_or_else(|| {
                    ToolError::Runtime("safe-check Run product is unavailable".into())
                })?;
                Some((
                    facts.entry.clone(),
                    facts.semantic.unsafe_reach(&facts.entry),
                ))
            }
            None => None,
        };
        Ok((checked, reach))
    }
}
struct Factory(Arc<SafeCheck>);
impl QueryFactory for Factory {
    fn create(&self, id: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if id != self.0.id {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task {
            owner: self.0.clone(),
            check: None,
        }))
    }
}
struct Task {
    owner: Arc<SafeCheck>,
    check: Option<ProductRef>,
}
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.owner.id
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: BYTES,
            scratch: BYTES,
            stack: 0,
        }
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let owner = &self.owner;
        if let Some(settled) = owner
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            return QueryPoll::Complete(settled.product.clone());
        }
        let check = match self.check.clone() {
            Some(product) => product,
            None => match cx.read(owner.check.query()) {
                Poll::Pending => return QueryPoll::Waiting(vec![owner.check.query()]),
                Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                Poll::Ready(Ok(product)) => {
                    self.check = Some(product.clone());
                    product
                }
            },
        };
        let Some(accepted) = owner.check.accepted(check.clone()) else {
            return QueryPoll::Failed(owner.errors.protocol);
        };
        let launch = owner.launch.filter(|_| accepted);
        let run = match launch {
            Some(query) => match cx.read(query) {
                Poll::Pending => return QueryPoll::Waiting(vec![query]),
                Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
                Poll::Ready(Ok(product)) => Some(product),
            },
            None => None,
        };
        let Ok(check_archive) = check.archive(ArchiveAdmission::Retained) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let mut bytes = check_archive.reference().output.0.0.to_vec();
        bytes.push(u8::from(run.is_some()));
        if let Some(run) = &run {
            let Ok(archive) = run.archive(ArchiveAdmission::Retained) else {
                return QueryPoll::Failed(owner.errors.resource);
            };
            bytes.extend_from_slice(&archive.reference().output.0.0);
        }
        let Some(record) = owner.store.put_content(OUTPUT, &bytes) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let action = Action(ContentHash::of_parts(
            "hirsel.safe-check.action.v1",
            [
                check.action.0.0.as_slice(),
                run.as_ref()
                    .map_or(&[][..], |run| run.action.0.0.as_slice()),
                record.output.0.0.as_slice(),
            ],
        ));
        let Some(archive) = owner.store.open_product_archive(record) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let product = ProductRef::import_archive(owner.id.address, action, archive);
        *owner.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Settled {
            product: product.clone(),
            check,
            run,
        });
        QueryPoll::Complete(product)
    }
}
