//! The durable call journal keeps every acknowledged record and drops a torn tail.
use hirsel_remote::journal::{DirJournal, Entry, Journal, StreamKey};
use hirsel_run_schema::CallOutcome;

fn stream() -> StreamKey {
    StreamKey {
        caller: "app".into(),
        stream: [5; 32],
    }
}

#[test]
fn a_dir_journal_recovers_its_records_and_drops_a_torn_tail() {
    let dir = tempfile::tempdir().unwrap();
    let entries = vec![
        Entry::Stream {
            stream: stream(),
            instance: [1; 16],
        },
        Entry::Admitted {
            stream: stream(),
            position: 1,
            key: [2; 32],
            digest: [3; 32],
        },
        Entry::Decided {
            stream: stream(),
            position: 1,
            key: [2; 32],
            digest: [3; 32],
            end: CallOutcome::Failed(b"no".to_vec()),
            callee: Some([4; 16]),
        },
        Entry::Acknowledged {
            stream: stream(),
            position: 1,
        },
    ];
    let incarnation = {
        let mut journal = DirJournal::open(dir.path(), [9; 16]).unwrap();
        for entry in &entries {
            journal.append(entry).unwrap();
        }
        journal.incarnation()
    };
    assert_eq!(incarnation, [9; 16]);
    // A torn record at the end: its length promises more than was written.
    let path = dir.path().join("calls.journal");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 10]);
    std::fs::write(&path, &bytes).unwrap();
    let mut journal = DirJournal::open(dir.path(), [8; 16]).unwrap();
    assert_eq!(journal.incarnation(), [9; 16], "the incarnation survives");
    assert_eq!(journal.recover().unwrap(), entries);
    // Appends follow the last intact record.
    let more = Entry::Acknowledged {
        stream: stream(),
        position: 2,
    };
    journal.append(&more).unwrap();
    let mut reopened = DirJournal::open(dir.path(), [7; 16]).unwrap();
    let mut expected = entries.clone();
    expected.push(more);
    assert_eq!(reopened.recover().unwrap(), expected);
}

#[test]
fn a_corrupt_record_ends_recovery() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut journal = DirJournal::open(dir.path(), [9; 16]).unwrap();
        for position in 1..=3 {
            journal
                .append(&Entry::Acknowledged {
                    stream: stream(),
                    position,
                })
                .unwrap();
        }
    }
    let path = dir.path().join("calls.journal");
    let mut bytes = std::fs::read(&path).unwrap();
    // Flip a byte in the second record's body.
    let second = 24 + (36 + bytes[24] as usize) + 40;
    bytes[second] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    let mut journal = DirJournal::open(dir.path(), [8; 16]).unwrap();
    assert_eq!(
        journal.recover().unwrap(),
        vec![Entry::Acknowledged {
            stream: stream(),
            position: 1
        }]
    );
}
