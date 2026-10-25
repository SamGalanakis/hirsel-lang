//! The runtime's cancellation flag and role-owned epoch cache, from the same
//! source, under competing writers and a reader changing role owners.

hirsel_guard::install!();
use hirsel_free_loom::cancel::{Cache, Epoch, Flag};
use loom::sync::{Arc, Mutex};

#[test]
fn competing_causes_have_one_winner_and_cannot_overwrite_it() {
    hirsel_free_loom::sync::model(|| {
        let flag = Arc::new(Flag::default());
        let a = loom::thread::spawn({
            let flag = flag.clone();
            move || flag.cancel(0)
        });
        let b = loom::thread::spawn({
            let flag = flag.clone();
            move || flag.cancel(2)
        });
        let a = a.join().unwrap();
        let b = b.join().unwrap();
        assert_ne!(a, b);
        assert_eq!(flag.cause(), Some(if a { 0 } else { 2 }));
        assert!(!flag.cancel(3));
        assert_eq!(flag.cause(), Some(if a { 0 } else { 2 }));
    });
}

#[test]
fn changing_role_owners_cannot_reuse_an_observation_before_cancellation() {
    hirsel_free_loom::sync::model(|| {
        let role = Arc::new(Mutex::new((
            Flag::default(),
            Epoch::default(),
            Cache::default(),
        )));
        {
            let state = role.lock().unwrap();
            assert_eq!(state.2.get(state.1.load(), || state.0.cause()), None);
        }
        let writer = loom::thread::spawn({
            let role = role.clone();
            move || {
                let state = role.lock().unwrap();
                assert!(state.0.cancel(1));
                state.1.changed();
            }
        });
        {
            let state = role.lock().unwrap();
            let cached = state.2.get(state.1.load(), || state.0.cause());
            assert_eq!(cached, state.0.cause());
        }
        writer.join().unwrap();
        let state = role.lock().unwrap();
        assert_eq!(state.2.get(state.1.load(), || state.0.cause()), Some(1));
    });
}
