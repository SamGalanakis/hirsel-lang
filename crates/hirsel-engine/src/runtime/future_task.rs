//! Drive Rust's saved, non-Send future state from the existing query latch
//! graph. As with Tokio LocalSet, polling and destruction stay at home; this
//! bridge adds no executor, worker, semantic cache or external wake queue.
use super::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    future::{Future, poll_fn},
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, Waker},
};

type Key = (u32, hirsel_syntax::ContentHash);
type Outcome = Result<ProductRef, Reference>;
fn key(query: QueryId) -> Key {
    (query.kind, query.address.0)
}
struct Reads {
    requested: BTreeMap<Key, QueryId>,
    completed: BTreeMap<Key, Outcome>,
    limit: usize,
    resource: Reference,
}
/// Task-local dependency mailbox. Clone only shares the one bounded mailbox;
/// reads use QueryContext on the next poll and never block a compiler worker.
#[derive(Clone)]
pub struct QueryReads(Rc<RefCell<Reads>>);
impl QueryReads {
    pub async fn read(&self, query: QueryId) -> Outcome {
        poll_fn(|_| {
            let mut reads = self.0.borrow_mut();
            if let Some(result) = reads.completed.get(&key(query)) {
                return Poll::Ready(*result);
            }
            if !reads.requested.contains_key(&key(query)) {
                if reads.completed.len() + reads.requested.len() >= reads.limit {
                    return Poll::Ready(Err(reads.resource));
                }
                reads.requested.insert(key(query), query);
            }
            Poll::Pending
        })
        .await
    }
}

/// A compiler-owned asynchronous computation whose only suspension points are
/// QueryReads. The factory constructs this on the home worker after admission.
/// Held interpreter values and producer inputs must fit the supplied retained
/// bound in addition to this adapter's checked future/mailbox metadata bound.
pub struct LocalFutureTask {
    id: QueryId,
    weight: MemoryWeight,
    reads: QueryReads,
    future: Pin<Box<dyn Future<Output = Outcome>>>,
    protocol: Reference,
    terminal: Option<Outcome>,
}
impl LocalFutureTask {
    pub fn new<F: Future<Output = Outcome> + 'static>(
        id: QueryId,
        weight: MemoryWeight,
        max_reads: usize,
        errors: RuntimeErrors,
        create: impl FnOnce(QueryReads) -> F,
    ) -> Result<Self, Reference> {
        // Includes both tree nodes, allocator/index overhead, temporary ready
        // lists and the shared mailbox. Payload ownership remains the producer's.
        let bytes = (max_reads as u64)
            .checked_mul(512)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<F>() as u64))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Self>() as u64 + 256))
            .ok_or(errors.resource)?;
        if bytes > weight.retained {
            return Err(errors.resource);
        }
        let reads = QueryReads(Rc::new(RefCell::new(Reads {
            requested: BTreeMap::new(),
            completed: BTreeMap::new(),
            limit: max_reads,
            resource: errors.resource,
        })));
        let future = Box::pin(create(reads.clone()));
        Ok(Self {
            id,
            weight,
            reads,
            future,
            protocol: errors.protocol,
            terminal: None,
        })
    }
    fn advance(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        loop {
            let requested = self
                .reads
                .0
                .borrow()
                .requested
                .values()
                .copied()
                .collect::<Vec<_>>();
            let mut waiting = Vec::new();
            for query in requested {
                match cx.read(query) {
                    Poll::Pending => waiting.push(query),
                    Poll::Ready(result) => {
                        let mut reads = self.reads.0.borrow_mut();
                        reads.requested.remove(&key(query));
                        reads.completed.insert(key(query), result);
                    }
                }
            }
            if !waiting.is_empty() {
                return QueryPoll::Waiting(waiting);
            }
            match self
                .future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                Poll::Ready(result) => {
                    self.terminal = Some(result);
                    return finish(result);
                }
                Poll::Pending if self.reads.0.borrow().requested.is_empty() => {
                    // External async operations cannot wake the query graph.
                    // Reject rather than leaving an unregistered sleeping task.
                    self.terminal = Some(Err(self.protocol));
                    return QueryPoll::Failed(self.protocol);
                }
                Poll::Pending => (),
            }
        }
    }
}
fn finish(result: Outcome) -> QueryPoll {
    match result {
        Ok(product) => QueryPoll::Complete(product),
        Err(error) => QueryPoll::Failed(error),
    }
}
impl LocalQueryTask for LocalFutureTask {
    fn id(&self) -> QueryId {
        self.id
    }
    fn weight(&self) -> MemoryWeight {
        self.weight
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        if let Some(result) = self.terminal {
            return finish(result);
        }
        self.advance(cx)
    }
}
