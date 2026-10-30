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
        poll_fn(|_| self.poll_read(query, false)).await
    }
    /// Consume one completed mailbox row. The query runtime keeps its settled
    /// latch and the producer keeps its product and admission owners; a later
    /// read subscribes to that same latch without rerunning the producer.
    /// Use this after each result is taken into independently admitted state,
    /// so sequential dependencies need only their concurrently live rows.
    pub async fn read_consuming(&self, query: QueryId) -> Outcome {
        poll_fn(|_| self.poll_read(query, true)).await
    }
    fn poll_read(&self, query: QueryId, consuming: bool) -> Poll<Outcome> {
        let mut reads = self.0.borrow_mut();
        let selected = if consuming {
            reads.completed.remove(&key(query))
        } else {
            reads.completed.get(&key(query)).cloned()
        };
        if let Some(result) = selected {
            return Poll::Ready(result);
        }
        if !reads.requested.contains_key(&key(query)) {
            if reads.completed.len() + reads.requested.len() >= reads.limit {
                return Poll::Ready(Err(reads.resource));
            }
            reads.requested.insert(key(query), query);
        }
        Poll::Pending
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
    /// Future plus bounded mailbox allocation, excluding producer-owned data.
    /// Each row covers both B-tree nodes and temporary ready-list metadata.
    pub fn retained_bytes_for(future_bytes: usize, reads: usize) -> Option<u64> {
        u64::try_from(reads)
            .ok()?
            .checked_mul(512)?
            .checked_add(u64::try_from(future_bytes).ok()?)?
            .checked_add(std::mem::size_of::<Self>() as u64 + 256)
    }
    /// A namespace limit cannot preallocate beyond the admitted task frame.
    /// Actual reads exceeding either limit return the normal resource refusal.
    pub fn read_capacity(retained: u64, future_bytes: usize, max_reads: usize) -> Option<usize> {
        let available = retained.checked_sub(Self::retained_bytes_for(future_bytes, 0)?)?;
        Some(max_reads.min((available / 512).min(usize::MAX as u64) as usize))
    }
    pub fn new<F: Future<Output = Outcome> + 'static>(
        id: QueryId,
        weight: MemoryWeight,
        max_reads: usize,
        errors: RuntimeErrors,
        create: impl FnOnce(QueryReads) -> F,
    ) -> Result<Self, Reference> {
        let max_reads = Self::read_capacity(weight.retained, std::mem::size_of::<F>(), max_reads)
            .ok_or(errors.resource)?;
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
                    self.terminal = Some(result.clone());
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
        if let Some(result) = self.terminal.clone() {
            return finish(result);
        }
        self.advance(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_artifact_store::{Action, Address, Domain};
    use hirsel_syntax::ContentHash;
    fn query(n: u8) -> QueryId {
        QueryId {
            kind: 1,
            address: Address(ContentHash::of_bytes(&[n])),
        }
    }
    fn product(id: QueryId) -> ProductRef {
        crate::runtime::test_product(
            id.address,
            Action(id.address.0),
            Domain { kind: 1, schema: 1 },
            b"resource",
        )
    }
    fn error() -> Reference {
        Domain { kind: 1, schema: 1 }
            .reference(b"resource")
            .unwrap()
    }
    fn errors() -> RuntimeErrors {
        RuntimeErrors {
            resource: error(),
            cycle: error(),
            cancelled: error(),
            protocol: error(),
            panicked: error(),
        }
    }
    fn reads() -> QueryReads {
        QueryReads(Rc::new(RefCell::new(Reads {
            requested: BTreeMap::new(),
            completed: BTreeMap::new(),
            limit: 0,
            resource: error(),
        })))
    }
    async fn computation(reads: QueryReads, count: u8) -> Outcome {
        for n in 1..=count {
            reads.read(query(n)).await?;
        }
        Ok(product(query(0)))
    }
    struct Completed;
    impl QueryContext for Completed {
        fn read(&mut self, id: QueryId) -> Poll<Outcome> {
            Poll::Ready(Ok(product(id)))
        }
    }
    #[test]
    fn mailbox_capacity_charges_actual_frame_and_refuses_excess_real_reads() {
        let frame = std::mem::size_of_val(&computation(reads(), 0));
        for (slots, namespace, count, accepted) in
            [(2, 4096, 2, true), (2, 4096, 3, false), (5, 2, 3, false)]
        {
            let retained = LocalFutureTask::retained_bytes_for(frame, slots).unwrap();
            let mut task = LocalFutureTask::new(
                query(0),
                MemoryWeight {
                    retained,
                    ..Default::default()
                },
                namespace,
                errors(),
                move |reads| computation(reads, count),
            )
            .unwrap();
            assert_eq!(task.reads.0.borrow().limit, slots.min(namespace));
            loop {
                match task.poll(&mut Completed) {
                    QueryPoll::Waiting(_) => (),
                    QueryPoll::Complete(value) => {
                        assert!(accepted);
                        assert_eq!(value, product(query(0)));
                        break;
                    }
                    QueryPoll::Failed(reference) => {
                        assert!(!accepted);
                        assert_eq!(reference, error());
                        break;
                    }
                }
            }
        }
    }
    #[test]
    fn consuming_reads_reuse_rows_and_deliver_dependency_failures() {
        async fn consume(reads: QueryReads) -> Outcome {
            for n in 1..=64 {
                // Repeated identities go through the same settled latch too.
                for _ in 0..2 {
                    let id = query(n);
                    let result = reads.read_consuming(id).await;
                    if n == 32 {
                        assert_eq!(result, Err(error()));
                    } else {
                        assert_eq!(result, Ok(product(id)));
                    }
                    let state = reads.0.borrow();
                    assert!(state.completed.is_empty());
                    assert!(state.requested.is_empty());
                    assert_eq!(state.limit, 2);
                }
            }
            Ok(product(query(0)))
        }
        struct Settled;
        impl QueryContext for Settled {
            fn read(&mut self, id: QueryId) -> Poll<Outcome> {
                Poll::Ready(if id == query(32) {
                    Err(error())
                } else {
                    Ok(product(id))
                })
            }
        }
        let frame = std::mem::size_of_val(&consume(reads()));
        let retained = LocalFutureTask::retained_bytes_for(frame, 2).unwrap();
        let mut task = LocalFutureTask::new(
            query(0),
            MemoryWeight {
                retained,
                ..Default::default()
            },
            2,
            errors(),
            consume,
        )
        .unwrap();
        assert!(
            matches!(task.poll(&mut Settled), QueryPoll::Complete(value) if value == product(query(0)))
        );
    }
    #[test]
    fn insufficient_future_frame_refuses_before_construction_and_overflow_is_checked() {
        let frame = std::mem::size_of_val(&computation(reads(), 0));
        let required = LocalFutureTask::retained_bytes_for(frame, 0).unwrap();
        let created = std::cell::Cell::new(false);
        assert!(
            LocalFutureTask::new(
                query(0),
                MemoryWeight {
                    retained: required - 1,
                    ..Default::default()
                },
                1,
                errors(),
                |reads| {
                    created.set(true);
                    computation(reads, 0)
                }
            )
            .is_err()
        );
        assert!(!created.get());
        assert_eq!(
            LocalFutureTask::retained_bytes_for(usize::MAX, usize::MAX),
            None
        );
    }
}
