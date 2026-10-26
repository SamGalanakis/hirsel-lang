//! Recorded resource footprints: replay serves exactly the keys, domains and
//! identities the live keyed adapter declared, through the shared tape codec.
use super::*;
use crate::io_keys::{COUPLED, Footprint, Key, KeySet, Mode, NAMESPACE};
use crate::io_sim::SimIo;

/// One request as the Ordered trace sees it: keys and domains before the
/// call, the result, then each result handle's identity.
type Seen = (
    Request,
    KeySet,
    Vec<(Key, u64, String)>,
    Result<Completion, AdapterError>,
    Vec<Option<(Key, Kind)>>,
);

fn words(result: &Result<Completion, AdapterError>) -> Vec<u64> {
    let mut result = result.clone();
    crate::io_observation::capture(&mut result).unwrap();
    crate::io_observation::fields(&mut result)
        .unwrap()
        .into_iter()
        .flatten()
        .map(|word| *word)
        .collect()
}

struct Live {
    io: ConcurrentIo,
    seen: Vec<Seen>,
}
impl Live {
    /// A granted temporary root holding `files`, with its streams replaced
    /// before admission discovers them.
    fn new(
        files: &[(&str, &[u8])],
        streams: impl FnOnce(&std::path::Path, &mut SendStreams),
    ) -> (tempfile::TempDir, Live) {
        let temp = tempfile::tempdir().unwrap();
        for (path, bytes) in files {
            std::fs::write(temp.path().join(path), bytes).unwrap();
        }
        let io = RealIo::grant(temp.path())
            .unwrap()
            .into_concurrent()
            .unwrap_or_else(|_| panic!("Send streams"));
        let mut core = io.core.into_inner().unwrap();
        streams(temp.path(), &mut core.streams);
        let io = ConcurrentIo::new(core);
        (temp, Live { io, seen: vec![] })
    }
    fn call(&mut self, request: Request) -> Completion {
        let keys = self.io.keys(&request);
        let domains = self.io.domains(&request);
        let result = self.io.call(request.clone());
        let identities = words(&result)
            .into_iter()
            .map(|word| self.io.identity(word))
            .collect();
        self.seen
            .push((request, keys, domains, result.clone(), identities));
        result.unwrap()
    }
    fn handle(&mut self, request: Request) -> u64 {
        match self.call(request) {
            Completion::Handle(Ok(handle)) => handle,
            other => panic!("{other:?}"),
        }
    }
}

/// The tape as an ordinary binary recording, read back by the replay decoder.
fn binary(tree: &Tree, events: &[Event]) -> crate::tape_binary::Tape {
    let header = crate::launch::Recording {
        arguments: vec![],
        run_identity: [1; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree: tree.clone(),
        events: vec![],
    };
    let mut bytes = Vec::new();
    let mut writer = crate::tape_binary::Writer::new(&mut bytes, &header).unwrap();
    for event in events {
        writer.append(event).unwrap();
    }
    writer.finish().unwrap();
    drop(writer);
    crate::tape_binary::Tape::read(Arc::new(bytes)).unwrap().1
}

fn replay(tree: Tree, events: &[Event], seen: &[Seen]) {
    let mut sim = SimIo::from_binary(tree.clone(), binary(&tree, events));
    for (request, keys, domains, result, identities) in seen {
        assert_eq!(sim.keys(request).as_ref(), Some(keys), "{request:?}");
        assert_eq!(&sim.domains(request), domains, "{request:?}");
        assert_eq!(&sim.call(request.clone()), result, "{request:?}");
        let replayed: Vec<_> = words(result)
            .into_iter()
            .map(|word| sim.identity(word))
            .collect();
        assert_eq!(&replayed, identities, "{request:?}");
        // After the call the consumed event still answers for its request.
        assert_eq!(sim.keys(request).as_ref(), Some(keys), "{request:?}");
    }
    sim.finish().unwrap();
}

fn print(stdout: u64) -> Request {
    Request::Print {
        stdout,
        bytes: b"x\n".to_vec(),
    }
}

#[test]
fn stdio_aliases_replay_their_one_recorded_sink_identity() {
    let shared = Arc::new(Mutex::new(Vec::<u8>::new()));
    let (_temp, mut live) = Live::new(&[], |_, streams| {
        streams.output = crate::host::SendSink::shared(shared.clone());
        streams.stderr = crate::host::SendSink::shared(shared.clone());
    });
    let stdout = live.handle(Request::Stdout { io: 1 });
    let stderr = live.handle(Request::Stderr { io: 1 });
    live.call(print(stdout));
    live.call(Request::PrintStderr {
        stderr,
        bytes: b"e\n".to_vec(),
    });
    let (out, err) = (
        live.io.identity(stdout).unwrap(),
        live.io.identity(stderr).unwrap(),
    );
    assert_eq!(out, err);
    assert_eq!(out.1, Kind::Sink { file_backed: false });
    let tape = live.io.tape();
    assert_eq!(shared.lock().unwrap().as_slice(), b"x\ne\n");
    assert!(tape.iter().all(|event| event.resources.is_some()));
    replay(Tree::new(BTreeMap::new()).unwrap(), &tape, &live.seen);
}

#[test]
fn an_unknown_writer_replays_its_coupled_file_backed_identity() {
    let (_temp, mut live) = Live::new(&[], |_, streams| {
        streams.output = crate::host::SendSink::new(std::io::sink());
    });
    let stdout = live.handle(Request::Stdout { io: 1 });
    let directory = live.handle(Request::Directory {
        io: 1,
        path: String::new(),
    });
    let file = live.handle(Request::Create {
        directory,
        path: "f".into(),
    });
    live.call(Request::FileWrite {
        directory,
        file,
        bytes: b"abc".to_vec(),
    });
    live.call(print(stdout));
    assert_eq!(
        live.io.identity(stdout),
        Some((COUPLED, Kind::Sink { file_backed: true }))
    );
    // Unknown storage may alias any file: namespace requests also take the coupled key.
    assert!(live.seen[2].1.contains(COUPLED, Mode::Exclusive));
    replay(
        Tree::new(BTreeMap::new()).unwrap(),
        &live.io.tape(),
        &live.seen,
    );
}

#[test]
fn a_sink_sharing_a_file_replays_one_resource_for_both() {
    let (_temp, mut live) = Live::new(&[("a", b"")], |root, streams| {
        streams.output = crate::host::SendSink::file(
            std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("a"))
                .unwrap(),
        );
    });
    let stdout = live.handle(Request::Stdout { io: 1 });
    let directory = live.handle(Request::Directory {
        io: 1,
        path: String::new(),
    });
    let file = live.handle(Request::Open {
        directory,
        path: "a".into(),
    });
    live.call(print(stdout));
    live.call(Request::FileRead {
        directory,
        file,
        max: 16,
    });
    let (sink, opened) = (
        live.io.identity(stdout).unwrap(),
        live.io.identity(file).unwrap(),
    );
    assert_eq!(sink, (sink.0, Kind::Sink { file_backed: true }));
    assert_eq!(opened, (sink.0, Kind::File));
    assert!(live.seen[3].1.contains(NAMESPACE, Mode::Shared));
    let tree = Tree::new(BTreeMap::from([("a".into(), b"".to_vec())])).unwrap();
    replay(tree, &live.io.tape(), &live.seen);
}

/// A recorded file's footprints, with the event index of each request.
fn file_tape() -> (Vec<Event>, Vec<Seen>) {
    let (_temp, mut live) = Live::new(&[], |_, _| {});
    let directory = live.handle(Request::Directory {
        io: 1,
        path: String::new(),
    });
    let file = live.handle(Request::Create {
        directory,
        path: "f".into(),
    });
    live.call(Request::FileWrite {
        directory,
        file,
        bytes: b"abc".to_vec(),
    });
    (live.io.tape(), live.seen)
}

fn refused(events: Vec<Event>, seen: &[Seen], at: usize) {
    let tree = Tree::new(BTreeMap::new()).unwrap();
    let mut sim = SimIo::from_binary(tree.clone(), binary(&tree, &events));
    for (index, (request, ..)) in seen.iter().enumerate() {
        let result = sim.call(request.clone());
        if index == at {
            assert_eq!(result, Err(AdapterError::ReplayMismatch(at)));
            return;
        }
        assert!(result.is_ok(), "{index}: {result:?}");
    }
    panic!("the forged footprint replayed");
}

fn footprint(event: &mut Event) -> &mut Footprint {
    event.resources.as_deref_mut().unwrap()
}

#[test]
fn forged_footprints_are_refused_where_the_graph_contradicts_them() {
    let (events, seen) = file_tape();
    let file = words(&seen[1].3)[0];
    replay(Tree::new(BTreeMap::new()).unwrap(), &events, &seen);

    // A created file claiming another resource leaves its write's key unestablished.
    let mut forged = events.clone();
    let row = footprint(&mut forged[1])
        .identities
        .iter_mut()
        .find(|row| row.0 == file)
        .unwrap();
    row.1 = Key(999);
    refused(forged, &seen, 2);

    // A later event restating the file under another identity.
    let mut forged = events.clone();
    footprint(&mut forged[2])
        .identities
        .push((file, Key(999), Kind::File));
    refused(forged, &seen, 2);

    // A result handle without its identity.
    let mut forged = events.clone();
    footprint(&mut forged[1])
        .identities
        .retain(|row| row.0 != file);
    refused(forged, &seen, 1);

    // A system resource is never a handle identity.
    let mut forged = events.clone();
    footprint(&mut forged[1])
        .identities
        .iter_mut()
        .find(|row| row.0 == file)
        .unwrap()
        .1 = crate::io_keys::ROOT;
    refused(forged, &seen, 1);

    // A request event stripped of its footprint.
    let mut forged = events.clone();
    forged[2].resources = None;
    refused(forged, &seen, 2);
}

#[test]
fn the_footprint_codec_round_trips_and_refuses_non_canonical_sets() {
    let footprint = Footprint {
        keys: KeySet::new(vec![(Key(5), Mode::Exclusive), (NAMESPACE, Mode::Shared)]),
        domains: vec![(Key(6), 4, "ports".into())],
        identities: vec![
            (4, Key(5), Kind::Other),
            (7, Key(8), Kind::Half { stream: Key(5) }),
        ],
    };
    let mut bytes = Vec::new();
    crate::tape_wire::Wire::encode(&footprint, &mut bytes);
    assert_eq!(
        crate::tape_wire::decode::<Footprint>(&bytes).unwrap(),
        footprint
    );
    let mut reversed = Vec::new();
    crate::tape_wire::Wire::encode(
        &vec![(Key(5), Mode::Exclusive), (NAMESPACE, Mode::Shared)],
        &mut reversed,
    );
    assert!(crate::tape_wire::decode::<KeySet>(&reversed).is_err());
    let mut unordered = footprint.clone();
    unordered.identities.reverse();
    let mut bytes = Vec::new();
    crate::tape_wire::Wire::encode(&unordered, &mut bytes);
    assert!(crate::tape_wire::decode::<Footprint>(&bytes).is_err());
    // Inspection text renders events without their footprints.
    let (events, _) = file_tape();
    let text = crate::launch::render_tape(&events).unwrap();
    let parsed = crate::launch::parse_tape(&text).unwrap();
    assert!(parsed.iter().all(|event| event.resources.is_none()));
    let stripped: Vec<_> = events
        .into_iter()
        .map(|event| Event {
            resources: None,
            ..event
        })
        .collect();
    assert_eq!(parsed, stripped);
}

#[test]
fn a_sink_sharing_a_mapped_file_replays_one_resource_for_both() {
    let (_temp, mut live) = Live::new(&[("a", b"")], |root, streams| {
        streams.output = crate::host::SendSink::file(
            std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("a"))
                .unwrap(),
        );
    });
    let stdout = live.handle(Request::Stdout { io: 1 });
    let directory = live.handle(Request::Directory {
        io: 1,
        path: String::new(),
    });
    live.call(print(stdout));
    let mapped = live.handle(Request::Map {
        directory,
        path: "a".into(),
    });
    live.call(Request::MapView { directory, mapped });
    assert_eq!(
        live.io.identity(mapped).unwrap().0,
        live.io.identity(stdout).unwrap().0
    );
    let tree = Tree::new(BTreeMap::from([("a".into(), b"x\n".to_vec())])).unwrap();
    replay(tree, &live.io.tape(), &live.seen);
}
