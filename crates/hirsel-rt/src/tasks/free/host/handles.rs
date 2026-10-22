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
    if !matches!(
        result,
        Err(_)
            | Ok(Completion::Unit(_)
                | Completion::Size(_)
                | Completion::Bytes(_)
                | Completion::Wait(_)
                | Completion::Handle(_))
    ) {
        return Err(Stop::Refused(
            "host result needs an unsupported logical handle codec".into(),
        ));
    }
    let mut canonical = result.clone();
    let created = if let Ok(Completion::Handle(Ok(word))) = &mut canonical {
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
        match (&mut result, envelope.created) {
            (Ok(Completion::Handle(Ok(word))), Some(created))
                if *word == 0 && created.ordinal != 0 =>
            {
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
            (Ok(Completion::Handle(Ok(_))), _) | (_, Some(_)) => {
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
