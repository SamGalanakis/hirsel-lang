use super::*;

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
    let mut frame = OwnedResumeFrame::new(40);
    unsafe { frame.as_mut_ptr().cast::<u32>().write(1) };
    c.save(SavedRepr::Resumable(frame));
    let SavedRepr::Resumable(frame) = c.activate() else {
        panic!("representation changed across save")
    };
    assert_eq!(frame.state(), 1);
    c.complete();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.start())).is_err());
}

#[test]
fn compact_frames_start_at_entry_with_aligned_whole_images() {
    for (bytes, rounded) in [(8, 16), (40, 48), (184, 192)] {
        let mut frame = OwnedResumeFrame::new(bytes);
        assert_eq!((frame.state(), frame.bytes()), (0, rounded));
        assert_eq!(frame.as_mut_ptr() as usize % 16, 0);
    }
}

#[test]
fn custody_adopts_once_and_withdrawal_is_terminal() {
    let save = PrivateSave {
        pc: SavedPc::Fiber(SavedFiber {
            sp: std::ptr::dangling_mut(),
        }),
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

#[test]
fn compact_private_saves_adopt_their_resume_state_once_never_the_entry() {
    let mut frame = OwnedResumeFrame::new(40);
    assert!(std::panic::catch_unwind(|| SavedPc::compact(&OwnedResumeFrame::new(40))).is_err());
    // The frame header names the state its boundary resumes at.
    unsafe { frame.as_mut_ptr().cast::<u32>().write(2) };
    let pc = SavedPc::compact(&frame);
    assert_eq!((pc, pc.sp()), (SavedPc::Compact { state: 2 }, None));
    let save = PrivateSave {
        pc,
        generation: 5,
        fuel: 7,
        descriptor: None,
    };
    let mut custody = Custody::Private { generation: 5 };
    assert_eq!(custody.adopt(save), save);
    assert_eq!(custody, Custody::Adopted(save));
    assert!(
        std::panic::catch_unwind(move || {
            let mut c = custody;
            c.adopt(save)
        })
        .is_err()
    );
    let mut stale = Custody::Private { generation: 6 };
    assert!(std::panic::catch_unwind(move || stale.adopt(save)).is_err());
}
