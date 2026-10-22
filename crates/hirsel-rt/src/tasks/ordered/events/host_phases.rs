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
    keys: Vec<(host_keys::Resource, Mode)>,
}

impl Phase {
    pub(super) fn invalidate(&mut self) {
        self.valid = false;
    }
}

pub(crate) fn request_keys(ctx: *mut TaskCtx, keys: Option<crate::io_keys::KeySet>) {
    let declared = keys.is_some();
    super::request_keys(ctx, &keys.unwrap_or_else(crate::io_keys::KeySet::coupled));
    let s = unsafe { (*(*(*ctx).run_ptr()).tasks.get()).as_deref_mut().unwrap() };
    let path = s.task(unsafe { (*ctx).task_id }).path.clone();
    let Logical::Ordered(driver) = &mut s.logical else {
        unreachable!()
    };
    let log = driver.events.as_mut().unwrap();
    let ordinal = log.ordinals[&path]
        .checked_sub(1)
        .expect("home request follows entry");
    let key = (path, ordinal);
    if declared {
        assert!(log.home_declared.insert(key.clone()));
    }
    assert!(log.home_requests.insert(key));
}

impl Log {
    /// Home adapters declare no worker key graph: all their attempts use the
    /// existing conservative coupled domain. Names are still field-qualified;
    /// no native handle word enters the canonical result.
    pub(super) fn discover_home(
        &mut self,
        key: &(Vec<u32>, u64),
        result: &Result<crate::io::Completion, AdapterError>,
    ) -> Result<(), &'static str> {
        if !self.home_requests.contains(key) || self.home_declared.contains(key) {
            return Ok(());
        }
        let result = self.discover_home_fields(key, result);
        if result.is_err() {
            self.invalid_home.insert(key.clone());
            self.host_creations.remove(key);
            if let Some(phase) = self.home_phases.get_mut(key) {
                phase.valid = false;
            }
        }
        result
    }

    fn discover_home_fields(
        &mut self,
        key: &(Vec<u32>, u64),
        result: &Result<crate::io::Completion, AdapterError>,
    ) -> Result<(), &'static str> {
        if matches!(result, Err(AdapterError::InvalidHandle(_))) {
            return Ok(());
        }
        let request = self
            .host_requests
            .get(key)
            .and_then(Option::as_ref)
            .ok_or("unknown home request authority")?;
        let decoded = crate::tape_wire::decode::<Request>(&request.payload)
            .map_err(|_| "invalid home request observation")?;
        if matches!(decoded, Request::NetSplit { .. }) {
            let parent = request
                .authorities
                .get(1)
                .and_then(|authority| match authority {
                    HostAuthority::Created(handle) => self.host_bindings.get(handle),
                    _ => None,
                })
                .ok_or("missing home split parent metadata")?;
            if parent.resource != host_keys::Resource::Coupled {
                return Err("home split parent has conflicting declared resource");
            }
        }
        let request = decoded;
        let mut image = result.clone();
        crate::io_observation::capture(&mut image).map_err(|_| "invalid home result mapping")?;
        let fields = crate::io_observation::fields(&mut image)?;
        let mut discovered = BTreeMap::new();
        for word in fields.into_iter().flatten() {
            let kind = match self
                .host_handles
                .get(word)
                .and_then(|handle| self.host_bindings.get(handle))
            {
                Some(row) if row.resource != host_keys::Resource::Coupled => {
                    return Err("home handle has conflicting declared resource");
                }
                Some(row) => match row.kind {
                    host_keys::ResourceKind::Half { .. } => crate::io_keys::Kind::Half {
                        stream: crate::io_keys::COUPLED,
                    },
                    host_keys::ResourceKind::Other => crate::io_keys::Kind::Other,
                    _ => return Err("home handle has conflicting resource kind"),
                },
                None if matches!(request, Request::NetSplit { .. }) => crate::io_keys::Kind::Half {
                    stream: crate::io_keys::COUPLED,
                },
                None => crate::io_keys::Kind::Other,
            };
            discovered.insert(*word, (crate::io_keys::COUPLED, kind));
        }
        let rows = self.host_created(
            ObjectId {
                creator: key.0.clone(),
                ordinal: key.1,
            },
            result,
            &discovered,
        )?;
        self.host_creations.insert(key.clone(), rows);
        Ok(())
    }

    fn home_created_events(
        &self,
        key: &(Vec<u32>, u64),
        result: &host_values::HostValue,
    ) -> Option<Subevents> {
        if result.handles.is_empty() {
            return Some(Vec::new());
        }
        let rows = self.host_creations.get(key)?;
        if rows.iter().map(|row| &row.handle).ne(result.handles.iter()) {
            return None;
        }
        if self.concurrent_requests.contains(key) || self.home_declared.contains(key) {
            let mut resources = BTreeSet::new();
            let mut events = Vec::new();
            for row in rows {
                let resource = CanonicalItem::HostResource(row.resource.clone());
                if !self.versions.contains_key(&resource) && resources.insert(row.resource.clone())
                {
                    events.push((resource, CanonicalOutcome::HostResourceCreated));
                }
                let item = CanonicalItem::HostHandle(row.handle.clone());
                let outcome = if self.versions.contains_key(&item) {
                    CanonicalOutcome::HostAlias(row.clone())
                } else {
                    CanonicalOutcome::HostCreated(row.clone())
                };
                events.push((item, outcome));
            }
            return Some(events);
        }
        Some(
            rows.iter()
                .map(|row| {
                    let item = CanonicalItem::HostHandle(row.handle.clone());
                    let outcome = if self.versions.contains_key(&item) {
                        CanonicalOutcome::HostAlias(row.clone())
                    } else {
                        CanonicalOutcome::HostCreated(row.clone())
                    };
                    (item, outcome)
                })
                .collect(),
        )
    }

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
        if (!self.home_requests.contains(&key) && !self.concurrent_requests.contains(&key))
            || self.invalid_home.contains(&key)
        {
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
        let Some(result) = initial.observation.clone() else {
            return;
        };
        let Some(created) = self.home_created_events(&key, &result) else {
            return;
        };
        let Some(keys) = self
            .host_key_observations
            .get(&key)
            .and_then(Option::as_ref)
            .cloned()
        else {
            return;
        };
        if self.home_requests.contains(&key)
            && !self.home_declared.contains(&key)
            && keys != vec![(host_keys::Resource::Coupled, Mode::Exclusive)]
        {
            return;
        }
        let done = initial.result != Err(AdapterError::Blocked);
        let operation = CanonicalItem::HostOperation(ObjectId {
            creator: path.to_vec(),
            ordinal,
        });
        let mut events = vec![(operation.clone(), CanonicalOutcome::HostRequest(request))];
        events.extend(
            self.host_domains
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|resource| {
                    !self
                        .versions
                        .contains_key(&CanonicalItem::HostResource((*resource).clone()))
                })
                .map(|resource| {
                    (
                        CanonicalItem::HostResource(resource.clone()),
                        CanonicalOutcome::HostResourceCreated,
                    )
                }),
        );
        events.extend(keys.iter().map(|(resource, mode)| {
            (
                CanonicalItem::HostResource(resource.clone()),
                CanonicalOutcome::HostAccess {
                    mode: *mode,
                    result: result.clone(),
                },
            )
        }));
        if done {
            events.extend(created);
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
                        valid: true,
                        keys,
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
        let created = attempt
            .completion
            .observation
            .as_ref()
            .and_then(|result| self.home_created_events(key, result));
        let Some(phase) = self.home_phases.get_mut(key) else {
            return;
        };
        let Some(created) = created else {
            phase.valid = false;
            return;
        };
        let Some(result) = attempt.completion.observation.clone() else {
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
        let mut events = phase
            .keys
            .iter()
            .map(|(resource, mode)| {
                (
                    CanonicalItem::HostResource(resource.clone()),
                    CanonicalOutcome::HostAccess {
                        mode: *mode,
                        result: result.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        if done {
            events.extend(created);
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
