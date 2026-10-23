//! Project the adapter's discovered resource graph, never its numeric key
//! allocation order. Alias discovery and request footprints remain io_keys'
//! responsibility; this table only names their already established resources.
use super::host_values::HostHandleId;
use crate::io_keys::{Key, KeySet, Kind, Mode};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
mod canonical;
#[cfg(test)]
mod discovery;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(in crate::tasks::ordered) enum Resource {
    Namespace,
    Coupled,
    Root,
    Place,
    Created(HostHandleId),
    Domain { owner: Box<Resource>, label: String },
}

impl Resource {
    /// These four domains exist before any host request; accessing or returning
    /// an alias of one cannot create it. Materialized resources and auxiliary
    /// domains retain their explicit creation evidence.
    pub(super) fn has_creation_event(&self) -> bool {
        matches!(self, Self::Created(_) | Self::Domain { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::ordered) enum ResourceKind {
    File,
    Sink { file_backed: bool },
    Other,
    Half { stream: Resource },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::ordered) struct Creation {
    pub handle: HostHandleId,
    pub resource: Resource,
    pub kind: ResourceKind,
}

impl super::Log {
    /// Certify only the bounded nonparking path whose request, complete key
    /// set, discovery and result are all present. Generic home waits keep
    /// their unfinished boundary until their own poll protocol is supported.
    pub(in crate::tasks::ordered) fn certify_host(
        &mut self,
        task: &[u32],
        ordinal: u64,
    ) -> Result<(), &'static str> {
        use super::{CanonicalItem as I, CanonicalOutcome as O};
        let key = (task.to_vec(), ordinal);
        let boundary = self
            .host_entries
            .get(&key)
            .ok_or("missing host entry")?
            .clone();
        let request = self
            .host_requests
            .get(&key)
            .and_then(Option::as_ref)
            .ok_or("incomplete host request authorities")?
            .clone();
        let keys = self
            .host_key_observations
            .get(&key)
            .and_then(Option::as_ref)
            .ok_or("incomplete host resource keys")?
            .clone();
        let created = self
            .host_creations
            .get(&key)
            .ok_or("incomplete host creation discovery")?
            .clone();
        let completion = self
            .host_completions
            .iter()
            .find(|done| done.task == task && done.ordinal == ordinal)
            .ok_or("incomplete host result")?;
        let initial = self
            .host_initials
            .get(&key)
            .ok_or("missing first host attempt")?;
        if initial != completion {
            return Err("bounded initial attempt differs from completion");
        }
        let result = completion
            .observation
            .as_ref()
            .ok_or("incomplete host result")?
            .clone();
        if created
            .iter()
            .map(|row| &row.handle)
            .ne(result.handles.iter())
        {
            return Err("host creation evidence differs from result handles");
        }
        if self.host_polled.contains(&key) || boundary.transition != super::Transition::Keep {
            return Err("parked host phases need a dedicated projection");
        }
        let output = self
            .host_outputs
            .get(&key)
            .ok_or("missing host output observation")?
            .as_ref()
            .map_err(|error| *error)?;
        let sink = if output.is_some() {
            let mut exclusive = keys.iter().filter(|(_, mode)| *mode == Mode::Exclusive);
            let sink = exclusive
                .next()
                .ok_or("output has no exclusive sink")?
                .0
                .clone();
            if exclusive.next().is_some() {
                return Err("output has ambiguous sink");
            }
            Some(sink)
        } else {
            None
        };
        let operation = crate::ordered::footprint::ObjectId {
            creator: task.to_vec(),
            ordinal,
        };
        let mut events = vec![(I::HostOperation(operation.clone()), O::HostRequest(request))];
        events.extend(
            self.host_domains
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|resource| {
                    !self
                        .versions
                        .contains_key(&I::HostResource((*resource).clone()))
                })
                .map(|resource| (I::HostResource(resource.clone()), O::HostResourceCreated)),
        );
        events.extend(keys.into_iter().map(|(key, mode)| {
            (
                I::HostResource(key),
                O::HostAccess {
                    mode,
                    result: result.clone(),
                },
            )
        }));
        let mut resources = BTreeSet::new();
        for row in created {
            if row.resource.has_creation_event()
                && !self
                    .versions
                    .contains_key(&I::HostResource(row.resource.clone()))
                && resources.insert(row.resource.clone())
            {
                events.push((
                    I::HostResource(row.resource.clone()),
                    O::HostResourceCreated,
                ));
            }
            let item = I::HostHandle(row.handle.clone());
            let outcome = if self.versions.contains_key(&item) {
                O::HostAlias(row)
            } else {
                O::HostCreated(row)
            };
            events.push((item, outcome));
        }
        if let (Some(sink), Some(bytes)) = (sink, output) {
            events.push((I::HostResource(sink), O::HostOutput(bytes.clone())));
        }
        events.push((I::HostOperation(operation), O::HostResult(result)));
        self.subevents(&boundary, boundary.op.unwrap(), events);
        self.host_entries.remove(&key);
        assert!(self.host_certified.insert(key));
        Ok(())
    }

    /// The caller supplies a completed discovery snapshot, so no adapter lock,
    /// home bridge or saved context is borrowed while this table is updated.
    /// Validate all initialized fields before publishing any new identities.
    pub(in crate::tasks::ordered) fn host_created(
        &mut self,
        operation: crate::ordered::footprint::ObjectId,
        result: &Result<crate::io::Completion, crate::io::AdapterError>,
        discovered: &BTreeMap<u64, (Key, Kind)>,
    ) -> Result<Vec<Creation>, &'static str> {
        let mut result = result.clone();
        crate::io_observation::capture(&mut result).map_err(|_| "invalid created host mapping")?;
        let fields = crate::io_observation::fields(&mut result)?;
        let mut resources = BTreeMap::new();
        let mut rows = Vec::new();
        let mut words = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for (field, word) in fields.into_iter().flatten().enumerate() {
            if !words.insert(*word) {
                return Err("duplicate host result handle");
            }
            let (key, kind) = *discovered
                .get(word)
                .ok_or("missing created host resource")?;
            let handle = self
                .host_handles
                .get(word)
                .cloned()
                .unwrap_or_else(|| HostHandleId {
                    operation: operation.clone(),
                    field: field as u32,
                });
            if !identities.insert(handle.clone())
                || self
                    .host_handles
                    .iter()
                    .any(|(prior, id)| prior != word && id == &handle)
            {
                return Err("host handle changed identity");
            }
            let resource = self
                .host_resources
                .resource(key)
                .ok()
                .or_else(|| resources.get(&key).cloned())
                .unwrap_or_else(|| Resource::Created(handle.clone()));
            resources.insert(key, resource.clone());
            let kind = match kind {
                Kind::File => ResourceKind::File,
                Kind::Sink { file_backed } => ResourceKind::Sink { file_backed },
                Kind::Other => ResourceKind::Other,
                Kind::Half { stream } => ResourceKind::Half {
                    stream: self.host_resources.resource(stream)?,
                },
            };
            let row = Creation {
                handle,
                resource,
                kind,
            };
            if self
                .host_bindings
                .get(&row.handle)
                .is_some_and(|prior| prior != &row)
            {
                return Err("host handle changed resource");
            }
            rows.push((*word, row));
        }
        self.host_resources
            .bind(&resources.into_iter().collect::<Vec<_>>())?;
        for (word, row) in &rows {
            self.host_handles.insert(*word, row.handle.clone());
            self.host_bindings.insert(row.handle.clone(), row.clone());
        }
        Ok(rows.into_iter().map(|(_, row)| row).collect())
    }
}

#[derive(Default)]
pub(in crate::tasks::ordered) struct Resources {
    keys: BTreeMap<Key, Resource>,
    identities: BTreeMap<Resource, Key>,
    domains: BTreeMap<(Resource, String), Resource>,
}

impl Resources {
    fn system(key: Key) -> Option<Resource> {
        Some(match key {
            crate::io_keys::NAMESPACE => Resource::Namespace,
            crate::io_keys::COUPLED => Resource::Coupled,
            crate::io_keys::ROOT => Resource::Root,
            crate::io_keys::PLACE => Resource::Place,
            _ => return None,
        })
    }

    pub(in crate::tasks::ordered) fn domains(
        &mut self,
        declarations: &[(Key, Resource, String)],
    ) -> Result<Vec<Resource>, &'static str> {
        let mut aliases = self.domains.clone();
        let mut bindings = BTreeMap::new();
        let mut created = BTreeSet::new();
        for (key, owner, label) in declarations {
            if label.is_empty() {
                return Err("empty host domain label");
            }
            let name = (owner.clone(), label.clone());
            let known = self
                .resource(*key)
                .ok()
                .or_else(|| bindings.get(key).cloned());
            let resource = aliases
                .get(&name)
                .cloned()
                .or_else(|| known.clone())
                .unwrap_or_else(|| {
                    let resource = Resource::Domain {
                        owner: Box::new(owner.clone()),
                        label: label.clone(),
                    };
                    created.insert(resource.clone());
                    resource
                });
            if known.as_ref().is_some_and(|prior| prior != &resource)
                || bindings
                    .insert(*key, resource.clone())
                    .is_some_and(|prior| prior != resource)
            {
                return Err("host domain changed identity");
            }
            aliases.insert(name, resource);
        }
        self.bind(&bindings.into_iter().collect::<Vec<_>>())?;
        self.domains = aliases;
        Ok(created.into_iter().collect())
    }

    /// Validate the whole discovery batch before installing any binding.
    /// A repeated key is an alias of its first creator, never a new identity.
    pub(in crate::tasks::ordered) fn bind(
        &mut self,
        discoveries: &[(Key, Resource)],
    ) -> Result<(), &'static str> {
        let mut batch_keys = BTreeMap::new();
        let mut batch_ids = BTreeMap::new();
        for (key, resource) in discoveries {
            if Self::system(*key)
                .as_ref()
                .is_some_and(|system| system != resource)
                || (!matches!(resource, Resource::Created(_) | Resource::Domain { .. })
                    && Self::system(*key).as_ref() != Some(resource))
                || self.keys.get(key).is_some_and(|prior| prior != resource)
                || self
                    .identities
                    .get(resource)
                    .is_some_and(|prior| prior != key)
                || batch_keys
                    .insert(*key, resource)
                    .is_some_and(|prior| prior != resource)
                || batch_ids
                    .insert(resource, *key)
                    .is_some_and(|prior| prior != *key)
            {
                return Err("host resource changed identity");
            }
        }
        for (key, resource) in discoveries {
            self.keys.insert(*key, resource.clone());
            self.identities.insert(resource.clone(), *key);
        }
        Ok(())
    }

    fn resource(&self, key: Key) -> Result<Resource, &'static str> {
        Self::system(key)
            .or_else(|| self.keys.get(&key).cloned())
            .ok_or("unmapped host resource")
    }

    /// Canonical key order is the stable resource order. Different physical
    /// discovery/interner orders cannot reorder this operation's subevents.
    pub(in crate::tasks::ordered) fn project(
        &self,
        keys: &KeySet,
    ) -> Result<Vec<(Resource, Mode)>, &'static str> {
        let mut out = BTreeSet::new();
        for &(key, mode) in keys.keys() {
            out.insert((self.resource(key)?, mode));
        }
        Ok(out.into_iter().collect())
    }

    pub(in crate::tasks::ordered) fn identity(
        &self,
        identity: Option<(Key, Kind)>,
    ) -> Result<(Resource, ResourceKind), &'static str> {
        let (key, kind) = identity.ok_or("missing host resource discovery")?;
        Ok((
            self.resource(key)?,
            match kind {
                Kind::File => ResourceKind::File,
                Kind::Sink { file_backed } => ResourceKind::Sink { file_backed },
                Kind::Other => ResourceKind::Other,
                Kind::Half { stream } => ResourceKind::Half {
                    stream: self.resource(stream)?,
                },
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::Request;
    use crate::io_keys::Registry;
    use crate::ordered::footprint::ObjectId;

    fn created(ordinal: u64, field: u32) -> Resource {
        Resource::Created(HostHandleId {
            operation: ObjectId {
                creator: vec![1],
                ordinal,
            },
            field,
        })
    }

    #[test]
    fn host_resource_projection_preserves_aliases_namespace_and_split_parent_keys() {
        let mut reference = None;
        for noise in [0, 3, 7] {
            let mut registry = Registry::default();
            for handle in 0..noise {
                registry.resource(handle, Kind::Other);
            }
            let file = registry.inode(100, Kind::File, 77, 88);
            let alias = registry.inode(101, Kind::File, 77, 88);
            assert_eq!(file, alias, "cross-mount hard links share one resource");
            registry.alias(100, 102);
            let stream = registry.resource(200, Kind::Other);
            registry.split(200, 201, 202);
            let read = registry.identity(201).unwrap().0;
            let write = registry.identity(202).unwrap().0;
            assert_ne!(read, write);
            let mut resources = Resources::default();
            resources
                .bind(&[
                    (file, created(4, 0)),
                    (stream, created(5, 0)),
                    (read, created(6, 0)),
                    (write, created(6, 1)),
                ])
                .unwrap();
            assert_eq!(
                resources.identity(registry.identity(100)).unwrap(),
                resources.identity(registry.identity(102)).unwrap()
            );
            let mut rows = Vec::new();
            for request in [
                Request::FileWrite {
                    directory: 9,
                    file: 100,
                    bytes: vec![1],
                },
                Request::FileWrite {
                    directory: 10,
                    file: 101,
                    bytes: vec![1],
                },
                Request::NetRead {
                    net: 9,
                    stream: 201,
                    max: 7,
                },
                Request::NetWrite {
                    net: 9,
                    stream: 202,
                    bytes: vec![1],
                },
                Request::NetJoin {
                    net: 9,
                    read: 201,
                    write: 202,
                },
                Request::Close { handle: 200 },
            ] {
                rows.push(resources.project(&registry.keys(&request)).unwrap());
            }
            assert_eq!(rows[0], rows[1]);
            assert_eq!(
                rows[2],
                [
                    (created(5, 0), Mode::Shared),
                    (created(6, 0), Mode::Exclusive)
                ]
            );
            assert_eq!(
                rows[3],
                [
                    (created(5, 0), Mode::Shared),
                    (created(6, 1), Mode::Exclusive)
                ]
            );
            assert_eq!(
                rows[4],
                [
                    (created(5, 0), Mode::Exclusive),
                    (created(6, 0), Mode::Exclusive),
                    (created(6, 1), Mode::Exclusive)
                ]
            );
            assert_eq!(
                resources.identity(registry.identity(201)).unwrap(),
                (
                    created(6, 0),
                    ResourceKind::Half {
                        stream: created(5, 0)
                    }
                )
            );
            if let Some(reference) = &reference {
                assert_eq!(&rows, reference);
            } else {
                reference = Some(rows);
            }
        }
    }

    #[test]
    fn host_resource_projection_rejects_unknown_keys_parents_and_conflicting_batches() {
        let mut resources = Resources::default();
        assert_eq!(
            resources.project(&KeySet::coupled()).unwrap(),
            [(Resource::Coupled, Mode::Exclusive)]
        );
        assert!(
            resources.identity(None).is_err(),
            "unknown handle is not a coupled resource"
        );
        resources.bind(&[(Key(50), created(4, 0))]).unwrap();
        for batch in [
            vec![(Key(51), created(5, 0)), (Key(50), created(6, 0))],
            vec![(Key(51), created(4, 0))],
            vec![(Key(51), created(5, 0)), (Key(51), created(6, 0))],
            vec![(Key(51), created(5, 0)), (Key(52), created(5, 0))],
            vec![(crate::io_keys::ROOT, created(5, 0))],
            vec![(Key(51), Resource::Root)],
        ] {
            assert!(resources.bind(&batch).is_err());
            assert!(
                resources.resource(Key(51)).is_err(),
                "a failed second discovery installs nothing"
            );
        }
        assert!(
            resources
                .project(&KeySet::new(vec![
                    (Key(50), Mode::Shared),
                    (Key(51), Mode::Exclusive)
                ]))
                .is_err()
        );
        assert!(
            resources
                .identity(Some((Key(50), Kind::Half { stream: Key(51) })))
                .is_err()
        );
        resources.bind(&[(Key(51), created(5, 0))]).unwrap();
        assert_eq!(
            resources
                .identity(Some((Key(50), Kind::Half { stream: Key(51) })))
                .unwrap(),
            (
                created(4, 0),
                ResourceKind::Half {
                    stream: created(5, 0)
                }
            )
        );
    }

    struct Check {
        read: Key,
        write: Key,
        calls: u8,
    }
    impl crate::io::Adapter for Check {
        fn keys(&self, _: &Request) -> Option<KeySet> {
            Some(KeySet::new(vec![
                (self.read, Mode::Exclusive),
                (self.write, Mode::Shared),
                (crate::io_keys::NAMESPACE, Mode::Shared),
                (
                    if self.calls == 0 { self.read } else { Key(199) },
                    Mode::Shared,
                ),
            ]))
        }
        fn preflight(
            &self,
            _: &Request,
        ) -> Result<Option<crate::io::Completion>, crate::io::AdapterError> {
            Ok(None)
        }
        fn call(
            &mut self,
            request: Request,
        ) -> Result<crate::io::Completion, crate::io::AdapterError> {
            let Request::TestCheck { condition, .. } = request else {
                panic!("test request")
            };
            self.calls += 1;
            Ok(crate::io::Completion::Checked(condition))
        }
    }

    #[test]
    fn native_host_issue_keys_keep_the_request_ordinal_and_ignore_numeric_key_order() {
        use crate::tasks::ordered::{engine, events, host};
        use std::sync::{Arc, Mutex};
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                // Invert the interner order; sorting adapter key numbers is
                // insufficient to choose the trace's subevent order.
                let (read, write) = if width % 2 == 0 {
                    (Key(91), Key(17))
                } else {
                    (Key(17), Key(91))
                };
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width, true);
                run.adapter = Some(Box::new(Check {
                    read,
                    write,
                    calls: 0,
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        events::enable(ctx);
                        let log = engine::driver(ctx).events.as_mut().unwrap();
                        log.host_handles.insert(
                            99,
                            HostHandleId {
                                operation: ObjectId {
                                    creator: vec![],
                                    ordinal: 77,
                                },
                                field: 0,
                            },
                        );
                        log.host_resources
                            .bind(&[(read, created(7, 0)), (write, created(7, 1))])
                            .unwrap();
                    },
                    move |ctx| unsafe {
                        for _ in 0..2 {
                            let (result, parked) = crate::host_waits::issue(
                                ctx,
                                Request::TestCheck {
                                    test: 99,
                                    condition: true,
                                },
                            )
                            .unwrap();
                            assert_eq!(result, Ok(crate::io::Completion::Checked(true)));
                            assert!(!parked);
                            host::finish(ctx, crate::ordered::Next::Keep).unwrap();
                        }
                        let log = engine::driver(ctx).events.as_ref().unwrap();
                        let keys = log.host_key_observations[&(vec![], 0)].clone().unwrap();
                        assert_eq!(
                            keys,
                            [
                                (Resource::Namespace, Mode::Shared),
                                (created(7, 0), Mode::Exclusive),
                                (created(7, 1), Mode::Shared)
                            ]
                        );
                        assert!(log.host_key_observations[&(vec![], 1)].is_none());
                        assert_eq!(log.host_requests.len(), 2);
                        assert_eq!(
                            log.host_completions
                                .iter()
                                .map(|done| done.ordinal)
                                .collect::<Vec<_>>(),
                            [0, 1]
                        );
                        assert!(
                            log.canonical().is_err(),
                            "an unknown declared key rejects the complete trace"
                        );
                        *seen.lock().unwrap() = keys;
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let keys = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&keys, reference);
                } else {
                    reference = Some(keys);
                }
            }
        }
    }
}

#[cfg(test)]
mod domain_tests {
    use super::*;
    fn owner(ordinal: u64) -> Resource {
        Resource::Created(HostHandleId {
            operation: crate::ordered::footprint::ObjectId {
                creator: vec![],
                ordinal,
            },
            field: 0,
        })
    }
    #[test]
    fn auxiliary_domains_keep_labels_aliases_and_shared_inherited_sinks_without_numeric_key_identity()
     {
        let mut reference = None;
        for offset in [0, 100] {
            let mut resources = Resources::default();
            let net = owner(1);
            let process = owner(2);
            let port = Key(40 + offset);
            let sink = Key(41 + offset);
            let created = resources
                .domains(&[
                    (port, net.clone(), "ports".into()),
                    (sink, process.clone(), "stdout".into()),
                    (sink, process.clone(), "stderr".into()),
                ])
                .unwrap();
            assert_eq!(created.len(), 2);
            assert_eq!(
                resources
                    .domains(&[(port, net.clone(), "ports".into())])
                    .unwrap(),
                []
            );
            let observed = resources
                .project(&KeySet::new(vec![
                    (port, Mode::Exclusive),
                    (sink, Mode::Shared),
                ]))
                .unwrap();
            if let Some(reference) = &reference {
                assert_eq!(&observed, reference);
            } else {
                reference = Some(observed);
            }
            assert!(
                resources
                    .domains(&[(Key(42 + offset), net.clone(), "ports".into())])
                    .is_err()
            );
            assert!(
                resources
                    .domains(&[(port, net.clone(), "".into())])
                    .is_err()
            );
            // A new process may inherit a sink already named by a previous
            // process or a returned handle; the resource remains the same.
            assert!(
                resources
                    .domains(&[(sink, owner(3), "stdout".into())])
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                resources.resource(sink).unwrap(),
                Resource::Domain {
                    owner: Box::new(process),
                    label: "stdout".into()
                }
            );
        }
    }
}
