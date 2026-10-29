use super::*;
use crate::sync::{AtomicU32, Ordering, model};

struct Lease(Arc<AtomicU32>);
impl Drop for Lease {
    fn drop(&mut self) {
        assert_eq!(self.0.swap(1, Ordering::AcqRel), 0);
    }
}
struct Image(Arc<AtomicU32>);
impl Drop for Image {
    fn drop(&mut self) {
        assert_eq!(
            self.0.load(Ordering::Acquire),
            0,
            "image outlived its lease"
        );
    }
}

#[test]
fn image_destruction_precedes_the_last_claim_lease_for_every_frame_phase() {
    model(|| {
        for (phase, retain_notice) in
            (0..3).flat_map(|phase| [false, true].map(|keep| (phase, keep)))
        {
            let dropped = Arc::new(AtomicU32::new(0));
            let (directory, mut saved) = Directory::admit(vec![Binding {
                key: Key {
                    slot: 7,
                    generation: 1,
                },
                path: vec![2, 3].into_boxed_slice(),
                lease: Lease(dropped.clone()),
                image: Image(dropped.clone()),
            }])
            .unwrap();
            let notice = retain_notice.then(|| {
                directory
                    .notice(
                        0,
                        Key {
                            slot: 7,
                            generation: 1,
                        },
                    )
                    .unwrap()
            });
            drop(directory);
            let saved = saved.pop().unwrap();
            match phase {
                0 => drop(saved),
                1 => drop(saved.start()),
                2 => assert!(saved.start().save(false).is_none()),
                _ => unreachable!(),
            }
            assert_eq!(dropped.load(Ordering::Acquire), u32::from(!retain_notice));
            drop(notice);
            assert_eq!(dropped.load(Ordering::Acquire), 1);
        }
    });
}

#[test]
fn duplicate_identity_or_path_refuses_the_whole_directory() {
    model(|| {
        let make = |slot, path| Binding {
            key: Key {
                slot,
                generation: 1,
            },
            path: vec![path].into_boxed_slice(),
            lease: (),
            image: (),
        };
        assert!(matches!(
            Directory::admit(vec![make(1, 1), make(1, 2)]),
            Err(AdmissionError::DuplicateKey)
        ));
        assert!(matches!(
            Directory::admit(vec![make(1, 1), make(2, 1)]),
            Err(AdmissionError::DuplicatePath)
        ));
        for duplicate_path in [false, true] {
            let dropped = [Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0))];
            let bindings = dropped
                .iter()
                .enumerate()
                .map(|(i, state)| Binding {
                    image: Image(state.clone()),
                    key: Key {
                        slot: if duplicate_path { i as u32 } else { 0 },
                        generation: 1,
                    },
                    path: vec![if duplicate_path { 0 } else { i as u32 }].into_boxed_slice(),
                    lease: Lease(state.clone()),
                })
                .collect();
            assert!(Directory::admit(bindings).is_err());
            for state in dropped {
                assert_eq!(
                    state.load(Ordering::Acquire),
                    1,
                    "refusal released every image and lease"
                );
            }
        }
        let (empty, saved) = Directory::<(), ()>::admit(vec![]).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(saved.is_empty());
    });
}
