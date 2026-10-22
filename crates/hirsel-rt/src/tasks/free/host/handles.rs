//! Creator-qualified host identities. Host words stay in the live adapter;
//! a tape carries the creating task, its handle ordinal and a resource key.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::free) struct Created {
    pub(in crate::tasks::free) path: Vec<u32>,
    pub(in crate::tasks::free) ordinal: u64,
    pub(in crate::tasks::free) resource: u32,
    pub(in crate::tasks::free) kind: crate::io_keys::Kind,
}

pub(in crate::tasks::free) struct Envelope {
    pub(in crate::tasks::free) result: Result<Completion, AdapterError>,
    pub(in crate::tasks::free) created: Vec<Created>,
    pub(in crate::tasks::free) output_count: Option<u64>,
}
impl Wire for Created {
    fn encode(&self, out: &mut Vec<u8>) {
        self.path.encode(out);
        self.ordinal.encode(out);
        self.resource.encode(out);
        match self.kind {
            crate::io_keys::Kind::File => 0u8.encode(out),
            crate::io_keys::Kind::Sink { file_backed } => {
                1u8.encode(out);
                file_backed.encode(out);
            }
            crate::io_keys::Kind::Other => 2u8.encode(out),
            crate::io_keys::Kind::Half { stream } => {
                3u8.encode(out);
                stream.0.encode(out);
            }
        }
    }
    fn decode(input: &mut crate::tape_wire::Decode<'_>) -> Result<Self, String> {
        Ok(Self {
            path: Wire::decode(input)?,
            ordinal: Wire::decode(input)?,
            resource: Wire::decode(input)?,
            kind: match u8::decode(input)? {
                0 => crate::io_keys::Kind::File,
                1 => crate::io_keys::Kind::Sink {
                    file_backed: Wire::decode(input)?,
                },
                2 => crate::io_keys::Kind::Other,
                3 => crate::io_keys::Kind::Half {
                    stream: crate::io_keys::Key(Wire::decode(input)?),
                },
                _ => return Err("unknown host resource kind".into()),
            },
        })
    }
}
impl Wire for Envelope {
    fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"F1H2");
        self.result.encode(out);
        self.created.encode(out);
        self.output_count.encode(out);
    }
    fn decode(input: &mut crate::tape_wire::Decode<'_>) -> Result<Self, String> {
        if input.take(4)? != b"F1H2" {
            return Err("unsupported Free host envelope".into());
        }
        Ok(Self {
            result: Wire::decode(input)?,
            created: Wire::decode(input)?,
            output_count: Wire::decode(input)?,
        })
    }
}

pub(in crate::tasks::free) fn encode(
    pending: &mut Pending,
    result: &mut Result<Completion, AdapterError>,
) -> Stopped<()> {
    capture(result)?;
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
    if pending.track_output && pending.output_count.is_none() {
        pending.output_count = match result {
            Ok(Completion::Unit(Ok(()))) => {
                Some(output_bytes(pending.request.buffers()).unwrap().len() as u64)
            }
            Ok(Completion::Unit(Err(
                crate::io::IoError::Canceled(_) | crate::io::IoError::NotGranted,
            ))) => Some(0),
            _ => {
                return Err(Stop::Refused(
                    "failed output needs an observed written prefix".into(),
                ));
            }
        };
    }
    if pending.track_output {
        checked_output(pending.request.buffers(), result, pending.output_count)?;
    }
    pending.encoded.clear();
    Envelope {
        output_count: pending.output_count,
        result: canonical,
        created,
    }
    .encode(&mut pending.encoded);
    Ok(())
}

// Preserve the established Free codec seam and callers.
pub(super) use crate::io_observation::{capture, fields};

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
    pub(in crate::tasks::free) fn identity(
        &self,
        word: u64,
    ) -> Stopped<(crate::io_keys::Key, crate::io_keys::Kind)> {
        self.words
            .get(&word)
            .map(|created| (crate::io_keys::Key(created.resource), created.kind))
            .ok_or_else(|| Stop::Diverged("unmapped replay host handle".into()))
    }
    pub(in crate::tasks::free) fn resource(&self, word: u64) -> Stopped<crate::io_keys::Key> {
        self.identity(word).map(|(key, _)| key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(path: &[u32], ordinal: u64, resource: u32) -> Envelope {
        Envelope {
            output_count: None,
            result: Ok(Completion::Handle(Ok(0))),
            created: vec![Created {
                path: path.to_vec(),
                ordinal,
                resource,
                kind: crate::io_keys::Kind::Other,
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
    fn resource_kinds_round_trip_and_changed_metadata_cannot_replace_an_identity() {
        use crate::io_keys::{Key, Kind};
        for (index, kind) in [
            Kind::File,
            Kind::Sink { file_backed: false },
            Kind::Sink { file_backed: true },
            Kind::Other,
            Kind::Half { stream: Key(71) },
        ]
        .into_iter()
        .enumerate()
        {
            let mut value = envelope(&[1, 2], index as u64 + 1, 17);
            value.created[0].kind = kind;
            let mut bytes = Vec::new();
            value.encode(&mut bytes);
            let decoded: Envelope = crate::tape_wire::decode(&bytes).unwrap();
            assert_eq!(decoded.created[0].kind, kind);
            let mut handles = Handles::default();
            let word = word(handles.substitute(decoded).unwrap());
            assert_eq!(handles.identity(word).unwrap(), (Key(17), kind));
            assert!(handles.identity(999).is_err());
            let mut changed = value;
            changed.created[0].kind = if kind == Kind::Other {
                Kind::File
            } else {
                Kind::Other
            };
            assert!(handles.substitute(changed).is_err());
            assert_eq!(handles.identity(word).unwrap(), (Key(17), kind));
            // Old or unknown private payload versions never decode as results.
            bytes[0] ^= 1;
            assert!(crate::tape_wire::decode::<Envelope>(&bytes).is_err());
        }
        let mut value = envelope(&[], 1, 17);
        value.created[0].kind = Kind::Other;
        let mut bytes = Vec::new();
        value.encode(&mut bytes);
        *bytes.last_mut().unwrap() = 255;
        assert!(crate::tape_wire::decode::<Envelope>(&bytes).is_err());
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
                output_count: None,
                result: Ok(completion.clone()),
                created: Vec::new(),
            }
            .encode(&mut bytes);
            let decoded = crate::tape_wire::decode(&bytes).unwrap();
            assert_eq!(handles.substitute(decoded).unwrap(), Ok(completion.clone()));
            assert!(
                handles
                    .substitute(Envelope {
                        output_count: None,
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
                        output_count: None,
                        result: result.clone(),
                        created: Vec::new()
                    })
                    .is_err()
            );
            assert!(
                handles
                    .substitute(Envelope {
                        output_count: None,
                        result,
                        created: envelope(&[], 1, 17).created
                    })
                    .is_err()
            );
        }
        assert!(
            handles
                .substitute(Envelope {
                    output_count: None,
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
            output_count: None,
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
                    output_count: None,
                    result: Ok(Completion::Handle(Ok(99))),
                    created: Vec::new()
                })
                .is_err()
        );
        assert!(
            handles
                .substitute(Envelope {
                    output_count: None,
                    result: Ok(Completion::Unit(Ok(()))),
                    created: vec![Created {
                        path: vec![],
                        ordinal: 1,
                        resource: 17,
                        kind: crate::io_keys::Kind::Other,
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
                output_count: None,
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
                        output_count: None,
                        result: Ok(completion),
                        created: vec![Created {
                            path: vec![1],
                            ordinal: 1,
                            resource: 17,
                            kind: crate::io_keys::Kind::Other,
                        }],
                    })
                    .is_err()
            );
            assert!(handles.words.is_empty());
        }
        assert_eq!(handles.next, 0);
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::io::MapView;

    #[test]
    fn native_mapped_result_survives_close_before_ended_and_replays_without_adapter() {
        use hirsel_free::projection::Projection;
        for width in [1, 8] {
            for durable in [false, true] {
                let path = std::env::temp_dir().join(format!(
                    "hirsel-f1-map-native-{}-{width}-{durable}",
                    std::process::id()
                ));
                let bytes = b"owned mapping\0\xff".to_vec();
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .unwrap();
                use std::io::Write;
                (&file).write_all(&bytes).unwrap();
                std::fs::remove_file(path).unwrap();
                let mut file = Some(file);
                let recording =
                    super::super::super::record::Recording::new(b"mapped host result", durable);
                let install = recording.clone();
                let expected = bytes.clone();
                let mut run = crate::host::BodyRun::new(width, false);
                run.free = true;
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        super::super::super::enable(ctx);
                        install.root(ctx);
                    },
                    move |ctx| unsafe {
                        let mapping = crate::mapping::Mapping::new(
                            file.take().unwrap(),
                            expected.len() as u64,
                        )
                        .unwrap();
                        let _role = role::Guard::new(ctx);
                        crate::stop::check(
                            ctx,
                            prepare(
                                ctx,
                                Request::MapView {
                                    directory: 1,
                                    mapped: 17,
                                },
                            ),
                        );
                        assert!(crate::stop::check(ctx, claim(ctx)));
                        crate::stop::check(
                            ctx,
                            invoke(ctx, |_| {
                                Completion::View(MapView {
                                    length: expected.len() as u64,
                                    bytes: None,
                                    block: mapping.block(),
                                })
                            }),
                        );
                        drop(mapping);
                        crate::stop::check(ctx, ended(ctx));
                        let Completion::View(view) = crate::stop::check(ctx, complete(ctx)) else {
                            panic!("view")
                        };
                        assert_eq!(view.block, 0);
                        assert_eq!(view.contents(), expected);
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                let cut = recording
                    .recorder
                    .stop("returned", None)
                    .unwrap()
                    .unwrap()
                    .0;
                let projection = Projection::new("returned", &cut.progress, &recording.trace());
                let tape = Arc::try_unwrap(recording)
                    .ok()
                    .unwrap()
                    .recorder
                    .into_sink()
                    .bytes;
                for width in [1, 8] {
                    let replay = super::super::super::replay::Replay::new(&tape).unwrap();
                    let install = replay.clone();
                    let expected = bytes.clone();
                    let mut run = crate::host::BodyRun::new(width, false);
                    run.free = true;
                    unsafe { run.use_physical_dispatch() };
                    let outcome = run.run(
                        move |ctx| unsafe { install.root(ctx) },
                        move |ctx| unsafe {
                            let _role = role::Guard::new(ctx);
                            let result = crate::stop::check(
                                ctx,
                                replay_request(
                                    ctx,
                                    Request::MapView {
                                        directory: 1,
                                        mapped: 17,
                                    },
                                    KeySet::coupled(),
                                ),
                            );
                            let Ok(Completion::View(view)) = result else {
                                panic!("view")
                            };
                            assert_eq!(view.block, 0);
                            assert_eq!(view.contents(), expected);
                            super::super::super::replay::start(ctx);
                            unreachable!("mapped result past cut");
                        },
                    );
                    assert!(
                        matches!(outcome.ended, Err(Stop::Blocked(_))),
                        "{outcome:?}"
                    );
                    assert_eq!(replay.finish().unwrap(), projection);
                }
            }
        }
    }

    #[test]
    fn mapped_views_snapshot_before_publication_and_replay_after_unmapping() {
        // Mapping's production static Bytes block: anonymous metadata prefix,
        // followed by the live file pages. Keep both forms in the same codec.
        let path = std::env::temp_dir().join(format!("hirsel-f1-map-{}", std::process::id()));
        let bytes = vec![0, 255, 7, 11, 0, 99];
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        (&file).write_all(&bytes).unwrap();
        let mapping = crate::mapping::Mapping::new(file, bytes.len() as u64).unwrap();
        let address = mapping.block();
        assert_ne!(address, 0);
        let borrowed = Ok(Completion::View(MapView {
            length: bytes.len() as u64,
            bytes: None,
            block: address,
        }));
        let mut canonical = borrowed.clone();
        capture(&mut canonical).unwrap();
        let Ok(Completion::View(view)) = &canonical else {
            panic!("view")
        };
        assert_eq!(view.block, 0);
        assert_eq!(view.bytes.as_ref(), Some(&bytes));
        let mut owned = canonical.clone();
        assert!(fields(&mut owned).unwrap().iter().all(Option::is_none));
        let mut tape = Vec::new();
        Envelope {
            output_count: None,
            result: canonical,
            created: Vec::new(),
        }
        .encode(&mut tape);
        assert!(
            !tape
                .windows(size_of::<usize>())
                .any(|part| part == address.to_le_bytes())
        );
        drop(mapping);
        std::fs::remove_file(path).unwrap();
        let mut handles = Handles::default();
        let decoded = crate::tape_wire::decode(&tape).unwrap();
        let replayed = handles.substitute(decoded).unwrap();
        let Ok(Completion::View(view)) = replayed else {
            panic!("view")
        };
        assert_eq!(view.block, 0);
        assert_eq!(view.length, bytes.len() as u64);
        assert_eq!(view.contents(), bytes);
        assert!(handles.words.is_empty());

        // Refuse invalid owned captures before serialization, and never
        // admit an uncaptured native address through the replay allow-list.
        let mut borrowed = borrowed;
        assert!(fields(&mut borrowed).is_err());
        assert!(
            capture(&mut Ok(Completion::View(MapView {
                length: 2,
                bytes: Some(vec![1]),
                block: 0
            })))
            .is_err()
        );
        assert!(
            capture(&mut Ok(Completion::View(MapView {
                length: 1,
                bytes: None,
                block: 0
            })))
            .is_err()
        );
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn handle_bearing_adapter_refusals_never_publish_physical_words() {
        const WORD: u64 = 0x1f23_4567_89ab_cdef;
        for width in [1, 8] {
            let recording = super::super::super::record::Recording::new(b"host refusal", false);
            let install = recording.clone();
            let mut run = crate::host::BodyRun::new(width, false);
            run.free = true;
            unsafe { run.use_physical_dispatch() };
            let outcome = run.run(
                move |ctx| unsafe {
                    super::super::super::enable(ctx);
                    install.root(ctx);
                },
                move |ctx| unsafe {
                    let _role = role::Guard::new(ctx);
                    crate::stop::check(ctx, prepare(ctx, Request::Stdout { io: 1 }));
                    assert!(crate::stop::check(ctx, claim(ctx)));
                    let result = invoke_result(ctx, |_| Err(AdapterError::InvalidHandle(WORD)));
                    assert!(matches!(result, Err(Stop::Refused(_))));
                    let pending = (*ctx).pending_host.as_ref().unwrap();
                    assert_eq!(
                        pending.completion,
                        Some(Err(AdapterError::InvalidHandle(WORD)))
                    );
                    assert!(pending.encoded.is_empty());
                    crate::STATUS_OK
                },
            );
            assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
            let (cut, sweep) = recording.recorder.stop("refused", None).unwrap().unwrap();
            assert_eq!(cut.progress[&Vec::new()], Progress::new(1, Phase::Claimed));
            assert_eq!(sweep.in_doubt, [1]);
            let tape = Arc::try_unwrap(recording)
                .ok()
                .unwrap()
                .recorder
                .into_sink()
                .bytes;
            assert!(!tape.windows(8).any(|bytes| bytes == WORD.to_le_bytes()));
        }
        // A crafted envelope cannot inject a native word on the replay side.
        let mut encoded = Vec::new();
        Envelope {
            output_count: None,
            result: Err(AdapterError::InvalidHandle(WORD)),
            created: Vec::new(),
        }
        .encode(&mut encoded);
        let mut handles = Handles::default();
        let value = crate::tape_wire::decode(&encoded).unwrap();
        assert!(handles.substitute(value).is_err());
        assert!(handles.words.is_empty());
    }
}
