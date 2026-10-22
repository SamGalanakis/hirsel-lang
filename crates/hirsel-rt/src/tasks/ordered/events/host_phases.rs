//! Coupled home requests with fully initialized scalar results. Each bounded
//! attempt is published where it happens; waits retain the original ordinal.
//! Handles and output require their own discovery/effect evidence.
use super::*;
use crate::io::{AdapterError, Request};
use crate::io_keys::Mode;

pub(super) struct Phase {
    entry: Boundary,
    result: Option<host_values::HostValue>,
    parked: bool,
    returned: bool,
    valid: bool,
}

pub(crate) fn request_keys(ctx: *mut TaskCtx) {
    super::request_keys(ctx, &crate::io_keys::KeySet::coupled());
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("home request follows entry");
    assert!(log.home_requests.insert((path, ordinal)));
}

impl Log {
    /// Quiescence already owns Scheduler and saved rows. Native request tokens
    /// contribute no identity; only a valid phase with no result can block.
    pub(super) fn home_wait(&self, path: &[u32]) -> Option<(&'static str, ObjectId)> {
        let (key, phase) = self
            .home_phases
            .iter()
            .find(|((owner, _), _)| owner == path)?;
        if !phase.valid || !phase.parked || phase.result.is_some() || phase.returned {
            return None;
        }
        let request = self.host_requests.get(key)?.as_ref()?;
        let request = crate::tape_wire::decode::<Request>(&request.payload).ok()?;
        Some((
            crate::host_waits::request_kind(&request),
            ObjectId {
                creator: path.to_vec(),
                ordinal: key.1,
            },
        ))
    }

    pub(super) fn start_home(&mut self, path: &[u32], ordinal: u64) {
        let key = (path.to_vec(), ordinal);
        if !self.home_requests.contains(&key) {
            return;
        }
        let Some(entry) = self.host_entries.get(&key).cloned() else {
            return;
        };
        let Some(request) = self
            .host_requests
            .get(&key)
            .and_then(Option::as_ref)
            .cloned()
        else {
            return;
        };
        let Ok(decoded) = crate::tape_wire::decode::<Request>(&request.payload) else {
            return;
        };
        if matches!(decoded, Request::Print { .. } | Request::PrintStderr { .. }) {
            return;
        }
        let Some(initial) = self.host_initials.get(&key) else {
            return;
        };
        let Some(result) = initial
            .observation
            .clone()
            .filter(|result| result.handles.is_empty())
        else {
            return;
        };
        if self.host_key_observations.get(&key)
            != Some(&Some(vec![(host_keys::Resource::Coupled, Mode::Exclusive)]))
        {
            return;
        }
        let done = initial.result != Err(AdapterError::Blocked);
        let operation = CanonicalItem::HostOperation(ObjectId {
            creator: path.to_vec(),
            ordinal,
        });
        let mut events = vec![
            (operation.clone(), CanonicalOutcome::HostRequest(request)),
            (
                CanonicalItem::HostResource(host_keys::Resource::Coupled),
                CanonicalOutcome::HostAccess {
                    mode: Mode::Exclusive,
                    result: result.clone(),
                },
            ),
        ];
        if done {
            events.push((operation, CanonicalOutcome::HostResult(result.clone())));
        }
        self.subevents(&entry, entry.op.unwrap(), events);
        self.host_entries.remove(&key);
        assert!(
            self.home_phases
                .insert(
                    key,
                    Phase {
                        entry,
                        result: done.then_some(result),
                        parked: false,
                        returned: false,
                        valid: true
                    }
                )
                .is_none()
        );
    }

    pub(super) fn continue_home(
        &mut self,
        key: &(Vec<u32>, u64),
        next: &Next,
    ) -> Option<Subevents> {
        let phase = self.home_phases.get_mut(key)?;
        let outcome = if phase.returned && matches!(next, Next::Keep | Next::Yield) {
            self.home_phases.remove(key);
            CanonicalOutcome::HostFinished
        } else if matches!(next, Next::Park | Next::Yield) {
            if matches!(next, Next::Park) {
                phase.parked = true;
            }
            CanonicalOutcome::HostWaiting
        } else {
            return None;
        };
        Some(vec![(
            CanonicalItem::HostOperation(ObjectId {
                creator: key.0.clone(),
                ordinal: key.1,
            }),
            outcome,
        )])
    }

    pub(super) fn attempt_home(
        &mut self,
        key: &(Vec<u32>, u64),
        attempt: &HostPollCompletion,
        before: Stretch,
    ) {
        let Some(phase) = self.home_phases.get_mut(key) else {
            return;
        };
        let Some(result) = attempt
            .completion
            .observation
            .clone()
            .filter(|result| result.handles.is_empty())
        else {
            phase.valid = false;
            return;
        };
        if !phase.parked || phase.result.is_some() {
            phase.valid = false;
            return;
        }
        let done = attempt.completion.result != Err(AdapterError::Blocked);
        let boundary = Boundary {
            retirement: attempt.retirement,
            before,
            credit: 0,
            transition: Transition::Keep,
            ..phase.entry.clone()
        };
        if done {
            phase.result = Some(result.clone());
        }
        let mut events = vec![(
            CanonicalItem::HostResource(host_keys::Resource::Coupled),
            CanonicalOutcome::HostAccess {
                mode: Mode::Exclusive,
                result: result.clone(),
            },
        )];
        if done {
            events.push((
                CanonicalItem::HostOperation(ObjectId {
                    creator: key.0.clone(),
                    ordinal: key.1,
                }),
                CanonicalOutcome::HostResult(result),
            ));
        }
        self.subevents(&boundary, boundary.op.unwrap(), events);
    }

    pub(super) fn return_home(
        &mut self,
        key: &(Vec<u32>, u64),
        completion: &HostCompletion,
        retirement: u64,
        before: Stretch,
    ) {
        let Some(phase) = self.home_phases.get_mut(key) else {
            return;
        };
        if !phase.valid || phase.result.is_none() || phase.result != completion.observation {
            return;
        }
        if !phase.parked {
            phase.returned = true;
            return;
        }
        let boundary = Boundary {
            retirement,
            before,
            credit: 0,
            transition: Transition::Keep,
            ..phase.entry.clone()
        };
        self.subevents(
            &boundary,
            boundary.op.unwrap(),
            vec![(
                CanonicalItem::HostOperation(ObjectId {
                    creator: key.0.clone(),
                    ordinal: key.1,
                }),
                CanonicalOutcome::HostFinished,
            )],
        );
        self.home_phases.remove(key);
    }
}

#[cfg(test)]
mod tests;
