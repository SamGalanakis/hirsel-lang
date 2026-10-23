//! Scope timers are control interests, not program host operations. They use
//! the admitted live provider and its shared readiness wait; replay arms none.
//! Home serializes arm, poll and disarm. No adapter call holds a table or role.
use super::*;

const TIMER: u64 = 1 << 62;

#[derive(Default)]
pub(super) struct Timers(Mutex<Table>);

#[derive(Default)]
struct Table {
    next: u64,
    rows: BTreeMap<u64, Timer>,
}

struct Timer {
    scope: u32,
    clock: u64,
    at: u64,
    due: bool,
}

impl Service {
    /// Home only, outside the role, while the entering scope retains its owner.
    pub(crate) fn arm(&self, scope: u32, clock: u64, at: u64) -> Stopped<Option<u64>> {
        let token = {
            let mut table = self.timers.0.lock().unwrap();
            assert!(table.next < 1 << 48, "timer counter below run tag bits");
            let token = TIMER | table.next;
            table.next += 1;
            token
        };
        match self.adapter.call_at(
            token,
            Request::SleepUntil {
                clock,
                deadline: at,
            },
        ) {
            Err(AdapterError::Blocked) => {
                self.adapter.parked(token);
                self.timers.0.lock().unwrap().rows.insert(
                    token,
                    Timer {
                        scope,
                        clock,
                        at,
                        due: false,
                    },
                );
                Ok(Some(token))
            }
            result => {
                self.adapter.finish(token);
                match result {
                    Ok(Completion::Wait(_)) => Ok(None),
                    Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
                    Err(error) => Err(Stop::refusal(error)),
                }
            }
        }
    }

    /// Home only, after every child cleanup rest has returned.
    pub(crate) fn disarm(&self, token: u64) {
        let timer = self.timers.0.lock().unwrap().rows.remove(&token);
        if timer.is_some_and(|t| !t.due) {
            self.adapter.finish(token);
        }
    }
}

impl Timers {
    pub(super) fn pending(&self) -> bool {
        !self.0.lock().unwrap().rows.is_empty()
    }

    pub(super) fn scopes(&self, s: &Scheduler) -> Vec<(u64, u32, u32)> {
        self.0
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter_map(|(&token, timer)| {
                let scope = s.scopes.get(timer.scope as usize)?.as_ref()?;
                (scope.deadline.as_ref()?.token == Some(token)).then_some((
                    token,
                    timer.scope,
                    scope.generation,
                ))
            })
            .collect()
    }

    /// Use the same readiness snapshot as parked program operations. A timer
    /// may become ready before scope/Done publishes its token; latch that edge.
    pub(super) fn poll(
        &self,
        adapter: &dyn ConcurrentAdapter,
        ready: &crate::io::Ready,
    ) -> Result<(), AdapterError> {
        let mut timers = self
            .0
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter_map(|(&token, t)| {
                (!t.due && ready.contains(token)).then_some((t.at, token, t.clock))
            })
            .collect::<Vec<_>>();
        timers.sort_unstable();
        for (at, token, clock) in timers {
            match adapter.call_at(
                token,
                Request::SleepUntil {
                    clock,
                    deadline: at,
                },
            ) {
                Err(AdapterError::Blocked) => continue,
                Ok(Completion::Wait(_)) => {}
                Ok(_) => crate::fault("a scope deadline's timer completed as another request"),
                Err(error) => return Err(error),
            }
            adapter.finish(token);
            self.0.lock().unwrap().rows.get_mut(&token).unwrap().due = true;
        }
        Ok(())
    }

    pub(super) fn expire(&self, s: &mut Scheduler, scopes: &[(u64, u32, u32)]) -> bool {
        let mut table = self.0.lock().unwrap();
        let mut due = scopes
            .iter()
            .filter_map(|&(token, scope, generation)| {
                let timer = table.rows.get(&token)?;
                let row = s.scopes.get(scope as usize)?.as_ref()?;
                (timer.due
                    && row.generation == generation
                    && row.deadline.as_ref()?.token == Some(token))
                .then_some((timer.at, token, scope))
            })
            .collect::<Vec<_>>();
        due.sort_unstable();
        let changed = !due.is_empty();
        for (_, token, scope) in due {
            table.rows.remove(&token);
            s.expire(scope);
        }
        changed
    }

    pub(super) fn clear(&mut self, adapter: &dyn ConcurrentAdapter) {
        for (&token, timer) in &self.0.get_mut().unwrap().rows {
            if !timer.due {
                adapter.finish(token);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    struct Clock {
        due: AtomicBool,
        finishes: Mutex<Vec<u64>>,
    }

    impl ConcurrentAdapter for Clock {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            panic!("scope control does not prepare a program host operation");
        }

        fn call(&self, request: Request) -> Result<Completion, AdapterError> {
            assert!(matches!(
                request,
                Request::SleepUntil {
                    clock: 11,
                    deadline: 100
                }
            ));
            if self.due.load(Ordering::Acquire) {
                Ok(Completion::Wait(Ok(())))
            } else {
                Err(AdapterError::Blocked)
            }
        }

        fn finish(&self, token: u64) {
            self.finishes.lock().unwrap().push(token);
        }
    }

    #[test]
    fn readiness_before_scope_publication_is_latched_and_each_timer_finishes_once() {
        let clock = Arc::new(Clock::default());
        let io = Service {
            adapter: clock.clone(),
            live: Default::default(),
            driver: None,
            rows: Default::default(),
            pending: AtomicUsize::new(0),
            opaque: Default::default(),
            timers: Default::default(),
        };
        let mut s = Scheduler::new(Schedule::default(), std::ptr::null_mut());
        s.scopes.push(Some(Scope {
            generation: 2,
            owner: 0,
            children: Vec::new(),
            canceled: Default::default(),
            deadline: Some(ScopeDeadline {
                token: None,
                expired: false,
            }),
            completions: Default::default(),
        }));
        let token = io.arm(0, 11, 100).unwrap().unwrap();
        assert!(io.pending());
        // Home observes readiness while the worker has not published Done.
        let before_done = io.timers.scopes(&s);
        assert!(before_done.is_empty());
        clock.due.store(true, Ordering::Release);
        io.timers
            .poll(&*clock, &crate::io::Ready::Tokens(vec![token]))
            .unwrap();
        assert!(!io.timers.expire(&mut s, &before_done));
        s.scope(0).deadline.as_mut().unwrap().token = Some(token);
        let saved = io.timers.scopes(&s);
        // An edge is consumed once; a later poll still sees the latched due bit.
        io.timers
            .poll(&*clock, &crate::io::Ready::Tokens(Vec::new()))
            .unwrap();
        s.scope(0).generation += 1;
        assert!(!io.timers.expire(&mut s, &saved), "stale scope generation");
        let current = io.timers.scopes(&s);
        assert!(io.timers.expire(&mut s, &current));
        assert!(s.scope(0).deadline.as_ref().unwrap().expired);
        assert_eq!(s.scope(0).deadline.as_ref().unwrap().token, None);
        assert!(!io.pending());
        io.disarm(token);
        clock.due.store(false, Ordering::Release);
        let canceled = io.arm(0, 11, 100).unwrap().unwrap();
        io.disarm(canceled);
        assert!(!io.pending());
        let abandoned = io.arm(0, 11, 100).unwrap().unwrap();
        drop(io);
        assert_eq!(
            *clock.finishes.lock().unwrap(),
            vec![token, canceled, abandoned]
        );
    }
}
