//! Scope timers are control interests, not program host operations. They use
//! the admitted live provider and its shared readiness wait; replay arms none.
//! Home serializes arm, poll and disarm. No adapter call holds a table or role.
use super::*;
use crate::tasks::registry::ScopeKey;

const TIMER: u64 = 1 << 62;

#[derive(Default)]
pub(super) struct Timers(Mutex<Table>);

#[derive(Default)]
struct Table {
    next: u64,
    #[cfg(test)]
    visits: AtomicUsize,
    rows: BTreeMap<u64, Timer>,
    deadlines: crate::deadlines::Deadlines<(u64, u64)>,
    due: std::collections::BTreeSet<(u64, u64)>,
}

struct Timer {
    owner: ScopeKey,
    clock: u64,
    at: u64,
    due: bool,
}

impl Service {
    /// Home only, outside the role, while the entering scope retains its owner.
    pub(crate) fn arm(&self, owner: ScopeKey, clock: u64, at: u64) -> Stopped<Option<u64>> {
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
                let mut table = self.timers.0.lock().unwrap();
                table.deadlines.insert(token, (clock, at));
                table.rows.insert(
                    token,
                    Timer {
                        owner,
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
        let timer = self.timers.0.lock().unwrap().remove(token);
        if timer.is_some_and(|t| !t.due) {
            self.adapter.finish(token);
        }
    }
}

impl Timers {
    pub(super) fn pending(&self) -> bool {
        !self.0.lock().unwrap().rows.is_empty()
    }

    /// Use the same readiness snapshot as parked program operations. A timer
    /// may become ready before scope/Done publishes its token; latch that edge.
    pub(super) fn poll(
        &self,
        adapter: &dyn ConcurrentAdapter,
        ready: &crate::io::Ready,
    ) -> Result<(), AdapterError> {
        // Readiness already identifies the affected registrations. An opaque
        // provider's Every explicitly affects all of them; token providers do
        // no work proportional to idle timers. Preserve (at, token) ties.
        let timers = {
            let table = self.0.lock().unwrap();
            let mut selected = std::collections::BTreeSet::new();
            let every = if matches!(ready, crate::io::Ready::Every) {
                table.rows.keys().copied().collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let mut select = |token| {
                #[cfg(test)]
                {
                    table.visits.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(t) = table.rows.get(&token)
                    && !t.due
                {
                    selected.insert((t.at, token, t.clock));
                }
            };
            match ready {
                crate::io::Ready::Tokens(tokens) => tokens.iter().copied().for_each(&mut select),
                crate::io::Ready::Every => every.into_iter().for_each(&mut select),
            }
            selected
        };
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
            let mut table = self.0.lock().unwrap();
            table.rows.get_mut(&token).unwrap().due = true;
            table.deadlines.remove(token);
            table.due.insert((at, token));
        }
        Ok(())
    }

    pub(super) fn expire(&self, s: &mut Scheduler) -> bool {
        let mut table = self.0.lock().unwrap();
        let mut changed = false;
        // Only completed timer tokens may inspect scope rows. A ready edge
        // before Done stays latched until that scope publishes its token.
        let due = std::mem::take(&mut table.due);
        for (at, token) in due {
            #[cfg(test)]
            {
                table.visits.fetch_add(1, Ordering::Relaxed);
            }
            let timer = &table.rows[&token];
            let Some(scope) = s.live_scope(timer.owner) else {
                table.remove(token);
                continue;
            };
            let row = s.scopes[scope as usize].as_ref().unwrap();
            if row.deadline.as_ref().and_then(|d| d.token) != Some(token) {
                table.due.insert((at, token));
                continue;
            }
            table.remove(token);
            s.expire(scope);
            changed = true;
        }
        changed
    }

    pub(super) fn clear(&mut self, adapter: &dyn ConcurrentAdapter) {
        let table = self.0.get_mut().unwrap();
        for (&token, timer) in &table.rows {
            if !timer.due {
                adapter.finish(token);
            }
        }
        *table = Table::default();
    }
}

impl Table {
    fn remove(&mut self, token: u64) -> Option<Timer> {
        let timer = self.rows.remove(&token)?;
        self.deadlines.remove(token);
        self.due.remove(&(timer.at, token));
        Some(timer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    pub(super) struct Clock {
        pub(super) due: AtomicBool,
        pub(super) finishes: Mutex<Vec<u64>>,
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
            jobs: Default::default(),
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
        let token = io
            .arm(
                ScopeKey {
                    slot: 0,
                    generation: 2,
                },
                11,
                100,
            )
            .unwrap()
            .unwrap();
        assert!(io.pending());
        // Home observes readiness while the worker has not published Done.

        clock.due.store(true, Ordering::Release);
        io.timers
            .poll(&*clock, &crate::io::Ready::Tokens(vec![token]))
            .unwrap();
        assert!(!io.timers.expire(&mut s));
        s.scope(0).deadline.as_mut().unwrap().token = Some(token);

        // An edge is consumed once; a later poll still sees the latched due bit.
        io.timers
            .poll(&*clock, &crate::io::Ready::Tokens(Vec::new()))
            .unwrap();
        assert!(io.timers.expire(&mut s));
        assert!(s.scope(0).deadline.as_ref().unwrap().expired);
        assert_eq!(s.scope(0).deadline.as_ref().unwrap().token, None);
        assert!(!io.pending());
        io.disarm(token);
        clock.due.store(false, Ordering::Release);
        let canceled = io
            .arm(
                ScopeKey {
                    slot: 0,
                    generation: 2,
                },
                11,
                100,
            )
            .unwrap()
            .unwrap();
        io.disarm(canceled);
        assert!(!io.pending());
        let abandoned = io
            .arm(
                ScopeKey {
                    slot: 0,
                    generation: 2,
                },
                11,
                100,
            )
            .unwrap()
            .unwrap();
        drop(io);
        assert_eq!(
            *clock.finishes.lock().unwrap(),
            vec![token, canceled, abandoned]
        );
    }
}

#[cfg(test)]
#[path = "timers/modules.rs"]
mod runtime50;
