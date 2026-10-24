use super::*;

fn shape() -> ResumeShape {
    ResumeShape {
        plan: [7; 32],
        bytes: vec![24, 16],
    }
}

#[test]
fn a_started_continuation_never_reenters_its_entry() {
    let mut c = Continuation::default();
    c.start();
    let sp = std::ptr::dangling_mut::<u8>();
    c.save(SavedRepr::Fiber(SavedFiber { sp }));
    assert_eq!(c.activate(), SavedRepr::Fiber(SavedFiber { sp }));
    for restart in [true, false] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut copy = Continuation::Active;
            if restart {
                copy.start()
            } else {
                drop(copy.activate())
            }
        }));
        assert!(result.is_err());
    }
    let frame = OwnedResumeFrame::save(&shape(), 1, &[1, 2]).unwrap();
    c.save(SavedRepr::Resumable(frame));
    let SavedRepr::Resumable(frame) = c.activate() else {
        panic!("representation changed across save")
    };
    assert_eq!(frame.state, 1);
    c.complete();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.start())).is_err());
}

#[test]
fn compact_frames_materialize_exact_state_images_and_refuse_foreign_shapes() {
    let s = shape();
    let frame = OwnedResumeFrame::save(&s, 2, &[9]).unwrap();
    assert_eq!(&*frame.image, &[2, 9]);
    assert!(frame.check(&s));
    assert!(OwnedResumeFrame::save(&s, 1, &[9]).is_none());
    assert!(OwnedResumeFrame::save(&s, 0, &[]).is_none());
    assert!(OwnedResumeFrame::save(&s, 3, &[]).is_none());
    let mut other = s.clone();
    other.plan[0] = 8;
    assert!(!frame.check(&other));
    let mut wrong = OwnedResumeFrame::save(&s, 2, &[9]).unwrap();
    wrong.state = 1;
    assert!(!wrong.check(&s));
}

#[test]
fn custody_adopts_once_and_withdrawal_is_terminal() {
    let save = PrivateSave {
        pc: SavedFiber {
            sp: std::ptr::dangling_mut(),
        },
        generation: 3,
        fuel: 11,
        descriptor: None,
    };
    let mut custody = Custody::Private { generation: 3 };
    assert_eq!(custody.adopt(save), save);
    assert_eq!(custody, Custody::Adopted(save));
    assert!(!custody.withdraw(3));
    assert!(
        std::panic::catch_unwind(move || {
            let mut c = custody;
            c.adopt(save)
        })
        .is_err()
    );
    let mut withdrawn = Custody::Private { generation: 3 };
    assert!(withdrawn.withdraw(3));
    assert!(!withdrawn.withdraw(3));
    assert!(
        std::panic::catch_unwind(move || {
            let mut c = withdrawn;
            c.adopt(save)
        })
        .is_err()
    );
    let mut logical = Custody::Logical;
    assert!(!logical.withdraw(9));
    assert!(
        std::panic::catch_unwind(move || {
            let mut c = logical;
            c.adopt(save)
        })
        .is_err()
    );
}
