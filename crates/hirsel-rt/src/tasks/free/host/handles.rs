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
    pub(in crate::tasks::free) created: Option<Created>,
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
    let created =
        if let Some(word) = handle(&mut canonical).map_err(|error| Stop::Refused(error.into()))? {
            let created = pending.created.clone().ok_or_else(|| {
                Stop::Diverged("host handle creation needs a declared resource".into())
            })?;
            *word = 0;
            Some(created)
        } else {
            None
        };
    pending.encoded.clear();
    Envelope {
        result: canonical,
        created,
    }
    .encode(&mut pending.encoded);
    Ok(())
}

/// The initialized handle field in an admitted result. Keep this exhaustive
/// allow-list common to recording and replay: unsupported results must never
/// leak a host word just because they lack creation metadata.
pub(super) fn handle(
    result: &mut Result<Completion, AdapterError>,
) -> Result<Option<&mut u64>, &'static str> {
    match result {
        Ok(
            Completion::Handle(Ok(word))
            | Completion::ClockGrant(Ok(word))
            | Completion::OptionalFile(Some(word))
            | Completion::NetHandle(Ok(word))
            | Completion::Spawned(Ok(word)),
        )
        | Ok(Completion::Listened(Ok((word, _))) | Completion::Accepted(Ok((word, _))))
        | Ok(Completion::Span { handle: word, .. }) => Ok(Some(word)),
        Err(_)
        | Ok(
            Completion::Unit(_)
            | Completion::Size(_)
            | Completion::Bytes(_)
            | Completion::Wait(_)
            | Completion::Handle(Err(_))
            | Completion::ClockGrant(Err(_))
            | Completion::OptionalFile(None)
            | Completion::NetHandle(Err(_))
            | Completion::Listened(Err(_))
            | Completion::Accepted(Err(_))
            | Completion::Spawned(Err(_)),
        ) => Ok(None),
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
        let word = handle(&mut result).map_err(|error| Stop::Diverged(error.into()))?;
        match (word, envelope.created) {
            (Some(word), Some(created)) if *word == 0 && created.ordinal != 0 => {
                let identity = (created.path.clone(), created.ordinal);
                let local = if let Some(&local) = self.identities.get(&identity) {
                    if self.words[&local] != created {
                        return Err(Stop::Diverged("host creation changed resource".into()));
                    }
                    local
                } else {
                    self.next =
                        self.next.max(1).checked_add(1).ok_or_else(|| {
                            Stop::Diverged("replay handle space exhausted".into())
                        })?;
                    let local = self.next;
                    self.identities.insert(identity, local);
                    self.words.insert(local, created);
                    local
                };
                *word = local;
            }
            (Some(_), _) | (_, Some(_)) => {
                return Err(Stop::Diverged("invalid logical host creation".into()));
            }
            (_, None) => {}
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
            created: Some(Created {
                path: path.to_vec(),
                ordinal,
                resource,
            }),
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
        ];
        let mut handles = Handles::default();
        for completion in failures {
            assert!(handle(&mut Ok(completion.clone())).unwrap().is_none());
            let mut bytes = Vec::new();
            Envelope {
                result: Ok(completion.clone()),
                created: None,
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
    fn unsupported_handle_shapes_and_noncanonical_words_refuse() {
        let mut handles = Handles::default();
        for shape in 0..8 {
            let result = Ok(super::super::tests::created_completion(shape, 99));
            assert!(
                handles
                    .substitute(Envelope {
                        result: result.clone(),
                        created: None
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
                    created: None
                })
                .is_err()
        );
        assert!(handles.words.is_empty());
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
                    created: None
                })
                .is_err()
        );
        assert!(
            handles
                .substitute(Envelope {
                    result: Ok(Completion::Unit(Ok(()))),
                    created: Some(Created {
                        path: vec![],
                        ordinal: 1,
                        resource: 17
                    })
                })
                .is_err()
        );
    }
}
