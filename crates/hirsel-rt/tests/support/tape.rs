//! Round trips through the production tape codec. The text inspection form
//! deliberately omits resource footprints and cannot stand in for this boundary.
use hirsel_rt::io::{Event, Tree};
use hirsel_rt::launch::{
    Recording, RecordingWriter, parse_recording, parse_recording_bytes, render_recording,
};

pub fn recording(record: &Recording) -> Recording {
    let mut bytes = vec![];
    let header = Recording {
        events: vec![],
        ..record.clone()
    };
    // Inspection still round trips every field it represents. Check those
    // separately, then require the binary round trip to preserve the full event.
    let inspected = parse_recording(&render_recording(record).unwrap()).unwrap();
    assert_eq!(inspected.events.len(), record.events.len());
    for (actual, expected) in inspected.events.iter().zip(&record.events) {
        assert_eq!(
            (&actual.request, &actual.completion, &actual.fault),
            (&expected.request, &expected.completion, &expected.fault)
        );
        assert!(actual.resources.is_none());
    }
    assert_eq!(
        Recording {
            events: vec![],
            ..inspected
        },
        header
    );
    let mut writer = RecordingWriter::new(&mut bytes, &header).unwrap();
    for event in &record.events {
        writer.append(event).unwrap();
    }
    writer.finish().unwrap();
    drop(writer);
    parse_recording_bytes(&bytes).unwrap()
}

pub fn events(events: &[Event]) -> Vec<Event> {
    recording(&Recording {
        arguments: vec![],
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree: Tree::new(Default::default()).unwrap(),
        events: events.to_vec(),
    })
    .events
}
