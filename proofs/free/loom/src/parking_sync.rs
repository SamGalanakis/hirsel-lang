//! Model the kernel's compare-and-sleep atomically against wake, including
//! spurious returns from Condvar. Production parking is included unchanged.
pub(crate) use loom::sync::atomic::{AtomicU32, Ordering};
#[derive(Default)]
pub(crate) struct Wait {
    lock: loom::sync::Mutex<()>,
    bell: loom::sync::Condvar,
}
impl Wait {
    pub(crate) fn wait(&self, word: &AtomicU32, expected: u32) {
        let guard = self.lock.lock().unwrap();
        if word.load(Ordering::Acquire) == expected {
            drop(self.bell.wait(guard).unwrap());
        }
    }
    pub(crate) fn wake(&self, _: &AtomicU32) {
        let _guard = self.lock.lock().unwrap();
        self.bell.notify_all();
    }
}
pub(crate) static SPINS: usize = 0;
pub(crate) fn spin() {}
pub(crate) fn fence() {
    loom::sync::atomic::fence(Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) use loom::thread::spawn;

#[cfg(test)]
pub(crate) use crate::sync::{Arc, model};
