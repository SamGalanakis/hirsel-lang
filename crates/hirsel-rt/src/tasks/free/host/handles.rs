//! Creator-qualified host identities. Host words stay in the live adapter;
//! a tape carries the creating task, its handle ordinal and a resource key.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::free) struct Created {
    pub(in crate::tasks::free) path: Vec<u32>,
    pub(in crate::tasks::free) ordinal: u64,
    pub(in crate::tasks::free) resource: u32,
}

pub(in crate::tasks::free) struct Envelope {
    pub(in crate::tasks::free) result: Result<Completion, AdapterError>,
    pub(in crate::tasks::free) created: Vec<Created>,
}
impl Wire for Created {
    fn encode(&self, out: &mut Vec<u8>) {
        self.path.encode(out);
        self.ordinal.encode(out);
        self.resource.encode(out);
    }
    fn decode(input: &mut crate::tape_wire::Decode<'_>) -> Result<Self, String> {
        Ok(Self {
            path: Wire::decode(input)?,
            ordinal: Wire::decode(input)?,
            resource: Wire::decode(input)?,
        })
    }
}
impl Wire for Envelope {
    fn encode(&self, out: &mut Vec<u8>) {
        self.result.encode(out);
        self.created.encode(out);
    }
    fn decode(input: &mut crate::tape_wire::Decode<'_>) -> Result<Self, String> {
        Ok(Self {
            result: Wire::decode(input)?,
            created: Wire::decode(input)?,
        })
    }
}

pub(in crate::tasks::free) fn encode(
    pending: &mut Pending,
    result: &Result<Completion, AdapterError>,
) -> Stopped<()> {
    let mut canonical = result.clone();
    let fields = fields(&mut canonical).map_err(|error| Stop::Refused(error.into()))?;
    let count = fields.iter().flatten().count();
    let created = if count == 0 {
        Vec::new()
    } else {
        if count != pending.created.len() {
            return Err(Stop::Diverged(
                "host handle creation needs one declared resource per field".into(),
            ));
        }
        for word in fields.into_iter().flatten() {
            *word = 0;
        }
        pending.created.clone()
    };
    pending.encoded.clear();
    Envelope {
        result: canonical,
        created,
    }
    .encode(&mut pending.encoded);
    Ok(())
}

/// The initialized handle fields in an admitted result. Keep this exhaustive
/// allow-list common to recording and replay: unsupported results must never
/// leak a host word just because they lack creation metadata.
pub(super) fn fields(
    result: &mut Result<Completion, AdapterError>,
) -> Result<[Option<&mut u64>; 2], &'static str> {
    match result {
        Ok(
            Completion::Handle(Ok(word))
            | Completion::ClockGrant(Ok(word))
            | Completion::OptionalFile(Some(word))
            | Completion::NetHandle(Ok(word))
            | Completion::Spawned(Ok(word)),
        )
        | Ok(Completion::Listened(Ok((word, _))) | Completion::Accepted(Ok((word, _))))
        | Ok(Completion::Span { handle: word, .. }) => Ok([Some(word), None]),
        Ok(Completion::Halves(Ok((read, write)))) => Ok([Some(read), Some(write)]),
        Err(_)
        | Ok(
            Completion::Input(_)
            | Completion::Env(_)
            | Completion::Monotonic(_)
            | Completion::Wall(_)
            | Completion::Random(_)
            | Completion::Signal(_)
            | Completion::Checked(_)
            | Completion::Paths(_)
            | Completion::Entries(_)
            | Completion::Metadata(_)
            | Completion::Addresses(_)
            | Completion::NetBytes(_)
            | Completion::Datagram(_)
            | Completion::Count(_)
            | Completion::NetUnit(_)
            | Completion::Certificates(_)
            | Completion::Protocol(_)
            | Completion::Address(_)
            | Completion::Watch(_)
            | Completion::Unit(_)
            | Completion::Size(_)
            | Completion::Bytes(_)
            | Completion::Wait(_)
            | Completion::Handle(Err(_))
            | Completion::ClockGrant(Err(_))
            | Completion::OptionalFile(None)
            | Completion::NetHandle(Err(_))
            | Completion::Listened(Err(_))
            | Completion::Accepted(Err(_))
            | Completion::Spawned(Err(_))
            | Completion::Halves(Err(_))
            | Completion::Process(_)
            | Completion::Pipeline(_)
            | Completion::ChildUnit(_)
            | Completion::ChildBytes(_)
            | Completion::ChildExit(_)
            | Completion::GroupExit(_),
        ) => Ok([None, None]),
        _ => Err("host result needs an unsupported logical handle codec"),
    }
}

#[derive(Default)]
pub(crate) struct Handles {
    identities: BTreeMap<(Vec<u32>, u64), u64>,
    words: BTreeMap<u64, Created>,
    next: u64,
}
impl Handles {
    pub(in crate::tasks::free) fn substitute(
        &mut self,
        envelope: Envelope,
    ) -> Stopped<Result<Completion, AdapterError>> {
        let mut result = envelope.result;
        let fields = fields(&mut result).map_err(|error| Stop::Diverged(error.into()))?;
        if fields.iter().flatten().count() != envelope.created.len() {
            return Err(Stop::Diverged("invalid logical host creation count".into()));
        }
        // Validate every field before changing the table, so a corrupt second
        // field cannot install even the first identity.
        for (index, (word, created)) in fields.iter().flatten().zip(&envelope.created).enumerate() {
            if **word != 0
                || created.ordinal == 0
                || envelope.created[..index]
                    .iter()
                    .any(|prior| prior.path == created.path && prior.ordinal == created.ordinal)
            {
                return Err(Stop::Diverged("invalid logical host creation".into()));
            }
            if let Some(&local) = self
                .identities
                .get(&(created.path.clone(), created.ordinal))
                && self.words[&local] != *created
            {
                return Err(Stop::Diverged("host creation changed resource".into()));
            }
        }
        let new_count = envelope
            .created
            .iter()
            .filter(|created| {
                !self
                    .identities
                    .contains_key(&(created.path.clone(), created.ordinal))
            })
            .count() as u64;
        self.next
            .max(1)
            .checked_add(new_count)
            .ok_or_else(|| Stop::Diverged("replay handle space exhausted".into()))?;
        for (word, created) in fields.into_iter().flatten().zip(envelope.created) {
            let identity = (created.path.clone(), created.ordinal);
            let local = if let Some(&local) = self.identities.get(&identity) {
                local
            } else {
                self.next = self.next.max(1) + 1;
                let local = self.next;
                self.identities.insert(identity, local);
                self.words.insert(local, created);
                local
            };
            *word = local;
        }
        Ok(result)
    }
    pub(in crate::tasks::free) fn resource(&self, word: u64) -> Stopped<crate::io_keys::Key> {
        self.words
            .get(&word)
            .map(|created| crate::io_keys::Key(created.resource))
            .ok_or_else(|| Stop::Diverged("unmapped replay host handle".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(path: &[u32], ordinal: u64, resource: u32) -> Envelope {
        Envelope {
            result: Ok(Completion::Handle(Ok(0))),
            created: vec![Created {
                path: path.to_vec(),
                ordinal,
                resource,
            }],
        }
    }
    fn word(result: Result<Completion, AdapterError>) -> u64 {
        let Ok(Completion::Handle(Ok(word))) = result else {
            panic!("logical handle")
        };
        word
    }
    #[test]
    fn absent_and_failed_creations_never_allocate_a_replay_identity() {
        let failures = [
            Completion::Handle(Err(crate::io::IoError::NotFound)),
            Completion::ClockGrant(Err(())),
            Completion::OptionalFile(None),
            Completion::NetHandle(Err(crate::net::NetError::NotGranted)),
            Completion::Listened(Err(crate::net::NetError::NotGranted)),
            Completion::Accepted(Err(crate::net::NetError::NotGranted)),
            Completion::Spawned(Err(hirsel_run_schema::ProcessFailure::NotGranted)),
            Completion::Halves(Err(crate::net::NetError::NotGranted)),
        ];
        let mut handles = Handles::default();
        for completion in failures {
            assert!(
                fields(&mut Ok(completion.clone()))
                    .unwrap()
                    .iter()
                    .all(Option::is_none)
            );
            let mut bytes = Vec::new();
            Envelope {
                result: Ok(completion.clone()),
                created: Vec::new(),
            }
            .encode(&mut bytes);
            let decoded = crate::tape_wire::decode(&bytes).unwrap();
            assert_eq!(handles.substitute(decoded).unwrap(), Ok(completion.clone()));
            assert!(
                handles
                    .substitute(Envelope {
                        result: Ok(completion),
                        created: envelope(&[], 1, 17).created
                    })
                    .is_err()
            );
        }
        assert_eq!(handles.next, 0);
        assert!(handles.words.is_empty());
    }

    #[test]
    fn unsupported_handle_counts_and_noncanonical_words_refuse() {
        let mut handles = Handles::default();
        for shape in 0..8 {
            let result = Ok(super::super::tests::created_completion(shape, 99));
            assert!(
                handles
                    .substitute(Envelope {
                        result: result.clone(),
                        created: Vec::new()
                    })
                    .is_err()
            );
            assert!(
                handles
                    .substitute(Envelope {
                        result,
                        created: envelope(&[], 1, 17).created
                    })
                    .is_err()
            );
        }
        assert!(
            handles
                .substitute(Envelope {
                    result: Ok(Completion::Halves(Ok((99, 100)))),
                    created: Vec::new()
                })
                .is_err()
        );
        assert!(handles.words.is_empty());
    }

    fn pair(second_ordinal: u64, second_resource: u32) -> Envelope {
        let mut created = envelope(&[], 1, 17).created;
        created.extend(envelope(&[], second_ordinal, second_resource).created);
        Envelope {
            result: Ok(Completion::Halves(Ok((0, 0)))),
            created,
        }
    }

    #[test]
    fn split_validates_all_fields_before_mapping_and_reuses_both_identities() {
        let mut handles = Handles::default();
        for mut invalid in [pair(0, 17), pair(1, 17), pair(2, 17)] {
            if invalid.created[1].ordinal == 2 {
                invalid.result = Ok(Completion::Halves(Ok((0, 99))));
            }
            assert!(handles.substitute(invalid).is_err());
            assert!(handles.words.is_empty());
        }
        let original = handles.substitute(pair(2, 17)).unwrap();
        let Ok(Completion::Halves(Ok((read, write)))) = original else {
            panic!("halves")
        };
        assert_ne!(read, write);
        assert_eq!(
            handles.resource(read).unwrap(),
            handles.resource(write).unwrap()
        );
        assert_eq!(handles.substitute(pair(2, 17)).unwrap(), original);
        assert!(handles.substitute(pair(2, 18)).is_err());
        assert_eq!(handles.words.len(), 2);
        assert_eq!(handles.resource(write).unwrap().0, 17);
        assert_eq!(handles.substitute(pair(2, 17)).unwrap(), original);
        let mut independent = Handles::default();
        let Ok(Completion::Halves(Ok((read, write)))) =
            independent.substitute(pair(2, 18)).unwrap()
        else {
            panic!("halves")
        };
        assert_eq!(independent.resource(read).unwrap().0, 17);
        assert_eq!(independent.resource(write).unwrap().0, 18);
    }

    #[test]
    fn creator_qualification_aliases_and_observation_completion_reuse() {
        let mut handles = Handles::default();
        let first = word(handles.substitute(envelope(&[0], 1, 17)).unwrap());
        assert_eq!(
            word(handles.substitute(envelope(&[0], 1, 17)).unwrap()),
            first
        );
        let second = word(handles.substitute(envelope(&[1], 1, 17)).unwrap());
        assert_ne!(first, second);
        assert_eq!(
            handles.resource(first).unwrap(),
            handles.resource(second).unwrap()
        );
        assert!(handles.substitute(envelope(&[0], 1, 18)).is_err());
        assert!(handles.substitute(envelope(&[0], 0, 17)).is_err());
        assert!(handles.resource(999).is_err());
        assert!(
            handles
                .substitute(Envelope {
                    result: Ok(Completion::Handle(Ok(99))),
                    created: Vec::new()
                })
                .is_err()
        );
        assert!(
            handles
                .substitute(Envelope {
                    result: Ok(Completion::Unit(Ok(()))),
                    created: vec![Created {
                        path: vec![],
                        ordinal: 1,
                        resource: 17
                    }]
                })
                .is_err()
        );
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;
    use crate::io::{
        ClockError, DirEntry, EnvError, FileKind, InputError, IoError, Metadata, RandomError,
        SignalError,
    };
    use crate::net::{NetError, SocketAddr};
    use crate::watch::WatchEvent;

    #[test]
    fn owned_scalar_directory_and_network_results_preserve_payloads_without_creations() {
        let address = SocketAddr::of("[2001:db8::7]:4488".parse().unwrap());
        let completions = vec![
            Completion::Input(Ok(vec![0, 255, 3])),
            Completion::Input(Err(InputError::Canceled(7))),
            Completion::Env(Ok(Some("value\0λ".into()))),
            Completion::Env(Ok(None)),
            Completion::Env(Err(EnvError::NotGranted)),
            Completion::Monotonic(Ok(u64::MAX)),
            Completion::Monotonic(Err(ClockError::Unavailable)),
            Completion::Wall(Ok(i64::MIN)),
            Completion::Wall(Err(ClockError::OutOfRange)),
            Completion::Random(Ok(vec![255, 0, 1, 2])),
            Completion::Random(Err(RandomError::Unavailable)),
            Completion::Signal(Ok(hirsel_run_schema::SignalKind::Terminate)),
            Completion::Signal(Err(SignalError::Canceled(9))),
            Completion::Checked(true),
            Completion::Checked(false),
            Completion::Paths(Ok(vec!["a/λ".into(), "b".into()])),
            Completion::Paths(Err(IoError::PermissionDenied)),
            Completion::Entries(Ok(vec![DirEntry {
                path: "a/λ".into(),
                kind: FileKind::Regular,
            }])),
            Completion::Entries(Err(IoError::NotDirectory)),
            Completion::Metadata(Ok(Metadata {
                kind: FileKind::Directory,
                size: u64::MAX,
                modified_unix_ns: Some(i128::MIN),
                permissions: 0o751,
            })),
            Completion::Metadata(Ok(Metadata {
                kind: FileKind::Regular,
                size: 0,
                modified_unix_ns: None,
                permissions: 0,
            })),
            Completion::Metadata(Err(IoError::NotFound)),
            Completion::Addresses(Ok(vec![address])),
            Completion::Addresses(Err(NetError::NotGranted)),
            Completion::NetBytes(Ok(vec![0, 255, 7])),
            Completion::NetBytes(Err(NetError::NotGranted)),
            Completion::Datagram(Ok((vec![255, 0], address, true))),
            Completion::Datagram(Err(NetError::NotGranted)),
            Completion::Count(Ok(u64::MAX)),
            Completion::Count(Err(NetError::NotGranted)),
            Completion::NetUnit(Ok(())),
            Completion::NetUnit(Err(NetError::NotGranted)),
            Completion::Certificates(Ok(Some(vec![0, 255, 1]))),
            Completion::Certificates(Ok(None)),
            Completion::Certificates(Err(NetError::NotGranted)),
            Completion::Protocol(Ok(Some("h2".into()))),
            Completion::Protocol(Ok(None)),
            Completion::Protocol(Err(NetError::NotGranted)),
            Completion::Address(Ok(address)),
            Completion::Address(Err(NetError::NotGranted)),
            Completion::Watch(Ok(WatchEvent::Renamed("old/λ".into(), "new".into()))),
            Completion::Watch(Ok(WatchEvent::Overflow)),
            Completion::Watch(Err(IoError::Canceled(3))),
        ];
        let mut handles = Handles::default();
        for completion in completions {
            let mut result = Ok(completion.clone());
            assert!(
                fields(&mut result).unwrap().iter().all(Option::is_none),
                "{completion:?}"
            );
            let mut bytes = Vec::new();
            Envelope {
                result,
                created: Vec::new(),
            }
            .encode(&mut bytes);
            let decoded = crate::tape_wire::decode(&bytes).unwrap();
            assert_eq!(handles.substitute(decoded).unwrap(), Ok(completion.clone()));
            // A scalar observation cannot smuggle an unrelated creation into
            // the replay map, including failed and absent result payloads.
            assert!(
                handles
                    .substitute(Envelope {
                        result: Ok(completion),
                        created: vec![Created {
                            path: vec![1],
                            ordinal: 1,
                            resource: 17
                        }],
                    })
                    .is_err()
            );
            assert!(handles.words.is_empty());
        }
        assert_eq!(handles.next, 0);
    }
}
