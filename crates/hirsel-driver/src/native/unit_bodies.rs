//! Go gc/compile.go:180-189 drops emitted IR but retains inlining products.
//! Each object's complete copy closure is one consumer of these immutable bodies.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct Body<V, R, E> {
    value: Option<V>,
    ready: Option<Result<Arc<R>, E>>,
}
pub(super) struct Bodies<V, R, E> {
    bodies: Vec<Mutex<Body<V, R, E>>>,
    users: Vec<AtomicUsize>,
    bytes: AtomicU64,
    peak: AtomicU64,
    weight: fn(&R) -> u64,
}
impl<V, R, E: Clone> Bodies<V, R, E> {
    pub(super) fn new(values: Vec<V>, copies: &[Vec<usize>], weight: fn(&R) -> u64) -> Self {
        let mut users = vec![1; values.len()];
        for copied in copies {
            for &i in copied {
                users[i] += 1;
            }
        }
        Self {
            bodies: values
                .into_iter()
                .map(|value| {
                    Mutex::new(Body {
                        value: Some(value),
                        ready: None,
                    })
                })
                .collect(),
            users: users.into_iter().map(AtomicUsize::new).collect(),
            bytes: AtomicU64::new(0),
            peak: AtomicU64::new(0),
            weight,
        }
    }
    pub(super) fn ready(
        &self,
        i: usize,
        lower: impl FnOnce(V) -> Result<R, E>,
    ) -> Result<Arc<R>, E> {
        let mut body = self.bodies[i].lock().unwrap_or_else(|e| e.into_inner());
        if body.ready.is_none() {
            let value = body
                .value
                .take()
                .expect("an unfinished consumer retains its body");
            body.ready = Some(lower(value).map(Arc::new));
            if let Some(Ok(ready)) = &body.ready {
                let bytes = (self.weight)(ready);
                let active = self.bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                self.peak.fetch_max(active, Ordering::Relaxed);
            }
        }
        body.ready.as_ref().unwrap().clone()
    }
    /// Called after publishing this object's fragment, even on a full cache hit
    /// or an error. Active emitters retain their own Arcs independently.
    pub(super) fn finished(&self, i: usize, copies: &[usize]) {
        for j in std::iter::once(i).chain(copies.iter().copied()) {
            if self.users[j].fetch_sub(1, Ordering::AcqRel) == 1 {
                let mut body = self.bodies[j].lock().unwrap_or_else(|e| e.into_inner());
                if let Some(Ok(ready)) = body.ready.take() {
                    self.bytes
                        .fetch_sub((self.weight)(&ready), Ordering::Relaxed);
                }
                body.value.take();
            }
        }
    }
    pub(super) fn peak_bytes(&self) -> u64 {
        self.peak.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_ready_bodies_survive_owner_publication_and_release_after_last_caller() {
        let copies = vec![vec![1], vec![], vec![1]];
        let values: Vec<_> = (0..3).map(Arc::new).collect();
        let value_weaks: Vec<_> = values.iter().map(Arc::downgrade).collect();
        let bodies = Bodies::<_, _, ()>::new(values, &copies, |_| 1);
        let ready = bodies.ready(1, Ok).unwrap();
        let weak = Arc::downgrade(&ready);
        drop(ready);
        bodies.finished(1, &copies[1]);
        assert!(
            weak.upgrade().is_some(),
            "owner still has optimizer consumers"
        );
        bodies.finished(0, &copies[0]);
        assert!(
            value_weaks[0].upgrade().is_none(),
            "cache hit releases Value"
        );
        let active = bodies.ready(1, |_| panic!("Ready must be shared")).unwrap();
        bodies.finished(2, &copies[2]);
        assert!(weak.upgrade().is_some(), "active caller owns its Arc");
        drop(active);
        assert!(weak.upgrade().is_none());
        assert!(value_weaks.iter().all(|weak| weak.upgrade().is_none()));
    }

    #[test]
    fn lowering_error_is_shared_without_retaining_value() {
        let value = Arc::new(());
        let weak = Arc::downgrade(&value);
        let bodies = Bodies::<_, (), _>::new(vec![value], &[vec![]], |_| 1);
        assert_eq!(bodies.ready(0, |_| Err("failure")), Err("failure"));
        assert!(weak.upgrade().is_none());
        assert_eq!(bodies.ready(0, |_| panic!("no retry")), Err("failure"));
        bodies.finished(0, &[]);
    }
}
